use std::collections::{BTreeMap, BTreeSet};
use std::sync::MutexGuard;
use std::time::Duration;

use chrono::Utc;
use serde_json::{json, Value};
use tauri::State;

use crate::{
    app::{AppState, CommandError},
    maintenance::MaintenanceActivityKind,
    mcp::{
        contains_protected_value, filter_tool_providers, validate_result_provider, McpError,
        McpLaunchConfig, McpSession,
    },
    run_scratch::RunScratch,
    store::GuruTerminalStore,
};

use super::{
    catalog::{
        bundled_catalog, MarketplaceCatalogDto, MarketplaceCredentialRequest,
        MarketplaceCredentialSaveRequest, MarketplaceCredentialStatusDto,
        MarketplaceCredentialVerification, MarketplaceEntryDto, MarketplaceRuntimeKind,
        MarketplaceSetupFieldKind,
    },
    connector_config::{connector_config_value, valid_setup_value},
    connector_lifecycle_lock, ensure_installable,
};

const MCP_CREDENTIAL_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const MCP_CREDENTIAL_SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum McpCredentialProbeError {
    Rejected,
    Unavailable,
}

pub(super) fn credential_statuses(
    entry: &MarketplaceEntryDto,
) -> Result<Vec<MarketplaceCredentialStatusDto>, CommandError> {
    let fields = entry
        .setup
        .as_ref()
        .map(|setup| setup.credential_fields.as_slice())
        .unwrap_or_default();
    if fields.is_empty() {
        return Ok(Vec::new());
    }
    let status = crate::finance_credentials::status(&entry.id)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    Ok(fields
        .iter()
        .map(|field| {
            let active = status.active_fields.contains(&field.id);
            let pending = status.candidate_fields.contains(&field.id);
            let verification = if pending {
                match status.verification {
                    crate::finance_credentials::CredentialVerification::Never => {
                        MarketplaceCredentialVerification::Never
                    }
                    crate::finance_credentials::CredentialVerification::Verified => {
                        MarketplaceCredentialVerification::Verified
                    }
                    crate::finance_credentials::CredentialVerification::Rejected => {
                        MarketplaceCredentialVerification::Rejected
                    }
                    crate::finance_credentials::CredentialVerification::TemporarilyUnavailable => {
                        MarketplaceCredentialVerification::TemporarilyUnavailable
                    }
                }
            } else if active {
                MarketplaceCredentialVerification::Verified
            } else {
                MarketplaceCredentialVerification::Never
            };
            let last_error = match verification {
                MarketplaceCredentialVerification::Rejected => {
                    Some("The provider rejected these credentials.".to_owned())
                }
                MarketplaceCredentialVerification::TemporarilyUnavailable => {
                    Some("The provider could not verify these credentials right now.".to_owned())
                }
                MarketplaceCredentialVerification::Never
                | MarketplaceCredentialVerification::Verified => None,
            };
            MarketplaceCredentialStatusDto {
                entry_id: entry.id.clone(),
                credential_id: field.id.clone(),
                stored: active || pending,
                active,
                pending,
                verification,
                verified_at: active.then_some(status.verified_at).flatten(),
                last_error,
            }
        })
        .collect())
}

pub(super) fn credential_provider_context(
    state: &AppState,
    entry_id: &str,
) -> Result<Option<String>, CommandError> {
    match entry_id {
        "sec.edgar" => connector_config_value(state, entry_id, "contact_email"),
        crate::finance_data::KIS_SOURCE_ID => {
            connector_config_value(state, entry_id, "environment")
        }
        _ => Ok(None),
    }
}

pub(super) fn credential_entry<'a>(
    catalog: &'a MarketplaceCatalogDto,
    entry_id: &str,
) -> Result<&'a MarketplaceEntryDto, CommandError> {
    let entry = catalog
        .entries
        .iter()
        .find(|entry| entry.id == entry_id)
        .ok_or_else(|| CommandError::not_found("Marketplace entry"))?;
    let fields = entry
        .setup
        .as_ref()
        .map(|setup| setup.credential_fields.as_slice())
        .unwrap_or_default();
    if fields.is_empty() {
        return Err(CommandError::invalid(
            "connector credentials are not declared",
        ));
    }
    Ok(entry)
}

pub(super) fn validate_credential_secrets(
    entry: &MarketplaceEntryDto,
    secrets: &BTreeMap<String, String>,
) -> Result<(), CommandError> {
    let fields = entry
        .setup
        .as_ref()
        .map(|setup| setup.credential_fields.as_slice())
        .unwrap_or_default();
    if secrets.is_empty()
        || secrets
            .keys()
            .any(|credential_id| !fields.iter().any(|field| field.id == *credential_id))
    {
        return Err(CommandError::invalid(
            "Enter at least one declared credential or profile field.",
        ));
    }
    for (credential_id, secret) in secrets {
        let field = fields
            .iter()
            .find(|field| field.id == *credential_id)
            .expect("submitted credential ids were checked above");
        if !(field.min_length..=field.max_length).contains(&secret.len()) {
            return Err(CommandError::invalid(format!(
                "{} must be between {} and {} characters.",
                field.label, field.min_length, field.max_length
            )));
        }
        if secret.contains('\0') || secret.chars().any(char::is_control) {
            return Err(CommandError::invalid(format!(
                "{} contains unsupported characters.",
                field.label
            )));
        }
        if matches!(field.kind, MarketplaceSetupFieldKind::ApiKey)
            && secret.chars().any(char::is_whitespace)
        {
            return Err(CommandError::invalid(format!(
                "{} cannot contain whitespace.",
                field.label
            )));
        }
        if !valid_setup_value(field, secret) {
            return Err(CommandError::invalid(format!(
                "Enter a valid {}.",
                field.label.to_lowercase()
            )));
        }
        if entry.id == crate::finance_data::KIS_SOURCE_ID
            && field.id == "account_number"
            && !secret.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(CommandError::invalid(
                "KIS account number must contain exactly 8 digits.",
            ));
        }
        if entry.id == crate::finance_data::KIS_SOURCE_ID
            && field.id == "account_product_code"
            && !secret.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(CommandError::invalid(
                "KIS account product code must be alphanumeric.",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_required_credential_fields(
    entry: &MarketplaceEntryDto,
    patch: &BTreeMap<String, String>,
    current: &crate::finance_credentials::CredentialStatus,
) -> Result<(), CommandError> {
    let fields = entry
        .setup
        .as_ref()
        .map(|setup| setup.credential_fields.as_slice())
        .unwrap_or_default();
    if let Some(missing) = fields.iter().find(|field| {
        field.required
            && !patch.contains_key(&field.id)
            && !current.active_fields.contains(&field.id)
            && !current.candidate_fields.contains(&field.id)
    }) {
        return Err(CommandError::invalid(format!(
            "{} is required.",
            missing.label
        )));
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn marketplace_credential_save(
    request: MarketplaceCredentialSaveRequest,
    state: State<'_, AppState>,
) -> Result<Vec<MarketplaceCredentialStatusDto>, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::MarketplaceCredential)?;
    let catalog = bundled_catalog()?;
    ensure_installable(&request.entry_id)?;
    let _lifecycle_guard = connector_lifecycle_lock()?;
    let entry = credential_entry(&catalog, &request.entry_id)?;
    validate_credential_secrets(entry, &request.secrets)?;
    let current = crate::finance_credentials::status(&request.entry_id)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    validate_required_credential_fields(entry, &request.secrets, &current)?;
    crate::finance_credentials::stage(&request.entry_id, &request.secrets)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    credential_statuses(entry)
}

pub(super) fn credential_verification_outcome(
    result: &Result<(), crate::finance_data::FinanceDataError>,
) -> crate::finance_credentials::VerificationOutcome {
    use crate::finance_data::FinanceDataError;

    match result {
        Ok(()) => crate::finance_credentials::VerificationOutcome::Verified,
        Err(FinanceDataError::CredentialRejected(_))
        | Err(FinanceDataError::KisCredentialRejected(_))
        | Err(FinanceDataError::InvalidQuery(_)) => {
            crate::finance_credentials::VerificationOutcome::Rejected
        }
        Err(_) => crate::finance_credentials::VerificationOutcome::TemporarilyUnavailable,
    }
}

pub(super) fn credential_verification_error(
    error: &crate::finance_data::FinanceDataError,
) -> CommandError {
    use crate::finance_data::FinanceDataError;

    match error {
        FinanceDataError::KisCredentialRejected(diagnostic) => CommandError::new(
            "credential_rejected",
            format!("KIS error · {}", diagnostic.summary()),
        ),
        FinanceDataError::KisRateLimited(diagnostic) => CommandError::new(
            "provider_unavailable",
            format!(
                "KIS token verification is temporarily limited. KIS error · {}",
                diagnostic.summary()
            ),
        ),
        FinanceDataError::CredentialRejected(_) | FinanceDataError::InvalidQuery(_) => {
            CommandError::new(
                "credential_rejected",
                "The provider rejected these credentials.",
            )
        }
        _ => CommandError::new(
            "provider_unavailable",
            "The provider could not verify these credentials right now.",
        ),
    }
}

fn credential_scope_snapshot(
    state: &AppState,
    entry: &MarketplaceEntryDto,
) -> Result<BTreeMap<String, String>, CommandError> {
    let Some(setup) = &entry.setup else {
        return Ok(BTreeMap::new());
    };
    setup
        .credential_scope_fields
        .iter()
        .map(|field_id| {
            let value = connector_config_value(state, &entry.id, field_id)?.ok_or_else(|| {
                CommandError::conflict("connector configuration must be completed first")
            })?;
            Ok((field_id.clone(), value))
        })
        .collect()
}

async fn verify_bundled_mcp_credential(
    state: &AppState,
    entry: &MarketplaceEntryDto,
    candidate_secrets: &BTreeMap<String, String>,
) -> Result<(), McpCredentialProbeError> {
    let server_id = entry
        .runtime
        .server_id
        .as_deref()
        .ok_or(McpCredentialProbeError::Unavailable)?;
    let runtime = state
        .artifacts
        .mcp_runtimes
        .get(server_id)
        .cloned()
        .ok_or(McpCredentialProbeError::Unavailable)?;
    let probe = entry
        .runtime
        .verification_probe
        .as_ref()
        .ok_or(McpCredentialProbeError::Unavailable)?;
    let enabled_provider_ids = entry
        .runtime
        .provider_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if enabled_provider_ids.is_empty() || !enabled_provider_ids.is_subset(&runtime.provider_ids) {
        return Err(McpCredentialProbeError::Unavailable);
    }
    let possible_network_hosts = enabled_provider_ids
        .iter()
        .filter_map(|provider| runtime.provider_network_hosts.get(provider))
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    let allowed_network_hosts = entry
        .permissions
        .network_hosts
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if allowed_network_hosts.is_empty() || allowed_network_hosts != possible_network_hosts {
        return Err(McpCredentialProbeError::Unavailable);
    }
    let provider_receipt_pointer = runtime.provider_receipt_pointer.clone();
    let tool_activation = runtime.tool_activation.clone();
    let control_tool_names = runtime.control_tool_names.clone();

    let mut credentials = BTreeMap::new();
    for (field_id, target_id) in &entry.runtime.credential_mapping {
        let secret = candidate_secrets
            .get(field_id)
            .ok_or(McpCredentialProbeError::Unavailable)?
            .clone();
        if credentials.insert(target_id.clone(), secret).is_some() {
            return Err(McpCredentialProbeError::Unavailable);
        }
    }
    let sensitive_values = credentials.values().cloned().collect::<Vec<_>>();
    let mut provider_config = BTreeMap::<String, BTreeMap<String, String>>::new();
    if !entry.runtime.config_mapping.is_empty() {
        let [provider_id] = entry.runtime.provider_ids.as_slice() else {
            return Err(McpCredentialProbeError::Unavailable);
        };
        let values = provider_config.entry(provider_id.clone()).or_default();
        for field_id in entry.runtime.config_mapping.keys() {
            let value = connector_config_value(state, &entry.id, field_id)
                .map_err(|_| McpCredentialProbeError::Unavailable)?
                .ok_or(McpCredentialProbeError::Unavailable)?;
            values.insert(field_id.clone(), value);
        }
    }

    let scratch = RunScratch::create(
        state.artifacts.deletion_root.clone(),
        "credential-probe",
        &format!("probe-{}", uuid::Uuid::new_v4().simple()),
    )
    .map_err(|_| McpCredentialProbeError::Unavailable)?;
    let launch = McpLaunchConfig {
        server_id: server_id.to_owned(),
        executable: runtime.executable,
        arguments: Vec::new(),
        private_working_dir: scratch.path().to_path_buf(),
        lease_dir: runtime.lease_dir,
        environment: BTreeMap::new(),
        bootstrap: json!({
            "type": "guruterminal.bootstrap",
            "protocol_version": 1,
            "run_id": format!("credential-probe:{}", uuid::Uuid::new_v4().simple()),
            "scratch_dir": scratch.path().to_string_lossy(),
            "credentials": credentials,
            "settings": {
                "allowed_categories": runtime.allowed_categories,
                "enabled_provider_ids": enabled_provider_ids,
                "allowed_network_hosts": allowed_network_hosts,
                "provider_config": provider_config,
            }
        }),
    };
    let (session, initial_tools) = McpSession::spawn(launch)
        .await
        .map_err(|_| McpCredentialProbeError::Unavailable)?;
    let result = async {
        if initial_tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>()
            != control_tool_names
        {
            return Err(McpCredentialProbeError::Unavailable);
        }
        if !initial_tools
            .iter()
            .any(|tool| tool.name == probe.tool_name)
        {
            let activation = tool_activation
                .as_ref()
                .ok_or(McpCredentialProbeError::Unavailable)?;
            if !initial_tools
                .iter()
                .any(|tool| tool.name == activation.tool_name)
            {
                return Err(McpCredentialProbeError::Unavailable);
            }
            let activation_arguments = Value::Object(serde_json::Map::from_iter([(
                activation.argument_name.clone(),
                json!([probe.tool_name.clone()]),
            )]));
            let activation_result = session
                .call_tool(
                    &activation.tool_name,
                    activation_arguments,
                    MCP_CREDENTIAL_PROBE_TIMEOUT,
                )
                .await
                .map_err(|_| McpCredentialProbeError::Unavailable)?;
            if activation_result.is_error {
                return Err(McpCredentialProbeError::Unavailable);
            }
        }
        let listed = session
            .list_tools()
            .await
            .map_err(|_| McpCredentialProbeError::Unavailable)?;
        let tool = listed
            .iter()
            .find(|tool| tool.name == probe.tool_name)
            .ok_or(McpCredentialProbeError::Unavailable)?;
        let filtered = filter_tool_providers(tool, &enabled_provider_ids, false)
            .map_err(|_| McpCredentialProbeError::Unavailable)?
            .ok_or(McpCredentialProbeError::Unavailable)?;
        let requested_provider = probe
            .arguments
            .get("provider")
            .and_then(|value| value.as_str());
        if requested_provider.is_some_and(|provider| !enabled_provider_ids.contains(provider)) {
            return Err(McpCredentialProbeError::Unavailable);
        }
        if filtered
            .input_schema
            .get("properties")
            .and_then(|value| value.get("provider"))
            .is_some()
            && requested_provider.is_none()
        {
            return Err(McpCredentialProbeError::Unavailable);
        }
        let response = session
            .call_tool(
                &probe.tool_name,
                probe.arguments.clone(),
                MCP_CREDENTIAL_PROBE_TIMEOUT,
            )
            .await
            .map_err(|error| match error {
                McpError::Remote(_) => McpCredentialProbeError::Rejected,
                _ => McpCredentialProbeError::Unavailable,
            })?;
        let encoded =
            serde_json::to_value(&response).map_err(|_| McpCredentialProbeError::Unavailable)?;
        if contains_protected_value(&encoded, &sensitive_values) {
            return Err(McpCredentialProbeError::Unavailable);
        }
        if response.is_error {
            return Err(McpCredentialProbeError::Rejected);
        }
        validate_result_provider(
            &response,
            &provider_receipt_pointer,
            requested_provider,
            &enabled_provider_ids,
        )
        .map_err(|_| McpCredentialProbeError::Unavailable)
    }
    .await;
    let shutdown = session.shutdown(MCP_CREDENTIAL_SHUTDOWN_GRACE).await;
    if result.is_ok() && shutdown.is_err() {
        return Err(McpCredentialProbeError::Unavailable);
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
pub async fn marketplace_credential_verify(
    request: MarketplaceCredentialRequest,
    state: State<'_, AppState>,
) -> Result<Vec<MarketplaceCredentialStatusDto>, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::MarketplaceCredential)?;
    let catalog = bundled_catalog()?;
    ensure_installable(&request.entry_id)?;
    let entry = credential_entry(&catalog, &request.entry_id)?;
    let (candidate, candidate_secrets, verification_secrets, provider_context, scope_snapshot) = {
        let _lifecycle_guard = connector_lifecycle_lock()?;
        let Some(candidate) = crate::finance_credentials::candidate(&request.entry_id)
            .map_err(|error| CommandError::internal(error.to_string()))?
        else {
            let statuses = credential_statuses(entry)?;
            return if statuses.iter().all(|status| status.active) {
                Ok(statuses)
            } else {
                Err(CommandError::conflict(
                    "connector credentials must be saved first",
                ))
            };
        };
        let required_ids = entry
            .setup
            .as_ref()
            .map(|setup| {
                setup
                    .credential_fields
                    .iter()
                    .filter(|field| field.required)
                    .map(|field| field.id.as_str())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let candidate_secrets = candidate.secrets().clone();
        let verification_secrets = candidate
            .secrets()
            .iter()
            .filter(|(credential_id, _)| required_ids.contains(credential_id.as_str()))
            .map(|(credential_id, secret)| (credential_id.clone(), secret.clone()))
            .collect::<BTreeMap<_, _>>();
        let provider_context = credential_provider_context(&state, &request.entry_id)?;
        let scope_snapshot = credential_scope_snapshot(&state, entry)?;
        (
            candidate,
            candidate_secrets,
            verification_secrets,
            provider_context,
            scope_snapshot,
        )
    };
    let revision = candidate.revision().to_owned();
    if entry.runtime.kind == MarketplaceRuntimeKind::BundledMcp {
        let result = verify_bundled_mcp_credential(&state, entry, &candidate_secrets).await;
        drop(candidate);
        let outcome = match result {
            Ok(()) => crate::finance_credentials::VerificationOutcome::Verified,
            Err(McpCredentialProbeError::Rejected) => {
                crate::finance_credentials::VerificationOutcome::Rejected
            }
            Err(McpCredentialProbeError::Unavailable) => {
                crate::finance_credentials::VerificationOutcome::TemporarilyUnavailable
            }
        };
        let _lifecycle_guard = connector_lifecycle_lock()?;
        if credential_scope_snapshot(&state, entry)? != scope_snapshot {
            return Err(CommandError::conflict(
                "connector configuration changed while credential verification was in progress",
            ));
        }
        let finished = crate::finance_credentials::finish_verification(
            &request.entry_id,
            &revision,
            outcome,
            Utc::now().timestamp_millis(),
        )
        .map_err(|error| CommandError::internal(error.to_string()))?;
        if finished == crate::finance_credentials::FinishVerification::Stale {
            return Err(CommandError::conflict(
                "connector credential changed while verification was in progress",
            ));
        }
        return match result {
            Ok(()) => credential_statuses(entry),
            Err(McpCredentialProbeError::Rejected) => Err(CommandError::new(
                "credential_rejected",
                "The provider rejected these credentials.",
            )),
            Err(McpCredentialProbeError::Unavailable) => Err(CommandError::new(
                "provider_unavailable",
                "The provider could not verify these credentials right now.",
            )),
        };
    }
    let result = state
        .finance_data
        .verify_credential(
            &request.entry_id,
            &verification_secrets,
            provider_context.as_deref(),
        )
        .await;
    drop(candidate);
    let outcome = credential_verification_outcome(&result);
    let _lifecycle_guard = connector_lifecycle_lock()?;
    if credential_provider_context(&state, &request.entry_id)? != provider_context
        || credential_scope_snapshot(&state, entry)? != scope_snapshot
    {
        return Err(CommandError::conflict(
            "connector configuration changed while credential verification was in progress",
        ));
    }
    let finished = crate::finance_credentials::finish_verification(
        &request.entry_id,
        &revision,
        outcome,
        Utc::now().timestamp_millis(),
    )
    .map_err(|error| CommandError::internal(error.to_string()))?;
    if finished == crate::finance_credentials::FinishVerification::Stale {
        return Err(CommandError::conflict(
            "connector credential changed while verification was in progress",
        ));
    }
    if let Err(error) = result {
        return Err(credential_verification_error(&error));
    }
    credential_statuses(entry)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn marketplace_credential_delete(
    request: MarketplaceCredentialRequest,
    state: State<'_, AppState>,
) -> Result<Vec<MarketplaceCredentialStatusDto>, CommandError> {
    let entry_id = request.entry_id.clone();
    let result = delete_credential_for_state(request, &state);
    if entry_id == crate::finance_data::KIS_SOURCE_ID {
        state.finance_data.clear_kis_token_cache().await;
    }
    result
}

pub(super) fn delete_credential_for_state(
    request: MarketplaceCredentialRequest,
    state: &AppState,
) -> Result<Vec<MarketplaceCredentialStatusDto>, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::MarketplaceCredential)?;
    let catalog = bundled_catalog()?;
    ensure_installable(&request.entry_id)?;
    let entry = credential_entry(&catalog, &request.entry_id)?;
    let lifecycle_guard = connector_lifecycle_lock()?;
    disable_bindings_and_delete_credentials_locked(state, &request.entry_id, &lifecycle_guard)?;
    credential_statuses(entry)
}

pub(super) fn disable_bindings_and_delete_credentials_locked(
    state: &AppState,
    entry_id: &str,
    _lifecycle_guard: &MutexGuard<'static, ()>,
) -> Result<(), CommandError> {
    // Disable every Guru binding before deleting the credential. If either
    // persistence boundary fails, no successful deletion can leave an enabled
    // binding that appears executable.
    for guru in state
        .store
        .list_gurus()
        .map_err(|error| CommandError::internal(error.to_string()))?
    {
        let Some(mut binding) = state
            .store
            .get_guru_capability(&guru.id, entry_id)
            .map_err(|error| CommandError::internal(error.to_string()))?
        else {
            continue;
        };
        if binding.enabled || !binding.granted_permissions.is_empty() {
            binding.enabled = false;
            binding.granted_permissions.clear();
            binding.updated_at_ms = Utc::now()
                .timestamp_millis()
                .max(binding.updated_at_ms.saturating_add(1));
            state
                .store
                .save_guru_capability(&binding)
                .map_err(|error| CommandError::internal(error.to_string()))?;
        }
    }
    crate::finance_credentials::delete(entry_id)
        .map_err(|error| CommandError::internal(error.to_string()))
}
