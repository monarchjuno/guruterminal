use crate::hashing::sha256;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
    task::JoinHandle,
    time::timeout,
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
    artifact_trust::{ensure_private_directory, verify_executable, VerifiedExecutable},
    process_lease::ProcessLeaseError,
};

pub const COMPUTE_PROTOCOL: &str = "guruterminal-compute/2";
pub const DENO_VERSION: &str = "2.9.5";
pub const PYODIDE_VERSION: &str = "314.0.3";
#[cfg(target_os = "macos")]
const DENO_ARCHIVE_SHA256: &str =
    "b796aadd131f6930560c1ee040cf0d6f53933fbb987464e9ff46bd7ea4830615";
#[cfg(target_os = "windows")]
const DENO_ARCHIVE_SHA256: &str =
    "171efab55ac6b9881fd53ee4c20f8bf3bb1340ffc618483746909014db12216a";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const DENO_ARCHIVE_SHA256: &str = "unsupported-platform";
const PYTHON_VERSION_PREFIX: &str = "3.14.";
const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(30);
const PYTHON_INIT_TIMEOUT: Duration = Duration::from_secs(60);
const JAVASCRIPT_INIT_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const BOOTSTRAP: &[u8] = include_bytes!("../../compute/bootstrap.mjs");
const JAVASCRIPT_HOST: &[u8] = include_bytes!("../../compute/javascript-host.mjs");
const CONTRACT: &[u8] = include_bytes!("../../compute/contract.mjs");
const RUNTIME_MANIFEST: &[u8] = include_bytes!("../../compute/runtime-manifest.json");

fn allowed_packages() -> BTreeSet<&'static str> {
    BTreeSet::from(["numpy", "pandas", "scipy", "statsmodels", "scikit-learn"])
}

fn expected_package_version(name: &str) -> Option<&'static str> {
    match name {
        "numpy" => Some("2.4.3"),
        "pandas" => Some("3.0.2"),
        "scipy" => Some("1.18.0"),
        "statsmodels" => Some("0.14.6"),
        "scikit-learn" => Some("1.8.0"),
        _ => None,
    }
}

#[derive(Debug, Error)]
pub enum ComputeError {
    #[error("compute runtime is unavailable")]
    MissingRuntime,
    #[error("compute runtime is not a trusted app artifact")]
    UntrustedRuntime,
    #[error("compute request is invalid")]
    InvalidRequest,
    #[error("compute worker I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("compute worker protocol failed")]
    Protocol,
    #[error("compute worker frame exceeded the size limit")]
    FrameTooLarge,
    #[error("compute execution timed out")]
    Timeout,
    #[error("compute execution failed: {0}")]
    Remote(String),
    #[error("compute process ownership failed: {0}")]
    Lease(#[from] ProcessLeaseError),
}

#[derive(Clone, Debug)]
pub struct ComputeArtifacts {
    pub executable: PathBuf,
    pub runtime_dir: PathBuf,
    pub bootstrap: PathBuf,
    pub lease_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputeLanguage {
    Python,
    Javascript,
}

impl ComputeLanguage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Javascript => "javascript",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeCall {
    pub language: ComputeLanguage,
    pub source: String,
    #[serde(default = "default_inputs")]
    pub inputs: Value,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub seed: u32,
}

fn default_inputs() -> Value {
    json!({})
}

fn contains_javascript_import(source: &str) -> bool {
    let bytes = source.as_bytes();
    bytes
        .windows(b"import".len())
        .enumerate()
        .any(|(index, word)| {
            if word != b"import" {
                return false;
            }
            let identifier = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$');
            index
                .checked_sub(1)
                .is_none_or(|before| !identifier(bytes[before]))
                && bytes
                    .get(index + b"import".len())
                    .is_none_or(|after| !identifier(*after))
        })
}

impl ComputeCall {
    pub fn validate(&self) -> Result<(), ComputeError> {
        if self.source.trim().is_empty()
            || self.source.len() > MAX_SOURCE_BYTES
            || self.source.contains('\0')
            || serde_json::to_vec(&self.inputs)
                .map_err(|_| ComputeError::InvalidRequest)?
                .len()
                > MAX_INPUT_BYTES
        {
            return Err(ComputeError::InvalidRequest);
        }
        match self.language {
            ComputeLanguage::Javascript => {
                if !self.packages.is_empty() || contains_javascript_import(&self.source) {
                    return Err(ComputeError::InvalidRequest);
                }
            }
            ComputeLanguage::Python => {
                let allowed = allowed_packages();
                let selected = self
                    .packages
                    .iter()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>();
                if selected.len() != self.packages.len()
                    || selected.len() > allowed.len()
                    || !selected.is_subset(&allowed)
                {
                    return Err(ComputeError::InvalidRequest);
                }
            }
        }
        Ok(())
    }

    fn package_key(&self) -> Vec<String> {
        match self.language {
            ComputeLanguage::Javascript => Vec::new(),
            ComputeLanguage::Python => {
                let mut packages = self.packages.clone();
                packages.sort();
                packages
            }
        }
    }
}

fn packages_covered_by(loaded: &[String], requested: &[String]) -> bool {
    requested
        .iter()
        .all(|name| loaded.iter().any(|loaded_name| loaded_name == name))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComputeLog {
    stream: String,
    text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComputeRuntime {
    language: ComputeLanguage,
    deno: String,
    v8: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pyodide: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    python: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    packages: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComputeRemoteError {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostEnvelope {
    protocol: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    language: Option<ComputeLanguage>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    logs: Option<Vec<ComputeLog>>,
    #[serde(default)]
    runtime: Option<ComputeRuntime>,
    #[serde(default)]
    error: Option<ComputeRemoteError>,
}

#[derive(Default)]
struct LanguageWorker {
    host: Option<RetainedHost>,
    package_key: Vec<String>,
}

impl LanguageWorker {
    fn kill(&mut self) {
        self.host.take();
        self.package_key.clear();
    }

    async fn shutdown(&mut self) {
        if let Some(mut host) = self.host.take() {
            let _ = host.shutdown().await;
        }
        self.package_key.clear();
    }
}

struct RunDiscardGuard<'a> {
    worker: &'a mut LanguageWorker,
    armed: bool,
}

impl<'a> RunDiscardGuard<'a> {
    fn new(worker: &'a mut LanguageWorker) -> Self {
        Self {
            worker,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RunDiscardGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.worker.kill();
        }
    }
}

/// A completed NDJSON result keeps the retained host. I/O timeout, crash,
/// protocol failure, and cancellation still poison it.
enum HostExchange {
    Output(Value),
    CellError(ComputeError),
}

/// Lazy, turn-local compute hosts. Chat admission does not spawn a process.
pub struct TurnComputeSession {
    artifacts: Option<ComputeArtifacts>,
    scratch: PathBuf,
    fake_host: Option<PathBuf>,
    python: Mutex<LanguageWorker>,
    javascript: Mutex<LanguageWorker>,
}

impl Default for TurnComputeSession {
    fn default() -> Self {
        Self::disabled()
    }
}

impl TurnComputeSession {
    pub fn new(artifacts: Option<ComputeArtifacts>, scratch: PathBuf) -> Self {
        Self {
            artifacts,
            scratch,
            fake_host: None,
            python: Mutex::new(LanguageWorker::default()),
            javascript: Mutex::new(LanguageWorker::default()),
        }
    }

    pub fn disabled() -> Self {
        Self::new(None, PathBuf::new())
    }

    #[cfg(test)]
    pub fn for_test(fake_host: PathBuf, scratch: PathBuf, lease_dir: PathBuf) -> Self {
        Self {
            artifacts: Some(ComputeArtifacts {
                executable: fake_host.clone(),
                runtime_dir: fake_host
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf(),
                bootstrap: fake_host.clone(),
                lease_dir,
            }),
            scratch,
            fake_host: Some(fake_host),
            python: Mutex::new(LanguageWorker::default()),
            javascript: Mutex::new(LanguageWorker::default()),
        }
    }

    pub async fn run(&self, call: ComputeCall) -> Result<Value, ComputeError> {
        call.validate()?;
        if self.artifacts.is_none() {
            return Err(ComputeError::MissingRuntime);
        }
        let slot = match call.language {
            ComputeLanguage::Python => &self.python,
            ComputeLanguage::Javascript => &self.javascript,
        };
        let mut worker = slot.lock().await;
        self.run_locked(&mut worker, call).await
    }

    async fn run_locked(
        &self,
        worker: &mut LanguageWorker,
        call: ComputeCall,
    ) -> Result<Value, ComputeError> {
        let package_key = call.package_key();
        if worker.host.is_some() && !packages_covered_by(&worker.package_key, &package_key) {
            worker.shutdown().await;
        }
        if worker.host.is_none() {
            worker.host = Some(self.spawn_host(call.language, &package_key).await?);
            worker.package_key = package_key.clone();
        }
        let run_timeout = if self.fake_host.is_some() {
            Duration::from_secs(2)
        } else {
            EXECUTION_TIMEOUT
        };
        let mut guard = RunDiscardGuard::new(worker);
        let host = guard.worker.host.as_mut().ok_or(ComputeError::Protocol)?;
        let output = match host.execute(&call, run_timeout).await {
            Ok(HostExchange::Output(output)) => output,
            Ok(HostExchange::CellError(error)) => {
                guard.disarm();
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        guard.disarm();
        Ok(output)
    }

    async fn spawn_host(
        &self,
        language: ComputeLanguage,
        package_key: &[String],
    ) -> Result<RetainedHost, ComputeError> {
        let artifacts = self
            .artifacts
            .as_ref()
            .ok_or(ComputeError::MissingRuntime)?;
        if self.fake_host.is_none() {
            let _verified = verify_runtime(artifacts)?;
        }
        if self.scratch.as_os_str().is_empty() {
            return Err(ComputeError::InvalidRequest);
        }
        let scratch = self.scratch.join(language.as_str());
        ensure_private_directory(&scratch).map_err(|_| ComputeError::InvalidRequest)?;
        let deno_cache = scratch.join("deno-cache");
        ensure_private_directory(&deno_cache).map_err(|_| ComputeError::InvalidRequest)?;
        let mut host =
            RetainedHost::spawn(artifacts, &deno_cache, language, self.fake_host.as_deref())
                .await?;
        let init = match language {
            ComputeLanguage::Python => json!({
                "protocol": COMPUTE_PROTOCOL,
                "type": "init",
                "language": "python",
                "packages": package_key,
            }),
            ComputeLanguage::Javascript => json!({
                "protocol": COMPUTE_PROTOCOL,
                "type": "init",
                "language": "javascript",
            }),
        };
        host.write_message(&init).await?;
        let init_timeout = match language {
            ComputeLanguage::Python => PYTHON_INIT_TIMEOUT,
            ComputeLanguage::Javascript => JAVASCRIPT_INIT_TIMEOUT,
        };
        let ready = host.read_message(init_timeout).await?;
        if ready.protocol != COMPUTE_PROTOCOL
            || ready.kind != "ready"
            || ready.language != Some(language)
        {
            return Err(ComputeError::Protocol);
        }
        host.reject_if_stderr()?;
        Ok(host)
    }

    pub async fn shutdown(&self) {
        self.python.lock().await.shutdown().await;
        self.javascript.lock().await.shutdown().await;
    }
}

impl Drop for TurnComputeSession {
    fn drop(&mut self) {
        if let Ok(mut worker) = self.python.try_lock() {
            worker.kill();
        }
        if let Ok(mut worker) = self.javascript.try_lock() {
            worker.kill();
        }
    }
}

pub async fn run_compute(
    artifacts: &ComputeArtifacts,
    private_working_dir: PathBuf,
    call: ComputeCall,
) -> Result<Value, ComputeError> {
    let session = TurnComputeSession::new(Some(artifacts.clone()), private_working_dir);
    let result = session.run(call).await;
    session.shutdown().await;
    result
}

struct RetainedHost {
    process: ComputeProcess,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Arc<std::sync::Mutex<Vec<u8>>>,
    _stderr_task: JoinHandle<Result<(), ComputeError>>,
}

impl RetainedHost {
    async fn spawn(
        artifacts: &ComputeArtifacts,
        deno_cache: &Path,
        language: ComputeLanguage,
        fake_host: Option<&Path>,
    ) -> Result<Self, ComputeError> {
        let mut process = ComputeProcess::spawn(artifacts, deno_cache, language, fake_host).await?;
        let stdin = process.child.stdin.take().ok_or(ComputeError::Protocol)?;
        let stdout = BufReader::new(process.child.stdout.take().ok_or(ComputeError::Protocol)?);
        let stderr = process.child.stderr.take().ok_or(ComputeError::Protocol)?;
        let stderr_buf = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stderr_task = tokio::spawn(read_stderr(stderr, stderr_buf.clone(), MAX_STDERR_BYTES));
        Ok(Self {
            process,
            stdin,
            stdout,
            stderr: stderr_buf,
            _stderr_task: stderr_task,
        })
    }

    async fn write_message(&mut self, value: &Value) -> Result<(), ComputeError> {
        let mut bytes = serde_json::to_vec(value).map_err(|_| ComputeError::Protocol)?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(ComputeError::FrameTooLarge);
        }
        bytes.push(b'\n');
        timeout(WRITE_TIMEOUT, self.stdin.write_all(&bytes))
            .await
            .map_err(|_| ComputeError::Timeout)??;
        timeout(WRITE_TIMEOUT, self.stdin.flush())
            .await
            .map_err(|_| ComputeError::Timeout)??;
        Ok(())
    }

    async fn read_message(&mut self, limit: Duration) -> Result<HostEnvelope, ComputeError> {
        timeout(limit, read_json_line(&mut self.stdout, MAX_FRAME_BYTES))
            .await
            .map_err(|_| ComputeError::Timeout)?
    }

    fn reject_if_stderr(&self) -> Result<(), ComputeError> {
        let bytes = self.stderr.lock().map_err(|_| ComputeError::Protocol)?;
        if bytes.is_empty() {
            Ok(())
        } else {
            Err(ComputeError::Protocol)
        }
    }

    async fn execute(
        &mut self,
        call: &ComputeCall,
        run_timeout: Duration,
    ) -> Result<HostExchange, ComputeError> {
        let id = Uuid::new_v4().simple().to_string();
        let request = json!({
            "protocol": COMPUTE_PROTOCOL,
            "type": "run",
            "id": id,
            "source": &call.source,
            "inputs": &call.inputs,
            "seed": call.seed,
        });
        self.write_message(&request).await?;
        let response = self.read_message(run_timeout).await?;
        self.reject_if_stderr()?;
        if response.protocol != COMPUTE_PROTOCOL
            || response.kind != "result"
            || response.id.as_deref() != Some(id.as_str())
        {
            return Err(ComputeError::Protocol);
        }
        if response.ok != Some(true) {
            let error = response.error.ok_or(ComputeError::Protocol)?;
            if error.code.is_empty() || error.message.is_empty() {
                return Err(ComputeError::Protocol);
            }
            if error.code == "timeout" {
                return Ok(HostExchange::CellError(ComputeError::Timeout));
            }
            return Ok(HostExchange::CellError(ComputeError::Remote(
                error.message.chars().take(500).collect(),
            )));
        }
        if response.error.is_some() {
            return Err(ComputeError::Protocol);
        }
        let result = response.result.ok_or(ComputeError::Protocol)?;
        let logs = response.logs.ok_or(ComputeError::Protocol)?;
        let runtime = response.runtime.ok_or(ComputeError::Protocol)?;
        validate_runtime_response(&runtime, call)?;
        let source_sha256 = sha256(call.source.as_bytes());
        let input_sha256 =
            sha256(&serde_json::to_vec(&call.inputs).map_err(|_| ComputeError::InvalidRequest)?);
        let result_sha256 =
            sha256(&serde_json::to_vec(&result).map_err(|_| ComputeError::Protocol)?);
        Ok(HostExchange::Output(json!({
            "schema_version": "guruterminal-compute-result/1",
            "data": result,
            "logs": logs,
            "runtime": runtime,
            "receipt": {
                "source_sha256": source_sha256,
                "input_sha256": input_sha256,
                "result_sha256": result_sha256,
                "seed": call.seed,
                "language": call.language.as_str(),
                "packages": call.packages,
            },
            "warnings": [
                "Agent-generated computation verifies execution only; it does not validate model-supplied inputs or create Evidence authority."
            ]
        })))
    }

    async fn shutdown(&mut self) -> Result<(), ComputeError> {
        let message = json!({
            "protocol": COMPUTE_PROTOCOL,
            "type": "shutdown",
        });
        let _ = self.write_message(&message).await;
        let _ = timeout(
            STOP_TIMEOUT,
            read_json_line(&mut self.stdout, MAX_FRAME_BYTES),
        )
        .await;
        let _ = timeout(STOP_TIMEOUT, self.process.child.wait()).await;
        self.process.complete().await
    }
}

fn verify_runtime(artifacts: &ComputeArtifacts) -> Result<VerifiedExecutable, ComputeError> {
    let verified =
        verify_executable(&artifacts.executable).map_err(|_| ComputeError::UntrustedRuntime)?;
    if !artifacts.runtime_dir.is_dir()
        || !std::fs::symlink_metadata(&artifacts.runtime_dir)
            .is_ok_and(|metadata| metadata.file_type().is_dir())
        || artifacts.bootstrap.parent() != Some(artifacts.runtime_dir.as_path())
        || !exact_file(&artifacts.bootstrap, BOOTSTRAP)
        || !exact_file(
            &artifacts.runtime_dir.join("javascript-host.mjs"),
            JAVASCRIPT_HOST,
        )
        || !exact_file(&artifacts.runtime_dir.join("contract.mjs"), CONTRACT)
        || !exact_file(
            &artifacts.runtime_dir.join("runtime-manifest.json"),
            RUNTIME_MANIFEST,
        )
        || marker(&artifacts.runtime_dir.join(".deno-version"))? != DENO_VERSION
        || marker(&artifacts.runtime_dir.join(".deno-archive.sha256"))? != DENO_ARCHIVE_SHA256
        || marker(&artifacts.runtime_dir.join(".pyodide-version"))? != PYODIDE_VERSION
        || marker(&artifacts.runtime_dir.join(".compute-manifest.sha256"))?
            != sha256(RUNTIME_MANIFEST)
        || marker(&artifacts.runtime_dir.join(".compute-executable.sha256"))?
            != file_sha256(&artifacts.executable)?
    {
        return Err(ComputeError::UntrustedRuntime);
    }
    Ok(verified)
}

fn validate_runtime_response(
    runtime: &ComputeRuntime,
    call: &ComputeCall,
) -> Result<(), ComputeError> {
    if runtime.deno != DENO_VERSION || runtime.v8.is_empty() || runtime.language != call.language {
        return Err(ComputeError::UntrustedRuntime);
    }
    match call.language {
        ComputeLanguage::Python => {
            if runtime.pyodide.as_deref() != Some(PYODIDE_VERSION)
                || runtime
                    .python
                    .as_ref()
                    .is_none_or(|version| !version.starts_with(PYTHON_VERSION_PREFIX))
                || runtime.packages.as_ref().is_none_or(|packages| {
                    packages
                        .keys()
                        .any(|name| expected_package_version(name).is_none())
                        || call.packages.iter().any(|name| {
                            packages
                                .get(name)
                                .zip(expected_package_version(name))
                                .is_none_or(|(actual, expected)| actual != expected)
                        })
                })
            {
                return Err(ComputeError::UntrustedRuntime);
            }
        }
        ComputeLanguage::Javascript => {
            if runtime.pyodide.is_some() || runtime.python.is_some() || runtime.packages.is_some() {
                return Err(ComputeError::UntrustedRuntime);
            }
        }
    }
    Ok(())
}

fn marker(path: &Path) -> Result<String, ComputeError> {
    if !std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file()) {
        return Err(ComputeError::UntrustedRuntime);
    }
    std::fs::read_to_string(path)
        .map(|value| value.trim().to_owned())
        .map_err(|_| ComputeError::UntrustedRuntime)
}

fn exact_file(path: &Path, expected: &[u8]) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
        && std::fs::read(path).is_ok_and(|value| value == expected)
}

fn file_sha256(path: &Path) -> Result<String, ComputeError> {
    if !std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file()) {
        return Err(ComputeError::UntrustedRuntime);
    }
    std::fs::read(path)
        .map(|bytes| sha256(&bytes))
        .map_err(|_| ComputeError::UntrustedRuntime)
}

fn runtime_read_permission(runtime_dir: &Path) -> Result<(PathBuf, OsString), ComputeError> {
    let runtime_dir = runtime_dir
        .canonicalize()
        .map_err(|_| ComputeError::UntrustedRuntime)?;
    let mut permission = OsString::from("--allow-read=");
    permission.push(&runtime_dir);
    Ok((runtime_dir, permission))
}

fn resolve_path_executable(name: &str) -> Result<PathBuf, ComputeError> {
    let path = std::env::var_os("PATH").ok_or(ComputeError::MissingRuntime)?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
        #[cfg(windows)]
        {
            let with_exe = directory.join(format!("{name}.exe"));
            if with_exe.is_file() {
                return Ok(with_exe);
            }
        }
    }
    Err(ComputeError::MissingRuntime)
}

async fn read_json_line<R>(reader: &mut R, limit: usize) -> Result<HostEnvelope, ComputeError>
where
    R: AsyncBufReadExt + Unpin,
{
    let mut bytes = Vec::new();
    let read = reader.read_until(b'\n', &mut bytes).await?;
    if read == 0 && bytes.is_empty() {
        return Err(ComputeError::Protocol);
    }
    if bytes.len() > limit {
        return Err(ComputeError::FrameTooLarge);
    }
    serde_json::from_slice(trim_ascii(&bytes)).map_err(|_| ComputeError::Protocol)
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

async fn read_stderr<R>(
    mut reader: R,
    buffer: Arc<std::sync::Mutex<Vec<u8>>>,
    limit: usize,
) -> Result<(), ComputeError>
where
    R: AsyncRead + Unpin,
{
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        let mut bytes = buffer.lock().map_err(|_| ComputeError::Protocol)?;
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|size| size > limit)
        {
            return Err(ComputeError::FrameTooLarge);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

struct ComputeProcess {
    child: Child,
    #[cfg(unix)]
    process_group_id: i32,
    #[cfg(unix)]
    lease: Option<ChildProcessLease>,
    #[cfg(windows)]
    job: Option<ChildProcessJob>,
}

impl ComputeProcess {
    async fn spawn(
        artifacts: &ComputeArtifacts,
        deno_cache: &Path,
        language: ComputeLanguage,
        fake_host: Option<&Path>,
    ) -> Result<Self, ComputeError> {
        let (mut command, expected_executable) = if let Some(script) = fake_host {
            let node = resolve_path_executable("node")?;
            let mut command = Command::new(&node);
            command
                .arg(script)
                .current_dir(script.parent().unwrap_or_else(|| Path::new(".")))
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("NO_COLOR", "1");
            (command, node)
        } else {
            let (runtime_dir, read_permission) = runtime_read_permission(&artifacts.runtime_dir)?;
            let host = match language {
                ComputeLanguage::Python => runtime_dir.join("bootstrap.mjs"),
                ComputeLanguage::Javascript => runtime_dir.join("javascript-host.mjs"),
            };
            let mut command = Command::new(&artifacts.executable);
            command
                .args([
                    "run",
                    "--no-config",
                    "--no-lock",
                    "--no-npm",
                    "--node-modules-dir=none",
                    "--cached-only",
                    "--no-prompt",
                    "--unstable-worker-options",
                    "--v8-flags=--max-old-space-size=512",
                    "--deny-import",
                    "--deny-net",
                    "--deny-env",
                    "--deny-run",
                    "--deny-write",
                    "--deny-sys",
                    "--deny-ffi",
                ])
                .arg(read_permission)
                .arg(host)
                .current_dir(runtime_dir)
                .env_clear()
                .env("DENO_DIR", deno_cache)
                .env("DENO_NO_UPDATE_CHECK", "1")
                .env("NO_COLOR", "1");
            (command, artifacts.executable.clone())
        };
        #[cfg(windows)]
        let _ = expected_executable;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        ChildProcessJob::configure_command(&mut command);
        let mut child = command.spawn()?;
        #[cfg(unix)]
        let process_group_id = child.id().ok_or(ComputeError::Protocol)? as i32;
        #[cfg(windows)]
        let job = match ChildProcessJob::assign(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.start_kill();
                let _ = timeout(STOP_TIMEOUT, child.wait()).await;
                return Err(error.into());
            }
        };
        #[cfg(unix)]
        let lease = match ChildProcessLease::register(
            &artifacts.lease_dir,
            ProcessKind::Compute,
            process_group_id,
            process_group_id,
            &expected_executable,
        ) {
            Ok(lease) => lease,
            Err(error) => {
                let _ = signal_process_group(process_group_id, libc::SIGKILL);
                let _ = timeout(STOP_TIMEOUT, child.wait()).await;
                return Err(error.into());
            }
        };
        Ok(Self {
            child,
            #[cfg(unix)]
            process_group_id,
            #[cfg(unix)]
            lease: Some(lease),
            #[cfg(windows)]
            job: Some(job),
        })
    }

    async fn complete(&mut self) -> Result<(), ComputeError> {
        #[cfg(unix)]
        {
            timeout(
                STOP_TIMEOUT,
                wait_for_process_group_exit(self.process_group_id),
            )
            .await
            .map_err(|_| ComputeError::Timeout)??;
            if let Some(lease) = self.lease.take() {
                lease.complete()?;
            }
        }
        #[cfg(windows)]
        self.job.take();
        Ok(())
    }
}

impl Drop for ComputeProcess {
    fn drop(&mut self) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn python_call(packages: &[&str], source: &str) -> ComputeCall {
        ComputeCall {
            language: ComputeLanguage::Python,
            source: source.into(),
            inputs: json!({"value": 1}),
            packages: packages.iter().map(|name| (*name).to_owned()).collect(),
            seed: 7,
        }
    }

    fn javascript_call(source: &str) -> ComputeCall {
        ComputeCall {
            language: ComputeLanguage::Javascript,
            source: source.into(),
            inputs: json!({"value": 1}),
            packages: Vec::new(),
            seed: 7,
        }
    }

    fn test_session(temporary: &tempfile::TempDir) -> TurnComputeSession {
        let lease_dir = temporary.path().join("leases");
        crate::process_lease::prepare_lease_directory(&lease_dir).unwrap();
        let scratch = temporary.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        TurnComputeSession::for_test(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../compute/test-host.mjs"),
            scratch,
            lease_dir,
        )
    }

    #[test]
    fn request_accepts_only_bundled_packages_and_bounded_source() {
        let call = python_call(&["numpy", "pandas"], "def main(inputs):\n    return inputs");
        assert!(call.validate().is_ok());
        let mut invalid = call.clone();
        invalid.packages.push("requests".into());
        assert!(matches!(
            invalid.validate(),
            Err(ComputeError::InvalidRequest)
        ));
        let mut javascript = javascript_call("async function main(inputs) { return inputs; }");
        assert!(javascript.validate().is_ok());
        javascript.packages.push("numpy".into());
        assert!(matches!(
            javascript.validate(),
            Err(ComputeError::InvalidRequest)
        ));
        let import = javascript_call(
            "async function main() { return import('data:text/javascript,export default 1'); }",
        );
        assert!(matches!(
            import.validate(),
            Err(ComputeError::InvalidRequest)
        ));
    }

    #[test]
    fn bundled_runtime_contract_versions_are_locked() {
        let manifest: Value = serde_json::from_slice(RUNTIME_MANIFEST).unwrap();
        assert_eq!(manifest["deno"]["version"], DENO_VERSION);
        assert_eq!(manifest["pyodide"]["version"], PYODIDE_VERSION);
        let bootstrap = std::str::from_utf8(BOOTSTRAP).unwrap();
        let javascript_host = std::str::from_utf8(JAVASCRIPT_HOST).unwrap();
        let contract = std::str::from_utf8(CONTRACT).unwrap();
        assert!(bootstrap.contains("Object.freeze(Object.create(null))"));
        assert!(bootstrap.contains("parseHostMessage"));
        assert!(bootstrap.contains("_importlib.import_module(name)"));
        assert!(javascript_host.contains("permissions: \"none\""));
        assert!(!javascript_host.to_ascii_lowercase().contains("pyodide"));
        assert!(contract.contains("scikit-learn"));
        assert!(contract.contains("guruterminal-compute/2"));
    }

    #[test]
    fn runtime_response_rejects_identity_drift() {
        let runtime = ComputeRuntime {
            language: ComputeLanguage::Python,
            deno: DENO_VERSION.into(),
            v8: "15.0".into(),
            pyodide: Some(PYODIDE_VERSION.into()),
            python: Some("3.14.2".into()),
            packages: Some(BTreeMap::from([("numpy".into(), "2.4.3".into())])),
        };
        let call = python_call(&["numpy"], "def main(inputs):\n    return inputs");
        assert!(validate_runtime_response(&runtime, &call).is_ok());
        let superset = ComputeRuntime {
            packages: Some(BTreeMap::from([
                ("numpy".into(), "2.4.3".into()),
                ("pandas".into(), "3.0.2".into()),
            ])),
            ..runtime.clone()
        };
        assert!(validate_runtime_response(&superset, &call).is_ok());
        let missing = python_call(&["pandas"], "def main(inputs):\n    return inputs");
        assert!(matches!(
            validate_runtime_response(&runtime, &missing),
            Err(ComputeError::UntrustedRuntime)
        ));
        let mut drifted = runtime;
        drifted.deno = "latest".into();
        assert!(matches!(
            validate_runtime_response(&drifted, &call),
            Err(ComputeError::UntrustedRuntime)
        ));
        let javascript_runtime = ComputeRuntime {
            language: ComputeLanguage::Javascript,
            deno: DENO_VERSION.into(),
            v8: "15.0".into(),
            pyodide: None,
            python: None,
            packages: None,
        };
        let javascript = javascript_call("async function main(inputs) { return inputs; }");
        assert!(validate_runtime_response(&javascript_runtime, &javascript).is_ok());
        let mut contaminated = javascript_runtime;
        contaminated.pyodide = Some(PYODIDE_VERSION.into());
        assert!(matches!(
            validate_runtime_response(&contaminated, &javascript),
            Err(ComputeError::UntrustedRuntime)
        ));
    }

    #[test]
    fn compute_read_permission_is_bound_to_the_absolute_runtime_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime_dir = temporary.path().join("Guru Terminal.app/compute worker");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let (absolute_runtime, permission) = runtime_read_permission(&runtime_dir).unwrap();
        let mut expected = OsString::from("--allow-read=");
        expected.push(&absolute_runtime);
        assert_eq!(permission, expected);
        assert_ne!(permission, OsString::from("--allow-read=."));
    }

    #[tokio::test]
    async fn same_python_package_set_reuses_one_process_with_independent_calls() {
        let temporary = tempfile::tempdir().unwrap();
        let session = test_session(&temporary);
        let first = session
            .run(python_call(
                &["numpy", "pandas"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        let second = session
            .run(python_call(
                &["pandas", "numpy"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        assert_eq!(first["data"]["pid"], second["data"]["pid"]);
        assert_eq!(first["data"]["calls"], 1);
        assert_eq!(second["data"]["calls"], 2);
        assert_eq!(first["receipt"]["packages"], json!(["numpy", "pandas"]));
        assert_eq!(second["receipt"]["packages"], json!(["pandas", "numpy"]));
        assert_eq!(first["receipt"]["language"], "python");
        session.shutdown().await;
    }

    #[tokio::test]
    async fn smaller_python_package_set_reuses_the_loaded_host() {
        let temporary = tempfile::tempdir().unwrap();
        let session = test_session(&temporary);
        let first = session
            .run(python_call(
                &["numpy", "pandas"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        let second = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        assert_eq!(first["data"]["pid"], second["data"]["pid"]);
        assert_eq!(second["data"]["calls"], 2);
        assert_eq!(second["receipt"]["packages"], json!(["numpy"]));
        session.shutdown().await;
    }

    #[tokio::test]
    async fn package_set_change_timeout_and_crash_replace_the_python_process() {
        let temporary = tempfile::tempdir().unwrap();
        let session = test_session(&temporary);
        let first = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        let replaced = session
            .run(python_call(
                &["pandas"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        assert_ne!(first["data"]["pid"], replaced["data"]["pid"]);
        assert_eq!(replaced["data"]["calls"], 1);

        let crashed = session
            .run(python_call(
                &["pandas"],
                "def main(inputs):\n    return '__crash__'",
            ))
            .await;
        assert!(crashed.is_err());
        let recovered = session
            .run(python_call(
                &["pandas"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        assert_ne!(replaced["data"]["pid"], recovered["data"]["pid"]);

        let timed_out = session
            .run(python_call(
                &["pandas"],
                "def main(inputs):\n    return '__timeout__'",
            ))
            .await;
        assert!(matches!(timed_out, Err(ComputeError::Timeout)));
        let after_timeout = session
            .run(python_call(
                &["pandas"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        assert_ne!(recovered["data"]["pid"], after_timeout["data"]["pid"]);
        session.shutdown().await;
    }

    #[tokio::test]
    async fn cell_failure_and_in_host_timeout_reuse_the_retained_host() {
        let temporary = tempfile::tempdir().unwrap();
        let session = test_session(&temporary);
        let first = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        let failed = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return '__fail__'",
            ))
            .await;
        assert!(matches!(failed, Err(ComputeError::Remote(_))));
        let recovered = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        assert_eq!(first["data"]["pid"], recovered["data"]["pid"]);
        assert_eq!(first["data"]["calls"], 1);
        assert_eq!(recovered["data"]["calls"], 3);

        let javascript = session
            .run(javascript_call(
                "async function main(inputs) { return inputs; }",
            ))
            .await
            .unwrap();
        let javascript_failed = session
            .run(javascript_call(
                "async function main(inputs) { return '__fail__'; }",
            ))
            .await;
        assert!(matches!(javascript_failed, Err(ComputeError::Remote(_))));
        let javascript_recovered = session
            .run(javascript_call(
                "async function main(inputs) { return inputs; }",
            ))
            .await
            .unwrap();
        assert_eq!(
            javascript["data"]["pid"],
            javascript_recovered["data"]["pid"]
        );
        assert_eq!(javascript_recovered["data"]["calls"], 3);

        let timed_out = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return '__cell_timeout__'",
            ))
            .await;
        assert!(matches!(timed_out, Err(ComputeError::Timeout)));
        let after_timeout = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        assert_eq!(recovered["data"]["pid"], after_timeout["data"]["pid"]);
        assert_eq!(after_timeout["data"]["calls"], 5);

        let javascript_timed_out = session
            .run(javascript_call(
                "async function main(inputs) { return '__cell_timeout__'; }",
            ))
            .await;
        assert!(matches!(javascript_timed_out, Err(ComputeError::Timeout)));
        let javascript_after_timeout = session
            .run(javascript_call(
                "async function main(inputs) { return inputs; }",
            ))
            .await
            .unwrap();
        assert_eq!(
            javascript_recovered["data"]["pid"],
            javascript_after_timeout["data"]["pid"]
        );
        assert_eq!(javascript_after_timeout["data"]["calls"], 5);
        session.shutdown().await;
    }

    #[tokio::test]
    async fn javascript_hang_and_crash_replace_the_retained_host() {
        let temporary = tempfile::tempdir().unwrap();
        let session = test_session(&temporary);
        let first = session
            .run(javascript_call(
                "async function main(inputs) { return inputs; }",
            ))
            .await
            .unwrap();

        let crashed = session
            .run(javascript_call(
                "async function main(inputs) { return '__crash__'; }",
            ))
            .await;
        assert!(crashed.is_err());
        let after_crash = session
            .run(javascript_call(
                "async function main(inputs) { return inputs; }",
            ))
            .await
            .unwrap();
        assert_ne!(first["data"]["pid"], after_crash["data"]["pid"]);
        assert_eq!(after_crash["data"]["calls"], 1);

        let timed_out = session
            .run(javascript_call(
                "async function main(inputs) { return '__timeout__'; }",
            ))
            .await;
        assert!(matches!(timed_out, Err(ComputeError::Timeout)));
        let after_hang = session
            .run(javascript_call(
                "async function main(inputs) { return inputs; }",
            ))
            .await
            .unwrap();
        assert_ne!(after_crash["data"]["pid"], after_hang["data"]["pid"]);
        assert_eq!(after_hang["data"]["calls"], 1);
        session.shutdown().await;
    }

    #[tokio::test]
    async fn dropping_an_in_flight_run_discards_the_host() {
        let temporary = tempfile::tempdir().unwrap();
        let session = test_session(&temporary);
        let first = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        {
            let hanging = session.run(python_call(
                &["numpy"],
                "def main(inputs):\n    return '__timeout__'",
            ));
            tokio::pin!(hanging);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                _ = &mut hanging => panic!("timeout cell finished before it was cancelled"),
            }
        }
        let recovered = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        assert_ne!(first["data"]["pid"], recovered["data"]["pid"]);

        let javascript = session
            .run(javascript_call(
                "async function main(inputs) { return inputs; }",
            ))
            .await
            .unwrap();
        {
            let hanging = session.run(javascript_call(
                "async function main(inputs) { return '__timeout__'; }",
            ));
            tokio::pin!(hanging);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                _ = &mut hanging => panic!("timeout cell finished before it was cancelled"),
            }
        }
        let javascript_recovered = session
            .run(javascript_call(
                "async function main(inputs) { return inputs; }",
            ))
            .await
            .unwrap();
        assert_ne!(
            javascript["data"]["pid"],
            javascript_recovered["data"]["pid"]
        );
        session.shutdown().await;
    }

    #[tokio::test]
    async fn javascript_and_python_hosts_are_separate_processes() {
        let temporary = tempfile::tempdir().unwrap();
        let session = test_session(&temporary);
        let python = session
            .run(python_call(&[], "def main(inputs):\n    return inputs"))
            .await
            .unwrap();
        let javascript = session
            .run(javascript_call(
                "async function main(inputs) { return inputs; }",
            ))
            .await
            .unwrap();
        assert_ne!(python["data"]["pid"], javascript["data"]["pid"]);
        assert_eq!(javascript["runtime"]["language"], "javascript");
        assert!(javascript["runtime"].get("pyodide").is_none());
        let javascript_again = session
            .run(javascript_call(
                "async function main(inputs) { return inputs; }",
            ))
            .await
            .unwrap();
        assert_eq!(javascript["data"]["pid"], javascript_again["data"]["pid"]);
        session.shutdown().await;
        assert!(!temporary
            .path()
            .join("scratch/python")
            .join("running")
            .exists());
    }

    #[tokio::test]
    async fn shutdown_stops_retained_hosts() {
        let temporary = tempfile::tempdir().unwrap();
        let session = test_session(&temporary);
        let first = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        session.shutdown().await;
        let second = session
            .run(python_call(
                &["numpy"],
                "def main(inputs):\n    return inputs",
            ))
            .await
            .unwrap();
        assert_ne!(first["data"]["pid"], second["data"]["pid"]);
        session.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "requires the platform compute runtime staged under src-tauri/resources"]
    async fn staged_runtime_executes_scientific_packages_without_host_capabilities() {
        let runtime_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/pi-runtime/compute-worker");
        #[cfg(windows)]
        let executable = runtime_dir.join("guruterminal-compute.exe");
        #[cfg(not(windows))]
        let executable = runtime_dir.join("guruterminal-compute");
        let temporary = tempfile::tempdir().unwrap();
        let lease_dir = temporary.path().join("leases");
        crate::process_lease::prepare_lease_directory(&lease_dir).unwrap();
        let artifacts = ComputeArtifacts {
            executable,
            bootstrap: runtime_dir.join("bootstrap.mjs"),
            runtime_dir,
            lease_dir,
        };
        let session = TurnComputeSession::new(Some(artifacts), temporary.path().join("run"));
        let first = session
            .run(ComputeCall {
                language: ComputeLanguage::Python,
                source: r#"def main(inputs):
    import js
    import numpy as np
    import pandas as pd
    import scipy
    import sklearn
    import statsmodels
    values = np.array(inputs["values"])
    np.guruterminal_cell_marker = "must not survive"
    return {
        "sum": values.sum(),
        "table": pd.DataFrame({"value": values}),
        "deno_visible": hasattr(js, "Deno"),
        "random": float(np.random.random()),
        "versions": {
            "scipy": scipy.__version__,
            "statsmodels": statsmodels.__version__,
            "scikit-learn": sklearn.__version__,
        },
    }
"#
                .into(),
                inputs: json!({"values": [1, 2, 3]}),
                packages: vec![
                    "numpy".into(),
                    "pandas".into(),
                    "scipy".into(),
                    "statsmodels".into(),
                    "scikit-learn".into(),
                ],
                seed: 11,
            })
            .await
            .unwrap();
        let second = session
            .run(ComputeCall {
                language: ComputeLanguage::Python,
                source: r#"def main(inputs):
    import numpy as np
    return {
        "random": float(np.random.random()),
        "seen_prior": "values" in globals(),
        "module_mutation_survived": hasattr(np, "guruterminal_cell_marker"),
    }
"#
                .into(),
                inputs: json!({}),
                packages: vec![
                    "numpy".into(),
                    "pandas".into(),
                    "scipy".into(),
                    "statsmodels".into(),
                    "scikit-learn".into(),
                ],
                seed: 11,
            })
            .await
            .unwrap();
        assert_eq!(first["data"]["sum"], 6);
        assert_eq!(first["data"]["table"]["kind"], "table");
        assert_eq!(first["data"]["deno_visible"], false);
        assert_eq!(first["data"]["versions"]["scipy"], "1.18.0");
        assert_eq!(first["data"]["versions"]["statsmodels"], "0.14.6");
        assert_eq!(first["data"]["versions"]["scikit-learn"], "1.8.0");
        assert_eq!(first["receipt"]["seed"], 11);
        assert_eq!(first["runtime"]["pyodide"], PYODIDE_VERSION);
        assert_eq!(first["data"]["random"], second["data"]["random"]);
        assert_eq!(second["data"]["seen_prior"], false);
        assert_eq!(second["data"]["module_mutation_survived"], false);
        let import = javascript_call(
            "async function main() { return import('data:text/javascript,export default 1'); }",
        );
        assert!(matches!(
            session.run(import).await,
            Err(ComputeError::InvalidRequest)
        ));
        let javascript = session
            .run(javascript_call(
                r#"async function main(inputs) {
  return {
    value: inputs.value * 2,
    random: Math.random(),
    deno: typeof Deno,
    fetch: typeof fetch,
  };
}"#,
            ))
            .await
            .unwrap();
        assert_eq!(javascript["data"]["value"], 2);
        assert_eq!(javascript["data"]["deno"], "undefined");
        assert_eq!(javascript["data"]["fetch"], "undefined");
        session.shutdown().await;
    }
}
