use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs as filesystem;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Repository, RepositoryInitOptions, Signature, Transaction};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const GIT_NAME: &str = "Guru Terminal";
const GIT_EMAIL: &str = "memory@guruterminal.local";
const MEMORY_PREFIX: &str = "guruterminal";

#[derive(Debug, Error)]
pub enum MemoryGitError {
    #[error("memory git failed: {0}")]
    Git(#[from] git2::Error),
    #[error("memory git I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("memory git path is invalid")]
    InvalidPath,
    #[error("memory git recovery is required after {operation}: {recovery}")]
    RecoveryRequired { operation: String, recovery: String },
}

impl MemoryGitError {
    pub fn recovery_required(&self) -> bool {
        matches!(self, Self::RecoveryRequired { .. })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryGitSnapshot {
    pub previous_head: Option<String>,
    pub symbolic_head: Option<String>,
    pub original_index_tree: String,
    pub published_index_tree: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PreparedMemoryCommit {
    pub commit_id: String,
    pub index_tree_id: String,
    snapshot: MemoryGitSnapshot,
    created_commit: bool,
}

#[derive(Clone, Debug)]
pub struct MemoryGitChange {
    pub relative_path: PathBuf,
    pub contents: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct MemoryCommit {
    pub commit_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviousMemoryVersion {
    pub markdown: String,
    pub commit_id: String,
    pub message: String,
}

pub fn ensure_repository(workspace: &Path) -> Result<Repository, MemoryGitError> {
    match Repository::open(workspace) {
        Ok(repo) => Ok(repo),
        Err(_) => {
            let mut options = RepositoryInitOptions::new();
            options.no_reinit(true);
            Ok(Repository::init_opts(workspace, &options)?)
        }
    }
}

pub fn commit_memory(workspace: &Path, message: &str) -> Result<String, MemoryGitError> {
    commit_memory_transaction(workspace, message).map(|commit| commit.commit_id)
}

pub fn commit_memory_transaction(
    workspace: &Path,
    message: &str,
) -> Result<MemoryCommit, MemoryGitError> {
    let mut snapshot = begin_memory_transaction(workspace)?;
    let prepared = prepare_memory_commit(workspace, message, &snapshot)?;
    snapshot.published_index_tree = Some(prepared.index_tree_id.clone());
    let expected_commit_id = prepared.commit_id.clone();
    match finalize_memory_commit(workspace, prepared) {
        Ok(commit) => Ok(commit),
        Err(finalize_error) => {
            if let Err(rollback_error) =
                rollback_memory_snapshot(workspace, &snapshot, Some(&expected_commit_id))
            {
                return Err(MemoryGitError::RecoveryRequired {
                    operation: finalize_error.to_string(),
                    recovery: rollback_error.to_string(),
                });
            }
            Err(finalize_error)
        }
    }
}

pub fn begin_memory_transaction(workspace: &Path) -> Result<MemoryGitSnapshot, MemoryGitError> {
    let repo = ensure_repository(workspace)?;
    let mut index = repo.index()?;
    let original_index_tree = index.write_tree()?.to_string();
    let symbolic_head = repo
        .find_reference("HEAD")?
        .symbolic_target()
        .map(str::to_owned);
    let previous_head = repo
        .head()
        .ok()
        .and_then(|head| head.peel_to_commit().ok())
        .map(|commit| commit.id().to_string());
    Ok(MemoryGitSnapshot {
        previous_head,
        symbolic_head,
        original_index_tree,
        published_index_tree: None,
    })
}

pub fn prepare_memory_commit(
    workspace: &Path,
    message: &str,
    snapshot: &MemoryGitSnapshot,
) -> Result<PreparedMemoryCommit, MemoryGitError> {
    prepare_memory_commit_inner(workspace, message, snapshot, None)
}

pub fn prepare_memory_commit_exact(
    workspace: &Path,
    message: &str,
    snapshot: &MemoryGitSnapshot,
    changes: &[MemoryGitChange],
) -> Result<PreparedMemoryCommit, MemoryGitError> {
    prepare_memory_commit_inner(workspace, message, snapshot, Some(changes))
}

fn prepare_memory_commit_inner(
    workspace: &Path,
    message: &str,
    snapshot: &MemoryGitSnapshot,
    changes: Option<&[MemoryGitChange]>,
) -> Result<PreparedMemoryCommit, MemoryGitError> {
    let repo = ensure_repository(workspace)?;
    ensure_snapshot_is_current(&repo, snapshot)?;
    let original_index_tree_id = git2::Oid::from_str(&snapshot.original_index_tree)?;
    let committed: Result<(git2::Oid, git2::Oid, bool), MemoryGitError> = (|| {
        let signature = Signature::now(GIT_NAME, GIT_EMAIL)?;
        let parent = snapshot
            .previous_head
            .as_deref()
            .map(git2::Oid::from_str)
            .transpose()?
            .map(|oid| repo.find_commit(oid))
            .transpose()?;
        let parent_tree_id = match &parent {
            Some(parent) => parent.tree_id(),
            None => repo.treebuilder(None)?.write()?,
        };
        // The commit is based on HEAD and receives only exact Memory-path
        // changes. The separately prepared index is based on the user's
        // original index, preserving unrelated staged entries without
        // smuggling them into Guru's commit.
        let commit_tree_id = build_memory_tree(workspace, parent_tree_id, changes)?;
        let index_tree_id = build_memory_tree(workspace, original_index_tree_id, changes)?;
        let tree = repo.find_tree(commit_tree_id)?;
        if let Some(parent) = &parent {
            if parent.tree_id() == commit_tree_id {
                return Ok((parent.id(), index_tree_id, false));
            }
        }
        let parents = parent.as_ref().into_iter().collect::<Vec<_>>();
        let message = if message.trim().is_empty() {
            "update memory"
        } else {
            message.trim()
        };
        // Create the immutable commit object without moving HEAD. The caller
        // durably records this OID before `finalize_memory_commit` publishes
        // the ref, closing the process-crash gap between Git and SQLite.
        let oid = repo.commit(None, &signature, &signature, message, &tree, &parents)?;
        Ok((oid, index_tree_id, true))
    })();
    let (commit_id, index_tree_id, created_commit) = committed?;
    let mut prepared_snapshot = snapshot.clone();
    prepared_snapshot.published_index_tree = Some(index_tree_id.to_string());
    Ok(PreparedMemoryCommit {
        commit_id: commit_id.to_string(),
        index_tree_id: index_tree_id.to_string(),
        snapshot: prepared_snapshot,
        created_commit,
    })
}

pub fn finalize_memory_commit(
    workspace: &Path,
    prepared: PreparedMemoryCommit,
) -> Result<MemoryCommit, MemoryGitError> {
    let repo = Repository::open(workspace)?;
    let mut references =
        lock_snapshot_references(&repo, &prepared.snapshot, "finalizing canonical Memory")?;
    ensure_snapshot_head_is_current(&repo, &prepared.snapshot)?;
    if prepared.created_commit {
        let commit_id = git2::Oid::from_str(&prepared.commit_id)?;
        let original_index_tree = git2::Oid::from_str(&prepared.snapshot.original_index_tree)?;
        let published_index_tree = git2::Oid::from_str(&prepared.index_tree_id)?;
        replace_index_tree_if_current(
            &repo,
            &[original_index_tree],
            published_index_tree,
            "publishing the canonical Memory index",
        )?;
        fail_after_index_write_for_test()?;
        match prepared.snapshot.symbolic_head.as_deref() {
            Some(reference_name) => references.set_target(
                reference_name,
                commit_id,
                None,
                "finalize canonical Memory transaction",
            )?,
            None => references.set_target(
                "HEAD",
                commit_id,
                None,
                "finalize detached canonical Memory transaction",
            )?,
        }
    }
    references
        .commit()
        .map_err(|error| MemoryGitError::RecoveryRequired {
            operation: "finalizing canonical Memory".into(),
            recovery: format!("the locked Memory reference update failed: {error}"),
        })?;
    Ok(MemoryCommit {
        commit_id: prepared.commit_id,
    })
}

pub fn rollback_memory_snapshot(
    workspace: &Path,
    snapshot: &MemoryGitSnapshot,
    expected_commit_id: Option<&str>,
) -> Result<(), MemoryGitError> {
    let repo = Repository::open(workspace)?;
    let mut references =
        lock_snapshot_references(&repo, snapshot, "rolling back a finalized Chat")?;
    let previous = snapshot
        .previous_head
        .as_deref()
        .map(git2::Oid::from_str)
        .transpose()?;
    let current = current_head(&repo);
    ensure_snapshot_head_shape_is_current(&repo, snapshot, "rolling back a finalized Chat")?;
    let expected = expected_commit_id.map(git2::Oid::from_str).transpose()?;
    if current != previous {
        if current != expected || expected.is_none() {
            return Err(MemoryGitError::RecoveryRequired {
                operation: "rolling back a finalized Chat".into(),
                recovery: "Memory HEAD no longer names the commit being rolled back".into(),
            });
        }
        let reset_result = match (previous, snapshot.symbolic_head.as_deref()) {
            (Some(previous), Some(reference_name)) => references.set_target(
                reference_name,
                previous,
                None,
                "rollback failed Chat finalization",
            ),
            (Some(previous), None) => references.set_target(
                "HEAD",
                previous,
                None,
                "rollback detached failed Chat finalization",
            ),
            (None, Some(reference_name)) => references.remove(reference_name),
            (None, None) => Err(git2::Error::from_str(
                "cannot restore an unborn detached Memory HEAD",
            )),
        };
        if let Err(reset_error) = reset_result {
            return Err(MemoryGitError::RecoveryRequired {
                operation: "rolling back a finalized Chat".into(),
                recovery: reset_error.to_string(),
            });
        }
    }
    let original_index_tree = git2::Oid::from_str(&snapshot.original_index_tree)?;
    let mut allowed_index_trees = vec![original_index_tree];
    if let Some(published) = snapshot
        .published_index_tree
        .as_deref()
        .map(git2::Oid::from_str)
        .transpose()?
    {
        if published != original_index_tree {
            allowed_index_trees.push(published);
        }
    }
    fail_index_restore_for_test()?;
    replace_index_tree_if_current(
        &repo,
        &allowed_index_trees,
        original_index_tree,
        "restoring the pre-Chat Memory index",
    )?;
    references
        .commit()
        .map_err(|error| MemoryGitError::RecoveryRequired {
            operation: "rolling back a finalized Chat".into(),
            recovery: format!("the locked Memory reference rollback failed: {error}"),
        })?;
    Ok(())
}

pub fn verify_finalized_memory_commit(
    workspace: &Path,
    expected_commit_id: &str,
) -> Result<(), MemoryGitError> {
    let repo = Repository::open(workspace)?;
    let expected = git2::Oid::from_str(expected_commit_id)?;
    if current_head(&repo) != Some(expected) {
        return Err(MemoryGitError::RecoveryRequired {
            operation: "verifying a finalized Chat".into(),
            recovery: "Memory HEAD does not name the journaled commit".into(),
        });
    }
    Ok(())
}

fn current_head(repo: &Repository) -> Option<git2::Oid> {
    repo.head()
        .ok()
        .and_then(|head| head.peel_to_commit().ok())
        .map(|head| head.id())
}

fn ensure_snapshot_head_is_current(
    repo: &Repository,
    snapshot: &MemoryGitSnapshot,
) -> Result<(), MemoryGitError> {
    ensure_snapshot_head_shape_is_current(repo, snapshot, "finalizing canonical Memory")?;
    let previous = snapshot
        .previous_head
        .as_deref()
        .map(git2::Oid::from_str)
        .transpose()?;
    if current_head(repo) != previous {
        return Err(MemoryGitError::RecoveryRequired {
            operation: "finalizing canonical Memory".into(),
            recovery: "Memory HEAD changed after the transaction was prepared".into(),
        });
    }
    Ok(())
}

fn ensure_snapshot_head_shape_is_current(
    repo: &Repository,
    snapshot: &MemoryGitSnapshot,
    operation: &str,
) -> Result<(), MemoryGitError> {
    let actual = repo
        .find_reference("HEAD")?
        .symbolic_target()
        .map(str::to_owned);
    if actual != snapshot.symbolic_head {
        return Err(MemoryGitError::RecoveryRequired {
            operation: operation.into(),
            recovery: "Memory HEAD changed between symbolic and detached state".into(),
        });
    }
    Ok(())
}

fn lock_snapshot_references<'repo>(
    repo: &'repo Repository,
    snapshot: &MemoryGitSnapshot,
    operation: &str,
) -> Result<Transaction<'repo>, MemoryGitError> {
    let mut transaction = repo
        .transaction()
        .map_err(|error| MemoryGitError::RecoveryRequired {
            operation: operation.into(),
            recovery: format!("Memory reference transaction could not start: {error}"),
        })?;
    transaction
        .lock_ref("HEAD")
        .map_err(|error| MemoryGitError::RecoveryRequired {
            operation: operation.into(),
            recovery: format!("Memory HEAD could not be locked: {error}"),
        })?;
    if let Some(reference_name) = snapshot.symbolic_head.as_deref() {
        transaction
            .lock_ref(reference_name)
            .map_err(|error| MemoryGitError::RecoveryRequired {
                operation: operation.into(),
                recovery: format!("Memory branch could not be locked: {error}"),
            })?;
    }
    Ok(transaction)
}

fn ensure_snapshot_is_current(
    repo: &Repository,
    snapshot: &MemoryGitSnapshot,
) -> Result<(), MemoryGitError> {
    ensure_snapshot_head_is_current(repo, snapshot)?;
    let expected_index = git2::Oid::from_str(&snapshot.original_index_tree)?;
    let actual_index = current_index_tree(repo)?;
    if actual_index != expected_index {
        return Err(MemoryGitError::RecoveryRequired {
            operation: "preparing canonical Memory".into(),
            recovery: "Memory index changed after the transaction was reserved".into(),
        });
    }
    Ok(())
}

fn build_memory_tree(
    workspace: &Path,
    base_tree_id: git2::Oid,
    changes: Option<&[MemoryGitChange]>,
) -> Result<git2::Oid, MemoryGitError> {
    let repo = Repository::open(workspace)?;
    let mut index = git2::Index::new()?;
    repo.set_index(&mut index)?;
    let base_tree = repo.find_tree(base_tree_id)?;
    index.read_tree(&base_tree)?;
    match changes {
        Some(changes) => {
            let mut paths = BTreeSet::new();
            for change in changes {
                let text = change
                    .relative_path
                    .to_str()
                    .ok_or(MemoryGitError::InvalidPath)?;
                let relative_path = checked_memory_path(text)?;
                if !paths.insert(relative_path.clone()) {
                    return Err(MemoryGitError::InvalidPath);
                }
                if let Some(contents) = &change.contents {
                    index.add_frombuffer(
                        &git2::IndexEntry {
                            ctime: git2::IndexTime::new(0, 0),
                            mtime: git2::IndexTime::new(0, 0),
                            dev: 0,
                            ino: 0,
                            mode: 0o100644,
                            uid: 0,
                            gid: 0,
                            file_size: 0,
                            id: git2::Oid::zero(),
                            flags: 0,
                            flags_extended: 0,
                            path: text.as_bytes().to_vec(),
                        },
                        contents,
                    )?;
                } else if let Err(error) = index.remove_path(&relative_path) {
                    if error.code() != git2::ErrorCode::NotFound {
                        return Err(error.into());
                    }
                }
            }
        }
        None => {
            if workspace.join(MEMORY_PREFIX).is_dir() {
                index.add_all([MEMORY_PREFIX], IndexAddOption::DEFAULT, None)?;
            }
        }
    }
    Ok(index.write_tree()?)
}

fn repository_index_path(repo: &Repository) -> Result<PathBuf, MemoryGitError> {
    repo.index()?
        .path()
        .map(Path::to_path_buf)
        .ok_or_else(|| git2::Error::from_str("repository index has no on-disk path").into())
}

fn current_index_tree(repo: &Repository) -> Result<git2::Oid, MemoryGitError> {
    let index_path = repository_index_path(repo)?;
    let exists = validate_index_file(&index_path)?;
    let mut index = if exists {
        git2::Index::open(&index_path)?
    } else {
        git2::Index::new()?
    };
    Ok(index.write_tree_to(repo)?)
}

fn validate_index_file(index_path: &Path) -> Result<bool, MemoryGitError> {
    match filesystem::symlink_metadata(index_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(MemoryGitError::RecoveryRequired {
                operation: "validating the Memory index".into(),
                recovery: "the Git index path is not a regular file".into(),
            })
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

struct RemoveFileOnDrop {
    path: PathBuf,
    armed: bool,
}

impl RemoveFileOnDrop {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RemoveFileOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let _ = filesystem::remove_file(&self.path);
        }
    }
}

fn replace_index_tree_if_current(
    repo: &Repository,
    allowed_current: &[git2::Oid],
    desired: git2::Oid,
    operation: &str,
) -> Result<(), MemoryGitError> {
    let index_path = repository_index_path(repo)?;
    validate_index_file(&index_path)?;
    let parent = index_path
        .parent()
        .ok_or_else(|| git2::Error::from_str("repository index has no parent directory"))?;
    let temporary_path = parent.join(format!(".guruterminal-index-{}.tmp", uuid::Uuid::new_v4()));
    let _temporary_guard = RemoveFileOnDrop::new(temporary_path.clone());
    let mut desired_index = git2::Index::open(&temporary_path)?;
    desired_index.read_tree(&repo.find_tree(desired)?)?;
    desired_index.write()?;

    let mut lock_name = OsString::from(index_path.as_os_str());
    lock_name.push(".lock");
    let lock_path = PathBuf::from(lock_name);
    let mut lock_guard = RemoveFileOnDrop::new(lock_path.clone());
    let mut lock_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| MemoryGitError::RecoveryRequired {
            operation: operation.into(),
            recovery: format!("the Git index could not be locked: {error}"),
        })?;
    if !lock_file.metadata()?.is_file() {
        return Err(MemoryGitError::RecoveryRequired {
            operation: operation.into(),
            recovery: "the Git index lock is not a regular file".into(),
        });
    }

    // External Git writers honor index.lock. Re-read the real index only
    // after owning that lock so the tree comparison and replacement form one
    // CAS operation rather than a check-then-write race.
    let index_exists = validate_index_file(&index_path)?;
    let mut current_index = if index_exists {
        git2::Index::open(&index_path)?
    } else {
        git2::Index::new()?
    };
    let current = current_index.write_tree_to(repo)?;
    if !allowed_current.contains(&current) {
        return Err(MemoryGitError::RecoveryRequired {
            operation: operation.into(),
            recovery: "the Git index changed outside the Memory transaction".into(),
        });
    }
    if current == desired {
        drop(lock_file);
        return Ok(());
    }

    let mut desired_file = filesystem::File::open(&temporary_path)?;
    std::io::copy(&mut desired_file, &mut lock_file)?;
    lock_file.flush()?;
    lock_file.sync_all()?;
    drop(lock_file);
    if let Err(error) = publish_locked_index(&lock_path, &index_path, parent, index_exists) {
        return Err(MemoryGitError::RecoveryRequired {
            operation: operation.into(),
            recovery: format!("the locked Git index could not be published: {error}"),
        });
    }
    lock_guard.disarm();
    sync_index_parent(parent)?;
    Ok(())
}

#[cfg(unix)]
fn publish_locked_index(
    lock_path: &Path,
    index_path: &Path,
    _parent: &Path,
    _index_exists: bool,
) -> std::io::Result<()> {
    filesystem::rename(lock_path, index_path)
}

#[cfg(windows)]
fn publish_locked_index(
    lock_path: &Path,
    index_path: &Path,
    parent: &Path,
    index_exists: bool,
) -> std::io::Result<()> {
    if !index_exists {
        return crate::windows_fs::move_file_no_replace(lock_path, index_path);
    }
    // `std::fs::rename` cannot replace an existing destination on Windows.
    // ReplaceFileW preserves the atomic Git lock publication boundary without
    // deleting the live index first and exposing a missing-index window.
    let backup_path = parent.join(format!(".guruterminal-index-{}.bak", uuid::Uuid::new_v4()));
    let _backup_guard = RemoveFileOnDrop::new(backup_path.clone());
    crate::windows_fs::replace_file_with_backup(index_path, lock_path, &backup_path)
}

#[cfg(unix)]
fn sync_index_parent(parent: &Path) -> Result<(), MemoryGitError> {
    filesystem::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_index_parent(_parent: &Path) -> Result<(), MemoryGitError> {
    Ok(())
}

#[cfg(test)]
thread_local! {
    static FAIL_AFTER_INDEX_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_INDEX_RESTORE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn fail_after_index_write_for_test() -> Result<(), MemoryGitError> {
    if FAIL_AFTER_INDEX_WRITE.with(|fail| fail.replace(false)) {
        return Err(git2::Error::from_str("injected failure after index write").into());
    }
    Ok(())
}

#[cfg(test)]
fn fail_index_restore_for_test() -> Result<(), MemoryGitError> {
    if FAIL_INDEX_RESTORE.with(|fail| fail.replace(false)) {
        return Err(git2::Error::from_str("injected index restore failure").into());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn fail_next_commit_and_index_restore_for_test() {
    FAIL_AFTER_INDEX_WRITE.with(|fail| fail.set(true));
    FAIL_INDEX_RESTORE.with(|fail| fail.set(true));
}

#[cfg(not(test))]
fn fail_after_index_write_for_test() -> Result<(), MemoryGitError> {
    Ok(())
}

#[cfg(not(test))]
fn fail_index_restore_for_test() -> Result<(), MemoryGitError> {
    Ok(())
}

pub fn read_previous_markdown(
    workspace: &Path,
    relative_path: &str,
) -> Result<Option<PreviousMemoryVersion>, MemoryGitError> {
    let path = checked_memory_path(relative_path)?;
    let repo = match Repository::open(workspace) {
        Ok(repo) => repo,
        Err(_) => return Ok(None),
    };
    let mut revwalk = repo.revwalk()?;
    if repo.head().is_err() {
        return Ok(None);
    }
    revwalk.push_head()?;
    for oid in revwalk {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        if !commit_touches_path(&repo, &commit, &path)? {
            continue;
        }
        if commit.parent_count() == 0 {
            return Ok(None);
        }
        let parent = commit.parent(0)?;
        let tree = parent.tree()?;
        let entry = match tree.get_path(&path) {
            Ok(entry) => entry,
            Err(_) => return Ok(None),
        };
        let blob = repo.find_blob(entry.id())?;
        return Ok(Some(PreviousMemoryVersion {
            markdown: String::from_utf8_lossy(blob.content()).into_owned(),
            commit_id: parent.id().to_string(),
            message: commit.message().unwrap_or_default().to_owned(),
        }));
    }
    Ok(None)
}

pub fn read_markdown_at_commit(
    workspace: &Path,
    relative_path: &str,
    commit_id: &str,
) -> Result<Option<String>, MemoryGitError> {
    let path = checked_memory_path(relative_path)?;
    let repo = match Repository::open(workspace) {
        Ok(repo) => repo,
        Err(_) => return Ok(None),
    };
    let oid = git2::Oid::from_str(commit_id)?;
    let commit = repo.find_commit(oid)?;
    let tree = commit.tree()?;
    let entry = match tree.get_path(&path) {
        Ok(entry) => entry,
        Err(_) => return Ok(None),
    };
    let blob = repo.find_blob(entry.id())?;
    Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
}

pub fn recent_wiki_lens_ids(workspace: &Path, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let repo = match Repository::open(workspace) {
        Ok(repo) => repo,
        Err(_) => return Vec::new(),
    };
    if repo.head().is_err() {
        return Vec::new();
    }
    let mut revwalk = match repo.revwalk() {
        Ok(walk) => walk,
        Err(_) => return Vec::new(),
    };
    if revwalk.push_head().is_err() {
        return Vec::new();
    }
    let mut ids = Vec::new();
    let mut seen = BTreeSet::new();
    for oid in revwalk.flatten() {
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        let message = commit.message().unwrap_or_default();
        for token in message.split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, ':' | '/' | '-' | '_'))
        }) {
            if (token.starts_with("wiki:") || token.starts_with("lens:"))
                && seen.insert(token.to_owned())
            {
                ids.push(token.to_owned());
                if ids.len() >= limit {
                    return ids;
                }
            }
        }
    }
    ids
}

fn checked_memory_path(relative_path: &str) -> Result<PathBuf, MemoryGitError> {
    let path = Path::new(relative_path);
    if relative_path.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || path
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str())
            != Some(MEMORY_PREFIX)
    {
        return Err(MemoryGitError::InvalidPath);
    }
    Ok(path.to_path_buf())
}

fn commit_touches_path(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    path: &Path,
) -> Result<bool, MemoryGitError> {
    let new_tree = commit.tree()?;
    let old_tree = if commit.parent_count() == 0 {
        None
    } else {
        Some(commit.parent(0)?.tree()?)
    };
    let diff = repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)?;
    let mut touched = false;
    diff.foreach(
        &mut |delta, _| {
            let matches = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .is_some_and(|changed| changed == path);
            if matches {
                touched = true;
            }
            true
        },
        None,
        None,
        None,
    )?;
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_competing_commit(
        repo: &Repository,
        parent_id: git2::Oid,
        message: &str,
    ) -> git2::Oid {
        let parent = repo.find_commit(parent_id).unwrap();
        let tree = parent.tree().unwrap();
        let signature = Signature::now("External Git", "external@example.test").unwrap();
        repo.commit(None, &signature, &signature, message, &tree, &[&parent])
            .unwrap()
    }

    #[test]
    fn commit_then_previous_version_returns_prior_markdown() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first\n").unwrap();
        commit_memory(workspace, "user: create wiki:quality").unwrap();
        fs::write(&path, "second\n").unwrap();
        commit_memory(workspace, "user: revise wiki:quality").unwrap();
        let previous = read_previous_markdown(workspace, "guruterminal/wiki/quality.md")
            .unwrap()
            .expect("prior version");
        assert_eq!(previous.markdown, "first\n");
        assert!(previous.message.contains("revise"));
    }

    #[test]
    fn first_commit_atomically_publishes_a_missing_repository_index() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let repo = ensure_repository(workspace).unwrap();
        let index_path = repository_index_path(&repo).unwrap();
        if index_path.exists() {
            fs::remove_file(&index_path).unwrap();
        }
        let path = workspace.join("guruterminal/wiki/first.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first\n").unwrap();

        commit_memory(workspace, "chat: first").unwrap();

        assert!(index_path.is_file());
        let repo = Repository::open(workspace).unwrap();
        assert!(repo
            .index()
            .unwrap()
            .get_path(Path::new("guruterminal/wiki/first.md"), 0)
            .is_some());
    }

    #[test]
    fn empty_second_commit_reuses_head() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "only\n").unwrap();
        let first = commit_memory(workspace, "chat: first").unwrap();
        let second = commit_memory(workspace, "chat: noop").unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn failure_after_index_write_restores_index_and_head() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first\n").unwrap();
        let first = commit_memory(workspace, "chat: first").unwrap();
        let repo = Repository::open(workspace).unwrap();
        let index_before = repo.index().unwrap().write_tree().unwrap();

        fs::write(&path, "second\n").unwrap();
        FAIL_AFTER_INDEX_WRITE.with(|fail| fail.set(true));
        assert!(commit_memory(workspace, "chat: injected failure").is_err());

        let repo = Repository::open(workspace).unwrap();
        assert_eq!(repo.head().unwrap().target().unwrap().to_string(), first);
        assert_eq!(repo.index().unwrap().write_tree().unwrap(), index_before);
        assert_eq!(fs::read_to_string(path).unwrap(), "second\n");
    }

    #[test]
    fn successful_commit_can_be_compensated_to_exact_head_and_index() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first\n").unwrap();
        let first = commit_memory(workspace, "chat: first").unwrap();
        let repo = Repository::open(workspace).unwrap();
        let index_before = repo.index().unwrap().write_tree().unwrap();

        fs::write(&path, "second\n").unwrap();
        let mut snapshot = begin_memory_transaction(workspace).unwrap();
        let prepared = prepare_memory_commit(workspace, "chat: second", &snapshot).unwrap();
        snapshot.published_index_tree = Some(prepared.index_tree_id.clone());
        let second = finalize_memory_commit(workspace, prepared).unwrap();
        assert_ne!(second.commit_id, first);
        rollback_memory_snapshot(workspace, &snapshot, Some(&second.commit_id)).unwrap();

        let repo = Repository::open(workspace).unwrap();
        assert_eq!(repo.head().unwrap().target().unwrap().to_string(), first);
        assert_eq!(repo.index().unwrap().write_tree().unwrap(), index_before);
        assert_eq!(fs::read_to_string(path).unwrap(), "second\n");
    }

    #[test]
    fn finalize_rejects_a_competing_branch_move_without_overwriting_it() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first\n").unwrap();
        let first = commit_memory(workspace, "chat: first").unwrap();

        fs::write(&path, "second\n").unwrap();
        let snapshot = begin_memory_transaction(workspace).unwrap();
        let prepared = prepare_memory_commit(workspace, "chat: second", &snapshot).unwrap();
        let repo = Repository::open(workspace).unwrap();
        let competing = create_competing_commit(
            &repo,
            git2::Oid::from_str(&first).unwrap(),
            "external: competing branch update",
        );
        repo.reference(
            snapshot.symbolic_head.as_deref().unwrap(),
            competing,
            true,
            "external branch update",
        )
        .unwrap();

        let error = finalize_memory_commit(workspace, prepared).unwrap_err();
        assert!(error.recovery_required());
        assert_eq!(current_head(&repo), Some(competing));
    }

    #[test]
    fn finalize_rejects_a_competing_detached_head_move_without_overwriting_it() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first\n").unwrap();
        let first = commit_memory(workspace, "chat: first").unwrap();
        let repo = Repository::open(workspace).unwrap();
        repo.set_head_detached(git2::Oid::from_str(&first).unwrap())
            .unwrap();

        fs::write(&path, "second\n").unwrap();
        let snapshot = begin_memory_transaction(workspace).unwrap();
        assert!(snapshot.symbolic_head.is_none());
        let prepared = prepare_memory_commit(workspace, "chat: second", &snapshot).unwrap();
        let competing = create_competing_commit(
            &repo,
            git2::Oid::from_str(&first).unwrap(),
            "external: competing detached update",
        );
        repo.set_head_detached(competing).unwrap();

        let error = finalize_memory_commit(workspace, prepared).unwrap_err();
        assert!(error.recovery_required());
        assert_eq!(current_head(&repo), Some(competing));
    }

    #[test]
    fn rollback_rejects_a_competing_branch_move_without_overwriting_it() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first\n").unwrap();
        commit_memory(workspace, "chat: first").unwrap();

        fs::write(&path, "second\n").unwrap();
        let snapshot = begin_memory_transaction(workspace).unwrap();
        let prepared = prepare_memory_commit(workspace, "chat: second", &snapshot).unwrap();
        let second = finalize_memory_commit(workspace, prepared).unwrap();
        let repo = Repository::open(workspace).unwrap();
        let competing = create_competing_commit(
            &repo,
            git2::Oid::from_str(&second.commit_id).unwrap(),
            "external: update after finalization",
        );
        repo.reference(
            snapshot.symbolic_head.as_deref().unwrap(),
            competing,
            true,
            "external branch update",
        )
        .unwrap();

        let error =
            rollback_memory_snapshot(workspace, &snapshot, Some(&second.commit_id)).unwrap_err();
        assert!(error.recovery_required());
        assert_eq!(current_head(&repo), Some(competing));
    }

    #[test]
    fn rollback_removes_the_first_branch_commit_without_detaching_head() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        ensure_repository(workspace).unwrap();
        let mut snapshot = begin_memory_transaction(workspace).unwrap();
        assert!(snapshot.previous_head.is_none());
        let symbolic_head = snapshot.symbolic_head.clone().unwrap();
        let path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first\n").unwrap();

        let prepared = prepare_memory_commit(workspace, "chat: first", &snapshot).unwrap();
        snapshot.published_index_tree = Some(prepared.index_tree_id.clone());
        let first = finalize_memory_commit(workspace, prepared).unwrap();
        rollback_memory_snapshot(workspace, &snapshot, Some(&first.commit_id)).unwrap();

        let repo = Repository::open(workspace).unwrap();
        assert_eq!(current_head(&repo), None);
        assert_eq!(
            repo.find_reference("HEAD").unwrap().symbolic_target(),
            Some(symbolic_head.as_str())
        );
        assert!(repo.find_reference(&symbolic_head).is_err());
    }

    #[test]
    fn memory_commit_preserves_but_does_not_commit_unrelated_staging() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let memory_path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(memory_path.parent().unwrap()).unwrap();
        fs::write(&memory_path, "first\n").unwrap();
        commit_memory(workspace, "chat: first").unwrap();

        let unrelated_path = workspace.join("notes.txt");
        fs::write(&unrelated_path, "user staging\n").unwrap();
        let repo = Repository::open(workspace).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("notes.txt")).unwrap();
        index.write().unwrap();
        fs::write(&memory_path, "second\n").unwrap();

        commit_memory(workspace, "chat: second").unwrap();
        let repo = Repository::open(workspace).unwrap();
        let head_tree = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .tree()
            .unwrap();
        assert!(head_tree.get_path(Path::new("notes.txt")).is_err());
        assert!(repo
            .index()
            .unwrap()
            .get_path(Path::new("notes.txt"), 0)
            .is_some());
    }

    #[test]
    fn finalize_rejects_a_competing_index_without_overwriting_it() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let memory_path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(memory_path.parent().unwrap()).unwrap();
        fs::write(&memory_path, "first\n").unwrap();
        let first = commit_memory(workspace, "chat: first").unwrap();
        fs::write(&memory_path, "second\n").unwrap();
        let snapshot = begin_memory_transaction(workspace).unwrap();
        let prepared = prepare_memory_commit(workspace, "chat: second", &snapshot).unwrap();

        fs::write(workspace.join("outside.txt"), "external staging\n").unwrap();
        let repo = Repository::open(workspace).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("outside.txt")).unwrap();
        index.write().unwrap();

        let error = finalize_memory_commit(workspace, prepared).unwrap_err();
        assert!(error.recovery_required());
        let repo = Repository::open(workspace).unwrap();
        assert_eq!(repo.head().unwrap().target().unwrap().to_string(), first);
        assert!(repo
            .index()
            .unwrap()
            .get_path(Path::new("outside.txt"), 0)
            .is_some());
    }

    #[test]
    fn rollback_rejects_a_competing_index_before_moving_head() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let memory_path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(memory_path.parent().unwrap()).unwrap();
        fs::write(&memory_path, "first\n").unwrap();
        commit_memory(workspace, "chat: first").unwrap();
        fs::write(&memory_path, "second\n").unwrap();
        let mut snapshot = begin_memory_transaction(workspace).unwrap();
        let prepared = prepare_memory_commit(workspace, "chat: second", &snapshot).unwrap();
        snapshot.published_index_tree = Some(prepared.index_tree_id.clone());
        let second = finalize_memory_commit(workspace, prepared).unwrap();

        fs::write(workspace.join("outside.txt"), "external staging\n").unwrap();
        let repo = Repository::open(workspace).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("outside.txt")).unwrap();
        index.write().unwrap();

        let error =
            rollback_memory_snapshot(workspace, &snapshot, Some(&second.commit_id)).unwrap_err();
        assert!(error.recovery_required());
        let repo = Repository::open(workspace).unwrap();
        assert_eq!(
            repo.head().unwrap().target().unwrap().to_string(),
            second.commit_id
        );
        assert!(repo
            .index()
            .unwrap()
            .get_path(Path::new("outside.txt"), 0)
            .is_some());
    }

    #[test]
    fn exact_memory_changes_define_the_commit_even_if_the_worktree_changes() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        ensure_repository(workspace).unwrap();
        let mut snapshot = begin_memory_transaction(workspace).unwrap();
        let relative = PathBuf::from("guruterminal/wiki/quality.md");
        let memory_path = workspace.join(&relative);
        fs::create_dir_all(memory_path.parent().unwrap()).unwrap();
        fs::write(&memory_path, "competing worktree bytes\n").unwrap();
        let prepared = prepare_memory_commit_exact(
            workspace,
            "chat: exact",
            &snapshot,
            &[MemoryGitChange {
                relative_path: relative.clone(),
                contents: Some(b"journaled bytes\n".to_vec()),
            }],
        )
        .unwrap();
        snapshot.published_index_tree = Some(prepared.index_tree_id.clone());
        let committed = finalize_memory_commit(workspace, prepared).unwrap();

        let repo = Repository::open(workspace).unwrap();
        let tree = repo
            .find_commit(git2::Oid::from_str(&committed.commit_id).unwrap())
            .unwrap()
            .tree()
            .unwrap();
        let blob = repo
            .find_blob(tree.get_path(&relative).unwrap().id())
            .unwrap();
        assert_eq!(blob.content(), b"journaled bytes\n");
        assert_eq!(
            fs::read(&memory_path).unwrap(),
            b"competing worktree bytes\n"
        );
    }

    #[test]
    fn failed_commit_and_failed_index_restore_requires_recovery() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path();
        let path = workspace.join("guruterminal/wiki/quality.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first\n").unwrap();
        commit_memory(workspace, "chat: first").unwrap();
        fs::write(&path, "second\n").unwrap();
        FAIL_AFTER_INDEX_WRITE.with(|fail| fail.set(true));
        FAIL_INDEX_RESTORE.with(|fail| fail.set(true));

        let error = commit_memory(workspace, "chat: injected failure").unwrap_err();
        assert!(error.recovery_required());
    }
}
