mod catalog;
pub(crate) mod connector_config;
pub(crate) mod credentials;

pub use catalog::*;
pub use connector_config::marketplace_connector_configure;
use connector_config::{config_state, read_connector_config};
pub(crate) use connector_config::{connector_config_value, web_research_policy};
use credentials::credential_statuses;
pub use credentials::{
    marketplace_credential_delete, marketplace_credential_save, marketplace_credential_verify,
};

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::Utc;
use tauri::State;

use crate::{
    app::{AppState, CommandError},
    domain::GuruCapabilityBinding,
    maintenance::MaintenanceActivityKind,
    store::GuruTerminalStore,
};

fn connector_lifecycle_lock() -> Result<MutexGuard<'static, ()>, CommandError> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(Default::default)
        .lock()
        .map_err(|_| CommandError::internal("connector lifecycle lock was poisoned"))
}

pub(crate) fn with_connector_lifecycle<T>(
    operation: impl FnOnce() -> Result<T, CommandError>,
) -> Result<T, CommandError> {
    let _guard = connector_lifecycle_lock()?;
    operation()
}

#[tauri::command(rename_all = "snake_case")]
pub fn marketplace_snapshot(
    state: State<'_, AppState>,
) -> Result<MarketplaceSnapshotDto, CommandError> {
    with_connector_lifecycle(|| marketplace_snapshot_for_state(&state))
}

#[tauri::command(rename_all = "snake_case")]
pub fn guru_capability_list(
    guru_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<GuruCapabilityBindingDto>, CommandError> {
    state.ensure_guru_available(&guru_id)?;
    with_connector_lifecycle(|| guru_capability_list_for_state(&guru_id, &state))
}

fn installed_entries(
    catalog: &MarketplaceCatalogDto,
    connectors: &[MarketplaceConnectorStatusDto],
) -> Vec<MarketplaceInstalledDto> {
    catalog
        .entries
        .iter()
        .map(|entry| {
            let entry_id = entry.id.as_str();
            let readiness = connectors
                .iter()
                .find(|connector| connector.entry_id == entry_id)
                .map(|connector| connector.readiness);
            let configured = readiness == Some(MarketplaceConnectorReadiness::Ready);
            MarketplaceInstalledDto {
                entry_id: entry.id.clone(),
                configured,
                health: match readiness {
                    Some(MarketplaceConnectorReadiness::Ready) => MarketplaceHealth::Ready,
                    Some(MarketplaceConnectorReadiness::RuntimeUnavailable) => {
                        MarketplaceHealth::Error
                    }
                    _ => MarketplaceHealth::NeedsConfiguration,
                },
            }
        })
        .collect()
}

fn marketplace_snapshot_for_state(
    state: &AppState,
) -> Result<MarketplaceSnapshotDto, CommandError> {
    let bundled = bundled_marketplace()?;
    let catalog = bundled.catalog.clone();
    let connectors = catalog
        .entries
        .iter()
        .map(|entry| connector_status(entry, state))
        .collect::<Result<Vec<_>, _>>()?;
    let installed = installed_entries(&catalog, &connectors);
    let installed_ids = installed
        .iter()
        .map(|entry| entry.entry_id.as_str())
        .collect::<BTreeSet<_>>();
    let connectors = connectors
        .into_iter()
        .filter(|connector| installed_ids.contains(connector.entry_id.as_str()))
        .collect();
    let snapshot = MarketplaceSnapshotDto {
        schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
        sources: marketplace_sources(&bundled.official_display_name),
        plugins: bundled.plugins.clone(),
        installed,
        connectors,
        catalog,
    };
    validate_snapshot(&snapshot).map_err(|message| {
        CommandError::internal(format!("Marketplace snapshot is invalid: {message}"))
    })?;
    Ok(snapshot)
}

fn guru_capability_list_for_state(
    guru_id: &str,
    state: &AppState,
) -> Result<Vec<GuruCapabilityBindingDto>, CommandError> {
    let catalog = bundled_catalog()?;
    let installed = installed_entries(
        &catalog,
        &catalog
            .entries
            .iter()
            .map(|entry| connector_status(entry, state))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let stored = state
        .store
        .list_guru_capabilities(guru_id)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    bindings_for_installed_entries(guru_id, installed, stored)
        .into_iter()
        .map(|binding| binding_dto(binding, &catalog, state))
        .collect()
}

fn bindings_for_installed_entries(
    guru_id: &str,
    installed: Vec<MarketplaceInstalledDto>,
    stored: Vec<GuruCapabilityBinding>,
) -> Vec<GuruCapabilityBinding> {
    let mut stored = stored
        .into_iter()
        .map(|binding| (binding.entry_id.clone(), binding))
        .collect::<BTreeMap<_, _>>();
    installed
        .into_iter()
        .map(|entry| {
            stored
                .remove(&entry.entry_id)
                .unwrap_or(GuruCapabilityBinding {
                    guru_id: guru_id.to_owned(),
                    entry_id: entry.entry_id,
                    enabled: false,
                    granted_permissions: Vec::new(),
                    config: BTreeMap::new(),
                    updated_at_ms: 0,
                })
        })
        .collect()
}

fn binding_dto(
    binding: GuruCapabilityBinding,
    catalog: &MarketplaceCatalogDto,
    state: &AppState,
) -> Result<GuruCapabilityBindingDto, CommandError> {
    let entry = catalog
        .entries
        .iter()
        .find(|entry| entry.id == binding.entry_id)
        .ok_or_else(|| CommandError::internal("capability is absent from the catalog"))?;
    let available = connector_ready(entry, state)?;
    Ok(GuruCapabilityBindingDto {
        entry_id: binding.entry_id,
        enabled: binding.enabled,
        granted_permissions: binding.granted_permissions,
        available,
    })
}

fn connector_status(
    entry: &MarketplaceEntryDto,
    state: &AppState,
) -> Result<MarketplaceConnectorStatusDto, CommandError> {
    let config = read_connector_config(state, &entry.id)?;
    let config_state = config_state(entry, &config);
    let credentials = credential_statuses(entry)?;
    let credentials_ready = entry
        .setup
        .as_ref()
        .map(|setup| {
            setup.credential_fields.iter().all(|field| {
                !field.required
                    || credentials
                        .iter()
                        .any(|credential| credential.credential_id == field.id && credential.active)
            })
        })
        .unwrap_or(true);
    let configuration_ready = !matches!(config_state, MarketplaceConfigState::Missing);
    let runtime_ready = runtime_ready(entry, state);
    let readiness = if !runtime_ready {
        MarketplaceConnectorReadiness::RuntimeUnavailable
    } else if configuration_ready && credentials_ready {
        MarketplaceConnectorReadiness::Ready
    } else {
        MarketplaceConnectorReadiness::NeedsConfiguration
    };
    Ok(MarketplaceConnectorStatusDto {
        entry_id: entry.id.clone(),
        config,
        config_state,
        credentials,
        readiness,
    })
}

fn runtime_ready(entry: &MarketplaceEntryDto, state: &AppState) -> bool {
    match entry.runtime.kind {
        MarketplaceRuntimeKind::BundledMcp => entry
            .runtime
            .server_id
            .as_deref()
            .is_some_and(|id| state.artifacts.mcp_runtimes.contains_key(id)),
        MarketplaceRuntimeKind::LocalWorker => match entry.runtime.worker_id.as_deref() {
            Some("compute") => state.artifacts.compute.is_some(),
            Some("finance-worker") => state.artifacts.finance_executable.is_some(),
            _ => false,
        },
        MarketplaceRuntimeKind::Native => true,
    }
}

fn connector_ready(entry: &MarketplaceEntryDto, state: &AppState) -> Result<bool, CommandError> {
    Ok(connector_status(entry, state)?.readiness == MarketplaceConnectorReadiness::Ready)
}

/// Rechecks mutable connector setup at the moment a run captures authority.
/// Stored-but-unverified candidates never make a capability executable.
pub(crate) fn execute_binding_ready(
    binding: &GuruCapabilityBinding,
    state: &AppState,
) -> Result<bool, CommandError> {
    let catalog = bundled_catalog()?;
    let Some(entry) = catalog
        .entries
        .iter()
        .find(|entry| entry.id == binding.entry_id)
    else {
        return Ok(false);
    };
    connector_ready(entry, state)
}

fn ensure_installable(entry_id: &str) -> Result<(), CommandError> {
    let catalog = bundled_catalog()?;
    if !catalog.entries.iter().any(|entry| entry.id == entry_id) {
        return Err(CommandError::not_found("Marketplace entry"));
    }
    Ok(())
}

fn save_binding(
    request: GuruCapabilityRequest,
    state: &AppState,
    enabled: bool,
) -> Result<GuruCapabilityBindingDto, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::MarketplaceConfiguration)?;
    state.ensure_guru_available(&request.guru_id)?;
    ensure_installable(&request.entry_id)?;
    let _lifecycle_guard = connector_lifecycle_lock()?;
    let catalog = bundled_catalog()?;
    let existing = state
        .store
        .get_guru_capability(&request.guru_id, &request.entry_id)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    let binding = GuruCapabilityBinding {
        guru_id: request.guru_id,
        entry_id: request.entry_id,
        enabled,
        granted_permissions: if enabled {
            vec!["execute".to_owned()]
        } else {
            Vec::new()
        },
        config: existing.map(|binding| binding.config).unwrap_or_default(),
        updated_at_ms: Utc::now().timestamp_millis(),
    };
    if enabled && !binding_dto(binding.clone(), &catalog, state)?.available {
        return Err(CommandError::conflict(
            "Marketplace connector must be configured before it is enabled",
        ));
    }
    state
        .store
        .save_guru_capability(&binding)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    binding_dto(binding, &catalog, state)
}

#[tauri::command(rename_all = "snake_case")]
pub fn guru_capability_enable(
    request: GuruCapabilityRequest,
    state: State<'_, AppState>,
) -> Result<GuruCapabilityBindingDto, CommandError> {
    save_binding(request, &state, true)
}

#[tauri::command(rename_all = "snake_case")]
pub fn guru_capability_disable(
    request: GuruCapabilityRequest,
    state: State<'_, AppState>,
) -> Result<GuruCapabilityBindingDto, CommandError> {
    save_binding(request, &state, false)
}

#[cfg(test)]
mod tests;
