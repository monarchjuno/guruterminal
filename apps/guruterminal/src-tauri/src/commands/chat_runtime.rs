use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    sync::Arc,
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use serde::{Serialize, Serializer};
use serde_json::Value;
use tauri::{ipc::Channel, State};

use crate::{
    agent_harness::{self, AgentHarnessSnapshot},
    app::{AppState, CommandError},
    broker::{start_tool_broker, tool_broker_endpoint, ToolPolicy, MAX_MEMORY_PROPOSALS},
    chat_control::{
        chat_control_channel, AcceptedChatControl, ChatControlError, ChatControlKind,
        ChatControlReceiver, ChatControlRequest,
    },
    chat_execution_session::ChatExecutionSession,
    chat_progress::{ChatProgress, ChatProgressOperation, ChatProgressProjection},
    chat_turn::ChatTurnResources,
    domain::{
        memory_refs_digest, CanonicalMemoryKind, ChatDecision, ChatMessage, ChatMessageStatus,
        ChatRole, MemoryAccess, MemoryPolicy, MemoryProposal, MemoryRefSnapshot, PiSessionCache,
        PiSessionCacheScope, MAX_MEMORY_REFS,
    },
    guru_root::profile_workspace,
    pi::{PiError, PiEvent, PiImageContent, PiLaunchConfig, PiProcess},
    pi_execution::{
        configure_pi_session_execution, read_pi_entries, PiEntriesState, PiExecutionConfig,
        PiSessionFileRequirement, PiSessionState,
    },
    pi_response::{AssistantTurnCapture, AssistantTurnEnd},
    run_coordinator::RunKind,
    settings::ExecutionModelLock,
    store::GuruTerminalStore,
};
use tokio::time::{timeout, Instant};

use super::{
    attachments::{
        attachment_prompt, persist_chat_attachments, pi_chat_turn_prompt, prepare_chat_attachments,
        ColdChatBootstrap,
    },
    capture_chat_connector_authority, current_user_skill_snapshots, enabled_skill_ids,
    fallback_chat_title,
    guru::managed_guru_dir,
    iso_time, map_internal, map_store, materialize_user_skill_snapshots, memory_updates, new_id,
    now_ms, require_text,
    tool_executor::{AppToolExecutor, ToolCapture},
    types::{
        memory_ref_dto, ChatControlModeDto, ChatControlReceiptDto, ChatControlRequestDto,
        ChatSendRequest, ChatStreamEvent, RunStarted,
    },
    MAX_CHAT_OUTPUT_BYTES, MAX_PROMPT_BYTES,
};

fn prompt_selects_memory_skill(prompt: &str) -> bool {
    prompt
        .split_whitespace()
        .any(|token| token == "$wiki" || token == "$lens")
}

fn enforce_prompt_memory_policy(prompt: &str, use_memory: &mut bool, update_memory: &mut bool) {
    if prompt_selects_memory_skill(prompt) {
        *use_memory = true;
        *update_memory = true;
    }
}

fn pi_event_stream_failure(
    error: tokio::sync::broadcast::error::RecvError,
) -> Option<&'static str> {
    match error {
        tokio::sync::broadcast::error::RecvError::Lagged(_) => None,
        tokio::sync::broadcast::error::RecvError::Closed => Some("Pi event stream was interrupted"),
    }
}

/// The prompt response and message lifecycle only confirm that Pi accepted
/// the turn. The first-progress watchdog must instead observe a body event
/// emitted by the provider stream.
fn pi_event_indicates_first_provider_body_progress(
    event: &Result<PiEvent, tokio::sync::broadcast::error::RecvError>,
) -> bool {
    match event {
        Ok(PiEvent::Rpc { payload }) => match payload.get("type").and_then(Value::as_str) {
            Some("message_update") => matches!(
                payload
                    .get("assistantMessageEvent")
                    .and_then(|event| event.get("type"))
                    .and_then(Value::as_str),
                Some(
                    "thinking_start"
                        | "thinking_delta"
                        | "thinking_end"
                        | "text_start"
                        | "text_delta"
                        | "text_end"
                        | "toolcall_start"
                        | "toolcall_delta"
                        | "toolcall_end"
                )
            ),
            Some("message_end") => {
                payload
                    .get("message")
                    .and_then(|message| message.get("role"))
                    .and_then(Value::as_str)
                    == Some("assistant")
            }
            _ => false,
        },
        Ok(PiEvent::ProtocolError { .. } | PiEvent::Exited)
        | Err(
            tokio::sync::broadcast::error::RecvError::Lagged(_)
            | tokio::sync::broadcast::error::RecvError::Closed,
        ) => false,
    }
}

/// Pi `compaction_end` has no `success` field. Failure is `result: null`
/// with `aborted` and/or `errorMessage`.
fn compaction_end_failed(payload: &Value) -> bool {
    payload.get("aborted").and_then(Value::as_bool) == Some(true)
        || payload
            .get("errorMessage")
            .and_then(Value::as_str)
            .is_some_and(|message| !message.is_empty())
        || !payload.get("result").is_some_and(Value::is_object)
}

pub(crate) async fn collect_learned_memory_index(
    state: &AppState,
    workspace: &crate::guru_root::BoundGuruRoot,
    cutoff: Option<chrono::NaiveDate>,
) -> Vec<agent_harness::LearnedMemoryIndexEntry> {
    let Ok(runtime) = state.runtime() else {
        return Vec::new();
    };
    let Ok(listed) = workspace.knowledge_list(&runtime, None).await else {
        return Vec::new();
    };
    let records = listed.as_array().cloned().unwrap_or_default();
    let recent_ids = crate::memory_git::recent_wiki_lens_ids(workspace.path(), 24);
    agent_harness::learned_memory_index_from_records(&records, &recent_ids, cutoff)
}

pub(super) async fn collect_charter(
    state: &AppState,
    workspace: &crate::guru_root::BoundGuruRoot,
    cutoff: Option<chrono::NaiveDate>,
) -> Option<String> {
    let runtime = state.runtime().ok()?;
    let read = workspace
        .knowledge_read(&runtime, agent_harness::CHARTER_RECORD_ID, None)
        .await
        .ok()?;
    agent_harness::charter_from_knowledge_read(&read, cutoff)
}

pub(super) fn parse_memory_kind(value: &str) -> Result<String, CommandError> {
    CanonicalMemoryKind::from_slug(&value.to_ascii_lowercase())
        .map(|kind| kind.slug().to_owned())
        .ok_or_else(|| CommandError::invalid("unknown memory kind"))
}

#[allow(clippy::large_enum_variant)]
enum ChatPiResume {
    Cold { session_id: String },
    Warm { cache: PiSessionCache },
}

impl ChatPiResume {
    fn session_id(&self) -> &str {
        match self {
            Self::Cold { session_id } => session_id,
            Self::Warm { cache } => cache
                .derived_session_id
                .as_deref()
                .expect("only validated cache metadata may warm-resume"),
        }
    }
}

fn select_chat_pi_resume(
    intended_cache: Option<PiSessionCache>,
    had_prior_messages: bool,
    chat_seed_session_id: &str,
    fresh_cold_session_id: impl FnOnce() -> String,
) -> ChatPiResume {
    match intended_cache {
        Some(cache) => ChatPiResume::Warm { cache },
        None => ChatPiResume::Cold {
            // The first turn may retain the immutable local Chat seed. Every
            // later cold rebuild receives a fresh provider/cache identity.
            session_id: if had_prior_messages {
                fresh_cold_session_id()
            } else {
                chat_seed_session_id.to_owned()
            },
        },
    }
}

#[cfg(test)]
mod chat_pi_resume_tests {
    use super::*;
    use crate::settings::ExecutionModelLock;
    use std::collections::BTreeMap;

    const SEED_SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";
    const FRESH_SESSION_ID: &str = "22222222-2222-4222-8222-222222222222";
    const CACHED_SESSION_ID: &str = "33333333-3333-4333-8333-333333333333";
    const HARNESS_DIGEST: &str = concat!(
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
    const RUNTIME_POLICY_DIGEST: &str = concat!(
        "cccccccccccccccccccccccccccccccc",
        "cccccccccccccccccccccccccccccccc"
    );
    const RUNTIME_SURFACE_DIGEST: &str = concat!(
        "dddddddddddddddddddddddddddddddd",
        "dddddddddddddddddddddddddddddddd"
    );

    fn cache() -> PiSessionCache {
        PiSessionCache {
            entries_sha256: "a".repeat(64),
            leaf_id: Some("leaf".into()),
            harness_digest: HARNESS_DIGEST.into(),
            runtime_policy_sha256: Some(RUNTIME_POLICY_DIGEST.into()),
            runtime_surface_sha256: Some(RUNTIME_SURFACE_DIGEST.into()),
            connector_authority_sha256: Some("e".repeat(64)),
            memory_access_enabled: Some(true),
            memory_update_enabled: Some(true),
            derived_session_id: Some(CACHED_SESSION_ID.into()),
            execution_model: ExecutionModelLock {
                profile_id: "profile".into(),
                name: "model".into(),
                provider: "provider".into(),
                model: "model".into(),
                thinking_level: "off".into(),
                run_options: BTreeMap::new(),
            },
        }
    }

    fn cache_scope<'a>(
        execution_model: &'a ExecutionModelLock,
        connector_authority_sha256: &'a str,
    ) -> PiSessionCacheScope<'a> {
        PiSessionCacheScope {
            harness_digest: HARNESS_DIGEST,
            runtime_policy_sha256: RUNTIME_POLICY_DIGEST,
            runtime_surface_sha256: RUNTIME_SURFACE_DIGEST,
            connector_authority_sha256,
            memory_access_enabled: true,
            memory_update_enabled: true,
            execution_model,
        }
    }

    #[test]
    fn matching_connector_authority_selects_warm_cache() {
        let cache = cache();
        let execution_model = cache.execution_model.clone();
        let authority_hash = "e".repeat(64);
        let intended_cache = cache
            .matches(&cache_scope(&execution_model, &authority_hash))
            .then_some(cache);
        let resume = select_chat_pi_resume(intended_cache, true, SEED_SESSION_ID, || {
            panic!("a warm cache must not allocate a cold session id")
        });

        assert!(matches!(resume, ChatPiResume::Warm { .. }));
        assert_eq!(resume.session_id(), CACHED_SESSION_ID);
    }

    #[test]
    fn changed_connector_authority_forces_a_fresh_cold_session_after_history() {
        let cache = cache();
        let execution_model = cache.execution_model.clone();
        let changed_authority_hash = "f".repeat(64);
        let intended_cache = cache
            .matches(&cache_scope(&execution_model, &changed_authority_hash))
            .then_some(cache);
        let resume = select_chat_pi_resume(intended_cache, true, SEED_SESSION_ID, || {
            FRESH_SESSION_ID.into()
        });

        assert!(matches!(resume, ChatPiResume::Cold { .. }));
        assert_eq!(resume.session_id(), FRESH_SESSION_ID);
        assert_ne!(resume.session_id(), CACHED_SESSION_ID);
    }

    #[test]
    fn cold_rebuild_after_prior_messages_rotates_the_provider_session_id() {
        let resume = select_chat_pi_resume(None, true, SEED_SESSION_ID, || FRESH_SESSION_ID.into());

        assert!(matches!(resume, ChatPiResume::Cold { .. }));
        assert_eq!(resume.session_id(), FRESH_SESSION_ID);
        assert_ne!(resume.session_id(), SEED_SESSION_ID);
    }

    #[test]
    fn empty_chat_cold_start_keeps_the_immutable_chat_seed() {
        let resume = select_chat_pi_resume(None, false, SEED_SESSION_ID, || {
            panic!("the first turn must not allocate a replacement session id")
        });

        assert!(matches!(resume, ChatPiResume::Cold { .. }));
        assert_eq!(resume.session_id(), SEED_SESSION_ID);
    }
}

/// A failed warm launch may be retried cold only after its Pi child is known
/// to have exited. Wiping the session directory while an unconfirmed child is
/// still alive could let it recreate JSONL after the new cold launch begins.
enum ChatPiLaunchFailure {
    Stopped(CommandError),
    StopUnconfirmed {
        error: CommandError,
        process_group_id: Option<i32>,
    },
}

impl ChatPiLaunchFailure {
    fn unconfirmed(error: CommandError) -> Self {
        Self::StopUnconfirmed {
            error,
            process_group_id: None,
        }
    }
}

#[cfg(unix)]
fn pi_process_group_id(pi: &PiProcess) -> Option<i32> {
    Some(pi.process_group_id())
}

#[cfg(not(unix))]
fn pi_process_group_id(_pi: &PiProcess) -> Option<i32> {
    None
}

fn record_unconfirmed_pi_stop(
    session: &ChatExecutionSession,
    process_group_id: Option<i32>,
) -> Result<(), CommandError> {
    #[cfg(unix)]
    if let Some(process_group_id) = process_group_id {
        session.record_unconfirmed_pi_stop(process_group_id)?;
    }
    #[cfg(not(unix))]
    let _ = (session, process_group_id);
    Ok(())
}

async fn stopped_launch_failure(pi: PiProcess, error: CommandError) -> ChatPiLaunchFailure {
    let process_group_id = pi_process_group_id(&pi);
    if pi.shutdown(Duration::from_secs(1)).await.is_ok() {
        ChatPiLaunchFailure::Stopped(error)
    } else {
        ChatPiLaunchFailure::StopUnconfirmed {
            error,
            process_group_id,
        }
    }
}

async fn launch_chat_pi(
    config: PiLaunchConfig,
    execution: &PiExecutionConfig,
    session: &ChatExecutionSession,
    resume: &ChatPiResume,
) -> Result<
    (
        PiProcess,
        tokio::sync::broadcast::Receiver<PiEvent>,
        ExecutionModelLock,
        PiSessionState,
    ),
    ChatPiLaunchFailure,
> {
    session
        .validate_current_binding()
        .map_err(ChatPiLaunchFailure::unconfirmed)?;
    let config = bind_chat_pi_session(config, session, resume.session_id()).map_err(|error| {
        ChatPiLaunchFailure::unconfirmed(CommandError::new("pi_unavailable", error.to_string()))
    })?;
    let pi = PiProcess::spawn(config).await.map_err(|error| {
        ChatPiLaunchFailure::unconfirmed(CommandError::new("pi_unavailable", error.to_string()))
    })?;
    let mut events = pi.subscribe();
    let file_requirement = match resume {
        ChatPiResume::Cold { .. } => PiSessionFileRequirement::ColdMayBeUnpersisted,
        ChatPiResume::Warm { .. } => PiSessionFileRequirement::Persisted,
    };
    match configure_pi_session_execution(
        &pi,
        &mut events,
        execution,
        resume.session_id(),
        session.session_directory(),
        file_requirement,
    )
    .await
    {
        Ok((model, state)) => {
            let entries = match read_pi_entries(&pi, &mut events, None).await {
                Ok(entries) => entries,
                Err(error) => {
                    return Err(stopped_launch_failure(pi, error).await);
                }
            };
            let acceptable = match resume {
                ChatPiResume::Cold { .. } => entries.cold_startup_only,
                ChatPiResume::Warm { cache } => entries.matches_cache(cache),
            };
            if !acceptable {
                return Err(stopped_launch_failure(
                    pi,
                    CommandError::new(
                        "pi_unavailable",
                        match resume {
                            ChatPiResume::Cold { .. } => {
                                "cold Pi session loaded unexpected prior context"
                            }
                            ChatPiResume::Warm { .. } => "warm Pi session cache digest mismatched",
                        },
                    ),
                )
                .await);
            }
            Ok((pi, events, model, state))
        }
        Err(error) => Err(stopped_launch_failure(pi, error).await),
    }
}

async fn launch_chat_pi_resuming(
    config: PiLaunchConfig,
    execution: &PiExecutionConfig,
    session: &mut ChatExecutionSession,
    resume: ChatPiResume,
) -> Result<
    (
        PiProcess,
        tokio::sync::broadcast::Receiver<PiEvent>,
        ExecutionModelLock,
        PiSessionState,
        bool,
    ),
    CommandError,
> {
    // A previous launch that could not prove its process group exited leaves
    // a durable sentinel. Resolve it before either warm reuse or a cold wipe.
    session.resolve_unconfirmed_pi_stops().await?;
    let cold_resume = match resume {
        ChatPiResume::Warm { cache } => {
            let warm_resume = ChatPiResume::Warm { cache };
            match launch_chat_pi(config.clone(), execution, session, &warm_resume).await {
                Ok((pi, events, model, state)) => return Ok((pi, events, model, state, true)),
                Err(ChatPiLaunchFailure::Stopped(_)) => {
                    session.wipe()?;
                    ChatPiResume::Cold {
                        session_id: uuid::Uuid::new_v4().to_string(),
                    }
                }
                Err(ChatPiLaunchFailure::StopUnconfirmed {
                    error,
                    process_group_id,
                }) => {
                    record_unconfirmed_pi_stop(session, process_group_id)?;
                    return Err(error);
                }
            }
        }
        ChatPiResume::Cold { session_id } => {
            session.wipe()?;
            ChatPiResume::Cold { session_id }
        }
    };
    let (pi, events, model, state) =
        match launch_chat_pi(config, execution, session, &cold_resume).await {
            Ok(launched) => launched,
            Err(ChatPiLaunchFailure::Stopped(error)) => return Err(error),
            Err(ChatPiLaunchFailure::StopUnconfirmed {
                error,
                process_group_id,
            }) => {
                record_unconfirmed_pi_stop(session, process_group_id)?;
                return Err(error);
            }
        };
    Ok((pi, events, model, state, false))
}

fn cache_from_entries(
    entries: PiEntriesState,
    scope: &PiSessionCacheScope<'_>,
    derived_session_id: &str,
) -> Option<PiSessionCache> {
    let cache = PiSessionCache {
        entries_sha256: entries.entries_sha256,
        leaf_id: entries.leaf_id,
        harness_digest: scope.harness_digest.to_owned(),
        runtime_policy_sha256: Some(scope.runtime_policy_sha256.to_owned()),
        runtime_surface_sha256: Some(scope.runtime_surface_sha256.to_owned()),
        connector_authority_sha256: Some(scope.connector_authority_sha256.to_owned()),
        memory_access_enabled: Some(scope.memory_access_enabled),
        memory_update_enabled: Some(scope.memory_update_enabled),
        derived_session_id: Some(derived_session_id.to_owned()),
        execution_model: scope.execution_model.clone(),
    };
    cache.validate().ok().map(|_| cache)
}

/// Execution-policy inputs which influence what existing Pi JSONL tool
/// results are safe to retain. The stable runtime/component surface and the
/// exact Memory authority profile are sealed separately.
#[derive(Serialize)]
struct PiSessionCacheRuntimePolicy<'a> {
    version: &'static str,
    pi_version: &'static str,
    as_of: Option<&'a str>,
    web_search_policy: PiSessionCacheWebSearchPolicy,
}

#[derive(Clone, Copy)]
enum PiSessionCacheWebSearchPolicy {
    Automatic,
    ModelOnly,
    ExaOnly,
}

impl From<crate::web::WebSearchPolicy> for PiSessionCacheWebSearchPolicy {
    fn from(policy: crate::web::WebSearchPolicy) -> Self {
        match policy {
            crate::web::WebSearchPolicy::Automatic => Self::Automatic,
            crate::web::WebSearchPolicy::ModelOnly => Self::ModelOnly,
            crate::web::WebSearchPolicy::ExaOnly => Self::ExaOnly,
        }
    }
}

impl Serialize for PiSessionCacheWebSearchPolicy {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Automatic => "automatic",
            Self::ModelOnly => "model_only",
            Self::ExaOnly => "exa_only",
        })
    }
}

fn pi_session_cache_runtime_policy_sha256(
    as_of: Option<&str>,
    web_search_policy: crate::web::WebSearchPolicy,
) -> Result<String, CommandError> {
    let policy = PiSessionCacheRuntimePolicy {
        version: crate::pi::PI_SESSION_CACHE_POLICY_VERSION,
        pi_version: crate::pi::PI_VERSION,
        as_of,
        web_search_policy: web_search_policy.into(),
    };
    let serialized = serde_json::to_vec(&policy).map_err(map_internal)?;
    Ok(crate::hashing::sha256(&serialized))
}

fn pi_session_cache_runtime_surface_sha256(
    capability_ids: &[String],
) -> Result<String, CommandError> {
    // `true, true` yields the complete Memory-capable core surface. Per-turn
    // Memory authority is sealed as an exact cache field, while this digest
    // keeps every component binding exact and rejects removed tool results.
    let surface = agent_harness::AgentRuntimeProfile::new("chat", true, true, capability_ids)
        .map_err(map_internal)?;
    let serialized = serde_json::to_vec(&surface).map_err(map_internal)?;
    Ok(crate::hashing::sha256(&serialized))
}

pub(super) fn bind_chat_pi_session(
    config: PiLaunchConfig,
    session: &ChatExecutionSession,
    session_id: &str,
) -> Result<PiLaunchConfig, PiError> {
    config.with_session(session.pi_config_with_id(session_id))
}

/// Builds the Chat-specific Pi launch configuration. The Pi session file is
/// tied to its CWD by Pi itself, so this must use the stable, app-private CWD
/// owned by `ChatExecutionSession`, never disposable turn scratch.
pub(super) fn chat_pi_launch_config(
    pi_artifacts: &crate::app::PiArtifacts,
    app_data_dir: &Path,
    guru_id: &str,
    run_id: &str,
    broker_socket: std::path::PathBuf,
    broker_token: String,
    session: &ChatExecutionSession,
) -> PiLaunchConfig {
    pi_artifacts.launch_config(
        app_data_dir,
        guru_id,
        run_id,
        session.runtime_working_directory().to_path_buf(),
        broker_socket,
        broker_token,
    )
}

#[derive(Clone)]
pub(super) struct DurableMemoryTrace {
    pub(super) refs: Vec<MemoryRefSnapshot>,
    pub(super) observed_exact_count: u64,
    pub(super) refs_truncated: bool,
    pub(super) refs_digest: String,
}

pub(super) fn durable_memory_trace(
    memories: impl IntoIterator<Item = MemoryRefSnapshot>,
    decision: Option<&ChatDecision>,
    proposals: &[MemoryProposal],
) -> Result<DurableMemoryTrace, CommandError> {
    let all = memories
        .into_iter()
        .filter(|memory| memory.access == MemoryAccess::ExactRead)
        .map(|memory| (memory.record_id.clone(), memory))
        .collect::<BTreeMap<_, _>>();
    let all_refs = all.values().cloned().collect::<Vec<_>>();
    let observed_exact_count = u64::try_from(all_refs.len())
        .map_err(|_| CommandError::internal("exact Memory read count overflowed"))?;
    let refs_digest = memory_refs_digest(&all_refs).map_err(map_internal)?;

    let mut retained_ids = BTreeSet::new();
    let mut refs = Vec::with_capacity(all.len().min(MAX_MEMORY_REFS));
    let mut retain = |record_id: &str| {
        if refs.len() < MAX_MEMORY_REFS && retained_ids.insert(record_id.to_owned()) {
            if let Some(memory) = all.get(record_id) {
                refs.push(memory.clone());
            }
        }
    };
    if let Some(decision) = decision {
        if let Some(evidence_ids) = decision
            .payload
            .get("evidence_ids")
            .and_then(Value::as_array)
        {
            for evidence_id in evidence_ids.iter().filter_map(Value::as_str) {
                retain(evidence_id);
            }
        }
    }
    for proposal in proposals {
        for source_id in &proposal.source_memory_ids {
            retain(source_id);
        }
    }
    for record_id in all.keys() {
        retain(record_id);
    }
    Ok(DurableMemoryTrace {
        refs_truncated: observed_exact_count > refs.len() as u64,
        refs,
        observed_exact_count,
        refs_digest,
    })
}

fn empty_memory_trace() -> Result<DurableMemoryTrace, CommandError> {
    durable_memory_trace(Vec::new(), None, &[])
}

fn emit_chat_progress(
    on_event: &Channel<ChatStreamEvent>,
    run_id: &str,
    projection: &ChatProgressProjection,
) {
    let _ = on_event.send(ChatStreamEvent::Progress {
        run_id: run_id.to_owned(),
        progress: projection.snapshot(),
    });
}

fn emit_completed_chat(
    on_event: &Channel<ChatStreamEvent>,
    run_id: &str,
    message: &ChatMessage,
    title: Option<String>,
    execution_model: &ExecutionModelLock,
    agent_harness: &AgentHarnessSnapshot,
) {
    if !message.memory_refs.is_empty() {
        let _ = on_event.send(ChatStreamEvent::Memory {
            run_id: run_id.to_owned(),
            memories: message.memory_refs.iter().map(memory_ref_dto).collect(),
        });
    }
    if let Some(title) = title {
        let _ = on_event.send(ChatStreamEvent::Title {
            run_id: run_id.to_owned(),
            title,
        });
    }
    if let Some(decision) = &message.decision {
        let _ = on_event.send(ChatStreamEvent::Decision {
            run_id: run_id.to_owned(),
            decision: decision.clone(),
        });
    }
    if let Some(result) = message.memory_update.clone() {
        let _ = on_event.send(ChatStreamEvent::MemoryUpdate {
            run_id: run_id.to_owned(),
            result: Box::new(result),
        });
    }
    for artifact in &message.artifact_refs {
        let _ = on_event.send(ChatStreamEvent::Artifact {
            run_id: run_id.to_owned(),
            artifact: artifact.clone(),
        });
    }
    let _ = on_event.send(ChatStreamEvent::Completed {
        run_id: run_id.to_owned(),
        message_id: message.id.clone(),
        final_text: message.content.clone(),
        created_at: iso_time(message.created_at_ms).unwrap_or_default(),
        execution_model: Box::new(execution_model.clone()),
        agent_harness: Box::new(agent_harness.clone()),
    });
}

pub(super) struct CanonicalCompletionExpectation<'a> {
    pub content: &'a str,
    pub memory_revision: &'a Option<String>,
    pub execution_model: &'a ExecutionModelLock,
    pub agent_harness: &'a AgentHarnessSnapshot,
    pub title: Option<&'a str>,
}

pub(super) fn recovered_canonical_completion(
    store: &dyn GuruTerminalStore,
    thread_id: &str,
    message_id: &str,
    expected: CanonicalCompletionExpectation<'_>,
) -> Option<(ChatMessage, Option<String>)> {
    let chat = store.get_chat(thread_id).ok()??;
    let message = chat
        .messages
        .iter()
        .find(|message| message.id == message_id)?;
    if message.role != ChatRole::Assistant
        || message.status != ChatMessageStatus::Complete
        || message.content != expected.content
        || &message.memory_revision != expected.memory_revision
        || message.execution_model.as_ref() != Some(expected.execution_model)
        || message.agent_harness.as_ref() != Some(expected.agent_harness)
    {
        return None;
    }
    let title = expected
        .title
        .filter(|expected| chat.title.as_str() == *expected)
        .map(|_| chat.title.clone());
    Some((message.clone(), title))
}

pub(super) const FAILED_CHAT_CONTENT: &str = "Response could not be completed.";
const CHAT_CONTROL_SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
// Any post-prompt Pi event arrives before provider streaming can make visible
// assistant progress. Its absence therefore indicates a broken Pi/RPC path,
// not normal model thinking. Never replay the prompt here: a hidden tool call
// could otherwise be duplicated.
const CHAT_FIRST_PROVIDER_BODY_PROGRESS_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
enum ChatTerminalStatus {
    Aborted,
    Error,
}

#[allow(clippy::too_many_arguments)]
fn persist_and_emit_terminal_chat(
    store: &dyn GuruTerminalStore,
    on_event: &Channel<ChatStreamEvent>,
    run_id: &str,
    thread_id: &str,
    message_id: &str,
    content: String,
    memory_trace: DurableMemoryTrace,
    memory_revision: Option<String>,
    execution_model: ExecutionModelLock,
    agent_harness: AgentHarnessSnapshot,
    progress: Option<ChatProgress>,
    terminal_status: ChatTerminalStatus,
) -> Result<RunStarted, CommandError> {
    let expected = store
        .get_chat(thread_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("chat thread"))?;
    let created_at_ms = now_ms().max(expected.updated_at_ms + 1);
    let mut chat = expected.clone();
    chat.messages.push(ChatMessage {
        id: message_id.to_owned(),
        role: ChatRole::Assistant,
        status: match terminal_status {
            ChatTerminalStatus::Aborted => ChatMessageStatus::Aborted,
            ChatTerminalStatus::Error => ChatMessageStatus::Error,
        },
        content: content.clone(),
        created_at_ms,
        memory_refs: memory_trace.refs,
        observed_exact_count: memory_trace.observed_exact_count,
        refs_truncated: memory_trace.refs_truncated,
        refs_digest: memory_trace.refs_digest,
        memory_update: None,
        memory_revision,
        execution_model: Some(execution_model.clone()),
        agent_harness: Some(agent_harness.clone()),
        decision: None,
        attachments: Vec::new(),
        artifact_refs: Vec::new(),
        progress: progress.clone(),
    });
    chat.updated_at_ms = created_at_ms;
    store.replace_chat(&expected, &chat).map_err(map_store)?;
    let _ = on_event.send(match terminal_status {
        ChatTerminalStatus::Aborted => ChatStreamEvent::Aborted {
            run_id: run_id.to_owned(),
        },
        ChatTerminalStatus::Error => ChatStreamEvent::Error {
            run_id: run_id.to_owned(),
            message: FAILED_CHAT_CONTENT.into(),
            message_id: message_id.to_owned(),
            final_text: content,
            created_at: iso_time(created_at_ms).unwrap_or_default(),
            execution_model: Box::new(execution_model),
            agent_harness: Box::new(agent_harness),
            progress,
        },
    });
    Ok(RunStarted {
        run_id: run_id.to_owned(),
    })
}

#[allow(clippy::too_many_arguments)]
fn persist_and_emit_aborted_chat(
    store: &dyn GuruTerminalStore,
    on_event: &Channel<ChatStreamEvent>,
    run_id: &str,
    thread_id: &str,
    message_id: &str,
    content: String,
    memory_trace: DurableMemoryTrace,
    memory_revision: Option<String>,
    execution_model: ExecutionModelLock,
    agent_harness: AgentHarnessSnapshot,
    progress: Option<ChatProgress>,
) -> Result<RunStarted, CommandError> {
    persist_and_emit_terminal_chat(
        store,
        on_event,
        run_id,
        thread_id,
        message_id,
        content,
        memory_trace,
        memory_revision,
        execution_model,
        agent_harness,
        progress,
        ChatTerminalStatus::Aborted,
    )
}

#[allow(clippy::too_many_arguments)]
fn persist_and_emit_failed_chat(
    store: &dyn GuruTerminalStore,
    on_event: &Channel<ChatStreamEvent>,
    run_id: &str,
    thread_id: &str,
    message_id: &str,
    memory_revision: Option<String>,
    execution_model: ExecutionModelLock,
    agent_harness: AgentHarnessSnapshot,
    progress: Option<ChatProgress>,
) -> Result<RunStarted, CommandError> {
    persist_and_emit_terminal_chat(
        store,
        on_event,
        run_id,
        thread_id,
        message_id,
        FAILED_CHAT_CONTENT.into(),
        empty_memory_trace()?,
        memory_revision,
        execution_model,
        agent_harness,
        progress,
        ChatTerminalStatus::Error,
    )
}

#[allow(clippy::too_many_arguments)]
fn persist_or_emit_failed_chat(
    store: &dyn GuruTerminalStore,
    on_event: &Channel<ChatStreamEvent>,
    run_id: &str,
    thread_id: &str,
    message_id: &str,
    memory_revision: Option<String>,
    execution_model: ExecutionModelLock,
    agent_harness: AgentHarnessSnapshot,
    progress: Option<ChatProgress>,
) -> RunStarted {
    match persist_and_emit_failed_chat(
        store,
        on_event,
        run_id,
        thread_id,
        message_id,
        memory_revision,
        execution_model.clone(),
        agent_harness.clone(),
        progress.clone(),
    ) {
        Ok(run) => run,
        Err(_) => {
            // The user turn was already durable. Never surface internal store
            // details or leave the renderer waiting if terminal persistence
            // itself is unavailable.
            let _ = on_event.send(ChatStreamEvent::Error {
                run_id: run_id.to_owned(),
                message: FAILED_CHAT_CONTENT.into(),
                message_id: message_id.to_owned(),
                final_text: FAILED_CHAT_CONTENT.into(),
                created_at: iso_time(now_ms()).unwrap_or_default(),
                execution_model: Box::new(execution_model),
                agent_harness: Box::new(agent_harness),
                progress,
            });
            RunStarted {
                run_id: run_id.to_owned(),
            }
        }
    }
}

/// Persists an early failure only if terminal completion wins the same
/// coordinator boundary as Stop. A launch/configuration error can arrive
/// while its await is in flight, after the user message is already durable.
/// In that race, a successful Stop must be represented as an aborted
/// assistant message rather than an Error that resurfaces after reload.
#[allow(clippy::too_many_arguments)]
pub(super) fn persist_or_emit_failed_chat_with_stop_precedence(
    state: &AppState,
    on_event: &Channel<ChatStreamEvent>,
    run_id: &str,
    thread_id: &str,
    message_id: &str,
    memory_revision: Option<String>,
    execution_model: ExecutionModelLock,
    agent_harness: AgentHarnessSnapshot,
    progress: Option<ChatProgress>,
) -> RunStarted {
    if matches!(state.claim_run_completion(run_id, RunKind::Chat), Ok(false)) {
        let Ok(memory_trace) = empty_memory_trace() else {
            let _ = on_event.send(ChatStreamEvent::Aborted {
                run_id: run_id.to_owned(),
            });
            return RunStarted {
                run_id: run_id.to_owned(),
            };
        };
        return match persist_and_emit_aborted_chat(
            state.store.as_ref(),
            on_event,
            run_id,
            thread_id,
            message_id,
            "Response stopped.".into(),
            memory_trace,
            memory_revision,
            execution_model,
            agent_harness,
            progress,
        ) {
            Ok(run) => run,
            Err(_) => {
                let _ = on_event.send(ChatStreamEvent::Aborted {
                    run_id: run_id.to_owned(),
                });
                RunStarted {
                    run_id: run_id.to_owned(),
                }
            }
        };
    }
    persist_or_emit_failed_chat(
        state.store.as_ref(),
        on_event,
        run_id,
        thread_id,
        message_id,
        memory_revision,
        execution_model,
        agent_harness,
        progress,
    )
}

pub(super) fn persist_chat_control_message(
    store: &dyn GuruTerminalStore,
    guru_id: &str,
    thread_id: &str,
    prompt: &str,
) -> Result<AcceptedChatControl, CommandError> {
    let expected = store
        .get_chat(thread_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("chat thread"))?;
    if expected.guru_id != guru_id {
        return Err(CommandError::conflict(
            "chat thread does not belong to the selected Guru",
        ));
    }
    let created_at_ms = now_ms().max(expected.updated_at_ms + 1);
    let message_id = new_id("message");
    let mut chat = expected.clone();
    chat.messages.push(ChatMessage {
        id: message_id.clone(),
        role: ChatRole::User,
        status: ChatMessageStatus::Complete,
        content: prompt.to_owned(),
        created_at_ms,
        memory_refs: Vec::new(),
        observed_exact_count: 0,
        refs_truncated: false,
        refs_digest: memory_refs_digest(&[]).map_err(map_internal)?,
        memory_update: None,
        memory_revision: None,
        execution_model: None,
        agent_harness: None,
        decision: None,
        attachments: Vec::new(),
        artifact_refs: Vec::new(),
        progress: None,
    });
    // Additional user context is now canonical, so any prior cursor can no
    // longer describe the Chat transcript that the active Pi process receives.
    chat.updated_at_ms = created_at_ms;
    store.replace_chat(&expected, &chat).map_err(map_store)?;
    Ok(AcceptedChatControl {
        message_id,
        created_at_ms,
    })
}

pub(super) fn settle_chat_control_response(
    store: &dyn GuruTerminalStore,
    guru_id: &str,
    thread_id: &str,
    control: ChatControlRequest,
    accepted_by_pi: bool,
) -> Result<(), CommandError> {
    if !accepted_by_pi {
        control.complete(Err(ChatControlError::Rejected(
            "Pi rejected the queued instruction".into(),
        )));
        return Ok(());
    }

    let accepted = match persist_chat_control_message(store, guru_id, thread_id, &control.prompt) {
        Ok(accepted) => accepted,
        Err(error) => {
            // Pi accepted the steer, but continuing would produce a response
            // that cannot be replayed from the canonical transcript.
            control.complete(Err(ChatControlError::Rejected(
                "Could not save the queued instruction".into(),
            )));
            return Err(error);
        }
    };
    control.complete(Ok(accepted));
    Ok(())
}

fn reject_pending_chat_controls(
    pending_controls: HashMap<u64, (u64, ChatControlRequest)>,
    settled_controls: BTreeMap<u64, (ChatControlRequest, bool)>,
) {
    for (_, (_, control)) in pending_controls {
        control.complete(Err(ChatControlError::Rejected(
            "The active response ended before the queued instruction was accepted".into(),
        )));
    }
    for (_, (control, _)) in settled_controls {
        control.complete(Err(ChatControlError::Rejected(
            "The active response ended before the queued instruction was accepted".into(),
        )));
    }
}

fn reject_queued_chat_controls(chat_controls: &mut ChatControlReceiver) {
    while let Some(control) = chat_controls.try_recv() {
        control.complete(Err(ChatControlError::Rejected(
            "The active response ended before the queued instruction was accepted".into(),
        )));
    }
}

fn record_chat_control_response(
    pending_controls: &mut HashMap<u64, (u64, ChatControlRequest)>,
    settled_controls: &mut BTreeMap<u64, (ChatControlRequest, bool)>,
    request_id: u64,
    accepted_by_pi: bool,
) -> bool {
    let Some((sequence, control)) = pending_controls.remove(&request_id) else {
        return false;
    };
    settled_controls.insert(sequence, (control, accepted_by_pi));
    true
}

fn persist_contiguous_chat_controls(
    store: &dyn GuruTerminalStore,
    guru_id: &str,
    thread_id: &str,
    settled_controls: &mut BTreeMap<u64, (ChatControlRequest, bool)>,
    next_control_to_persist: &mut u64,
) -> Result<(), CommandError> {
    while let Some((control, accepted_by_pi)) = settled_controls.remove(next_control_to_persist) {
        settle_chat_control_response(store, guru_id, thread_id, control, accepted_by_pi)?;
        *next_control_to_persist += 1;
    }
    Ok(())
}

pub(super) async fn settle_chat_controls_before_completion(
    pi_events: &mut tokio::sync::broadcast::Receiver<PiEvent>,
    pending_controls: &mut HashMap<u64, (u64, ChatControlRequest)>,
    settled_controls: &mut BTreeMap<u64, (ChatControlRequest, bool)>,
    next_control_to_persist: &mut u64,
    store: &dyn GuruTerminalStore,
    guru_id: &str,
    thread_id: &str,
) -> Result<(), CommandError> {
    let deadline = Instant::now() + CHAT_CONTROL_SETTLE_TIMEOUT;
    loop {
        persist_contiguous_chat_controls(
            store,
            guru_id,
            thread_id,
            settled_controls,
            next_control_to_persist,
        )?;
        if pending_controls.is_empty() && settled_controls.is_empty() {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(CommandError::internal(
                "Pi did not confirm queued instructions before the response ended",
            ));
        }
        match timeout(remaining, pi_events.recv()).await {
            Ok(Ok(PiEvent::Rpc { payload })) => {
                if payload.get("type").and_then(Value::as_str) != Some("response") {
                    continue;
                }
                let Some(request_id) = payload.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                if record_chat_control_response(
                    pending_controls,
                    settled_controls,
                    request_id,
                    payload.get("success").and_then(Value::as_bool) == Some(true),
                ) {
                    continue;
                }
            }
            Ok(Ok(PiEvent::ProtocolError { .. }))
            | Ok(Ok(PiEvent::Exited))
            | Ok(Err(tokio::sync::broadcast::error::RecvError::Closed))
            | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_)))
            | Err(_) => {
                return Err(CommandError::internal(
                    "Pi did not confirm queued instructions before the response ended",
                ));
            }
        }
    }
}

async fn submit_chat_control(
    request: ChatControlRequestDto,
    kind: ChatControlKind,
    state: State<'_, AppState>,
) -> Result<ChatControlReceiptDto, CommandError> {
    let guru_id = require_text(&request.guru_id, "Guru", 512)?;
    let thread_id = require_text(&request.thread_id, "Chat thread", 512)?;
    let prompt = require_text(&request.prompt, "prompt", MAX_PROMPT_BYTES)?;
    let accepted = state
        .submit_chat_control(&guru_id, &thread_id, kind, prompt.clone())
        .await?;
    Ok(ChatControlReceiptDto {
        message_id: accepted.message_id,
        prompt,
        created_at: iso_time(accepted.created_at_ms)?,
        mode: ChatControlModeDto::Steer,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_steer(
    request: ChatControlRequestDto,
    state: State<'_, AppState>,
) -> Result<ChatControlReceiptDto, CommandError> {
    submit_chat_control(request, ChatControlKind::Steer, state).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_send(
    mut request: ChatSendRequest,
    on_event: Channel<ChatStreamEvent>,
    state: State<'_, AppState>,
) -> Result<RunStarted, CommandError> {
    enforce_prompt_memory_policy(
        &request.prompt,
        &mut request.use_memory,
        &mut request.update_memory,
    );
    state.ensure_guru_available(&request.guru_id)?;
    let run_id = validated_chat_run_id(&request.run_id)?;
    require_text(&request.guru_id, "Guru", 512)?;
    require_text(&request.thread_id, "Chat thread", 512)?;
    // Admit the renderer-owned run immediately after its target binding is
    // syntactically sealed. Another Tokio worker may issue Stop while this
    // command is still performing synchronous store/profile checks.
    let (chat_control, mut chat_controls) = chat_control_channel();
    let registration = state.register_chat_run(
        run_id.clone(),
        request.guru_id.clone(),
        request.thread_id.clone(),
        chat_control,
    )?;
    let mut cancel = registration.cancel;
    let run_lease = registration.lease;
    let run_scratch = crate::run_scratch::RunScratch::create(
        state.artifacts.deletion_root.clone(),
        &request.guru_id,
        &run_id,
    )?;
    let _ = on_event.send(ChatStreamEvent::Started {
        run_id: run_id.clone(),
    });
    let prompt = request.prompt.trim().to_owned();
    if prompt.len() > MAX_PROMPT_BYTES || prompt.contains('\0') {
        return Err(CommandError::invalid(format!(
            "prompt must contain at most {MAX_PROMPT_BYTES} bytes"
        )));
    }
    let prepared_attachments = prepare_chat_attachments(&request.attachments)?;
    if prompt.is_empty() && prepared_attachments.is_empty() {
        return Err(CommandError::invalid(
            "a prompt or at least one attachment is required",
        ));
    }
    let pi_execution = state.pi_execution(
        &request.model_profile_id,
        request.thinking_level.as_str(),
        &request.run_options,
    )?;
    let pi_artifacts = pi_execution.artifacts.clone();
    let configured_model = pi_execution.model.clone();
    let has_images = prepared_attachments
        .iter()
        .any(|attachment| attachment.record.media_type.starts_with("image/"));
    if has_images && !configured_model.input.iter().any(|input| input == "image") {
        return Err(CommandError::invalid(
            "the selected model does not accept image attachments",
        ));
    }
    if prepared_attachments.iter().any(|attachment| {
        attachment.record.media_type.starts_with("image/")
            && !matches!(
                attachment.record.media_type.as_str(),
                "image/jpeg" | "image/png" | "image/gif" | "image/webp"
            )
    }) {
        return Err(CommandError::invalid(
            "image attachment type is unsupported",
        ));
    }
    let profile = state
        .store
        .get_guru(&request.guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let skill_ids = enabled_skill_ids(state.store.as_ref(), &profile.id)?;
    let run_skill_ids = agent_harness::run_skill_ids(&skill_ids).map_err(map_internal)?;
    let user_skill_snapshots = current_user_skill_snapshots(state.store.as_ref(), &profile.id)?;
    let agent_root = pi_artifacts
        .system_prompt
        .parent()
        .ok_or_else(|| CommandError::new("pi_unavailable", "Pi agent resource is invalid"))?;
    let mut skill_files = agent_harness::resolve_skill_paths(agent_root, &run_skill_ids)
        .map_err(|error| CommandError::new("pi_unavailable", error.to_string()))?;
    let connector_authority = capture_chat_connector_authority(&state, &profile.id)?;
    let capability_ids = connector_authority.capability_ids.clone();
    let web_search_policy = if capability_ids
        .iter()
        .any(|capability_id| capability_id == "community.web-research")
    {
        crate::marketplace::web_research_policy(state.inner())?
    } else {
        crate::web::WebSearchPolicy::default()
    };
    let harness = agent_harness::snapshot_with_user_skills(
        "chat",
        &skill_ids,
        &user_skill_snapshots,
        &capability_ids,
    )
    .map_err(map_internal)?;
    let host_context =
        agent_harness::append_snapshot_to_context("{}", &harness).map_err(map_internal)?;
    let runtime_profile = agent_harness::AgentRuntimeProfile::new(
        "chat",
        request.use_memory,
        request.update_memory,
        &capability_ids,
    )
    .map_err(map_internal)?;
    let host_context =
        agent_harness::append_runtime_profile_to_context(&host_context, &runtime_profile)
            .map_err(map_internal)?;
    // Pi JSONL retains prior tool results in model context. A warm resume is
    // therefore allowed only when its static surface, connector authority,
    // and per-turn Memory profile exactly match the sealed cache metadata.
    let runtime_policy_sha256 =
        pi_session_cache_runtime_policy_sha256(request.as_of.as_deref(), web_search_policy)?;
    let runtime_surface_sha256 = pi_session_cache_runtime_surface_sha256(&capability_ids)?;
    let workspace = profile_workspace(&profile)?;
    let guru_dir = managed_guru_dir(state.inner(), &profile.id)?;
    let chat_workbench = guru_dir.join("workbench");
    let runtime = state.runtime()?;
    let mut chat = state
        .store
        .get_chat(&request.thread_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("chat thread"))?;
    if chat.guru_id != profile.id {
        return Err(CommandError::conflict(
            "chat thread does not belong to the selected Guru",
        ));
    }
    let had_prior_messages = !chat.messages.is_empty();
    let expected_chat = chat.clone();
    workspace.validate(&runtime).await.map_err(map_internal)?;
    skill_files.extend(materialize_user_skill_snapshots(
        state.store.as_ref(),
        &profile.id,
        &user_skill_snapshots,
        &run_scratch.path().join("user-skills"),
    )?);
    let memory_revision = if request.use_memory {
        Some(workspace.inspect_memory_tree().map_err(map_internal)?.0)
    } else {
        None
    };
    let execution_session =
        ChatExecutionSession::prepare(state.artifacts.deletion_root.clone(), &chat)?;
    let mut turn_resources = ChatTurnResources::new(run_lease, run_scratch, execution_session);
    let timestamp = now_ms().max(chat.updated_at_ms + 1);
    let user_message_id = new_id("message");
    let current_prompt =
        attachment_prompt(&prompt, &chat.id, &user_message_id, &prepared_attachments);
    let current_image_bytes = prepared_attachments
        .iter()
        .filter(|attachment| attachment.record.media_type.starts_with("image/"))
        .map(|attachment| attachment.bytes.len())
        .sum::<usize>();
    let current_pi_images = prepared_attachments
        .iter()
        .filter(|attachment| attachment.record.media_type.starts_with("image/"))
        .map(|attachment| PiImageContent {
            data: BASE64.encode(&attachment.bytes),
            mime_type: attachment.record.media_type.clone(),
        })
        .collect::<Vec<_>>();
    let requested_execution_model = pi_execution.model_lock();
    let requested_cache_scope = PiSessionCacheScope {
        harness_digest: &harness.digest,
        runtime_policy_sha256: &runtime_policy_sha256,
        runtime_surface_sha256: &runtime_surface_sha256,
        connector_authority_sha256: &connector_authority.sha256,
        memory_access_enabled: request.use_memory,
        memory_update_enabled: request.update_memory,
        execution_model: &requested_execution_model,
    };
    let intended_cache = connector_authority
        .cacheable
        .then(|| chat.pi_session_cache.clone())
        .flatten()
        .filter(|cache| cache.matches(&requested_cache_scope));
    let pending_attachments = persist_chat_attachments(
        &state,
        &profile.id,
        &chat.id,
        &user_message_id,
        &prepared_attachments,
    )?;
    chat.messages.push(ChatMessage {
        id: user_message_id.clone(),
        role: ChatRole::User,
        status: ChatMessageStatus::Complete,
        content: prompt.clone(),
        created_at_ms: timestamp,
        memory_refs: Vec::new(),
        observed_exact_count: 0,
        refs_truncated: false,
        refs_digest: memory_refs_digest(&[]).map_err(map_internal)?,
        memory_update: None,
        memory_revision: None,
        execution_model: None,
        agent_harness: None,
        decision: None,
        attachments: prepared_attachments
            .iter()
            .map(|attachment| attachment.record.clone())
            .collect(),
        artifact_refs: Vec::new(),
        progress: None,
    });
    chat.memory_policy = MemoryPolicy {
        use_memory: request.use_memory,
        update_memory: request.update_memory,
    };
    // The prior Pi cursor describes the transcript before this user turn. The
    // user-message CAS durably marks it dirty so a hard crash cannot resume a
    // generation that Pi may already have mutated. Assistant content and the
    // replacement cache state are sealed together by the later exact CAS.
    chat.pi_session_cache = None;
    chat.updated_at_ms = timestamp;
    if let Err(error) = state.store.replace_chat(&expected_chat, &chat) {
        if let Some(pending) = pending_attachments {
            pending.rollback()?;
        }
        return Err(map_store(error));
    }
    if let Some(pending) = pending_attachments {
        pending.commit();
    }
    let should_generate_title = chat.title == "New chat" && chat.messages.len() == 1;
    let assistant_message_id = new_id("message");
    if state
        .store
        .set_guru_last_model_profile(&profile.id, &configured_model.id, timestamp)
        .is_err()
    {
        return Ok(persist_or_emit_failed_chat_with_stop_precedence(
            state.inner(),
            &on_event,
            &run_id,
            &chat.id,
            &assistant_message_id,
            memory_revision.clone(),
            pi_execution.model_lock(),
            harness.clone(),
            None,
        ));
    }

    if *cancel.borrow() {
        return persist_and_emit_aborted_chat(
            state.store.as_ref(),
            &on_event,
            &run_id,
            &chat.id,
            &assistant_message_id,
            "Response stopped.".into(),
            empty_memory_trace()?,
            memory_revision.clone(),
            pi_execution.model_lock(),
            harness.clone(),
            None,
        );
    }
    let broker_socket =
        tool_broker_endpoint(state.artifacts.broker_dir.join(format!("{run_id}.sock")));
    let capture = {
        let mut capture = ToolCapture::for_chat(assistant_message_id.clone());
        capture.compute = Arc::new(crate::compute::TurnComputeSession::new(
            state.artifacts.compute.clone(),
            turn_resources.run_scratch_path().join("compute"),
        ));
        capture.mcp_scratch_root = Some(turn_resources.run_scratch_path().join("mcp"));
        capture.mcp_pool = Some(state.mcp_pool.clone());
        capture.web_search_policy = web_search_policy;
        capture.search_cancel = crate::web::SearchCancel::from_watch(cancel.clone());
        Arc::new(capture)
    };
    let app_state = state.inner().clone();
    let executor = Arc::new(AppToolExecutor {
        state: app_state.clone(),
        capture: capture.clone(),
        guru_id: profile.id.clone(),
        guru_root: workspace.clone(),
        capability_ids: harness.capability_ids.iter().cloned().collect(),
        chat_provider: configured_model.provider.clone(),
    });
    let policy = ToolPolicy {
        guru_id: profile.id.clone(),
        session_id: chat.id.clone(),
        use_memory: request.use_memory,
        propose_memory_updates: request.update_memory,
        memory_proposal_budget: if request.update_memory {
            MAX_MEMORY_PROPOSALS
        } else {
            0
        },
        as_of: request.as_of.clone(),
    };
    let broker = match start_tool_broker(broker_socket.clone(), policy, executor).await {
        Ok(broker) => broker,
        Err(_) => {
            return Ok(persist_or_emit_failed_chat_with_stop_precedence(
                state.inner(),
                &on_event,
                &run_id,
                &chat.id,
                &assistant_message_id,
                memory_revision.clone(),
                pi_execution.model_lock(),
                harness.clone(),
                None,
            ));
        }
    };
    if *cancel.borrow() {
        let _ = broker.shutdown().await;
        return persist_and_emit_aborted_chat(
            state.store.as_ref(),
            &on_event,
            &run_id,
            &chat.id,
            &assistant_message_id,
            "Response stopped.".into(),
            empty_memory_trace()?,
            memory_revision.clone(),
            pi_execution.model_lock(),
            harness.clone(),
            None,
        );
    }
    let (learned_index, charter) = if request.use_memory {
        let cutoff = request
            .as_of
            .as_deref()
            .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
        let learned_index = collect_learned_memory_index(&state, &workspace, cutoff).await;
        let charter = collect_charter(&state, &workspace, cutoff).await;
        (learned_index, charter)
    } else {
        (Vec::new(), None)
    };
    let turn_envelope = agent_harness::turn_envelope_block(
        Utc::now(),
        request.use_memory,
        &learned_index,
        charter.as_deref(),
    )
    .unwrap_or_else(|_| {
        serde_json::to_string(&serde_json::json!({
            "live_time": agent_harness::live_time_envelope(Utc::now())
        }))
        .unwrap_or_else(|_| r#"{"live_time":{"not_evidence":true}}"#.into())
    });
    let pi_config = chat_pi_launch_config(
        &pi_artifacts,
        &state.artifacts.app_data_dir,
        &profile.id,
        &run_id,
        broker_socket,
        broker.token().to_owned(),
        turn_resources.pi_session(),
    );
    #[cfg(any(test, feature = "e2e"))]
    let pi_config = match crate::app::with_live_pi_agent_data_dir_override(pi_config) {
        Ok(config) => config,
        Err(_) => {
            let _ = broker.shutdown().await;
            return Ok(persist_or_emit_failed_chat_with_stop_precedence(
                state.inner(),
                &on_event,
                &run_id,
                &chat.id,
                &assistant_message_id,
                memory_revision.clone(),
                pi_execution.model_lock(),
                harness.clone(),
                None,
            ));
        }
    };
    let pi_config = match pi_config
        .with_skills(skill_files)
        .and_then(|config| config.with_host_context(host_context))
    {
        Ok(config) => config,
        Err(_) => {
            let _ = broker.shutdown().await;
            return Ok(persist_or_emit_failed_chat_with_stop_precedence(
                state.inner(),
                &on_event,
                &run_id,
                &chat.id,
                &assistant_message_id,
                memory_revision.clone(),
                pi_execution.model_lock(),
                harness.clone(),
                None,
            ));
        }
    };
    let resume = select_chat_pi_resume(
        intended_cache,
        had_prior_messages,
        turn_resources.pi_session().pi_session_id(),
        || uuid::Uuid::new_v4().to_string(),
    );
    let (mut pi, mut pi_events, execution_model, session_state, resumed) =
        match launch_chat_pi_resuming(
            pi_config.clone(),
            &pi_execution,
            turn_resources.pi_session_mut(),
            resume,
        )
        .await
        {
            Ok(launched) => launched,
            Err(_) => {
                let _ = broker.shutdown().await;
                return Ok(persist_or_emit_failed_chat_with_stop_precedence(
                    state.inner(),
                    &on_event,
                    &run_id,
                    &chat.id,
                    &assistant_message_id,
                    memory_revision.clone(),
                    pi_execution.model_lock(),
                    harness.clone(),
                    None,
                ));
            }
        };
    let mut historical_images = Vec::new();
    let pi_prompt = match pi_chat_turn_prompt(
        &current_prompt,
        &turn_envelope,
        (!resumed).then_some(ColdChatBootstrap {
            workbench: &chat_workbench,
            thread_id: &chat.id,
            history: {
                let history = &chat.messages[..chat.messages.len().saturating_sub(1)];
                #[cfg(feature = "e2e")]
                if crate::commands::attachments::e2e_omit_cold_history() {
                    &[]
                } else {
                    history
                }
                #[cfg(not(feature = "e2e"))]
                history
            },
            current_image_bytes,
            current_image_count: current_pi_images.len(),
            historical_images: &mut historical_images,
        }),
    ) {
        Ok(prompt) => prompt,
        Err(_) => {
            let pi_stopped = pi.shutdown(Duration::from_secs(1)).await.is_ok();
            let _ = broker.shutdown().await;
            if pi_stopped {
                let _ = turn_resources.pi_session_mut().wipe();
            }
            return Ok(persist_or_emit_failed_chat_with_stop_precedence(
                state.inner(),
                &on_event,
                &run_id,
                &chat.id,
                &assistant_message_id,
                memory_revision.clone(),
                execution_model,
                harness.clone(),
                None,
            ));
        }
    };
    let mut pi_images = historical_images;
    pi_images.extend(current_pi_images.iter().cloned());
    if *cancel.borrow() {
        let pi_stopped = pi.shutdown(Duration::from_secs(1)).await.is_ok();
        let _ = broker.shutdown().await;
        if pi_stopped {
            let _ = turn_resources.pi_session_mut().wipe();
        }
        return persist_and_emit_aborted_chat(
            state.store.as_ref(),
            &on_event,
            &run_id,
            &chat.id,
            &assistant_message_id,
            "Response stopped.".into(),
            empty_memory_trace()?,
            memory_revision.clone(),
            execution_model,
            harness.clone(),
            None,
        );
    }
    let prompt_request_id = match pi.prompt_with_images(&pi_prompt, &pi_images).await {
        Ok(id) => id,
        Err(_) => {
            let pi_stopped = pi.shutdown(Duration::from_secs(1)).await.is_ok();
            let _ = broker.shutdown().await;
            if pi_stopped {
                let _ = turn_resources.pi_session_mut().wipe();
            }
            return Ok(persist_or_emit_failed_chat_with_stop_precedence(
                state.inner(),
                &on_event,
                &run_id,
                &chat.id,
                &assistant_message_id,
                memory_revision.clone(),
                execution_model,
                harness.clone(),
                None,
            ));
        }
    };
    if *cancel.borrow() {
        let _ = pi.abort().await;
        let pi_stopped = pi.shutdown(Duration::from_secs(1)).await.is_ok();
        let _ = broker.shutdown().await;
        if pi_stopped {
            let _ = turn_resources.pi_session_mut().wipe();
        }
        return persist_and_emit_aborted_chat(
            state.store.as_ref(),
            &on_event,
            &run_id,
            &chat.id,
            &assistant_message_id,
            "Response stopped.".into(),
            empty_memory_trace()?,
            memory_revision.clone(),
            execution_model,
            harness.clone(),
            None,
        );
    }
    let task_run_id = run_id.clone();
    let thread_id = chat.id.clone();
    let task_message_id = assistant_message_id.clone();
    let task_guru_id = profile.id.clone();
    let task_use_memory = request.use_memory;
    let task_update_memory = request.update_memory;
    let task_connector_authority_sha256 = connector_authority.sha256;
    let task_connector_authority_cacheable = connector_authority.cacheable;
    let task_derived_session_id = session_state.session_id;
    let fallback_title = fallback_chat_title(if prompt.is_empty() {
        &prepared_attachments[0].record.filename
    } else {
        &prompt
    });
    let first_provider_body_progress_deadline =
        Instant::now() + CHAT_FIRST_PROVIDER_BODY_PROGRESS_TIMEOUT;
    tauri::async_runtime::spawn(async move {
        let mut content = String::new();
        let mut assistant_capture = AssistantTurnCapture::default();
        let mut completed = false;
        let mut aborted = false;
        let mut terminal_completion_claimed = false;
        let mut failure: Option<String> = None;
        let mut pending_controls = HashMap::new();
        let mut settled_controls = BTreeMap::new();
        let mut next_control_sequence = 0_u64;
        let mut next_control_to_persist = 0_u64;
        let mut progress = ChatProgressProjection::new(now_ms());
        let first_provider_body_progress_timer =
            tokio::time::sleep_until(first_provider_body_progress_deadline);
        tokio::pin!(first_provider_body_progress_timer);
        let mut saw_first_provider_body_progress = false;
        loop {
            tokio::select! {
                _ = &mut first_provider_body_progress_timer, if !saw_first_provider_body_progress => {
                    failure = Some("Pi did not emit provider body progress".into());
                    break;
                }
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        aborted = true;
                        let _ = pi.abort().await;
                        break;
                    }
                }
                control = chat_controls.recv() => {
                    let Some(control) = control else {
                        continue;
                    };
                    let prompt = control.prompt.clone();
                    match pi.steer(&prompt).await {
                        Ok(request_id) => {
                            let sequence = next_control_sequence;
                            next_control_sequence += 1;
                            pending_controls.insert(request_id, (sequence, control));
                        }
                        Err(_) => control.complete(Err(ChatControlError::Rejected(
                            "Pi did not accept the queued instruction".into(),
                        ))),
                    }
                }
                event = pi_events.recv() => {
                    if pi_event_indicates_first_provider_body_progress(&event) {
                        saw_first_provider_body_progress = true;
                    }
                    match event {
                        Ok(PiEvent::Rpc { payload }) => {
                            match payload.get("type").and_then(Value::as_str) {
                                Some("message_start") => {
                                    if let Err(message) = assistant_capture.observe_message_start(&payload) {
                                        failure = Some(message.into());
                                        let _ = pi.abort().await;
                                        break;
                                    }
                                    if payload
                                        .get("message")
                                        .and_then(|message| message.get("role"))
                                        .and_then(Value::as_str)
                                        == Some("assistant")
                                    {
                                        progress.start_assistant_turn();
                                    }
                                }
                                Some("tool_execution_start") => {
                                    if let (Some(tool_call_id), Some(tool_name)) = (
                                        payload.get("toolCallId").and_then(Value::as_str),
                                        payload.get("toolName").and_then(Value::as_str),
                                    ) {
                                        let args = payload.get("args").unwrap_or(&Value::Null);
                                        let web_source = if tool_name == "web_fetch" {
                                            if let Some(source_id) =
                                                args.get("source_id").and_then(Value::as_str)
                                            {
                                                capture
                                                    .web_sources
                                                    .lock()
                                                    .await
                                                    .get(source_id)
                                                    .cloned()
                                            } else if let Some(url) =
                                                args.get("url").and_then(Value::as_str)
                                            {
                                                crate::web::source_from_url(url).ok()
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        };
                                        progress.start_tool(
                                            tool_call_id,
                                            tool_name,
                                            args,
                                            web_source.as_ref(),
                                            now_ms(),
                                        );
                                        emit_chat_progress(&on_event, &task_run_id, &progress);
                                    }
                                }
                                Some("tool_execution_end") => {
                                    if let Some(tool_call_id) = payload
                                        .get("toolCallId")
                                        .and_then(Value::as_str)
                                    {
                                        progress.finish_tool(
                                            tool_call_id,
                                            payload.get("isError").and_then(Value::as_bool).unwrap_or(false),
                                            now_ms(),
                                        );
                                        emit_chat_progress(&on_event, &task_run_id, &progress);
                                    }
                                }
                                Some("compaction_start") => {
                                    progress.start_system(
                                        "compaction",
                                        ChatProgressOperation::Compact,
                                        "Compacting conversation context",
                                        None,
                                        now_ms(),
                                    );
                                    emit_chat_progress(&on_event, &task_run_id, &progress);
                                }
                                Some("compaction_end") => {
                                    progress.finish_system(
                                        "compaction",
                                        compaction_end_failed(&payload),
                                        now_ms(),
                                    );
                                    emit_chat_progress(&on_event, &task_run_id, &progress);
                                }
                                Some("auto_retry_start") => {
                                    let target = match (
                                        payload.get("attempt").and_then(Value::as_u64),
                                        payload.get("maxAttempts").and_then(Value::as_u64),
                                    ) {
                                        (Some(attempt), Some(max)) => Some(format!("attempt {attempt} of {max}")),
                                        (Some(attempt), None) => Some(format!("attempt {attempt}")),
                                        _ => None,
                                    };
                                    progress.start_system(
                                        "retry",
                                        ChatProgressOperation::Retry,
                                        "Retrying model request",
                                        target,
                                        now_ms(),
                                    );
                                    emit_chat_progress(&on_event, &task_run_id, &progress);
                                }
                                Some("auto_retry_end") => {
                                    progress.finish_system(
                                        "retry",
                                        payload.get("success").and_then(Value::as_bool) == Some(false),
                                        now_ms(),
                                    );
                                    emit_chat_progress(&on_event, &task_run_id, &progress);
                                }
                                Some("message_update") => {
                                    let assistant_event = payload.get("assistantMessageEvent");
                                    match assistant_event
                                        .and_then(|event| event.get("type"))
                                        .and_then(Value::as_str)
                                    {
                                        Some("thinking_delta" | "thinking_end") => {}
                                        Some("text_delta") => {
                                            if let Some(delta) = assistant_event
                                                .and_then(|event| event.get("delta"))
                                                .and_then(Value::as_str)
                                            {
                                                if let Err(message) = assistant_capture
                                                    .observe_text_delta(delta, MAX_CHAT_OUTPUT_BYTES)
                                                {
                                                    failure = Some(message.into());
                                                    let _ = pi.abort().await;
                                                    break;
                                                }
                                                progress.append_commentary(delta);
                                                emit_chat_progress(&on_event, &task_run_id, &progress);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                Some("message_end") => {
                                    match assistant_capture
                                        .observe_message_end(&payload, MAX_CHAT_OUTPUT_BYTES)
                                    {
                                        Ok(Some(AssistantTurnEnd::Stop(authoritative))) => {
                                            progress.finish_assistant_turn(true);
                                            let streamed = if content.is_empty() {
                                                authoritative.clone()
                                            } else {
                                                format!("\n\n{authoritative}")
                                            };
                                            content.push_str(&streamed);
                                            emit_chat_progress(&on_event, &task_run_id, &progress);
                                            let _ = on_event.send(ChatStreamEvent::Delta {
                                                run_id: task_run_id.clone(),
                                                text: streamed,
                                            });
                                        }
                                        Ok(Some(AssistantTurnEnd::ToolUse(_))) => {
                                            progress.finish_assistant_turn(false);
                                            emit_chat_progress(&on_event, &task_run_id, &progress);
                                        }
                                        Ok(Some(AssistantTurnEnd::Length | AssistantTurnEnd::Error | AssistantTurnEnd::Aborted)) => {
                                            progress.finish_assistant_turn(false);
                                        }
                                        Ok(None) => {}
                                        Err(message) => {
                                            failure = Some(message.into());
                                            let _ = pi.abort().await;
                                            break;
                                        }
                                    }
                                }
                                Some("agent_settled") => {
                                    match assistant_capture.finish_settled() {
                                        Ok(authoritative) => match app_state
                                            .claim_run_completion(&task_run_id, RunKind::Chat)
                                        {
                                            Ok(true) => {
                                                terminal_completion_claimed = true;
                                                // The terminal run cutoff prevents a steer from
                                                // being accepted after this point. Every steer
                                                // already sent to Pi must still receive its RPC
                                                // acknowledgement before this answer can become
                                                // canonical; Pi may emit that acknowledgement
                                                // after `agent_settled`.
                                                reject_queued_chat_controls(&mut chat_controls);
                                                match settle_chat_controls_before_completion(
                                                    &mut pi_events,
                                                    &mut pending_controls,
                                                    &mut settled_controls,
                                                    &mut next_control_to_persist,
                                                    app_state.store.as_ref(),
                                                    &profile.id,
                                                    &thread_id,
                                                )
                                                .await
                                                {
                                                    Ok(()) => {
                                                        if content.is_empty() {
                                                            content = authoritative.to_owned();
                                                        }
                                                        completed = true;
                                                    }
                                                    Err(error) => failure = Some(error.message),
                                                }
                                                reject_queued_chat_controls(&mut chat_controls);
                                            }
                                            Ok(false) => aborted = true,
                                            Err(error) => failure = Some(error.message),
                                        },
                                        Err(message) => failure = Some(message.into()),
                                    }
                                    break;
                                }
                                Some("response") => {
                                    if let Some(request_id) = payload.get("id").and_then(Value::as_u64) {
                                        if record_chat_control_response(
                                            &mut pending_controls,
                                            &mut settled_controls,
                                            request_id,
                                            payload.get("success").and_then(Value::as_bool) == Some(true),
                                        ) {
                                            if let Err(error) = persist_contiguous_chat_controls(
                                                app_state.store.as_ref(),
                                                &profile.id,
                                                &thread_id,
                                                &mut settled_controls,
                                                &mut next_control_to_persist,
                                            ) {
                                                failure = Some(error.message);
                                                let _ = pi.abort().await;
                                                break;
                                            }
                                        } else if request_id == prompt_request_id
                                            && payload.get("success").and_then(Value::as_bool) == Some(false)
                                        {
                                            // A provider-controlled error may echo request or
                                            // authentication material. Keep it out of renderer
                                            // events and durable Chat state.
                                            failure = Some("Pi rejected the prompt".into());
                                            break;
                                        }
                                    }
                                }
                                Some("extension_error") => {
                                    failure = Some("Pi extension failed".into());
                                    break;
                                }
                                _ => {}
                            }
                        }
                        Ok(PiEvent::ProtocolError { message }) => {
                            failure = Some(message);
                            break;
                        }
                        Ok(PiEvent::Exited) => {
                            failure = Some(match pi.try_exit_code() {
                                Some(code) => {
                                    format!("Pi stopped before the turn settled (exit code {code})")
                                }
                                None => "Pi stopped before the turn settled".into(),
                            });
                            break;
                        }
                        Err(error) => {
                            if let Some(message) = pi_event_stream_failure(error) {
                                failure = Some(message.into());
                                break;
                            }
                        }
                    }
                }
            }
        }

        reject_pending_chat_controls(pending_controls, settled_controls);
        let cache_entries = if completed && !aborted {
            read_pi_entries(&pi, &mut pi_events, None).await.ok()
        } else {
            None
        };
        let pi_process_group_id = pi_process_group_id(&pi);
        let pi_stopped = if completed && !aborted {
            pi.shutdown_settled().await
        } else {
            pi.shutdown(Duration::from_secs(2)).await
        }
        .is_ok();
        if !pi_stopped {
            if let Err(error) =
                record_unconfirmed_pi_stop(turn_resources.pi_session(), pi_process_group_id)
            {
                failure = Some(error.message);
            }
        }
        // A Pi child owns the JSONL. Only publish a cursor after it has
        // acknowledged a settled shutdown, otherwise a still-running child
        // could append after the cursor is sealed.
        let connector_authority_unchanged = completed
            && !aborted
            && task_connector_authority_cacheable
            && capture_chat_connector_authority(&app_state, &task_guru_id)
                .map(|current| {
                    current.cacheable && current.sha256 == task_connector_authority_sha256
                })
                .unwrap_or(false);
        let sealed_cache = (pi_stopped && connector_authority_unchanged)
            .then(|| {
                cache_entries.and_then(|entries| {
                    let cache_scope = PiSessionCacheScope {
                        harness_digest: &harness.digest,
                        runtime_policy_sha256: &runtime_policy_sha256,
                        runtime_surface_sha256: &runtime_surface_sha256,
                        connector_authority_sha256: &task_connector_authority_sha256,
                        memory_access_enabled: task_use_memory,
                        memory_update_enabled: task_update_memory,
                        execution_model: &execution_model,
                    };
                    cache_from_entries(entries, &cache_scope, &task_derived_session_id)
                })
            })
            .flatten();
        let _ = broker.shutdown().await;
        capture.compute.shutdown().await;
        capture.shutdown_mcp().await;
        // Pi owns the JSONL session file. Stop it before wiping an incomplete
        // run so a still-draining child cannot recreate a cache we just removed.
        if (aborted || !completed) && pi_stopped {
            let _ = turn_resources.pi_session_mut().wipe();
        }

        if aborted {
            let progress_snapshot = progress.finish(now_ms(), true);
            let memories = capture
                .memories
                .lock()
                .await
                .values()
                .cloned()
                .collect::<Vec<_>>();
            let aborted_content = if content.trim().is_empty() {
                "Response stopped.".to_owned()
            } else {
                content.clone()
            };
            let persist_result = durable_memory_trace(memories, None, &[]).and_then(|trace| {
                persist_and_emit_aborted_chat(
                    app_state.store.as_ref(),
                    &on_event,
                    &task_run_id,
                    &thread_id,
                    &task_message_id,
                    aborted_content,
                    trace,
                    memory_revision.clone(),
                    execution_model.clone(),
                    harness.clone(),
                    progress_snapshot,
                )
            });
            match persist_result {
                Ok(_) => {}
                Err(error) => failure = Some(error.message),
            }
        } else if completed {
            let all_memories = capture
                .memories
                .lock()
                .await
                .values()
                .cloned()
                .collect::<Vec<_>>();
            if content.trim().is_empty() {
                failure = Some("Pi settled without an assistant message".into());
            } else {
                let message_id = task_message_id.clone();
                let proposals = {
                    let mut proposals = capture.proposal.lock().await.clone();
                    for proposal in &mut proposals {
                        proposal.source_message_id = Some(message_id.clone());
                    }
                    proposals
                };
                let decision = capture.decision.lock().await.clone();
                let memory_trace =
                    match durable_memory_trace(all_memories, decision.as_ref(), &proposals) {
                        Ok(trace) => trace,
                        Err(_) => {
                            let _ = persist_or_emit_failed_chat(
                                app_state.store.as_ref(),
                                &on_event,
                                &task_run_id,
                                &thread_id,
                                &task_message_id,
                                memory_revision.clone(),
                                execution_model.clone(),
                                harness.clone(),
                                progress.finish(now_ms(), false),
                            );
                            return;
                        }
                    };
                let title = should_generate_title.then(|| fallback_title.clone());
                let created_at_ms = now_ms();
                let progress_snapshot = progress.finish(created_at_ms, false);
                let artifact_commits = capture.artifacts.lock().await.clone();
                let artifact_refs = artifact_commits
                    .iter()
                    .map(|commit| commit.revision.artifact_ref(commit.artifact.title.clone()))
                    .collect::<Vec<_>>();
                // Reserve the FIFO writer while the Chat reader is still
                // active. The handoff then releases this turn's reader and
                // waits for older readers without allowing a new reader to
                // barge between Chat completion and canonical finalization.
                let (write_id, pending_writer) =
                    match memory_updates::reserve_chat_memory_finalization(
                        &app_state,
                        &task_guru_id,
                    ) {
                        Ok(reservation) => reservation,
                        Err(_) => {
                            let _ = persist_or_emit_failed_chat(
                                app_state.store.as_ref(),
                                &on_event,
                                &task_run_id,
                                &thread_id,
                                &task_message_id,
                                memory_revision.clone(),
                                execution_model.clone(),
                                harness.clone(),
                                progress_snapshot.clone(),
                            );
                            return;
                        }
                    };
                let memory_writer =
                    match turn_resources.handoff_to_memory_write(pending_writer).await {
                        Ok(writer) => writer,
                        Err(_) => {
                            let _ = persist_or_emit_failed_chat(
                                app_state.store.as_ref(),
                                &on_event,
                                &task_run_id,
                                &thread_id,
                                &task_message_id,
                                memory_revision.clone(),
                                execution_model.clone(),
                                harness.clone(),
                                progress_snapshot.clone(),
                            );
                            return;
                        }
                    };
                let finalized = memory_updates::apply_chat_memory_update_with_registered_finalize(
                    &app_state,
                    &task_guru_id,
                    &thread_id,
                    &message_id,
                    task_update_memory,
                    &capture,
                    write_id,
                    memory_writer,
                    |memory_update| {
                        let expected = app_state
                            .store
                            .get_chat(&thread_id)
                            .map_err(map_store)?
                            .ok_or_else(|| CommandError::not_found("chat thread"))?;
                        let mut chat = expected.clone();
                        let completed_message = ChatMessage {
                            id: message_id.clone(),
                            role: ChatRole::Assistant,
                            status: ChatMessageStatus::Complete,
                            content: content.clone(),
                            created_at_ms,
                            memory_refs: memory_trace.refs.clone(),
                            observed_exact_count: memory_trace.observed_exact_count,
                            refs_truncated: memory_trace.refs_truncated,
                            refs_digest: memory_trace.refs_digest.clone(),
                            memory_update,
                            memory_revision: memory_revision.clone(),
                            execution_model: Some(execution_model.clone()),
                            agent_harness: Some(harness.clone()),
                            decision: decision.clone(),
                            attachments: Vec::new(),
                            artifact_refs,
                            progress: progress_snapshot.clone(),
                        };
                        chat.messages.push(completed_message.clone());
                        let title_applied = match &title {
                            Some(title) if chat.title == "New chat" => {
                                chat.title = title.clone();
                                true
                            }
                            _ => false,
                        };
                        chat.pi_session_cache = sealed_cache.clone();
                        chat.updated_at_ms = created_at_ms.max(chat.updated_at_ms + 1).max(
                            artifact_commits
                                .iter()
                                .map(|commit| commit.artifact.updated_at_ms)
                                .max()
                                .unwrap_or_default(),
                        );
                        if artifact_commits.is_empty() {
                            app_state
                                .store
                                .replace_chat(&expected, &chat)
                                .map_err(map_store)?;
                        } else {
                            app_state
                                .store
                                .save_chat_with_artifacts(&expected, &chat, &artifact_commits)
                                .map_err(map_store)?;
                        }
                        Ok((title_applied, completed_message))
                    },
                )
                .await;
                match finalized {
                    Ok((_, (title_applied, completed_message))) => {
                        emit_completed_chat(
                            &on_event,
                            &task_run_id,
                            &completed_message,
                            if title_applied { title.clone() } else { None },
                            &execution_model,
                            &harness,
                        );
                    }
                    Err(error) => {
                        if let Some((completed_message, recovered_title)) =
                            recovered_canonical_completion(
                                app_state.store.as_ref(),
                                &thread_id,
                                &task_message_id,
                                CanonicalCompletionExpectation {
                                    content: &content,
                                    memory_revision: &memory_revision,
                                    execution_model: &execution_model,
                                    agent_harness: &harness,
                                    title: title.as_deref(),
                                },
                            )
                        {
                            emit_completed_chat(
                                &on_event,
                                &task_run_id,
                                &completed_message,
                                recovered_title,
                                &execution_model,
                                &harness,
                            );
                        } else {
                            failure = Some(error.message);
                        }
                    }
                }
            }
        }
        if failure.is_some() {
            let progress_snapshot = progress.finish(now_ms(), false);
            if terminal_completion_claimed {
                let _ = persist_or_emit_failed_chat(
                    app_state.store.as_ref(),
                    &on_event,
                    &task_run_id,
                    &thread_id,
                    &task_message_id,
                    memory_revision,
                    execution_model,
                    harness,
                    progress_snapshot,
                );
            } else {
                let _ = persist_or_emit_failed_chat_with_stop_precedence(
                    &app_state,
                    &on_event,
                    &task_run_id,
                    &thread_id,
                    &task_message_id,
                    memory_revision,
                    execution_model,
                    harness,
                    progress_snapshot,
                );
            }
        }
    });
    Ok(RunStarted { run_id })
}

#[cfg(test)]
mod memory_skill_policy_tests {
    use super::*;

    #[test]
    fn exact_wiki_or_lens_skill_token_forces_both_memory_controls() {
        for prompt in ["Review this $wiki now", "Update the $lens next"] {
            let mut use_memory = false;
            let mut update_memory = false;
            enforce_prompt_memory_policy(prompt, &mut use_memory, &mut update_memory);
            assert!(use_memory, "expected lock for {prompt}");
            assert!(update_memory, "expected lock for {prompt}");
        }
    }

    #[test]
    fn similar_text_does_not_change_the_requested_memory_policy() {
        for prompt in [
            "$wiki-extra",
            "prefix$wiki",
            "wiki",
            "$lens-extra",
            "prefix$lens",
            "lens",
        ] {
            let mut use_memory = false;
            let mut update_memory = false;
            enforce_prompt_memory_policy(prompt, &mut use_memory, &mut update_memory);
            assert!(!use_memory, "unexpected match for {prompt}");
            assert!(!update_memory, "unexpected match for {prompt}");
        }
    }

    #[test]
    fn chat_send_forces_both_memory_switches_when_the_prompt_selects_wiki_or_lens() {
        for prompt in [
            "Please $wiki the company facts",
            "Review this $lens against the outcome",
        ] {
            let mut request = ChatSendRequest {
                run_id: "run-1".into(),
                guru_id: "guru-a".into(),
                thread_id: "chat-a".into(),
                prompt: prompt.into(),
                use_memory: false,
                update_memory: false,
                as_of: None,
                model_profile_id: "fixture".into(),
                thinking_level: "off".into(),
                run_options: Default::default(),
                attachments: Vec::new(),
            };
            enforce_prompt_memory_policy(
                &request.prompt,
                &mut request.use_memory,
                &mut request.update_memory,
            );
            assert!(request.use_memory, "expected Use memory for {prompt}");
            assert!(request.update_memory, "expected Update memory for {prompt}");
        }
    }

    #[test]
    fn first_provider_body_watchdog_ignores_lifecycle_only_events() {
        use tokio::sync::broadcast::error::RecvError;

        let lifecycle_events = [
            ("response", serde_json::json!({"type": "response"})),
            ("agent start", serde_json::json!({"type": "agent_start"})),
            (
                "assistant message start",
                serde_json::json!({
                    "type": "message_start",
                    "message": {"role": "assistant"},
                }),
            ),
            (
                "user message end",
                serde_json::json!({
                    "type": "message_end",
                    "message": {"role": "user"},
                }),
            ),
            (
                "tool message end",
                serde_json::json!({
                    "type": "message_end",
                    "message": {"role": "tool"},
                }),
            ),
            (
                "message update without a provider event",
                serde_json::json!({"type": "message_update"}),
            ),
            (
                "nested assistant lifecycle event",
                serde_json::json!({
                    "type": "message_update",
                    "assistantMessageEvent": {"type": "message_start"},
                }),
            ),
            (
                "nested tool result event",
                serde_json::json!({
                    "type": "message_update",
                    "assistantMessageEvent": {"type": "tool_result"},
                }),
            ),
            (
                "tool execution start",
                serde_json::json!({"type": "tool_execution_start"}),
            ),
            (
                "tool execution end",
                serde_json::json!({"type": "tool_execution_end"}),
            ),
            (
                "compaction start",
                serde_json::json!({"type": "compaction_start"}),
            ),
            (
                "compaction end",
                serde_json::json!({"type": "compaction_end"}),
            ),
            (
                "retry start",
                serde_json::json!({"type": "auto_retry_start"}),
            ),
            ("retry end", serde_json::json!({"type": "auto_retry_end"})),
        ];
        for (description, payload) in lifecycle_events {
            assert!(
                !pi_event_indicates_first_provider_body_progress(&Ok(PiEvent::Rpc { payload })),
                "{description} is not provider body progress"
            );
        }
        assert!(!pi_event_indicates_first_provider_body_progress(&Err(
            RecvError::Lagged(1,)
        )));
        assert!(!pi_event_indicates_first_provider_body_progress(&Err(
            RecvError::Closed
        )));
        assert!(!pi_event_indicates_first_provider_body_progress(&Ok(
            PiEvent::Exited
        )));
        assert!(!pi_event_indicates_first_provider_body_progress(&Ok(
            PiEvent::ProtocolError {
                message: "malformed event".into(),
            }
        )));
        assert_eq!(pi_event_stream_failure(RecvError::Lagged(1)), None);
        assert_eq!(
            pi_event_stream_failure(RecvError::Closed),
            Some("Pi event stream was interrupted")
        );
    }

    #[test]
    fn first_provider_body_watchdog_accepts_provider_body_updates_and_assistant_completion() {
        for assistant_event_type in [
            "thinking_start",
            "thinking_delta",
            "thinking_end",
            "text_start",
            "text_delta",
            "text_end",
            "toolcall_start",
            "toolcall_delta",
            "toolcall_end",
        ] {
            assert!(
                pi_event_indicates_first_provider_body_progress(&Ok(PiEvent::Rpc {
                    payload: serde_json::json!({
                        "type": "message_update",
                        "assistantMessageEvent": {"type": assistant_event_type},
                    }),
                })),
                "{assistant_event_type} is provider body progress"
            );
        }
        assert!(!pi_event_indicates_first_provider_body_progress(&Ok(
            PiEvent::Rpc {
                payload: serde_json::json!({"type": "message_update"}),
            }
        )));
        assert!(pi_event_indicates_first_provider_body_progress(&Ok(
            PiEvent::Rpc {
                payload: serde_json::json!({
                    "type": "message_end",
                    "message": {"role": "assistant"},
                }),
            }
        )));
    }

    #[test]
    fn compaction_end_marks_progress_failed_from_pi_result_not_a_success_field() {
        use crate::chat_progress::ChatProgressStatus;

        let succeeded = serde_json::json!({
            "type": "compaction_end",
            "reason": "threshold",
            "result": {
                "summary": "Kept the latest cash-flow debate.",
                "firstKeptEntryId": "kept-1",
                "tokensBefore": 150000
            },
            "aborted": false,
            "willRetry": false
        });
        let overflow_retry = serde_json::json!({
            "type": "compaction_end",
            "reason": "overflow",
            "result": {
                "summary": "Recovered after a context overflow.",
                "firstKeptEntryId": "kept-2",
                "tokensBefore": 180000
            },
            "aborted": false,
            "willRetry": true
        });
        let failed = serde_json::json!({
            "type": "compaction_end",
            "reason": "overflow",
            "result": null,
            "aborted": false,
            "willRetry": false,
            "errorMessage": "Context overflow recovery failed: quota exceeded"
        });
        let aborted = serde_json::json!({
            "type": "compaction_end",
            "reason": "manual",
            "result": null,
            "aborted": true,
            "willRetry": false
        });

        assert!(!compaction_end_failed(&succeeded));
        assert!(!compaction_end_failed(&overflow_retry));
        assert!(compaction_end_failed(&failed));
        assert!(compaction_end_failed(&aborted));
        // A success field must not hide a missing result — that is Pi's real schema.
        assert!(compaction_end_failed(&serde_json::json!({
            "type": "compaction_end",
            "success": true,
            "result": null,
            "aborted": false
        })));

        let mut projection = ChatProgressProjection::new(1);
        projection.start_system(
            "compaction",
            ChatProgressOperation::Compact,
            "Compacting conversation context",
            None,
            1,
        );
        projection.finish_system("compaction", compaction_end_failed(&failed), 2);
        assert!(matches!(
            projection.snapshot().items.as_slice(),
            [crate::chat_progress::ChatProgressItem::System {
                operation: ChatProgressOperation::Compact,
                status: ChatProgressStatus::Failed,
                ..
            }]
        ));
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_abort(run_id: String, state: State<'_, AppState>) -> Result<(), CommandError> {
    validated_chat_run_id(&run_id)?;
    state.cancel_run(&run_id, RunKind::Chat).await
}

fn validated_chat_run_id(value: &str) -> Result<String, CommandError> {
    crate::run_id::validate_run_id(value, "Chat")
}
