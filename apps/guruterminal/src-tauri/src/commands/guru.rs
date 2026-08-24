use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use rfd::AsyncFileDialog;
use tauri::State;

use crate::{
    agent_harness::{self, AgentSkillSummary},
    app::{AppState, CommandError, GuruAccess, GuruRecoveryAction, QuarantineSource},
    artifact_trust::{create_private_directory, ensure_private_directory},
    deletion,
    domain::{GuruCapabilityBinding, GuruProfile, GuruStorageKind},
    guru_root::{profile_workspace, BoundGuruRoot},
    hashing::sha256,
    maintenance::MaintenanceActivityKind,
    store::{GuruTerminalStore, StoreError},
};

use super::{
    enabled_skill_ids, map_internal, map_store, memory_write, new_id, now_ms, profile_summary,
    profile_summary_at, profile_summary_with_availability, require_text,
    types::{
        chat_dto, AgentSkillsUpdateRequest, GuruCreateRequest, GuruDeleteRequest,
        GuruExportReceipt, GuruRecoverRequest, GuruRenameRequest, GuruSummary, GuruWorkspace,
    },
};

#[tauri::command(rename_all = "snake_case")]
pub async fn guru_list(state: State<'_, AppState>) -> Result<Vec<GuruSummary>, CommandError> {
    list_available_gurus(&state).await
}

pub(super) async fn recover_guru_inner(
    state: &AppState,
    request: &GuruRecoverRequest,
) -> Result<GuruSummary, CommandError> {
    let _recovery = state.register_guru_recovery(
        new_id("guru-recovery"),
        request.guru_id.clone(),
        request.action,
    )?;
    match request.action {
        GuruRecoveryAction::RecoverMemory => {
            memory_write::retry_quarantined_guru_recovery(state, &request.guru_id).await?
        }
    }
    let profile = state
        .store
        .get_guru(&request.guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    profile_summary(state, &profile).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn guru_recover(
    request: GuruRecoverRequest,
    state: State<'_, AppState>,
) -> Result<GuruSummary, CommandError> {
    recover_guru_inner(&state, &request).await
}

pub(crate) async fn bootstrap_default_guru(state: &AppState) -> Result<(), CommandError> {
    if state.store.list_gurus().map_err(map_store)?.is_empty() {
        create_managed_guru(state, "My Agent".to_owned(), None).await?;
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn agent_skill_catalog(
    guru_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<AgentSkillSummary>, CommandError> {
    state.ensure_guru_available(&guru_id)?;
    state
        .store
        .get_guru(&guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let enabled = enabled_skill_ids(state.store.as_ref(), &guru_id)?;
    let mut catalog = agent_harness::skill_catalog(&enabled);
    let user_skills = state
        .store
        .list_user_skills_for_guru(&guru_id)
        .map_err(map_store)?;
    for skill in user_skills {
        skill.validate().map_err(map_internal)?;
        let binding_id = agent_harness::user_skill_binding_id(&skill.id).map_err(map_internal)?;
        let enabled = state
            .store
            .get_guru_capability(&guru_id, &binding_id)
            .map_err(map_store)?
            .is_some_and(|binding| binding.enabled && binding.granted_permissions == ["load"]);
        catalog.push(AgentSkillSummary {
            id: crate::user_skill::skill_slug(&skill.id)
                .map_err(map_internal)?
                .to_owned(),
            name: skill.name,
            description: skill.description,
            enabled,
            ownership: "user".into(),
            editable: true,
            current_revision_id: Some(skill.current_revision_id),
        });
    }
    Ok(catalog)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agent_skills_update(
    request: AgentSkillsUpdateRequest,
    state: State<'_, AppState>,
) -> Result<GuruSummary, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::GuruMutation)?;
    state.ensure_guru_available(&request.guru_id)?;
    let profile = state
        .store
        .get_guru(&request.guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let selected = request.skill_ids.iter().cloned().collect::<BTreeSet<_>>();
    if selected.len() != request.skill_ids.len() {
        return Err(CommandError::invalid("agent skill selection is invalid"));
    }
    let bundled_ids = agent_harness::default_skill_ids();
    let user_skills = state
        .store
        .list_user_skills_for_guru(&profile.id)
        .map_err(map_store)?;
    for skill in &user_skills {
        skill.validate().map_err(map_internal)?;
    }
    let user_ids = user_skills
        .iter()
        .map(|skill| crate::user_skill::skill_slug(&skill.id).map(str::to_owned))
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(map_internal)?;
    if selected
        .iter()
        .any(|id| !bundled_ids.iter().any(|bundled| bundled == id) && !user_ids.contains(id))
    {
        return Err(CommandError::invalid("agent skill selection is invalid"));
    }
    let timestamp = now_ms().max(profile.updated_at_ms);
    for id in bundled_ids {
        let entry_id = agent_harness::skill_binding_id(&id).map_err(map_internal)?;
        let previous = state
            .store
            .get_guru_capability(&profile.id, &entry_id)
            .map_err(map_store)?;
        let binding = GuruCapabilityBinding {
            guru_id: profile.id.clone(),
            entry_id,
            enabled: selected.contains(&id),
            granted_permissions: vec!["load".into()],
            config: previous.map(|binding| binding.config).unwrap_or_default(),
            updated_at_ms: timestamp,
        };
        state
            .store
            .save_guru_capability(&binding)
            .map_err(map_store)?;
    }
    for skill in user_skills {
        let slug = crate::user_skill::skill_slug(&skill.id).map_err(map_internal)?;
        let entry_id = agent_harness::user_skill_binding_id(&skill.id).map_err(map_internal)?;
        let binding = GuruCapabilityBinding {
            guru_id: profile.id.clone(),
            entry_id,
            enabled: selected.contains(slug),
            granted_permissions: vec!["load".into()],
            config: BTreeMap::from([("skill_id".into(), skill.id)]),
            updated_at_ms: timestamp,
        };
        state
            .store
            .save_guru_capability(&binding)
            .map_err(map_store)?;
    }
    profile_summary(state.inner(), &profile).await
}

pub(super) async fn list_available_gurus(
    state: &AppState,
) -> Result<Vec<GuruSummary>, CommandError> {
    let profiles = state.store.list_gurus().map_err(map_store)?;
    let mut summaries = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let availability = match state.guru_access(&profile.id) {
            GuruAccess::Visible(availability) => availability,
            GuruAccess::Hidden => continue,
        };
        match profile_summary_with_availability(state, &profile, availability).await {
            Ok(summary) => summaries.push(summary),
            // A moved or replaced root must never prevent the user from opening
            // other Gurus or reaching the explicit Import action.
            Err(error)
                if matches!(error.code.as_str(), "conflict" | "guru_storage_unavailable") =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(summaries)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn guru_select(
    guru_id: String,
    state: State<'_, AppState>,
) -> Result<GuruWorkspace, CommandError> {
    state.ensure_guru_available(&guru_id)?;
    let profile = state
        .store
        .get_guru(&guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let workspace = profile_workspace(&profile)?;
    let runtime = state.runtime()?;
    workspace.validate(&runtime).await.map_err(map_internal)?;
    let chats = state
        .store
        .list_chats_for_guru(&profile.id)
        .map_err(map_store)?;
    let threads = chats
        .iter()
        .filter_map(|chat| match chat_dto(chat) {
            Ok(thread) => Some(thread),
            Err(error) => {
                eprintln!(
                    "Guru Terminal skipped unreadable Chat {} ({error})",
                    chat.id
                );
                None
            }
        })
        .collect();
    Ok(GuruWorkspace {
        guru: profile_summary_at(&state, &profile, &workspace).await?,
        threads,
    })
}

pub(super) fn managed_guru_dir(state: &AppState, guru_id: &str) -> Result<PathBuf, CommandError> {
    if guru_id.is_empty()
        || !guru_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(CommandError::invalid("Guru id is unsafe"));
    }
    state
        .artifacts
        .deletion_root
        .absolute_path(&PathBuf::from("gurus").join(guru_id))
}

pub(super) fn write_new_memory_file(path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
    let parent = path
        .parent()
        .ok_or_else(|| CommandError::internal("memory target has no parent"))?;
    ensure_private_directory(parent).map_err(map_internal)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = options.open(path).map_err(map_internal)?;
    file.write_all(bytes).map_err(map_internal)?;
    file.sync_all().map_err(map_internal)
}

pub(super) fn copy_memory_records(
    source: &BoundGuruRoot,
    destination: &BoundGuruRoot,
) -> Result<(String, usize), CommandError> {
    let records = source.inspect_memory_tree().map_err(map_internal)?.1;
    for record in &records {
        let relative = Path::new(&record.relative_path);
        let source_relative = Path::new("guruterminal").join(relative);
        let bytes = source
            .read_memory_record(&source_relative)
            .map_err(map_internal)?
            .ok_or_else(|| CommandError::conflict("memory changed during transfer"))?;
        if sha256(&bytes) != record.content_sha256 {
            return Err(CommandError::conflict("memory changed during transfer"));
        }
        write_new_memory_file(
            &destination.path().join("guruterminal").join(relative),
            &bytes,
        )?;
    }
    let commit_id = crate::memory_git::commit_memory(destination.path(), "user: import memory")
        .map_err(map_internal)?;
    Ok((commit_id, records.len()))
}

pub(super) async fn create_managed_guru(
    state: &AppState,
    name: String,
    source: Option<&BoundGuruRoot>,
) -> Result<GuruSummary, CommandError> {
    let name = require_text(&name, "Guru name", 80)?;
    let guru_id = new_id("guru");
    let guru_relative = PathBuf::from("gurus").join(&guru_id);
    let guru_guard = state
        .artifacts
        .deletion_root
        .ensure_private_subdirectory(&guru_relative)?;
    let guru_identity = match state
        .artifacts
        .deletion_root
        .directory_identity(&guru_relative)
    {
        Ok(Some(identity)) => identity,
        Ok(None) => {
            drop(guru_guard);
            let _ = state.artifacts.deletion_root.remove_tree(&guru_relative);
            return Err(CommandError::internal("new Guru root identity is missing"));
        }
        Err(error) => {
            drop(guru_guard);
            let _ = state.artifacts.deletion_root.remove_tree(&guru_relative);
            return Err(error);
        }
    };
    let guru_dir = state
        .artifacts
        .deletion_root
        .absolute_path(&guru_relative)?;
    let workspace_path = guru_dir.join("workspace");
    if let Err(error) = create_private_directory(&workspace_path) {
        return Err(cleanup_failed_guru_creation(
            state,
            &guru_relative,
            &guru_identity,
            guru_guard,
            map_internal(error),
        ));
    }
    if let Err(error) = create_private_directory(&guru_dir.join("workbench")) {
        return Err(cleanup_failed_guru_creation(
            state,
            &guru_relative,
            &guru_identity,
            guru_guard,
            map_internal(error),
        ));
    }
    let workspace = match BoundGuruRoot::open_unbound(workspace_path) {
        Ok(workspace) => workspace,
        Err(error) => {
            return Err(cleanup_failed_guru_creation(
                state,
                &guru_relative,
                &guru_identity,
                guru_guard,
                error,
            ));
        }
    };
    let runtime = match state.runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            drop(workspace);
            return Err(cleanup_failed_guru_creation(
                state,
                &guru_relative,
                &guru_identity,
                guru_guard,
                error,
            ));
        }
    };
    if let Err(error) = workspace.initialize(&runtime).await {
        drop(workspace);
        return Err(cleanup_failed_guru_creation(
            state,
            &guru_relative,
            &guru_identity,
            guru_guard,
            map_internal(error),
        ));
    }
    if let Some(source) = source {
        if let Err(error) = copy_memory_records(source, &workspace) {
            drop(workspace);
            return Err(cleanup_failed_guru_creation(
                state,
                &guru_relative,
                &guru_identity,
                guru_guard,
                error,
            ));
        }
        if let Err(error) = workspace.validate(&runtime).await {
            drop(workspace);
            return Err(cleanup_failed_guru_creation(
                state,
                &guru_relative,
                &guru_identity,
                guru_guard,
                map_internal(error),
            ));
        }
    }
    let timestamp = now_ms();
    let description = "An investment Guru refined through research and reflection".to_owned();
    let profile = GuruProfile {
        id: guru_id.clone(),
        name,
        description: description.clone(),
        storage_kind: GuruStorageKind::Managed,
        memory_root: workspace.path().to_string_lossy().into_owned(),
        root_filesystem_identity: workspace.identity(),
        last_model_profile_id: None,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    if let Err(error) = state
        .store
        .create_guru(&profile)
        .map_err(|error| match error {
            StoreError::Conflict(_) => {
                CommandError::conflict("this Guru root is already registered")
            }
            other => map_store(other),
        })
    {
        drop(workspace);
        return Err(cleanup_failed_guru_creation(
            state,
            &guru_relative,
            &guru_identity,
            guru_guard,
            error,
        ));
    }
    drop(guru_guard);
    profile_summary_at(state, &profile, &workspace).await
}

pub(super) fn cleanup_failed_guru_creation(
    state: &AppState,
    relative: &Path,
    expected_identity: &crate::domain::RootFilesystemIdentity,
    guard: crate::secure_delete::PrivateDirectoryGuard,
    primary: CommandError,
) -> CommandError {
    drop(guard);
    match state
        .artifacts
        .deletion_root
        .remove_tree_expected(relative, Some(expected_identity))
    {
        Ok(()) => primary,
        Err(cleanup) => CommandError::internal(format!(
            "Guru creation failed and its private root could not be cleaned: {}; cleanup: {}",
            primary.message, cleanup.message
        )),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn guru_create(
    request: GuruCreateRequest,
    state: State<'_, AppState>,
) -> Result<GuruSummary, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::GuruMutation)?;
    create_managed_guru(state.inner(), request.name, None).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn guru_import_memory(
    state: State<'_, AppState>,
) -> Result<Option<GuruSummary>, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::GuruTransfer)?;
    #[cfg(feature = "e2e")]
    let e2e_folder = std::env::var_os("GURUTERMINAL_E2E_IMPORT_DIR").map(PathBuf::from);
    #[cfg(not(feature = "e2e"))]
    let e2e_folder: Option<PathBuf> = None;
    if e2e_folder.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(CommandError::invalid(
            "GURUTERMINAL_E2E_IMPORT_DIR must be absolute",
        ));
    }
    let Some(folder) = (match e2e_folder {
        Some(folder) => Some(folder),
        None => AsyncFileDialog::new()
            .set_title("Import Guru Memory")
            .pick_folder()
            .await
            .map(|folder| folder.path().to_path_buf()),
    }) else {
        return Ok(None);
    };
    let workspace = folder
        .canonicalize()
        .map_err(|error| CommandError::invalid(error.to_string()))?;
    let source = BoundGuruRoot::open_unbound(workspace)?;
    let runtime = state.runtime()?;
    source.validate(&runtime).await.map_err(map_internal)?;
    let name = source
        .path()
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Imported Guru")
        .to_owned();
    create_managed_guru(state.inner(), name, Some(&source))
        .await
        .map(Some)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn guru_export_memory(
    guru_id: String,
    state: State<'_, AppState>,
) -> Result<Option<GuruExportReceipt>, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::GuruTransfer)?;
    state.ensure_guru_available(&guru_id)?;
    let profile = state
        .store
        .get_guru(&guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let source = profile_workspace(&profile)?;
    let Some(folder) = AsyncFileDialog::new()
        .set_title("Export Guru Memory")
        .pick_folder()
        .await
    else {
        return Ok(None);
    };
    if folder
        .path()
        .read_dir()
        .map_err(map_internal)?
        .next()
        .is_some()
    {
        return Err(CommandError::conflict("export destination must be empty"));
    }
    let destination_path = folder.path().canonicalize().map_err(map_internal)?;
    let destination = BoundGuruRoot::open_unbound(destination_path)?;
    let runtime = state.runtime()?;
    destination
        .initialize(&runtime)
        .await
        .map_err(map_internal)?;
    let (memory_revision, record_count) = copy_memory_records(&source, &destination)?;
    destination.validate(&runtime).await.map_err(map_internal)?;
    Ok(Some(GuruExportReceipt {
        guru_id,
        record_count,
        memory_revision,
    }))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn guru_rename(
    request: GuruRenameRequest,
    state: State<'_, AppState>,
) -> Result<GuruSummary, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::GuruMutation)?;
    state.ensure_guru_available(&request.guru_id)?;
    let name = require_text(&request.name, "Guru name", 80)?;
    let profile = state
        .store
        .rename_guru(&request.guru_id, &name, now_ms())
        .map_err(map_store)?;
    profile_summary(state.inner(), &profile).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn guru_delete(
    request: GuruDeleteRequest,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    delete_guru_inner(state.inner(), &request.guru_id).await
}

pub(super) async fn delete_guru_inner(state: &AppState, guru_id: &str) -> Result<(), CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::GuruDeletion)?;
    state.ensure_guru_available(guru_id)?;
    let profile = state
        .store
        .get_guru(guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let guru_dir = managed_guru_dir(state, &profile.id)?;
    let expected_workspace = guru_dir.join("workspace");
    if profile.storage_kind != GuruStorageKind::Managed
        || Path::new(&profile.memory_root) != expected_workspace
    {
        return Err(CommandError::conflict(
            "Guru storage is not owned by this Guru Terminal installation",
        ));
    }
    profile_workspace(&profile)?;
    let metadata = fs::symlink_metadata(&guru_dir).map_err(map_internal)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CommandError::conflict("Guru storage boundary is invalid"));
    }
    let gurus_root = state.artifacts.app_data_dir.join("gurus");
    let canonical_gurus_root = gurus_root.canonicalize().map_err(map_internal)?;
    let canonical_guru_dir = guru_dir.canonicalize().map_err(map_internal)?;
    if canonical_guru_dir.parent() != Some(canonical_gurus_root.as_path()) {
        return Err(CommandError::conflict(
            "Guru storage is outside the app-owned Guru directory",
        ));
    }

    state.begin_guru_deletion(guru_id)?;
    if let Err(error) = deletion::delete_guru(
        state.store.as_ref(),
        state.artifacts.deletion_root.as_ref(),
        &profile,
        now_ms(),
    ) {
        if !deletion::has_pending_for(state.store.as_ref(), guru_id, guru_id).unwrap_or(true) {
            state.clear_guru_quarantine(guru_id, QuarantineSource::Deletion);
        }
        return Err(error);
    }
    state.forget_guru(&profile.id).await;
    Ok(())
}
