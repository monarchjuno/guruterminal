use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter},
    process::{Child, ChildStdin, Command},
    sync::{oneshot, Mutex},
    task::JoinHandle,
    time::{sleep, timeout, Instant},
};
use uuid::Uuid;

#[cfg(windows)]
use crate::process_lease::ChildProcessJob;
#[cfg(unix)]
use crate::process_lease::{
    signal_process_group, terminate_and_reap_process_group, wait_for_process_group_exit,
    ChildProcessLease, ProcessKind,
};
use crate::{
    artifact_trust::{digest_bounded_regular_file, verify_executable},
    process_lease::ProcessLeaseError,
};

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_BOOTSTRAP_BYTES: usize = 64 * 1024;
const MAX_TOOLS: usize = 512;
const MAX_TOOL_LIST_PAGES: usize = 64;
const MAX_TOOL_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_TOOL_DESCRIPTOR_BYTES: usize = 128 * 1024;
const MAX_TOOL_INVENTORY_BYTES: usize = 8 * 1024 * 1024;
const MAX_CURSOR_BYTES: usize = 512;
const MAX_TOOL_NAME_BYTES: usize = 128;
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(unix)]
const PROCESS_IDENTITY_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(unix)]
const PROCESS_IDENTITY_SETTLE_INTERVAL: Duration = Duration::from_millis(10);
const MAX_RUNTIME_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_RUNTIME_LOCK_BYTES: u64 = 32 * 1024 * 1024;

#[cfg(all(test, unix))]
pub(crate) static MCP_PROCESS_TEST_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Clone, Debug)]
pub struct BundledMcpRuntime {
    pub server_id: String,
    pub executable: PathBuf,
    pub runtime_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub lease_dir: PathBuf,
    pub allowed_categories: Vec<String>,
    pub provider_ids: BTreeSet<String>,
    pub provider_network_hosts: BTreeMap<String, BTreeSet<String>>,
    pub provider_config_fields: BTreeMap<String, BTreeSet<String>>,
    pub providerless_tool_policy: ProviderlessToolPolicy,
    pub provider_receipt_pointer: String,
    pub tool_activation: Option<McpToolActivation>,
    pub control_tool_names: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderlessToolPolicy {
    pub local_tools: BTreeSet<String>,
    pub implicit_provider: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct McpToolActivation {
    pub tool_name: String,
    pub argument_name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeManifestIdentity {
    schema_version: String,
    runtime_id: String,
    executable: String,
    #[serde(default)]
    python: Option<String>,
    uv_lock_sha256: String,
    #[serde(default)]
    packages: BTreeMap<String, String>,
    protocol: RuntimeProtocolManifest,
    security: RuntimeSecurityManifest,
    providerless_tool_policy: RuntimeProviderlessToolPolicy,
    allowed_categories: Vec<String>,
    providers: Vec<RuntimeProviderManifest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeProtocolManifest {
    transport: String,
    bootstrap_type: String,
    bootstrap_version: u64,
    #[serde(default)]
    bootstrap_max_bytes: Option<u64>,
    #[serde(default)]
    initial_tools: Option<String>,
    provider_receipt_pointer: String,
    #[serde(default)]
    tool_activation: Option<RuntimeToolActivation>,
    control_tool_names: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeToolActivation {
    tool_name: String,
    argument_name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeSecurityManifest {
    read_only: bool,
    #[serde(default)]
    allowed_http_methods: Vec<String>,
    #[serde(default)]
    read_only_post_routes: Vec<String>,
    #[serde(default)]
    disabled_surfaces: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeProviderManifest {
    id: String,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default, rename = "keyless")]
    _keyless: Option<bool>,
    #[serde(default)]
    credential_mapping: BTreeMap<String, String>,
    #[serde(default)]
    config_mapping: BTreeMap<String, String>,
    network_hosts: Vec<String>,
    #[serde(default)]
    verification_probe: Option<RuntimeVerificationProbe>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeVerificationProbe {
    tool: String,
    arguments: Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeProviderlessToolPolicy {
    local_tools: Vec<String>,
    implicit_provider: BTreeMap<String, String>,
}

type ValidatedRuntimeManifest = (
    Vec<String>,
    BTreeSet<String>,
    BTreeMap<String, BTreeSet<String>>,
    BTreeMap<String, BTreeSet<String>>,
    String,
    ProviderlessToolPolicy,
    String,
    Option<McpToolActivation>,
    BTreeSet<String>,
);

/// Discovers only bundled runtimes carrying an explicit manifest. Unknown
/// directories in pi-runtime are ignored; malformed manifests fail closed for
/// that runtime and never become executable capability inventory.
pub fn discover_bundled_runtimes(
    root: &Path,
    lease_dir: &Path,
) -> BTreeMap<String, BundledMcpRuntime> {
    let mut runtimes = BTreeMap::new();
    let Ok(entries) = fs::read_dir(root) else {
        return runtimes;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let runtime_dir = entry.path();
        let manifest_path = runtime_dir.join("runtime-manifest.json");
        let Ok(metadata) = fs::symlink_metadata(&manifest_path) else {
            continue;
        };
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() == 0
            || metadata.len() > MAX_RUNTIME_MANIFEST_BYTES
        {
            continue;
        }
        let Ok(encoded) = fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<RuntimeManifestIdentity>(&encoded) else {
            continue;
        };
        let Some((
            allowed_categories,
            provider_ids,
            provider_network_hosts,
            provider_config_fields,
            lock_digest,
            providerless_tool_policy,
            provider_receipt_pointer,
            tool_activation,
            control_tool_names,
        )) = validate_runtime_manifest(&manifest)
        else {
            continue;
        };
        if manifest.schema_version != "guruterminal-mcp-runtime/1"
            || validate_server_id(&manifest.runtime_id).is_err()
            || manifest.executable.is_empty()
            || manifest.executable.len() > 128
            || manifest.executable.contains(['/', '\\', '\0'])
        {
            continue;
        }
        let executable_name = if std::env::consts::EXE_SUFFIX.is_empty()
            || manifest.executable.ends_with(std::env::consts::EXE_SUFFIX)
        {
            manifest.executable.clone()
        } else {
            format!("{}{}", manifest.executable, std::env::consts::EXE_SUFFIX)
        };
        let executable = runtime_dir.join(executable_name);
        let lock_path = runtime_dir.join("uv.lock");
        if !executable.is_file()
            || !matches!(
                digest_bounded_regular_file(&lock_path, MAX_RUNTIME_LOCK_BYTES),
                Ok(actual) if actual == lock_digest
            )
            || runtimes.contains_key(&manifest.runtime_id)
        {
            continue;
        }
        runtimes.insert(
            manifest.runtime_id.clone(),
            BundledMcpRuntime {
                server_id: manifest.runtime_id,
                executable,
                runtime_dir,
                manifest_path,
                lease_dir: lease_dir.to_path_buf(),
                allowed_categories,
                provider_ids,
                provider_network_hosts,
                provider_config_fields,
                providerless_tool_policy,
                provider_receipt_pointer,
                tool_activation,
                control_tool_names,
            },
        );
    }
    runtimes
}

fn validate_runtime_manifest(
    manifest: &RuntimeManifestIdentity,
) -> Option<ValidatedRuntimeManifest> {
    if manifest.protocol.transport != "stdio"
        || manifest.protocol.bootstrap_type != "guruterminal.bootstrap"
        || manifest.protocol.bootstrap_version != 1
        || manifest
            .protocol
            .bootstrap_max_bytes
            .is_some_and(|limit| limit == 0 || limit > MAX_BOOTSTRAP_BYTES as u64)
        || manifest.protocol.initial_tools.as_deref() != Some("admin_only")
        || !valid_json_pointer(&manifest.protocol.provider_receipt_pointer)
        || manifest
            .protocol
            .tool_activation
            .as_ref()
            .is_some_and(|activation| {
                !valid_dynamic_tool_name(&activation.tool_name)
                    || !valid_schema_property_name(&activation.argument_name)
            })
    {
        return None;
    }
    let control_tool_names = manifest
        .protocol
        .control_tool_names
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if control_tool_names.is_empty()
        || control_tool_names.len() != manifest.protocol.control_tool_names.len()
        || control_tool_names.len() > 32
        || control_tool_names
            .iter()
            .any(|name| !valid_dynamic_tool_name(name))
        || manifest
            .protocol
            .tool_activation
            .as_ref()
            .is_some_and(|activation| !control_tool_names.contains(&activation.tool_name))
    {
        return None;
    }
    if !manifest.security.read_only
        || manifest
            .security
            .allowed_http_methods
            .iter()
            .any(|method| !matches!(method.as_str(), "GET" | "POST"))
        || manifest
            .security
            .read_only_post_routes
            .iter()
            .any(|route| route.is_empty() || route.len() > 256 || !route.starts_with('/'))
        || manifest
            .security
            .disabled_surfaces
            .iter()
            .any(|surface| surface.is_empty() || surface.len() > 64)
    {
        return None;
    }
    if manifest
        .python
        .as_ref()
        .is_some_and(|version| version.is_empty() || version.len() > 32)
        || manifest.packages.len() > 256
        || manifest.packages.iter().any(|(package, version)| {
            package.is_empty() || package.len() > 128 || version.is_empty() || version.len() > 64
        })
    {
        return None;
    }
    let lock_digest = manifest.uv_lock_sha256.as_str();
    if lock_digest.len() != 64
        || !lock_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return None;
    }
    let allowed_categories = &manifest.allowed_categories;
    if allowed_categories.is_empty()
        || allowed_categories.len() > 128
        || allowed_categories.iter().any(|value| {
            value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
        || allowed_categories.iter().collect::<BTreeSet<_>>().len() != allowed_categories.len()
    {
        return None;
    }
    let mut provider_ids = BTreeSet::new();
    let mut provider_network_hosts = BTreeMap::new();
    let mut provider_config_fields = BTreeMap::new();
    for provider in &manifest.providers {
        let id = provider.id.as_str();
        let network_hosts = provider
            .network_hosts
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if validate_server_id(id).is_err()
            || !provider_ids.insert(id.to_owned())
            || network_hosts.is_empty()
            || network_hosts.len() != provider.network_hosts.len()
            || network_hosts.len() > 128
            || network_hosts.iter().any(|host| !valid_network_host(host))
            || provider
                .package
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > 128)
            || provider
                .version
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > 64)
            || provider.credential_mapping.len() > 32
            || provider.config_mapping.len() > 32
            || provider
                .credential_mapping
                .iter()
                .chain(provider.config_mapping.iter())
                .any(|(source, target)| {
                    source.is_empty()
                        || source.len() > 128
                        || target.is_empty()
                        || target.len() > 128
                })
            || provider.verification_probe.as_ref().is_some_and(|probe| {
                probe.tool.is_empty()
                    || probe.tool.len() > MAX_TOOL_NAME_BYTES
                    || probe.arguments.len() > 128
            })
        {
            return None;
        }
        provider_network_hosts.insert(id.to_owned(), network_hosts);
        provider_config_fields.insert(
            id.to_owned(),
            provider.config_mapping.keys().cloned().collect(),
        );
    }
    if provider_ids.is_empty() || provider_ids.len() > 128 {
        return None;
    }
    let local_tools = manifest
        .providerless_tool_policy
        .local_tools
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let implicit_provider = &manifest.providerless_tool_policy.implicit_provider;
    if local_tools.len() != manifest.providerless_tool_policy.local_tools.len()
        || local_tools.len() + implicit_provider.len() > MAX_TOOLS
        || local_tools
            .iter()
            .chain(implicit_provider.keys())
            .any(|name| !valid_dynamic_tool_name(name))
        || implicit_provider
            .values()
            .any(|provider| !provider_ids.contains(provider))
        || local_tools
            .iter()
            .any(|name| implicit_provider.contains_key(name))
    {
        return None;
    }
    Some((
        allowed_categories.clone(),
        provider_ids,
        provider_network_hosts,
        provider_config_fields,
        lock_digest.to_owned(),
        ProviderlessToolPolicy {
            local_tools,
            implicit_provider: implicit_provider.clone(),
        },
        manifest.protocol.provider_receipt_pointer.clone(),
        manifest
            .protocol
            .tool_activation
            .as_ref()
            .map(|activation| McpToolActivation {
                tool_name: activation.tool_name.clone(),
                argument_name: activation.argument_name.clone(),
            }),
        control_tool_names,
    ))
}

fn valid_network_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.contains('*')
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}

#[derive(Debug, Error)]
pub enum McpError {
    #[error("MCP server executable is missing")]
    MissingExecutable,
    #[error("MCP server executable is not a trusted app artifact")]
    UntrustedExecutable,
    #[error("MCP server I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("MCP protocol failed")]
    Protocol,
    #[error("MCP protocol version is unsupported")]
    VersionMismatch,
    #[error("MCP request timed out")]
    Timeout,
    #[error("MCP server rejected the request: {0}")]
    Remote(String),
    #[error("MCP server stopped")]
    Stopped,
    #[error("MCP frame exceeded the size limit")]
    FrameTooLarge,
    #[error("MCP tool inventory exceeded the size limit")]
    ToolLimit,
    #[error("MCP process ownership failed: {0}")]
    Lease(#[from] ProcessLeaseError),
}

#[derive(Clone)]
pub struct McpLaunchConfig {
    pub server_id: String,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub private_working_dir: PathBuf,
    pub lease_dir: PathBuf,
    /// Non-secret environment required by a bundled runtime. Credentials must
    /// be sent in `bootstrap` instead of process arguments or environment.
    pub environment: BTreeMap<String, String>,
    pub bootstrap: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(
        default,
        rename = "outputSchema",
        skip_serializing_if = "Option::is_none"
    )]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpCallResult {
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(default, rename = "structuredContent")]
    pub structured_content: Option<Value>,
    #[serde(default, rename = "isError")]
    pub is_error: bool,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, McpError>>>>>;
type McpStdin = Arc<Mutex<BufWriter<ChildStdin>>>;

/// A broker peer can cancel by disconnecting, which drops the in-flight MCP
/// future. This guard removes the orphaned response slot and sends the MCP
/// cancellation notification even when normal timeout handling never runs.
struct PendingRequestCancellation {
    id: Option<String>,
    pending: Pending,
    stdin: McpStdin,
}

impl PendingRequestCancellation {
    fn disarm(&mut self) {
        self.id.take();
    }
}

impl Drop for PendingRequestCancellation {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let pending = self.pending.clone();
        let stdin = self.stdin.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            pending.lock().await.remove(&id);
            let notification = json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": id },
            });
            if let Ok(encoded) = encode_line(&notification) {
                let _ = write_bytes(&stdin, &encoded, WRITE_TIMEOUT).await;
            }
        });
    }
}

pub struct McpSession {
    server_id: String,
    child: Mutex<Child>,
    #[cfg(unix)]
    process_group_id: i32,
    #[cfg(unix)]
    lease: Option<ChildProcessLease>,
    #[cfg(windows)]
    job: Option<ChildProcessJob>,
    stdin: McpStdin,
    pending: Pending,
    tools_changed: Arc<AtomicBool>,
    reader: JoinHandle<()>,
}

impl McpSession {
    pub async fn spawn(config: McpLaunchConfig) -> Result<(Self, Vec<McpTool>), McpError> {
        if !config.executable.is_file() {
            return Err(McpError::MissingExecutable);
        }
        validate_server_id(&config.server_id)?;
        let _verified_executable =
            verify_executable(&config.executable).map_err(|_| McpError::UntrustedExecutable)?;
        std::fs::create_dir_all(&config.private_working_dir)?;

        let mut command = Command::new(&config.executable);
        command
            .args(&config.arguments)
            .current_dir(&config.private_working_dir)
            .env_clear()
            .env("LANG", "C.UTF-8")
            .env("PYTHONHASHSEED", "0")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PYTHONNOUSERSITE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for (key, value) in &config.environment {
            if !valid_environment_name(key) || value.contains('\0') {
                return Err(McpError::Protocol);
            }
            command.env(key, value);
        }
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        ChildProcessJob::configure_command(&mut command);

        let mut child = command.spawn()?;
        #[cfg(unix)]
        let process_group_id = child.id().ok_or(McpError::Protocol)? as i32;
        #[cfg(windows)]
        let job = match ChildProcessJob::assign(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.start_kill();
                let _ = timeout(Duration::from_secs(2), child.wait()).await;
                return Err(error.into());
            }
        };
        let stdin = child.stdin.take().ok_or(McpError::Protocol)?;
        let stdout = child.stdout.take().ok_or(McpError::Protocol)?;
        #[cfg(unix)]
        let lease = match register_process_lease(
            &config.lease_dir,
            process_group_id,
            &config.executable,
            &mut child,
        )
        .await
        {
            Ok(lease) => lease,
            Err(error) => {
                let _ = signal_process_group(process_group_id, libc::SIGKILL);
                let _ = timeout(Duration::from_secs(2), child.wait()).await;
                return Err(error.into());
            }
        };

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let reader_pending = pending.clone();
        let stdin = Arc::new(Mutex::new(BufWriter::new(stdin)));
        let reader_stdin = stdin.clone();
        let tools_changed = Arc::new(AtomicBool::new(false));
        let reader_tools_changed = tools_changed.clone();
        let reader = tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            loop {
                match read_frame(&mut stdout).await {
                    Ok(Some(frame)) => {
                        if dispatch_frame(
                            &frame,
                            &reader_pending,
                            &reader_tools_changed,
                            &reader_stdin,
                        )
                        .await
                        .is_err()
                        {
                            fail_pending(&reader_pending, PendingFailure::Protocol).await;
                            break;
                        }
                    }
                    Ok(None) => {
                        fail_pending(&reader_pending, PendingFailure::Stopped).await;
                        break;
                    }
                    Err(_) => {
                        fail_pending(&reader_pending, PendingFailure::Protocol).await;
                        break;
                    }
                }
            }
        });
        let session = Self {
            server_id: config.server_id,
            child: Mutex::new(child),
            #[cfg(unix)]
            process_group_id,
            #[cfg(unix)]
            lease: Some(lease),
            #[cfg(windows)]
            job: Some(job),
            stdin,
            pending,
            tools_changed,
            reader,
        };

        if let Err(error) = session.write_bootstrap(config.bootstrap).await {
            let _ = session.shutdown(Duration::from_secs(1)).await;
            return Err(error);
        }
        let initialized = session
            .call(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "Guru Terminal", "version": env!("CARGO_PKG_VERSION") }
                }),
                STARTUP_TIMEOUT,
            )
            .await;
        match initialized {
            Ok(value) if valid_initialize_result(&value) => {}
            Ok(_) => {
                let _ = session.shutdown(Duration::from_secs(1)).await;
                return Err(McpError::VersionMismatch);
            }
            Err(error) => {
                let _ = session.shutdown(Duration::from_secs(1)).await;
                return Err(error);
            }
        }
        session
            .notify("notifications/initialized", json!({}))
            .await?;
        let tools = match session.list_tools().await {
            Ok(tools) => tools,
            Err(error) => {
                let _ = session.shutdown(Duration::from_secs(1)).await;
                return Err(error);
            }
        };
        Ok((session, tools))
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    pub fn take_tools_changed(&self) -> bool {
        self.tools_changed.swap(false, Ordering::AcqRel)
    }

    pub async fn is_running(&self) -> Result<bool, McpError> {
        if self.reader.is_finished() {
            return Ok(false);
        }
        Ok(self.child.lock().await.try_wait()?.is_none())
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        timeout(LIST_TIMEOUT, async {
            let mut tools = Vec::new();
            let mut cursor: Option<String> = None;
            let mut seen_cursors = BTreeSet::new();
            let mut inventory_bytes = 0_usize;
            for _ in 0..MAX_TOOL_LIST_PAGES {
                let params = cursor
                    .as_ref()
                    .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
                let value = self.call("tools/list", params, LIST_TIMEOUT).await?;
                let object = value.as_object().ok_or(McpError::Protocol)?;
                let page = object
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or(McpError::Protocol)?;
                if page.is_empty() && object.get("nextCursor").is_some() {
                    return Err(McpError::Protocol);
                }
                for value in page {
                    accumulate_tool_inventory_bytes(&mut inventory_bytes, value)?;
                    let tool: McpTool =
                        serde_json::from_value(value.clone()).map_err(|_| McpError::Protocol)?;
                    validate_tool(&tool)?;
                    if tools
                        .iter()
                        .any(|existing: &McpTool| existing.name == tool.name)
                    {
                        return Err(McpError::Protocol);
                    }
                    tools.push(tool);
                    if tools.len() > MAX_TOOLS {
                        return Err(McpError::ToolLimit);
                    }
                }
                cursor = match object.get("nextCursor") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(value))
                        if !value.is_empty() && value.len() <= MAX_CURSOR_BYTES =>
                    {
                        if !seen_cursors.insert(value.clone()) {
                            return Err(McpError::Protocol);
                        }
                        Some(value.clone())
                    }
                    _ => return Err(McpError::Protocol),
                };
                if cursor.is_none() {
                    return Ok(tools);
                }
            }
            Err(McpError::ToolLimit)
        })
        .await
        .map_err(|_| McpError::Timeout)?
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        deadline: Duration,
    ) -> Result<McpCallResult, McpError> {
        validate_tool_name(name)?;
        if !arguments.is_object() {
            return Err(McpError::Protocol);
        }
        let value = self
            .call(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                deadline,
            )
            .await?;
        let result: McpCallResult =
            serde_json::from_value(value).map_err(|_| McpError::Protocol)?;
        let encoded = serde_json::to_vec(&result).map_err(|_| McpError::Protocol)?;
        if encoded.len() > MAX_FRAME_BYTES {
            return Err(McpError::FrameTooLarge);
        }
        Ok(result)
    }

    async fn write_bootstrap(&self, bootstrap: Value) -> Result<(), McpError> {
        let object = bootstrap.as_object().ok_or(McpError::Protocol)?;
        if object.get("type").and_then(Value::as_str) != Some("guruterminal.bootstrap") {
            return Err(McpError::Protocol);
        }
        if serde_json::to_vec(&bootstrap)
            .map_err(|_| McpError::Protocol)?
            .len()
            > MAX_BOOTSTRAP_BYTES
        {
            return Err(McpError::FrameTooLarge);
        }
        self.write_value(&bootstrap, WRITE_TIMEOUT).await
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, McpError> {
        let started = Instant::now();
        if !matches!(method, "initialize" | "tools/list" | "tools/call") {
            return Err(McpError::Protocol);
        }
        let id = Uuid::new_v4().simple().to_string();
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let encoded = encode_line(&request)?;
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), sender);
        let mut cancellation = PendingRequestCancellation {
            id: Some(id.clone()),
            pending: self.pending.clone(),
            stdin: self.stdin.clone(),
        };
        if let Err(error) = write_bytes(&self.stdin, &encoded, deadline.min(WRITE_TIMEOUT)).await {
            self.pending.lock().await.remove(&id);
            cancellation.disarm();
            return Err(error);
        }
        let remaining = deadline.saturating_sub(started.elapsed());
        let result = match timeout(remaining, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(McpError::Stopped),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                let _ = self
                    .notify("notifications/cancelled", json!({ "requestId": id }))
                    .await;
                Err(McpError::Timeout)
            }
        };
        cancellation.disarm();
        result
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        if !matches!(
            method,
            "notifications/initialized" | "notifications/cancelled"
        ) {
            return Err(McpError::Protocol);
        }
        self.write_value(
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
            WRITE_TIMEOUT,
        )
        .await
    }

    async fn write_value(&self, value: &Value, deadline: Duration) -> Result<(), McpError> {
        let encoded = encode_line(value)?;
        write_bytes(&self.stdin, &encoded, deadline).await
    }

    pub async fn shutdown(mut self, grace: Duration) -> Result<(), McpError> {
        {
            let mut stdin = self.stdin.lock().await;
            let _ = stdin.shutdown().await;
        }
        let deadline = Instant::now() + grace;
        let mut child = self.child.lock().await;
        let mut child_exited = false;
        while Instant::now() < deadline {
            match child.try_wait()? {
                Some(_) => {
                    child_exited = true;
                    break;
                }
                None => sleep(Duration::from_millis(25)).await,
            }
        }
        let graceful: Result<(), McpError> = async {
            #[cfg(unix)]
            {
                signal_process_group(self.process_group_id, libc::SIGTERM)?;
                if !child_exited {
                    timeout(Duration::from_secs(2), child.wait())
                        .await
                        .map_err(|_| McpError::Timeout)??;
                }
                timeout(
                    Duration::from_secs(2),
                    wait_for_process_group_exit(self.process_group_id),
                )
                .await
                .map_err(|_| McpError::Timeout)??;
            }
            #[cfg(windows)]
            {
                if let Some(job) = &self.job {
                    job.terminate_and_wait(Duration::from_secs(2)).await?;
                }
                if !child_exited {
                    timeout(Duration::from_secs(2), child.wait())
                        .await
                        .map_err(|_| McpError::Timeout)??;
                }
            }
            #[cfg(not(any(unix, windows)))]
            if !child_exited {
                child.start_kill()?;
                timeout(Duration::from_secs(2), child.wait())
                    .await
                    .map_err(|_| McpError::Timeout)??;
            }
            Ok(())
        }
        .await;
        let stopped = if graceful.is_ok() {
            graceful
        } else {
            async {
                #[cfg(unix)]
                {
                    signal_process_group(self.process_group_id, libc::SIGKILL)?;
                    let _ = timeout(Duration::from_secs(2), child.wait()).await;
                    timeout(
                        Duration::from_secs(2),
                        wait_for_process_group_exit(self.process_group_id),
                    )
                    .await
                    .map_err(|_| McpError::Timeout)??;
                }
                #[cfg(windows)]
                {
                    if let Some(job) = &self.job {
                        job.terminate_and_wait(Duration::from_secs(2)).await?;
                    } else if child.try_wait()?.is_none() {
                        child.start_kill()?;
                    }
                    if child.try_wait()?.is_none() {
                        timeout(Duration::from_secs(2), child.wait())
                            .await
                            .map_err(|_| McpError::Timeout)??;
                    }
                }
                #[cfg(not(any(unix, windows)))]
                if child.try_wait()?.is_none() {
                    child.start_kill()?;
                    timeout(Duration::from_secs(2), child.wait())
                        .await
                        .map_err(|_| McpError::Timeout)??;
                }
                Ok(())
            }
            .await
        };
        drop(child);
        self.reader.abort();
        let _ = timeout(Duration::from_secs(2), &mut self.reader).await;
        #[cfg(windows)]
        self.job.take();
        stopped?;
        #[cfg(unix)]
        if let Some(lease) = self.lease.take() {
            lease.complete()?;
        }
        Ok(())
    }
}

#[cfg(unix)]
async fn register_process_lease(
    lease_dir: &Path,
    process_group_id: i32,
    executable: &Path,
    child: &mut Child,
) -> Result<ChildProcessLease, ProcessLeaseError> {
    let started = Instant::now();
    loop {
        match ChildProcessLease::register(
            lease_dir,
            ProcessKind::Mcp,
            process_group_id,
            process_group_id,
            executable,
        ) {
            Err(ProcessLeaseError::IdentityMismatch)
                if started.elapsed() < PROCESS_IDENTITY_SETTLE_TIMEOUT =>
            {
                if child.try_wait()?.is_some() {
                    return Err(ProcessLeaseError::IdentityMismatch);
                }
                sleep(PROCESS_IDENTITY_SETTLE_INTERVAL).await;
            }
            outcome => return outcome,
        }
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        self.reader.abort();
        #[cfg(unix)]
        {
            if let Some(lease) = self.lease.take() {
                terminate_and_reap_process_group(self.process_group_id, lease);
            } else {
                let _ = signal_process_group(self.process_group_id, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            let _ = job.terminate();
        }
    }
}

fn valid_initialize_result(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let Some(protocol) = object.get("protocolVersion").and_then(Value::as_str) else {
        return false;
    };
    protocol == MCP_PROTOCOL_VERSION
        && object.get("capabilities").is_some_and(Value::is_object)
        && object
            .get("serverInfo")
            .and_then(Value::as_object)
            .and_then(|info| info.get("name"))
            .and_then(Value::as_str)
            .is_some_and(|name| !name.trim().is_empty() && name.len() <= 128)
}

fn validate_server_id(value: &str) -> Result<(), McpError> {
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
        })
    {
        return Err(McpError::Protocol);
    }
    Ok(())
}

fn validate_tool(tool: &McpTool) -> Result<(), McpError> {
    validate_tool_name(&tool.name)?;
    validate_input_schema(&tool.input_schema)?;
    if serde_json::to_vec(tool)
        .map_err(|_| McpError::Protocol)?
        .len()
        > MAX_TOOL_DESCRIPTOR_BYTES
    {
        return Err(McpError::ToolLimit);
    }
    if tool
        .title
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > 160)
        || tool
            .description
            .as_ref()
            .is_some_and(|value| value.len() > 4_096)
        || tool.output_schema.as_ref().is_some_and(|schema| {
            !schema.is_object()
                || serde_json::to_vec(schema)
                    .map_or(true, |encoded| encoded.len() > MAX_TOOL_SCHEMA_BYTES)
        })
        || tool
            .annotations
            .as_ref()
            .is_some_and(|annotations| !annotations.is_object())
        || tool
            .metadata
            .as_ref()
            .is_some_and(|metadata| !metadata.is_object())
        || serde_json::to_vec(&tool.input_schema)
            .map_err(|_| McpError::Protocol)?
            .len()
            > MAX_TOOL_SCHEMA_BYTES
    {
        return Err(McpError::Protocol);
    }
    Ok(())
}

fn accumulate_tool_inventory_bytes(total: &mut usize, value: &Value) -> Result<(), McpError> {
    let descriptor_bytes = serde_json::to_vec(value)
        .map_err(|_| McpError::Protocol)?
        .len();
    if descriptor_bytes > MAX_TOOL_DESCRIPTOR_BYTES {
        return Err(McpError::ToolLimit);
    }
    *total = total
        .checked_add(descriptor_bytes)
        .filter(|total| *total <= MAX_TOOL_INVENTORY_BYTES)
        .ok_or(McpError::ToolLimit)?;
    Ok(())
}

fn validate_input_schema(schema: &Value) -> Result<(), McpError> {
    let schema = schema.as_object().ok_or(McpError::Protocol)?;
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(McpError::Protocol);
    }
    let properties = match schema.get("properties") {
        None => None,
        Some(Value::Object(properties)) => Some(properties),
        Some(_) => return Err(McpError::Protocol),
    };
    if let Some(provider) = properties.and_then(|properties| properties.get("provider")) {
        if !provider.is_object() {
            return Err(McpError::Protocol);
        }
    }
    if let Some(required) = schema.get("required") {
        let required = required.as_array().ok_or(McpError::Protocol)?;
        let properties = properties.ok_or(McpError::Protocol)?;
        let mut unique = BTreeSet::new();
        for value in required {
            let name = value.as_str().ok_or(McpError::Protocol)?;
            if !unique.insert(name) || !properties.contains_key(name) {
                return Err(McpError::Protocol);
            }
        }
    }
    Ok(())
}

fn validate_tool_name(value: &str) -> Result<(), McpError> {
    if value.is_empty()
        || value.len() > MAX_TOOL_NAME_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/'))
    {
        return Err(McpError::Protocol);
    }
    Ok(())
}

fn valid_dynamic_tool_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOOL_NAME_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_schema_property_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_json_pointer(value: &str) -> bool {
    crate::json_pointer::valid_json_pointer(value, 512, false)
}

fn valid_environment_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_uppercase() || byte == b'_' || (index > 0 && byte.is_ascii_digit())
        })
}

fn encode_line(value: &Value) -> Result<Vec<u8>, McpError> {
    let mut encoded = serde_json::to_vec(value).map_err(|_| McpError::Protocol)?;
    if encoded.len() > MAX_FRAME_BYTES {
        return Err(McpError::FrameTooLarge);
    }
    encoded.push(b'\n');
    Ok(encoded)
}

async fn write_bytes<W>(writer: &Mutex<W>, bytes: &[u8], deadline: Duration) -> Result<(), McpError>
where
    W: AsyncWrite + Unpin,
{
    timeout(deadline, async {
        let mut writer = writer.lock().await;
        writer.write_all(bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| McpError::Timeout)??;
    Ok(())
}

async fn dispatch_frame<W>(
    frame: &[u8],
    pending: &Pending,
    tools_changed: &AtomicBool,
    stdin: &Mutex<W>,
) -> Result<(), McpError>
where
    W: AsyncWrite + Unpin,
{
    let value: Value = serde_json::from_slice(frame).map_err(|_| McpError::Protocol)?;
    let object = value.as_object().ok_or(McpError::Protocol)?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(McpError::Protocol);
    }
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        match method {
            "notifications/tools/list_changed" if !object.contains_key("id") => {
                tools_changed.store(true, Ordering::Release);
                return Ok(());
            }
            "ping" => {
                let id = object.get("id").ok_or(McpError::Protocol)?;
                let valid_id = match id {
                    Value::String(value) => {
                        !value.is_empty()
                            && value.len() <= 256
                            && !value.contains(['\n', '\r', '\0'])
                    }
                    Value::Number(value) => value.is_i64() || value.is_u64(),
                    _ => false,
                };
                if !valid_id
                    || object
                        .get("params")
                        .is_some_and(|params| !params.is_null() && !params.is_object())
                {
                    return Err(McpError::Protocol);
                }
                let response = encode_line(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {}
                }))?;
                write_bytes(stdin, &response, WRITE_TIMEOUT).await?;
                return Ok(());
            }
            // Official OpenBB still emits resources/prompts notifications even
            // when those surfaces are disabled. Treat unknown notifications as
            // no-ops so a successful activate cannot kill the stdio reader.
            method if method.starts_with("notifications/") && !object.contains_key("id") => {
                return Ok(());
            }
            _ => return Err(McpError::Protocol),
        }
    }
    let id = match object.get("id") {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        _ => return Err(McpError::Protocol),
    };
    if let Some(sender) = pending.lock().await.remove(&id) {
        let result = match (object.get("result"), object.get("error")) {
            (Some(result), None) => Ok(result.clone()),
            (None, Some(error)) => {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("MCP server rejected the request")
                    .chars()
                    .take(300)
                    .collect();
                Err(McpError::Remote(message))
            }
            _ => Err(McpError::Protocol),
        };
        let _ = sender.send(result);
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum PendingFailure {
    Stopped,
    Protocol,
}

async fn fail_pending(pending: &Pending, failure: PendingFailure) {
    let entries = pending.lock().await.drain().collect::<Vec<_>>();
    for (_, sender) in entries {
        let error = match failure {
            PendingFailure::Stopped => McpError::Stopped,
            PendingFailure::Protocol => McpError::Protocol,
        };
        let _ = sender.send(Err(error));
    }
}

async fn read_frame<R>(reader: &mut R) -> Result<Option<Vec<u8>>, McpError>
where
    R: AsyncBufRead + Unpin,
{
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(McpError::Protocol)
            };
        }
        let (take, complete) = match available.iter().position(|byte| *byte == b'\n') {
            Some(position) => (position, true),
            None => (available.len(), false),
        };
        if frame.len() + take > MAX_FRAME_BYTES {
            return Err(McpError::FrameTooLarge);
        }
        frame.extend_from_slice(&available[..take]);
        reader.consume(take + usize::from(complete));
        if complete {
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            return if frame.is_empty() {
                Err(McpError::Protocol)
            } else {
                Ok(Some(frame))
            };
        }
    }
}

pub fn tool_to_agent_schema(server_id: &str, tool: &McpTool) -> Result<Value, McpError> {
    validate_server_id(server_id)?;
    validate_tool(tool)?;
    if !valid_dynamic_tool_name(&tool.name) {
        return Err(McpError::Protocol);
    }
    let dynamic_name = format!("mcp__{}__{}", server_id.replace(['.', '-'], "_"), tool.name);
    if dynamic_name.len() > 240 {
        return Err(McpError::Protocol);
    }
    let mut object = Map::new();
    object.insert("name".into(), Value::String(dynamic_name));
    object.insert("mcp_name".into(), Value::String(tool.name.clone()));
    object.insert("server_id".into(), Value::String(server_id.to_owned()));
    object.insert(
        "label".into(),
        Value::String(tool.title.clone().unwrap_or_else(|| tool.name.clone())),
    );
    object.insert(
        "description".into(),
        Value::String(tool.description.clone().unwrap_or_default()),
    );
    object.insert("parameters".into(), tool.input_schema.clone());
    Ok(Value::Object(object))
}

/// Narrows a provider-aware MCP schema to the providers granted for this run.
/// The original server schema is never widened. Tools with no usable provider
/// remain hidden, while provider-neutral tools pass through unchanged.
pub fn filter_tool_providers(
    tool: &McpTool,
    enabled_provider_ids: &BTreeSet<String>,
    control_tool: bool,
) -> Result<Option<McpTool>, McpError> {
    validate_tool(tool)?;
    if !control_tool {
        let explicitly_read_only = tool.annotations.as_ref().is_some_and(|annotations| {
            annotations.get("readOnlyHint").and_then(Value::as_bool) == Some(true)
                && annotations.get("destructiveHint").and_then(Value::as_bool) != Some(true)
        });
        if !explicitly_read_only {
            return Ok(None);
        }
    }
    let mut filtered = tool.clone();
    let Some(schema) = filtered.input_schema.as_object_mut() else {
        return Err(McpError::Protocol);
    };
    let properties = match schema.get_mut("properties") {
        None => return Ok(Some(filtered)),
        Some(Value::Object(properties)) => properties,
        Some(_) => return Err(McpError::Protocol),
    };
    let provider_schema = match properties.get_mut("provider") {
        None => return Ok(Some(filtered)),
        Some(Value::Object(provider_schema)) => provider_schema,
        Some(_) => return Err(McpError::Protocol),
    };
    // Never widen a provider property merely because the server omitted an
    // enum. A small number of generated OpenBB schemas expose their only
    // provider as a `const` or `default`; any schema with no explicit provider
    // candidate remains hidden instead of inheriting every Guru grant.
    let candidates = match provider_schema.get("enum") {
        Some(Value::Array(values)) if !values.is_empty() => values
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .ok_or(McpError::Protocol)?
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        Some(_) => return Err(McpError::Protocol),
        None => match provider_schema
            .get("const")
            .or_else(|| provider_schema.get("default"))
        {
            Some(Value::String(provider)) if !provider.is_empty() => vec![provider.clone()],
            Some(_) => return Err(McpError::Protocol),
            None => return Ok(None),
        },
    };
    let available = candidates
        .into_iter()
        .filter(|provider| enabled_provider_ids.contains(provider))
        .collect::<Vec<_>>();
    if available.is_empty() {
        return Ok(None);
    }
    if let Some(constant) = provider_schema.get("const") {
        let constant = constant.as_str().ok_or(McpError::Protocol)?;
        if available.as_slice() != [constant] {
            return Ok(None);
        }
    }
    provider_schema.insert(
        "enum".into(),
        Value::Array(available.iter().cloned().map(Value::String).collect()),
    );
    if provider_schema
        .get("default")
        .and_then(Value::as_str)
        .is_some_and(|default| !available.iter().any(|provider| provider == default))
    {
        provider_schema.remove("default");
    }
    // A provider-aware OpenBB endpoint must never fall back to a server
    // default after Guru authorization narrowed its provider set. Requiring
    // the argument also satisfies the multi-provider contract and makes a
    // single remaining grant explicit instead of ambiguous.
    let required = schema
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or(McpError::Protocol)?;
    if !required
        .iter()
        .any(|value| value.as_str() == Some("provider"))
    {
        required.push(Value::String("provider".into()));
    }
    validate_tool(&filtered)?;
    Ok(Some(filtered))
}

/// Provider-backed runtimes declare one canonical provider receipt location in
/// their signed runtime manifest. Rust follows only that exact JSON Pointer;
/// text blocks and unrelated nested fields are never treated as provenance.
pub fn validate_result_provider(
    result: &McpCallResult,
    provider_receipt_pointer: &str,
    requested_provider: Option<&str>,
    enabled_provider_ids: &BTreeSet<String>,
) -> Result<(), McpError> {
    let Some(requested_provider) = requested_provider else {
        return Ok(());
    };
    if !enabled_provider_ids.contains(requested_provider) {
        return Err(McpError::Protocol);
    }
    if !valid_json_pointer(provider_receipt_pointer) {
        return Err(McpError::Protocol);
    }
    let encoded = serde_json::to_value(result).map_err(|_| McpError::Protocol)?;
    let reported_provider = encoded
        .pointer(provider_receipt_pointer)
        .and_then(Value::as_str)
        .filter(|provider| !provider.is_empty())
        .ok_or(McpError::Protocol)?;
    if reported_provider != requested_provider || !enabled_provider_ids.contains(reported_provider)
    {
        return Err(McpError::Protocol);
    }
    Ok(())
}

pub fn contains_protected_value(value: &Value, protected_values: &[String]) -> bool {
    match value {
        Value::String(value) => protected_values
            .iter()
            .any(|protected| !protected.is_empty() && value.contains(protected)),
        Value::Array(values) => values
            .iter()
            .any(|value| contains_protected_value(value, protected_values)),
        Value::Object(object) => {
            object.keys().any(|key| {
                protected_values
                    .iter()
                    .any(|protected| !protected.is_empty() && key.contains(protected))
            }) || object
                .values()
                .any(|value| contains_protected_value(value, protected_values))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[cfg(unix)]
    const MOCK_MCP_SCRIPT: &str = r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$MCP_TEST_TRANSCRIPT"
  case "$line" in
    *'"type":"guruterminal.bootstrap"'*)
      ;;
    *'"method":"initialize"'*)
      id=${line#*\"id\":\"}; id=${id%%\"*}
      printf '{"jsonrpc":"2.0","id":"server-ping","method":"ping","params":{}}\n'
      printf '{"jsonrpc":"2.0","id":"%s","result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"Guru MCP test server","version":"1"}}}\n' "$id"
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"notifications/cancelled"'*)
      printf 'cancelled\n' >> "$MCP_TEST_CANCELLED"
      ;;
    *'"method":"tools/list"'*'"cursor":"page-2"'*)
      id=${line#*\"id\":\"}; id=${id%%\"*}
      if [ "$MCP_TEST_MODE" = "repeat_cursor" ]; then
        printf '{"jsonrpc":"2.0","id":"%s","result":{"tools":[{"name":"equity_quote","description":"quote","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"}},"required":["symbol"]}}],"nextCursor":"page-2"}}\n' "$id"
      else
        printf '{"jsonrpc":"2.0","id":"%s","result":{"tools":[{"name":"equity_quote","description":"quote","inputSchema":{"type":"object","properties":{"symbol":{"type":"string"}},"required":["symbol"]}},{"name":"hang","inputSchema":{"type":"object","properties":{}}},{"name":"crash","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      fi
      ;;
    *'"method":"tools/list"'*)
      id=${line#*\"id\":\"}; id=${id%%\"*}
      printf '{"jsonrpc":"2.0","id":"%s","result":{"tools":[{"name":"admin_discovery","description":"discovery","inputSchema":{"type":"object","properties":{}}}],"nextCursor":"page-2"}}\n' "$id"
      ;;
    *'"method":"tools/call"'*'"name":"equity_quote"'*)
      id=${line#*\"id\":\"}; id=${id%%\"*}
      printf '{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}\n'
      printf '{"jsonrpc":"2.0","id":"%s","result":{"content":[{"type":"text","text":"quote"}],"structuredContent":{"price":123},"isError":false}}\n' "$id"
      ;;
    *'"method":"tools/call"'*'"name":"hang"'*)
      ;;
    *'"method":"tools/call"'*'"name":"crash"'*)
      exit 0
      ;;
  esac
done
"#;

    #[cfg(unix)]
    async fn spawn_mock_mcp(
        temporary: &tempfile::TempDir,
        name: &str,
    ) -> (McpSession, Vec<McpTool>, PathBuf, PathBuf) {
        let transcript = temporary.path().join(format!("{name}-transcript.jsonl"));
        let cancelled = temporary.path().join(format!("{name}-cancelled"));
        // macOS `/bin/sh` is a dispatcher that re-execs `/private/var/select/sh`.
        // Use the concrete shell so the process lease observes one executable.
        let executable = std::fs::canonicalize("/bin/bash").unwrap();
        let environment = BTreeMap::from([
            (
                "MCP_TEST_TRANSCRIPT".into(),
                transcript.to_string_lossy().into_owned(),
            ),
            (
                "MCP_TEST_CANCELLED".into(),
                cancelled.to_string_lossy().into_owned(),
            ),
            ("MCP_TEST_MODE".into(), name.to_owned()),
        ]);
        let (session, tools) = McpSession::spawn(McpLaunchConfig {
            server_id: "mock".into(),
            executable,
            arguments: vec!["-c".into(), MOCK_MCP_SCRIPT.into()],
            private_working_dir: temporary.path().join(format!("{name}-scratch")),
            lease_dir: temporary.path().join("leases"),
            environment,
            bootstrap: json!({
                "type": "guruterminal.bootstrap",
                "protocol_version": 1,
                "credentials": {}
            }),
        })
        .await
        .unwrap();
        (session, tools, transcript, cancelled)
    }

    fn tool() -> McpTool {
        McpTool {
            name: "equity_price_quote".into(),
            title: Some("Quote".into()),
            description: Some("Read a quote".into()),
            input_schema: json!({
                "type": "object",
                "properties": { "symbol": { "type": "string" } },
                "required": ["symbol"]
            }),
            output_schema: None,
            annotations: Some(json!({
                "readOnlyHint": true,
                "destructiveHint": false
            })),
            metadata: None,
        }
    }

    #[test]
    fn runtime_discovery_requires_the_manifest_lock_digest() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = temporary.path().join("openbb-runtime");
        std::fs::create_dir(&runtime).unwrap();
        let lock = b"version = 1\n";
        std::fs::write(runtime.join("uv.lock"), lock).unwrap();
        std::fs::write(
            runtime.join(format!(
                "guruterminal-openbb{}",
                std::env::consts::EXE_SUFFIX
            )),
            b"executable",
        )
        .unwrap();
        std::fs::write(
            runtime.join("runtime-manifest.json"),
            serde_json::to_vec(&json!({
                "schema_version": "guruterminal-mcp-runtime/1",
                "runtime_id": "openbb",
                "executable": "guruterminal-openbb",
                "uv_lock_sha256": crate::hashing::sha256(lock),
                "protocol": {
                    "transport": "stdio",
                    "bootstrap_type": "guruterminal.bootstrap",
                    "bootstrap_version": 1,
                    "initial_tools": "admin_only",
                    "provider_receipt_pointer": "/structuredContent/provider",
                    "tool_activation": {
                        "tool_name": "activate_tools",
                        "argument_name": "tool_names"
                    },
                    "control_tool_names": ["activate_tools"]
                },
                "security": { "read_only": true },
                "providerless_tool_policy": {
                    "local_tools": ["technical_sma"],
                    "implicit_provider": {}
                },
                "allowed_categories": ["equity"],
                "providers": [{
                    "id": "yfinance",
                    "network_hosts": ["query1.finance.yahoo.com"]
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let discovered = discover_bundled_runtimes(temporary.path(), temporary.path());
        assert!(discovered.contains_key("openbb"));

        let manifest_path = runtime.join("runtime-manifest.json");
        let mut manifest: Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["protocol"]["provider_receipt_pointer"] = json!("/bad~2");
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(discover_bundled_runtimes(temporary.path(), temporary.path()).is_empty());
        manifest["protocol"]["provider_receipt_pointer"] = json!("/structuredContent/provider");
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        std::fs::write(runtime.join("uv.lock"), b"tampered").unwrap();
        assert!(discover_bundled_runtimes(temporary.path(), temporary.path()).is_empty());
    }

    #[test]
    fn dynamic_tool_schema_is_namespaced() {
        let value = tool_to_agent_schema("openbb", &tool()).unwrap();
        assert_eq!(value["name"], "mcp__openbb__equity_price_quote");
        assert_eq!(value["mcp_name"], "equity_price_quote");
        assert_eq!(value["parameters"]["required"], json!(["symbol"]));
    }

    #[test]
    fn unsafe_tool_names_and_schemas_are_rejected() {
        let mut candidate = tool();
        candidate.name = "../../install".into();
        assert!(tool_to_agent_schema("openbb", &candidate).is_err());
        let mut candidate = tool();
        candidate.input_schema = Value::String("not a schema".into());
        assert!(validate_tool(&candidate).is_err());
        for schema in [
            json!({"properties": {}}),
            json!({"type": "array", "properties": {}}),
            json!({"type": "object", "properties": []}),
            json!({"type": "object", "properties": {"provider": true}}),
            json!({"type": "object", "properties": {"symbol": {"type": "string"}}, "required": "symbol"}),
            json!({"type": "object", "properties": {"symbol": {"type": "string"}}, "required": ["symbol", "symbol"]}),
            json!({"type": "object", "properties": {"symbol": {"type": "string"}}, "required": ["missing"]}),
        ] {
            let mut candidate = tool();
            candidate.input_schema = schema;
            assert!(validate_tool(&candidate).is_err());
            assert!(filter_tool_providers(
                &candidate,
                &BTreeSet::from(["yfinance".to_owned()]),
                false,
            )
            .is_err());
        }

        let mut candidate = tool();
        candidate.output_schema = Some(json!({"padding": "x".repeat(MAX_TOOL_SCHEMA_BYTES)}));
        assert!(matches!(
            validate_tool(&candidate),
            Err(McpError::ToolLimit) | Err(McpError::Protocol)
        ));

        let mut candidate = tool();
        candidate.metadata = Some(json!({"padding": "x".repeat(MAX_TOOL_DESCRIPTOR_BYTES)}));
        assert!(matches!(
            validate_tool(&candidate),
            Err(McpError::ToolLimit)
        ));
    }

    #[test]
    fn cumulative_tool_inventory_is_bounded_across_pages() {
        let descriptor = json!({"_meta": {"padding": "x".repeat(120 * 1024)}});
        let mut total = 0;
        let mut rejected_on_second_page = false;
        for page in 0..2 {
            for _ in 0..40 {
                if accumulate_tool_inventory_bytes(&mut total, &descriptor).is_err() {
                    rejected_on_second_page = page == 1;
                    break;
                }
            }
            if rejected_on_second_page {
                break;
            }
        }
        assert!(rejected_on_second_page);
    }

    #[test]
    fn provider_schema_is_intersected_and_made_explicit() {
        let mut candidate = tool();
        candidate.input_schema = json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string" },
                "provider": { "type": "string", "enum": ["fmp", "yfinance", "intrinio"] }
            },
            "required": ["symbol"]
        });
        let filtered = filter_tool_providers(
            &candidate,
            &BTreeSet::from(["fmp".to_owned(), "intrinio".to_owned()]),
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            filtered.input_schema["properties"]["provider"]["enum"],
            json!(["fmp", "intrinio"])
        );
        assert_eq!(
            filtered.input_schema["required"],
            json!(["symbol", "provider"])
        );

        let narrowed =
            filter_tool_providers(&candidate, &BTreeSet::from(["yfinance".to_owned()]), false)
                .unwrap()
                .unwrap();
        assert_eq!(
            narrowed.input_schema["properties"]["provider"]["enum"],
            json!(["yfinance"])
        );
        assert_eq!(
            narrowed.input_schema["required"],
            json!(["symbol", "provider"])
        );
    }

    #[test]
    fn provider_schema_without_enum_never_widens_authority() {
        let mut candidate = tool();
        candidate.input_schema = json!({
            "type": "object",
            "properties": {
                "provider": { "type": "string", "default": "congress_gov" }
            }
        });
        assert!(
            filter_tool_providers(&candidate, &BTreeSet::from(["yfinance".to_owned()]), false,)
                .unwrap()
                .is_none()
        );

        let filtered = filter_tool_providers(
            &candidate,
            &BTreeSet::from(["congress_gov".to_owned(), "yfinance".to_owned()]),
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            filtered.input_schema["properties"]["provider"]["enum"],
            json!(["congress_gov"])
        );
        assert_eq!(filtered.input_schema["required"], json!(["provider"]));

        candidate.input_schema = json!({
            "type": "object",
            "properties": { "provider": { "type": "string" } }
        });
        assert!(filter_tool_providers(
            &candidate,
            &BTreeSet::from(["congress_gov".to_owned()]),
            false,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn contradictory_result_provider_is_rejected() {
        let result = McpCallResult {
            content: vec![json!({
                "type": "text",
                "text": "{\"provider\":\"intrinio\",\"results\":[{\"price\":1}]}"
            })],
            structured_content: Some(json!({
                "provider": "intrinio",
                "results": [{"price": 1}]
            })),
            is_error: false,
            metadata: None,
        };
        assert!(validate_result_provider(
            &result,
            "/structuredContent/provider",
            Some("fmp"),
            &BTreeSet::from(["fmp".to_owned(), "intrinio".to_owned()]),
        )
        .is_err());
    }

    #[test]
    fn requested_provider_requires_a_structured_provider_receipt() {
        let enabled = BTreeSet::from(["fmp".to_owned(), "intrinio".to_owned()]);
        let missing = McpCallResult {
            content: vec![json!({
                "type": "text",
                "text": "{\"provider\":\"fmp\",\"results\":[]}"
            })],
            structured_content: None,
            is_error: false,
            metadata: None,
        };
        assert!(validate_result_provider(
            &missing,
            "/structuredContent/provider",
            Some("fmp"),
            &enabled,
        )
        .is_err());

        let matching = McpCallResult {
            content: vec![json!({"type": "text", "text": "ignored"})],
            structured_content: Some(json!({
                "provider": "fmp",
                "results": [{"provider": "internet_carrier", "price": 1}]
            })),
            is_error: false,
            metadata: None,
        };
        assert!(validate_result_provider(
            &matching,
            "/structuredContent/provider",
            Some("fmp"),
            &enabled,
        )
        .is_ok());
    }

    #[tokio::test]
    async fn list_changed_notification_is_observed() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let changed = AtomicBool::new(false);
        let (stdin, _server) = duplex(1024);
        let stdin = Mutex::new(stdin);
        dispatch_frame(
            br#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
            &pending,
            &changed,
            &stdin,
        )
        .await
        .unwrap();
        assert!(changed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn unknown_server_notifications_do_not_fail_the_session() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let changed = AtomicBool::new(false);
        let (stdin, _server) = duplex(1024);
        let stdin = Mutex::new(stdin);
        dispatch_frame(
            br#"{"jsonrpc":"2.0","method":"notifications/resources/list_changed"}"#,
            &pending,
            &changed,
            &stdin,
        )
        .await
        .unwrap();
        assert!(!changed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn oversized_frames_fail_closed() {
        let (mut writer, reader) = duplex(MAX_FRAME_BYTES + 32);
        let payload = vec![b'x'; MAX_FRAME_BYTES + 1];
        tokio::spawn(async move {
            writer.write_all(&payload).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
        });
        let mut reader = BufReader::new(reader);
        assert!(matches!(
            read_frame(&mut reader).await,
            Err(McpError::FrameTooLarge)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn session_transcript_covers_initialize_pagination_call_list_change_and_cancel() {
        let _guard = MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let (session, tools, transcript, cancelled) = spawn_mock_mcp(&temporary, "lifecycle").await;
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["admin_discovery", "equity_quote", "hang", "crash"]
        );

        let result = session
            .call_tool(
                "equity_quote",
                json!({"symbol": "TEST"}),
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(result.structured_content.unwrap()["price"], 123);
        assert!(session.take_tools_changed());
        assert!(!session.take_tools_changed());
        assert_eq!(session.list_tools().await.unwrap().len(), 4);

        assert!(matches!(
            session
                .call_tool("hang", json!({}), Duration::from_millis(50))
                .await,
            Err(McpError::Timeout)
        ));
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if cancelled.is_file() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        let result = session
            .call_tool(
                "equity_quote",
                json!({"symbol": "AFTER_CANCEL"}),
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(result.structured_content.unwrap()["price"], 123);
        let _ = session.shutdown(Duration::from_millis(250)).await;

        let transcript = std::fs::read_to_string(transcript).unwrap();
        let bootstrap = transcript
            .find("guruterminal.bootstrap")
            .expect("bootstrap must be first");
        let initialize = transcript.find("\"method\":\"initialize\"").unwrap();
        let initialized = transcript
            .find("\"method\":\"notifications/initialized\"")
            .unwrap();
        let first_list = transcript.find("\"method\":\"tools/list\"").unwrap();
        assert!(bootstrap < initialize && initialize < initialized && initialized < first_list);
        assert!(transcript.contains("\"cursor\":\"page-2\""));
        assert!(transcript.contains("\"method\":\"tools/call\""));
        assert!(transcript.contains("\"method\":\"notifications/cancelled\""));
        assert!(transcript.lines().any(|line| {
            serde_json::from_str::<Value>(line).is_ok_and(|value| {
                value.get("id").and_then(Value::as_str) == Some("server-ping")
                    && value.get("result") == Some(&json!({}))
            })
        }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repeated_tool_list_cursor_is_rejected() {
        let _guard = MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let transcript = temporary.path().join("repeat-cursor-transcript.jsonl");
        let cancelled = temporary.path().join("repeat-cursor-cancelled");
        let result = McpSession::spawn(McpLaunchConfig {
            server_id: "mock".into(),
            executable: std::fs::canonicalize("/bin/bash").unwrap(),
            arguments: vec!["-c".into(), MOCK_MCP_SCRIPT.into()],
            private_working_dir: temporary.path().join("repeat-cursor-scratch"),
            lease_dir: temporary.path().join("leases"),
            environment: BTreeMap::from([
                (
                    "MCP_TEST_TRANSCRIPT".into(),
                    transcript.to_string_lossy().into_owned(),
                ),
                (
                    "MCP_TEST_CANCELLED".into(),
                    cancelled.to_string_lossy().into_owned(),
                ),
                ("MCP_TEST_MODE".into(), "repeat_cursor".into()),
            ]),
            bootstrap: json!({
                "type": "guruterminal.bootstrap",
                "protocol_version": 1,
                "credentials": {}
            }),
        })
        .await;
        assert!(matches!(result, Err(McpError::Protocol)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_an_inflight_call_notifies_mcp_cancellation() {
        let _guard = MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let (session, _tools, _transcript, cancelled) =
            spawn_mock_mcp(&temporary, "peer-cancel").await;
        {
            let call = session.call_tool("hang", json!({}), Duration::from_secs(30));
            tokio::pin!(call);
            tokio::select! {
                result = &mut call => panic!("hanging call unexpectedly settled: {result:?}"),
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if cancelled.is_file() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        session.shutdown(Duration::from_millis(250)).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stopped_session_can_be_replaced_by_a_fresh_session() {
        let _guard = MCP_PROCESS_TEST_LOCK.lock().await;
        let temporary = tempfile::tempdir().unwrap();
        let (session, _, _, _) = spawn_mock_mcp(&temporary, "crashed").await;
        assert!(matches!(
            session
                .call_tool("crash", json!({}), Duration::from_secs(1))
                .await,
            Err(McpError::Stopped)
        ));
        drop(session);

        let (replacement, tools, _, _) = spawn_mock_mcp(&temporary, "replacement").await;
        assert_eq!(tools.len(), 4);
        let result = replacement
            .call_tool(
                "equity_quote",
                json!({"symbol": "RESTARTED"}),
                Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(result.structured_content.unwrap()["price"], 123);
        let _ = replacement.shutdown(Duration::from_millis(250)).await;
    }
}
