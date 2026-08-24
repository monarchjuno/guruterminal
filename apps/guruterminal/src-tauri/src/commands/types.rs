//! Renderer-facing DTOs shared by the command modules, plus their converters
//! from domain and runtime values.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    agent_harness::AgentHarnessSnapshot,
    app::{CommandError, GuruAvailability, GuruRecoveryAction},
    chart_engine::ChartDataset,
    chat_artifacts::{ChatArtifact, ChatArtifactRef, ChatArtifactRevision},
    chat_progress::ChatProgress,
    domain::{
        CanonicalMemoryKind, ChatDecision, ChatMessageStatus, ChatRole, ChatSession, MemoryAccess,
        MemoryRefSnapshot,
    },
    settings::ExecutionModelLock,
};

use super::{iso_time, json_text, map_internal};

#[derive(Clone, Debug, Serialize)]
pub struct GuruSummary {
    pub id: String,
    pub name: String,
    pub philosophy: String,
    pub record_count: usize,
    pub updated_at: String,
    pub accent: String,
    pub last_model_profile_id: Option<String>,
    pub enabled_skill_ids: Vec<String>,
    pub availability: GuruAvailability,
}

#[derive(Clone, Debug, Serialize)]
pub struct GuruWorkspace {
    pub guru: GuruSummary,
    pub threads: Vec<ChatThreadDto>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuruCreateRequest {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuruRenameRequest {
    pub guru_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuruDeleteRequest {
    pub guru_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuruRecoverRequest {
    pub guru_id: String,
    pub action: GuruRecoveryAction,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSkillsUpdateRequest {
    pub guru_id: String,
    pub skill_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GuruExportReceipt {
    pub guru_id: String,
    pub record_count: usize,
    pub memory_revision: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryRefDto {
    pub record_id: String,
    pub kind: String,
    pub title: String,
    pub excerpt: String,
    pub as_of: Option<String>,
    pub section: Option<String>,
    pub access: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatMessageDto {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
    pub status: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub memory_refs: Vec<MemoryRefDto>,
    pub observed_exact_count: u64,
    pub refs_truncated: bool,
    pub refs_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_update: Option<crate::domain::MemoryUpdateResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_model: Option<ExecutionModelLock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_harness: Option<AgentHarnessSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<ChatDecision>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ChatAttachmentDto>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ChatArtifactRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ChatProgress>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatAttachmentDto {
    pub id: String,
    pub filename: String,
    pub media_type: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatThreadDto {
    pub id: String,
    pub guru_id: String,
    pub title: String,
    pub updated_at: String,
    pub use_memory: bool,
    pub update_memory: bool,
    pub messages: Vec<ChatMessageDto>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatCreateRequest {
    pub guru_id: String,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatRenameRequest {
    pub guru_id: String,
    pub thread_id: String,
    pub title: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatDeleteRequest {
    pub guru_id: String,
    pub thread_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatSendRequest {
    pub run_id: String,
    pub guru_id: String,
    pub thread_id: String,
    pub prompt: String,
    pub use_memory: bool,
    pub update_memory: bool,
    #[serde(default)]
    pub as_of: Option<String>,
    pub model_profile_id: String,
    pub thinking_level: String,
    pub run_options: std::collections::BTreeMap<String, String>,
    pub attachments: Vec<ChatAttachmentInput>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatControlRequestDto {
    pub guru_id: String,
    pub thread_id: String,
    pub prompt: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatControlReceiptDto {
    pub message_id: String,
    pub prompt: String,
    pub created_at: String,
    pub mode: ChatControlModeDto,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatControlModeDto {
    Steer,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatAttachmentInput {
    pub filename: String,
    pub media_type: String,
    pub data_base64: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatAttachmentReadRequest {
    pub guru_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub attachment_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatAttachmentReadDto {
    pub data_url: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatArtifactListRequest {
    pub guru_id: String,
    pub thread_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatArtifactReadRequest {
    pub guru_id: String,
    pub thread_id: String,
    pub artifact_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatArtifactViewDto {
    pub artifact: ChatArtifact,
    pub revision: ChatArtifactRevision,
    pub chart_dataset: Option<ChartDataset>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatStreamEvent {
    Started {
        run_id: String,
    },
    Memory {
        run_id: String,
        memories: Vec<MemoryRefDto>,
    },
    Delta {
        run_id: String,
        text: String,
    },
    Title {
        run_id: String,
        title: String,
    },
    Progress {
        run_id: String,
        progress: ChatProgress,
    },
    MemoryUpdate {
        run_id: String,
        result: Box<crate::domain::MemoryUpdateResult>,
    },
    Decision {
        run_id: String,
        decision: ChatDecision,
    },
    Artifact {
        run_id: String,
        artifact: ChatArtifactRef,
    },
    Completed {
        run_id: String,
        message_id: String,
        final_text: String,
        created_at: String,
        execution_model: Box<ExecutionModelLock>,
        agent_harness: Box<AgentHarnessSnapshot>,
    },
    Aborted {
        run_id: String,
    },
    Error {
        run_id: String,
        message: String,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct RunStarted {
    pub run_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibrarySearchRequest {
    pub guru_id: String,
    pub query: String,
    pub kinds: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LibrarySummaryDto {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub excerpt: String,
    pub as_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<LibraryRelationDto>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LibraryRelationDto {
    pub relation: String,
    pub target_id: String,
    pub target_title: String,
    pub target_title_source: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LibraryRecordDto {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub excerpt: String,
    pub as_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<String>,
    pub markdown: String,
    pub relationships: Vec<LibraryRelationDto>,
}

pub(crate) fn memory_access_label(access: MemoryAccess) -> &'static str {
    match access {
        MemoryAccess::SearchDiscovered => "search_discovered",
        MemoryAccess::ExactRead => "exact_read",
    }
}

pub(crate) fn memory_ref_dto(memory: &MemoryRefSnapshot) -> MemoryRefDto {
    MemoryRefDto {
        record_id: memory.record_id.clone(),
        kind: memory.kind.clone(),
        title: memory.title.clone(),
        excerpt: memory.excerpt.clone(),
        as_of: memory.as_of.clone(),
        section: memory.section.clone(),
        access: memory_access_label(memory.access).into(),
    }
}

pub(crate) fn memory_kind_from_id(record_id: &str) -> Result<String, CommandError> {
    CanonicalMemoryKind::parse_record_id(record_id)
        .map(|(kind, _)| kind.label().to_owned())
        .ok_or_else(|| CommandError::internal("Runtime returned an invalid Memory record id"))
}

pub(crate) fn chat_dto(chat: &ChatSession) -> Result<ChatThreadDto, CommandError> {
    chat.validate().map_err(map_internal)?;
    let messages = chat
        .messages
        .iter()
        .map(|message| {
            let role = match message.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
                ChatRole::System | ChatRole::Tool => {
                    return Err(CommandError::internal(
                        "stored non-user chat role cannot cross the renderer boundary",
                    ));
                }
            };
            Ok(ChatMessageDto {
                id: message.id.clone(),
                role: role.into(),
                content: message.content.clone(),
                created_at: iso_time(message.created_at_ms)?,
                status: match message.status {
                    ChatMessageStatus::Complete => "complete",
                    ChatMessageStatus::Aborted => "aborted",
                }
                .into(),
                memory_refs: message.memory_refs.iter().map(memory_ref_dto).collect(),
                observed_exact_count: message.observed_exact_count,
                refs_truncated: message.refs_truncated,
                refs_digest: message.refs_digest.clone(),
                memory_update: message.memory_update.clone(),
                memory_revision: message.memory_revision.clone(),
                execution_model: message.execution_model.clone(),
                agent_harness: message.agent_harness.clone(),
                decision: message.decision.clone(),
                attachments: message
                    .attachments
                    .iter()
                    .map(|attachment| ChatAttachmentDto {
                        id: attachment.id.clone(),
                        filename: attachment.filename.clone(),
                        media_type: attachment.media_type.clone(),
                        size_bytes: attachment.size_bytes,
                    })
                    .collect(),
                artifact_refs: message.artifact_refs.clone(),
                progress: message.progress.clone(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ChatThreadDto {
        id: chat.id.clone(),
        guru_id: chat.guru_id.clone(),
        title: chat.title.clone(),
        updated_at: iso_time(chat.updated_at_ms)?,
        use_memory: chat.memory_policy.use_memory,
        update_memory: chat.memory_policy.update_memory,
        messages,
    })
}

pub(crate) fn runtime_record_summary(value: &Value) -> Result<LibrarySummaryDto, CommandError> {
    let id = json_text(value, "id")?.to_owned();
    let (id_kind, _) = CanonicalMemoryKind::parse_record_id(&id)
        .ok_or_else(|| CommandError::internal("Runtime returned an invalid Memory record id"))?;
    let kind = match value.get("kind").and_then(Value::as_str) {
        Some(value) => {
            let reported_kind = CanonicalMemoryKind::from_slug(value)
                .ok_or_else(|| CommandError::internal("Runtime returned an unknown Memory kind"))?;
            if reported_kind != id_kind {
                return Err(CommandError::internal(
                    "Runtime returned inconsistent Memory identity",
                ));
            }
            reported_kind.label().to_owned()
        }
        None => memory_kind_from_id(&id)?,
    };
    let title = json_text(value, "title")?.to_owned();
    let excerpt = value
        .get("summary")
        .and_then(Value::as_str)
        .or_else(|| value.get("text").and_then(Value::as_str))
        .unwrap_or("")
        .chars()
        .take(320)
        .collect();
    let as_of = value
        .get("as_of")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "active" | "revoked"))
        .map(str::to_owned);
    let revoked_by = value
        .get("revoked_by")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let mut relationships = Vec::new();
    if let Some(items) = value.get("relationships").and_then(Value::as_array) {
        for item in items {
            let Some(relation) = item.get("type").and_then(Value::as_str) else {
                continue;
            };
            let Some(target) = item.get("target").and_then(Value::as_str) else {
                continue;
            };
            if target.is_empty() {
                continue;
            }
            relationships.push(LibraryRelationDto {
                relation: relation.to_owned(),
                target_id: target.to_owned(),
                target_title: target.to_owned(),
                target_title_source: "record_id_fallback".into(),
            });
        }
    }
    if let Some(items) = value.get("see_also").and_then(Value::as_array) {
        for target in items.iter().filter_map(Value::as_str) {
            if target.is_empty() {
                continue;
            }
            relationships.push(LibraryRelationDto {
                relation: "see_also".into(),
                target_id: target.to_owned(),
                target_title: target.to_owned(),
                target_title_source: "record_id_fallback".into(),
            });
        }
    }
    Ok(LibrarySummaryDto {
        id,
        kind,
        title,
        excerpt,
        as_of,
        status,
        revoked_by,
        relationships,
    })
}
