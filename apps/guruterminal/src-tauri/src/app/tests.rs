use super::*;

#[test]
fn development_build_isolated_from_installed_app_data() {
    let production_data = PathBuf::from("app-data");

    assert_eq!(
        app_data_dir_for_build(production_data.clone(), PRODUCTION_APP_IDENTIFIER),
        production_data.join("development")
    );
    assert_eq!(
        app_data_dir_for_build(production_data.clone(), "com.monarchjuno.guruterminal.e2e"),
        production_data
    );
}

#[test]
fn hidden_models_are_rejected_before_pi_execution() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app-data"));
    state.set_model_visible("fixture", false).unwrap();

    let error = state
        .pi_execution("fixture", "off", &std::collections::BTreeMap::new())
        .err()
        .unwrap();
    assert!(error.to_string().contains("hidden from Chat"));
}

#[test]
fn development_finance_worker_uses_the_staged_runtime_before_local_dist() {
    let manifest_dir = PathBuf::from("apps/guruterminal/src-tauri");

    assert_eq!(
        local_debug_finance_candidates(&manifest_dir),
        [
            manifest_dir
                .join("resources/pi-runtime/finance-worker")
                .join(platform_binary("guruterminal-finance")),
            manifest_dir
                .join("../python/dist/guruterminal-finance")
                .join(platform_binary("guruterminal-finance")),
        ]
    );
}

#[cfg(feature = "e2e")]
#[test]
fn e2e_app_data_requires_an_explicit_absolute_root() {
    let absolute = std::env::current_dir().unwrap().join("isolated-e2e");

    assert!(e2e_app_data_dir(None).is_err());
    assert!(e2e_app_data_dir(Some(PathBuf::from("relative"))).is_err());
    assert_eq!(e2e_app_data_dir(Some(absolute.clone())).unwrap(), absolute);
}

#[cfg(unix)]
#[test]
fn app_data_paths_are_derived_from_one_canonical_root() {
    let temporary = tempfile::tempdir().unwrap();
    let actual_parent = temporary.path().join("actual");
    ensure_private_directory(&actual_parent).unwrap();
    let alias_parent = temporary.path().join("alias");
    std::os::unix::fs::symlink(&actual_parent, &alias_parent).unwrap();
    let requested = alias_parent.join("app-data");

    let state = AppState::for_test(requested.clone());

    assert_eq!(
        state.artifacts.app_data_dir,
        requested.canonicalize().unwrap()
    );
    assert!(state
        .artifacts
        .process_lease_dir
        .starts_with(&state.artifacts.app_data_dir));
    assert!(!state
        .artifacts
        .app_data_dir
        .join("finance-snapshots")
        .exists());
}

#[cfg(target_os = "macos")]
#[test]
fn transient_broker_socket_fits_macos_unix_path_limit() {
    use std::os::unix::ffi::OsStrExt;

    let socket = transient_broker_directory().join(format!("{}.sock", "0".repeat(32)));
    assert!(socket.starts_with("/tmp"));
    assert!(socket.as_os_str().as_bytes().len() < 104);
}

#[test]
fn target_sidecar_name_is_never_a_path() {
    let value = target_binary("guruterminal-core");
    assert!(value.starts_with("guruterminal-core"));
    assert!(!value.contains('/'));
    assert!(!value.contains('\\'));
}

#[test]
fn bundled_resource_sidecars_do_not_use_tauri_external_bin_suffixes() {
    let pi = platform_binary("guruterminal-pi");
    let finance = platform_binary("guruterminal-finance");
    assert_eq!(
        pi,
        format!("guruterminal-pi{}", std::env::consts::EXE_SUFFIX)
    );
    assert_eq!(
        finance,
        format!("guruterminal-finance{}", std::env::consts::EXE_SUFFIX)
    );
    assert!(!pi.contains(env::consts::ARCH));
    assert!(!finance.contains(env::consts::ARCH));
}

#[test]
fn local_debug_core_uses_the_platform_executable_suffix() {
    let candidate = local_debug_core_binary(Path::new("manifest"));
    assert_eq!(
        candidate.file_name().and_then(|name| name.to_str()),
        Some(format!("guruterminal-core{}", std::env::consts::EXE_SUFFIX).as_str())
    );
}

#[test]
fn app_data_is_owned_by_only_one_live_instance() {
    let temporary = tempfile::tempdir().unwrap();
    ensure_private_directory(temporary.path()).unwrap();
    let first = AppInstanceLock::acquire(temporary.path()).unwrap();
    let conflict = AppInstanceLock::acquire(temporary.path()).err().unwrap();
    assert!(is_app_instance_conflict(&conflict));
    drop(first);
    AppInstanceLock::acquire(temporary.path()).unwrap();
}

#[test]
fn unrelated_initialization_errors_are_not_instance_conflicts() {
    assert!(!is_app_instance_conflict(&CommandError::internal(
        "database initialization failed"
    )));
    assert!(!is_app_instance_conflict(&CommandError::conflict(
        "an unrelated command conflict"
    )));
}

#[test]
fn clearing_memory_write_recovery_does_not_clear_deletion_quarantine() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().to_path_buf());
    state.quarantine_guru("guru-a", QuarantineSource::Deletion, "pending deletion");
    state.quarantine_guru(
        "guru-a",
        QuarantineSource::MemoryWrite,
        "interrupted memory write",
    );

    state.clear_guru_quarantine("guru-a", QuarantineSource::MemoryWrite);

    assert!(state.is_guru_quarantined("guru-a"));
    assert_eq!(state.guru_access("guru-a"), GuruAccess::Hidden);
    assert_eq!(
        state.ensure_guru_available("guru-a").unwrap_err().code,
        "guru_storage_unavailable"
    );
}

#[test]
fn guru_recovery_admission_bypasses_only_its_authoritative_recovery_state() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().to_path_buf());
    let error = state
        .register_guru_recovery(
            "recovery-none".into(),
            "guru-a".into(),
            GuruRecoveryAction::RecoverMemory,
        )
        .err()
        .unwrap();
    assert_eq!(error.code, "conflict");

    state.quarantine_guru(
        "guru-a",
        QuarantineSource::MemoryWrite,
        "interrupted memory write",
    );

    let recovery = state
        .register_guru_recovery(
            "recovery-a".into(),
            "guru-a".into(),
            GuruRecoveryAction::RecoverMemory,
        )
        .unwrap();
    drop(recovery);
    assert_eq!(
        state.ensure_guru_available("guru-a").unwrap_err().code,
        "guru_recovery_required"
    );

    state.quarantine_guru("guru-a", QuarantineSource::Deletion, "pending deletion");
    let error = state
        .register_guru_recovery(
            "recovery-b".into(),
            "guru-a".into(),
            GuruRecoveryAction::RecoverMemory,
        )
        .unwrap_err();
    assert_eq!(error.code, "guru_storage_unavailable");
}

#[test]
fn guru_availability_has_a_stable_tagged_wire_shape() {
    assert_eq!(
        serde_json::to_value(GuruAvailability::Available).unwrap(),
        serde_json::json!({ "status": "available" })
    );
    assert_eq!(
        serde_json::to_value(GuruAvailability::RecoveryRequired {
            reason: GuruRecoveryReason::InterruptedMemoryUpdate,
            action: GuruRecoveryAction::RecoverMemory,
        })
        .unwrap(),
        serde_json::json!({
            "status": "recovery_required",
            "reason": "interrupted_memory_update",
            "action": "recover_memory",
        })
    );
}

#[cfg(windows)]
#[test]
fn live_instance_lock_cannot_be_replaced() {
    let temporary = tempfile::tempdir().unwrap();
    ensure_private_directory(temporary.path()).unwrap();
    let lock = AppInstanceLock::acquire(temporary.path()).unwrap();
    let lock_path = temporary.path().join("guruterminal.instance.lock");
    let conflict = AppInstanceLock::acquire(temporary.path()).err().unwrap();
    assert!(is_app_instance_conflict(&conflict));
    assert!(std::fs::remove_file(&lock_path).is_err());
    drop(lock);
    std::fs::remove_file(lock_path).unwrap();
}

#[cfg(unix)]
#[test]
fn test_state_hardens_app_data_children_and_lock() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let app_data = temporary.path().join("app");
    let state = AppState::for_test(app_data.clone());
    assert_eq!(
        std::fs::metadata(&app_data).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for child in ["brokers", "pi", "runs", "process-leases"] {
        assert_eq!(
            std::fs::metadata(app_data.join(child))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    assert_eq!(
        std::fs::metadata(app_data.join("guruterminal.instance.lock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(state);
}

#[test]
fn local_debug_profiles_replace_obsolete_app_databases() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("app-data");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("guruterminal.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE sentinel (value TEXT NOT NULL) STRICT;\
             INSERT INTO sentinel (value) VALUES ('preserve-me');\
             PRAGMA user_version = 2;",
        )
        .unwrap();
    drop(connection);

    let (store, fresh) = open_app_store(&path).unwrap();
    assert!(fresh);
    drop(store);

    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        crate::store::STORE_SCHEMA_VERSION
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'sentinel'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}
