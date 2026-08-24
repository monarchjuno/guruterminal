use super::*;

#[cfg(unix)]
#[tokio::test]
async fn managed_guru_creation_owns_storage_and_seeds_free_tools() {
    use std::{collections::BTreeSet, os::unix::fs::PermissionsExt};

    let temporary = tempfile::tempdir().unwrap();
    let runtime_path = temporary.path().join("guruterminal-core-managed-fixture");
    fs::write(
        &runtime_path,
        "#!/bin/sh\nif [ \"$1\" = init ]; then mkdir -p .guruterminal guruterminal/wiki guruterminal/lens guruterminal/evidence guruterminal/decision; printf '{\"schema_version\":1}\\n' > .guruterminal/workspace.json; fi\nprintf '{}\\n'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime_path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&runtime_path, permissions).unwrap();

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let summary = create_managed_guru(&state, "Quality Guru".into(), None)
        .await
        .unwrap();
    let profile = state.store.get_guru(&summary.id).unwrap().unwrap();

    assert_eq!(summary.name, "Quality Guru");
    assert_eq!(profile.storage_kind, GuruStorageKind::Managed);
    assert_eq!(
        Path::new(&profile.memory_root),
        state
            .artifacts
            .app_data_dir
            .canonicalize()
            .unwrap()
            .join("gurus")
            .join(&profile.id)
            .join("workspace")
    );
    assert!(Path::new(&profile.memory_root)
        .join(".guruterminal/workspace.json")
        .is_file());
    let capabilities = state.store.list_guru_capabilities(&profile.id).unwrap();
    let catalog = crate::marketplace::bundled_catalog().unwrap();
    assert_eq!(
        capabilities.len(),
        catalog.entries.len() + agent_harness::default_skill_ids().len()
    );
    let enabled = capabilities
        .iter()
        .filter(|binding| binding.enabled && !binding.entry_id.starts_with("skill."))
        .map(|binding| binding.entry_id.clone())
        .collect::<BTreeSet<_>>();
    let expected_enabled = catalog
        .entries
        .into_iter()
        .filter(|entry| {
            entry.setup.as_ref().is_none_or(|setup| {
                setup.config_fields.iter().all(|field| !field.required)
                    && setup.credential_fields.iter().all(|field| !field.required)
            })
        })
        .map(|entry| entry.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(enabled, expected_enabled);
    assert_eq!(
        summary.enabled_skill_ids,
        agent_harness::default_skill_ids()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn fresh_install_bootstrap_creates_exactly_one_default_agent() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let runtime_path = temporary.path().join("guruterminal-core-bootstrap-fixture");
    fs::write(
        &runtime_path,
        "#!/bin/sh\nif [ \"$1\" = init ]; then mkdir -p .guruterminal guruterminal/wiki guruterminal/lens guruterminal/evidence guruterminal/decision; printf '{\"schema_version\":1}\\n' > .guruterminal/workspace.json; fi\nprintf '{}\\n'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime_path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&runtime_path, permissions).unwrap();

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));

    bootstrap_default_guru(&state).await.unwrap();
    bootstrap_default_guru(&state).await.unwrap();

    let profiles = state.store.list_gurus().unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].name, "My Agent");
}

#[cfg(unix)]
#[test]
fn memory_transfer_copies_only_markdown_without_mutating_source() {
    let temporary = tempfile::tempdir().unwrap();
    let source_path = temporary.path().join("source");
    let destination_path = temporary.path().join("destination");
    initialized_workspace(&source_path, "source");
    initialized_workspace(&destination_path, "destination");
    let markdown = b"---\nid: lens:quality/check\ntitle: Check\n---\n\n# Check\n";
    fs::write(source_path.join("guruterminal/lens/check.md"), markdown).unwrap();
    fs::write(source_path.join("app-only.json"), b"secret app state").unwrap();
    let source = bound_root(&source_path);
    let destination = bound_root(&destination_path);

    let (_, count) = copy_memory_records(&source, &destination).unwrap();

    assert_eq!(count, 1);
    assert_eq!(
        fs::read(source_path.join("guruterminal/lens/check.md")).unwrap(),
        markdown
    );
    assert_eq!(
        fs::read(destination_path.join("guruterminal/lens/check.md")).unwrap(),
        markdown
    );
    assert!(!destination_path.join("app-only.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn managed_guru_import_initializes_and_validates_the_copied_memory() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let source_path = temporary.path().join("import-source");
    initialized_workspace(&source_path, "source");
    let markdown = wiki_markdown("wiki:quality/imported", "Imported discipline");
    fs::write(
        source_path.join("guruterminal/wiki/imported.md"),
        markdown.as_bytes(),
    )
    .unwrap();
    fs::write(source_path.join("app-only.json"), b"private app state").unwrap();

    let runtime_path = temporary.path().join("guruterminal-core-import-fixture");
    fs::write(
        &runtime_path,
        "#!/bin/sh\nif [ \"$1\" = init ]; then mkdir -p .guruterminal guruterminal/wiki guruterminal/lens guruterminal/evidence guruterminal/decision; printf '{\"schema_version\":1}\\n' > .guruterminal/workspace.json; printf '{}\\n'; else printf '{\"valid\":true,\"documents\":1,\"errors\":[]}\\n'; fi\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime_path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&runtime_path, permissions).unwrap();

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let source = bound_root(&source_path);

    let summary = create_managed_guru(&state, "Imported Guru".into(), Some(&source))
        .await
        .unwrap();
    let profile = state.store.get_guru(&summary.id).unwrap().unwrap();
    let imported = Path::new(&profile.memory_root);

    assert_eq!(summary.name, "Imported Guru");
    assert_eq!(
        fs::read(imported.join("guruterminal/wiki/imported.md")).unwrap(),
        markdown.as_bytes()
    );
    assert!(!imported.join("app-only.json").exists());
}

#[tokio::test]
async fn guru_delete_rejects_an_active_run_then_removes_only_its_owned_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let app_data = temporary.path().join("app");
    fs::create_dir(&app_data).unwrap();
    let app_data = app_data.canonicalize().unwrap();
    let state = AppState::for_test(app_data.clone());
    let guru_dir = app_data.join("gurus/guru-delete");
    let workspace = guru_dir.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir(guru_dir.join("workbench")).unwrap();
    seed_profile(state.store.as_ref(), &profile("guru-delete", &workspace, 1));
    let active = state
        .register_run(
            "run-delete".into(),
            "guru-delete".into(),
            RunKind::Chat,
            RunTarget::ChatThread("chat-delete".into()),
        )
        .unwrap();
    let error = delete_guru_inner(&state, "guru-delete").await.unwrap_err();
    assert_eq!(error.code, "conflict");
    assert!(guru_dir.is_dir());
    assert!(state.store.get_guru("guru-delete").unwrap().is_some());

    drop(active);
    delete_guru_inner(&state, "guru-delete").await.unwrap();
    assert!(!guru_dir.exists());
    assert!(state.store.get_guru("guru-delete").unwrap().is_none());
}

#[tokio::test]
async fn update_maintenance_rejects_guru_deletion_before_native_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let _update = state.maintenance.begin_update().unwrap();

    let error = delete_guru_inner(&state, "guru-delete").await.unwrap_err();

    assert_eq!(error.code, "maintenance_active");
}

#[cfg(unix)]
#[test]
fn managed_guru_rejects_a_rebound_root_path() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    fs::create_dir(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let profile = profile("guru-a", &workspace, 1);
    assert_eq!(profile_workspace(&profile).unwrap().path(), workspace);

    let displaced = temporary.path().join("guru-original");
    fs::rename(&workspace, displaced).unwrap();
    fs::create_dir(&workspace).unwrap();

    assert!(profile_workspace(&profile).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn guru_list_omits_a_stale_root_without_blocking_valid_gurus() {
    let temporary = tempfile::tempdir().unwrap();
    let valid_workspace = temporary.path().join("valid-guru");
    let stale_workspace = temporary.path().join("stale-guru");
    fs::create_dir(&valid_workspace).unwrap();
    fs::create_dir(&stale_workspace).unwrap();

    let state = AppState::for_test(temporary.path().join("app"));
    seed_profile(state.store.as_ref(), &profile("valid", &valid_workspace, 1));
    seed_profile(state.store.as_ref(), &profile("stale", &stale_workspace, 2));

    fs::rename(&stale_workspace, temporary.path().join("stale-original")).unwrap();
    fs::create_dir(&stale_workspace).unwrap();

    let summaries = list_available_gurus(&state).await.unwrap();
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary.id.as_str())
            .collect::<Vec<_>>(),
        vec!["valid"]
    );
}

#[test]
fn obsolete_skill_bindings_receive_current_bundled_defaults() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let profile = profile("guru-skills", &workspace, 1);
    seed_profile(state.store.as_ref(), &profile);

    state
        .store
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM guru_capability_bindings
             WHERE guru_id = ?1
               AND entry_id IN ('skill.research', 'skill.wiki', 'skill.lens', 'skill.decision')",
            [&profile.id],
        )
        .unwrap();
    for obsolete in [
        "finance-research",
        "valuation-analysis",
        "thesis-stress-test",
        "comparative-analysis",
        "market-event-explanation",
        "investment-postmortem",
    ] {
        state
            .store
            .save_guru_capability(&crate::domain::GuruCapabilityBinding {
                guru_id: profile.id.clone(),
                entry_id: format!("skill.{obsolete}"),
                enabled: true,
                granted_permissions: vec!["load".into()],
                config: Default::default(),
                updated_at_ms: 2,
            })
            .unwrap();
    }

    assert_eq!(
        enabled_skill_ids(state.store.as_ref(), &profile.id).unwrap(),
        crate::agent_harness::default_skill_ids()
    );
    let entry_ids = state
        .store
        .list_guru_capabilities(&profile.id)
        .unwrap()
        .into_iter()
        .map(|binding| binding.entry_id)
        .collect::<std::collections::BTreeSet<_>>();
    for obsolete in [
        "skill.finance-research",
        "skill.valuation-analysis",
        "skill.thesis-stress-test",
        "skill.comparative-analysis",
        "skill.market-event-explanation",
        "skill.investment-postmortem",
    ] {
        assert!(entry_ids.contains(obsolete), "{obsolete} should remain");
    }
    for current in [
        "skill.research",
        "skill.wiki",
        "skill.lens",
        "skill.decision",
    ] {
        assert!(entry_ids.contains(current), "{current} should be seeded");
    }
}

#[test]
fn existing_current_skill_bindings_are_not_reenabled() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let profile = profile("guru-skills-off", &workspace, 1);
    seed_profile(state.store.as_ref(), &profile);

    for id in crate::agent_harness::default_skill_ids() {
        state
            .store
            .save_guru_capability(&crate::domain::GuruCapabilityBinding {
                guru_id: profile.id.clone(),
                entry_id: crate::agent_harness::skill_binding_id(&id).unwrap(),
                enabled: false,
                granted_permissions: vec!["load".into()],
                config: Default::default(),
                updated_at_ms: 3,
            })
            .unwrap();
    }

    assert!(enabled_skill_ids(state.store.as_ref(), &profile.id)
        .unwrap()
        .is_empty());
}
