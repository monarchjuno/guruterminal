#[cfg(unix)]
use rustix::{
    fd::AsFd,
    fs::{fstat, openat, FileType, Mode, OFlags},
    io::Errno,
};
use serde_json::Value;
#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    time::timeout,
};
const RUNTIME_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RUNTIME_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RUNTIME_STDOUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_RUNTIME_STDERR_BYTES: usize = 64 * 1024;

#[cfg(unix)]
use crate::pinned_root::{PinnedGuruRoot, PinnedRootError};
use crate::{
    artifact_trust::{digest_bounded_regular_file, verify_executable},
    domain::CanonicalMemoryKind,
};

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Guru Terminal Runtime is missing")]
    MissingRuntime,
    #[error("Guru Terminal Runtime is not a trusted app artifact")]
    UntrustedRuntime,
    #[error("Guru Terminal workspace is not initialized")]
    NotInitialized,
    #[error("memory target is outside the Guru Memory tree")]
    InvalidTarget,
    #[error("memory changed after the write was prepared")]
    BeforeHashMismatch,
    #[error("proposed memory does not match the prepared digest")]
    ProposedHashMismatch,
    #[error("memory changed during write and cannot be rolled back safely")]
    RollbackConflict,
    #[error("a previous memory transaction artifact requires recovery")]
    PendingArtifact,
    #[error("Guru Terminal Runtime failed: {0}")]
    Runtime(String),
    #[error("Guru Terminal Runtime timed out")]
    Timeout,
    #[error("Guru Terminal Runtime output exceeded the bounded app contract")]
    OutputTooLarge,
    #[error("Guru Memory exceeds the bounded Runtime input contract")]
    MemoryBoundary,
    #[error("memory transaction failed: {0}")]
    Io(#[from] io::Error),
    #[cfg(unix)]
    #[error("pinned Guru root failed: {0}")]
    PinnedRoot(#[from] PinnedRootError),
    #[error("Guru Terminal Runtime returned malformed JSON")]
    MalformedOutput,
}

#[derive(Clone, Debug)]
pub struct GuruTerminalRuntime {
    executable: PathBuf,
}

mod memory_apply;

#[cfg(all(test, unix))]
pub(crate) use memory_apply::fail_next_memory_sync_after_publish_for_test;
#[cfg(test)]
use memory_apply::MAX_MEMORY_FILE_BYTES;
pub use memory_apply::{sha256, AppliedMemoryChange, StagedMemoryChange};

impl GuruTerminalRuntime {
    pub fn new(executable: PathBuf) -> Result<Self, RuntimeError> {
        let _verified_executable =
            verify_executable(&executable).map_err(|_| RuntimeError::UntrustedRuntime)?;
        Ok(Self { executable })
    }

    /// Returns a bounded digest of the exact trusted Runtime bytes without
    /// disclosing the artifact path.
    pub fn artifact_digest(&self) -> Result<String, RuntimeError> {
        let _verified_executable =
            verify_executable(&self.executable).map_err(|_| RuntimeError::UntrustedRuntime)?;
        digest_bounded_regular_file(&self.executable, MAX_RUNTIME_ARTIFACT_BYTES)
            .map_err(|_| RuntimeError::UntrustedRuntime)
    }

    pub async fn knowledge_search(
        &self,
        workspace: &Path,
        query: &str,
        kind: Option<&str>,
        limit: u8,
        include_revoked: bool,
        as_of: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        if query.is_empty() || query.len() > 4096 || limit == 0 || limit > 20 {
            return Err(RuntimeError::InvalidTarget);
        }
        if let Some(kind) = kind {
            ensure_kind(kind)?;
        }
        preflight_memory_tree(workspace)?;
        let mut args = vec![
            OsString::from("knowledge"),
            OsString::from("search"),
            OsString::from(query),
        ];
        if let Some(kind) = kind {
            args.extend([OsString::from("--kind"), OsString::from(kind)]);
        }
        if include_revoked {
            args.push(OsString::from("--include-revoked"));
        }
        if let Some(as_of) = as_of {
            args.extend([OsString::from("--as-of"), OsString::from(as_of)]);
        }
        args.extend([
            OsString::from("--limit"),
            OsString::from(limit.to_string()),
            OsString::from("--workspace"),
            workspace.as_os_str().to_owned(),
            OsString::from("--json"),
        ]);
        self.run_json(args).await
    }

    /// Runs Runtime retrieval inside the exact directory descriptor captured
    /// for this Guru. The pathname is never re-resolved by the app or child.
    #[cfg(unix)]
    pub async fn knowledge_search_at(
        &self,
        root: &PinnedGuruRoot,
        query: &str,
        kind: Option<&str>,
        limit: u8,
        include_revoked: bool,
        as_of: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        if query.is_empty() || query.len() > 4096 || limit == 0 || limit > 20 {
            return Err(RuntimeError::InvalidTarget);
        }
        if let Some(kind) = kind {
            ensure_kind(kind)?;
        }
        preflight_memory_tree_at(root)?;
        let mut args = vec![
            OsString::from("knowledge"),
            OsString::from("search"),
            OsString::from(query),
        ];
        if let Some(kind) = kind {
            args.extend([OsString::from("--kind"), OsString::from(kind)]);
        }
        if include_revoked {
            args.push(OsString::from("--include-revoked"));
        }
        if let Some(as_of) = as_of {
            args.extend([OsString::from("--as-of"), OsString::from(as_of)]);
        }
        args.extend([
            OsString::from("--limit"),
            OsString::from(limit.to_string()),
            OsString::from("--workspace"),
            OsString::from("."),
            OsString::from("--json"),
        ]);
        self.run_json_at(root, args).await
    }

    pub async fn initialize(&self, workspace: &Path) -> Result<(), RuntimeError> {
        if path_entry_exists(&workspace.join(".guruterminal"))?
            || path_entry_exists(&workspace.join("guruterminal"))?
        {
            return Err(RuntimeError::Runtime(
                "the selected folder already contains Guru Terminal state".into(),
            ));
        }
        self.run_json(vec![
            OsString::from("init"),
            workspace.as_os_str().to_owned(),
            OsString::from("--json"),
        ])
        .await?;
        self.validate(workspace).await
    }

    #[cfg(unix)]
    pub async fn initialize_at(&self, root: &PinnedGuruRoot) -> Result<(), RuntimeError> {
        ensure_uninitialized_layout_at(root)?;
        // `init` accepts its workspace as a positional argument. The child is
        // already inside the pinned descriptor, so `.` cannot resolve to a
        // replacement directory at the stored pathname.
        self.run_json_at(
            root,
            [
                OsString::from("init"),
                OsString::from("."),
                OsString::from("--json"),
            ],
        )
        .await?;
        self.validate_at(root).await
    }

    pub async fn knowledge_read(
        &self,
        workspace: &Path,
        id: &str,
        section: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        if id.is_empty() || id.len() > 512 || id.contains(['\n', '\r', '\0']) {
            return Err(RuntimeError::InvalidTarget);
        }
        preflight_memory_tree(workspace)?;
        let mut args = vec![
            OsString::from("knowledge"),
            OsString::from("read"),
            OsString::from(id),
        ];
        if let Some(section) = section {
            if section.is_empty() || section.len() > 512 || section.contains('\0') {
                return Err(RuntimeError::InvalidTarget);
            }
            args.extend([OsString::from("--section"), OsString::from(section)]);
        }
        args.extend([
            OsString::from("--workspace"),
            workspace.as_os_str().to_owned(),
            OsString::from("--json"),
        ]);
        self.run_json(args).await
    }

    #[cfg(unix)]
    pub async fn knowledge_read_at(
        &self,
        root: &PinnedGuruRoot,
        id: &str,
        section: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        if id.is_empty() || id.len() > 512 || id.contains(['\n', '\r', '\0']) {
            return Err(RuntimeError::InvalidTarget);
        }
        preflight_memory_tree_at(root)?;
        let mut args = vec![
            OsString::from("knowledge"),
            OsString::from("read"),
            OsString::from(id),
        ];
        if let Some(section) = section {
            if section.is_empty() || section.len() > 512 || section.contains('\0') {
                return Err(RuntimeError::InvalidTarget);
            }
            args.extend([OsString::from("--section"), OsString::from(section)]);
        }
        args.extend([
            OsString::from("--workspace"),
            OsString::from("."),
            OsString::from("--json"),
        ]);
        self.run_json_at(root, args).await
    }

    pub async fn knowledge_list(
        &self,
        workspace: &Path,
        kind: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        preflight_memory_tree(workspace)?;
        let mut args = vec![OsString::from("knowledge"), OsString::from("list")];
        if let Some(kind) = kind {
            ensure_kind(kind)?;
            args.extend([OsString::from("--kind"), OsString::from(kind)]);
        }
        args.extend([
            OsString::from("--workspace"),
            workspace.as_os_str().to_owned(),
            OsString::from("--json"),
        ]);
        self.run_json(args).await
    }

    #[cfg(unix)]
    pub async fn knowledge_list_at(
        &self,
        root: &PinnedGuruRoot,
        kind: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        preflight_memory_tree_at(root)?;
        let mut args = vec![OsString::from("knowledge"), OsString::from("list")];
        if let Some(kind) = kind {
            ensure_kind(kind)?;
            args.extend([OsString::from("--kind"), OsString::from(kind)]);
        }
        args.extend([
            OsString::from("--workspace"),
            OsString::from("."),
            OsString::from("--json"),
        ]);
        self.run_json_at(root, args).await
    }

    pub async fn validate(&self, workspace: &Path) -> Result<(), RuntimeError> {
        preflight_memory_tree(workspace)?;
        for command in ["check", "health"] {
            self.run_json(vec![
                OsString::from("knowledge"),
                OsString::from(command),
                OsString::from("--workspace"),
                workspace.as_os_str().to_owned(),
                OsString::from("--json"),
            ])
            .await?;
        }
        Ok(())
    }

    #[cfg(unix)]
    pub async fn validate_at(&self, root: &PinnedGuruRoot) -> Result<(), RuntimeError> {
        preflight_memory_tree_at(root)?;
        for command in ["check", "health"] {
            self.run_json_at(
                root,
                vec![
                    OsString::from("knowledge"),
                    OsString::from(command),
                    OsString::from("--workspace"),
                    OsString::from("."),
                    OsString::from("--json"),
                ],
            )
            .await?;
        }
        Ok(())
    }

    async fn run_json<I>(&self, args: I) -> Result<Value, RuntimeError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let _verified_executable =
            verify_executable(&self.executable).map_err(|_| RuntimeError::UntrustedRuntime)?;
        let mut command = Command::new(&self.executable);
        command
            .args(args)
            .env_clear()
            .env("LANG", "C.UTF-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        execute_json(command).await
    }

    #[cfg(unix)]
    async fn run_json_at<I>(&self, root: &PinnedGuruRoot, args: I) -> Result<Value, RuntimeError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let _verified_executable =
            verify_executable(&self.executable).map_err(|_| RuntimeError::UntrustedRuntime)?;
        let root_descriptor = root.duplicate()?;
        let mut command = Command::new(&self.executable);
        command
            .args(args)
            .env_clear()
            .env("LANG", "C.UTF-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // SAFETY: the closure only calls the async-signal-safe `fchdir` syscall.
        // The owned CLOEXEC duplicate is captured so it remains valid between
        // spawning and exec, then the kernel closes it when exec succeeds.
        unsafe {
            command.pre_exec(move || {
                if libc::fchdir(root_descriptor.as_raw_fd()) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        execute_json(command).await
    }
}

async fn execute_json(mut command: Command) -> Result<Value, RuntimeError> {
    execute_json_with_limits(
        &mut command,
        MAX_RUNTIME_STDOUT_BYTES,
        MAX_RUNTIME_STDERR_BYTES,
    )
    .await
}

async fn execute_json_with_limits(
    command: &mut Command,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<Value, RuntimeError> {
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        RuntimeError::Runtime("Guru Terminal Runtime stdout was not captured".into())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        RuntimeError::Runtime("Guru Terminal Runtime stderr was not captured".into())
    })?;
    let completed = timeout(RUNTIME_TIMEOUT, async {
        let (stdout, stderr, status) = tokio::try_join!(
            read_bounded_runtime_stream(stdout, stdout_limit),
            read_bounded_runtime_stream(stderr, stderr_limit),
            async { child.wait().await.map_err(RuntimeError::Io) },
        )?;
        Ok::<_, RuntimeError>((status, stdout, stderr))
    })
    .await;
    let (status, stdout, stderr) = match completed {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            terminate_runtime_child(&mut child).await;
            return Err(error);
        }
        Err(_) => {
            terminate_runtime_child(&mut child).await;
            return Err(RuntimeError::Timeout);
        }
    };
    if !status.success() {
        return Err(RuntimeError::Runtime(runtime_failure_summary(
            &stdout, &stderr,
        )));
    }
    serde_json::from_slice(&stdout).map_err(|_| RuntimeError::MalformedOutput)
}

fn runtime_failure_summary(stdout: &[u8], stderr: &[u8]) -> String {
    let structured_issue = serde_json::from_slice::<Value>(stdout)
        .ok()
        .and_then(|value| value.get("errors").and_then(Value::as_array).cloned())
        .and_then(|errors| errors.into_iter().next())
        .and_then(|issue| {
            Some(format!(
                "{}: {}: {}",
                issue.get("path")?.as_str()?,
                issue.get("field")?.as_str()?,
                issue.get("message")?.as_str()?,
            ))
        });
    let stderr = String::from_utf8_lossy(stderr);
    structured_issue
        .or_else(|| stderr.lines().next().map(str::to_owned))
        .unwrap_or_else(|| "unknown failure".into())
        .chars()
        .take(300)
        .collect()
}

async fn read_bounded_runtime_stream<R>(
    mut stream: R,
    limit: usize,
) -> Result<Vec<u8>, RuntimeError>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Ok(output);
        }
        if output
            .len()
            .checked_add(read)
            .is_none_or(|size| size > limit)
        {
            return Err(RuntimeError::OutputTooLarge);
        }
        output.extend_from_slice(&buffer[..read]);
    }
}

async fn terminate_runtime_child(child: &mut Child) {
    let _ = child.start_kill();
    let _ = timeout(Duration::from_secs(2), child.wait()).await;
}

fn preflight_memory_tree(workspace: &Path) -> Result<(), RuntimeError> {
    ensure_initialized_layout(workspace)?;
    crate::snapshot::inspect_memory_tree(workspace)
        .map(|_| ())
        .map_err(|_| RuntimeError::MemoryBoundary)
}

#[cfg(unix)]
fn preflight_memory_tree_at(root: &PinnedGuruRoot) -> Result<(), RuntimeError> {
    ensure_initialized_layout_at(root)?;
    crate::snapshot::inspect_memory_tree_at(root)
        .map(|_| ())
        .map_err(|_| RuntimeError::MemoryBoundary)
}

fn ensure_kind(kind: &str) -> Result<(), RuntimeError> {
    CanonicalMemoryKind::from_slug(kind)
        .map(|_| ())
        .ok_or(RuntimeError::InvalidTarget)
}

fn ensure_initialized_layout(workspace: &Path) -> Result<(), RuntimeError> {
    if !workspace.join(".guruterminal/workspace.json").is_file()
        || !workspace.join("guruterminal").is_dir()
        || CanonicalMemoryKind::ALL
            .iter()
            .any(|kind| !workspace.join("guruterminal").join(kind.slug()).is_dir())
    {
        return Err(RuntimeError::NotInitialized);
    }
    Ok(())
}

fn path_entry_exists(path: &Path) -> Result<bool, RuntimeError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(RuntimeError::Io(error)),
    }
}

#[cfg(unix)]
fn ensure_initialized_layout_at(root: &PinnedGuruRoot) -> Result<(), RuntimeError> {
    let internal = root
        .open_directory(Path::new(".guruterminal"))
        .map_err(initialized_root_error)?;
    let marker = openat(
        &internal,
        "workspace.json",
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| match error {
        Errno::NOENT | Errno::LOOP | Errno::NOTDIR => RuntimeError::NotInitialized,
        _ => rustix_io(error),
    })?;
    let marker_metadata = fstat(&marker).map_err(rustix_io)?;
    if FileType::from_raw_mode(marker_metadata.st_mode) != FileType::RegularFile {
        return Err(RuntimeError::NotInitialized);
    }
    root.open_directory(Path::new("guruterminal"))
        .map_err(initialized_root_error)?;
    for kind in CanonicalMemoryKind::ALL {
        root.open_directory(Path::new("guruterminal").join(kind.slug()).as_path())
            .map_err(initialized_root_error)?;
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_uninitialized_layout_at(root: &PinnedGuruRoot) -> Result<(), RuntimeError> {
    if entry_exists_at(root, OsStr::new(".guruterminal"))?
        || entry_exists_at(root, OsStr::new("guruterminal"))?
    {
        return Err(RuntimeError::Runtime(
            "the selected folder already contains Guru Terminal state".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn entry_exists_at<Fd: AsFd>(directory: Fd, name: &OsStr) -> Result<bool, RuntimeError> {
    match openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(_) | Err(Errno::LOOP) => Ok(true),
        Err(Errno::NOENT) => Ok(false),
        Err(error) => Err(rustix_io(error)),
    }
}

#[cfg(unix)]
fn initialized_root_error(error: PinnedRootError) -> RuntimeError {
    match error {
        PinnedRootError::InvalidPath
        | PinnedRootError::IdentityMismatch
        | PinnedRootError::NotDirectory
        | PinnedRootError::UnsupportedPlatform => RuntimeError::NotInitialized,
        PinnedRootError::Io(error)
            if error
                .raw_os_error()
                .is_some_and(|code| matches!(code, libc::ENOENT | libc::ENOTDIR | libc::ELOOP)) =>
        {
            RuntimeError::NotInitialized
        }
        PinnedRootError::Io(error) => RuntimeError::Io(error),
    }
}

#[cfg(unix)]
fn rustix_io(error: Errno) -> RuntimeError {
    RuntimeError::Io(io::Error::from(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::{fs, fs::File};

    #[cfg(unix)]
    fn write_test_runtime(directory: &Path, name: &str, script: &str) -> GuruTerminalRuntime {
        let executable = directory.join(name);
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        GuruTerminalRuntime::new(executable).unwrap()
    }

    #[cfg(unix)]
    fn create_initialized_workspace(path: &Path, marker: &str) {
        fs::create_dir_all(path.join(".guruterminal")).unwrap();
        for kind in ["wiki", "lens", "evidence", "decision"] {
            fs::create_dir_all(path.join("guruterminal").join(kind)).unwrap();
        }
        fs::write(
            path.join(".guruterminal/workspace.json"),
            "{\n  \"schema_version\": 1\n}\n",
        )
        .unwrap();
        fs::write(path.join("runtime-marker"), format!("{marker}\n")).unwrap();
    }

    #[test]
    fn runtime_artifact_digest_tracks_exact_bytes_without_returning_its_path() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("guruterminal-private-location");
        fs::write(&executable, b"runtime-a").unwrap();
        let runtime = GuruTerminalRuntime::new(executable.clone()).unwrap();
        let first = runtime.artifact_digest().unwrap();
        fs::write(&executable, b"runtime-b").unwrap();
        let second = runtime.artifact_digest().unwrap();
        assert_ne!(first, second);
        assert!(!first.contains("guruterminal-private-location"));
        assert!(!second.contains("guruterminal-private-location"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_output_is_bounded_and_the_child_is_stopped_on_overflow() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("guruterminal-output-overflow");
        fs::write(
            &executable,
            "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 4096 ]; do printf x; i=$((i + 1)); done\nexec /bin/sleep 30\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let started = std::time::Instant::now();
        let error = execute_json_with_limits(&mut command, 1_024, 1_024)
            .await
            .unwrap_err();

        assert!(matches!(error, RuntimeError::OutputTooLarge));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_failure_reports_the_first_structured_knowledge_issue() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("guruterminal-structured-error");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' '{\"valid\":false,\"documents\":1,\"errors\":[{\"path\":\"guruterminal/lens/gate.md\",\"field\":\"supports\",\"message\":\"target does not exist: evidence:missing\"}]}'\nprintf '%s\\n' 'guruterminal-core: knowledge check failed' >&2\nexit 1\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let error = execute_json_with_limits(&mut command, 4_096, 4_096)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            RuntimeError::Runtime(message)
                if message == "guruterminal/lens/gate.md: supports: target does not exist: evidence:missing"
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn oversized_memory_is_rejected_before_the_runtime_child_starts() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        create_initialized_workspace(&workspace, "bounded");
        let oversized = workspace.join("guruterminal/lens/oversized.md");
        File::create(&oversized)
            .unwrap()
            .set_len(MAX_MEMORY_FILE_BYTES + 1)
            .unwrap();
        let runtime = write_test_runtime(
            temporary.path(),
            "guruterminal-preflight-test",
            "#!/bin/sh\nprintf invoked > runtime-invoked\nprintf '{}\\n'\n",
        );
        let pinned = PinnedGuruRoot::open_unbound(&workspace).unwrap();

        let error = runtime.knowledge_list_at(&pinned, None).await.unwrap_err();

        assert!(matches!(error, RuntimeError::MemoryBoundary));
        assert!(!workspace.join("runtime-invoked").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pinned_runtime_child_stays_in_a_after_path_is_replaced_by_b() {
        let temporary = tempfile::tempdir().unwrap();
        let root_a = temporary.path().join("guru-a");
        let root_b = temporary.path().join("guru-b");
        let moved_a = temporary.path().join("guru-a-original");
        create_initialized_workspace(&root_a, "A");
        create_initialized_workspace(&root_b, "B");
        let pinned = PinnedGuruRoot::open_unbound(&root_a).unwrap();
        let runtime = write_test_runtime(
            temporary.path(),
            "guruterminal-cwd-test",
            "#!/bin/sh\n\
             if [ \"$#\" -ne 5 ] || [ \"$1\" != knowledge ] || [ \"$2\" != list ] || \
                [ \"$3\" != --workspace ] || [ \"$4\" != . ] || [ \"$5\" != --json ]; then\n\
               printf 'unexpected arguments\\n' >&2\n\
               exit 64\n\
             fi\n\
             IFS= read -r marker < runtime-marker\n\
             printf '{\"marker\":\"%s\"}\\n' \"$marker\"\n",
        );

        fs::rename(&root_a, &moved_a).unwrap();
        fs::rename(&root_b, &root_a).unwrap();

        let result = runtime.knowledge_list_at(&pinned, None).await.unwrap();
        assert_eq!(result["marker"], "A");
        assert_eq!(
            fs::read_to_string(root_a.join("runtime-marker")).unwrap(),
            "B\n"
        );
        assert_eq!(
            fs::read_to_string(moved_a.join("runtime-marker")).unwrap(),
            "A\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pinned_initialize_stays_in_a_and_returns_the_same_identity_after_swap() {
        let temporary = tempfile::tempdir().unwrap();
        let root_a = temporary.path().join("guru-a");
        let root_b = temporary.path().join("guru-b");
        let moved_a = temporary.path().join("guru-a-original");
        fs::create_dir(&root_a).unwrap();
        fs::create_dir(&root_b).unwrap();
        let pinned = PinnedGuruRoot::open_unbound(&root_a).unwrap();
        let captured_identity = pinned.identity().clone();
        let runtime = write_test_runtime(
            temporary.path(),
            "guruterminal-init-test",
            "#!/bin/sh\n\
             if [ \"$1\" = init ]; then\n\
               [ \"$#\" -eq 3 ] && [ \"$2\" = . ] && [ \"$3\" = --json ] || exit 64\n\
               /bin/mkdir .guruterminal guruterminal guruterminal/wiki guruterminal/lens guruterminal/evidence guruterminal/decision || exit 65\n\
               printf '{\"schema_version\":1}\\n' > .guruterminal/workspace.json\n\
             elif [ \"$1\" = knowledge ]; then\n\
               [ \"$#\" -eq 5 ] && [ \"$3\" = --workspace ] && \
                 [ \"$4\" = . ] && [ \"$5\" = --json ] || exit 66\n\
             else\n\
               exit 67\n\
             fi\n\
             printf '{}\\n'\n",
        );

        fs::rename(&root_a, &moved_a).unwrap();
        fs::rename(&root_b, &root_a).unwrap();
        runtime.initialize_at(&pinned).await.unwrap();

        assert_eq!(pinned.identity(), &captured_identity);
        assert_eq!(
            fs::read_to_string(moved_a.join(".guruterminal/workspace.json")).unwrap(),
            "{\"schema_version\":1}\n"
        );
        assert!(moved_a.join("guruterminal/lens").is_dir());
        assert!(!root_a.join(".guruterminal").exists());
        assert!(!root_a.join("guruterminal").exists());
    }
}
