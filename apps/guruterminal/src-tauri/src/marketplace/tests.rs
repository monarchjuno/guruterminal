use std::sync::{Arc, Barrier};

use super::{
    connector_config::{configure_connector, valid_setup_value},
    credentials::{
        credential_entry, credential_verification_error, credential_verification_outcome,
        delete_credential_for_state, validate_credential_secrets,
        validate_required_credential_fields,
    },
    *,
};

fn seed_guru(state: &AppState, id: &str) {
    let profile = crate::domain::GuruProfile {
        id: id.to_owned(),
        name: "Credential race Guru".to_owned(),
        description: String::new(),
        storage_kind: crate::domain::GuruStorageKind::Managed,
        memory_root: format!("/tmp/{id}"),
        root_filesystem_identity: None,
        last_model_profile_id: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    state.store.create_guru(&profile).unwrap();
}

fn promote_test_credential(entry_id: &str, secret: &str, timestamp: i64) {
    crate::finance_credentials::stage(
        entry_id,
        &BTreeMap::from([("api_key".to_owned(), secret.to_owned())]),
    )
    .unwrap();
    let candidate = crate::finance_credentials::candidate(entry_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::finance_credentials::finish_verification(
            entry_id,
            candidate.revision(),
            crate::finance_credentials::VerificationOutcome::Verified,
            timestamp,
        )
        .unwrap(),
        crate::finance_credentials::FinishVerification::Applied
    );
}

fn add_test_openbb_runtime(state: &mut AppState, root: &std::path::Path) {
    let catalog = bundled_catalog().unwrap();
    let provider_ids = catalog
        .entries
        .iter()
        .filter(|entry| entry.runtime.server_id.as_deref() == Some("openbb"))
        .flat_map(|entry| entry.runtime.provider_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    let provider_network_hosts = provider_ids
        .iter()
        .map(|provider| {
            (
                provider.clone(),
                BTreeSet::from([format!("{provider}.example.test")]),
            )
        })
        .collect();
    let provider_config_fields = catalog
        .entries
        .iter()
        .filter(|entry| entry.runtime.server_id.as_deref() == Some("openbb"))
        .filter_map(|entry| {
            let [provider_id] = entry.runtime.provider_ids.as_slice() else {
                return None;
            };
            (!entry.runtime.config_mapping.is_empty()).then(|| {
                (
                    provider_id.clone(),
                    entry.runtime.config_mapping.keys().cloned().collect(),
                )
            })
        })
        .collect();
    Arc::get_mut(&mut state.artifacts)
        .unwrap()
        .mcp_runtimes
        .insert(
            "openbb".into(),
            crate::mcp::BundledMcpRuntime {
                server_id: "openbb".into(),
                executable: root.join("guruterminal-openbb"),
                runtime_dir: root.to_path_buf(),
                manifest_path: root.join("runtime-manifest.json"),
                lease_dir: root.join("leases"),
                allowed_categories: vec!["equity".into()],
                provider_ids,
                provider_network_hosts,
                provider_config_fields,
                providerless_tool_policy: Default::default(),
                provider_receipt_pointer: "/structuredContent/provider".into(),
                tool_activation: Some(crate::mcp::McpToolActivation {
                    tool_name: "activate_tools".into(),
                    argument_name: "tool_names".into(),
                }),
                control_tool_names: BTreeSet::from(["activate_tools".into()]),
            },
        );
}

fn zero_setup_catalog_ids() -> BTreeSet<String> {
    bundled_catalog()
        .unwrap()
        .entries
        .into_iter()
        .filter(|entry| {
            entry.setup.as_ref().is_none_or(|setup| {
                setup.config_fields.iter().all(|field| !field.required)
                    && setup.credential_fields.iter().all(|field| !field.required)
            })
        })
        .map(|entry| entry.id)
        .collect()
}

#[test]
fn bundled_catalog_is_strict_unique_and_snapshot_consistent() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let snapshot = marketplace_snapshot_for_state(&state).unwrap();
    validate_snapshot(&snapshot).expect("bundled Marketplace snapshot");
    assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_VERSION);
    assert_eq!(snapshot.catalog.schema_version, CATALOG_SCHEMA_VERSION);
    assert_eq!(
        snapshot
            .sources
            .iter()
            .map(|source| source.id.as_str())
            .collect::<Vec<_>>(),
        ["official", "community", "libraries"]
    );
    assert_eq!(
        snapshot.sources[1].status,
        crate::marketplace::MarketplaceSourceStatus::ComingSoon
    );
    assert!(snapshot
        .plugins
        .iter()
        .any(|plugin| plugin.name == "openbb"));
    assert!(!snapshot.catalog.entries.is_empty());
    assert!(snapshot.catalog.entries.len() <= MAX_CATALOG_ENTRIES);
    let web_research = snapshot
        .catalog
        .entries
        .iter()
        .find(|entry| entry.id == "community.web-research")
        .unwrap();
    let web_research_setup = web_research.setup.as_ref().unwrap();
    assert_eq!(web_research_setup.config_fields.len(), 1);
    assert_eq!(web_research_setup.config_fields[0].id, "search_policy");
    assert!(!web_research_setup.config_fields[0].required);
    assert_eq!(
        web_research_setup.config_fields[0].options,
        ["automatic", "model_only", "exa_only"]
    );
    let catalog_installed_ids = snapshot
        .catalog
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<BTreeSet<_>>();
    let snapshot_installed_ids = snapshot
        .installed
        .iter()
        .map(|entry| entry.entry_id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(snapshot_installed_ids, catalog_installed_ids);

    let entry = |id: &str| {
        snapshot
            .catalog
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .unwrap()
    };
    let openbb = entry("openbb.platform");
    assert_eq!(openbb.plugin, "openbb");
    assert_eq!(openbb.runtime.server_id.as_deref(), Some("openbb"));
    assert!(openbb.runtime.provider_ids.contains(&"yfinance".into()));
    for id in ["sec.edgar", "fred.macro", "alpha-vantage.market-data"] {
        assert_eq!(entry(id).runtime.kind, MarketplaceRuntimeKind::BundledMcp);
        assert_eq!(entry(id).runtime.server_id.as_deref(), Some("openbb"));
    }
    for id in [
        "world-bank.indicators",
        "opendart.disclosures",
        "krx.market-data",
        "koreainvestment.market-data",
    ] {
        assert_eq!(entry(id).runtime.kind, MarketplaceRuntimeKind::Native);
    }
    assert_eq!(
        entry("guruterminal.compute-python")
            .runtime
            .worker_id
            .as_deref(),
        Some("compute")
    );
    assert_eq!(
        entry("guruterminal.finance-core")
            .runtime
            .worker_id
            .as_deref(),
        Some("finance-worker")
    );

    let status_by_id = snapshot
        .connectors
        .iter()
        .map(|connector| (connector.entry_id.as_str(), connector.readiness))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        status_by_id["guruterminal.compute-python"],
        MarketplaceConnectorReadiness::RuntimeUnavailable
    );
    assert_eq!(
        status_by_id["guruterminal.finance-core"],
        MarketplaceConnectorReadiness::RuntimeUnavailable
    );
    assert_eq!(
        status_by_id["openbb.platform"],
        MarketplaceConnectorReadiness::RuntimeUnavailable
    );
    assert_eq!(
        status_by_id["world-bank.indicators"],
        MarketplaceConnectorReadiness::Ready
    );
    assert_eq!(
        status_by_id["community.web-research"],
        MarketplaceConnectorReadiness::Ready
    );
    let web_research_status = snapshot
        .connectors
        .iter()
        .find(|connector| connector.entry_id == "community.web-research")
        .unwrap();
    assert!(web_research_status.config.is_empty());
    assert_eq!(
        web_research_status.config_state,
        MarketplaceConfigState::Valid
    );
    let installed_by_id = snapshot
        .installed
        .iter()
        .map(|installed| (installed.entry_id.as_str(), installed))
        .collect::<BTreeMap<_, _>>();
    for entry_id in [
        "guruterminal.compute-python",
        "guruterminal.finance-core",
        "openbb.platform",
    ] {
        assert!(!installed_by_id[entry_id].configured);
        assert_eq!(installed_by_id[entry_id].health, MarketplaceHealth::Error);
    }
    assert!(installed_by_id["world-bank.indicators"].configured);
    assert_eq!(
        installed_by_id["world-bank.indicators"].health,
        MarketplaceHealth::Ready
    );
}

#[test]
fn staged_openbb_runtime_marks_the_platform_ready_without_sec_setup() {
    let temporary = tempfile::tempdir().unwrap();
    let mut state = AppState::for_test(temporary.path().join("app"));
    add_test_openbb_runtime(&mut state, temporary.path());

    let snapshot = marketplace_snapshot_for_state(&state).unwrap();
    let status_by_id = snapshot
        .connectors
        .iter()
        .map(|connector| (connector.entry_id.as_str(), connector.readiness))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        status_by_id["openbb.platform"],
        MarketplaceConnectorReadiness::Ready
    );
    assert_eq!(
        status_by_id["sec.edgar"],
        MarketplaceConnectorReadiness::NeedsConfiguration
    );
    let installed = snapshot
        .installed
        .iter()
        .find(|entry| entry.entry_id == "openbb.platform")
        .unwrap();
    assert!(installed.configured);
    assert_eq!(installed.health, MarketplaceHealth::Ready);
}

#[test]
fn local_worker_readiness_uses_runtime_identity_after_entry_rename() {
    let temporary = tempfile::tempdir().unwrap();
    let mut state = AppState::for_test(temporary.path().join("app"));
    let artifacts = Arc::get_mut(&mut state.artifacts).unwrap();
    artifacts.finance_executable = Some(temporary.path().join("finance-worker"));
    artifacts.compute = Some(crate::compute::ComputeArtifacts {
        executable: temporary.path().join("compute"),
        runtime_dir: temporary.path().join("compute-runtime"),
        bootstrap: temporary.path().join("bootstrap"),
        lease_dir: temporary.path().join("leases"),
    });

    let catalog = bundled_catalog().unwrap();
    for (original_id, renamed_id) in [
        ("guruterminal.compute-python", "synthetic.compute-entry"),
        ("guruterminal.finance-core", "synthetic.finance-entry"),
    ] {
        let mut entry = catalog
            .entries
            .iter()
            .find(|entry| entry.id == original_id)
            .unwrap()
            .clone();
        entry.id = renamed_id.to_owned();
        assert!(
            runtime_ready(&entry, &state),
            "{renamed_id} should resolve its packaged worker from runtime.worker_id"
        );
    }

    let mut missing_worker = catalog.clone();
    missing_worker
        .entries
        .iter_mut()
        .find(|entry| entry.id == "guruterminal.compute-python")
        .unwrap()
        .runtime
        .worker_id = None;
    assert!(validate_catalog(&missing_worker).is_err());
}

#[test]
fn openbb_catalog_authority_matches_the_bundled_runtime_manifest() {
    let catalog = bundled_catalog().unwrap();
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../openbb/runtime-manifest.json")).unwrap();
    let providers = manifest["providers"].as_array().unwrap();
    let manifest_ids = providers
        .iter()
        .map(|provider| provider["id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    let catalog_ids = catalog
        .entries
        .iter()
        .filter(|entry| entry.runtime.server_id.as_deref() == Some("openbb"))
        .flat_map(|entry| entry.runtime.provider_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    assert_eq!(catalog_ids, manifest_ids);

    let platform = catalog
        .entries
        .iter()
        .find(|entry| entry.id == "openbb.platform")
        .unwrap();
    let keyless_manifest_ids = providers
        .iter()
        .filter(|provider| provider["keyless"].as_bool() == Some(true))
        .map(|provider| provider["id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        platform
            .runtime
            .provider_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>(),
        keyless_manifest_ids
    );
    for provider_id in &platform.runtime.provider_ids {
        let provider = providers
            .iter()
            .find(|provider| provider["id"].as_str() == Some(provider_id))
            .unwrap();
        assert_eq!(provider["keyless"].as_bool(), Some(true));
    }
    for entry in catalog
        .entries
        .iter()
        .filter(|entry| entry.runtime.server_id.as_deref() == Some("openbb"))
    {
        let manifest_hosts = entry
            .runtime
            .provider_ids
            .iter()
            .flat_map(|provider_id| {
                providers
                    .iter()
                    .find(|provider| provider["id"].as_str() == Some(provider_id))
                    .unwrap()["network_hosts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|host| host.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            entry
                .permissions
                .network_hosts
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
            manifest_hosts,
            "catalog network authority must exactly match the OpenBB manifest for {}",
            entry.id
        );
        for provider_id in &entry.runtime.provider_ids {
            let provider = providers
                .iter()
                .find(|provider| provider["id"].as_str() == Some(provider_id))
                .unwrap();
            if entry.id != "openbb.platform" {
                assert_eq!(entry.runtime.provider_ids.len(), 1);
                assert_eq!(
                    entry.runtime.credential_mapping,
                    serde_json::from_value(
                        provider
                            .get("credential_mapping")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                    )
                    .unwrap()
                );
                assert_eq!(
                    entry.runtime.config_mapping,
                    serde_json::from_value(
                        provider
                            .get("config_mapping")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                    )
                    .unwrap()
                );
                let manifest_probe = provider
                    .get("verification_probe")
                    .filter(|value| !value.is_null());
                match (&entry.runtime.verification_probe, manifest_probe) {
                    (Some(catalog_probe), Some(manifest_probe)) => {
                        assert_eq!(catalog_probe.tool_name, manifest_probe["tool"]);
                        assert_eq!(catalog_probe.arguments, manifest_probe["arguments"]);
                    }
                    (None, None) => {}
                    _ => panic!("verification probe mismatch for {provider_id}"),
                }
            }
        }
    }
}

#[test]
fn every_marketplace_entry_reaches_the_agent_runtime_profile() {
    let catalog = bundled_catalog().unwrap();
    let capability_ids = catalog
        .entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    let profile =
        crate::agent_harness::AgentRuntimeProfile::new("chat", false, false, &capability_ids)
            .unwrap();
    for entry in catalog.entries {
        let represented = match entry.runtime.kind {
            MarketplaceRuntimeKind::BundledMcp => profile.components.iter().any(|component| {
                component.kind == "mcp"
                    && component.server_id == entry.runtime.server_id
                    && entry
                        .runtime
                        .provider_ids
                        .iter()
                        .all(|provider| component.provider_ids.contains(provider))
            }),
            MarketplaceRuntimeKind::Native | MarketplaceRuntimeKind::LocalWorker => profile
                .components
                .iter()
                .any(|component| component.provider_ids.contains(&entry.id)),
        };
        assert!(
            represented,
            "Marketplace entry {} has no runtime component",
            entry.id
        );
    }
}

#[test]
fn new_gurus_enable_only_the_bundled_zero_setup_capabilities() {
    let enabled = crate::domain::default_guru_capability_bindings("guru-default", 1)
        .into_iter()
        .filter(|binding| binding.enabled && !binding.entry_id.starts_with("skill."))
        .map(|binding| binding.entry_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(enabled, zero_setup_catalog_ids());
}

#[test]
fn installed_entries_synthesize_disabled_bindings_for_unknown_tools() {
    let installed = vec![MarketplaceInstalledDto {
        entry_id: "future.bundled-tool".to_owned(),
        configured: true,
        health: MarketplaceHealth::Ready,
    }];
    let bindings = bindings_for_installed_entries("guru-existing", installed, Vec::new());
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].guru_id, "guru-existing");
    assert_eq!(bindings[0].entry_id, "future.bundled-tool");
    assert!(!bindings[0].enabled);
    assert!(bindings[0].granted_permissions.is_empty());
}

#[test]
fn synthesizing_missing_bindings_does_not_reenable_stored_disabled_defaults() {
    let stored = vec![crate::domain::GuruCapabilityBinding {
        guru_id: "guru-existing".to_owned(),
        entry_id: "guruterminal.finance-core".to_owned(),
        enabled: false,
        granted_permissions: Vec::new(),
        config: BTreeMap::new(),
        updated_at_ms: 1,
    }];
    let installed = vec![MarketplaceInstalledDto {
        entry_id: "guruterminal.finance-core".to_owned(),
        configured: true,
        health: MarketplaceHealth::Ready,
    }];
    let bindings = bindings_for_installed_entries("guru-existing", installed, stored);
    assert_eq!(bindings.len(), 1);
    assert!(!bindings[0].enabled);
    assert!(bindings[0].granted_permissions.is_empty());
}

#[test]
fn fresh_guru_enables_credential_free_capabilities_and_registers_finance_calculate() {
    use std::sync::Arc;

    let temporary = tempfile::tempdir().unwrap();
    let mut state = AppState::for_test(temporary.path().join("app"));
    let artifacts = Arc::get_mut(&mut state.artifacts).unwrap();
    artifacts.finance_executable = Some(temporary.path().join("finance-worker"));
    artifacts.compute = Some(crate::compute::ComputeArtifacts {
        executable: temporary.path().join("compute"),
        runtime_dir: temporary.path().join("compute-runtime"),
        bootstrap: temporary.path().join("bootstrap"),
        lease_dir: temporary.path().join("leases"),
    });
    add_test_openbb_runtime(&mut state, temporary.path());
    seed_guru(&state, "guru-fresh");

    let bindings = guru_capability_list_for_state("guru-fresh", &state).unwrap();
    let enabled = bindings
        .iter()
        .filter(|binding| binding.enabled)
        .map(|binding| binding.entry_id.as_str())
        .collect::<BTreeSet<_>>();
    for entry_id in zero_setup_catalog_ids() {
        assert!(
            enabled.contains(entry_id.as_str()),
            "{entry_id} should be enabled on a fresh Guru"
        );
        let binding = bindings
            .iter()
            .find(|binding| binding.entry_id == entry_id)
            .unwrap();
        assert!(
            binding.available,
            "{entry_id} should be available for a first Chat turn"
        );
        assert_eq!(binding.granted_permissions, ["execute"]);
    }
    for entry_id in [
        "sec.edgar",
        "opendart.disclosures",
        "krx.market-data",
        "fred.macro",
        "alpha-vantage.market-data",
        "koreainvestment.market-data",
    ] {
        assert!(
            !enabled.contains(entry_id),
            "{entry_id} must stay off until the user enables it"
        );
    }

    let ready = crate::commands::enabled_execute_capability_ids(&state, "guru-fresh").unwrap();
    for entry_id in zero_setup_catalog_ids() {
        assert!(
            ready.iter().any(|id| id == &entry_id),
            "{entry_id} missing from execute set {ready:?}"
        );
    }
    let profile =
        crate::agent_harness::AgentRuntimeProfile::new("chat", false, false, &ready).unwrap();
    assert!(
        profile.components.iter().any(|component| component
            .tool_names
            .iter()
            .any(|name| name == "finance_calculate")),
        "fresh Guru runtime must register finance_calculate"
    );
    let openbb = profile
        .components
        .iter()
        .find(|component| component.id == "mcp/openbb")
        .unwrap();
    assert_eq!(openbb.server_id.as_deref(), Some("openbb"));
    assert!(openbb.tool_names.is_empty());
    assert!(openbb.provider_ids.contains(&"yfinance".into()));
    assert!(profile
        .components
        .iter()
        .any(|component| component.tool_names.iter().any(|name| name == "web_search")));
}

#[test]
fn listing_capabilities_does_not_reenable_a_user_disabled_default() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    seed_guru(&state, "guru-disabled");
    save_binding(
        GuruCapabilityRequest {
            guru_id: "guru-disabled".to_owned(),
            entry_id: "guruterminal.finance-core".to_owned(),
        },
        &state,
        false,
    )
    .unwrap();

    let bindings = guru_capability_list_for_state("guru-disabled", &state).unwrap();
    let finance = bindings
        .iter()
        .find(|binding| binding.entry_id == "guruterminal.finance-core")
        .unwrap();
    assert!(!finance.enabled);
    assert!(
        !crate::commands::enabled_execute_capability_ids(&state, "guru-disabled")
            .unwrap()
            .iter()
            .any(|id| id == "guruterminal.finance-core")
    );
}

#[test]
fn kis_credential_patch_accepts_optional_profile_fields_but_requires_app_keys_overall() {
    let catalog = bundled_catalog().unwrap();
    let entry = credential_entry(&catalog, "koreainvestment.market-data").unwrap();
    let complete = BTreeMap::from([
        ("app_key".to_owned(), "test-kis-app-key".to_owned()),
        ("app_secret".to_owned(), "test-kis-app-secret".to_owned()),
    ]);
    assert!(validate_credential_secrets(entry, &complete).is_ok());
    let partial = BTreeMap::from([("account_number".to_owned(), "12345678".to_owned())]);
    assert!(validate_credential_secrets(entry, &partial).is_ok());
    let missing = validate_required_credential_fields(
        entry,
        &partial,
        &crate::finance_credentials::CredentialStatus {
            stored: false,
            active: false,
            pending: false,
            active_fields: BTreeSet::new(),
            candidate_fields: BTreeSet::new(),
            verification: crate::finance_credentials::CredentialVerification::Never,
            verified_at: None,
        },
    )
    .unwrap_err();
    assert_eq!(missing.message, "KIS app key is required.");
    let existing = crate::finance_credentials::CredentialStatus {
        stored: true,
        active: true,
        pending: false,
        active_fields: BTreeSet::from(["app_key".to_owned(), "app_secret".to_owned()]),
        candidate_fields: BTreeSet::new(),
        verification: crate::finance_credentials::CredentialVerification::Verified,
        verified_at: Some(1),
    };
    assert!(validate_required_credential_fields(entry, &partial, &existing).is_ok());
    let unexpected = validate_credential_secrets(
        entry,
        &BTreeMap::from([
            ("app_key".to_owned(), "test-kis-app-key".to_owned()),
            ("app_secret".to_owned(), "test-kis-app-secret".to_owned()),
            ("unexpected".to_owned(), "test-extra-secret".to_owned()),
        ]),
    )
    .unwrap_err();
    assert_eq!(
        unexpected.message,
        "Enter at least one declared credential or profile field."
    );
    let whitespace = validate_credential_secrets(
        entry,
        &BTreeMap::from([
            ("app_key".to_owned(), "test kis app key".to_owned()),
            ("app_secret".to_owned(), "test-kis-app-secret".to_owned()),
        ]),
    )
    .unwrap_err();
    assert_eq!(whitespace.message, "KIS app key cannot contain whitespace.");
    let non_numeric = validate_credential_secrets(
        entry,
        &BTreeMap::from([("account_number".to_owned(), "1234ABCD".to_owned())]),
    )
    .unwrap_err();
    assert_eq!(
        non_numeric.message,
        "KIS account number must contain exactly 8 digits."
    );
}

#[test]
fn kis_environment_is_an_exact_non_secret_select_allowlist() {
    let mut catalog = bundled_catalog().unwrap();
    let entry = catalog
        .entries
        .iter_mut()
        .find(|entry| entry.id == crate::finance_data::KIS_SOURCE_ID)
        .unwrap();
    let setup = entry.setup.as_mut().unwrap();
    assert_eq!(setup.config_fields.len(), 1);
    let environment = &setup.config_fields[0];
    assert_eq!(environment.id, "environment");
    assert_eq!(environment.kind, MarketplaceSetupFieldKind::Select);
    assert_eq!(environment.options, ["real", "demo"]);
    assert!(environment.required);
    assert!(valid_setup_value(environment, "real"));
    assert!(valid_setup_value(environment, "demo"));
    assert!(!valid_setup_value(environment, "prod"));
    assert_eq!(setup.credential_scope_fields, ["environment"]);

    assert_eq!(setup.credential_fields.len(), 5);
    assert_eq!(setup.credential_fields[0].id, "app_key");
    assert!(setup.credential_fields[0].required);
    assert_eq!(setup.credential_fields[1].id, "app_secret");
    assert!(setup.credential_fields[1].required);
    assert_eq!(setup.credential_fields[2].id, "account_number");
    assert!(!setup.credential_fields[2].required);
    assert_eq!(
        (
            setup.credential_fields[2].min_length,
            setup.credential_fields[2].max_length,
        ),
        (8, 8)
    );
    assert_eq!(setup.credential_fields[3].id, "account_product_code");
    assert!(!setup.credential_fields[3].required);
    assert_eq!(
        (
            setup.credential_fields[3].min_length,
            setup.credential_fields[3].max_length,
        ),
        (2, 2)
    );
    assert_eq!(setup.credential_fields[4].id, "hts_id");
    assert!(!setup.credential_fields[4].required);

    setup.config_fields[0].options[1] = "paper".to_owned();
    assert!(validate_catalog(&catalog).is_err());
}

#[test]
fn changing_kis_environment_disables_bindings_and_deletes_credentials() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let entry_id = crate::finance_data::KIS_SOURCE_ID;
    let guru_id = "guru-kis-environment-change";
    crate::finance_credentials::delete(entry_id).unwrap();
    seed_guru(&state, guru_id);

    configure_connector(
        MarketplaceConnectorConfigureRequest {
            entry_id: entry_id.to_owned(),
            config: BTreeMap::from([("environment".to_owned(), "real".to_owned())]),
        },
        &state,
    )
    .unwrap();
    crate::finance_credentials::stage(
        entry_id,
        &BTreeMap::from([
            ("app_key".to_owned(), "test-kis-app-key".to_owned()),
            ("app_secret".to_owned(), "test-kis-app-secret".to_owned()),
        ]),
    )
    .unwrap();
    let candidate = crate::finance_credentials::candidate(entry_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::finance_credentials::finish_verification(
            entry_id,
            candidate.revision(),
            crate::finance_credentials::VerificationOutcome::Verified,
            1,
        )
        .unwrap(),
        crate::finance_credentials::FinishVerification::Applied
    );
    save_binding(
        GuruCapabilityRequest {
            guru_id: guru_id.to_owned(),
            entry_id: entry_id.to_owned(),
        },
        &state,
        true,
    )
    .unwrap();

    let status = configure_connector(
        MarketplaceConnectorConfigureRequest {
            entry_id: entry_id.to_owned(),
            config: BTreeMap::from([("environment".to_owned(), "demo".to_owned())]),
        },
        &state,
    )
    .unwrap();
    assert_eq!(
        status.config.get("environment").map(String::as_str),
        Some("demo")
    );
    assert_eq!(
        status.readiness,
        MarketplaceConnectorReadiness::NeedsConfiguration
    );
    assert!(!crate::finance_credentials::has_active(entry_id).unwrap());
    let binding = state
        .store
        .get_guru_capability(guru_id, entry_id)
        .unwrap()
        .unwrap();
    assert!(!binding.enabled);
    assert!(binding.granted_permissions.is_empty());

    let error = configure_connector(
        MarketplaceConnectorConfigureRequest {
            entry_id: entry_id.to_owned(),
            config: BTreeMap::from([("environment".to_owned(), "prod".to_owned())]),
        },
        &state,
    )
    .unwrap_err();
    assert_eq!(error.code, "invalid_request");
    crate::finance_credentials::delete(entry_id).unwrap();
}

#[test]
fn credential_verification_errors_are_redacted_and_classified() {
    let rejected = credential_verification_outcome(&Err(
        crate::finance_data::FinanceDataError::CredentialRejected("secret-provider"),
    ));
    assert_eq!(
        rejected,
        crate::finance_credentials::VerificationOutcome::Rejected
    );

    let unavailable = credential_verification_outcome(&Err(
        crate::finance_data::FinanceDataError::NetworkSafe("secret-provider"),
    ));
    assert_eq!(
        unavailable,
        crate::finance_credentials::VerificationOutcome::TemporarilyUnavailable
    );

    let detailed = credential_verification_error(
        &crate::finance_data::FinanceDataError::KisCredentialRejected(
            crate::finance_data::SafeProviderDiagnostic::for_test(
                "EGW00123",
                "The app key and secret do not match.",
            ),
        ),
    );
    assert_eq!(detailed.code, "credential_rejected");
    assert_eq!(
        detailed.message,
        "KIS error · EGW00123: The app key and secret do not match."
    );
}

#[test]
fn update_maintenance_rejects_connector_and_credential_mutations() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let _update = state.maintenance.begin_update().unwrap();

    let config_error = configure_connector(
        MarketplaceConnectorConfigureRequest {
            entry_id: "sec.edgar".to_owned(),
            config: BTreeMap::from([(
                "contact_email".to_owned(),
                "research@example.com".to_owned(),
            )]),
        },
        &state,
    )
    .unwrap_err();
    let credential_error = delete_credential_for_state(
        MarketplaceCredentialRequest {
            entry_id: "krx.market-data".to_owned(),
        },
        &state,
    )
    .unwrap_err();

    assert_eq!(config_error.code, "maintenance_active");
    assert_eq!(credential_error.code, "maintenance_active");
}

#[test]
fn global_connector_configuration_survives_state_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let app_data = temporary.path().join("app");
    let state = AppState::for_test(app_data.clone());
    configure_connector(
        MarketplaceConnectorConfigureRequest {
            entry_id: "sec.edgar".to_owned(),
            config: BTreeMap::from([(
                "contact_email".to_owned(),
                "research@example.com".to_owned(),
            )]),
        },
        &state,
    )
    .unwrap();
    drop(state);

    let restarted = AppState::for_test(app_data);
    assert_eq!(
        connector_config_value(&restarted, "sec.edgar", "contact_email").unwrap(),
        Some("research@example.com".to_owned())
    );
}

#[test]
fn web_research_policy_defaults_persists_and_does_not_change_guru_binding() {
    let temporary = tempfile::tempdir().unwrap();
    let app_data = temporary.path().join("app");
    let state = AppState::for_test(app_data.clone());
    let guru_id = "guru-web-research-policy";
    seed_guru(&state, guru_id);
    assert_eq!(
        web_research_policy(&state).unwrap(),
        crate::web::WebSearchPolicy::Automatic
    );
    save_binding(
        GuruCapabilityRequest {
            guru_id: guru_id.to_owned(),
            entry_id: "community.web-research".to_owned(),
        },
        &state,
        true,
    )
    .unwrap();

    let status = configure_connector(
        MarketplaceConnectorConfigureRequest {
            entry_id: "community.web-research".to_owned(),
            config: BTreeMap::from([("search_policy".to_owned(), "model_only".to_owned())]),
        },
        &state,
    )
    .unwrap();
    assert_eq!(status.readiness, MarketplaceConnectorReadiness::Ready);
    assert_eq!(
        web_research_policy(&state).unwrap(),
        crate::web::WebSearchPolicy::ModelOnly
    );
    assert!(
        state
            .store
            .get_guru_capability(guru_id, "community.web-research")
            .unwrap()
            .unwrap()
            .enabled
    );
    drop(state);

    let restarted = AppState::for_test(app_data);
    assert_eq!(
        web_research_policy(&restarted).unwrap(),
        crate::web::WebSearchPolicy::ModelOnly
    );
}

#[test]
fn pending_secret_is_not_ready_and_delete_disables_run_capture() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let guru_id = "guru-credential-capture";
    let second_guru_id = "guru-credential-capture-two";
    let entry_id = "krx.market-data";
    crate::finance_credentials::delete(entry_id).unwrap();
    seed_guru(&state, guru_id);
    seed_guru(&state, second_guru_id);

    crate::finance_credentials::stage(
        entry_id,
        &BTreeMap::from([("api_key".to_owned(), "pending-krx-secret".to_owned())]),
    )
    .unwrap();
    assert!(save_binding(
        GuruCapabilityRequest {
            guru_id: guru_id.to_owned(),
            entry_id: entry_id.to_owned(),
        },
        &state,
        true,
    )
    .is_err());
    assert!(
        !crate::commands::enabled_execute_capability_ids(&state, guru_id)
            .unwrap()
            .contains(&entry_id.to_owned())
    );

    let pending = crate::finance_credentials::candidate(entry_id)
        .unwrap()
        .unwrap();
    crate::finance_credentials::finish_verification(
        entry_id,
        pending.revision(),
        crate::finance_credentials::VerificationOutcome::Verified,
        10,
    )
    .unwrap();
    save_binding(
        GuruCapabilityRequest {
            guru_id: guru_id.to_owned(),
            entry_id: entry_id.to_owned(),
        },
        &state,
        true,
    )
    .unwrap();
    save_binding(
        GuruCapabilityRequest {
            guru_id: second_guru_id.to_owned(),
            entry_id: entry_id.to_owned(),
        },
        &state,
        true,
    )
    .unwrap();
    assert!(
        crate::commands::enabled_execute_capability_ids(&state, guru_id)
            .unwrap()
            .contains(&entry_id.to_owned())
    );

    delete_credential_for_state(
        MarketplaceCredentialRequest {
            entry_id: entry_id.to_owned(),
        },
        &state,
    )
    .unwrap();
    let binding = state
        .store
        .get_guru_capability(guru_id, entry_id)
        .unwrap()
        .unwrap();
    assert!(!binding.enabled);
    assert!(binding.granted_permissions.is_empty());
    let second_binding = state
        .store
        .get_guru_capability(second_guru_id, entry_id)
        .unwrap()
        .unwrap();
    assert!(!second_binding.enabled);
    assert!(second_binding.granted_permissions.is_empty());
    assert!(
        !crate::commands::enabled_execute_capability_ids(&state, guru_id)
            .unwrap()
            .contains(&entry_id.to_owned())
    );
    assert!(crate::finance_credentials::get(entry_id).unwrap().is_none());
}

#[test]
fn concurrent_enable_and_delete_never_leave_enabled_without_an_active_key() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let guru_id = "guru-credential-lifecycle";
    let entry_id = "alpha-vantage.market-data";
    crate::finance_credentials::delete(entry_id).unwrap();
    seed_guru(&state, guru_id);

    for iteration in 0..12 {
        promote_test_credential(
            entry_id,
            &format!("active-secret-{iteration:02}"),
            100 + iteration,
        );
        let barrier = Arc::new(Barrier::new(3));
        let enable_state = state.clone();
        let enable_barrier = barrier.clone();
        let enable = std::thread::spawn(move || {
            enable_barrier.wait();
            save_binding(
                GuruCapabilityRequest {
                    guru_id: guru_id.to_owned(),
                    entry_id: entry_id.to_owned(),
                },
                &enable_state,
                true,
            )
        });
        let delete_state = state.clone();
        let delete_barrier = barrier.clone();
        let delete = std::thread::spawn(move || {
            delete_barrier.wait();
            delete_credential_for_state(
                MarketplaceCredentialRequest {
                    entry_id: entry_id.to_owned(),
                },
                &delete_state,
            )
        });
        barrier.wait();
        let _ = enable.join().unwrap();
        delete.join().unwrap().unwrap();

        let binding = state
            .store
            .get_guru_capability(guru_id, entry_id)
            .unwrap()
            .unwrap();
        assert!(!binding.enabled);
        assert!(binding.granted_permissions.is_empty());
        assert!(!crate::finance_credentials::has_active(entry_id).unwrap());
        assert!(
            !crate::commands::enabled_execute_capability_ids(&state, guru_id)
                .unwrap()
                .contains(&entry_id.to_owned())
        );
    }
}

#[test]
fn concurrent_configure_and_disable_never_reenable_a_stale_binding() {
    let temporary = tempfile::tempdir().unwrap();
    let mut state = AppState::for_test(temporary.path().join("app"));
    add_test_openbb_runtime(&mut state, temporary.path());
    let guru_id = "guru-configure-lifecycle";
    let entry_id = "sec.edgar";
    seed_guru(&state, guru_id);

    configure_connector(
        MarketplaceConnectorConfigureRequest {
            entry_id: entry_id.to_owned(),
            config: BTreeMap::from([(
                "contact_email".to_owned(),
                "research@example.com".to_owned(),
            )]),
        },
        &state,
    )
    .unwrap();

    for iteration in 0..12 {
        save_binding(
            GuruCapabilityRequest {
                guru_id: guru_id.to_owned(),
                entry_id: entry_id.to_owned(),
            },
            &state,
            true,
        )
        .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let configure_state = state.clone();
        let configure_barrier = barrier.clone();
        let configure = std::thread::spawn(move || {
            configure_barrier.wait();
            configure_connector(
                MarketplaceConnectorConfigureRequest {
                    entry_id: entry_id.to_owned(),
                    config: BTreeMap::from([(
                        "contact_email".to_owned(),
                        format!("research-{iteration:02}@example.com"),
                    )]),
                },
                &configure_state,
            )
        });
        let disable_state = state.clone();
        let disable_barrier = barrier.clone();
        let disable = std::thread::spawn(move || {
            disable_barrier.wait();
            save_binding(
                GuruCapabilityRequest {
                    guru_id: guru_id.to_owned(),
                    entry_id: entry_id.to_owned(),
                },
                &disable_state,
                false,
            )
        });
        barrier.wait();
        configure.join().unwrap().unwrap();
        disable.join().unwrap().unwrap();

        let binding = state
            .store
            .get_guru_capability(guru_id, entry_id)
            .unwrap()
            .unwrap();
        assert!(!binding.enabled);
        assert!(binding.granted_permissions.is_empty());
    }
}
