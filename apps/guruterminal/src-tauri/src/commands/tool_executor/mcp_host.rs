use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use serde::Deserialize;
use serde_json::{json, Value};

use super::*;
use crate::{
    artifact_trust::{create_private_directory, ensure_private_directory},
    marketplace::{bundled_catalog, MarketplaceEntryDto, MarketplaceRuntimeKind},
    mcp::{
        contains_protected_value, filter_tool_providers, tool_to_agent_schema,
        validate_result_provider, BundledMcpRuntime, McpError, McpLaunchConfig, McpSession,
        McpTool, ProviderlessToolPolicy,
    },
    mcp_pool::{authority_fingerprint, McpPoolKey, TurnMcpServer},
};

const MCP_CALL_TIMEOUT: Duration = Duration::from_secs(45);
const MCP_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

struct McpRunAuthority {
    runtime: BundledMcpRuntime,
    enabled_provider_ids: BTreeSet<String>,
    allowed_network_hosts: BTreeSet<String>,
    credentials: BTreeMap<String, String>,
    provider_config: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpConnectRequest {
    server_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpToolCallRequest {
    server_id: String,
    tool_name: String,
    arguments: Value,
}

impl ToolCapture {
    pub(crate) async fn shutdown_mcp(&self) {
        let sessions = {
            let mut sessions = self.mcp_sessions.lock().await;
            std::mem::take(&mut *sessions)
        };
        match &self.mcp_pool {
            Some(pool) => {
                for (_, server) in sessions {
                    pool.release(server).await;
                }
            }
            None => {
                for (_, server) in sessions {
                    let _ = server.session.shutdown(MCP_SHUTDOWN_GRACE).await;
                }
            }
        }
    }
}

impl AppToolExecutor {
    pub(super) async fn mcp_connect(&self, params: Value) -> Result<Value, BrokerError> {
        let request: McpConnectRequest =
            serde_json::from_value(params).map_err(|_| BrokerError::Malformed)?;
        {
            let mut sessions = self.capture.mcp_sessions.lock().await;
            if sessions.contains_key(&request.server_id) {
                let cached = {
                    let server = sessions
                        .get_mut(&request.server_id)
                        .expect("cached MCP session disappeared");
                    match server.session.is_running().await {
                        Ok(false) => Ok(None),
                        Err(error) => Err(map_mcp_error(error)),
                        Ok(true) => {
                            refresh_inventory(&request.server_id, server, false).await?;
                            Ok(Some(agent_tool_cards(
                                &request.server_id,
                                server.tools.values(),
                            )?))
                        }
                    }
                };
                match cached {
                    Ok(Some(tools)) => {
                        return Ok(json!({
                            "server_id": request.server_id,
                            "tools": tools,
                        }))
                    }
                    Ok(None) => {
                        sessions.remove(&request.server_id);
                    }
                    Err(error) => {
                        sessions.remove(&request.server_id);
                        return Err(error);
                    }
                }
            }
        }
        let authority = self.mcp_run_authority(&request.server_id).await?;
        {
            let sessions = self.capture.mcp_sessions.lock().await;
            if let Some(server) = sessions.get(&request.server_id) {
                return Ok(json!({
                    "server_id": request.server_id,
                    "tools": agent_tool_cards(&request.server_id, server.tools.values())?,
                }));
            }
        }

        self.attach_or_spawn_mcp(&request.server_id, authority)
            .await
    }

    async fn attach_or_spawn_mcp(
        &self,
        server_id: &str,
        authority: McpRunAuthority,
    ) -> Result<Value, BrokerError> {
        let pool_key = self
            .capture
            .mcp_pool
            .as_ref()
            .map(|_| pool_key(&self.guru_id, server_id, &authority));
        if let (Some(pool), Some(key)) = (&self.capture.mcp_pool, pool_key.as_ref()) {
            if let Some(mut server) = pool.acquire(key).await {
                match prepare_acquired_session(server_id, &mut server).await {
                    Ok(tools) => {
                        let mut sessions = self.capture.mcp_sessions.lock().await;
                        if let Some(existing) = sessions.get(server_id) {
                            let cards = agent_tool_cards(server_id, existing.tools.values())?;
                            drop(sessions);
                            pool.release(server).await;
                            return Ok(json!({
                                "server_id": server_id,
                                "tools": cards,
                            }));
                        }
                        sessions.insert(server_id.to_owned(), server);
                        return Ok(json!({ "server_id": server_id, "tools": tools }));
                    }
                    Err(_) => {
                        server.discard().await;
                    }
                }
            }
        }

        let scratch = match &self.capture.mcp_pool {
            Some(pool) => pool
                .create_scratch(&self.guru_id, server_id)
                .map_err(|_| BrokerError::Execution("MCP run isolation failed".into()))?,
            None => {
                let scratch_root = self.capture.mcp_scratch_root.as_ref().ok_or_else(|| {
                    BrokerError::Execution("MCP run isolation is unavailable".into())
                })?;
                ensure_private_directory(scratch_root)
                    .map_err(|_| BrokerError::Execution("MCP run isolation failed".into()))?;
                let scratch =
                    scratch_root.join(format!("{}-{}", server_id, uuid::Uuid::new_v4().simple()));
                create_private_directory(&scratch)
                    .map_err(|_| BrokerError::Execution("MCP run isolation failed".into()))?;
                scratch
            }
        };
        let sensitive_values = authority.credentials.values().cloned().collect::<Vec<_>>();
        let providerless_tool_policy = authority.runtime.providerless_tool_policy.clone();
        let provider_receipt_pointer = authority.runtime.provider_receipt_pointer.clone();
        let control_tool_names = authority.runtime.control_tool_names.clone();
        let enabled_provider_ids = authority.enabled_provider_ids.clone();
        let bootstrap = json!({
            "type": "guruterminal.bootstrap",
            "protocol_version": 1,
            "run_id": format!("mcp:{}", uuid::Uuid::new_v4().simple()),
            "scratch_dir": scratch.to_string_lossy(),
            "credentials": authority.credentials,
            "settings": {
                "allowed_categories": authority.runtime.allowed_categories.clone(),
                "enabled_provider_ids": enabled_provider_ids.clone(),
                "allowed_network_hosts": authority.allowed_network_hosts,
                "provider_config": authority.provider_config,
            }
        });
        let launch = McpLaunchConfig {
            server_id: server_id.to_owned(),
            executable: authority.runtime.executable,
            arguments: Vec::new(),
            private_working_dir: scratch.clone(),
            lease_dir: authority.runtime.lease_dir,
            environment: BTreeMap::new(),
            bootstrap,
        };
        // Spawn without holding the session map. OpenBB cold start is several
        // seconds; blocking the map would stall shutdown and a second connect.
        let (session, initial_tools) = McpSession::spawn(launch).await.map_err(map_mcp_error)?;
        if let Err(error) =
            ensure_inventory_contains_no_protected_values(&initial_tools, &sensitive_values)
        {
            let _ = session.shutdown(MCP_SHUTDOWN_GRACE).await;
            let _ = tokio::fs::remove_dir_all(&scratch).await;
            return Err(error);
        }
        let initial_tool_names = initial_tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>();
        if initial_tool_names != control_tool_names {
            let _ = session.shutdown(MCP_SHUTDOWN_GRACE).await;
            let _ = tokio::fs::remove_dir_all(&scratch).await;
            return Err(BrokerError::Execution(
                "MCP runtime initial inventory did not match its signed control surface".into(),
            ));
        }
        let tools = filter_inventory(
            &initial_tools,
            &control_tool_names,
            &enabled_provider_ids,
            &providerless_tool_policy,
        )?;
        let cards = agent_tool_cards(server_id, tools.values())?;
        let mut sessions = self.capture.mcp_sessions.lock().await;
        if let Some(server) = sessions.get(server_id) {
            let existing = agent_tool_cards(server_id, server.tools.values())?;
            drop(sessions);
            let _ = session.shutdown(MCP_SHUTDOWN_GRACE).await;
            let _ = tokio::fs::remove_dir_all(&scratch).await;
            return Ok(json!({
                "server_id": server_id,
                "tools": existing,
            }));
        }
        sessions.insert(
            server_id.to_owned(),
            TurnMcpServer {
                session,
                control_tool_names,
                tools,
                enabled_provider_ids,
                providerless_tool_policy,
                provider_receipt_pointer,
                sensitive_values,
                pool_key,
                scratch_dir: Some(scratch),
            },
        );
        Ok(json!({ "server_id": server_id, "tools": cards }))
    }

    pub(super) async fn mcp_call(
        &self,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let request: McpToolCallRequest =
            serde_json::from_value(params).map_err(|_| BrokerError::Malformed)?;
        let mut sessions = self.capture.mcp_sessions.lock().await;
        if !sessions.contains_key(&request.server_id) {
            return Ok(stopped_mcp_response());
        }
        let preflight = {
            let server = sessions
                .get_mut(&request.server_id)
                .expect("loaded MCP session disappeared");
            if !server.session.is_running().await.map_err(map_mcp_error)? {
                Err(BrokerError::Execution("bundled MCP runtime failed".into()))
            } else {
                refresh_inventory(&request.server_id, server, false).await
            }
        };
        let preflight_tools = match preflight {
            Ok(tools) => tools,
            Err(_) => {
                sessions.remove(&request.server_id);
                return Ok(stopped_mcp_response());
            }
        };
        let server = sessions
            .get_mut(&request.server_id)
            .expect("loaded MCP session disappeared after refresh");
        let Some(tool) = server.tools.get(&request.tool_name).cloned() else {
            return mcp_call_error_with_inventory(preflight_tools, BrokerError::MethodDenied);
        };
        if let Err(error) =
            validate_provider_argument(&tool, &request.arguments, &server.enabled_provider_ids)
        {
            return mcp_call_error_with_inventory(preflight_tools, error);
        }
        let requested_provider = request
            .arguments
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                server
                    .providerless_tool_policy
                    .implicit_provider
                    .get(&request.tool_name)
                    .cloned()
            });
        let control_tool = server.control_tool_names.contains(&request.tool_name);
        let call = server
            .session
            .call_tool(
                &request.tool_name,
                request.arguments.clone(),
                MCP_CALL_TIMEOUT,
            )
            .await;
        let result = match call {
            Ok(result) => result,
            Err(error) => {
                let terminal = matches!(
                    &error,
                    McpError::Io(_)
                        | McpError::Protocol
                        | McpError::Stopped
                        | McpError::FrameTooLarge
                );
                let mapped = map_mcp_error(error);
                if terminal {
                    sessions.remove(&request.server_id);
                    return Ok(stopped_mcp_response());
                }
                return mcp_call_error_with_inventory(preflight_tools, mapped);
            }
        };
        if result.is_error {
            return mcp_call_error_with_inventory(
                preflight_tools,
                BrokerError::Execution("MCP tool rejected the request".into()),
            );
        }
        if validate_result_provider(
            &result,
            &server.provider_receipt_pointer,
            requested_provider.as_deref(),
            &server.enabled_provider_ids,
        )
        .is_err()
        {
            sessions.remove(&request.server_id);
            return Ok(stopped_mcp_response());
        }

        let mut payload = serde_json::to_value(&result)
            .map_err(|_| BrokerError::Execution("MCP result was invalid".into()))?;
        if contains_protected_value(&payload, &server.sensitive_values) {
            sessions.remove(&request.server_id);
            return Ok(stopped_mcp_response());
        }
        let list_changed = server.session.take_tools_changed();
        let refresh = control_tool || list_changed;
        let postflight_tools = match refresh_inventory(&request.server_id, server, refresh).await {
            Ok(tools) => tools,
            Err(_) => {
                sessions.remove(&request.server_id);
                return Ok(stopped_mcp_response());
            }
        };
        let tools = postflight_tools.or(preflight_tools);
        if !control_tool {
            let capture_request = json!({
                "server_id": request.server_id.clone(),
                "tool_name": request.tool_name.clone(),
                "arguments": request.arguments.clone(),
            });
            let result_ref = match self
                .capture
                .stage_run_result(
                    delivery_id,
                    RunResultProducer {
                        runtime_id: request.server_id.clone(),
                        tool_name: request.tool_name.clone(),
                        provider: requested_provider,
                    },
                    &capture_request,
                    payload.clone(),
                    Vec::new(),
                )
                .await
            {
                Ok(result_ref) => result_ref,
                Err(error) => return mcp_call_error_with_inventory(tools, error),
            };
            attach_result_ref(&mut payload, &result_ref);
        }
        Ok(match tools {
            Some(tools) => json!({ "result": payload, "tools": tools }),
            None => json!({ "result": payload }),
        })
    }

    async fn mcp_run_authority(&self, server_id: &str) -> Result<McpRunAuthority, BrokerError> {
        let runtime = self
            .state
            .artifacts
            .mcp_runtimes
            .get(server_id)
            .cloned()
            .ok_or_else(|| BrokerError::Execution("MCP runtime is unavailable".into()))?;
        let catalog = bundled_catalog()
            .map_err(|_| BrokerError::Execution("Marketplace catalog is unavailable".into()))?;
        let entries = catalog
            .entries
            .iter()
            .filter(|entry| {
                self.capability_ids.contains(&entry.id)
                    && entry.runtime.kind == MarketplaceRuntimeKind::BundledMcp
                    && entry.runtime.server_id.as_deref() == Some(server_id)
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return Err(BrokerError::MethodDenied);
        }
        let requested_provider_ids = entries
            .iter()
            .flat_map(|entry| entry.runtime.provider_ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        if requested_provider_ids.is_empty()
            || !requested_provider_ids.is_subset(&runtime.provider_ids)
        {
            return Err(BrokerError::Execution(
                "MCP provider authority is invalid".into(),
            ));
        }
        for entry in &entries {
            let possible_hosts = entry
                .runtime
                .provider_ids
                .iter()
                .filter_map(|provider| runtime.provider_network_hosts.get(provider))
                .flatten()
                .cloned()
                .collect::<BTreeSet<_>>();
            let declared_hosts = entry
                .permissions
                .network_hosts
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            if declared_hosts.is_empty() || declared_hosts != possible_hosts {
                return Err(BrokerError::Execution(
                    "MCP network authority is invalid".into(),
                ));
            }
        }

        let credential_specs = entries
            .iter()
            .filter(|entry| !entry.runtime.credential_mapping.is_empty())
            .map(|entry| (entry.id.clone(), entry.runtime.credential_mapping.clone()))
            .collect::<Vec<_>>();
        let credentials = tokio::task::spawn_blocking(move || {
            let mut credentials = BTreeMap::new();
            for (entry_id, mapping) in credential_specs {
                let bundle = crate::finance_credentials::get(&entry_id)
                    .map_err(|_| ())?
                    .ok_or(())?;
                for (field_id, target_id) in mapping {
                    let secret = bundle.get(&field_id).ok_or(())?.to_owned();
                    if credentials.insert(target_id, secret).is_some() {
                        return Err(());
                    }
                }
            }
            Ok::<_, ()>(credentials)
        })
        .await
        .map_err(|_| BrokerError::Execution("credential lookup failed".into()))?
        .map_err(|_| BrokerError::Execution("credential lookup failed".into()))?;

        let provider_config = collect_server_provider_config(
            server_id,
            &catalog.entries,
            &self.capability_ids,
            |entry_id, field_id| {
                crate::marketplace::connector_config_value(&self.state, entry_id, field_id)
                    .map_err(|_| BrokerError::Execution("connector setup is unavailable".into()))
            },
        )?;
        // A manifest-keyless provider can still require non-secret operational
        // configuration (SEC contact identity is the current case). Keep it in
        // the platform catalog, but do not expose or authorize it for a run
        // until every manifest-declared config field is present.
        let enabled_provider_ids = configured_provider_ids(
            requested_provider_ids,
            &runtime.provider_config_fields,
            &provider_config,
        );
        if enabled_provider_ids.is_empty() {
            return Err(BrokerError::Execution(
                "MCP provider setup is incomplete".into(),
            ));
        }
        let allowed_network_hosts = enabled_provider_ids
            .iter()
            .filter_map(|provider| runtime.provider_network_hosts.get(provider))
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();
        Ok(McpRunAuthority {
            runtime,
            enabled_provider_ids,
            allowed_network_hosts,
            credentials,
            provider_config,
        })
    }
}

fn configured_provider_ids(
    requested: BTreeSet<String>,
    required_config: &BTreeMap<String, BTreeSet<String>>,
    provider_config: &BTreeMap<String, BTreeMap<String, String>>,
) -> BTreeSet<String> {
    requested
        .into_iter()
        .filter(|provider_id| {
            required_config.get(provider_id).is_none_or(|required| {
                required.is_empty()
                    || provider_config.get(provider_id).is_some_and(|values| {
                        required.iter().all(|field| values.contains_key(field))
                    })
            })
        })
        .collect()
}

/// Non-secret provider config (SEC contact email today) is stored on the
/// sibling Marketplace entry that collects it. A run that enables
/// `openbb.platform` still receives that setup even when `sec.edgar` is
/// not itself enabled for the Guru.
fn collect_server_provider_config(
    server_id: &str,
    catalog_entries: &[MarketplaceEntryDto],
    enabled_entry_ids: &BTreeSet<String>,
    mut read_field: impl FnMut(&str, &str) -> Result<Option<String>, BrokerError>,
) -> Result<BTreeMap<String, BTreeMap<String, String>>, BrokerError> {
    let mut provider_config = BTreeMap::<String, BTreeMap<String, String>>::new();
    for entry in catalog_entries {
        if entry.runtime.kind != MarketplaceRuntimeKind::BundledMcp
            || entry.runtime.server_id.as_deref() != Some(server_id)
            || entry.runtime.config_mapping.is_empty()
        {
            continue;
        }
        let enabled = enabled_entry_ids.contains(&entry.id);
        let [provider_id] = entry.runtime.provider_ids.as_slice() else {
            return Err(BrokerError::Execution(
                "MCP provider configuration is ambiguous".into(),
            ));
        };
        let mut values = BTreeMap::new();
        let mut complete = true;
        for field_id in entry.runtime.config_mapping.keys() {
            match read_field(&entry.id, field_id)? {
                Some(value) => {
                    values.insert(field_id.clone(), value);
                }
                None => complete = false,
            }
        }
        if !complete {
            if enabled {
                return Err(BrokerError::Execution(
                    "connector setup is incomplete".into(),
                ));
            }
            continue;
        }
        let dest = provider_config.entry(provider_id.clone()).or_default();
        for (field_id, value) in values {
            if dest.insert(field_id, value).is_some() {
                return Err(BrokerError::Execution(
                    "MCP provider configuration is ambiguous".into(),
                ));
            }
        }
    }
    Ok(provider_config)
}

async fn refresh_inventory(
    server_id: &str,
    server: &mut TurnMcpServer,
    force: bool,
) -> Result<Option<Vec<Value>>, BrokerError> {
    if !force && !server.session.take_tools_changed() {
        return Ok(None);
    }
    let listed = server.session.list_tools().await.map_err(map_mcp_error)?;
    ensure_inventory_contains_no_protected_values(&listed, &server.sensitive_values)?;
    let filtered = filter_inventory(
        &listed,
        &server.control_tool_names,
        &server.enabled_provider_ids,
        &server.providerless_tool_policy,
    )?;
    let cards = agent_tool_cards(server_id, filtered.values())?;
    server.tools = filtered;
    Ok(Some(cards))
}

fn ensure_inventory_contains_no_protected_values(
    tools: &[McpTool],
    sensitive_values: &[String],
) -> Result<(), BrokerError> {
    let inventory = serde_json::to_value(tools)
        .map_err(|_| BrokerError::Execution("MCP tool inventory was invalid".into()))?;
    if contains_protected_value(&inventory, sensitive_values) {
        return Err(BrokerError::Execution(
            "MCP tool inventory contained protected credential data".into(),
        ));
    }
    Ok(())
}

fn mcp_call_error_with_inventory(
    tools: Option<Vec<Value>>,
    error: BrokerError,
) -> Result<Value, BrokerError> {
    match tools {
        Some(tools) => Ok(json!({ "call_error": true, "tools": tools })),
        None => Err(error),
    }
}

fn stopped_mcp_response() -> Value {
    json!({ "call_error": true, "session_stopped": true })
}

fn filter_inventory(
    tools: &[McpTool],
    control_tool_names: &BTreeSet<String>,
    enabled_provider_ids: &BTreeSet<String>,
    providerless_tool_policy: &ProviderlessToolPolicy,
) -> Result<BTreeMap<String, McpTool>, BrokerError> {
    let mut filtered = BTreeMap::new();
    let mut dynamic_names = BTreeSet::new();
    for tool in tools {
        let control = control_tool_names.contains(&tool.name);
        let has_provider_parameter = tool
            .input_schema
            .get("properties")
            .and_then(|properties| properties.get("provider"))
            .is_some();
        if !control && !has_provider_parameter {
            let local = providerless_tool_policy.local_tools.contains(&tool.name);
            let implicit_enabled = providerless_tool_policy
                .implicit_provider
                .get(&tool.name)
                .is_some_and(|provider| enabled_provider_ids.contains(provider));
            if !local && !implicit_enabled {
                continue;
            }
        }
        let Some(tool) =
            filter_tool_providers(tool, enabled_provider_ids, control).map_err(map_mcp_error)?
        else {
            continue;
        };
        let schema = tool_to_agent_schema("runtime", &tool).map_err(map_mcp_error)?;
        let dynamic_suffix = schema
            .get("name")
            .and_then(Value::as_str)
            .and_then(|name| name.strip_prefix("mcp__runtime__"))
            .ok_or_else(|| BrokerError::Execution("MCP tool schema is invalid".into()))?;
        if !dynamic_names.insert(dynamic_suffix.to_owned()) {
            return Err(BrokerError::Execution(
                "MCP tool namespace collision".into(),
            ));
        }
        filtered.insert(tool.name.clone(), tool);
    }
    if filtered.is_empty() {
        return Err(BrokerError::Execution(
            "MCP runtime exposed no authorized tools".into(),
        ));
    }
    Ok(filtered)
}

fn agent_tool_cards<'a>(
    server_id: &str,
    tools: impl Iterator<Item = &'a McpTool>,
) -> Result<Vec<Value>, BrokerError> {
    tools
        .map(|tool| tool_to_agent_schema(server_id, tool).map_err(map_mcp_error))
        .collect()
}

fn validate_provider_argument(
    tool: &McpTool,
    arguments: &Value,
    enabled_provider_ids: &BTreeSet<String>,
) -> Result<(), BrokerError> {
    let arguments = arguments.as_object().ok_or(BrokerError::Malformed)?;
    let Some(provider_schema) = tool
        .input_schema
        .get("properties")
        .and_then(|value| value.get("provider"))
    else {
        return Ok(());
    };
    let required = tool
        .input_schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values
                .iter()
                .any(|value| value.as_str() == Some("provider"))
        });
    let provider = arguments.get("provider").and_then(Value::as_str);
    if required && provider.is_none() {
        return Err(BrokerError::Malformed);
    }
    if let Some(provider) = provider {
        let schema_allows = provider_schema
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(provider)));
        if !enabled_provider_ids.contains(provider) || !schema_allows {
            return Err(BrokerError::MethodDenied);
        }
    }
    Ok(())
}

fn map_mcp_error(_error: McpError) -> BrokerError {
    BrokerError::Execution("bundled MCP runtime failed".into())
}

fn pool_key(guru_id: &str, server_id: &str, authority: &McpRunAuthority) -> McpPoolKey {
    let canonical = json!({
        "allowed_categories": authority.runtime.allowed_categories,
        "allowed_network_hosts": authority.allowed_network_hosts,
        "control_tool_names": authority.runtime.control_tool_names,
        "credentials": authority.credentials,
        "enabled_provider_ids": authority.enabled_provider_ids,
        "executable": authority.runtime.executable,
        "provider_config": authority.provider_config,
    });
    McpPoolKey {
        guru_id: guru_id.to_owned(),
        server_id: server_id.to_owned(),
        authority_fingerprint: authority_fingerprint(&canonical),
    }
}

async fn prepare_acquired_session(
    server_id: &str,
    server: &mut TurnMcpServer,
) -> Result<Vec<Value>, BrokerError> {
    if !server.session.is_running().await.map_err(map_mcp_error)? {
        return Err(BrokerError::Execution("bundled MCP runtime failed".into()));
    }
    let tools = refresh_inventory(server_id, server, true)
        .await?
        .ok_or_else(|| BrokerError::Execution("bundled MCP runtime failed".into()))?;
    let names = server.tools.keys().cloned().collect::<BTreeSet<_>>();
    if names != server.control_tool_names {
        return Err(BrokerError::Execution(
            "MCP runtime reused inventory did not match its signed control surface".into(),
        ));
    }
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::{fs, path::PathBuf, sync::Arc};

    #[cfg(unix)]
    const ISOLATION_MCP_SCRIPT: &str = r#"
activated=0
async_inventory_announced=0
if [ -n "${MCP_TEST_PID:-}" ]; then
  printf '%s\n' "$$" > "$MCP_TEST_PID"
fi
trap 'printf exited > "$MCP_TEST_EXITED"' EXIT
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$MCP_TEST_TRANSCRIPT"
  case "$line" in
    *'"type":"guruterminal.bootstrap"'*)
      ;;
    *'"method":"initialize"'*)
      id=${line#*\"id\":\"}; id=${id%%\"*}
      printf '{"jsonrpc":"2.0","id":"%s","result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"Guru MCP isolation test server","version":"1"}}}\n' "$id"
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"tools/list"'*)
      id=${line#*\"id\":\"}; id=${id%%\"*}
      if [ "${MCP_TEST_ASYNC_LIST_CHANGED:-0}" = 1 ] && [ "$async_inventory_announced" = 0 ]; then
        printf '{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}\n'
        async_inventory_announced=1
        activate_after_response=1
      fi
      if [ "$activated" = 1 ]; then
        printf '{"jsonrpc":"2.0","id":"%s","result":{"tools":[{"name":"activate_tools","description":"activate","inputSchema":{"type":"object","properties":{"tool_names":{"type":"array"}},"required":["tool_names"]}},{"name":"deactivate_tools","description":"deactivate","inputSchema":{"type":"object","properties":{"tool_names":{"type":"array"}},"required":["tool_names"]}},{"name":"equity_quote","description":"quote","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"},"provider":{"type":"string","enum":["fmp","yfinance"]}},"required":["symbol"]},"annotations":{"readOnlyHint":true,"destructiveHint":false}}]}}\n' "$id"
      else
        printf '{"jsonrpc":"2.0","id":"%s","result":{"tools":[{"name":"activate_tools","description":"activate","inputSchema":{"type":"object","properties":{"tool_names":{"type":"array"}},"required":["tool_names"]}},{"name":"deactivate_tools","description":"deactivate","inputSchema":{"type":"object","properties":{"tool_names":{"type":"array"}},"required":["tool_names"]}}]}}\n' "$id"
      fi
      if [ "${activate_after_response:-0}" = 1 ]; then
        activated=1
        activate_after_response=0
      fi
      ;;
    *'"method":"tools/call"'*'"name":"activate_tools"'*)
      id=${line#*\"id\":\"}; id=${id%%\"*}
      activated=1
      printf '{"jsonrpc":"2.0","id":"%s","result":{"content":[{"type":"text","text":"activated"}],"structuredContent":{"activated":true},"isError":false}}\n' "$id"
      ;;
    *'"method":"tools/call"'*'"name":"deactivate_tools"'*)
      id=${line#*\"id\":\"}; id=${id%%\"*}
      activated=0
      printf '{"jsonrpc":"2.0","id":"%s","result":{"content":[{"type":"text","text":"deactivated"}],"structuredContent":{"deactivated":true},"isError":false}}\n' "$id"
      ;;
    *'"method":"tools/call"'*'"name":"equity_quote"'*)
      id=${line#*\"id\":\"}; id=${id%%\"*}
      case "$line" in
        *'"provider":"fmp"'*) provider=fmp ;;
        *) provider=yfinance ;;
      esac
      printf '{"jsonrpc":"2.0","id":"%s","result":{"content":[{"type":"text","text":"quote"}],"structuredContent":{"provider":"%s","price":123},"isError":false}}\n' "$id" "$provider"
      ;;
  esac
done
"#;

    fn providerless_tool(name: &str) -> McpTool {
        McpTool {
            name: name.into(),
            title: None,
            description: None,
            input_schema: json!({"type": "object", "properties": {}}),
            output_schema: None,
            annotations: Some(json!({"readOnlyHint": true})),
            metadata: None,
        }
    }

    #[cfg(unix)]
    fn poolable_control_tools() -> BTreeSet<String> {
        BTreeSet::from(["activate_tools".into(), "deactivate_tools".into()])
    }

    #[cfg(unix)]
    async fn spawn_isolated_turn_server(
        temporary: &tempfile::TempDir,
        name: &str,
        enabled_provider_ids: BTreeSet<String>,
        credential: (&str, &str),
        async_list_changed: bool,
    ) -> (TurnMcpServer, PathBuf, PathBuf) {
        spawn_isolated_turn_server_with_control(
            temporary,
            name,
            enabled_provider_ids,
            credential,
            async_list_changed,
            BTreeSet::from(["activate_tools".into()]),
        )
        .await
    }

    #[cfg(unix)]
    async fn spawn_isolated_turn_server_with_control(
        temporary: &tempfile::TempDir,
        name: &str,
        enabled_provider_ids: BTreeSet<String>,
        credential: (&str, &str),
        async_list_changed: bool,
        control_tool_names: BTreeSet<String>,
    ) -> (TurnMcpServer, PathBuf, PathBuf) {
        let transcript = temporary.path().join(format!("{name}-transcript.jsonl"));
        let exited = temporary.path().join(format!("{name}-exited"));
        let pid = temporary.path().join(format!("{name}-pid"));
        let scratch = temporary.path().join(format!("{name}-scratch"));
        let mut environment = BTreeMap::from([
            (
                "MCP_TEST_TRANSCRIPT".into(),
                transcript.to_string_lossy().into_owned(),
            ),
            (
                "MCP_TEST_EXITED".into(),
                exited.to_string_lossy().into_owned(),
            ),
            ("MCP_TEST_PID".into(), pid.to_string_lossy().into_owned()),
        ]);
        if async_list_changed {
            environment.insert("MCP_TEST_ASYNC_LIST_CHANGED".into(), "1".into());
        }
        let (session, initial_tools) = McpSession::spawn(McpLaunchConfig {
            server_id: "openbb".into(),
            // macOS `/bin/sh` re-execs the selected shell, which would change
            // the executable identity after the lease is registered.
            executable: fs::canonicalize("/bin/bash").unwrap(),
            arguments: vec!["-c".into(), ISOLATION_MCP_SCRIPT.into()],
            private_working_dir: scratch.clone(),
            lease_dir: temporary.path().join(format!("{name}-leases")),
            environment,
            bootstrap: json!({
                "type": "guruterminal.bootstrap",
                "protocol_version": 1,
                "run_id": format!("run-{name}"),
                "scratch_dir": scratch,
                "credentials": { credential.0: credential.1 },
                "settings": {
                    "enabled_provider_ids": enabled_provider_ids,
                }
            }),
        })
        .await
        .unwrap();
        let tools = filter_inventory(
            &initial_tools,
            &control_tool_names,
            &enabled_provider_ids,
            &ProviderlessToolPolicy::default(),
        )
        .unwrap();
        (
            TurnMcpServer {
                session,
                control_tool_names,
                tools,
                enabled_provider_ids,
                providerless_tool_policy: ProviderlessToolPolicy::default(),
                provider_receipt_pointer: "/structuredContent/provider".into(),
                sensitive_values: vec![credential.1.into()],
                pool_key: None,
                scratch_dir: Some(scratch),
            },
            transcript,
            exited,
        )
    }

    #[cfg(unix)]
    fn isolated_capture(temporary: &tempfile::TempDir, name: &str) -> Arc<ToolCapture> {
        Arc::new(ToolCapture {
            mcp_scratch_root: Some(temporary.path().join(format!("{name}-run"))),
            ..ToolCapture::default()
        })
    }

    #[cfg(unix)]
    fn pooled_capture(
        temporary: &tempfile::TempDir,
        name: &str,
        pool: crate::mcp_pool::McpProcessPool,
    ) -> Arc<ToolCapture> {
        Arc::new(ToolCapture {
            mcp_scratch_root: Some(temporary.path().join(format!("{name}-run"))),
            mcp_pool: Some(pool),
            ..ToolCapture::default()
        })
    }

    #[cfg(unix)]
    fn test_authority(
        temporary: &tempfile::TempDir,
        name: &str,
        enabled_provider_ids: BTreeSet<String>,
        credentials: BTreeMap<String, String>,
    ) -> McpRunAuthority {
        McpRunAuthority {
            runtime: BundledMcpRuntime {
                server_id: "openbb".into(),
                executable: fs::canonicalize("/bin/bash").unwrap(),
                runtime_dir: temporary.path().to_path_buf(),
                manifest_path: temporary.path().join(format!("{name}-manifest.json")),
                lease_dir: temporary.path().join(format!("{name}-runtime-leases")),
                allowed_categories: vec!["equity".into()],
                provider_ids: BTreeSet::from(["yfinance".into(), "fmp".into()]),
                provider_network_hosts: BTreeMap::new(),
                provider_config_fields: BTreeMap::new(),
                providerless_tool_policy: ProviderlessToolPolicy::default(),
                provider_receipt_pointer: "/structuredContent/provider".into(),
                tool_activation: None,
                control_tool_names: poolable_control_tools(),
            },
            enabled_provider_ids,
            allowed_network_hosts: BTreeSet::new(),
            credentials,
            provider_config: BTreeMap::new(),
        }
    }

    #[cfg(unix)]
    fn isolated_executor(
        temporary: &tempfile::TempDir,
        state: &AppState,
        guru_id: &str,
        capture: Arc<ToolCapture>,
    ) -> AppToolExecutor {
        let workspace = temporary.path().join(format!("{guru_id}-workspace"));
        fs::create_dir_all(&workspace).unwrap();
        AppToolExecutor {
            state: state.clone(),
            capture,
            guru_id: guru_id.into(),
            guru_root: crate::commands::tests::bound_root(&workspace),
            capability_ids: BTreeSet::new(),
            chat_provider: String::new(),
        }
    }

    #[cfg(unix)]
    async fn wait_for_exit_marker(path: &std::path::Path) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !path.is_file() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn credential_echoes_are_detected_before_result_capture() {
        let secrets = vec!["provider-secret-123".to_owned()];
        assert!(contains_protected_value(
            &json!({"content": [{"type": "text", "text": "error: provider-secret-123"}]}),
            &secrets,
        ));
        let mut keyed_echo = serde_json::Map::new();
        keyed_echo.insert("provider-secret-123".into(), json!("credential-shaped key"));
        assert!(contains_protected_value(
            &Value::Object(keyed_echo),
            &secrets,
        ));
        assert!(!contains_protected_value(
            &json!({"content": [{"type": "text", "text": "quote unavailable"}]}),
            &secrets,
        ));
    }

    #[test]
    fn credential_echoes_in_tool_inventory_are_rejected_before_exposure() {
        let mut tool = providerless_tool("technical_sma");
        tool.description = Some("schema leaked provider-secret-123".into());
        assert!(matches!(
            ensure_inventory_contains_no_protected_values(
                &[tool],
                &["provider-secret-123".into()],
            ),
            Err(BrokerError::Execution(message))
                if message == "MCP tool inventory contained protected credential data"
        ));
    }

    #[test]
    fn refreshed_inventory_is_preserved_when_a_stale_call_is_rejected() {
        let tools = vec![json!({"name": "mcp__openbb__activate_tools"})];
        let response =
            mcp_call_error_with_inventory(Some(tools.clone()), BrokerError::MethodDenied).unwrap();
        assert_eq!(response["call_error"], true);
        assert_eq!(response["tools"], Value::Array(tools));
        assert!(matches!(
            mcp_call_error_with_inventory(None, BrokerError::MethodDenied),
            Err(BrokerError::MethodDenied)
        ));
    }

    #[test]
    fn providerless_tools_require_manifest_authority() {
        let tools = vec![
            providerless_tool("available_tools"),
            providerless_tool("technical_sma"),
            providerless_tool("imf_utils_list_tables"),
            providerless_tool("undeclared_external_tool"),
        ];
        let control = BTreeSet::from(["available_tools".into()]);
        let policy = ProviderlessToolPolicy {
            local_tools: BTreeSet::from(["technical_sma".into()]),
            implicit_provider: BTreeMap::from([("imf_utils_list_tables".into(), "imf".into())]),
        };
        let enabled = BTreeSet::from(["yfinance".into()]);
        let filtered = filter_inventory(&tools, &control, &enabled, &policy).unwrap();
        assert!(filtered.contains_key("available_tools"));
        assert!(filtered.contains_key("technical_sma"));
        assert!(!filtered.contains_key("imf_utils_list_tables"));
        assert!(!filtered.contains_key("undeclared_external_tool"));

        let enabled = BTreeSet::from(["imf".into()]);
        let filtered = filter_inventory(&tools, &control, &enabled, &policy).unwrap();
        assert!(filtered.contains_key("imf_utils_list_tables"));
    }

    #[test]
    fn non_control_tools_require_explicit_read_only_annotations() {
        let mut missing = providerless_tool("technical_sma");
        missing.annotations = None;
        let mut destructive = providerless_tool("technical_ema");
        destructive.annotations = Some(json!({
            "readOnlyHint": true,
            "destructiveHint": true
        }));
        let policy = ProviderlessToolPolicy {
            local_tools: BTreeSet::from(["technical_sma".into(), "technical_ema".into()]),
            implicit_provider: BTreeMap::new(),
        };
        let filtered = filter_inventory(
            &[missing, destructive],
            &BTreeSet::new(),
            &BTreeSet::from(["yfinance".into()]),
            &policy,
        );
        assert!(matches!(
            filtered,
            Err(BrokerError::Execution(message)) if message == "MCP runtime exposed no authorized tools"
        ));
    }

    #[test]
    fn providers_with_required_non_secret_config_fail_closed_until_configured() {
        let requested = BTreeSet::from(["sec".into(), "yfinance".into()]);
        let required = BTreeMap::from([
            ("sec".into(), BTreeSet::from(["contact_email".into()])),
            ("yfinance".into(), BTreeSet::new()),
        ]);
        assert_eq!(
            configured_provider_ids(requested.clone(), &required, &BTreeMap::new()),
            BTreeSet::from(["yfinance".into()])
        );
        assert_eq!(
            configured_provider_ids(
                requested,
                &required,
                &BTreeMap::from([(
                    "sec".into(),
                    BTreeMap::from([("contact_email".into(), "research@example.com".into())]),
                )]),
            ),
            BTreeSet::from(["sec".into(), "yfinance".into()])
        );
    }

    #[test]
    fn openbb_platform_receives_sec_email_from_the_sibling_connector() {
        let catalog = bundled_catalog().unwrap();
        let enabled = BTreeSet::from(["openbb.platform".into()]);
        let stored: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::from([(
            "sec.edgar".to_owned(),
            BTreeMap::from([("contact_email".into(), "research@example.com".into())]),
        )]);
        let config = collect_server_provider_config(
            "openbb",
            &catalog.entries,
            &enabled,
            |entry_id, field_id| {
                Ok(stored
                    .get(entry_id)
                    .and_then(|values| values.get(field_id))
                    .cloned())
            },
        )
        .unwrap();
        assert_eq!(
            config
                .get("sec")
                .and_then(|values| values.get("contact_email"))
                .map(String::as_str),
            Some("research@example.com")
        );

        let without_email =
            collect_server_provider_config("openbb", &catalog.entries, &enabled, |_, _| Ok(None))
                .unwrap();
        assert!(!without_email.contains_key("sec"));

        let enabled_sec = BTreeSet::from(["sec.edgar".into()]);
        let error =
            collect_server_provider_config("openbb", &catalog.entries, &enabled_sec, |_, _| {
                Ok(None)
            })
            .unwrap_err();
        assert!(matches!(
            error,
            BrokerError::Execution(message) if message == "connector setup is incomplete"
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repeated_connect_in_one_turn_reuses_the_same_mcp_process_and_cleanup_ends_it() {
        let _guard = crate::mcp::MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let capture = isolated_capture(&temporary, "one-turn");
        let (server, transcript, exited) = spawn_isolated_turn_server(
            &temporary,
            "one-turn",
            BTreeSet::from(["yfinance".into()]),
            ("yfinance_api_key", "turn-secret"),
            false,
        )
        .await;
        capture
            .mcp_sessions
            .lock()
            .await
            .insert("openbb".into(), server);
        let executor = isolated_executor(&temporary, &state, "guru-a", capture.clone());

        let first = executor
            .mcp_connect(json!({"server_id": "openbb"}))
            .await
            .unwrap();
        let second = executor
            .mcp_connect(json!({"server_id": "openbb"}))
            .await
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(capture.mcp_sessions.lock().await.len(), 1);
        let transcript = fs::read_to_string(transcript).unwrap();
        assert_eq!(transcript.matches("\"method\":\"initialize\"").count(), 1);

        capture.shutdown_mcp().await;
        assert!(capture.mcp_sessions.lock().await.is_empty());
        wait_for_exit_marker(&exited).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn asynchronous_list_changed_inventory_is_returned_with_the_next_call() {
        let _guard = crate::mcp::MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let capture = isolated_capture(&temporary, "async-inventory");
        let (server, _transcript, exited) = spawn_isolated_turn_server(
            &temporary,
            "async-inventory",
            BTreeSet::from(["yfinance".into()]),
            ("yfinance_api_key", "turn-secret"),
            true,
        )
        .await;
        assert_eq!(
            server.tools.keys().cloned().collect::<Vec<_>>(),
            vec!["activate_tools"]
        );
        capture
            .mcp_sessions
            .lock()
            .await
            .insert("openbb".into(), server);
        let executor = isolated_executor(&temporary, &state, "guru-a", capture.clone());

        let response = executor
            .mcp_call(
                json!({
                    "server_id": "openbb",
                    "tool_name": "equity_quote",
                    "arguments": {"symbol": "TEST", "provider": "yfinance"}
                }),
                "delivery-async-inventory",
            )
            .await
            .unwrap();
        assert!(response["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["mcp_name"] == "equity_quote"));
        assert_eq!(
            response["result"]["structuredContent"]["provider"],
            "yfinance"
        );

        capture.discard_delivery("delivery-async-inventory").await;
        capture.shutdown_mcp().await;
        wait_for_exit_marker(&exited).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn turns_and_gurus_do_not_share_activation_provider_or_credential_authority() {
        let _guard = crate::mcp::MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let capture_a = isolated_capture(&temporary, "turn-a");
        let capture_b = isolated_capture(&temporary, "turn-b");
        let (server_a, transcript_a, exited_a) = spawn_isolated_turn_server(
            &temporary,
            "turn-a",
            BTreeSet::from(["yfinance".into()]),
            ("yfinance_api_key", "secret-a"),
            false,
        )
        .await;
        let (server_b, transcript_b, exited_b) = spawn_isolated_turn_server(
            &temporary,
            "turn-b",
            BTreeSet::from(["fmp".into()]),
            ("fmp_api_key", "secret-b"),
            false,
        )
        .await;
        capture_a
            .mcp_sessions
            .lock()
            .await
            .insert("openbb".into(), server_a);
        capture_b
            .mcp_sessions
            .lock()
            .await
            .insert("openbb".into(), server_b);
        let executor_a = isolated_executor(&temporary, &state, "guru-a", capture_a.clone());
        let executor_b = isolated_executor(&temporary, &state, "guru-b", capture_b.clone());

        let activated_a = executor_a
            .mcp_call(
                json!({
                    "server_id": "openbb",
                    "tool_name": "activate_tools",
                    "arguments": {"tool_names": ["equity_quote"]}
                }),
                "delivery-a-activate",
            )
            .await
            .unwrap();
        assert_eq!(
            activated_a["tools"][1]["parameters"]["properties"]["provider"]["enum"],
            json!(["yfinance"])
        );
        let still_admin_only_b = executor_b
            .mcp_connect(json!({"server_id": "openbb"}))
            .await
            .unwrap();
        assert_eq!(still_admin_only_b["tools"].as_array().unwrap().len(), 1);
        assert!(matches!(
            executor_b
                .mcp_call(
                    json!({
                        "server_id": "openbb",
                        "tool_name": "equity_quote",
                        "arguments": {"symbol": "TEST", "provider": "fmp"}
                    }),
                    "delivery-b-before-activation",
                )
                .await,
            Err(BrokerError::MethodDenied)
        ));
        assert!(matches!(
            executor_a
                .mcp_call(
                    json!({
                        "server_id": "openbb",
                        "tool_name": "equity_quote",
                        "arguments": {"symbol": "TEST", "provider": "fmp"}
                    }),
                    "delivery-a-wrong-provider",
                )
                .await,
            Err(BrokerError::MethodDenied)
        ));
        executor_a
            .mcp_call(
                json!({
                    "server_id": "openbb",
                    "tool_name": "equity_quote",
                    "arguments": {"symbol": "TEST", "provider": "yfinance"}
                }),
                "delivery-a-quote",
            )
            .await
            .unwrap();

        let activated_b = executor_b
            .mcp_call(
                json!({
                    "server_id": "openbb",
                    "tool_name": "activate_tools",
                    "arguments": {"tool_names": ["equity_quote"]}
                }),
                "delivery-b-activate",
            )
            .await
            .unwrap();
        assert_eq!(
            activated_b["tools"][1]["parameters"]["properties"]["provider"]["enum"],
            json!(["fmp"])
        );
        executor_b
            .mcp_call(
                json!({
                    "server_id": "openbb",
                    "tool_name": "equity_quote",
                    "arguments": {"symbol": "TEST", "provider": "fmp"}
                }),
                "delivery-b-quote",
            )
            .await
            .unwrap();

        let transcript_a = fs::read_to_string(transcript_a).unwrap();
        let transcript_b = fs::read_to_string(transcript_b).unwrap();
        assert!(transcript_a.contains("run-turn-a"));
        assert!(transcript_a.contains("secret-a"));
        assert!(!transcript_a.contains("secret-b"));
        assert!(transcript_b.contains("run-turn-b"));
        assert!(transcript_b.contains("secret-b"));
        assert!(!transcript_b.contains("secret-a"));

        capture_a.shutdown_mcp().await;
        capture_b.shutdown_mcp().await;
        wait_for_exit_marker(&exited_a).await;
        wait_for_exit_marker(&exited_b).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sequential_turns_reuse_pooled_mcp_process_and_reset_activation() {
        let _guard = crate::mcp::MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let pool = state.mcp_pool.clone();
        let authority = test_authority(
            &temporary,
            "reuse",
            BTreeSet::from(["yfinance".into()]),
            BTreeMap::from([("yfinance_api_key".into(), "reuse-secret".into())]),
        );
        let key = pool_key("guru-a", "openbb", &authority);
        let capture = pooled_capture(&temporary, "reuse-one", pool.clone());
        let (mut server, transcript, exited) = spawn_isolated_turn_server_with_control(
            &temporary,
            "reuse",
            BTreeSet::from(["yfinance".into()]),
            ("yfinance_api_key", "reuse-secret"),
            false,
            poolable_control_tools(),
        )
        .await;
        server.pool_key = Some(key);
        capture
            .mcp_sessions
            .lock()
            .await
            .insert("openbb".into(), server);
        let executor = isolated_executor(&temporary, &state, "guru-a", capture.clone());

        executor
            .mcp_call(
                json!({
                    "server_id": "openbb",
                    "tool_name": "activate_tools",
                    "arguments": {"tool_names": ["equity_quote"]}
                }),
                "delivery-reuse-activate",
            )
            .await
            .unwrap();
        executor
            .mcp_call(
                json!({
                    "server_id": "openbb",
                    "tool_name": "equity_quote",
                    "arguments": {"symbol": "TEST", "provider": "yfinance"}
                }),
                "delivery-reuse-quote",
            )
            .await
            .unwrap();

        capture.shutdown_mcp().await;
        assert!(capture.mcp_sessions.lock().await.is_empty());
        assert!(!exited.is_file());
        assert_eq!(pool.idle_count().await, 1);

        let next = pooled_capture(&temporary, "reuse-two", pool.clone());
        let next_executor = isolated_executor(&temporary, &state, "guru-a", next.clone());
        let connected = next_executor
            .attach_or_spawn_mcp("openbb", authority)
            .await
            .unwrap();
        let tools = connected["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert!(tools
            .iter()
            .any(|tool| tool["mcp_name"] == "activate_tools"));
        assert!(tools
            .iter()
            .any(|tool| tool["mcp_name"] == "deactivate_tools"));
        assert!(tools.iter().all(|tool| tool["mcp_name"] != "equity_quote"));
        assert!(matches!(
            next_executor
                .mcp_call(
                    json!({
                        "server_id": "openbb",
                        "tool_name": "equity_quote",
                        "arguments": {"symbol": "TEST", "provider": "yfinance"}
                    }),
                    "delivery-reuse-denied",
                )
                .await,
            Err(BrokerError::MethodDenied)
        ));
        assert_eq!(
            fs::read_to_string(transcript)
                .unwrap()
                .matches("\"method\":\"initialize\"")
                .count(),
            1
        );

        next.shutdown_mcp().await;
        assert!(!exited.is_file());
        pool.shutdown().await;
        wait_for_exit_marker(&exited).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn different_guru_or_authority_does_not_reuse_pooled_mcp_process() {
        let _guard = crate::mcp::MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let pool = state.mcp_pool.clone();
        let yfinance = test_authority(
            &temporary,
            "authority",
            BTreeSet::from(["yfinance".into()]),
            BTreeMap::from([("yfinance_api_key".into(), "secret-a".into())]),
        );
        let fmp = test_authority(
            &temporary,
            "authority",
            BTreeSet::from(["fmp".into()]),
            BTreeMap::from([("fmp_api_key".into(), "secret-b".into())]),
        );
        let key_a = pool_key("guru-a", "openbb", &yfinance);
        let key_other_guru = pool_key("guru-b", "openbb", &yfinance);
        let key_fmp = pool_key("guru-a", "openbb", &fmp);
        let capture = pooled_capture(&temporary, "authority", pool.clone());
        let (mut server, _transcript, exited) = spawn_isolated_turn_server_with_control(
            &temporary,
            "authority",
            BTreeSet::from(["yfinance".into()]),
            ("yfinance_api_key", "secret-a"),
            false,
            poolable_control_tools(),
        )
        .await;
        server.pool_key = Some(key_a.clone());
        capture
            .mcp_sessions
            .lock()
            .await
            .insert("openbb".into(), server);
        capture.shutdown_mcp().await;
        assert_eq!(pool.idle_count().await, 1);
        assert!(pool.acquire(&key_other_guru).await.is_none());
        assert!(pool.acquire(&key_fmp).await.is_none());
        assert!(!exited.is_file());
        let reused = pool
            .acquire(&key_a)
            .await
            .expect("matching authority should reuse");
        reused.discard().await;
        wait_for_exit_marker(&exited).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_same_guru_turns_do_not_share_one_pooled_mcp_process() {
        let _guard = crate::mcp::MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let pool = state.mcp_pool.clone();
        let authority = test_authority(
            &temporary,
            "concurrent",
            BTreeSet::from(["yfinance".into()]),
            BTreeMap::new(),
        );
        let key = pool_key("guru-a", "openbb", &authority);
        let capture = pooled_capture(&temporary, "concurrent", pool.clone());
        let (mut server, _transcript, exited) = spawn_isolated_turn_server_with_control(
            &temporary,
            "concurrent",
            BTreeSet::from(["yfinance".into()]),
            ("yfinance_api_key", "concurrent-secret"),
            false,
            poolable_control_tools(),
        )
        .await;
        server.pool_key = Some(key.clone());
        capture
            .mcp_sessions
            .lock()
            .await
            .insert("openbb".into(), server);
        capture.shutdown_mcp().await;

        let first = pool.acquire(&key);
        let second = pool.acquire(&key);
        let (first, second) = tokio::join!(first, second);
        let leased = match (first, second) {
            (Some(server), None) | (None, Some(server)) => server,
            (Some(_), Some(_)) => panic!("concurrent acquires shared one pooled MCP process"),
            (None, None) => panic!("pooled MCP process was not reusable"),
        };
        assert_eq!(pool.idle_count().await, 0);
        assert!(!exited.is_file());
        leased.discard().await;
        wait_for_exit_marker(&exited).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn idle_ttl_evicts_pooled_mcp_process() {
        let _guard = crate::mcp::MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("ttl-pool");
        crate::artifact_trust::ensure_private_directory(&root).unwrap();
        let pool = crate::mcp_pool::McpProcessPool::with_limits(root, Duration::from_millis(40), 4);
        let authority = test_authority(
            &temporary,
            "ttl",
            BTreeSet::from(["yfinance".into()]),
            BTreeMap::new(),
        );
        let key = pool_key("guru-a", "openbb", &authority);
        let capture = pooled_capture(&temporary, "ttl", pool.clone());
        let (mut server, _transcript, exited) = spawn_isolated_turn_server_with_control(
            &temporary,
            "ttl",
            BTreeSet::from(["yfinance".into()]),
            ("yfinance_api_key", "ttl-secret"),
            false,
            poolable_control_tools(),
        )
        .await;
        server.pool_key = Some(key.clone());
        capture
            .mcp_sessions
            .lock()
            .await
            .insert("openbb".into(), server);
        capture.shutdown_mcp().await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(pool.acquire(&key).await.is_none());
        wait_for_exit_marker(&exited).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pool_evicts_oldest_idle_when_at_cap() {
        let _guard = crate::mcp::MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cap-pool");
        crate::artifact_trust::ensure_private_directory(&root).unwrap();
        let pool = crate::mcp_pool::McpProcessPool::with_limits(root, Duration::from_secs(60), 1);
        let authority = test_authority(
            &temporary,
            "cap",
            BTreeSet::from(["yfinance".into()]),
            BTreeMap::new(),
        );
        let key = pool_key("guru-a", "openbb", &authority);
        let capture_a = pooled_capture(&temporary, "cap-a", pool.clone());
        let capture_b = pooled_capture(&temporary, "cap-b", pool.clone());
        let (mut server_a, _transcript_a, exited_a) = spawn_isolated_turn_server_with_control(
            &temporary,
            "cap-a",
            BTreeSet::from(["yfinance".into()]),
            ("yfinance_api_key", "cap-a"),
            false,
            poolable_control_tools(),
        )
        .await;
        let (mut server_b, _transcript_b, exited_b) = spawn_isolated_turn_server_with_control(
            &temporary,
            "cap-b",
            BTreeSet::from(["yfinance".into()]),
            ("yfinance_api_key", "cap-b"),
            false,
            poolable_control_tools(),
        )
        .await;
        server_a.pool_key = Some(key.clone());
        server_b.pool_key = Some(key.clone());
        capture_a
            .mcp_sessions
            .lock()
            .await
            .insert("openbb".into(), server_a);
        capture_b
            .mcp_sessions
            .lock()
            .await
            .insert("openbb".into(), server_b);
        capture_a.shutdown_mcp().await;
        capture_b.shutdown_mcp().await;
        wait_for_exit_marker(&exited_a).await;
        assert!(!exited_b.is_file());
        assert_eq!(pool.idle_count().await, 1);
        let kept = pool
            .acquire(&key)
            .await
            .expect("newest idle process should remain");
        kept.discard().await;
        wait_for_exit_marker(&exited_b).await;
    }
}
