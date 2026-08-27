use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use tauri::{ipc::Channel, State};
use tokio::time::{interval, timeout, MissedTickBehavior};
use uuid::Uuid;

use crate::{
    app::{AppState, CommandError},
    artifact_trust::{
        create_private_directory, ensure_private_directory, ensure_private_regular_file,
    },
    pi::{PiEvent, PiProcess, PiSupportLaunchConfig},
    settings::{
        catalog_allows_authorization, provider_credential_from_environment,
        provider_credential_generation, provider_options, ConfiguredModel, ModelCatalogView,
        ModelProviderOption, ModelRunControl,
    },
    support_coordinator::ProviderSupportLease,
    web::{SearchCancel, SearchHits, SearchProviderId, WebError, WebSearchQuery, WebSource},
};

const PROVIDER_PROTOCOL: &str = "guruterminal-provider/1";
const MAX_RESULT_BYTES: u64 = 512 * 1024;
const MAX_API_KEY_BYTES: usize = 8 * 1024;
const SUPPORT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const PROVIDER_MODEL_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_PROVIDER_MODEL_CACHE_ENTRIES: usize = 32;

#[derive(Clone)]
pub(crate) struct ProviderModelDiscoveryCache {
    inner: Arc<Mutex<VecDeque<ProviderModelCacheEntry>>>,
    ttl: Duration,
    max_entries: usize,
}

struct ProviderModelCacheEntry {
    provider: String,
    credential_generation: String,
    refreshed_at: Instant,
}

impl Default for ProviderModelDiscoveryCache {
    fn default() -> Self {
        Self::with_limits(PROVIDER_MODEL_CACHE_TTL, MAX_PROVIDER_MODEL_CACHE_ENTRIES)
    }
}

impl ProviderModelDiscoveryCache {
    fn with_limits(ttl: Duration, max_entries: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::new())),
            ttl,
            max_entries: max_entries.max(1),
        }
    }

    fn is_fresh(&self, provider: &str, credential_generation: &str) -> bool {
        self.is_fresh_at(provider, credential_generation, Instant::now())
    }

    fn is_fresh_at(&self, provider: &str, credential_generation: &str, now: Instant) -> bool {
        let mut entries = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.retain(|entry| now.saturating_duration_since(entry.refreshed_at) < self.ttl);
        let Some(index) = entries.iter().position(|entry| entry.provider == provider) else {
            return false;
        };
        let entry = entries
            .remove(index)
            .expect("provider model cache index disappeared");
        if entry.credential_generation != credential_generation {
            return false;
        }
        entries.push_back(entry);
        true
    }

    fn record(&self, provider: &str, credential_generation: String) {
        self.record_at(provider, credential_generation, Instant::now());
    }

    fn record_at(&self, provider: &str, credential_generation: String, now: Instant) {
        let mut entries = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.retain(|entry| {
            entry.provider != provider
                && now.saturating_duration_since(entry.refreshed_at) < self.ttl
        });
        while entries.len() >= self.max_entries {
            entries.pop_front();
        }
        entries.push_back(ProviderModelCacheEntry {
            provider: provider.to_owned(),
            credential_generation,
            refreshed_at: now,
        });
    }

    pub(crate) fn invalidate(&self, provider: &str) {
        let mut entries = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.retain(|entry| entry.provider != provider);
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderModelsRequest {
    pub provider: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConnectionRequest {
    pub provider: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfigureRequest {
    pub provider: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_saved_key: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderModelOption {
    pub id: String,
    pub name: String,
    pub reasoning: bool,
    pub context_window: u64,
    pub max_tokens: u64,
    pub input: Vec<String>,
    pub thinking_levels: Vec<String>,
    pub thinking_level_map: std::collections::BTreeMap<String, Option<String>>,
    pub run_controls: Vec<ModelRunControl>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderConnectionEvent {
    OpeningBrowser { message: String },
    Waiting { message: String },
    Connected { message: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderResult {
    protocol: String,
    #[serde(rename = "type")]
    result_type: String,
    provider: String,
    models: Vec<ProviderModelOption>,
}

struct SupportFiles {
    directory: PathBuf,
    result: PathBuf,
    request: Option<PathBuf>,
}

impl Drop for SupportFiles {
    fn drop(&mut self) {
        if let Some(request) = &self.request {
            let _ = std::fs::remove_file(request);
        }
        let _ = std::fs::remove_file(&self.result);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn provider_models(
    request: ProviderModelsRequest,
    state: State<'_, AppState>,
) -> Result<ModelCatalogView, CommandError> {
    validate_provider(&request.provider)?;
    if let Some(catalog) = cached_provider_models(&state, &request.provider)? {
        return Ok(catalog);
    }
    let admission = state.provider_support.try_acquire()?;
    discover_provider_models(&state, &request.provider, &admission).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn provider_configure(
    request: ProviderConfigureRequest,
    state: State<'_, AppState>,
) -> Result<ModelCatalogView, CommandError> {
    let provider = provider_option(&request.provider)?;
    if !provider.api_key {
        return Err(CommandError::invalid(
            "this provider must be connected through its browser sign-in",
        ));
    }
    let api_key = request
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(api_key) = api_key {
        validate_api_key(api_key)?;
    }
    let admission = state.provider_support.try_acquire()?;
    if api_key.is_some() || request.clear_saved_key {
        // Invalidate before mutation: a support command can change its durable
        // credential authority even when its response or shutdown later fails.
        state.provider_model_cache.invalidate(&request.provider);
        let operation = if api_key.is_some() { "set" } else { "clear" };
        let result = run_support_command(
            &state,
            format!(
                "/guruterminal-provider-api-key {} {operation}",
                request.provider
            ),
            false,
            None,
            None,
            api_key.map(str::to_owned),
            &admission,
        )
        .await?;
        if result.result_type != "credential_updated" || result.provider != request.provider {
            return Err(CommandError::internal(
                "Pi returned the wrong credential update result",
            ));
        }
    }
    if request.clear_saved_key && api_key.is_none() {
        return state.model_catalog_view();
    }
    discover_provider_models(&state, &request.provider, &admission).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn provider_connect(
    request: ProviderConnectionRequest,
    on_event: Channel<ProviderConnectionEvent>,
    state: State<'_, AppState>,
) -> Result<ModelCatalogView, CommandError> {
    let provider = provider_option(&request.provider)?;
    if provider.oauth.is_none() {
        return Err(CommandError::invalid(
            "provider does not support OAuth connection",
        ));
    }
    let admission = state.provider_support.try_acquire_oauth()?;

    // OAuth may replace durable authority before the support command reports
    // completion, so stale model discovery must become unreachable first.
    state.provider_model_cache.invalidate(&request.provider);

    let result = run_support_command(
        &state,
        format!("/guruterminal-provider-login {}", request.provider),
        true,
        Some(&on_event),
        None,
        None,
        &admission,
    )
    .await?;
    if result.result_type != "credential_updated" || result.provider != request.provider {
        return Err(CommandError::internal("Pi returned the wrong OAuth result"));
    }
    send_event(
        &on_event,
        ProviderConnectionEvent::Connected {
            message: format!(
                "{} is connected. Pi's available models are ready for Guru runs.",
                provider.label
            ),
        },
    )?;
    discover_provider_models(&state, &request.provider, &admission).await
}

#[tauri::command(rename_all = "snake_case")]
pub fn provider_connect_cancel(state: State<'_, AppState>) {
    state.provider_support.cancel_oauth();
}

#[tauri::command(rename_all = "snake_case")]
pub fn provider_connect_open_browser(state: State<'_, AppState>) -> Result<(), CommandError> {
    let url = state
        .provider_support
        .oauth_authorization_url()
        .ok_or_else(|| CommandError::conflict("no active sign-in page is ready"))?;
    open_authorization_url(&url)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn provider_disconnect(
    request: ProviderConnectionRequest,
    state: State<'_, AppState>,
) -> Result<ModelCatalogView, CommandError> {
    validate_provider(&request.provider)?;
    let admission = state.provider_support.try_acquire()?;
    state.provider_model_cache.invalidate(&request.provider);
    let result = run_support_command(
        &state,
        format!("/guruterminal-provider-logout {}", request.provider),
        false,
        None,
        None,
        None,
        &admission,
    )
    .await?;
    if result.result_type != "credential_updated" || result.provider != request.provider {
        return Err(CommandError::internal(
            "Pi returned the wrong logout result",
        ));
    }
    state.replace_provider_models(&request.provider, Vec::new())?;
    state.model_catalog_view()
}

fn provider_option(provider: &str) -> Result<ModelProviderOption, CommandError> {
    provider_options()
        .into_iter()
        .find(|option| option.id == provider)
        .ok_or_else(|| CommandError::invalid("unsupported Pi model provider"))
}

async fn discover_provider_models(
    state: &AppState,
    provider: &str,
    admission: &ProviderSupportLease,
) -> Result<ModelCatalogView, CommandError> {
    let agent_data_dir = state.pi_agent_data_dir()?;
    let credential_generation = provider_credential_generation(&agent_data_dir, provider)?
        .ok_or_else(|| {
            CommandError::invalid("connect the provider before loading its Pi models")
        })?;
    // Recheck after admission. A preceding request may have completed while
    // this caller was entering the support path.
    if state
        .provider_model_cache
        .is_fresh(provider, &credential_generation)
    {
        return state.model_catalog_view();
    }
    let result = run_support_command(
        state,
        format!("/guruterminal-provider-models {provider}"),
        false,
        None,
        provider_credential_from_environment(provider),
        None,
        admission,
    )
    .await?;
    if result.result_type != "models" || result.provider != provider {
        return Err(CommandError::internal(
            "Pi returned the wrong provider catalog",
        ));
    }
    let models = result
        .models
        .into_iter()
        .map(|model| configured_model(provider, model))
        .collect::<Result<Vec<_>, _>>()?;
    state.replace_provider_models(provider, models)?;
    // Do not cache a catalog if the credential authority rotated while Pi was
    // discovering it. The result is still valid for this response, but the
    // next request must refresh under the new authority.
    if provider_credential_generation(&agent_data_dir, provider)?.as_deref()
        == Some(credential_generation.as_str())
    {
        state
            .provider_model_cache
            .record(provider, credential_generation);
    }
    state.model_catalog_view()
}

fn cached_provider_models(
    state: &AppState,
    provider: &str,
) -> Result<Option<ModelCatalogView>, CommandError> {
    let agent_data_dir = state.pi_agent_data_dir()?;
    let Some(credential_generation) = provider_credential_generation(&agent_data_dir, provider)?
    else {
        state.provider_model_cache.invalidate(provider);
        return Ok(None);
    };
    if state
        .provider_model_cache
        .is_fresh(provider, &credential_generation)
    {
        return state.model_catalog_view().map(Some);
    }
    Ok(None)
}

fn configured_model(
    provider: &str,
    model: ProviderModelOption,
) -> Result<ConfiguredModel, CommandError> {
    let configured = ConfiguredModel {
        id: format!("{provider}/{}", model.id),
        name: model.name,
        provider: provider.to_owned(),
        model: model.id,
        input: model.input,
        reasoning: model.reasoning,
        context_window: model.context_window,
        max_tokens: model.max_tokens,
        thinking_levels: model.thinking_levels,
        thinking_level_map: model.thinking_level_map,
        run_controls: model.run_controls,
    };
    configured.validate()?;
    Ok(configured)
}

fn validate_provider(provider: &str) -> Result<(), CommandError> {
    if provider_options()
        .iter()
        .any(|option| option.id == provider)
    {
        Ok(())
    } else {
        Err(CommandError::invalid("unsupported Pi model provider"))
    }
}

fn validate_api_key(api_key: &str) -> Result<(), CommandError> {
    if api_key.is_empty()
        || api_key.len() > MAX_API_KEY_BYTES
        || api_key.chars().any(char::is_control)
    {
        return Err(CommandError::invalid("API key is invalid"));
    }
    Ok(())
}

async fn run_support_command(
    state: &AppState,
    command: String,
    open_authorization: bool,
    events: Option<&Channel<ProviderConnectionEvent>>,
    provider_credential: Option<(String, String)>,
    mutation_api_key: Option<String>,
    admission: &ProviderSupportLease,
) -> Result<ProviderResult, CommandError> {
    let pi = state
        .artifacts
        .pi
        .as_ref()
        .ok_or_else(|| CommandError::unavailable("pi"))?;
    let files = create_support_files(&state.artifacts.app_data_dir)?;
    let process = PiProcess::spawn_support(PiSupportLaunchConfig {
        executable: pi.executable.clone(),
        runtime_dir: pi.runtime_dir.clone(),
        extension: pi.provider_extension.clone(),
        agent_data_dir: state.pi_agent_data_dir()?,
        private_working_dir: files.directory.clone(),
        lease_dir: state.artifacts.process_lease_dir.clone(),
        result_file: files.result.clone(),
        request_file: None,
        provider_credential,
        mutation_api_key,
    })
    .await
    .map_err(|error| CommandError::internal(error.to_string()))?;
    let mut receiver = process.subscribe();
    let request_id = match process.prompt(&command).await {
        Ok(request_id) => request_id,
        Err(error) => {
            let _ = process.shutdown(Duration::from_secs(1)).await;
            return Err(CommandError::internal(error.to_string()));
        }
    };

    let outcome = timeout(SUPPORT_TIMEOUT, async {
        tokio::select! {
            _ = admission.cancelled() => {
                Err(CommandError::new("cancelled", "Provider sign-in was cancelled."))
            }
            outcome = async {
                loop {
                    match receiver.recv().await {
                        Ok(PiEvent::Rpc { payload }) => {
                            if let Some(event) = provider_event(&payload)? {
                                handle_provider_event(
                                    event,
                                    open_authorization,
                                    admission,
                                    events,
                                )?;
                            }
                            if payload.get("type").and_then(Value::as_str) == Some("response")
                                && payload.get("id").and_then(Value::as_u64) == Some(request_id)
                            {
                                if payload.get("success").and_then(Value::as_bool) == Some(true) {
                                    break Ok(());
                                }
                                break Err(CommandError::internal(
                                    payload
                                        .get("error")
                                        .and_then(Value::as_str)
                                        .unwrap_or("Pi provider setup failed"),
                                ));
                            }
                        }
                        Ok(PiEvent::ProtocolError { message }) => {
                            break Err(CommandError::internal(message));
                        }
                        Ok(PiEvent::Exited) => {
                            break Err(CommandError::internal("Pi provider setup stopped early"));
                        }
                        Err(_) => break Err(CommandError::internal("Pi provider setup channel closed")),
                    }
                }
            } => outcome,
        }
    })
    .await
    .unwrap_or_else(|_| Err(CommandError::internal("Pi provider setup timed out")));

    let shutdown = process
        .shutdown(Duration::from_secs(1))
        .await
        .map_err(|error| CommandError::internal(error.to_string()));
    outcome?;
    shutdown?;
    read_support_result(&files.result)
}

fn create_support_files(app_data_dir: &Path) -> Result<SupportFiles, CommandError> {
    let root = app_data_dir.join("provider-support");
    ensure_private_directory(&root).map_err(|error| CommandError::internal(error.to_string()))?;
    let directory = root.join(Uuid::new_v4().simple().to_string());
    create_private_directory(&directory)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    let result = directory.join("result.json");
    ensure_private_regular_file(&result)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    Ok(SupportFiles {
        directory,
        result,
        request: None,
    })
}

fn create_search_support_files(app_data_dir: &Path) -> Result<SupportFiles, CommandError> {
    let mut files = create_support_files(app_data_dir)?;
    let request = files.directory.join("request.json");
    ensure_private_regular_file(&request)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    files.request = Some(request);
    Ok(files)
}

fn read_support_result(path: &Path) -> Result<ProviderResult, CommandError> {
    ensure_private_regular_file(path).map_err(|error| CommandError::internal(error.to_string()))?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    if file
        .metadata()
        .map_err(|error| CommandError::internal(error.to_string()))?
        .len()
        > MAX_RESULT_BYTES
    {
        return Err(CommandError::internal("Pi provider result is too large"));
    }
    let mut encoded = Vec::new();
    file.read_to_end(&mut encoded)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    let result: ProviderResult = serde_json::from_slice(&encoded)
        .map_err(|_| CommandError::internal("Pi provider result is invalid"))?;
    if result.protocol != PROVIDER_PROTOCOL {
        return Err(CommandError::internal("Pi provider protocol is invalid"));
    }
    Ok(result)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeSearchFileResult {
    protocol: String,
    #[serde(rename = "type")]
    result_type: String,
    provider: String,
    status: String,
    #[serde(default)]
    sources: Vec<NativeSearchSource>,
    model: Option<String>,
    #[serde(rename = "requestId")]
    #[allow(dead_code)]
    request_id: Option<String>,
    usage: Option<NativeSearchUsage>,
    #[serde(rename = "searchRequestCount")]
    search_request_count: Option<u32>,
    error_kind: Option<String>,
    retry_after_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeSearchSource {
    title: String,
    url: String,
    snippet: Option<String>,
    #[serde(rename = "publishedAt")]
    published_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeSearchUsage {
    #[serde(rename = "inputTokens")]
    #[allow(dead_code)]
    input_tokens: Option<u32>,
    #[serde(rename = "outputTokens")]
    #[allow(dead_code)]
    output_tokens: Option<u32>,
    #[serde(rename = "totalTokens")]
    #[allow(dead_code)]
    total_tokens: Option<u32>,
    #[serde(rename = "searchRequests")]
    search_requests: Option<u32>,
}

fn write_search_request(
    path: &Path,
    provider: SearchProviderId,
    query: &WebSearchQuery,
) -> Result<(), WebError> {
    let body = serde_json::json!({
        "protocol": PROVIDER_PROTOCOL,
        "type": "search",
        "provider": provider.as_str(),
        "query": query.query,
        "limit": query.limit,
        "recency": query.recency.map(|value| match value {
            crate::web::WebRecency::Day => "day",
            crate::web::WebRecency::Week => "week",
            crate::web::WebRecency::Month => "month",
            crate::web::WebRecency::Year => "year",
        }),
        "include_domains": query.include_domains,
        "exclude_domains": query.exclude_domains,
    });
    let encoded = serde_json::to_vec(&body).map_err(|_| WebError::InvalidProviderResponse)?;
    if encoded.len() as u64 > MAX_RESULT_BYTES {
        return Err(WebError::InvalidProviderResponse);
    }
    std::fs::write(path, encoded).map_err(|_| WebError::Provider)?;
    Ok(())
}

fn read_native_search_result(path: &Path) -> Result<NativeSearchFileResult, WebError> {
    let encoded = std::fs::read(path).map_err(|_| {
        WebError::ProviderMessage("web provider result file was unavailable".into())
    })?;
    if encoded.len() as u64 > MAX_RESULT_BYTES {
        return Err(WebError::ProviderMessage(
            "web provider result exceeded its protocol size limit".into(),
        ));
    }
    let result: NativeSearchFileResult = serde_json::from_slice(&encoded).map_err(|error| {
        WebError::ProviderMessage(format!(
            "web provider result {}",
            native_search_schema_failure(&encoded, &error)
        ))
    })?;
    if result.protocol != PROVIDER_PROTOCOL || result.result_type != "search" {
        return Err(WebError::ProviderMessage(
            "web provider result used an unsupported protocol".into(),
        ));
    }
    Ok(result)
}

fn native_search_result_to_hits(
    provider: SearchProviderId,
    result: NativeSearchFileResult,
) -> Result<SearchHits, WebError> {
    if result.protocol != PROVIDER_PROTOCOL || result.result_type != "search" {
        return Err(WebError::InvalidProviderResponse);
    }
    if result.provider != provider.as_str() {
        return Err(WebError::ProviderMessage(
            "web provider result named a different provider".into(),
        ));
    }
    if result.status != "ok" {
        return Err(native_search_error(&result));
    }
    let search_request_count = result.search_request_count.or(result
        .usage
        .as_ref()
        .and_then(|usage| usage.search_requests));
    let sources = result
        .sources
        .into_iter()
        .map(|source| {
            crate::web::web_source(
                if source.title.trim().is_empty() {
                    "Untitled source".into()
                } else {
                    source.title
                },
                source.url,
                source.snippet.unwrap_or_default(),
                source.published_at,
            )
        })
        .collect::<Vec<WebSource>>();
    Ok(SearchHits {
        sources,
        model: result.model,
        search_request_count,
        retry_after: None,
    })
}

fn native_search_schema_failure(encoded: &[u8], error: &serde_json::Error) -> &'static str {
    if encoded.is_empty() {
        return "file was empty";
    }
    if error.is_eof() {
        return "JSON ended before the result was complete";
    }
    if std::str::from_utf8(encoded).is_err() {
        return "was not UTF-8 protocol JSON";
    }
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(encoded) else {
        return "was not valid protocol JSON";
    };
    let root_fields = [
        "protocol",
        "type",
        "provider",
        "status",
        "sources",
        "model",
        "requestId",
        "usage",
        "searchRequestCount",
        "error_kind",
        "retry_after_ms",
    ];
    if root.keys().any(|key| !root_fields.contains(&key.as_str())) {
        return "contained an unexpected top-level field";
    }
    if root
        .get("sources")
        .and_then(Value::as_array)
        .is_some_and(|sources| {
            sources.iter().any(|source| {
                source.as_object().is_some_and(|source| {
                    source.keys().any(|key| {
                        !["title", "url", "snippet", "publishedAt"].contains(&key.as_str())
                    })
                })
            })
        })
    {
        return "contained an unexpected source field";
    }
    if root
        .get("usage")
        .and_then(Value::as_object)
        .is_some_and(|usage| {
            usage.keys().any(|key| {
                ![
                    "inputTokens",
                    "outputTokens",
                    "totalTokens",
                    "searchRequests",
                ]
                .contains(&key.as_str())
            })
        })
    {
        return "contained an unexpected usage field";
    }
    let detail = error.to_string();
    if detail.contains("missing field") {
        "was missing a required protocol field"
    } else if detail.contains("invalid type") {
        "contained an invalid protocol field type"
    } else if detail.contains("invalid value") {
        "contained an invalid protocol field value"
    } else {
        "did not match its protocol schema"
    }
}

fn native_search_error(result: &NativeSearchFileResult) -> WebError {
    match result.error_kind.as_deref() {
        Some("unavailable") => WebError::ProviderUnavailable,
        Some("rate_limited") => WebError::RateLimited {
            retry_after: result.retry_after_ms.map(Duration::from_millis),
        },
        Some("timeout") => WebError::Timeout,
        Some("cancelled") => WebError::Cancelled,
        Some("malformed") => {
            WebError::ProviderMessage("web provider returned malformed search data".into())
        }
        Some("no_search_tool") => {
            WebError::ProviderMessage("web provider completed without running hosted search".into())
        }
        Some("transport") => WebError::Transport,
        _ => WebError::Provider,
    }
}

fn search_support_result_ready(prompt_succeeded: bool, result: &Path) -> Result<bool, WebError> {
    if !prompt_succeeded {
        return Ok(false);
    }
    let encoded = std::fs::read(result).map_err(|_| WebError::Provider)?;
    if encoded.is_empty() {
        return Ok(false);
    }
    let Ok(result) = serde_json::from_slice::<NativeSearchFileResult>(&encoded) else {
        return Ok(false);
    };
    Ok(result.protocol == PROVIDER_PROTOCOL && result.result_type == "search")
}

pub async fn run_native_web_search(
    state: &AppState,
    provider: SearchProviderId,
    query: &WebSearchQuery,
    remaining: Duration,
    cancel: &SearchCancel,
) -> Result<SearchHits, WebError> {
    if !provider.is_native() {
        return Err(WebError::ProviderUnavailable);
    }
    if cancel.is_cancelled() {
        return Err(WebError::Cancelled);
    }
    let _admission = state
        .provider_support
        .try_acquire()
        .map_err(|_| WebError::ProviderUnavailable)?;
    let pi = state
        .artifacts
        .pi
        .as_ref()
        .ok_or(WebError::ProviderUnavailable)?;
    let files = create_search_support_files(&state.artifacts.app_data_dir)
        .map_err(|_| WebError::Provider)?;
    let request = files.request.as_ref().ok_or(WebError::Provider)?;
    write_search_request(request, provider, query)?;
    let process = PiProcess::spawn_support(PiSupportLaunchConfig {
        executable: pi.executable.clone(),
        runtime_dir: pi.runtime_dir.clone(),
        extension: pi.provider_extension.clone(),
        agent_data_dir: state.pi_agent_data_dir().map_err(|_| WebError::Provider)?,
        private_working_dir: files.directory.clone(),
        lease_dir: state.artifacts.process_lease_dir.clone(),
        result_file: files.result.clone(),
        request_file: files.request.clone(),
        provider_credential: None,
        mutation_api_key: None,
    })
    .await
    .map_err(|_| WebError::Provider)?;
    let mut receiver = process.subscribe();
    let request_id = match process.prompt("/guruterminal-provider-search").await {
        Ok(request_id) => request_id,
        Err(_) => {
            let _ = process.shutdown(Duration::from_secs(1)).await;
            return Err(WebError::Provider);
        }
    };
    let outcome = timeout(remaining, async {
        let mut prompt_succeeded = false;
        let mut result_poll = interval(Duration::from_millis(10));
        result_poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = result_poll.tick() => {
                    if cancel.is_cancelled() {
                        break Err(WebError::Cancelled);
                    }
                    if search_support_result_ready(prompt_succeeded, &files.result)? {
                        break Ok(());
                    }
                }
                event = receiver.recv() => match event {
                    Ok(PiEvent::Rpc { payload }) => {
                        if payload.get("type").and_then(Value::as_str) == Some("response")
                            && payload.get("id").and_then(Value::as_u64) == Some(request_id)
                        {
                            if payload.get("success").and_then(Value::as_bool) == Some(true) {
                                // Pi acknowledges the slash-command prompt before an async
                                // extension handler finishes. Do not kill the support process
                                // until its fsynced result has become observable.
                                prompt_succeeded = true;
                            } else {
                                break Err(WebError::Provider);
                            }
                        }
                    }
                    Ok(PiEvent::Exited) if search_support_result_ready(prompt_succeeded, &files.result)? => {
                        break Ok(());
                    }
                    Ok(PiEvent::ProtocolError { .. }) | Ok(PiEvent::Exited) | Err(_) => {
                        break Err(WebError::Provider);
                    }
                },
            }
        }
    })
    .await
    .unwrap_or(Err(WebError::Timeout));
    let _ = process.shutdown(Duration::from_secs(1)).await;
    if let Err(WebError::Cancelled) = outcome {
        return Err(WebError::Cancelled);
    }
    outcome?;
    native_search_result_to_hits(provider, read_native_search_result(&files.result)?)
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ExtensionProviderEvent {
    AuthorizationUrl {
        url: String,
        #[serde(default)]
        instructions: Option<String>,
    },
    Waiting {
        message: String,
    },
    Connected {
        message: String,
    },
    Progress {
        message: String,
    },
}

fn provider_event(payload: &Value) -> Result<Option<ExtensionProviderEvent>, CommandError> {
    if payload.get("type").and_then(Value::as_str) != Some("extension_ui_request")
        || payload.get("method").and_then(Value::as_str) != Some("notify")
    {
        return Ok(None);
    }
    let Some(message) = payload.get("message").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(encoded) = message.strip_prefix(&format!("{PROVIDER_PROTOCOL}:")) else {
        return Ok(None);
    };
    serde_json::from_str(encoded)
        .map(Some)
        .map_err(|_| CommandError::internal("Pi provider event is invalid"))
}

fn handle_provider_event(
    event: ExtensionProviderEvent,
    open_authorization: bool,
    admission: &ProviderSupportLease,
    channel: Option<&Channel<ProviderConnectionEvent>>,
) -> Result<(), CommandError> {
    match event {
        ExtensionProviderEvent::AuthorizationUrl { url, instructions } => {
            let _ = instructions;
            if !open_authorization {
                return Err(CommandError::internal(
                    "unexpected OAuth authorization event",
                ));
            }
            let channel =
                channel.ok_or_else(|| CommandError::internal("OAuth event channel is missing"))?;
            validate_authorization_url(&url)?;
            admission.set_oauth_authorization_url(url.clone())?;
            open_authorization_url(&url)?;
            send_event(
                channel,
                ProviderConnectionEvent::OpeningBrowser {
                    message: "Continue in your browser to finish signing in.".into(),
                },
            )
        }
        ExtensionProviderEvent::Waiting { message }
        | ExtensionProviderEvent::Progress { message } => {
            if let Some(channel) = channel {
                send_event(channel, ProviderConnectionEvent::Waiting { message })?;
            }
            Ok(())
        }
        ExtensionProviderEvent::Connected { message } => {
            if let Some(channel) = channel {
                send_event(channel, ProviderConnectionEvent::Connected { message })?;
            }
            Ok(())
        }
    }
}

fn open_authorization_url(url: &str) -> Result<(), CommandError> {
    validate_authorization_url(url)?;
    crate::external_browser::open(url).map_err(|error| {
        CommandError::internal(format!(
            "Provider sign-in could not open the trusted authorization page: {error}"
        ))
    })
}

fn untrusted_authorization_url() -> CommandError {
    CommandError::internal("Pi returned an untrusted authorization URL")
}

fn validate_authorization_url(url: &str) -> Result<(), CommandError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| CommandError::internal("Pi returned an invalid authorization URL"))?;
    let host = parsed.host_str().ok_or_else(untrusted_authorization_url)?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.fragment().is_some()
        || !catalog_allows_authorization(host, parsed.path())
    {
        return Err(untrusted_authorization_url());
    }
    Ok(())
}

fn send_event(
    channel: &Channel<ProviderConnectionEvent>,
    event: ProviderConnectionEvent,
) -> Result<(), CommandError> {
    channel
        .send(event)
        .map_err(|_| CommandError::internal("provider setup event delivery failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_urls_are_locked_to_cataloged_oauth_hosts() {
        assert!(validate_authorization_url(
            "https://auth.openai.com/oauth/authorize?client_id=test"
        )
        .is_ok());
        assert!(
            validate_authorization_url("https://claude.ai/oauth/authorize?client_id=test").is_ok()
        );
        assert!(validate_authorization_url(
            "https://openrouter.ai/auth?callback_url=http://127.0.0.1/cb"
        )
        .is_ok());
        assert!(
            validate_authorization_url("https://accounts.x.ai/oauth2/device?user_code=ABCD")
                .is_ok()
        );
        assert!(validate_authorization_url("https://auth.x.ai/activate?user_code=ABCD").is_err());
        assert!(validate_authorization_url("https://example.com/oauth/authorize").is_err());
        assert!(validate_authorization_url("http://auth.openai.com/oauth/authorize").is_err());
        assert!(validate_authorization_url("https://auth.openai.com/other").is_err());
        assert!(
            validate_authorization_url("https://user@auth.openai.com/oauth/authorize").is_err()
        );
        assert!(validate_authorization_url("https://auth.openai.com:444/oauth/authorize").is_err());
        assert!(
            validate_authorization_url("https://accounts.x.ai.evil.example/oauth2/device").is_err()
        );
        assert!(validate_authorization_url("https://claude.ai/oauth/authorize/extra").is_err());
    }

    #[test]
    fn provider_mutation_inputs_and_results_never_accept_secret_payloads() {
        assert!(validate_api_key("valid-api-key").is_ok());
        assert!(validate_api_key("").is_err());
        assert!(validate_api_key("line\nbreak").is_err());
        assert!(validate_api_key(&"x".repeat(MAX_API_KEY_BYTES + 1)).is_err());

        let leaked_result = serde_json::json!({
            "protocol": PROVIDER_PROTOCOL,
            "type": "credential_updated",
            "provider": "anthropic",
            "models": [],
            "credential": { "type": "api_key", "key": "must-not-cross-result-boundary" }
        });
        assert!(serde_json::from_value::<ProviderResult>(leaked_result).is_err());
    }

    #[test]
    fn disconnect_accepts_oauth_only_cataloged_providers() {
        let openai_codex = provider_option("openai-codex").expect("cataloged");
        assert!(!openai_codex.api_key);
        assert!(openai_codex.oauth.is_some());
        assert!(validate_provider("openai-codex").is_ok());
        assert!(validate_provider("anthropic").is_ok());
        assert!(validate_provider("not-a-provider").is_err());
    }

    #[test]
    fn provider_model_cache_is_lru_bounded_and_authority_scoped() {
        let cache = ProviderModelDiscoveryCache::with_limits(Duration::from_secs(60), 2);
        let now = Instant::now();
        cache.record_at("anthropic", "authority-a".into(), now);
        cache.record_at("openai", "authority-b".into(), now);
        assert!(cache.is_fresh_at("anthropic", "authority-a", now));

        cache.record_at("google", "authority-c".into(), now);
        assert!(cache.is_fresh_at("anthropic", "authority-a", now));
        assert!(!cache.is_fresh_at("openai", "authority-b", now));
        assert!(cache.is_fresh_at("google", "authority-c", now));

        assert!(!cache.is_fresh_at("anthropic", "rotated", now));
        assert!(!cache.is_fresh_at("anthropic", "authority-a", now));
    }

    #[test]
    fn provider_model_cache_expires_and_invalidates_explicitly() {
        let cache = ProviderModelDiscoveryCache::with_limits(Duration::from_secs(10), 2);
        let now = Instant::now();
        cache.record_at("anthropic", "authority-a".into(), now);
        assert!(!cache.is_fresh_at("anthropic", "authority-a", now + Duration::from_secs(10)));

        cache.record_at("anthropic", "authority-a".into(), now);
        cache.invalidate("anthropic");
        assert!(!cache.is_fresh_at("anthropic", "authority-a", now));
    }

    #[test]
    fn native_web_search_results_reject_credentials_answers_and_headers() {
        let leaked = serde_json::json!({
            "protocol": PROVIDER_PROTOCOL,
            "type": "search",
            "provider": "anthropic",
            "status": "ok",
            "sources": [],
            "answer": "provider synthesized answer",
            "credential": { "key": "secret" }
        });
        let leaked_encoded = serde_json::to_vec(&leaked).unwrap();
        let leaked_error = serde_json::from_slice::<NativeSearchFileResult>(&leaked_encoded)
            .expect_err("secret-bearing fields must be rejected");
        assert_eq!(
            native_search_schema_failure(&leaked_encoded, &leaked_error),
            "contained an unexpected top-level field"
        );

        let header_leak = serde_json::json!({
            "protocol": PROVIDER_PROTOCOL,
            "type": "search",
            "provider": "openai-codex",
            "status": "ok",
            "sources": [],
            "authorization": "Bearer secret"
        });
        assert!(serde_json::from_value::<NativeSearchFileResult>(header_leak).is_err());

        let valid = serde_json::json!({
            "protocol": PROVIDER_PROTOCOL,
            "type": "search",
            "provider": "xai",
            "status": "ok",
            "sources": [{
                "title": "Report",
                "url": "https://example.com/report",
                "snippet": "A public snippet",
                "publishedAt": "2024-01-01"
            }],
            "model": "grok-4.6",
            "requestId": "resp_1",
            "usage": { "inputTokens": 1, "outputTokens": 2, "searchRequests": 1 },
            "searchRequestCount": 1
        });
        let parsed = serde_json::from_value::<NativeSearchFileResult>(valid).unwrap();
        assert_eq!(parsed.provider, "xai");
        assert_eq!(parsed.sources[0].url, "https://example.com/report");
        assert!(parsed.error_kind.is_none());
    }

    #[test]
    fn native_search_safe_protocol_failures_preserve_their_kind() {
        let no_search = serde_json::json!({
            "protocol": PROVIDER_PROTOCOL,
            "type": "search",
            "provider": "openai-codex",
            "status": "error",
            "error_kind": "no_search_tool"
        });
        let parsed = serde_json::from_value::<NativeSearchFileResult>(no_search).unwrap();
        assert_eq!(
            native_search_error(&parsed).to_string(),
            "web provider completed without running hosted search"
        );

        let malformed = serde_json::json!({
            "protocol": PROVIDER_PROTOCOL,
            "type": "search",
            "provider": "openai-codex",
            "status": "error",
            "error_kind": "malformed"
        });
        let parsed = serde_json::from_value::<NativeSearchFileResult>(malformed).unwrap();
        assert_eq!(
            native_search_error(&parsed).to_string(),
            "web provider returned malformed search data"
        );
    }

    #[test]
    fn native_search_prompt_acknowledgement_waits_for_a_complete_json_result() {
        let root =
            std::env::temp_dir().join(format!("guruterminal-search-gate-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let result = root.join("result.json");
        std::fs::write(&result, []).unwrap();
        assert!(!search_support_result_ready(false, &result).unwrap());
        assert!(!search_support_result_ready(true, &result).unwrap());
        std::fs::write(&result, br#"{"protocol":"guruterminal-provider/1""#).unwrap();
        assert!(!search_support_result_ready(true, &result).unwrap());
        std::fs::write(&result, b"{}").unwrap();
        assert!(!search_support_result_ready(true, &result).unwrap());
        std::fs::write(
            &result,
            br#"{"protocol":"guruterminal-provider/1","type":"search","provider":"xai","status":"error","error_kind":"provider"}"#,
        )
        .unwrap();
        assert!(search_support_result_ready(true, &result).unwrap());
        std::fs::remove_file(&result).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
}
