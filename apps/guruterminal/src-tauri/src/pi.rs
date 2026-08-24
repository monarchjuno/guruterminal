use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(any(windows, not(debug_assertions)))]
use std::fs::File;
#[cfg(not(debug_assertions))]
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

#[cfg(windows)]
use crate::process_lease::ChildProcessJob;
#[cfg(unix)]
use crate::process_lease::{
    signal_process_group, terminate_and_reap_process_group, wait_for_process_group_exit,
    ChildProcessLease, ProcessKind,
};
#[cfg(windows)]
use crate::windows_fs::{metadata_is_reparse, open_directory_no_reparse, open_regular_no_reparse};
use crate::{
    agent_harness,
    artifact_trust::{ensure_private_directory, verify_executable, VerifiedExecutable},
    process_lease::ProcessLeaseError,
    settings::{PI_THINKING_LEVELS, PROVIDER_CREDENTIAL_ENVIRONMENTS},
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter},
    process::{Child, ChildStdin, Command},
    sync::{broadcast, Mutex},
    task::JoinHandle,
    time::timeout,
};

#[cfg(not(debug_assertions))]
use std::sync::OnceLock;

pub const PI_VERSION: &str = "0.84.2";
const GURUTERMINAL_SYSTEM_PROMPT: &str = include_str!("../../agent/SYSTEM.md");
pub const PI_DARWIN_ARM64_ARCHIVE_SHA256: &str =
    "c996e888b7f7dce44bcf24f69176ac646c44139d3916bd49a6b28e5a8c5e3a65";
pub const PI_WINDOWS_X64_ARCHIVE_SHA256: &str =
    "741fc1ae1afecb573ac2888e011188ff446b3940f4aabe1583f60bf55be8a3d0";
const MAX_RPC_FRAME_BYTES: usize = 8 * 1024 * 1024;
const PI_EVENT_BUFFER_CAPACITY: usize = 1_024;
const MAX_HOST_CONTEXT_BYTES: usize = 64 * 1024;
const RPC_WRITE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
pub enum PiError {
    #[error("Pi executable is missing: {0}")]
    MissingExecutable(PathBuf),
    #[error("Pi runtime directory is missing: {0}")]
    MissingRuntime(PathBuf),
    #[error("Pi runtime metadata does not match the pinned release")]
    UntrustedRuntime,
    #[error("Guru Terminal Pi extension is missing: {0}")]
    MissingExtension(PathBuf),
    #[error("unsupported provider credential environment name")]
    UnsupportedCredential,
    #[error("invalid Pi launch value")]
    InvalidLaunchValue,
    #[error("Pi I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("Pi RPC frame exceeded {MAX_RPC_FRAME_BYTES} bytes")]
    FrameTooLarge,
    #[error("Pi RPC returned malformed JSON")]
    MalformedFrame,
    #[error("Pi RPC write timed out")]
    WriteTimeout,
    #[error("Pi process did not stop")]
    StopTimeout,
    #[error("Pi process ownership failed: {0}")]
    Lease(#[from] ProcessLeaseError),
}

#[derive(Clone)]
pub struct PiLaunchConfig {
    pub executable: PathBuf,
    pub runtime_dir: PathBuf,
    pub extension: PathBuf,
    pub system_prompt: PathBuf,
    pub agent_data_dir: PathBuf,
    pub working_dir: PathBuf,
    pub private_run_dir: PathBuf,
    pub lease_dir: PathBuf,
    pub broker_socket: PathBuf,
    pub broker_token: String,
    pub provider: String,
    pub model: String,
    pub thinking_level: String,
    pub run_options: BTreeMap<String, String>,
    pub provider_credential: Option<(String, String)>,
    pub host_context: Option<String>,
    pub skill_files: Vec<PathBuf>,
    pub session: Option<PiSessionConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PiSessionConfig {
    pub id: String,
    pub directory: PathBuf,
}

#[derive(Clone, Debug)]
pub struct PiImageContent {
    pub data: String,
    pub mime_type: String,
}

impl PiLaunchConfig {
    pub fn with_session(mut self, session: PiSessionConfig) -> Result<Self, PiError> {
        validate_session_config(&session)?;
        self.session = Some(session);
        Ok(self)
    }

    pub fn with_host_context(mut self, host_context: String) -> Result<Self, PiError> {
        if host_context.trim().is_empty()
            || host_context.len() > MAX_HOST_CONTEXT_BYTES
            || host_context.contains('\0')
        {
            return Err(PiError::InvalidLaunchValue);
        }
        self.host_context = Some(host_context);
        Ok(self)
    }

    pub fn with_skills(mut self, skill_files: Vec<PathBuf>) -> Result<Self, PiError> {
        let agent_root = self
            .system_prompt
            .parent()
            .ok_or(PiError::InvalidLaunchValue)?;
        for (index, path) in skill_files.iter().enumerate() {
            if agent_harness::validate_skill_path(agent_root, path).is_err() {
                agent_harness::validate_user_skill_path(&self.private_run_dir, path)
                    .map_err(|_| PiError::InvalidLaunchValue)?;
            }
            if skill_files[..index].iter().any(|seen| seen == path) {
                return Err(PiError::InvalidLaunchValue);
            }
        }
        self.skill_files = skill_files;
        Ok(self)
    }

    fn validate(&self) -> Result<(), PiError> {
        if agent_harness::validate_extension_bundle(&self.extension).is_err() {
            return Err(PiError::MissingExtension(self.extension.clone()));
        }
        if !is_exact_regular_file(&self.system_prompt, include_bytes!("../../agent/SYSTEM.md")) {
            return Err(PiError::MissingExtension(self.system_prompt.clone()));
        }
        let agent_root = self
            .system_prompt
            .parent()
            .ok_or(PiError::InvalidLaunchValue)?;
        for (index, path) in self.skill_files.iter().enumerate() {
            if agent_harness::validate_skill_path(agent_root, path).is_err() {
                agent_harness::validate_user_skill_path(&self.private_run_dir, path)
                    .map_err(|_| PiError::InvalidLaunchValue)?;
            }
            if self.skill_files[..index].iter().any(|seen| seen == path) {
                return Err(PiError::InvalidLaunchValue);
            }
        }
        for value in [&self.provider, &self.model, &self.broker_token] {
            if value.is_empty() || value.len() > 512 || value.contains('\0') {
                return Err(PiError::InvalidLaunchValue);
            }
        }
        if !PI_THINKING_LEVELS.contains(&self.thinking_level.as_str()) {
            return Err(PiError::InvalidLaunchValue);
        }
        let run_options =
            serde_json::to_string(&self.run_options).map_err(|_| PiError::InvalidLaunchValue)?;
        if run_options.len() > 4 * 1024 || run_options.contains('\0') {
            return Err(PiError::InvalidLaunchValue);
        }
        let host_context = self
            .host_context
            .as_deref()
            .ok_or(PiError::InvalidLaunchValue)?;
        if host_context.trim().is_empty()
            || host_context.len() > MAX_HOST_CONTEXT_BYTES
            || host_context.contains('\0')
        {
            return Err(PiError::InvalidLaunchValue);
        }
        if let Some((name, secret)) = &self.provider_credential {
            if !PROVIDER_CREDENTIAL_ENVIRONMENTS.contains(&name.as_str())
                || secret.is_empty()
                || secret.contains('\0')
            {
                return Err(PiError::UnsupportedCredential);
            }
        }
        if let Some(session) = &self.session {
            validate_session_config(session)?;
        }
        Ok(())
    }

    pub fn rpc_arguments(&self) -> Vec<String> {
        let mut arguments = vec!["--mode".into(), "rpc".into()];
        if let Some(session) = &self.session {
            arguments.extend([
                "--session-dir".into(),
                session.directory.to_string_lossy().into_owned(),
                "--session-id".into(),
                session.id.clone(),
            ]);
        } else {
            arguments.push("--no-session".into());
        }
        arguments.extend([
            "--no-builtin-tools".into(),
            "--no-extensions".into(),
            "--extension".into(),
            self.extension.to_string_lossy().into_owned(),
            "--system-prompt".into(),
            GURUTERMINAL_SYSTEM_PROMPT.into(),
            "--no-skills".into(),
        ]);
        for skill_file in &self.skill_files {
            arguments.push("--skill".into());
            arguments.push(skill_file.to_string_lossy().into_owned());
        }
        arguments.extend([
            "--no-prompt-templates".into(),
            "--no-themes".into(),
            "--no-context-files".into(),
            "--offline".into(),
            "--provider".into(),
            self.provider.clone(),
            "--model".into(),
            self.model.clone(),
            "--thinking".into(),
            self.thinking_level.clone(),
        ]);
        arguments
    }

    fn environment(&self, host_context_file: &Path) -> BTreeMap<String, String> {
        let mut env = BTreeMap::from([
            (
                "PI_CODING_AGENT_DIR".into(),
                self.agent_data_dir.to_string_lossy().into_owned(),
            ),
            (
                "PI_PACKAGE_DIR".into(),
                self.runtime_dir.to_string_lossy().into_owned(),
            ),
            ("PI_OFFLINE".into(), "1".into()),
            ("PI_TELEMETRY".into(), "0".into()),
            (
                "GURUTERMINAL_BROKER_SOCKET".into(),
                self.broker_socket.to_string_lossy().into_owned(),
            ),
            (
                "GURUTERMINAL_BROKER_TOKEN".into(),
                self.broker_token.clone(),
            ),
            (
                "GURUTERMINAL_HOST_CONTEXT_FILE".into(),
                host_context_file.to_string_lossy().into_owned(),
            ),
        ]);
        if let Some((name, value)) = &self.provider_credential {
            env.insert(name.clone(), value.clone());
        }
        env.insert(
            "GURUTERMINAL_SKILL_FILES".into(),
            serde_json::to_string(
                &self
                    .skill_files
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
            )
            .expect("skill path list serializes"),
        );
        env.insert(
            "GURUTERMINAL_MODEL_RUN_OPTIONS".into(),
            serde_json::to_string(&self.run_options).expect("model run options serialize"),
        );
        env
    }
}

fn validate_session_config(session: &PiSessionConfig) -> Result<(), PiError> {
    if uuid::Uuid::parse_str(&session.id)
        .ok()
        .map(|id| id.to_string())
        .as_deref()
        != Some(session.id.as_str())
        || !session.directory.is_absolute()
    {
        return Err(PiError::InvalidLaunchValue);
    }
    Ok(())
}

#[derive(Clone)]
pub struct PiSupportLaunchConfig {
    pub executable: PathBuf,
    pub runtime_dir: PathBuf,
    pub extension: PathBuf,
    pub agent_data_dir: PathBuf,
    pub private_working_dir: PathBuf,
    pub lease_dir: PathBuf,
    pub result_file: PathBuf,
    pub request_file: Option<PathBuf>,
    pub provider_credential: Option<(String, String)>,
    pub mutation_api_key: Option<String>,
}

impl PiSupportLaunchConfig {
    fn validate(&self) -> Result<(), PiError> {
        if agent_harness::validate_provider_extension_bundle(&self.extension).is_err() {
            return Err(PiError::MissingExtension(self.extension.clone()));
        }
        if self.result_file.as_os_str().is_empty()
            || self
                .request_file
                .as_ref()
                .is_some_and(|path| path.as_os_str().is_empty())
        {
            return Err(PiError::InvalidLaunchValue);
        }
        if let Some((name, secret)) = &self.provider_credential {
            if !PROVIDER_CREDENTIAL_ENVIRONMENTS.contains(&name.as_str())
                || secret.is_empty()
                || secret.contains('\0')
            {
                return Err(PiError::UnsupportedCredential);
            }
        }
        if self.mutation_api_key.as_ref().is_some_and(|secret| {
            secret.is_empty() || secret.len() > 8 * 1024 || secret.contains('\0')
        }) {
            return Err(PiError::UnsupportedCredential);
        }
        Ok(())
    }

    fn rpc_arguments(&self) -> Vec<String> {
        vec![
            "--mode".into(),
            "rpc".into(),
            "--no-session".into(),
            "--no-builtin-tools".into(),
            "--no-extensions".into(),
            "--extension".into(),
            self.extension.to_string_lossy().into_owned(),
            "--no-skills".into(),
            "--no-prompt-templates".into(),
            "--no-themes".into(),
            "--no-context-files".into(),
            "--provider".into(),
            "google".into(),
            "--model".into(),
            "gemini-2.5-flash".into(),
        ]
    }

    fn environment(&self) -> BTreeMap<String, String> {
        let mut environment = BTreeMap::from([
            (
                "PI_CODING_AGENT_DIR".into(),
                self.agent_data_dir.to_string_lossy().into_owned(),
            ),
            (
                "PI_PACKAGE_DIR".into(),
                self.runtime_dir.to_string_lossy().into_owned(),
            ),
            ("PI_TELEMETRY".into(), "0".into()),
            // The support command is intercepted by the bundled extension before
            // any model request. A non-secret bootstrap value lets Pi initialize
            // the RPC session without borrowing a user's provider credential.
            (
                "GEMINI_API_KEY".into(),
                "guruterminal-provider-bootstrap".into(),
            ),
            (
                "GURUTERMINAL_PROVIDER_RESULT_FILE".into(),
                self.result_file.to_string_lossy().into_owned(),
            ),
        ]);
        if let Some(request_file) = &self.request_file {
            environment.insert(
                "GURUTERMINAL_PROVIDER_REQUEST_FILE".into(),
                request_file.to_string_lossy().into_owned(),
            );
        }
        if let Some((name, value)) = &self.provider_credential {
            environment.insert(name.clone(), value.clone());
        }
        if let Some(value) = &self.mutation_api_key {
            environment.insert("GURUTERMINAL_PROVIDER_API_KEY".into(), value.clone());
        }
        environment
    }
}

#[must_use]
struct VerifiedPiRuntime {
    _executable: VerifiedExecutable,
    #[cfg(windows)]
    _tree_handles: Vec<File>,
}

fn verify_pi_runtime(executable: &Path, runtime_dir: &Path) -> Result<VerifiedPiRuntime, PiError> {
    let verified_executable =
        verify_executable(executable).map_err(|_| PiError::UntrustedRuntime)?;
    if !runtime_dir.is_dir() {
        return Err(PiError::MissingRuntime(runtime_dir.to_path_buf()));
    }
    let version = std::fs::read_to_string(runtime_dir.join(".pi-version"))
        .map_err(|_| PiError::UntrustedRuntime)?;
    let archive_digest = std::fs::read_to_string(runtime_dir.join(".pi-archive.sha256"))
        .map_err(|_| PiError::UntrustedRuntime)?;
    #[cfg(debug_assertions)]
    let executable_matches_development_metadata = {
        let executable_digest = std::fs::read_to_string(runtime_dir.join(".pi-executable.sha256"))
            .map_err(|_| PiError::UntrustedRuntime)?;
        file_sha256(executable)? == executable_digest.trim()
    };
    #[cfg(not(debug_assertions))]
    let executable_matches_development_metadata = true;
    let package: Value = serde_json::from_slice(
        &std::fs::read(runtime_dir.join("package.json")).map_err(|_| PiError::UntrustedRuntime)?,
    )
    .map_err(|_| PiError::UntrustedRuntime)?;
    let expected_archive_digest = pinned_pi_archive_sha256().ok_or(PiError::UntrustedRuntime)?;
    if version.trim() != PI_VERSION
        || archive_digest.trim() != expected_archive_digest
        || package.get("version").and_then(Value::as_str) != Some(PI_VERSION)
        || !executable_matches_development_metadata
    {
        return Err(PiError::UntrustedRuntime);
    }
    #[cfg(not(debug_assertions))]
    let _tree_handles = verify_pi_runtime_tree(runtime_dir)?;
    Ok(VerifiedPiRuntime {
        _executable: verified_executable,
        #[cfg(all(windows, not(debug_assertions)))]
        _tree_handles,
        #[cfg(all(windows, debug_assertions))]
        _tree_handles: Vec::new(),
    })
}

#[cfg(not(debug_assertions))]
fn verify_pi_runtime_tree(runtime_dir: &Path) -> Result<Vec<File>, PiError> {
    static VERIFIED_TREE: OnceLock<String> = OnceLock::new();
    let expected = env!("GURUTERMINAL_PI_RUNTIME_TREE_SHA256");
    if VERIFIED_TREE.get().is_some_and(|digest| digest == expected) {
        return Ok(Vec::new());
    }
    let (tree_digest, handles) = pi_runtime_tree_digest(runtime_dir)?;
    if tree_digest != expected {
        return Err(PiError::UntrustedRuntime);
    }
    let _ = VERIFIED_TREE.set(tree_digest);
    Ok(handles)
}

#[cfg(not(debug_assertions))]
fn pi_runtime_tree_digest(runtime_dir: &Path) -> Result<(String, Vec<File>), PiError> {
    let mut records = Vec::new();
    let mut handles = Vec::new();
    let mut entries = 0_usize;
    let mut bytes = 0_u64;
    collect_pi_runtime_tree(
        runtime_dir,
        runtime_dir,
        0,
        &mut entries,
        &mut bytes,
        &mut records,
        &mut handles,
    )?;
    records.sort_by(|left, right| left.0.cmp(&right.0));
    let mut tree = Sha256::new();
    for (relative, kind, size, digest) in records {
        tree.update([kind]);
        tree.update((relative.len() as u64).to_be_bytes());
        tree.update(relative.as_bytes());
        tree.update(size.to_be_bytes());
        tree.update(digest);
    }
    Ok((hex::encode(tree.finalize()), handles))
}

#[cfg(not(debug_assertions))]
#[allow(clippy::too_many_arguments)]
fn collect_pi_runtime_tree(
    root: &Path,
    directory: &Path,
    depth: usize,
    entries: &mut usize,
    bytes: &mut u64,
    records: &mut Vec<(String, u8, u64, [u8; 32])>,
    handles: &mut Vec<File>,
) -> Result<(), PiError> {
    const MAX_ENTRIES: usize = 20_000;
    const MAX_DEPTH: usize = 64;
    const MAX_BYTES: u64 = 512 * 1024 * 1024;
    if depth > MAX_DEPTH {
        return Err(PiError::UntrustedRuntime);
    }
    #[cfg(windows)]
    handles.push(open_directory_no_reparse(directory).map_err(|_| PiError::UntrustedRuntime)?);
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        *entries += 1;
        if *entries > MAX_ENTRIES {
            return Err(PiError::UntrustedRuntime);
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink()
            || cfg!(windows) && pi_windows_metadata_is_reparse(&metadata)
        {
            return Err(PiError::UntrustedRuntime);
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| PiError::UntrustedRuntime)?
            .to_str()
            .ok_or(PiError::UntrustedRuntime)?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if metadata.is_dir() {
            records.push((relative, b'd', 0, [0; 32]));
            collect_pi_runtime_tree(root, &path, depth + 1, entries, bytes, records, handles)?;
        } else if metadata.is_file() {
            *bytes = bytes.saturating_add(metadata.len());
            if *bytes > MAX_BYTES {
                return Err(PiError::UntrustedRuntime);
            }
            #[cfg(windows)]
            let mut file = open_regular_no_reparse(&path).map_err(|_| PiError::UntrustedRuntime)?;
            #[cfg(not(windows))]
            let mut file = {
                let mut options = OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
                options.open(&path)?
            };
            let mut digest = Sha256::new();
            let mut total = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                total += read as u64;
                digest.update(&buffer[..read]);
            }
            if total != metadata.len() {
                return Err(PiError::UntrustedRuntime);
            }
            records.push((relative, b'f', total, digest.finalize().into()));
            #[cfg(windows)]
            handles.push(file);
        } else {
            return Err(PiError::UntrustedRuntime);
        }
    }
    Ok(())
}

#[cfg(all(windows, not(debug_assertions)))]
fn pi_windows_metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata_is_reparse(metadata)
}

#[cfg(all(not(windows), not(debug_assertions)))]
fn pi_windows_metadata_is_reparse(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn pinned_pi_archive_sha256() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Some(PI_DARWIN_ARM64_ARCHIVE_SHA256);
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Some(PI_WINDOWS_X64_ARCHIVE_SHA256);
    #[allow(unreachable_code)]
    None
}

struct HostContextFile(PathBuf);

impl HostContextFile {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for HostContextFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_host_context_file(config: &PiLaunchConfig) -> Result<HostContextFile, PiError> {
    let metadata = std::fs::symlink_metadata(&config.private_run_dir)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PiError::InvalidLaunchValue);
    }
    #[cfg(unix)]
    std::fs::set_permissions(
        &config.private_run_dir,
        std::fs::Permissions::from_mode(0o700),
    )?;

    let host_context = config
        .host_context
        .as_deref()
        .ok_or(PiError::InvalidLaunchValue)?;
    let path = config.private_run_dir.join(format!(
        ".guruterminal-host-context-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    std::io::Write::write_all(&mut file, host_context.as_bytes())?;
    file.sync_all()?;
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(HostContextFile(path))
}

fn file_sha256(path: &Path) -> Result<String, PiError> {
    let metadata = std::fs::symlink_metadata(path).map_err(PiError::Io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PiError::UntrustedRuntime);
    }
    Ok(hex::encode(Sha256::digest(std::fs::read(path)?)))
}

fn is_exact_regular_file(path: &Path, expected: &[u8]) -> bool {
    file_sha256(path).is_ok_and(|digest| digest == hex::encode(Sha256::digest(expected)))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PiEvent {
    Rpc { payload: Value },
    ProtocolError { message: String },
    Exited,
}

pub struct PiProcess {
    child: Child,
    #[cfg(unix)]
    process_group_id: i32,
    #[cfg(unix)]
    lease: Option<ChildProcessLease>,
    #[cfg(windows)]
    job: Option<ChildProcessJob>,
    stdin: Arc<Mutex<BufWriter<ChildStdin>>>,
    events: broadcast::Sender<PiEvent>,
    reader: JoinHandle<()>,
    next_id: AtomicU64,
    host_context_file: Option<HostContextFile>,
}

impl Drop for PiProcess {
    fn drop(&mut self) {
        self.reader.abort();
        self.host_context_file.take();
        #[cfg(unix)]
        {
            if let Some(lease) = self.lease.take() {
                terminate_and_reap_process_group(self.process_group_id, lease);
            } else {
                let _ = signal_process_group(self.process_group_id, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            if let Some(job) = &self.job {
                let _ = job.terminate();
            }
            #[cfg(not(windows))]
            let _ = self.child.start_kill();
        }
    }
}

impl PiProcess {
    pub async fn spawn(config: PiLaunchConfig) -> Result<Self, PiError> {
        config.validate()?;
        for directory in [
            &config.agent_data_dir,
            &config.working_dir,
            &config.private_run_dir,
        ] {
            ensure_private_directory(directory).map_err(|_| PiError::InvalidLaunchValue)?;
        }
        if let Some(session) = &config.session {
            ensure_private_directory(&session.directory)
                .map_err(|_| PiError::InvalidLaunchValue)?;
        }
        let host_context_file = write_host_context_file(&config)?;

        Self::spawn_command(
            &config.executable,
            &config.runtime_dir,
            config.rpc_arguments(),
            &config.working_dir,
            config.environment(host_context_file.path()),
            &config.lease_dir,
            Some(host_context_file),
        )
        .await
    }

    pub async fn spawn_support(config: PiSupportLaunchConfig) -> Result<Self, PiError> {
        config.validate()?;
        std::fs::create_dir_all(&config.agent_data_dir)?;
        std::fs::create_dir_all(&config.private_working_dir)?;
        Self::spawn_command(
            &config.executable,
            &config.runtime_dir,
            config.rpc_arguments(),
            &config.private_working_dir,
            config.environment(),
            &config.lease_dir,
            None,
        )
        .await
    }

    async fn spawn_command(
        executable: &Path,
        runtime_dir: &Path,
        arguments: Vec<String>,
        working_dir: &Path,
        environment: BTreeMap<String, String>,
        lease_dir: &Path,
        host_context_file: Option<HostContextFile>,
    ) -> Result<Self, PiError> {
        #[cfg(not(unix))]
        let _ = lease_dir;
        let _verified_runtime = verify_pi_runtime(executable, runtime_dir)?;
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .current_dir(working_dir)
            .env_clear()
            .envs(environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        ChildProcessJob::configure_command(&mut command);

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return Err(error.into());
            }
        };
        #[cfg(unix)]
        let process_group_id = child.id().ok_or(PiError::InvalidLaunchValue)? as i32;
        #[cfg(windows)]
        let job = match ChildProcessJob::assign(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.start_kill();
                let _ = timeout(Duration::from_secs(2), child.wait()).await;
                return Err(error.into());
            }
        };
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "Pi stdin was not created"))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "Pi stdout was not created")
        })?;
        #[cfg(unix)]
        let lease = match ChildProcessLease::register(
            lease_dir,
            ProcessKind::Pi,
            process_group_id,
            process_group_id,
            executable,
        ) {
            Ok(lease) => lease,
            Err(error) => {
                let _ = signal_process_group(process_group_id, libc::SIGKILL);
                let _ = timeout(Duration::from_secs(2), child.wait()).await;
                return Err(error.into());
            }
        };
        let (events, _) = broadcast::channel(PI_EVENT_BUFFER_CAPACITY);
        let event_writer = events.clone();
        let reader = tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            loop {
                match read_rpc_frame(&mut stdout).await {
                    Ok(Some(frame)) => match serde_json::from_slice(&frame) {
                        Ok(payload) => {
                            let _ = event_writer.send(PiEvent::Rpc { payload });
                        }
                        Err(_) => {
                            let _ = event_writer.send(PiEvent::ProtocolError {
                                message: "Pi emitted malformed JSON".into(),
                            });
                            break;
                        }
                    },
                    Ok(None) => {
                        let _ = event_writer.send(PiEvent::Exited);
                        break;
                    }
                    Err(error) => {
                        let _ = event_writer.send(PiEvent::ProtocolError {
                            message: error.to_string(),
                        });
                        break;
                    }
                }
            }
        });

        Ok(Self {
            child,
            #[cfg(unix)]
            process_group_id,
            #[cfg(unix)]
            lease: Some(lease),
            #[cfg(windows)]
            job: Some(job),
            stdin: Arc::new(Mutex::new(BufWriter::new(stdin))),
            events,
            reader,
            next_id: AtomicU64::new(1),
            host_context_file,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PiEvent> {
        self.events.subscribe()
    }

    pub fn try_exit_code(&mut self) -> Option<i32> {
        self.child
            .try_wait()
            .ok()
            .flatten()
            .and_then(|status| status.code())
    }

    pub async fn prompt(&self, message: &str) -> Result<u64, PiError> {
        self.prompt_with_images(message, &[]).await
    }

    pub async fn prompt_with_images(
        &self,
        message: &str,
        images: &[PiImageContent],
    ) -> Result<u64, PiError> {
        self.send(prompt_request(message, images)?).await
    }

    /// Queue an instruction for the active Pi turn. Unlike `prompt`, Pi accepts
    /// this command while it is streaming and applies it at its next steering
    /// boundary.
    pub async fn steer(&self, message: &str) -> Result<u64, PiError> {
        self.send(text_request("steer", message)?).await
    }

    pub async fn get_state(&self) -> Result<u64, PiError> {
        self.send(json!({ "type": "get_state" })).await
    }

    pub async fn get_entries(&self, since: Option<&str>) -> Result<u64, PiError> {
        let mut request = json!({ "type": "get_entries" });
        if let Some(since) = since {
            bounded_rpc_value(since)?;
            request
                .as_object_mut()
                .ok_or(PiError::InvalidLaunchValue)?
                .insert("since".into(), Value::String(since.to_owned()));
        }
        self.send(request).await
    }

    pub async fn set_auto_compaction(&self, enabled: bool) -> Result<u64, PiError> {
        self.send(json!({
            "type": "set_auto_compaction",
            "enabled": enabled,
        }))
        .await
    }

    pub async fn set_auto_retry(&self, enabled: bool) -> Result<u64, PiError> {
        self.send(json!({
            "type": "set_auto_retry",
            "enabled": enabled,
        }))
        .await
    }

    pub async fn set_model(&self, provider: &str, model_id: &str) -> Result<u64, PiError> {
        bounded_rpc_value(provider)?;
        bounded_rpc_value(model_id)?;
        self.send(json!({
            "type": "set_model",
            "provider": provider,
            "modelId": model_id,
        }))
        .await
    }

    pub async fn get_available_thinking_levels(&self) -> Result<u64, PiError> {
        self.send(json!({ "type": "get_available_thinking_levels" }))
            .await
    }

    pub async fn set_thinking_level(&self, level: &str) -> Result<u64, PiError> {
        bounded_rpc_value(level)?;
        self.send(json!({
            "type": "set_thinking_level",
            "level": level,
        }))
        .await
    }

    pub async fn abort(&self) -> Result<u64, PiError> {
        self.send(json!({ "type": "abort" })).await
    }

    async fn send(&self, mut request: Value) -> Result<u64, PiError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        request
            .as_object_mut()
            .ok_or(PiError::InvalidLaunchValue)?
            .insert("id".into(), Value::from(id));
        let mut encoded = serde_json::to_vec(&request).map_err(|_| PiError::MalformedFrame)?;
        if encoded.len() > MAX_RPC_FRAME_BYTES {
            return Err(PiError::FrameTooLarge);
        }
        encoded.push(b'\n');
        write_rpc_bytes(&self.stdin, &encoded, RPC_WRITE_TIMEOUT).await?;
        Ok(id)
    }

    async fn stop_owned_process(&mut self) -> Result<(), PiError> {
        #[cfg(unix)]
        {
            signal_process_group(self.process_group_id, libc::SIGTERM)?;
            if self.child.try_wait()?.is_none() {
                timeout(Duration::from_secs(2), self.child.wait())
                    .await
                    .map_err(|_| PiError::StopTimeout)??;
            }
            timeout(
                Duration::from_secs(2),
                wait_for_process_group_exit(self.process_group_id),
            )
            .await
            .map_err(|_| PiError::StopTimeout)??;
            Ok(())
        }
        #[cfg(windows)]
        {
            if let Some(job) = &self.job {
                job.terminate_and_wait(Duration::from_secs(2)).await?;
            }
            if self.child.try_wait()?.is_none() {
                timeout(Duration::from_secs(2), self.child.wait())
                    .await
                    .map_err(|_| PiError::StopTimeout)??;
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            if self.child.try_wait()?.is_none() {
                self.child.start_kill()?;
                timeout(Duration::from_secs(2), self.child.wait())
                    .await
                    .map_err(|_| PiError::StopTimeout)??;
            }
            Ok(())
        }
    }

    async fn force_stop_owned_process(&mut self) -> Result<(), PiError> {
        #[cfg(unix)]
        {
            signal_process_group(self.process_group_id, libc::SIGKILL)?;
            // The process-group observation below is authoritative. A failed
            // `try_wait`/leader reap must not skip the forced group fallback.
            let _ = timeout(Duration::from_secs(2), self.child.wait()).await;
            timeout(
                Duration::from_secs(2),
                wait_for_process_group_exit(self.process_group_id),
            )
            .await
            .map_err(|_| PiError::StopTimeout)??;
            Ok(())
        }
        #[cfg(windows)]
        {
            if let Some(job) = &self.job {
                job.terminate_and_wait(Duration::from_secs(2)).await?;
            } else if self.child.try_wait()?.is_none() {
                self.child.start_kill()?;
            }
            if self.child.try_wait()?.is_none() {
                timeout(Duration::from_secs(2), self.child.wait())
                    .await
                    .map_err(|_| PiError::StopTimeout)??;
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            if self.child.try_wait()?.is_none() {
                self.child.start_kill()?;
                timeout(Duration::from_secs(2), self.child.wait())
                    .await
                    .map_err(|_| PiError::StopTimeout)??;
            }
            Ok(())
        }
    }

    pub async fn shutdown(self, grace: Duration) -> Result<(), PiError> {
        let mut events = self.subscribe();
        let _ = self.abort().await;
        let _ = timeout(grace, async {
            loop {
                match events.recv().await {
                    Ok(PiEvent::Rpc { payload }) if is_agent_settled(&payload) => break,
                    Ok(PiEvent::Exited) | Err(broadcast::error::RecvError::Closed) => break,
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }
        })
        .await;

        self.finish_shutdown().await
    }

    /// Stops a process only after its session-level `agent_settled` boundary.
    /// Sending a late abort here can append a second terminal mutation after
    /// the host has read the cursor that it is about to seal in SQLite.
    pub async fn shutdown_settled(self) -> Result<(), PiError> {
        self.finish_shutdown().await
    }

    async fn finish_shutdown(mut self) -> Result<(), PiError> {
        let stop_result = match self.stop_owned_process().await {
            Ok(()) => Ok(()),
            Err(_) => self.force_stop_owned_process().await,
        };
        self.reader.abort();
        let _ = timeout(Duration::from_secs(2), &mut self.reader).await;
        self.host_context_file.take();
        #[cfg(windows)]
        self.job.take();
        stop_result?;
        #[cfg(unix)]
        if let Some(lease) = self.lease.take() {
            lease.complete()?;
        }
        Ok(())
    }
}

fn text_request(command: &'static str, message: &str) -> Result<Value, PiError> {
    if !matches!(command, "prompt" | "steer")
        || message.trim().is_empty()
        || message.len() > 512 * 1024
        || message.contains('\0')
    {
        return Err(PiError::InvalidLaunchValue);
    }
    Ok(json!({ "type": command, "message": message }))
}

fn prompt_request(message: &str, images: &[PiImageContent]) -> Result<Value, PiError> {
    if images.len() > 4
        || images.iter().any(|image| {
            image.data.is_empty()
                || image.data.len() > 7 * 1024 * 1024
                || !matches!(
                    image.mime_type.as_str(),
                    "image/jpeg" | "image/png" | "image/gif" | "image/webp"
                )
        })
    {
        return Err(PiError::InvalidLaunchValue);
    }
    let mut request = text_request("prompt", message)?;
    if !images.is_empty() {
        request
            .as_object_mut()
            .ok_or(PiError::InvalidLaunchValue)?
            .insert(
                "images".into(),
                Value::Array(
                    images
                        .iter()
                        .map(|image| {
                            json!({
                                "type": "image",
                                "data": image.data,
                                "mimeType": image.mime_type,
                            })
                        })
                        .collect(),
                ),
            );
    }
    Ok(request)
}

fn bounded_rpc_value(value: &str) -> Result<(), PiError> {
    if value.is_empty() || value.len() > 512 || value.contains('\0') {
        Err(PiError::InvalidLaunchValue)
    } else {
        Ok(())
    }
}

async fn write_rpc_bytes<W>(
    writer: &Mutex<W>,
    bytes: &[u8],
    deadline: Duration,
) -> Result<(), PiError>
where
    W: AsyncWrite + Unpin,
{
    timeout(deadline, async {
        let mut writer = writer.lock().await;
        writer.write_all(bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| PiError::WriteTimeout)??;
    Ok(())
}

fn is_agent_settled(payload: &Value) -> bool {
    payload.get("type").and_then(Value::as_str) == Some("agent_settled")
}

async fn read_rpc_frame<R>(reader: &mut R) -> Result<Option<Vec<u8>>, PiError>
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
                Err(PiError::MalformedFrame)
            };
        }
        let (take, complete) = match available.iter().position(|byte| *byte == b'\n') {
            Some(position) => (position, true),
            None => (available.len(), false),
        };
        if frame.len() + take > MAX_RPC_FRAME_BYTES {
            return Err(PiError::FrameTooLarge);
        }
        frame.extend_from_slice(&available[..take]);
        reader.consume(take + usize::from(complete));
        if complete {
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            if frame.is_empty() {
                return Err(PiError::MalformedFrame);
            }
            return Ok(Some(frame));
        }
    }
}

#[cfg(test)]
mod tests;
