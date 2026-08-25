use std::collections::BTreeMap;
use std::fs;
use std::io::Write;

use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

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
const CONNECTOR_CONFIG_SCHEMA_VERSION: &str = "guruterminal-connector-config/1";
const WEB_RESEARCH_ID: &str = "community.web-research";
const WEB_RESEARCH_POLICY_FIELD: &str = "search_policy";

/// A non-secret representation of the configuration state that determines
/// whether a cached Pi session is safe to resume.
///
/// `Legacy` deliberately has no revision: callers must treat it as
/// non-cacheable until the configuration is next saved in the revisioned
/// envelope format.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(tag = "state", content = "revision", rename_all = "snake_case")]
pub(crate) enum ConnectorConfigRevision {
    Absent,
    Legacy,
    Revision(String),
}

impl ConnectorConfigRevision {
    pub(crate) const fn is_cacheable(&self) -> bool {
        !matches!(self, Self::Legacy)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RevisionedConnectorConfig {
    schema_version: String,
    revision: String,
    config: BTreeMap<String, String>,
}

enum StoredConnectorConfig {
    Absent,
    Legacy(BTreeMap<String, String>),
    Revisioned(RevisionedConnectorConfig),
}

impl StoredConnectorConfig {
    fn config(self) -> BTreeMap<String, String> {
        match self {
            Self::Absent => BTreeMap::new(),
            Self::Legacy(config) => config,
            Self::Revisioned(envelope) => envelope.config,
        }
    }

    fn revision(&self) -> ConnectorConfigRevision {
        match self {
            Self::Absent => ConnectorConfigRevision::Absent,
            Self::Legacy(_) => ConnectorConfigRevision::Legacy,
            Self::Revisioned(envelope) => {
                ConnectorConfigRevision::Revision(envelope.revision.clone())
            }
        }
    }
}

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
    Ok(read_stored_connector_config(state, entry_id)?.config())
}

/// Returns the configuration revision without exposing or hashing any
/// configuration values. `Legacy` must invalidate Pi session reuse because a
/// raw legacy file has no stable non-secret revision marker.
pub(crate) fn connector_config_revision(
    state: &AppState,
    entry_id: &str,
) -> Result<ConnectorConfigRevision, CommandError> {
    Ok(read_stored_connector_config(state, entry_id)?.revision())
}

fn read_stored_connector_config(
    state: &AppState,
    entry_id: &str,
) -> Result<StoredConnectorConfig, CommandError> {
    let path = connector_config_path(state, entry_id)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StoredConnectorConfig::Absent)
        }
        Err(error) => return Err(CommandError::internal(error.to_string())),
        Ok(_) => {}
    }
    let bytes = read_private_regular_file_bounded(&path, MAX_CONNECTOR_CONFIG_BYTES as u64)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| CommandError::internal("connector configuration is invalid"))?;
    if let Ok(envelope) = serde_json::from_value::<RevisionedConnectorConfig>(value.clone()) {
        validate_revisioned_config(&envelope)?;
        return Ok(StoredConnectorConfig::Revisioned(envelope));
    }
    let config = serde_json::from_value(value)
        .map_err(|_| CommandError::internal("connector configuration is invalid"))?;
    Ok(StoredConnectorConfig::Legacy(config))
}

pub(super) fn write_connector_config(
    state: &AppState,
    entry_id: &str,
    config: &BTreeMap<String, String>,
) -> Result<(), CommandError> {
    let existing = read_stored_connector_config(state, entry_id)?;
    let revision = match existing {
        StoredConnectorConfig::Revisioned(existing) if existing.config == *config => {
            existing.revision
        }
        StoredConnectorConfig::Absent
        | StoredConnectorConfig::Legacy(_)
        | StoredConnectorConfig::Revisioned(_) => Uuid::new_v4().hyphenated().to_string(),
    };
    let envelope = RevisionedConnectorConfig {
        schema_version: CONNECTOR_CONFIG_SCHEMA_VERSION.to_owned(),
        revision,
        config: config.clone(),
    };
    let path = connector_config_path(state, entry_id)?;
    let bytes =
        serde_json::to_vec(&envelope).map_err(|error| CommandError::internal(error.to_string()))?;
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

fn validate_revisioned_config(envelope: &RevisionedConnectorConfig) -> Result<(), CommandError> {
    if envelope.schema_version != CONNECTOR_CONFIG_SCHEMA_VERSION {
        return Err(CommandError::internal("connector configuration is invalid"));
    }
    let parsed = Uuid::parse_str(&envelope.revision)
        .map_err(|_| CommandError::internal("connector configuration is invalid"))?;
    if parsed.hyphenated().to_string() != envelope.revision {
        return Err(CommandError::internal("connector configuration is invalid"));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ENTRY_ID: &str = "sec.edgar";

    fn test_config() -> BTreeMap<String, String> {
        BTreeMap::from([(
            "contact_email".to_owned(),
            "research@example.invalid".to_owned(),
        )])
    }

    fn revision(state: &AppState) -> String {
        match connector_config_revision(state, TEST_ENTRY_ID).unwrap() {
            ConnectorConfigRevision::Revision(revision) => revision,
            state => panic!("expected a revisioned connector config, got {state:?}"),
        }
    }

    #[test]
    fn absent_config_has_a_secret_free_cache_state() {
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));

        let state = connector_config_revision(&state, TEST_ENTRY_ID).unwrap();

        assert_eq!(state, ConnectorConfigRevision::Absent);
        assert!(state.is_cacheable());
    }

    #[test]
    fn legacy_raw_config_remains_readable_but_is_not_cacheable() {
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let config = test_config();
        let path = connector_config_path(&state, TEST_ENTRY_ID).unwrap();
        let bytes = serde_json::to_vec(&config).unwrap();
        fs::write(&path, bytes).unwrap();
        ensure_private_regular_file(&path).unwrap();

        assert_eq!(
            read_connector_config(&state, TEST_ENTRY_ID).unwrap(),
            config
        );
        let state = connector_config_revision(&state, TEST_ENTRY_ID).unwrap();
        assert_eq!(state, ConnectorConfigRevision::Legacy);
        assert!(!state.is_cacheable());
    }

    #[test]
    fn legacy_raw_config_with_a_schema_like_key_remains_legacy() {
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let config = BTreeMap::from([(
            "schema_version".to_owned(),
            CONNECTOR_CONFIG_SCHEMA_VERSION.to_owned(),
        )]);
        let path = connector_config_path(&state, TEST_ENTRY_ID).unwrap();
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        ensure_private_regular_file(&path).unwrap();

        assert_eq!(
            read_connector_config(&state, TEST_ENTRY_ID).unwrap(),
            config
        );
        assert_eq!(
            connector_config_revision(&state, TEST_ENTRY_ID).unwrap(),
            ConnectorConfigRevision::Legacy
        );
    }

    #[test]
    fn writing_legacy_config_migrates_it_to_a_revisioned_envelope() {
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let config = test_config();
        let path = connector_config_path(&state, TEST_ENTRY_ID).unwrap();
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        ensure_private_regular_file(&path).unwrap();

        write_connector_config(&state, TEST_ENTRY_ID, &config).unwrap();

        assert_eq!(
            read_connector_config(&state, TEST_ENTRY_ID).unwrap(),
            config
        );
        assert!(matches!(
            connector_config_revision(&state, TEST_ENTRY_ID).unwrap(),
            ConnectorConfigRevision::Revision(_)
        ));
    }

    #[test]
    fn writing_equal_config_preserves_its_revision() {
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let config = test_config();

        write_connector_config(&state, TEST_ENTRY_ID, &config).unwrap();
        let first = revision(&state);
        write_connector_config(&state, TEST_ENTRY_ID, &config).unwrap();

        assert_eq!(revision(&state), first);
    }

    #[test]
    fn writing_changed_config_rotates_its_revision() {
        let temporary = tempfile::tempdir().unwrap();
        let state = AppState::for_test(temporary.path().join("app"));
        let config = test_config();

        write_connector_config(&state, TEST_ENTRY_ID, &config).unwrap();
        let first = revision(&state);
        let changed = BTreeMap::from([(
            "contact_email".to_owned(),
            "updated@example.invalid".to_owned(),
        )]);
        write_connector_config(&state, TEST_ENTRY_ID, &changed).unwrap();

        assert_ne!(revision(&state), first);
        assert_eq!(
            read_connector_config(&state, TEST_ENTRY_ID).unwrap(),
            changed
        );
    }
}
