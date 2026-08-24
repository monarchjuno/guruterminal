use std::collections::BTreeSet;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::Value;
use tauri::State;
use uuid::Uuid;

use crate::{
    app::{AppState, CommandError, QuarantineSource},
    chat_artifacts::{ChatArtifact, ChatArtifactPayload},
    deletion,
    domain::{ChatSession, MemoryPolicy},
    guru_root::profile_workspace,
    maintenance::MaintenanceActivityKind,
    run_coordinator::{RunKind, RunTarget},
    store::GuruTerminalStore,
};

use super::{
    attachments::{chat_attachment_message_dir, read_chat_attachment_file},
    chat_runtime::parse_memory_kind,
    json_text, map_internal, map_store, new_id, now_ms, require_text,
    types::{
        chat_dto, runtime_record_summary, ChatArtifactListRequest, ChatArtifactReadRequest,
        ChatArtifactViewDto, ChatAttachmentReadDto, ChatAttachmentReadRequest, ChatCreateRequest,
        ChatDeleteRequest, ChatRenameRequest, ChatThreadDto, LibraryRecordDto, LibraryRelationDto,
        LibrarySearchRequest, LibrarySummaryDto,
    },
    MAX_CHAT_TITLE_BYTES,
};

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_create(
    request: ChatCreateRequest,
    state: State<'_, AppState>,
) -> Result<ChatThreadDto, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::ChatMutation)?;
    state.ensure_guru_available(&request.guru_id)?;
    let profile = state
        .store
        .get_guru(&request.guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    profile_workspace(&profile)?;
    let title = match request.title {
        Some(title) => require_text(&title, "chat title", MAX_CHAT_TITLE_BYTES)?,
        None => "New chat".into(),
    };
    let timestamp = now_ms();
    let chat = ChatSession {
        id: new_id("chat"),
        guru_id: profile.id,
        pi_session_id: Uuid::new_v4().to_string(),
        pi_session_cache: None,
        title,
        memory_policy: MemoryPolicy::default(),
        messages: Vec::new(),
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    state.store.create_chat(&chat).map_err(map_store)?;
    chat_dto(&chat)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_rename(
    request: ChatRenameRequest,
    state: State<'_, AppState>,
) -> Result<ChatThreadDto, CommandError> {
    let _mutation = state.register_run(
        new_id("chat-mutation"),
        request.guru_id.clone(),
        RunKind::ChatMutation,
        RunTarget::ChatThread(request.thread_id.clone()),
    )?;
    let expected = artifact_chat(&state, &request.guru_id, &request.thread_id)?;
    let mut chat = expected.clone();
    chat.title = require_text(&request.title, "chat title", MAX_CHAT_TITLE_BYTES)?;
    chat.updated_at_ms = now_ms().max(chat.updated_at_ms.saturating_add(1));
    state
        .store
        .replace_chat(&expected, &chat)
        .map_err(map_store)?;
    chat_dto(&chat)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_delete(
    request: ChatDeleteRequest,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let _mutation = state.register_run(
        new_id("chat-mutation"),
        request.guru_id.clone(),
        RunKind::ChatMutation,
        RunTarget::ChatThread(request.thread_id.clone()),
    )?;
    let expected = artifact_chat(&state, &request.guru_id, &request.thread_id)?;
    if let Err(error) = deletion::delete_chat(
        state.store.as_ref(),
        state.artifacts.deletion_root.as_ref(),
        &expected,
        now_ms(),
    ) {
        if state
            .store
            .get_chat(&expected.id)
            .map_err(map_store)?
            .is_some()
            && deletion::has_pending_for(state.store.as_ref(), &expected.guru_id, &expected.id)
                .unwrap_or(true)
        {
            state.quarantine_guru(
                &expected.guru_id,
                QuarantineSource::Deletion,
                "Chat deletion recovery is required before this Guru can be used",
            );
        }
        return Err(error);
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn chat_attachment_read(
    request: ChatAttachmentReadRequest,
    state: State<'_, AppState>,
) -> Result<ChatAttachmentReadDto, CommandError> {
    let chat = artifact_chat(&state, &request.guru_id, &request.thread_id)?;
    let message = chat
        .messages
        .iter()
        .find(|message| message.id == request.message_id)
        .ok_or_else(|| CommandError::not_found("chat message"))?;
    let attachment = message
        .attachments
        .iter()
        .find(|attachment| attachment.id == request.attachment_id)
        .ok_or_else(|| CommandError::not_found("chat attachment"))?;
    let path = chat_attachment_message_dir(
        &state,
        &request.guru_id,
        &request.thread_id,
        &request.message_id,
    )?
    .join(&attachment.id);
    let bytes = read_chat_attachment_file(&path, attachment)?;
    Ok(ChatAttachmentReadDto {
        data_url: format!(
            "data:{};base64,{}",
            attachment.media_type,
            BASE64.encode(bytes)
        ),
    })
}

fn artifact_chat(
    state: &AppState,
    guru_id: &str,
    thread_id: &str,
) -> Result<ChatSession, CommandError> {
    state.ensure_guru_available(guru_id)?;
    let chat = state
        .store
        .get_chat(thread_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("chat thread"))?;
    if chat.guru_id != guru_id {
        return Err(CommandError::conflict(
            "chat thread does not belong to the selected Guru",
        ));
    }
    Ok(chat)
}

#[tauri::command(rename_all = "snake_case")]
pub fn chat_artifact_list(
    request: ChatArtifactListRequest,
    state: State<'_, AppState>,
) -> Result<Vec<ChatArtifact>, CommandError> {
    artifact_chat(&state, &request.guru_id, &request.thread_id)?;
    state
        .store
        .list_chat_artifacts(&request.thread_id)
        .map_err(map_store)
}

#[tauri::command(rename_all = "snake_case")]
pub fn chat_artifact_read(
    request: ChatArtifactReadRequest,
    state: State<'_, AppState>,
) -> Result<ChatArtifactViewDto, CommandError> {
    artifact_chat(&state, &request.guru_id, &request.thread_id)?;
    let artifact = state
        .store
        .get_chat_artifact(&request.artifact_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("artifact"))?;
    if artifact.chat_session_id != request.thread_id {
        return Err(CommandError::conflict(
            "artifact does not belong to this Chat thread",
        ));
    }
    let revision = state
        .store
        .get_chat_artifact_current(&artifact.id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("artifact content"))?;
    let chart_dataset = match &revision.payload {
        ChatArtifactPayload::Chart { chart, .. } => {
            let dataset = state
                .store
                .get_chart_dataset(&chart.dataset_id)
                .map_err(map_store)?
                .ok_or_else(|| CommandError::not_found("chart dataset"))?;
            chart.validate_dataset(&dataset).map_err(map_internal)?;
            Some(dataset)
        }
        ChatArtifactPayload::Markdown { .. } => None,
    };
    Ok(ChatArtifactViewDto {
        artifact,
        revision,
        chart_dataset,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn library_search(
    request: LibrarySearchRequest,
    state: State<'_, AppState>,
) -> Result<Vec<LibrarySummaryDto>, CommandError> {
    state.ensure_guru_available(&request.guru_id)?;
    let profile = state
        .store
        .get_guru(&request.guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let workspace = profile_workspace(&profile)?;
    let runtime = state.runtime()?;
    let kinds = request
        .kinds
        .unwrap_or_default()
        .into_iter()
        .map(|kind| parse_memory_kind(&kind))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let query = request.query.trim();
    if query.len() > 4_096 {
        return Err(CommandError::invalid("library query is too long"));
    }
    let value = if query.is_empty() {
        workspace
            .knowledge_list(&runtime, None)
            .await
            .map_err(map_internal)?
    } else {
        workspace
            .knowledge_search(&runtime, query, None, 20, true, None)
            .await
            .map_err(map_internal)?
    };
    let records = value
        .as_array()
        .ok_or_else(|| CommandError::internal("Runtime list result is not an array"))?;
    records
        .iter()
        .filter(|record| {
            kinds.is_empty()
                || record
                    .get("kind")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kinds.contains(kind))
        })
        .map(runtime_record_summary)
        .collect()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn library_read(
    guru_id: String,
    record_id: String,
    state: State<'_, AppState>,
) -> Result<LibraryRecordDto, CommandError> {
    state.ensure_guru_available(&guru_id)?;
    let profile = state
        .store
        .get_guru(&guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let workspace = profile_workspace(&profile)?;
    let runtime = state.runtime()?;
    let value = workspace
        .knowledge_read(&runtime, &record_id, None)
        .await
        .map_err(map_internal)?;
    let document = value
        .get("document")
        .ok_or_else(|| CommandError::internal("Runtime read result is missing document"))?;
    if json_text(document, "id")? != record_id {
        return Err(CommandError::internal(
            "Runtime returned a different record",
        ));
    }
    let summary = runtime_record_summary(document)?;
    let markdown = value
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::internal("Runtime read result is missing content"))?
        .to_owned();
    let mut authored_relationships = document
        .get("relationships")
        .and_then(Value::as_array)
        .map(|relationships| {
            relationships
                .iter()
                .map(|relationship| {
                    let relation = json_text(relationship, "type")?;
                    if !matches!(relation, "uses" | "supports" | "updates" | "contradicts") {
                        return Err(CommandError::internal(
                            "Runtime returned an unknown relationship",
                        ));
                    }
                    Ok((
                        relation.to_owned(),
                        json_text(relationship, "target")?.to_owned(),
                    ))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let see_also = document
        .get("see_also")
        .and_then(Value::as_array)
        .ok_or_else(|| CommandError::internal("Runtime document is missing see_also"))?;
    authored_relationships.extend(
        see_also
            .iter()
            .map(|target| {
                target
                    .as_str()
                    .filter(|target| !target.is_empty())
                    .map(|target| ("see_also".to_owned(), target.to_owned()))
                    .ok_or_else(|| CommandError::internal("Runtime returned an invalid see_also"))
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mut relationships = Vec::with_capacity(authored_relationships.len());
    for (relation, target_id) in authored_relationships {
        let resolved_title = match workspace.knowledge_read(&runtime, &target_id, None).await {
            Ok(value) => value
                .get("document")
                .filter(|document| {
                    document.get("id").and_then(Value::as_str) == Some(target_id.as_str())
                })
                .and_then(|document| document.get("title"))
                .and_then(Value::as_str)
                .filter(|title| !title.is_empty())
                .map(str::to_owned),
            Err(_) => None,
        };
        relationships.push(LibraryRelationDto {
            relation,
            target_id: target_id.clone(),
            target_title: resolved_title.clone().unwrap_or(target_id),
            target_title_source: if resolved_title.is_some() {
                "record"
            } else {
                "record_id_fallback"
            }
            .into(),
        });
    }
    Ok(LibraryRecordDto {
        id: summary.id,
        kind: summary.kind,
        title: summary.title,
        excerpt: summary.excerpt,
        as_of: summary.as_of,
        status: summary.status,
        revoked_by: summary.revoked_by,
        markdown,
        relationships,
    })
}
