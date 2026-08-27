use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::fs::File;
#[cfg(any(unix, windows))]
use std::sync::Arc;

use serde_json::Value;

use crate::{
    app::CommandError,
    domain::{GuruProfile, RootFilesystemIdentity},
    runtime::{GuruTerminalRuntime, RuntimeError, StagedMemoryChange},
    snapshot::{SnapshotError, SnapshotRecord},
};

#[cfg(unix)]
use crate::pinned_root::{PinnedGuruRoot, PinnedRootError};
#[cfg(windows)]
use crate::windows_fs::{
    filesystem_identity, metadata_is_reparse, open_directory_no_reparse, open_regular_no_reparse,
};

#[cfg(windows)]
#[derive(Debug)]
struct WindowsPinnedGuruRoot {
    handle: File,
    identity: RootFilesystemIdentity,
    opened_path: PathBuf,
}

#[cfg(windows)]
impl WindowsPinnedGuruRoot {
    fn open_unbound(path: PathBuf) -> Result<Self, std::io::Error> {
        let handle = open_directory_no_reparse(&path)?;
        let identity = filesystem_identity(&handle)?;
        Ok(Self {
            handle,
            identity,
            opened_path: path,
        })
    }

    fn open(path: PathBuf, expected: &RootFilesystemIdentity) -> Result<Self, std::io::Error> {
        let pinned = Self::open_unbound(path)?;
        if &pinned.identity != expected {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Guru root filesystem identity changed",
            ));
        }
        Ok(pinned)
    }

    fn verify_path(&self) -> Result<(), std::io::Error> {
        if filesystem_identity(&self.handle)? != self.identity {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "held Guru root identity changed",
            ));
        }
        let reopened = open_directory_no_reparse(&self.opened_path)?;
        if filesystem_identity(&reopened)? != self.identity {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Guru root path was rebound",
            ));
        }
        Ok(())
    }
}

#[cfg(windows)]
struct WindowsGuruOperationGuard {
    _handles: Vec<File>,
}

#[cfg(windows)]
impl WindowsGuruOperationGuard {
    fn open(workspace: &Path, include_files: bool) -> Result<Self, std::io::Error> {
        let mut handles = vec![open_directory_no_reparse(workspace)?];
        for child in [".guruterminal", "guruterminal"] {
            let path = workspace.join(child);
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() && !metadata_is_reparse(&metadata) => {
                    guard_windows_tree(&path, 0, include_files, &mut handles)?;
                }
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Guru workspace contains a reparse or non-directory root",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(Self { _handles: handles })
    }
}

#[cfg(windows)]
fn guard_windows_tree(
    directory: &Path,
    depth: usize,
    include_files: bool,
    handles: &mut Vec<File>,
) -> Result<(), std::io::Error> {
    const MAX_GUARDED_DEPTH: usize = 64;
    const MAX_GUARDED_ENTRIES: usize = 10_000;
    if depth > MAX_GUARDED_DEPTH || handles.len() >= MAX_GUARDED_ENTRIES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Guru workspace exceeds its guarded filesystem boundary",
        ));
    }
    handles.push(open_directory_no_reparse(directory)?);
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if handles.len() >= MAX_GUARDED_ENTRIES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Guru workspace exceeds its guarded filesystem boundary",
            ));
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || metadata_is_reparse(&metadata) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Guru workspace contains a reparse point",
            ));
        }
        if metadata.is_dir() {
            guard_windows_tree(&path, depth + 1, include_files, handles)?;
        } else if metadata.is_file() {
            if include_files {
                handles.push(open_regular_no_reparse(&path)?);
            }
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Guru workspace contains an unsupported filesystem entry",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct BoundGuruRoot {
    #[cfg(unix)]
    pinned: Arc<PinnedGuruRoot>,
    #[cfg(windows)]
    pinned: Arc<WindowsPinnedGuruRoot>,
    #[cfg(not(unix))]
    path: PathBuf,
}

impl BoundGuruRoot {
    pub(crate) fn open_unbound(path: PathBuf) -> Result<Self, CommandError> {
        if !path.is_absolute() {
            return Err(CommandError::invalid("selected Guru root is not absolute"));
        }
        #[cfg(unix)]
        {
            let pinned = PinnedGuruRoot::open_unbound(&path).map_err(map_selected_root_error)?;
            Ok(Self {
                pinned: Arc::new(pinned),
            })
        }
        #[cfg(windows)]
        {
            let pinned = WindowsPinnedGuruRoot::open_unbound(path.clone()).map_err(map_internal)?;
            Ok(Self {
                pinned: Arc::new(pinned),
                path,
            })
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            let metadata = std::fs::symlink_metadata(&path).map_err(map_internal)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(CommandError::invalid(
                    "selected Guru root is not a regular directory",
                ));
            }
            Ok(Self { path })
        }
    }

    pub(crate) fn path(&self) -> &Path {
        #[cfg(unix)]
        {
            self.pinned.opened_path()
        }
        #[cfg(not(unix))]
        {
            &self.path
        }
    }

    pub(crate) fn identity(&self) -> Option<RootFilesystemIdentity> {
        #[cfg(unix)]
        {
            Some(self.pinned.identity().clone())
        }
        #[cfg(windows)]
        {
            Some(self.pinned.identity.clone())
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            None
        }
    }

    pub(crate) async fn initialize(
        &self,
        runtime: &GuruTerminalRuntime,
    ) -> Result<(), RuntimeError> {
        #[cfg(unix)]
        {
            runtime.initialize_at(&self.pinned).await
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            let _operation_guard = self.windows_operation_guard(false)?;
            runtime.initialize(&self.path).await
        }
    }

    pub(crate) async fn validate(&self, runtime: &GuruTerminalRuntime) -> Result<(), RuntimeError> {
        #[cfg(unix)]
        {
            runtime.validate_at(&self.pinned).await
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            let _operation_guard = self.windows_operation_guard(true)?;
            runtime.validate(&self.path).await
        }
    }

    pub(crate) async fn knowledge_search(
        &self,
        runtime: &GuruTerminalRuntime,
        query: &str,
        kind: Option<&str>,
        limit: u8,
        include_revoked: bool,
        as_of: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        #[cfg(unix)]
        {
            runtime
                .knowledge_search_at(&self.pinned, query, kind, limit, include_revoked, as_of)
                .await
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            let _operation_guard = self.windows_operation_guard(true)?;
            runtime
                .knowledge_search(&self.path, query, kind, limit, include_revoked, as_of)
                .await
        }
    }

    pub(crate) async fn knowledge_read(
        &self,
        runtime: &GuruTerminalRuntime,
        id: &str,
        section: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        #[cfg(unix)]
        {
            runtime.knowledge_read_at(&self.pinned, id, section).await
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            let _operation_guard = self.windows_operation_guard(true)?;
            runtime.knowledge_read(&self.path, id, section).await
        }
    }

    pub(crate) async fn knowledge_list(
        &self,
        runtime: &GuruTerminalRuntime,
        kind: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        #[cfg(unix)]
        {
            runtime.knowledge_list_at(&self.pinned, kind).await
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            let _operation_guard = self.windows_operation_guard(true)?;
            runtime.knowledge_list(&self.path, kind).await
        }
    }

    pub(crate) async fn knowledge_context(
        &self,
        runtime: &GuruTerminalRuntime,
    ) -> Result<Value, RuntimeError> {
        #[cfg(unix)]
        {
            runtime.knowledge_context_at(&self.pinned).await
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            let _operation_guard = self.windows_operation_guard(true)?;
            runtime.knowledge_context(&self.path).await
        }
    }

    pub(crate) fn inspect_memory_tree(
        &self,
    ) -> Result<(String, Vec<SnapshotRecord>), SnapshotError> {
        #[cfg(unix)]
        {
            crate::snapshot::inspect_memory_tree_at(&self.pinned)
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            self.verify_windows_snapshot()?;
            crate::snapshot::inspect_memory_tree(&self.path)
        }
    }

    pub(crate) fn read_memory_record(
        &self,
        target_relative_path: &Path,
    ) -> Result<Option<Vec<u8>>, SnapshotError> {
        #[cfg(unix)]
        {
            crate::snapshot::read_memory_record_at(&self.pinned, target_relative_path)
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            self.verify_windows_snapshot()?;
            crate::snapshot::read_memory_record(&self.path, target_relative_path)
        }
    }

    pub(crate) async fn apply_memory_markdown_set(
        &self,
        runtime: &GuruTerminalRuntime,
        approved: &[StagedMemoryChange],
    ) -> Result<(), RuntimeError> {
        #[cfg(unix)]
        {
            runtime
                .apply_memory_markdown_set_at(&self.pinned, approved)
                .await
                .map(|_| ())
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            let _operation_guard = self.windows_operation_guard(false)?;
            let result = runtime
                .apply_memory_markdown_set(&self.path, approved)
                .await
                .map(|_| ());
            #[cfg(windows)]
            self.verify_windows_runtime()?;
            result
        }
    }

    pub(crate) async fn rollback_memory_markdown_set(
        &self,
        runtime: &GuruTerminalRuntime,
        approved: &[StagedMemoryChange],
    ) -> Result<(), RuntimeError> {
        #[cfg(unix)]
        {
            runtime
                .rollback_memory_markdown_set_at(&self.pinned, approved)
                .await
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            let _operation_guard = self.windows_operation_guard(false)?;
            let result = runtime
                .rollback_memory_markdown_set(&self.path, approved)
                .await;
            #[cfg(windows)]
            self.verify_windows_runtime()?;
            result
        }
    }

    pub(crate) fn reconcile_memory_artifact(
        &self,
        runtime: &GuruTerminalRuntime,
        approved: &StagedMemoryChange,
        target_is_proposed: bool,
    ) -> Result<(), RuntimeError> {
        #[cfg(unix)]
        {
            runtime.reconcile_memory_artifact_at(&self.pinned, approved, target_is_proposed)
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            let _operation_guard = self.windows_operation_guard(false)?;
            let result =
                runtime.reconcile_memory_artifact(&self.path, approved, target_is_proposed);
            #[cfg(windows)]
            self.verify_windows_runtime()?;
            result
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn stage_memory_artifact_for_test(
        &self,
        runtime: &GuruTerminalRuntime,
        approved: &StagedMemoryChange,
        bytes: &[u8],
    ) -> Result<(), RuntimeError> {
        runtime.stage_memory_artifact_at_for_test(&self.pinned, approved, bytes)
    }

    #[cfg(windows)]
    fn verify_windows_runtime(&self) -> Result<(), RuntimeError> {
        self.pinned
            .verify_path()
            .map_err(|_| RuntimeError::MemoryBoundary)
    }

    #[cfg(windows)]
    fn windows_operation_guard(
        &self,
        include_files: bool,
    ) -> Result<WindowsGuruOperationGuard, RuntimeError> {
        self.verify_windows_runtime()?;
        WindowsGuruOperationGuard::open(&self.path, include_files)
            .map_err(|_| RuntimeError::MemoryBoundary)
    }

    #[cfg(windows)]
    fn verify_windows_snapshot(&self) -> Result<(), SnapshotError> {
        self.pinned
            .verify_path()
            .map_err(|_| SnapshotError::UnsupportedEntry)
    }
}

pub(crate) fn profile_workspace(profile: &GuruProfile) -> Result<BoundGuruRoot, CommandError> {
    profile.validate().map_err(map_internal)?;
    let path = PathBuf::from(&profile.memory_root);
    if !path.is_absolute() {
        return Err(CommandError::internal("stored Guru root is not absolute"));
    }
    #[cfg(unix)]
    {
        let expected = profile.root_filesystem_identity.as_ref().ok_or_else(|| {
            CommandError::conflict("Guru root identity is missing; import it again")
        })?;
        let pinned = PinnedGuruRoot::open(&path, expected).map_err(map_profile_root_error)?;
        Ok(BoundGuruRoot {
            pinned: Arc::new(pinned),
        })
    }
    #[cfg(windows)]
    {
        let expected = profile.root_filesystem_identity.as_ref().ok_or_else(|| {
            CommandError::conflict("Guru root identity is missing; import it again")
        })?;
        let pinned = WindowsPinnedGuruRoot::open(path.clone(), expected).map_err(|_| {
            CommandError::conflict("Guru root moved or was rebound; import it again")
        })?;
        Ok(BoundGuruRoot {
            pinned: Arc::new(pinned),
            path,
        })
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let canonical = path
            .canonicalize()
            .map_err(|_| CommandError::conflict("Guru root moved; import it again"))?;
        if canonical != path {
            return Err(CommandError::conflict(
                "Guru root was rebound; import it again",
            ));
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(map_internal)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(CommandError::conflict(
                "Guru root is not the imported directory",
            ));
        }
        Ok(BoundGuruRoot { path: canonical })
    }
}

fn map_internal(error: impl std::fmt::Display) -> CommandError {
    CommandError::internal(error.to_string())
}

#[cfg(unix)]
fn map_selected_root_error(error: PinnedRootError) -> CommandError {
    match error {
        PinnedRootError::InvalidPath | PinnedRootError::NotDirectory => {
            CommandError::invalid("selected Guru root is not a regular directory")
        }
        PinnedRootError::IdentityMismatch => {
            CommandError::conflict("Guru root identity changed; import it again")
        }
        PinnedRootError::UnsupportedPlatform => {
            CommandError::internal("pinned Guru roots are unavailable")
        }
        PinnedRootError::Io(error) => CommandError::invalid(error.to_string()),
    }
}

#[cfg(unix)]
fn map_profile_root_error(error: PinnedRootError) -> CommandError {
    match error {
        PinnedRootError::IdentityMismatch => {
            CommandError::conflict("Guru root identity changed; import it again")
        }
        PinnedRootError::InvalidPath
        | PinnedRootError::NotDirectory
        | PinnedRootError::UnsupportedPlatform
        | PinnedRootError::Io(_) => {
            CommandError::conflict("Guru root moved or was rebound; import it again")
        }
    }
}
