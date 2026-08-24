#[cfg(unix)]
use rustix::{
    fd::{AsFd, OwnedFd},
    fs::{
        fstat, fsync, mkdirat, open, openat, renameat_with, unlinkat, AtFlags, FileType, Mode,
        OFlags, RenameFlags,
    },
    io::Errno,
};
use serde::{Deserialize, Serialize};
#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};
#[cfg(all(not(unix), not(windows)))]
use uuid::Uuid;

#[cfg(unix)]
use super::ensure_initialized_layout_at;
use super::{ensure_initialized_layout, GuruTerminalRuntime, RuntimeError};
use crate::domain::CanonicalMemoryKind;
pub use crate::hashing::sha256;
#[cfg(unix)]
use crate::pinned_root::PinnedGuruRoot;
#[cfg(windows)]
use crate::windows_fs::{
    ensure_no_reparse_points, metadata_is_reparse, move_file_no_replace, replace_file_with_backup,
};

pub(super) const MAX_MEMORY_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[cfg(all(test, unix))]
thread_local! {
    static FAIL_MEMORY_SYNC_AFTER_PUBLISH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(all(test, unix))]
pub(crate) fn fail_next_memory_sync_after_publish_for_test() {
    FAIL_MEMORY_SYNC_AFTER_PUBLISH.with(|fail| fail.set(true));
}

#[cfg(all(test, unix))]
fn fail_memory_sync_after_publish_for_test() -> Result<(), RuntimeError> {
    if FAIL_MEMORY_SYNC_AFTER_PUBLISH.with(|fail| fail.replace(false)) {
        return Err(RuntimeError::Io(io::Error::other(
            "injected directory sync failure after publication",
        )));
    }
    Ok(())
}

#[cfg(all(not(test), unix))]
fn fail_memory_sync_after_publish_for_test() -> Result<(), RuntimeError> {
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StagedMemoryChange {
    pub guru_id: String,
    pub session_id: String,
    pub relative_path: PathBuf,
    pub before_sha256: Option<String>,
    pub before_markdown: Option<String>,
    pub proposed_sha256: String,
    pub proposed_markdown: String,
    #[serde(default)]
    pub delete: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppliedMemoryChange {
    pub absolute_path: PathBuf,
    pub before_sha256: Option<String>,
    pub after_sha256: String,
}

impl GuruTerminalRuntime {
    /// Applies one Memory change set as a single recovery unit. Every target is
    /// preflighted against its current bytes, the proposed Markdown is published
    /// with temp+rename, and the Memory tree is validated only after the bundle
    /// is present. Any failure rolls already-published targets back in reverse
    /// order before returning.
    pub async fn apply_memory_markdown_set(
        &self,
        workspace: &Path,
        approved: &[StagedMemoryChange],
    ) -> Result<Vec<AppliedMemoryChange>, RuntimeError> {
        validate_approved_change_set(approved)?;
        self.validate(workspace).await?;
        for change in approved {
            preflight_approved_target(workspace, change, false)?;
        }

        let mut applied = Vec::with_capacity(approved.len());
        for change in approved {
            match commit_approved_change(workspace, change) {
                Ok(item) => applied.push(item),
                Err(error) => {
                    rollback_applied_prefix(workspace, approved, applied.len())?;
                    return Err(error);
                }
            }
        }
        if let Err(error) = self.validate(workspace).await {
            rollback_applied_prefix(workspace, approved, applied.len())?;
            self.validate(workspace).await?;
            return Err(error);
        }
        Ok(applied)
    }

    pub async fn rollback_memory_markdown_set(
        &self,
        workspace: &Path,
        approved: &[StagedMemoryChange],
    ) -> Result<(), RuntimeError> {
        validate_approved_change_set(approved)?;
        ensure_initialized_layout(workspace)?;
        for change in approved {
            preflight_approved_target(workspace, change, true)?;
        }
        rollback_applied_prefix(workspace, approved, approved.len())?;
        self.validate(workspace).await
    }

    #[cfg(unix)]
    pub async fn apply_memory_markdown_set_at(
        &self,
        root: &PinnedGuruRoot,
        approved: &[StagedMemoryChange],
    ) -> Result<Vec<AppliedMemoryChange>, RuntimeError> {
        validate_approved_change_set(approved)?;
        self.validate_at(root).await?;
        for change in approved {
            preflight_approved_target_at(root, change, false)?;
        }

        let mut applied = Vec::with_capacity(approved.len());
        for change in approved {
            match commit_approved_change_at(root, change) {
                Ok(item) => applied.push(item),
                Err(error) => {
                    rollback_applied_prefix_at(root, approved, applied.len())?;
                    return Err(error);
                }
            }
        }
        if let Err(error) = self.validate_at(root).await {
            rollback_applied_prefix_at(root, approved, applied.len())?;
            self.validate_at(root).await?;
            return Err(error);
        }
        Ok(applied)
    }

    #[cfg(unix)]
    pub async fn rollback_memory_markdown_set_at(
        &self,
        root: &PinnedGuruRoot,
        approved: &[StagedMemoryChange],
    ) -> Result<(), RuntimeError> {
        validate_approved_change_set(approved)?;
        ensure_initialized_layout_at(root)?;
        for change in approved {
            preflight_approved_target_at(root, change, true)?;
        }
        rollback_applied_prefix_at(root, approved, approved.len())?;
        self.validate_at(root).await
    }

    pub async fn apply_memory_markdown(
        &self,
        workspace: &Path,
        approved: &StagedMemoryChange,
    ) -> Result<AppliedMemoryChange, RuntimeError> {
        self.validate(workspace).await?;
        let target = resolve_memory_target(workspace, &approved.relative_path)?;
        let proposed_hash = validate_approved_content(approved)?;

        #[cfg(unix)]
        let actual_before = {
            let transaction = PinnedMemoryTransaction::open(workspace, approved)?;
            let existing = transaction.read_target()?;
            let actual_before = existing.as_deref().map(sha256);
            if actual_before != approved.before_sha256 {
                return Err(RuntimeError::BeforeHashMismatch);
            }
            if approved.delete {
                transaction.commit_delete(
                    approved
                        .before_sha256
                        .as_deref()
                        .ok_or(RuntimeError::BeforeHashMismatch)?,
                )?;
            } else {
                transaction.write_artifact(approved.proposed_markdown.as_bytes())?;
                transaction.commit_proposed(approved.before_sha256.as_deref(), &proposed_hash)?;
            }
            actual_before
        };

        #[cfg(windows)]
        let actual_before = {
            let transaction = WindowsMemoryTransaction::open(workspace, approved)?;
            let existing = transaction.read_target()?;
            let actual_before = existing.as_deref().map(sha256);
            if actual_before != approved.before_sha256 {
                return Err(RuntimeError::BeforeHashMismatch);
            }
            if approved.delete {
                transaction.commit_delete(
                    approved
                        .before_sha256
                        .as_deref()
                        .ok_or(RuntimeError::BeforeHashMismatch)?,
                )?;
            } else {
                transaction.write_incoming(approved.proposed_markdown.as_bytes())?;
                transaction.commit_proposed(approved.before_sha256.as_deref(), &proposed_hash)?;
            }
            actual_before
        };

        #[cfg(all(not(unix), not(windows)))]
        let actual_before = {
            let existing = match fs::read(&target) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(RuntimeError::Io(error)),
            };
            let actual_before = existing.as_deref().map(sha256);
            if actual_before != approved.before_sha256 {
                return Err(RuntimeError::BeforeHashMismatch);
            }
            let parent = target.parent().ok_or(RuntimeError::InvalidTarget)?;
            if approved.delete {
                fs::remove_file(&target)?;
            } else {
                let transaction = Uuid::new_v4().simple().to_string();
                let staged = parent.join(format!(".guruterminal-{transaction}.staged"));
                write_new_synced(&staged, approved.proposed_markdown.as_bytes())?;
                commit_staged_file(&staged, &target, approved.before_sha256.as_deref())?;
            }
            sync_directory(parent)?;
            actual_before
        };

        if let Err(validation_error) = self.validate(workspace).await {
            self.rollback_memory_markdown(workspace, approved).await?;
            return Err(validation_error);
        }

        Ok(AppliedMemoryChange {
            absolute_path: target,
            before_sha256: actual_before,
            after_sha256: proposed_hash,
        })
    }

    /// Applies, validates, and (when required) rolls back through one pinned
    /// root. `absolute_path` is display-only; all filesystem authority is the
    /// retained descriptor and its `openat` descendants.
    #[cfg(unix)]
    pub async fn apply_memory_markdown_at(
        &self,
        root: &PinnedGuruRoot,
        approved: &StagedMemoryChange,
    ) -> Result<AppliedMemoryChange, RuntimeError> {
        self.validate_at(root).await?;
        validate_memory_relative_path(&approved.relative_path)?;
        let proposed_hash = validate_approved_content(approved)?;
        let transaction = PinnedMemoryTransaction::open_at(root, approved)?;
        let existing = transaction.read_target()?;
        let actual_before = existing.as_deref().map(sha256);
        if actual_before != approved.before_sha256 {
            return Err(RuntimeError::BeforeHashMismatch);
        }
        if approved.delete {
            transaction.commit_delete(
                approved
                    .before_sha256
                    .as_deref()
                    .ok_or(RuntimeError::BeforeHashMismatch)?,
            )?;
        } else {
            transaction.write_artifact(approved.proposed_markdown.as_bytes())?;
            transaction.commit_proposed(approved.before_sha256.as_deref(), &proposed_hash)?;
        }

        if let Err(validation_error) = self.validate_at(root).await {
            self.rollback_memory_markdown_at(root, approved).await?;
            return Err(validation_error);
        }

        Ok(AppliedMemoryChange {
            absolute_path: root.opened_path().join(&approved.relative_path),
            before_sha256: actual_before,
            after_sha256: proposed_hash,
        })
    }

    pub async fn rollback_memory_markdown(
        &self,
        workspace: &Path,
        approved: &StagedMemoryChange,
    ) -> Result<(), RuntimeError> {
        ensure_initialized_layout(workspace)?;
        validate_approved_content(approved)?;

        #[cfg(unix)]
        {
            let transaction = PinnedMemoryTransaction::open(workspace, approved)?;
            if approved.delete {
                transaction.restore_deleted(approved)?;
            } else {
                transaction.rollback(approved)?;
            }
        }

        #[cfg(windows)]
        {
            let transaction = WindowsMemoryTransaction::open(workspace, approved)?;
            if approved.delete {
                transaction.restore_deleted(approved)?;
            } else {
                transaction.rollback(approved)?;
            }
        }

        #[cfg(all(not(unix), not(windows)))]
        {
            let target = resolve_memory_target(workspace, &approved.relative_path)?;
            let parent = target.parent().ok_or(RuntimeError::InvalidTarget)?;
            if approved.delete {
                if target.exists() {
                    return Err(RuntimeError::RollbackConflict);
                }
                let before = approved
                    .before_markdown
                    .as_deref()
                    .ok_or(RuntimeError::RollbackConflict)?;
                write_new_synced(&target, before.as_bytes())?;
            } else {
                let current = match fs::read(&target) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        return Err(RuntimeError::RollbackConflict);
                    }
                    Err(error) => return Err(RuntimeError::Io(error)),
                };
                if sha256(&current) != approved.proposed_sha256 {
                    return Err(RuntimeError::RollbackConflict);
                }
                if let Some(before_markdown) = &approved.before_markdown {
                    let transaction = Uuid::new_v4().simple().to_string();
                    let staged = parent.join(format!(".guruterminal-{transaction}.rollback"));
                    write_new_synced(&staged, before_markdown.as_bytes())?;
                    if !target_has_digest(&target, &approved.proposed_sha256)? {
                        let _ = fs::remove_file(&staged);
                        return Err(RuntimeError::RollbackConflict);
                    }
                    if let Err(error) = fs::rename(&staged, &target) {
                        let _ = fs::remove_file(&staged);
                        return Err(RuntimeError::Io(error));
                    }
                } else {
                    fs::remove_file(&target)?;
                }
            }
            sync_directory(parent)?;
        }
        self.validate(workspace).await
    }

    #[cfg(unix)]
    pub async fn rollback_memory_markdown_at(
        &self,
        root: &PinnedGuruRoot,
        approved: &StagedMemoryChange,
    ) -> Result<(), RuntimeError> {
        ensure_initialized_layout_at(root)?;
        validate_approved_content(approved)?;
        let transaction = PinnedMemoryTransaction::open_at(root, approved)?;
        if approved.delete {
            transaction.restore_deleted(approved)?;
        } else {
            transaction.rollback(approved)?;
        }
        self.validate_at(root).await
    }

    pub fn reconcile_memory_artifact(
        &self,
        workspace: &Path,
        approved: &StagedMemoryChange,
        target_is_proposed: bool,
    ) -> Result<(), RuntimeError> {
        validate_approved_content(approved)?;
        #[cfg(unix)]
        {
            let transaction = PinnedMemoryTransaction::open(workspace, approved)?;
            transaction.reconcile_artifact(approved, target_is_proposed)
        }
        #[cfg(windows)]
        {
            WindowsMemoryTransaction::open(workspace, approved)?
                .reconcile_artifact(approved, target_is_proposed)
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            let _ = (workspace, target_is_proposed);
            Ok(())
        }
    }

    #[cfg(unix)]
    pub fn reconcile_memory_artifact_at(
        &self,
        root: &PinnedGuruRoot,
        approved: &StagedMemoryChange,
        target_is_proposed: bool,
    ) -> Result<(), RuntimeError> {
        validate_approved_content(approved)?;
        PinnedMemoryTransaction::open_at(root, approved)?
            .reconcile_artifact(approved, target_is_proposed)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn stage_memory_artifact_at_for_test(
        &self,
        root: &PinnedGuruRoot,
        approved: &StagedMemoryChange,
        bytes: &[u8],
    ) -> Result<(), RuntimeError> {
        PinnedMemoryTransaction::open_at(root, approved)?.write_artifact(bytes)
    }
}

fn validate_approved_content(approved: &StagedMemoryChange) -> Result<String, RuntimeError> {
    if approved.delete
        && (approved.before_markdown.is_none() || !approved.proposed_markdown.is_empty())
    {
        return Err(RuntimeError::InvalidTarget);
    }
    let proposed_hash = sha256(approved.proposed_markdown.as_bytes());
    if proposed_hash != approved.proposed_sha256 {
        return Err(RuntimeError::ProposedHashMismatch);
    }
    let before_hash = approved
        .before_markdown
        .as_deref()
        .map(|markdown| sha256(markdown.as_bytes()));
    if before_hash != approved.before_sha256 {
        return Err(RuntimeError::BeforeHashMismatch);
    }
    Ok(proposed_hash)
}

fn validate_approved_change_set(approved: &[StagedMemoryChange]) -> Result<(), RuntimeError> {
    if approved.is_empty() || approved.len() > 24 {
        return Err(RuntimeError::InvalidTarget);
    }
    let guru_id = &approved[0].guru_id;
    let session_id = &approved[0].session_id;
    let mut paths = std::collections::BTreeSet::new();
    for change in approved {
        if change.guru_id != *guru_id
            || change.session_id != *session_id
            || !paths.insert(change.relative_path.clone())
        {
            return Err(RuntimeError::InvalidTarget);
        }
        validate_memory_relative_path(&change.relative_path)?;
        validate_approved_content(change)?;
    }
    Ok(())
}

fn preflight_approved_target(
    workspace: &Path,
    approved: &StagedMemoryChange,
    expect_proposed: bool,
) -> Result<(), RuntimeError> {
    let actual = {
        #[cfg(unix)]
        {
            PinnedMemoryTransaction::open(workspace, approved)?.read_target()?
        }
        #[cfg(windows)]
        {
            WindowsMemoryTransaction::open(workspace, approved)?.read_target()?
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            let target = resolve_memory_target(workspace, &approved.relative_path)?;
            match fs::read(target) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(RuntimeError::Io(error)),
            }
        }
    };
    let actual_digest = actual.as_deref().map(sha256);
    let expected = if expect_proposed && approved.delete {
        None
    } else if expect_proposed {
        Some(approved.proposed_sha256.clone())
    } else {
        approved.before_sha256.clone()
    };
    if actual_digest != expected {
        return Err(if expect_proposed {
            RuntimeError::RollbackConflict
        } else {
            RuntimeError::BeforeHashMismatch
        });
    }
    Ok(())
}

fn commit_approved_change(
    workspace: &Path,
    approved: &StagedMemoryChange,
) -> Result<AppliedMemoryChange, RuntimeError> {
    let target = resolve_memory_target(workspace, &approved.relative_path)?;
    let proposed_hash = validate_approved_content(approved)?;
    preflight_approved_target(workspace, approved, false)?;
    #[cfg(unix)]
    {
        let transaction = PinnedMemoryTransaction::open(workspace, approved)?;
        if approved.delete {
            transaction.commit_delete(
                approved
                    .before_sha256
                    .as_deref()
                    .ok_or(RuntimeError::BeforeHashMismatch)?,
            )?;
        } else {
            transaction.write_artifact(approved.proposed_markdown.as_bytes())?;
            transaction.commit_proposed(approved.before_sha256.as_deref(), &proposed_hash)?;
        }
    }
    #[cfg(windows)]
    {
        let transaction = WindowsMemoryTransaction::open(workspace, approved)?;
        if approved.delete {
            transaction.commit_delete(
                approved
                    .before_sha256
                    .as_deref()
                    .ok_or(RuntimeError::BeforeHashMismatch)?,
            )?;
        } else {
            transaction.write_incoming(approved.proposed_markdown.as_bytes())?;
            transaction.commit_proposed(approved.before_sha256.as_deref(), &proposed_hash)?;
        }
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let parent = target.parent().ok_or(RuntimeError::InvalidTarget)?;
        if approved.delete {
            fs::remove_file(&target)?;
        } else {
            let transaction = Uuid::new_v4().simple().to_string();
            let staged = parent.join(format!(".guruterminal-{transaction}.staged"));
            write_new_synced(&staged, approved.proposed_markdown.as_bytes())?;
            commit_staged_file(&staged, &target, approved.before_sha256.as_deref())?;
        }
        sync_directory(parent)?;
    }
    Ok(AppliedMemoryChange {
        absolute_path: target,
        before_sha256: approved.before_sha256.clone(),
        after_sha256: proposed_hash,
    })
}

fn rollback_approved_change(
    workspace: &Path,
    approved: &StagedMemoryChange,
) -> Result<(), RuntimeError> {
    #[cfg(unix)]
    {
        let transaction = PinnedMemoryTransaction::open(workspace, approved)?;
        if approved.delete {
            transaction.restore_deleted(approved)
        } else {
            transaction.rollback(approved)
        }
    }
    #[cfg(windows)]
    {
        let transaction = WindowsMemoryTransaction::open(workspace, approved)?;
        if approved.delete {
            transaction.restore_deleted(approved)
        } else {
            transaction.rollback(approved)
        }
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let target = resolve_memory_target(workspace, &approved.relative_path)?;
        let parent = target.parent().ok_or(RuntimeError::InvalidTarget)?;
        if approved.delete {
            let before = approved
                .before_markdown
                .as_deref()
                .ok_or(RuntimeError::RollbackConflict)?;
            write_new_synced(&target, before.as_bytes())?;
        } else if let Some(before) = &approved.before_markdown {
            let staged = parent.join(format!(
                ".guruterminal-{}.rollback",
                Uuid::new_v4().simple()
            ));
            write_new_synced(&staged, before.as_bytes())?;
            if !target_has_digest(&target, &approved.proposed_sha256)? {
                let _ = fs::remove_file(&staged);
                return Err(RuntimeError::RollbackConflict);
            }
            fs::rename(&staged, &target)?;
        } else if target_has_digest(&target, &approved.proposed_sha256)? {
            fs::remove_file(&target)?;
        } else {
            return Err(RuntimeError::RollbackConflict);
        }
        sync_directory(parent)
    }
}

fn rollback_applied_prefix(
    workspace: &Path,
    approved: &[StagedMemoryChange],
    applied_len: usize,
) -> Result<(), RuntimeError> {
    for change in approved[..applied_len].iter().rev() {
        rollback_approved_change(workspace, change)?;
    }
    Ok(())
}

#[cfg(unix)]
fn preflight_approved_target_at(
    root: &PinnedGuruRoot,
    approved: &StagedMemoryChange,
    expect_proposed: bool,
) -> Result<(), RuntimeError> {
    let actual = PinnedMemoryTransaction::open_at(root, approved)?.read_target()?;
    let actual_digest = actual.as_deref().map(sha256);
    let expected = if expect_proposed && approved.delete {
        None
    } else if expect_proposed {
        Some(approved.proposed_sha256.clone())
    } else {
        approved.before_sha256.clone()
    };
    if actual_digest != expected {
        return Err(if expect_proposed {
            RuntimeError::RollbackConflict
        } else {
            RuntimeError::BeforeHashMismatch
        });
    }
    Ok(())
}

#[cfg(unix)]
fn commit_approved_change_at(
    root: &PinnedGuruRoot,
    approved: &StagedMemoryChange,
) -> Result<AppliedMemoryChange, RuntimeError> {
    validate_memory_relative_path(&approved.relative_path)?;
    let proposed_hash = validate_approved_content(approved)?;
    preflight_approved_target_at(root, approved, false)?;
    let transaction = PinnedMemoryTransaction::open_at(root, approved)?;
    if approved.delete {
        transaction.commit_delete(
            approved
                .before_sha256
                .as_deref()
                .ok_or(RuntimeError::BeforeHashMismatch)?,
        )?;
    } else {
        transaction.write_artifact(approved.proposed_markdown.as_bytes())?;
        transaction.commit_proposed(approved.before_sha256.as_deref(), &proposed_hash)?;
    }
    Ok(AppliedMemoryChange {
        absolute_path: root.opened_path().join(&approved.relative_path),
        before_sha256: approved.before_sha256.clone(),
        after_sha256: proposed_hash,
    })
}

#[cfg(unix)]
fn rollback_applied_prefix_at(
    root: &PinnedGuruRoot,
    approved: &[StagedMemoryChange],
    applied_len: usize,
) -> Result<(), RuntimeError> {
    for change in approved[..applied_len].iter().rev() {
        let transaction = PinnedMemoryTransaction::open_at(root, change)?;
        if change.delete {
            transaction.restore_deleted(change)?;
        } else {
            transaction.rollback(change)?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn target_has_digest(path: &Path, expected: &str) -> Result<bool, RuntimeError> {
    match fs::read(path) {
        Ok(bytes) => Ok(sha256(&bytes) == expected),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(RuntimeError::Io(error)),
    }
}

#[cfg(windows)]
struct WindowsMemoryTransaction {
    target: PathBuf,
    artifact: PathBuf,
    incoming: PathBuf,
    _directory_guards: Vec<File>,
}

#[cfg(windows)]
impl WindowsMemoryTransaction {
    fn open(workspace: &Path, approved: &StagedMemoryChange) -> Result<Self, RuntimeError> {
        let target = resolve_memory_target(workspace, &approved.relative_path)?;
        let internal = workspace.join(".guruterminal");
        ensure_no_reparse_points(&internal).map_err(|_| RuntimeError::InvalidTarget)?;
        let transaction_directory = internal.join("guruterminal-transactions");
        match fs::create_dir(&transaction_directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(RuntimeError::Io(error)),
        }
        ensure_no_reparse_points(&transaction_directory)
            .map_err(|_| RuntimeError::InvalidTarget)?;

        let artifact_key = format!(
            "{}\0{}\0{}",
            approved.guru_id,
            approved.session_id,
            approved.relative_path.to_string_lossy()
        );
        let prefix = format!("memory-{}", &sha256(artifact_key.as_bytes())[..32]);
        let mut directory_guards = vec![
            crate::windows_fs::open_directory_no_reparse(workspace)
                .map_err(|_| RuntimeError::InvalidTarget)?,
            crate::windows_fs::open_directory_no_reparse(&internal)
                .map_err(|_| RuntimeError::InvalidTarget)?,
            crate::windows_fs::open_directory_no_reparse(&transaction_directory)
                .map_err(|_| RuntimeError::InvalidTarget)?,
        ];
        let mut target_directory = workspace.to_path_buf();
        for component in approved
            .relative_path
            .components()
            .take(approved.relative_path.components().count() - 1)
        {
            target_directory.push(component.as_os_str());
            directory_guards.push(
                crate::windows_fs::open_directory_no_reparse(&target_directory)
                    .map_err(|_| RuntimeError::InvalidTarget)?,
            );
        }
        Ok(Self {
            target,
            artifact: transaction_directory.join(format!("{prefix}.swap")),
            incoming: transaction_directory.join(format!("{prefix}.incoming")),
            _directory_guards: directory_guards,
        })
    }

    fn read_target(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        read_regular_windows(&self.target)
    }

    fn read_artifact(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        read_regular_windows(&self.artifact)
    }

    fn read_incoming(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        read_regular_windows(&self.incoming)
    }

    fn write_incoming(&self, bytes: &[u8]) -> Result<(), RuntimeError> {
        if self.read_artifact()?.is_some() || self.read_incoming()?.is_some() {
            return Err(RuntimeError::PendingArtifact);
        }
        write_new_synced(&self.incoming, bytes)
    }

    fn remove_file_if_present(&self, path: &Path) -> Result<(), RuntimeError> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(RuntimeError::Io(error)),
        }
    }

    fn commit_proposed(
        &self,
        expected_before: Option<&str>,
        proposed_digest: &str,
    ) -> Result<(), RuntimeError> {
        match expected_before {
            None => match move_file_no_replace(&self.incoming, &self.target) {
                Ok(()) => Ok(()),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
                    ) =>
                {
                    self.remove_file_if_present(&self.incoming)?;
                    Err(RuntimeError::BeforeHashMismatch)
                }
                Err(error) => {
                    let _ = self.remove_file_if_present(&self.incoming);
                    Err(RuntimeError::Io(error))
                }
            },
            Some(expected_digest) => {
                if let Err(error) =
                    replace_file_with_backup(&self.target, &self.incoming, &self.artifact)
                {
                    let _ = self.remove_file_if_present(&self.incoming);
                    return Err(RuntimeError::Io(error));
                }
                let displaced = self
                    .read_artifact()?
                    .ok_or(RuntimeError::RollbackConflict)?;
                if sha256(&displaced) == expected_digest {
                    return self.remove_file_if_present(&self.artifact);
                }
                if self.try_restore_replace(proposed_digest)? {
                    return Err(RuntimeError::BeforeHashMismatch);
                }
                Err(RuntimeError::RollbackConflict)
            }
        }
    }

    fn commit_delete(&self, expected_digest: &str) -> Result<(), RuntimeError> {
        if self.read_artifact()?.is_some() || self.read_incoming()?.is_some() {
            return Err(RuntimeError::PendingArtifact);
        }
        match move_file_no_replace(&self.target, &self.artifact) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(RuntimeError::BeforeHashMismatch)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(RuntimeError::PendingArtifact)
            }
            Err(error) => return Err(RuntimeError::Io(error)),
        }
        let displaced = self
            .read_artifact()?
            .ok_or(RuntimeError::RollbackConflict)?;
        if sha256(&displaced) == expected_digest {
            return self.remove_file_if_present(&self.artifact);
        }
        if self.read_target()?.is_none() {
            move_file_no_replace(&self.artifact, &self.target).map_err(RuntimeError::Io)?;
        }
        Err(RuntimeError::BeforeHashMismatch)
    }

    fn restore_deleted(&self, approved: &StagedMemoryChange) -> Result<(), RuntimeError> {
        if self.read_target()?.is_some()
            || self.read_artifact()?.is_some()
            || self.read_incoming()?.is_some()
        {
            return Err(RuntimeError::RollbackConflict);
        }
        let before = approved
            .before_markdown
            .as_deref()
            .ok_or(RuntimeError::RollbackConflict)?;
        self.write_incoming(before.as_bytes())?;
        match move_file_no_replace(&self.incoming, &self.target) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = self.remove_file_if_present(&self.incoming);
                Err(RuntimeError::Io(error))
            }
        }
    }

    fn rollback(&self, approved: &StagedMemoryChange) -> Result<(), RuntimeError> {
        if self.read_artifact()?.is_some() || self.read_incoming()?.is_some() {
            return Err(RuntimeError::PendingArtifact);
        }
        match &approved.before_markdown {
            Some(before) => {
                self.write_incoming(before.as_bytes())?;
                replace_file_with_backup(&self.target, &self.incoming, &self.artifact)
                    .map_err(RuntimeError::Io)?;
                let displaced = self
                    .read_artifact()?
                    .ok_or(RuntimeError::RollbackConflict)?;
                if sha256(&displaced) == approved.proposed_sha256 {
                    return self.remove_file_if_present(&self.artifact);
                }
                let before_digest = approved
                    .before_sha256
                    .as_deref()
                    .ok_or(RuntimeError::RollbackConflict)?;
                self.try_restore_replace(before_digest)?;
                Err(RuntimeError::RollbackConflict)
            }
            None => {
                match move_file_no_replace(&self.target, &self.artifact) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        return Err(RuntimeError::RollbackConflict)
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        return Err(RuntimeError::PendingArtifact)
                    }
                    Err(error) => return Err(RuntimeError::Io(error)),
                }
                let displaced = self
                    .read_artifact()?
                    .ok_or(RuntimeError::RollbackConflict)?;
                if sha256(&displaced) == approved.proposed_sha256 {
                    return self.remove_file_if_present(&self.artifact);
                }
                if self.read_target()?.is_none() {
                    move_file_no_replace(&self.artifact, &self.target).map_err(RuntimeError::Io)?;
                }
                Err(RuntimeError::RollbackConflict)
            }
        }
    }

    fn reconcile_artifact(
        &self,
        approved: &StagedMemoryChange,
        target_is_proposed: bool,
    ) -> Result<(), RuntimeError> {
        let artifact = self.read_artifact()?;
        let incoming = self.read_incoming()?;
        validate_windows_recovery_artifacts(
            approved,
            target_is_proposed,
            artifact.as_deref(),
            incoming.as_deref(),
        )?;

        // The swap contains the displaced canonical bytes and is the primary
        // recovery journal. Remove an optional validated incoming file first,
        // so a cleanup interruption preserves the stronger artifact.
        if incoming.is_some() {
            self.remove_file_if_present(&self.incoming)?;
        }
        if artifact.is_some() {
            self.remove_file_if_present(&self.artifact)?;
        }
        Ok(())
    }

    fn try_restore_replace(&self, published_digest: &str) -> Result<bool, RuntimeError> {
        if !self
            .read_target()?
            .is_some_and(|bytes| sha256(&bytes) == published_digest)
        {
            return Ok(false);
        }
        replace_file_with_backup(&self.target, &self.artifact, &self.incoming)
            .map_err(RuntimeError::Io)?;
        let displaced = self
            .read_incoming()?
            .ok_or(RuntimeError::RollbackConflict)?;
        if sha256(&displaced) != published_digest {
            return Err(RuntimeError::RollbackConflict);
        }
        self.remove_file_if_present(&self.incoming)?;
        Ok(true)
    }
}

#[cfg(any(windows, test))]
fn validate_windows_recovery_artifacts(
    approved: &StagedMemoryChange,
    target_is_proposed: bool,
    artifact: Option<&[u8]>,
    incoming: Option<&[u8]>,
) -> Result<(), RuntimeError> {
    if let Some(artifact) = artifact {
        let expected_digest = if target_is_proposed {
            approved
                .before_sha256
                .as_deref()
                .ok_or(RuntimeError::RollbackConflict)?
        } else {
            approved.proposed_sha256.as_str()
        };
        if sha256(artifact) != expected_digest {
            return Err(RuntimeError::RollbackConflict);
        }
    }
    if let Some(incoming) = incoming {
        let digest = sha256(incoming);
        if digest != approved.proposed_sha256
            && approved.before_sha256.as_deref() != Some(digest.as_str())
        {
            return Err(RuntimeError::RollbackConflict);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn read_regular_windows(path: &Path) -> Result<Option<Vec<u8>>, RuntimeError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RuntimeError::Io(error)),
    };
    if metadata.file_type().is_symlink()
        || metadata_is_reparse(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_MEMORY_FILE_BYTES
    {
        return Err(RuntimeError::InvalidTarget);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    crate::windows_fs::add_open_reparse_point_flag(&mut options);
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || metadata_is_reparse(&opened) {
        return Err(RuntimeError::InvalidTarget);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_MEMORY_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MEMORY_FILE_BYTES {
        return Err(RuntimeError::InvalidTarget);
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
struct PinnedMemoryTransaction {
    target_parent: OwnedFd,
    transaction_directory: OwnedFd,
    target_name: OsString,
    artifact_name: OsString,
}

#[cfg(unix)]
impl PinnedMemoryTransaction {
    fn open(workspace: &Path, approved: &StagedMemoryChange) -> Result<Self, RuntimeError> {
        validate_memory_relative_path(&approved.relative_path)?;
        let directory_flags =
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let root_path = workspace.canonicalize()?;
        let root = open(&root_path, directory_flags, Mode::empty()).map_err(rustix_io)?;
        Self::open_from_root(&root, approved)
    }

    fn open_at(root: &PinnedGuruRoot, approved: &StagedMemoryChange) -> Result<Self, RuntimeError> {
        validate_memory_relative_path(&approved.relative_path)?;
        Self::open_from_root(root, approved)
    }

    fn open_from_root<Fd: AsFd>(
        root: Fd,
        approved: &StagedMemoryChange,
    ) -> Result<Self, RuntimeError> {
        let components = approved
            .relative_path
            .components()
            .map(|component| component.as_os_str().to_os_string())
            .collect::<Vec<_>>();
        let target_name = components
            .last()
            .cloned()
            .ok_or(RuntimeError::InvalidTarget)?;
        let directory_flags =
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;

        let internal =
            openat(&root, ".guruterminal", directory_flags, Mode::empty()).map_err(rustix_io)?;
        match mkdirat(&internal, "guruterminal-transactions", Mode::RWXU) {
            Ok(()) | Err(Errno::EXIST) => {}
            Err(error) => return Err(rustix_io(error)),
        }
        let transaction_directory = openat(
            &internal,
            "guruterminal-transactions",
            directory_flags,
            Mode::empty(),
        )
        .map_err(rustix_io)?;

        let mut target_parent =
            openat(&root, "guruterminal", directory_flags, Mode::empty()).map_err(rustix_io)?;
        for component in components.iter().skip(1).take(components.len() - 2) {
            target_parent = openat(
                &target_parent,
                component.as_os_str(),
                directory_flags,
                Mode::empty(),
            )
            .map_err(rustix_io)?;
        }

        let artifact_key = format!(
            "{}\0{}\0{}",
            approved.guru_id,
            approved.session_id,
            approved.relative_path.to_string_lossy()
        );
        let artifact_name = OsString::from(format!(
            "memory-{}.swap",
            &sha256(artifact_key.as_bytes())[..32]
        ));
        Ok(Self {
            target_parent,
            transaction_directory,
            target_name,
            artifact_name,
        })
    }

    fn read_target(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        read_regular_at(&self.target_parent, &self.target_name)
    }

    fn read_artifact(&self) -> Result<Option<Vec<u8>>, RuntimeError> {
        read_regular_at(&self.transaction_directory, &self.artifact_name)
    }

    fn write_artifact(&self, bytes: &[u8]) -> Result<(), RuntimeError> {
        let descriptor = match openat(
            &self.transaction_directory,
            &self.artifact_name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(descriptor) => descriptor,
            Err(Errno::EXIST) => return Err(RuntimeError::PendingArtifact),
            Err(error) => return Err(rustix_io(error)),
        };
        let mut file = File::from(descriptor);
        file.write_all(bytes)?;
        file.sync_all()?;
        fsync(&self.transaction_directory).map_err(rustix_io)
    }

    fn remove_artifact(&self) -> Result<(), RuntimeError> {
        match unlinkat(
            &self.transaction_directory,
            &self.artifact_name,
            AtFlags::empty(),
        ) {
            Ok(()) => fsync(&self.transaction_directory).map_err(rustix_io),
            Err(Errno::NOENT) => Ok(()),
            Err(error) => Err(rustix_io(error)),
        }
    }

    fn exchange(&self) -> Result<(), RuntimeError> {
        renameat_with(
            &self.transaction_directory,
            &self.artifact_name,
            &self.target_parent,
            &self.target_name,
            RenameFlags::EXCHANGE,
        )
        .map_err(rustix_io)?;
        self.sync_directories()
    }

    fn commit_proposed(
        &self,
        expected_before: Option<&str>,
        proposed_digest: &str,
    ) -> Result<(), RuntimeError> {
        match expected_before {
            None => {
                match renameat_with(
                    &self.transaction_directory,
                    &self.artifact_name,
                    &self.target_parent,
                    &self.target_name,
                    RenameFlags::NOREPLACE,
                ) {
                    Ok(()) => self.sync_directories(),
                    Err(Errno::EXIST) | Err(Errno::NOENT) => {
                        self.remove_artifact()?;
                        Err(RuntimeError::BeforeHashMismatch)
                    }
                    Err(error) => {
                        let _ = self.remove_artifact();
                        Err(rustix_io(error))
                    }
                }
            }
            Some(expected_digest) => {
                if let Err(error) = self.exchange() {
                    let _ = self.remove_artifact();
                    return Err(error);
                }
                let displaced = self
                    .read_artifact()?
                    .ok_or(RuntimeError::RollbackConflict)?;
                if sha256(&displaced) == expected_digest {
                    return self.remove_artifact();
                }

                // Restore only if nobody edited the newly published target.
                if self.try_restore_exchange(proposed_digest)? {
                    return Err(RuntimeError::BeforeHashMismatch);
                }
                Err(RuntimeError::RollbackConflict)
            }
        }
    }

    fn commit_delete(&self, expected_digest: &str) -> Result<(), RuntimeError> {
        if self.read_artifact()?.is_some() {
            return Err(RuntimeError::PendingArtifact);
        }
        match renameat_with(
            &self.target_parent,
            &self.target_name,
            &self.transaction_directory,
            &self.artifact_name,
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => self.sync_directories()?,
            Err(Errno::NOENT) => return Err(RuntimeError::BeforeHashMismatch),
            Err(Errno::EXIST) => return Err(RuntimeError::PendingArtifact),
            Err(error) => return Err(rustix_io(error)),
        }
        let displaced = self
            .read_artifact()?
            .ok_or(RuntimeError::RollbackConflict)?;
        if sha256(&displaced) == expected_digest {
            return self.remove_artifact();
        }
        if self.read_target()?.is_none() {
            renameat_with(
                &self.transaction_directory,
                &self.artifact_name,
                &self.target_parent,
                &self.target_name,
                RenameFlags::NOREPLACE,
            )
            .map_err(rustix_io)?;
            self.sync_directories()?;
        }
        Err(RuntimeError::BeforeHashMismatch)
    }

    fn restore_deleted(&self, approved: &StagedMemoryChange) -> Result<(), RuntimeError> {
        if self.read_target()?.is_some() || self.read_artifact()?.is_some() {
            return Err(RuntimeError::RollbackConflict);
        }
        let before = approved
            .before_markdown
            .as_deref()
            .ok_or(RuntimeError::RollbackConflict)?;
        self.write_artifact(before.as_bytes())?;
        match renameat_with(
            &self.transaction_directory,
            &self.artifact_name,
            &self.target_parent,
            &self.target_name,
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => self.sync_directories(),
            Err(error) => {
                let _ = self.remove_artifact();
                Err(rustix_io(error))
            }
        }
    }

    fn rollback(&self, approved: &StagedMemoryChange) -> Result<(), RuntimeError> {
        if self.read_artifact()?.is_some() {
            return Err(RuntimeError::PendingArtifact);
        }
        match &approved.before_markdown {
            Some(before) => {
                self.write_artifact(before.as_bytes())?;
                self.exchange()?;
                let displaced = self
                    .read_artifact()?
                    .ok_or(RuntimeError::RollbackConflict)?;
                if sha256(&displaced) == approved.proposed_sha256 {
                    return self.remove_artifact();
                }
                let before_digest = approved
                    .before_sha256
                    .as_deref()
                    .ok_or(RuntimeError::RollbackConflict)?;
                self.try_restore_exchange(before_digest)?;
                Err(RuntimeError::RollbackConflict)
            }
            None => {
                match renameat_with(
                    &self.target_parent,
                    &self.target_name,
                    &self.transaction_directory,
                    &self.artifact_name,
                    RenameFlags::NOREPLACE,
                ) {
                    Ok(()) => self.sync_directories()?,
                    Err(Errno::NOENT) => return Err(RuntimeError::RollbackConflict),
                    Err(Errno::EXIST) => return Err(RuntimeError::PendingArtifact),
                    Err(error) => return Err(rustix_io(error)),
                }
                let displaced = self
                    .read_artifact()?
                    .ok_or(RuntimeError::RollbackConflict)?;
                if sha256(&displaced) == approved.proposed_sha256 {
                    return self.remove_artifact();
                }
                if self.read_target()?.is_none() {
                    renameat_with(
                        &self.transaction_directory,
                        &self.artifact_name,
                        &self.target_parent,
                        &self.target_name,
                        RenameFlags::NOREPLACE,
                    )
                    .map_err(rustix_io)?;
                    self.sync_directories()?;
                }
                Err(RuntimeError::RollbackConflict)
            }
        }
    }

    fn reconcile_artifact(
        &self,
        approved: &StagedMemoryChange,
        target_is_proposed: bool,
    ) -> Result<(), RuntimeError> {
        let Some(artifact) = self.read_artifact()? else {
            return Ok(());
        };
        let expected = if target_is_proposed {
            approved
                .before_markdown
                .as_deref()
                .ok_or(RuntimeError::RollbackConflict)?
                .as_bytes()
        } else {
            approved.proposed_markdown.as_bytes()
        };
        if sha256(&artifact) != sha256(expected) {
            return Err(RuntimeError::RollbackConflict);
        }
        self.remove_artifact()
    }

    fn try_restore_exchange(&self, published_digest: &str) -> Result<bool, RuntimeError> {
        if !self
            .read_target()?
            .is_some_and(|bytes| sha256(&bytes) == published_digest)
        {
            return Ok(false);
        }
        self.exchange()?;
        let restored_artifact = self
            .read_artifact()?
            .ok_or(RuntimeError::RollbackConflict)?;
        if sha256(&restored_artifact) != published_digest {
            // A writer won between the target check and EXCHANGE. Its bytes are
            // retained in the transaction directory for explicit recovery.
            return Err(RuntimeError::RollbackConflict);
        }
        self.remove_artifact()?;
        Ok(true)
    }

    fn sync_directories(&self) -> Result<(), RuntimeError> {
        fail_memory_sync_after_publish_for_test()?;
        fsync(&self.target_parent).map_err(rustix_io)?;
        fsync(&self.transaction_directory).map_err(rustix_io)
    }
}

#[cfg(unix)]
fn read_regular_at(directory: &OwnedFd, name: &OsStr) -> Result<Option<Vec<u8>>, RuntimeError> {
    let descriptor = match openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => return Ok(None),
        Err(error) => return Err(rustix_io(error)),
    };
    let metadata = fstat(&descriptor).map_err(rustix_io)?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
        || metadata.st_size as u64 > MAX_MEMORY_FILE_BYTES
    {
        return Err(RuntimeError::InvalidTarget);
    }
    let mut bytes = Vec::with_capacity(metadata.st_size as usize);
    File::from(descriptor)
        .take(MAX_MEMORY_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MEMORY_FILE_BYTES {
        return Err(RuntimeError::InvalidTarget);
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
fn rustix_io(error: Errno) -> RuntimeError {
    RuntimeError::Io(io::Error::from(error))
}

#[cfg(any(test, not(unix)))]
fn commit_staged_file(
    staged: &Path,
    target: &Path,
    expected_before: Option<&str>,
) -> Result<(), RuntimeError> {
    match expected_before {
        None => {
            // A hard link is an atomic no-clobber publication because the staged
            // file lives in the same directory/filesystem as the target.
            if let Err(error) = fs::hard_link(staged, target) {
                let _ = fs::remove_file(staged);
                return if error.kind() == io::ErrorKind::AlreadyExists {
                    Err(RuntimeError::BeforeHashMismatch)
                } else {
                    Err(RuntimeError::Io(error))
                };
            }
            fs::remove_file(staged)?;
            Ok(())
        }
        Some(expected) => {
            // Re-read after staging so an ordinary external edit during the
            // validation/write window cannot be silently overwritten.
            let metadata = fs::symlink_metadata(target).map_err(RuntimeError::Io)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                let _ = fs::remove_file(staged);
                return Err(RuntimeError::InvalidTarget);
            }
            let current = fs::read(target).map_err(RuntimeError::Io)?;
            if sha256(&current) != expected {
                let _ = fs::remove_file(staged);
                return Err(RuntimeError::BeforeHashMismatch);
            }
            if let Err(error) = fs::rename(staged, target) {
                let _ = fs::remove_file(staged);
                return Err(RuntimeError::Io(error));
            }
            Ok(())
        }
    }
}

fn resolve_memory_target(workspace: &Path, relative: &Path) -> Result<PathBuf, RuntimeError> {
    validate_memory_relative_path(relative)?;
    let mut components = relative.components();
    let _ = components.next();
    let _ = components.next();

    #[cfg(windows)]
    let root = {
        ensure_no_reparse_points(workspace).map_err(|_| RuntimeError::InvalidTarget)?;
        workspace.to_path_buf()
    };
    #[cfg(not(windows))]
    let root = workspace.canonicalize()?;
    let target = root.join(relative);
    let parent = target.parent().ok_or(RuntimeError::InvalidTarget)?;
    #[cfg(windows)]
    ensure_no_reparse_points(parent).map_err(|_| RuntimeError::InvalidTarget)?;
    #[cfg(not(windows))]
    let canonical_parent = parent.canonicalize()?;
    #[cfg(not(windows))]
    if !canonical_parent.starts_with(&root) {
        return Err(RuntimeError::InvalidTarget);
    }
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || cfg!(windows) && windows_metadata_is_reparse(&metadata)
        {
            return Err(RuntimeError::InvalidTarget);
        }
    }
    Ok(target)
}

fn validate_memory_relative_path(relative: &Path) -> Result<(), RuntimeError> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || relative.extension().and_then(|value| value.to_str()) != Some("md")
    {
        return Err(RuntimeError::InvalidTarget);
    }
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or(RuntimeError::InvalidTarget)?;
    if components.len() < 3
        || components[0] != "guruterminal"
        || CanonicalMemoryKind::from_slug(components[1]).is_none()
    {
        return Err(RuntimeError::InvalidTarget);
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
fn sync_directory(path: &Path) -> Result<(), RuntimeError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(windows)]
fn windows_metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    metadata_is_reparse(metadata)
}

#[cfg(not(windows))]
fn windows_metadata_is_reparse(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests;
