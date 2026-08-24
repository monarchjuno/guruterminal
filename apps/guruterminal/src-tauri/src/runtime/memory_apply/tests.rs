use super::*;

#[cfg(unix)]
use std::{
    ffi::CString,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
};

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

#[cfg(unix)]
fn runtime_fixture() -> (tempfile::TempDir, GuruTerminalRuntime, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    create_initialized_workspace(&workspace, "runtime");
    let runtime = write_test_runtime(
        temp.path(),
        "guruterminal-test",
        "#!/bin/sh\nprintf '{}\\n'\n",
    );
    (temp, runtime, workspace)
}

fn approved_change(before: Option<&str>, proposed: &str) -> StagedMemoryChange {
    StagedMemoryChange {
        guru_id: "guru-1".into(),
        session_id: "memory-change-1".into(),
        relative_path: PathBuf::from("guruterminal/lens/quality.md"),
        before_sha256: before.map(|markdown| sha256(markdown.as_bytes())),
        before_markdown: before.map(str::to_owned),
        proposed_sha256: sha256(proposed.as_bytes()),
        proposed_markdown: proposed.into(),
        delete: false,
    }
}

fn approved_delete(before: &str) -> StagedMemoryChange {
    let mut approved = approved_change(Some(before), "");
    approved.delete = true;
    approved
}

#[test]
fn windows_recovery_matrix_validates_every_artifact_before_cleanup() {
    let approved = approved_change(Some("before"), "proposed");
    assert!(validate_windows_recovery_artifacts(
        &approved,
        true,
        Some(b"before"),
        Some(b"proposed"),
    )
    .is_ok());
    assert!(validate_windows_recovery_artifacts(
        &approved,
        false,
        Some(b"proposed"),
        Some(b"before"),
    )
    .is_ok());

    assert!(matches!(
        validate_windows_recovery_artifacts(&approved, true, Some(b"before"), Some(b"tampered"),),
        Err(RuntimeError::RollbackConflict)
    ));
    assert!(matches!(
        validate_windows_recovery_artifacts(&approved, false, Some(b"tampered"), Some(b"proposed"),),
        Err(RuntimeError::RollbackConflict)
    ));
}

#[test]
fn target_is_limited_to_canonical_memory_markdown_kinds() {
    let temp = tempfile::tempdir().unwrap();
    for kind in ["wiki", "lens", "evidence", "decision"] {
        fs::create_dir_all(temp.path().join("guruterminal").join(kind)).unwrap();
    }
    assert!(resolve_memory_target(temp.path(), Path::new("guruterminal/lens/a.md")).is_ok());
    assert!(resolve_memory_target(temp.path(), Path::new("guruterminal/evidence/a.md")).is_ok());
    assert!(resolve_memory_target(temp.path(), Path::new("guruterminal/decision/a.md")).is_ok());
    assert!(resolve_memory_target(temp.path(), Path::new("../outside.md")).is_err());
    assert!(resolve_memory_target(temp.path(), Path::new("guruterminal/lens/a.txt")).is_err());
}

#[test]
fn digest_is_stable() {
    assert_eq!(
        sha256(b"guru"),
        "298bab1136dcde8c0157190fa5374cbf36c33f79b13a7597da8027c5afe8dc31"
    );
}

#[cfg(unix)]
#[test]
fn pinned_reads_reject_special_and_oversized_files() {
    let directory = tempfile::tempdir().unwrap();
    let descriptor = open(
        directory.path(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .unwrap();

    fs::write(
        directory.path().join("oversized.md"),
        vec![b'x'; MAX_MEMORY_FILE_BYTES as usize + 1],
    )
    .unwrap();
    assert!(matches!(
        read_regular_at(&descriptor, OsStr::new("oversized.md")),
        Err(RuntimeError::InvalidTarget)
    ));

    let fifo = directory.path().join("memory.fifo");
    let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: `fifo_path` is a NUL-terminated path owned for this call.
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
    assert!(read_regular_at(&descriptor, OsStr::new("memory.fifo")).is_err());
}

#[test]
fn staged_publish_does_not_clobber_a_new_or_edited_target() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target.md");
    let staged_new = directory.path().join("new.staged");
    fs::write(&staged_new, "approved").unwrap();
    fs::write(&target, "concurrent create").unwrap();
    assert!(matches!(
        commit_staged_file(&staged_new, &target, None),
        Err(RuntimeError::BeforeHashMismatch)
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), "concurrent create");

    let staged_existing = directory.path().join("existing.staged");
    fs::write(&staged_existing, "approved").unwrap();
    assert!(matches!(
        commit_staged_file(&staged_existing, &target, Some(&sha256(b"approved before")),),
        Err(RuntimeError::BeforeHashMismatch)
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), "concurrent create");
}

#[cfg(unix)]
#[tokio::test]
async fn pinned_memory_write_and_rollback_never_write_replacement_b() {
    let temporary = tempfile::tempdir().unwrap();
    let root_a = temporary.path().join("guru-a");
    let root_b = temporary.path().join("guru-b");
    let moved_a = temporary.path().join("guru-a-original");
    create_initialized_workspace(&root_a, "A");
    create_initialized_workspace(&root_b, "B");
    fs::write(root_a.join("guruterminal/lens/quality.md"), "a-old").unwrap();
    fs::write(root_b.join("guruterminal/lens/quality.md"), "b-old").unwrap();
    let pinned = PinnedGuruRoot::open_unbound(&root_a).unwrap();
    let runtime = write_test_runtime(
        temporary.path(),
        "guruterminal-memory-write-test",
        "#!/bin/sh\nprintf '{}\\n'\n",
    );
    let approved = approved_change(Some("a-old"), "a-new");

    fs::rename(&root_a, &moved_a).unwrap();
    fs::rename(&root_b, &root_a).unwrap();

    runtime
        .stage_memory_artifact_at_for_test(
            &pinned,
            &approved,
            approved.proposed_markdown.as_bytes(),
        )
        .unwrap();
    runtime
        .reconcile_memory_artifact_at(&pinned, &approved, false)
        .unwrap();
    runtime
        .apply_memory_markdown_at(&pinned, &approved)
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(moved_a.join("guruterminal/lens/quality.md")).unwrap(),
        "a-new"
    );
    assert_eq!(
        fs::read_to_string(root_a.join("guruterminal/lens/quality.md")).unwrap(),
        "b-old"
    );
    assert!(!root_a
        .join(".guruterminal/guruterminal-transactions")
        .exists());

    runtime
        .rollback_memory_markdown_at(&pinned, &approved)
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(moved_a.join("guruterminal/lens/quality.md")).unwrap(),
        "a-old"
    );
    assert_eq!(
        fs::read_to_string(root_a.join("guruterminal/lens/quality.md")).unwrap(),
        "b-old"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn existing_target_is_atomically_replaced_and_can_be_restored() {
    let (_temp, runtime, workspace) = runtime_fixture();
    let target = workspace.join("guruterminal/lens/quality.md");
    fs::write(&target, "old").unwrap();
    let approved = approved_change(Some("old"), "new");

    let applied = runtime
        .apply_memory_markdown(&workspace, &approved)
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    assert_eq!(applied.before_sha256, approved.before_sha256);
    assert_eq!(applied.after_sha256, approved.proposed_sha256);
    assert!(fs::read_dir(target.parent().unwrap())
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("backup")));

    runtime
        .rollback_memory_markdown(&workspace, &approved)
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(target).unwrap(), "old");
}

#[cfg(unix)]
#[tokio::test]
async fn existing_target_can_be_atomically_deleted_and_restored() {
    let (_temp, runtime, workspace) = runtime_fixture();
    let target = workspace.join("guruterminal/lens/quality.md");
    fs::write(&target, "created by the revision").unwrap();
    let approved = approved_delete("created by the revision");

    runtime
        .apply_memory_markdown(&workspace, &approved)
        .await
        .unwrap();
    assert!(!target.exists());

    runtime
        .rollback_memory_markdown(&workspace, &approved)
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "created by the revision"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn evidence_lens_and_decision_change_set_applies_and_rolls_back_as_one_unit() {
    let (_temp, runtime, workspace) = runtime_fixture();
    let lens_path = workspace.join("guruterminal/lens/quality.md");
    let evidence_path = workspace.join("guruterminal/evidence/source.md");
    let decision_path = workspace.join("guruterminal/decision/chat.md");
    fs::write(&lens_path, "lens-old").unwrap();
    fs::write(&evidence_path, "evidence-old").unwrap();
    fs::write(&decision_path, "decision-old").unwrap();
    let lens = approved_change(Some("lens-old"), "lens-new");
    let mut evidence = approved_change(Some("evidence-old"), "evidence-new");
    evidence.relative_path = PathBuf::from("guruterminal/evidence/source.md");
    let mut decision = approved_change(Some("decision-old"), "decision-new");
    decision.relative_path = PathBuf::from("guruterminal/decision/chat.md");
    let changes = vec![evidence, lens, decision];

    runtime
        .apply_memory_markdown_set(&workspace, &changes)
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&lens_path).unwrap(), "lens-new");
    assert_eq!(fs::read_to_string(&evidence_path).unwrap(), "evidence-new");
    assert_eq!(fs::read_to_string(&decision_path).unwrap(), "decision-new");

    runtime
        .rollback_memory_markdown_set(&workspace, &changes)
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(lens_path).unwrap(), "lens-old");
    assert_eq!(fs::read_to_string(evidence_path).unwrap(), "evidence-old");
    assert_eq!(fs::read_to_string(decision_path).unwrap(), "decision-old");
}

#[cfg(unix)]
#[tokio::test]
async fn rollback_refuses_to_overwrite_a_concurrent_edit() {
    let (_temp, runtime, workspace) = runtime_fixture();
    let target = workspace.join("guruterminal/lens/quality.md");
    fs::write(&target, "old").unwrap();
    let approved = approved_change(Some("old"), "new");
    runtime
        .apply_memory_markdown(&workspace, &approved)
        .await
        .unwrap();
    fs::write(&target, "concurrent edit").unwrap();

    assert!(matches!(
        runtime
            .rollback_memory_markdown(&workspace, &approved)
            .await,
        Err(RuntimeError::RollbackConflict)
    ));
    assert_eq!(fs::read_to_string(target).unwrap(), "concurrent edit");
}

#[cfg(unix)]
#[tokio::test]
async fn rollback_removes_a_target_that_did_not_exist_before_the_write() {
    let (_temp, runtime, workspace) = runtime_fixture();
    let target = workspace.join("guruterminal/lens/quality.md");
    let approved = approved_change(None, "new");
    runtime
        .apply_memory_markdown(&workspace, &approved)
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");

    runtime
        .rollback_memory_markdown(&workspace, &approved)
        .await
        .unwrap();
    assert!(!target.exists());
}

#[cfg(unix)]
#[test]
fn recovery_removes_an_exact_artifact_left_before_publish() {
    let (_temp, runtime, workspace) = runtime_fixture();
    let target = workspace.join("guruterminal/lens/quality.md");
    fs::write(&target, "old").unwrap();
    let approved = approved_change(Some("old"), "new");
    let transaction = PinnedMemoryTransaction::open(&workspace, &approved).unwrap();
    transaction
        .write_artifact(approved.proposed_markdown.as_bytes())
        .unwrap();

    runtime
        .reconcile_memory_artifact(&workspace, &approved, false)
        .unwrap();

    assert_eq!(fs::read_to_string(target).unwrap(), "old");
    assert!(transaction.read_artifact().unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn recovery_removes_the_expected_displaced_base_after_exchange() {
    let (_temp, runtime, workspace) = runtime_fixture();
    let target = workspace.join("guruterminal/lens/quality.md");
    fs::write(&target, "old").unwrap();
    let approved = approved_change(Some("old"), "new");
    let transaction = PinnedMemoryTransaction::open(&workspace, &approved).unwrap();
    transaction
        .write_artifact(approved.proposed_markdown.as_bytes())
        .unwrap();
    transaction.exchange().unwrap();

    runtime
        .reconcile_memory_artifact(&workspace, &approved, true)
        .unwrap();

    assert_eq!(fs::read_to_string(target).unwrap(), "new");
    assert!(transaction.read_artifact().unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn recovery_preserves_a_concurrent_edit_displaced_by_exchange() {
    let (_temp, runtime, workspace) = runtime_fixture();
    let target = workspace.join("guruterminal/lens/quality.md");
    fs::write(&target, "old").unwrap();
    let approved = approved_change(Some("old"), "new");
    let transaction = PinnedMemoryTransaction::open(&workspace, &approved).unwrap();
    transaction
        .write_artifact(approved.proposed_markdown.as_bytes())
        .unwrap();
    fs::write(&target, "concurrent edit").unwrap();
    transaction.exchange().unwrap();

    assert!(matches!(
        runtime.reconcile_memory_artifact(&workspace, &approved, true),
        Err(RuntimeError::RollbackConflict)
    ));
    assert_eq!(fs::read_to_string(target).unwrap(), "new");
    assert_eq!(
        transaction.read_artifact().unwrap().as_deref(),
        Some(b"concurrent edit".as_slice())
    );
}

#[cfg(unix)]
#[test]
fn recovery_does_not_delete_a_tampered_prepublish_artifact() {
    let (_temp, runtime, workspace) = runtime_fixture();
    let target = workspace.join("guruterminal/lens/quality.md");
    fs::write(&target, "old").unwrap();
    let approved = approved_change(Some("old"), "new");
    let transaction = PinnedMemoryTransaction::open(&workspace, &approved).unwrap();
    transaction.write_artifact(b"tampered").unwrap();

    assert!(matches!(
        runtime.reconcile_memory_artifact(&workspace, &approved, false),
        Err(RuntimeError::RollbackConflict)
    ));
    assert_eq!(
        transaction.read_artifact().unwrap().as_deref(),
        Some(b"tampered".as_slice())
    );
}
