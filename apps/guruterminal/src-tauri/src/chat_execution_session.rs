use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(unix)]
use std::{
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    time::Duration,
};

#[cfg(unix)]
use rustix::{
    fs::{fstat, fsync, openat, unlinkat, AtFlags, FileType, Mode, OFlags},
    io::Errno,
};

use crate::{
    app::CommandError,
    domain::ChatSession,
    pi::PiSessionConfig,
    secure_delete::{PrivateDirectoryGuard, SecureDeletionRoot},
};

#[cfg(windows)]
use crate::windows_fs::metadata_is_reparse;

// This marker belongs in the stable runtime directory rather than the
// disposable JSONL cache. A cold cache wipe must not be able to erase the
// record which says its previous Pi process could still be writing that cache.
#[cfg(unix)]
const UNCONFIRMED_PI_STOP_MARKER: &str = ".guruterminal-pi-stop-quarantine-v1";
#[cfg(unix)]
const MAX_UNCONFIRMED_PI_STOP_MARKER_BYTES: u64 = 64;
#[cfg(unix)]
const UNCONFIRMED_PI_STOP_CONFIRMATION_DEADLINE: Duration = Duration::from_secs(1);

#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
struct UnconfirmedPiStop {
    process_group_id: i32,
    device: u64,
    inode: u64,
}

/// Owns the app-private Pi directory for one Chat thread.
/// SQLite is the transcript authority; this directory holds a digest-bound
/// derived JSONL cache that survives turns until wipe, mismatch, or deletion.
#[derive(Debug)]
pub struct ChatExecutionSession {
    pi_session_id: String,
    directory: PathBuf,
    directory_guard: Option<PrivateDirectoryGuard>,
    runtime_directory: PathBuf,
    runtime_directory_guard: Option<PrivateDirectoryGuard>,
    deletion_root: Arc<SecureDeletionRoot>,
    relative_directory: PathBuf,
}

impl ChatExecutionSession {
    pub fn prepare(
        deletion_root: Arc<SecureDeletionRoot>,
        chat: &ChatSession,
    ) -> Result<Self, CommandError> {
        chat.validate()
            .map_err(|error| CommandError::internal(error.to_string()))?;
        let relative_directory = PathBuf::from("gurus")
            .join(&chat.guru_id)
            .join("pi-sessions")
            .join(&chat.id);
        // The session JSONL is disposable cache. Pi also binds its session
        // handling to the CWD, so keep that private CWD outside the disposable
        // cache tree and preserve it across cold cache rebuilds.
        let relative_runtime_directory = PathBuf::from("gurus")
            .join(&chat.guru_id)
            .join("pi-runtime")
            .join(&chat.id);
        let directory = deletion_root.absolute_path(&relative_directory)?;
        let directory_guard = Some(deletion_root.ensure_private_subdirectory(&relative_directory)?);
        let runtime_directory = deletion_root.absolute_path(&relative_runtime_directory)?;
        let runtime_directory_guard =
            Some(deletion_root.ensure_private_subdirectory(&relative_runtime_directory)?);
        let session = Self {
            pi_session_id: chat.pi_session_id.clone(),
            directory,
            directory_guard,
            runtime_directory,
            runtime_directory_guard,
            deletion_root,
            relative_directory,
        };
        session.validate_current_binding()?;
        Ok(session)
    }

    pub fn session_directory(&self) -> &Path {
        &self.directory
    }

    pub fn pi_session_id(&self) -> &str {
        &self.pi_session_id
    }

    /// A stable, app-private CWD for Pi. Pi binds a persisted JSONL session to
    /// its CWD, so it must survive across warm launches for this Chat thread.
    pub fn runtime_working_directory(&self) -> &Path {
        &self.runtime_directory
    }

    /// Builds the Pi session binding for an effective derived-session ID.
    /// `ChatSession::pi_session_id` names the canonical local Chat identity;
    /// cold cache rebuilds deliberately use a fresh provider-facing ID.
    pub fn pi_config_with_id(&self, session_id: &str) -> PiSessionConfig {
        PiSessionConfig {
            id: session_id.to_owned(),
            directory: self.directory.clone(),
        }
    }

    /// Durably quarantines this Chat's cache after a Pi process group could
    /// not be confirmed stopped. The marker survives a later cold-cache wipe
    /// and is resolved before any future launch is allowed to reuse the cache.
    pub fn record_unconfirmed_pi_stop(&self, process_group_id: i32) -> Result<(), CommandError> {
        #[cfg(unix)]
        {
            self.validate_current_binding()
                .map_err(|_| pi_quarantine_unavailable())?;
            write_unconfirmed_pi_stop(
                self.runtime_directory_guard
                    .as_ref()
                    .ok_or_else(pi_quarantine_unavailable)?,
                process_group_id,
            )
            .map_err(|_| pi_quarantine_unavailable())
        }
        #[cfg(not(unix))]
        {
            let _ = process_group_id;
            Ok(())
        }
    }

    /// Resolves the durable Pi-stop quarantine before a launch or cold cache
    /// rebuild. Only a confirmed process-group exit removes the marker; every
    /// other outcome keeps the cache unavailable.
    pub async fn resolve_unconfirmed_pi_stops(&self) -> Result<(), CommandError> {
        #[cfg(unix)]
        {
            self.resolve_unconfirmed_pi_stops_with_deadline(
                UNCONFIRMED_PI_STOP_CONFIRMATION_DEADLINE,
            )
            .await
        }
        #[cfg(not(unix))]
        {
            Ok(())
        }
    }

    #[cfg(unix)]
    async fn resolve_unconfirmed_pi_stops_with_deadline(
        &self,
        deadline: Duration,
    ) -> Result<(), CommandError> {
        self.validate_current_binding()
            .map_err(|_| pi_quarantine_unavailable())?;
        let guard = self
            .runtime_directory_guard
            .as_ref()
            .ok_or_else(pi_quarantine_unavailable)?;
        let Some(marker) =
            read_unconfirmed_pi_stop(guard).map_err(|_| pi_quarantine_unavailable())?
        else {
            return Ok(());
        };

        match crate::process_lease::confirm_process_group_exit(marker.process_group_id, deadline)
            .await
        {
            crate::process_lease::ProcessGroupTermination::Confirmed => {
                clear_unconfirmed_pi_stop(guard, marker).map_err(|_| pi_quarantine_unavailable())
            }
            crate::process_lease::ProcessGroupTermination::Unconfirmed => {
                Err(pi_quarantine_unavailable())
            }
        }
    }

    pub fn wipe(&mut self) -> Result<(), CommandError> {
        self.validate_current_binding()?;
        self.ensure_no_unconfirmed_pi_stop()?;
        self.directory_guard = None;
        // `remove_tree` treats a missing directory as success. Any other
        // failure must leave this session unpinned and fail closed instead of
        // recreating a directory that may still contain stale Pi JSONL.
        self.deletion_root.remove_tree(&self.relative_directory)?;
        self.directory = self.deletion_root.absolute_path(&self.relative_directory)?;
        self.directory_guard = Some(
            self.deletion_root
                .ensure_private_subdirectory(&self.relative_directory)?,
        );
        self.validate_current_binding()
    }

    pub fn validate_current_binding(&self) -> Result<(), CommandError> {
        validate_pinned_directory(
            "Pi Chat session directory",
            &self.directory,
            self.directory_guard.as_ref(),
        )?;
        validate_pinned_directory(
            "Pi Chat runtime directory",
            &self.runtime_directory,
            self.runtime_directory_guard.as_ref(),
        )
    }

    fn ensure_no_unconfirmed_pi_stop(&self) -> Result<(), CommandError> {
        #[cfg(unix)]
        {
            let guard = self
                .runtime_directory_guard
                .as_ref()
                .ok_or_else(pi_quarantine_unavailable)?;
            if read_unconfirmed_pi_stop(guard)
                .map_err(|_| pi_quarantine_unavailable())?
                .is_some()
            {
                return Err(pi_quarantine_unavailable());
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn pi_quarantine_unavailable() -> CommandError {
    CommandError::new(
        "pi_unavailable",
        "a previous Pi process is still being confirmed stopped",
    )
}

#[cfg(unix)]
fn write_unconfirmed_pi_stop(
    directory: &PrivateDirectoryGuard,
    process_group_id: i32,
) -> Result<(), ()> {
    if process_group_id <= 1 {
        return Err(());
    }
    let descriptor = match openat(
        directory.file(),
        UNCONFIRMED_PI_STOP_MARKER,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(descriptor) => descriptor,
        Err(Errno::EXIST) => {
            let Some(existing) = read_unconfirmed_pi_stop(directory)? else {
                return Err(());
            };
            return if existing.process_group_id == process_group_id {
                Ok(())
            } else {
                Err(())
            };
        }
        Err(_) => return Err(()),
    };

    // If the process crashes mid-write, keep the partial marker. Parsing it
    // will fail closed on the next launch rather than allowing a cache reuse.
    let mut marker = fs::File::from(descriptor);
    marker
        .write_all(format!("{process_group_id}\n").as_bytes())
        .map_err(|_| ())?;
    marker.sync_all().map_err(|_| ())?;
    let metadata = fstat(&marker).map_err(|_| ())?;
    require_private_regular_marker(&metadata)?;
    drop(marker);
    fsync(directory.file()).map_err(|_| ())
}

#[cfg(unix)]
fn read_unconfirmed_pi_stop(
    directory: &PrivateDirectoryGuard,
) -> Result<Option<UnconfirmedPiStop>, ()> {
    let descriptor = match openat(
        directory.file(),
        UNCONFIRMED_PI_STOP_MARKER,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => return Ok(None),
        Err(_) => return Err(()),
    };
    let before = fstat(&descriptor).map_err(|_| ())?;
    require_private_regular_marker(&before)?;
    if before.st_size < 0 || before.st_size as u64 > MAX_UNCONFIRMED_PI_STOP_MARKER_BYTES {
        return Err(());
    }
    let advertised_size = before.st_size as usize;
    let mut marker = fs::File::from(descriptor);
    let mut contents = Vec::with_capacity(advertised_size);
    (&mut marker)
        .take(MAX_UNCONFIRMED_PI_STOP_MARKER_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|_| ())?;
    let after = fstat(&marker).map_err(|_| ())?;
    if contents.len() > MAX_UNCONFIRMED_PI_STOP_MARKER_BYTES as usize
        || contents.len() != advertised_size
        || after.st_size != before.st_size
        || after.st_ino != before.st_ino
        || after.st_dev != before.st_dev
    {
        return Err(());
    }
    let process_group_id = parse_unconfirmed_pi_stop(&contents)?;
    Ok(Some(UnconfirmedPiStop {
        process_group_id,
        device: before.st_dev as u64,
        inode: before.st_ino as u64,
    }))
}

#[cfg(unix)]
fn parse_unconfirmed_pi_stop(contents: &[u8]) -> Result<i32, ()> {
    let value = std::str::from_utf8(contents)
        .map_err(|_| ())?
        .strip_suffix('\n')
        .ok_or(())?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    let process_group_id = value.parse::<i32>().map_err(|_| ())?;
    if process_group_id <= 1 {
        return Err(());
    }
    Ok(process_group_id)
}

#[cfg(unix)]
fn clear_unconfirmed_pi_stop(
    directory: &PrivateDirectoryGuard,
    marker: UnconfirmedPiStop,
) -> Result<(), ()> {
    let current = match read_unconfirmed_pi_stop(directory)? {
        Some(current) => current,
        // We already confirmed the recorded process group exited. If the
        // marker disappeared meanwhile, no stale process can make the cache
        // unsafe, so there is nothing left to clear.
        None => return Ok(()),
    };
    if current.process_group_id != marker.process_group_id
        || current.device != marker.device
        || current.inode != marker.inode
    {
        return Err(());
    }
    unlinkat(
        directory.file(),
        UNCONFIRMED_PI_STOP_MARKER,
        AtFlags::empty(),
    )
    .map_err(|_| ())?;
    fsync(directory.file()).map_err(|_| ())
}

#[cfg(unix)]
fn require_private_regular_marker(metadata: &rustix::fs::Stat) -> Result<(), ()> {
    if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
        || metadata.st_uid != effective_uid()
        || metadata.st_nlink != 1
        || metadata.st_mode & 0o077 != 0
    {
        return Err(());
    }
    Ok(())
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

fn validate_pinned_directory(
    label: &str,
    directory: &Path,
    guard: Option<&PrivateDirectoryGuard>,
) -> Result<(), CommandError> {
    let guard = guard.ok_or_else(|| CommandError::internal(format!("{label} is not pinned")))?;
    let path_metadata = fs::symlink_metadata(directory).map_err(map_io)?;
    if metadata_untrusted(&path_metadata) || !path_metadata.is_dir() {
        return Err(CommandError::internal(format!("{label} binding changed")));
    }
    #[cfg(unix)]
    let guard_metadata = guard.file().metadata().map_err(map_io)?;
    #[cfg(unix)]
    if guard_metadata.dev() != path_metadata.dev() || guard_metadata.ino() != path_metadata.ino() {
        return Err(CommandError::internal(format!("{label} binding changed")));
    }
    #[cfg(windows)]
    if crate::windows_fs::filesystem_identity(guard.file()).map_err(map_io)?
        != crate::windows_fs::filesystem_identity(
            &crate::windows_fs::open_directory_no_reparse(directory).map_err(map_io)?,
        )
        .map_err(map_io)?
    {
        return Err(CommandError::internal(format!("{label} binding changed")));
    }
    Ok(())
}

fn metadata_untrusted(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    if metadata_is_reparse(metadata) {
        return true;
    }
    false
}

fn map_io(error: std::io::Error) -> CommandError {
    CommandError::internal(format!("Pi Chat session boundary failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{ChatSession, MemoryPolicy},
        secure_delete::SecureDeletionRoot,
    };

    fn deletion_root(path: &Path) -> Arc<SecureDeletionRoot> {
        Arc::new(SecureDeletionRoot::open(&path.canonicalize().unwrap()).unwrap())
    }

    fn chat() -> ChatSession {
        ChatSession {
            id: "chat-a".into(),
            guru_id: "guru-a".into(),
            pi_session_id: "123e4567-e89b-42d3-a456-426614174000".into(),
            pi_session_cache: None,
            title: "A".into(),
            memory_policy: MemoryPolicy::default(),
            messages: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn prepare_reuses_the_thread_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let first =
            ChatExecutionSession::prepare(deletion_root(temporary.path()), &chat()).unwrap();
        fs::write(first.session_directory().join("session.jsonl"), b"cached").unwrap();
        let first_path = first.session_directory().to_owned();
        let first_runtime_path = first.runtime_working_directory().to_owned();
        drop(first);

        let second =
            ChatExecutionSession::prepare(deletion_root(temporary.path()), &chat()).unwrap();
        assert_eq!(second.session_directory(), first_path);
        assert_eq!(second.runtime_working_directory(), first_runtime_path);
        assert_eq!(
            fs::read(second.session_directory().join("session.jsonl")).unwrap(),
            b"cached"
        );
        let path = second.session_directory().to_owned();
        drop(second);
        assert!(path.join("session.jsonl").exists());
    }

    #[test]
    fn wipe_removes_derived_session_files_but_preserves_the_stable_runtime_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let mut session =
            ChatExecutionSession::prepare(deletion_root(temporary.path()), &chat()).unwrap();
        fs::write(session.session_directory().join("session.jsonl"), b"stale").unwrap();
        let runtime_directory = session.runtime_working_directory().to_owned();
        #[cfg(unix)]
        let runtime_inode = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(&runtime_directory).unwrap().ino()
        };
        fs::create_dir_all(session.runtime_working_directory().join(".pi")).unwrap();
        fs::write(
            session
                .runtime_working_directory()
                .join(".pi/settings.json"),
            b"trusted settings",
        )
        .unwrap();
        session.wipe().unwrap();
        assert!(!session.session_directory().join("session.jsonl").exists());
        assert!(session.session_directory().is_dir());
        assert!(session.runtime_working_directory().is_dir());
        assert_eq!(session.runtime_working_directory(), runtime_directory);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                fs::metadata(session.runtime_working_directory())
                    .unwrap()
                    .ino(),
                runtime_inode
            );
        }
        assert_eq!(
            fs::read(
                session
                    .runtime_working_directory()
                    .join(".pi/settings.json")
            )
            .unwrap(),
            b"trusted settings"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unconfirmed_stop_quarantine_survives_prepare_and_blocks_cache_wipe() {
        let temporary = tempfile::tempdir().unwrap();
        let session =
            ChatExecutionSession::prepare(deletion_root(temporary.path()), &chat()).unwrap();
        fs::write(session.session_directory().join("session.jsonl"), b"cached").unwrap();
        session.record_unconfirmed_pi_stop(424_242).unwrap();
        let marker = session
            .runtime_working_directory()
            .join(UNCONFIRMED_PI_STOP_MARKER);
        assert!(marker.is_file());
        drop(session);

        let mut resumed =
            ChatExecutionSession::prepare(deletion_root(temporary.path()), &chat()).unwrap();
        let error = resumed.wipe().unwrap_err();
        assert_eq!(error.code, "pi_unavailable");
        assert!(resumed.session_directory().join("session.jsonl").is_file());
        assert!(marker.is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn malformed_stop_quarantine_fails_closed_without_removing_the_marker() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let session =
            ChatExecutionSession::prepare(deletion_root(temporary.path()), &chat()).unwrap();
        let marker = session
            .runtime_working_directory()
            .join(UNCONFIRMED_PI_STOP_MARKER);
        fs::write(&marker, b"not-a-process-group\n").unwrap();
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();

        let error = session.resolve_unconfirmed_pi_stops().await.unwrap_err();
        assert_eq!(error.code, "pi_unavailable");
        assert!(marker.is_file());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn resolved_stop_quarantine_only_clears_after_the_group_exits() {
        use std::{
            os::unix::process::CommandExt,
            process::{Command, Stdio},
            time::Duration,
        };

        let temporary = tempfile::tempdir().unwrap();
        let mut session =
            ChatExecutionSession::prepare(deletion_root(temporary.path()), &chat()).unwrap();
        fs::write(session.session_directory().join("session.jsonl"), b"cached").unwrap();

        let mut command = Command::new("/bin/sleep");
        command
            .arg("5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let mut child = command.spawn().unwrap();
        let process_group_id = child.id() as i32;
        session
            .record_unconfirmed_pi_stop(process_group_id)
            .unwrap();
        let marker = session
            .runtime_working_directory()
            .join(UNCONFIRMED_PI_STOP_MARKER);

        let unresolved = session
            .resolve_unconfirmed_pi_stops_with_deadline(Duration::from_millis(10))
            .await
            .unwrap_err();
        assert_eq!(unresolved.code, "pi_unavailable");
        assert!(marker.is_file());

        crate::process_lease::signal_process_group(process_group_id, libc::SIGKILL).unwrap();
        child.wait().unwrap();
        session
            .resolve_unconfirmed_pi_stops_with_deadline(Duration::from_secs(2))
            .await
            .unwrap();
        assert!(!marker.exists());
        session.wipe().unwrap();
        assert!(!session.session_directory().join("session.jsonl").exists());
    }
}
