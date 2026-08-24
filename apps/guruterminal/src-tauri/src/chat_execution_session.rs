use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::{
    app::CommandError,
    domain::ChatSession,
    pi::PiSessionConfig,
    secure_delete::{PrivateDirectoryGuard, SecureDeletionRoot},
};

#[cfg(windows)]
use crate::windows_fs::metadata_is_reparse;

/// Owns the app-private Pi directory for one Chat thread.
/// SQLite is the transcript authority; this directory holds a digest-bound
/// derived JSONL cache that survives turns until wipe, mismatch, or deletion.
#[derive(Debug)]
pub struct ChatExecutionSession {
    pi_session_id: String,
    directory: PathBuf,
    directory_guard: Option<PrivateDirectoryGuard>,
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
        let directory = deletion_root.absolute_path(&relative_directory)?;
        let directory_guard = Some(deletion_root.ensure_private_subdirectory(&relative_directory)?);
        let session = Self {
            pi_session_id: chat.pi_session_id.clone(),
            directory,
            directory_guard,
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

    pub fn pi_config(&self) -> PiSessionConfig {
        PiSessionConfig {
            id: self.pi_session_id.clone(),
            directory: self.directory.clone(),
        }
    }

    pub fn wipe(&mut self) -> Result<(), CommandError> {
        self.directory_guard = None;
        let _ = self.deletion_root.remove_tree(&self.relative_directory);
        self.directory = self.deletion_root.absolute_path(&self.relative_directory)?;
        self.directory_guard = Some(
            self.deletion_root
                .ensure_private_subdirectory(&self.relative_directory)?,
        );
        self.validate_current_binding()
    }

    pub fn validate_current_binding(&self) -> Result<(), CommandError> {
        let guard = self
            .directory_guard
            .as_ref()
            .ok_or_else(|| CommandError::internal("Pi Chat session directory is not pinned"))?;
        let path_metadata = fs::symlink_metadata(&self.directory).map_err(map_io)?;
        if metadata_untrusted(&path_metadata) || !path_metadata.is_dir() {
            return Err(CommandError::internal(
                "Pi Chat session directory binding changed",
            ));
        }
        let guard_metadata = guard.file().metadata().map_err(map_io)?;
        #[cfg(unix)]
        if guard_metadata.dev() != path_metadata.dev()
            || guard_metadata.ino() != path_metadata.ino()
        {
            return Err(CommandError::internal(
                "Pi Chat session directory binding changed",
            ));
        }
        #[cfg(windows)]
        if crate::windows_fs::filesystem_identity(guard.file()).map_err(map_io)?
            != crate::windows_fs::filesystem_identity(
                &crate::windows_fs::open_directory_no_reparse(&self.directory).map_err(map_io)?,
            )
            .map_err(map_io)?
        {
            return Err(CommandError::internal(
                "Pi Chat session directory binding changed",
            ));
        }
        Ok(())
    }
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
        drop(first);

        let second =
            ChatExecutionSession::prepare(deletion_root(temporary.path()), &chat()).unwrap();
        assert_eq!(second.session_directory(), first_path);
        assert_eq!(
            fs::read(second.session_directory().join("session.jsonl")).unwrap(),
            b"cached"
        );
        let path = second.session_directory().to_owned();
        drop(second);
        assert!(path.join("session.jsonl").exists());
    }

    #[test]
    fn wipe_removes_derived_session_files() {
        let temporary = tempfile::tempdir().unwrap();
        let mut session =
            ChatExecutionSession::prepare(deletion_root(temporary.path()), &chat()).unwrap();
        fs::write(session.session_directory().join("session.jsonl"), b"stale").unwrap();
        session.wipe().unwrap();
        assert!(!session.session_directory().join("session.jsonl").exists());
        assert!(session.session_directory().is_dir());
    }
}
