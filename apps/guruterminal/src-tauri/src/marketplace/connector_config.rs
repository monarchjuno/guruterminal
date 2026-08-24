use std::collections::BTreeMap;
use std::fs;
use std::io::Write;

use tauri::State;

use crate::{
    app::{AppState, CommandError},
    artifact_trust::{
        ensure_private_directory, ensure_private_regular_file, read_private_regular_file_bounded,
    },
    maintenance::MaintenanceActivityKind,
};

use super::{
    catalog::{
        bundled_catalog, MarketplaceConfigState, MarketplaceConnectorConfigureRequest,
        MarketplaceConnectorStatusDto, MarketplaceEntryDto, MarketplaceSetupFieldDto,
        MarketplaceSetupFieldKind,
    },
    connector_lifecycle_lock, connector_status,
    credentials::disable_bindings_and_delete_credentials_locked,
    ensure_installable,
};

pub(super) const MAX_CONNECTOR_CONFIG_BYTES: usize = 16 * 1024;
const WEB_RESEARCH_ID: &str = "community.web-research";
const WEB_RESEARCH_POLICY_FIELD: &str = "search_policy";

pub(super) fn config_state(
    entry: &MarketplaceEntryDto,
    config: &BTreeMap<String, String>,
) -> MarketplaceConfigState {
    let fields = entry
        .setup
        .as_ref()
        .map(|setup| setup.config_fields.as_slice())
        .unwrap_or_default();
    if fields.is_empty() {
        return MarketplaceConfigState::NotRequired;
    }
    let has_unknown_field = config
        .keys()
        .any(|key| !fields.iter().any(|field| field.id == *key));
    let has_missing_or_invalid_field = fields.iter().any(|field| match config.get(&field.id) {
        Some(value) => !valid_setup_value(field, value),
        None => field.required,
    });
    if has_unknown_field || has_missing_or_invalid_field {
        MarketplaceConfigState::Missing
    } else {
        MarketplaceConfigState::Valid
    }
}

pub(super) fn connector_config_path(
    state: &AppState,
    entry_id: &str,
) -> Result<std::path::PathBuf, CommandError> {
    ensure_installable(entry_id)?;
    ensure_private_directory(&state.artifacts.connector_config_dir)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    Ok(state
        .artifacts
        .connector_config_dir
        .join(format!("{entry_id}.json")))
}

pub(super) fn read_connector_config(
    state: &AppState,
    entry_id: &str,
) -> Result<BTreeMap<String, String>, CommandError> {
    let path = connector_config_path(state, entry_id)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(CommandError::internal(error.to_string())),
        Ok(_) => {}
    }
    let bytes = read_private_regular_file_bounded(&path, MAX_CONNECTOR_CONFIG_BYTES as u64)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        CommandError::internal(format!("connector configuration is invalid: {error}"))
    })
}

pub(super) fn write_connector_config(
    state: &AppState,
    entry_id: &str,
    config: &BTreeMap<String, String>,
) -> Result<(), CommandError> {
    let path = connector_config_path(state, entry_id)?;
    let bytes =
        serde_json::to_vec(config).map_err(|error| CommandError::internal(error.to_string()))?;
    if bytes.len() > MAX_CONNECTOR_CONFIG_BYTES {
        return Err(CommandError::invalid(
            "connector configuration is too large",
        ));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&state.artifacts.connector_config_dir)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    temporary
        .as_file_mut()
        .write_all(&bytes)
        .and_then(|()| temporary.as_file_mut().sync_all())
        .map_err(|error| CommandError::internal(error.to_string()))?;
    temporary
        .persist(&path)
        .map_err(|error| CommandError::internal(error.error.to_string()))?;
    ensure_private_regular_file(&path).map_err(|error| CommandError::internal(error.to_string()))
}

pub(crate) fn connector_config_value(
    state: &AppState,
    entry_id: &str,
    field_id: &str,
) -> Result<Option<String>, CommandError> {
    Ok(read_connector_config(state, entry_id)?.remove(field_id))
}

pub(crate) fn web_research_policy(
    state: &AppState,
) -> Result<crate::web::WebSearchPolicy, CommandError> {
    let config = read_connector_config(state, WEB_RESEARCH_ID)?;
    crate::web::WebSearchPolicy::from_config_value(
        config.get(WEB_RESEARCH_POLICY_FIELD).map(String::as_str),
    )
    .ok_or_else(|| CommandError::internal("Web Research routing configuration is invalid"))
}

pub(super) fn valid_setup_value(field: &MarketplaceSetupFieldDto, value: &str) -> bool {
    if !(field.min_length..=field.max_length).contains(&value.len())
        || value.contains('\0')
        || value.chars().any(char::is_control)
    {
        return false;
    }
    match field.kind {
        MarketplaceSetupFieldKind::ApiKey => !value.chars().any(char::is_whitespace),
        MarketplaceSetupFieldKind::Email => {
            let mut parts = value.split('@');
            parts.next().is_some_and(|part| !part.is_empty())
                && parts.next().is_some_and(|part| part.contains('.'))
                && parts.next().is_none()
                && !value.chars().any(char::is_whitespace)
        }
        MarketplaceSetupFieldKind::Select => field.options.iter().any(|option| option == value),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn marketplace_connector_configure(
    request: MarketplaceConnectorConfigureRequest,
    state: State<'_, AppState>,
) -> Result<MarketplaceConnectorStatusDto, CommandError> {
    let entry_id = request.entry_id.clone();
    let result = configure_connector(request, &state);
    // Configuration changes finish their keyring/SQLite lifecycle before the
    // async cache clear. Clear even if a later config write failed after
    // credential invalidation; clearing on an unchanged or rejected KIS
    // selection is harmless.
    if entry_id == crate::finance_data::KIS_SOURCE_ID {
        state.finance_data.clear_kis_token_cache().await;
    }
    result
}

pub(crate) fn configure_connector(
    request: MarketplaceConnectorConfigureRequest,
    state: &AppState,
) -> Result<MarketplaceConnectorStatusDto, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::MarketplaceConfiguration)?;
    ensure_installable(&request.entry_id)?;
    let _lifecycle_guard = connector_lifecycle_lock()?;
    let catalog = bundled_catalog()?;
    let entry = catalog
        .entries
        .iter()
        .find(|entry| entry.id == request.entry_id)
        .ok_or_else(|| CommandError::not_found("Marketplace entry"))?;
    let fields = entry
        .setup
        .as_ref()
        .map(|setup| setup.config_fields.as_slice())
        .unwrap_or_default();
    if fields
        .iter()
        .any(|field| match request.config.get(&field.id) {
            Some(value) => !valid_setup_value(field, value),
            None => field.required,
        })
        || request
            .config
            .keys()
            .any(|key| !fields.iter().any(|field| field.id == *key))
    {
        return Err(CommandError::invalid(
            "connector configuration does not match its setup contract",
        ));
    }
    let previous_config = read_connector_config(state, &request.entry_id)?;
    let credential_scope_changed = entry
        .setup
        .as_ref()
        .map(|setup| setup.credential_scope_fields.as_slice())
        .unwrap_or_default()
        .iter()
        .any(|scope_id| previous_config.get(scope_id) != request.config.get(scope_id));
    if credential_scope_changed {
        disable_bindings_and_delete_credentials_locked(
            state,
            &request.entry_id,
            &_lifecycle_guard,
        )?;
    }
    write_connector_config(state, &request.entry_id, &request.config)?;
    connector_status(entry, state)
}
