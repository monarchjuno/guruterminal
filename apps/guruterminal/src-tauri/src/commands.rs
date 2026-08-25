pub(crate) mod attachments;
pub(crate) mod chat_runtime;
pub(crate) mod guru;
pub(crate) mod memory_crud;
mod memory_updates;
pub(crate) mod memory_write;
pub(crate) mod records;
mod tool_executor;
pub mod types;

#[cfg(test)]
pub use chat_runtime::{chat_abort, chat_send, chat_steer};
pub(crate) use guru::bootstrap_default_guru;
pub use guru::{
    agent_skill_catalog, agent_skills_update, guru_create, guru_delete, guru_export_memory,
    guru_import_memory, guru_list, guru_recover, guru_rename, guru_select,
};
#[cfg(test)]
use guru::{copy_memory_records, create_managed_guru, delete_guru_inner, list_available_gurus};
pub use memory_crud::{
    library_memory_create, library_memory_delete, library_memory_revert, library_memory_update,
    LibraryDraftDto, LibraryMemoryCreateRequest, LibraryMemoryDeleteRequest,
    LibraryMemoryMutationDto, LibraryMemoryRevertRequest, LibraryMemoryUpdateRequest,
};
pub use records::{
    chat_artifact_list, chat_artifact_read, chat_attachment_read, chat_create, chat_delete,
    chat_rename, library_read, library_search,
};
pub use types::*;

#[cfg(test)]
pub(crate) use attachments::*;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use chrono::{SecondsFormat, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;
use uuid::Uuid;

use crate::{
    agent_harness::{self, UserSkillSnapshot},
    app::{AppState, CommandError, GuruAccess, GuruAvailability},
    artifact_trust::ensure_private_directory,
    domain::{GuruCapabilityBinding, GuruProfile},
    guru_root::{profile_workspace, BoundGuruRoot},
    settings::ModelCatalogView,
    store::GuruTerminalStore,
};

pub(crate) const MAX_PROMPT_BYTES: usize = 128 * 1024;
pub(crate) const MAX_CHAT_OUTPUT_BYTES: usize = 512 * 1024;
pub(crate) const MAX_CHAT_CONTEXT_BYTES: usize = 256 * 1024;
pub(crate) const MAX_CHAT_TITLE_BYTES: usize = 200;

fn clean_frontmatter_scalar(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('\'') && value.ends_with('\''))
            || (value.starts_with('"') && value.ends_with('"')))
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

pub(crate) fn json_text_from_markdown(markdown: &str, field: &str) -> Option<String> {
    let mut lines = markdown.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.trim() == field {
                return Some(clean_frontmatter_scalar(value));
            }
        }
    }
    None
}

#[tauri::command(rename_all = "snake_case")]
pub fn model_catalog_get(state: State<'_, AppState>) -> Result<ModelCatalogView, CommandError> {
    state.model_catalog_view()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelVisibilityUpdateRequest {
    pub model_profile_id: String,
    pub visible_in_chat: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub fn model_visibility_update(
    request: ModelVisibilityUpdateRequest,
    state: State<'_, AppState>,
) -> Result<ModelCatalogView, CommandError> {
    state.set_model_visible(&request.model_profile_id, request.visible_in_chat)?;
    state.model_catalog_view()
}

#[tauri::command(rename_all = "snake_case")]
pub fn run_activity_list(
    state: State<'_, AppState>,
) -> Result<Vec<crate::run_coordinator::RunActivity>, CommandError> {
    state.run_coordinator.activities()
}

#[tauri::command(rename_all = "snake_case")]
pub fn open_external_url(url: String) -> Result<(), CommandError> {
    let url = crate::browser::validated_http_url(&url)?;
    crate::external_browser::open(url.as_str())
        .map_err(|error| CommandError::internal(format!("could not open external link: {error}")))
}

pub(crate) fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub(crate) fn iso_time(timestamp_ms: i64) -> Result<String, CommandError> {
    Utc.timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or_else(|| CommandError::internal("stored timestamp is invalid"))
}

pub(crate) fn require_text(
    value: &str,
    label: &str,
    maximum: usize,
) -> Result<String, CommandError> {
    let value = value.trim();
    if value.is_empty() || value.len() > maximum || value.contains('\0') {
        return Err(CommandError::invalid(format!(
            "{label} must contain between 1 and {maximum} bytes"
        )));
    }
    Ok(value.to_owned())
}

pub(crate) fn fallback_chat_title(prompt: &str) -> String {
    let title = prompt
        .lines()
        .next()
        .unwrap_or(prompt)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let title = title.chars().take(40).collect::<String>();
    if title.is_empty() {
        "New chat".into()
    } else {
        title
    }
}

pub(crate) fn map_internal(error: impl std::fmt::Display) -> CommandError {
    CommandError::internal(error.to_string())
}

pub(crate) fn map_runtime(error: crate::runtime::RuntimeError) -> CommandError {
    match error {
        crate::runtime::RuntimeError::BeforeHashMismatch => {
            CommandError::conflict(error.to_string())
        }
        other => CommandError::internal(other.to_string()),
    }
}

/// Preserve not-found and conflict when mapping a store error to a command error.
pub(crate) fn map_store(error: crate::store::StoreError) -> CommandError {
    use crate::store::StoreError;
    match error {
        StoreError::Conflict(message) => CommandError::conflict(message),
        StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows) => {
            CommandError::not_found("record")
        }
        other => CommandError::internal(other.to_string()),
    }
}

pub(crate) fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

pub(crate) fn enabled_skill_ids(
    store: &dyn GuruTerminalStore,
    guru_id: &str,
) -> Result<Vec<String>, CommandError> {
    ensure_current_skill_bindings(store, guru_id)?;
    let requested = store
        .list_guru_capabilities(guru_id)
        .map_err(map_store)?
        .into_iter()
        .filter(|binding| binding.enabled)
        .filter_map(|binding| {
            agent_harness::skill_id_from_binding(&binding.entry_id).map(str::to_owned)
        })
        .collect::<Vec<_>>();
    agent_harness::normalize_selectable_skill_ids(&requested).map_err(map_internal)
}

fn ensure_current_skill_bindings(
    store: &dyn GuruTerminalStore,
    guru_id: &str,
) -> Result<(), CommandError> {
    let bindings = store.list_guru_capabilities(guru_id).map_err(map_store)?;
    if bindings
        .iter()
        .any(|binding| agent_harness::skill_id_from_binding(&binding.entry_id).is_some())
    {
        return Ok(());
    }
    let timestamp = now_ms();
    for skill_id in agent_harness::default_skill_ids() {
        let entry_id = agent_harness::skill_binding_id(&skill_id).map_err(map_internal)?;
        store
            .save_guru_capability(&GuruCapabilityBinding {
                guru_id: guru_id.to_owned(),
                entry_id,
                enabled: true,
                granted_permissions: vec!["load".to_owned()],
                config: BTreeMap::new(),
                updated_at_ms: timestamp,
            })
            .map_err(map_store)?;
    }
    Ok(())
}

pub(crate) fn current_user_skill_snapshots(
    store: &dyn GuruTerminalStore,
    guru_id: &str,
) -> Result<Vec<UserSkillSnapshot>, CommandError> {
    let mut snapshots = Vec::new();
    for skill in store
        .list_user_skills_for_guru(guru_id)
        .map_err(map_store)?
    {
        skill.validate().map_err(map_internal)?;
        let binding_id = agent_harness::user_skill_binding_id(&skill.id).map_err(map_internal)?;
        let enabled = store
            .get_guru_capability(guru_id, &binding_id)
            .map_err(map_store)?
            .is_some_and(|binding| binding.enabled && binding.granted_permissions == ["load"]);
        if !enabled {
            continue;
        }
        let revision = store
            .get_user_skill_revision(&skill.current_revision_id)
            .map_err(map_store)?
            .ok_or_else(|| CommandError::internal("current user Skill revision is missing"))?;
        revision.validate().map_err(map_internal)?;
        if revision.skill_id != skill.id || revision.guru_id != guru_id {
            return Err(CommandError::internal(
                "current user Skill revision binding is invalid",
            ));
        }
        snapshots.push(UserSkillSnapshot {
            id: skill.id,
            revision_id: revision.id,
            content_sha256: revision.content_sha256,
        });
    }
    snapshots.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(snapshots)
}

pub(crate) fn materialize_user_skill_snapshots(
    store: &dyn GuruTerminalStore,
    guru_id: &str,
    snapshots: &[UserSkillSnapshot],
    destination: &Path,
) -> Result<Vec<PathBuf>, CommandError> {
    ensure_private_directory(destination).map_err(map_internal)?;
    let mut paths = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let revision = store
            .get_user_skill_revision(&snapshot.revision_id)
            .map_err(map_store)?
            .ok_or_else(|| CommandError::conflict("pinned user Skill revision is missing"))?;
        revision.validate().map_err(map_internal)?;
        if revision.skill_id != snapshot.id
            || revision.guru_id != guru_id
            || revision.content_sha256 != snapshot.content_sha256
        {
            return Err(CommandError::conflict("pinned user Skill revision changed"));
        }
        let slug = crate::user_skill::skill_slug(&snapshot.id).map_err(map_internal)?;
        let skill_dir = destination.join(slug);
        ensure_private_directory(&skill_dir).map_err(map_internal)?;
        let path = skill_dir.join("SKILL.md");
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&path).map_err(map_internal)?;
        file.write_all(agent_harness::apply_user_skill_banner(&revision.markdown).as_bytes())
            .map_err(map_internal)?;
        file.sync_all().map_err(map_internal)?;
        let mut permissions = file.metadata().map_err(map_internal)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).map_err(map_internal)?;
        paths.push(path);
    }
    Ok(paths)
}

/// Non-secret authority snapshot for retaining Pi JSONL tool-result context.
///
/// Configuration and credential values are deliberately excluded. Their
/// opaque, independently-rotated revisions bind cache reuse instead.
#[derive(Clone, Debug)]
pub(crate) struct ChatConnectorAuthority {
    pub(crate) capability_ids: Vec<String>,
    pub(crate) sha256: String,
    pub(crate) cacheable: bool,
}

#[derive(Clone, Serialize)]
struct ChatConnectorAuthoritySeal {
    version: &'static str,
    bindings: Vec<ChatConnectorBindingSeal>,
    connectors: Vec<ChatConnectorSeal>,
}

#[derive(Clone, Serialize)]
struct ChatConnectorBindingSeal {
    entry_id: String,
    enabled: bool,
    execute: bool,
    updated_at_ms: i64,
}

#[derive(Clone, Serialize)]
struct ChatConnectorSeal {
    entry_id: String,
    config_revision: crate::marketplace::connector_config::ConnectorConfigRevision,
    active_credential_revision: Option<String>,
}

const CHAT_CONNECTOR_AUTHORITY_SEAL_VERSION: &str = "chat-connector-authority/v1";

fn canonicalize_chat_connector_authority_seal(seal: &mut ChatConnectorAuthoritySeal) {
    seal.bindings
        .sort_by(|left, right| left.entry_id.cmp(&right.entry_id));
    seal.connectors
        .sort_by(|left, right| left.entry_id.cmp(&right.entry_id));
}

fn chat_connector_authority_sha256(
    seal: &ChatConnectorAuthoritySeal,
) -> Result<String, CommandError> {
    let serialized = serde_json::to_vec(seal).map_err(map_internal)?;
    Ok(crate::hashing::sha256(&serialized))
}

fn binding_grants_execute(binding: &GuruCapabilityBinding) -> bool {
    binding
        .granted_permissions
        .iter()
        .any(|permission| permission == "execute")
}

fn enabled_execute_capability_ids_from_bindings(
    state: &AppState,
    bindings: &[GuruCapabilityBinding],
) -> Result<Vec<String>, CommandError> {
    let mut ready = BTreeSet::new();
    for binding in bindings {
        if binding.enabled
            && binding_grants_execute(binding)
            && crate::marketplace::execute_binding_ready(binding, state)?
        {
            ready.insert(binding.entry_id.clone());
        }
    }
    Ok(ready.into_iter().collect())
}

#[cfg(test)]
pub(crate) fn enabled_execute_capability_ids(
    state: &AppState,
    guru_id: &str,
) -> Result<Vec<String>, CommandError> {
    crate::marketplace::with_connector_lifecycle(|| {
        let bindings = state
            .store
            .list_guru_capabilities(guru_id)
            .map_err(map_store)?;
        enabled_execute_capability_ids_from_bindings(state, &bindings)
    })
}

/// Captures every connector authority which may influence a Pi session. The
/// whole catalog is included: a disabled connector's global config can be a
/// runtime dependency of another enabled connector.
pub(crate) fn capture_chat_connector_authority(
    state: &AppState,
    guru_id: &str,
) -> Result<ChatConnectorAuthority, CommandError> {
    crate::marketplace::with_connector_lifecycle(|| {
        let bindings = state
            .store
            .list_guru_capabilities(guru_id)
            .map_err(map_store)?;
        let capability_ids = enabled_execute_capability_ids_from_bindings(state, &bindings)?;
        let binding_seals = bindings
            .into_iter()
            .map(|binding| {
                let execute = binding_grants_execute(&binding);
                ChatConnectorBindingSeal {
                    entry_id: binding.entry_id,
                    enabled: binding.enabled,
                    execute,
                    updated_at_ms: binding.updated_at_ms,
                }
            })
            .collect::<Vec<_>>();
        let catalog = crate::marketplace::bundled_catalog()?;
        let mut cacheable = true;
        let connector_seals = catalog
            .entries
            .into_iter()
            .map(|entry| {
                let config_revision =
                    match crate::marketplace::connector_config::connector_config_revision(
                        state, &entry.id,
                    ) {
                        Ok(revision) => revision,
                        // A malformed or unreadable dormant connector must never
                        // keep an old Pi cache alive, but it need not prevent a
                        // Chat that does not execute that connector.
                        Err(_) => {
                            cacheable = false;
                            crate::marketplace::connector_config::ConnectorConfigRevision::Legacy
                        }
                    };
                if !config_revision.is_cacheable() {
                    cacheable = false;
                }
                let active_credential_revision =
                    match crate::finance_credentials::active_revision(&entry.id) {
                        Ok(revision) => revision,
                        Err(_) => {
                            cacheable = false;
                            None
                        }
                    };
                ChatConnectorSeal {
                    entry_id: entry.id,
                    config_revision,
                    active_credential_revision,
                }
            })
            .collect::<Vec<_>>();
        let mut seal = ChatConnectorAuthoritySeal {
            version: CHAT_CONNECTOR_AUTHORITY_SEAL_VERSION,
            bindings: binding_seals,
            connectors: connector_seals,
        };
        canonicalize_chat_connector_authority_seal(&mut seal);
        Ok(ChatConnectorAuthority {
            capability_ids,
            sha256: chat_connector_authority_sha256(&seal)?,
            cacheable,
        })
    })
}

pub(crate) async fn profile_summary(
    state: &AppState,
    profile: &GuruProfile,
) -> Result<GuruSummary, CommandError> {
    match state.guru_access(&profile.id) {
        GuruAccess::Visible(availability) => {
            profile_summary_with_availability(state, profile, availability).await
        }
        GuruAccess::Hidden => Err(CommandError::new(
            "guru_storage_unavailable",
            "Guru storage is unavailable",
        )),
    }
}

pub(crate) async fn profile_summary_at(
    state: &AppState,
    profile: &GuruProfile,
    workspace: &BoundGuruRoot,
) -> Result<GuruSummary, CommandError> {
    state.ensure_guru_available(&profile.id)?;
    profile_summary_from_parts(
        state,
        profile,
        record_count_at(state, workspace).await,
        GuruAvailability::Available,
    )
}

async fn profile_summary_with_availability(
    state: &AppState,
    profile: &GuruProfile,
    availability: GuruAvailability,
) -> Result<GuruSummary, CommandError> {
    let record_count = match availability {
        GuruAvailability::Available => {
            state.ensure_guru_available(&profile.id)?;
            let workspace = profile_workspace(profile)?;
            record_count_at(state, &workspace).await
        }
        GuruAvailability::RecoveryRequired { .. } => 0,
    };
    profile_summary_from_parts(state, profile, record_count, availability)
}

async fn record_count_at(state: &AppState, workspace: &BoundGuruRoot) -> usize {
    match &state.runtime {
        Some(runtime) => workspace
            .knowledge_list(runtime, None)
            .await
            .ok()
            .and_then(|value| value.as_array().map(Vec::len))
            .unwrap_or(0),
        None => 0,
    }
}

fn profile_summary_from_parts(
    state: &AppState,
    profile: &GuruProfile,
    record_count: usize,
    availability: GuruAvailability,
) -> Result<GuruSummary, CommandError> {
    let palette = ["#a4530c", "#4d5f8e", "#76508d", "#2f6f83", "#5f6b7a"];
    let accent_index = profile
        .id
        .bytes()
        .fold(0_usize, |total, byte| total.wrapping_add(byte as usize))
        % palette.len();
    Ok(GuruSummary {
        id: profile.id.clone(),
        name: profile.name.clone(),
        philosophy: profile.description.clone(),
        record_count,
        updated_at: iso_time(profile.updated_at_ms)?,
        accent: palette[accent_index].into(),
        last_model_profile_id: profile.last_model_profile_id.clone(),
        enabled_skill_ids: enabled_skill_ids(state.store.as_ref(), &profile.id)?,
        availability,
    })
}

pub(crate) fn json_text<'a>(value: &'a Value, key: &str) -> Result<&'a str, CommandError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| CommandError::internal(format!("Runtime result is missing {key}")))
}

#[cfg(test)]
mod tests;
