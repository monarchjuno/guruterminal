use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    app::CommandError,
    run_id::validate_run_id,
    secure_delete::{PrivateDirectoryGuard, SecureDeletionRoot},
};

/// Retained capability and RAII cleanup for one disposable `runs/<run-id>`
/// tree. Durable state must never point into this directory.
#[derive(Debug)]
pub(crate) struct RunScratch {
    root: Arc<SecureDeletionRoot>,
    relative: PathBuf,
    path: PathBuf,
    guard: Option<PrivateDirectoryGuard>,
}

impl RunScratch {
    pub(crate) fn create(
        root: Arc<SecureDeletionRoot>,
        guru_id: &str,
        run_id: &str,
    ) -> Result<Self, CommandError> {
        validate_run_id(guru_id, "Guru")?;
        validate_run_id(run_id, "private")?;
        let relative = PathBuf::from("runs").join(guru_id).join(run_id);
        if root.entry_exists(&relative)? {
            root.remove_tree(&relative)?;
        }
        let guard = root.ensure_private_subdirectory(&relative)?;
        let path = root.absolute_path(&relative)?;
        Ok(Self {
            root,
            relative,
            path,
            guard: Some(guard),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RunScratch {
    fn drop(&mut self) {
        self.guard = None;
        let _ = self.root.remove_tree(&self.relative);
    }
}

pub(crate) fn sweep_stale_runs(root: &SecureDeletionRoot) -> Result<(), CommandError> {
    // Scratch is derived and must not brick unrelated Gurus if an adversarial
    // stale entry cannot be reclaimed. New UUID run IDs remain isolated.
    let _ = root.remove_tree(Path::new("runs"));
    root.ensure_private_subdirectory(Path::new("runs"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn drop_removes_sensitive_scratch_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let root =
            Arc::new(SecureDeletionRoot::open(&temporary.path().canonicalize().unwrap()).unwrap());
        let path = {
            let scratch = RunScratch::create(root, "guru-a", "chat-ui-123").unwrap();
            fs::write(scratch.path().join("skill.md"), b"unique-secret").unwrap();
            scratch.path().to_owned()
        };
        assert!(!path.exists());
    }

    #[test]
    fn startup_sweep_removes_crashed_runs() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir_all(temporary.path().join("runs/crashed")).unwrap();
        fs::write(temporary.path().join("runs/crashed/secret"), b"secret").unwrap();
        let root = SecureDeletionRoot::open(&temporary.path().canonicalize().unwrap()).unwrap();

        sweep_stale_runs(&root).unwrap();

        assert!(temporary.path().join("runs").is_dir());
        assert!(!temporary.path().join("runs/crashed").exists());
    }
}
