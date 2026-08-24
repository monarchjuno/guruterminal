use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use serde_json::Value;
use tauri::{ipc::Channel, State};

use crate::{
    agent_harness::{self, AgentHarnessSnapshot},
    app::{AppState, CommandError},
    broker::{start_tool_broker, tool_broker_endpoint, ToolPolicy, MAX_MEMORY_PROPOSALS},
    chat_control::{chat_control_channel, AcceptedChatControl, ChatControlError, ChatControlKind},
    chat_execution_session::ChatExecutionSession,
    chat_progress::{ChatProgress, ChatProgressOperation, ChatProgressProjection},
    chat_turn::ChatTurnResources,
    domain::{
        memory_refs_digest, CanonicalMemoryKind, ChatDecision, ChatMessage, ChatMessageStatus,
        ChatRole, MemoryAccess, MemoryPolicy, MemoryProposal, MemoryRefSnapshot, PiSessionCache,
        MAX_MEMORY_REFS,
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

use super::{
    attachments::{
        attachment_prompt, persist_chat_attachments, pi_chat_turn_prompt, prepare_chat_attachments,
        ColdChatBootstrap,
    },
    current_user_skill_snapshots, enabled_execute_capability_ids, enabled_skill_ids,
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
    Cold,
    Warm { cache: PiSessionCache },
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
    CommandError,
> {
    session.validate_current_binding()?;
    let pi = PiProcess::spawn(config)
        .await
        .map_err(|error| CommandError::new("pi_unavailable", error.to_string()))?;
    let mut events = pi.subscribe();
    let file_requirement = match resume {
        ChatPiResume::Cold => PiSessionFileRequirement::ColdMayBeUnpersisted,
        ChatPiResume::Warm { .. } => PiSessionFileRequirement::Persisted,
    };
    match configure_pi_session_execution(
        &pi,
        &mut events,
        execution,
        session.pi_session_id(),
        session.session_directory(),
        file_requirement,
    )
    .await
    {
        Ok((model, state)) => {
            let entries = match read_pi_entries(&pi, &mut events, None).await {
                Ok(entries) => entries,
                Err(error) => {
                    let _ = pi.shutdown(Duration::from_secs(1)).await;
                    return Err(error);
                }
            };
            let acceptable = match resume {
                ChatPiResume::Cold => entries.cold_startup_only,
                ChatPiResume::Warm { cache } => entries.matches_cache(cache),
            };
            if !acceptable {
                let _ = pi.shutdown(Duration::from_secs(1)).await;
                return Err(CommandError::new(
                    "pi_unavailable",
                    match resume {
                        ChatPiResume::Cold => "cold Pi session loaded unexpected prior context",
                        ChatPiResume::Warm { .. } => "warm Pi session cache digest mismatched",
                    },
                ));
            }
            Ok((pi, events, model, state))
        }
        Err(error) => {
            let _ = pi.shutdown(Duration::from_secs(1)).await;
            Err(error)
        }
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
    if matches!(resume, ChatPiResume::Warm { .. }) {
        match launch_chat_pi(config.clone(), execution, session, &resume).await {
            Ok((pi, events, model, state)) => return Ok((pi, events, model, state, true)),
            Err(_) => session.wipe()?,
        }
    } else {
        session.wipe()?;
    }
    let (pi, events, model, state) =
        launch_chat_pi(config, execution, session, &ChatPiResume::Cold).await?;
    Ok((pi, events, model, state, false))
}

fn cache_from_entries(
    entries: PiEntriesState,
    harness: &AgentHarnessSnapshot,
    execution_model: &ExecutionModelLock,
) -> Option<PiSessionCache> {
    let cache = PiSessionCache {
        entries_sha256: entries.entries_sha256,
        leaf_id: entries.leaf_id,
        harness_digest: harness.digest.clone(),
        execution_model: execution_model.clone(),
    };
    cache.validate().ok().map(|_| cache)
}

pub(super) fn bind_chat_pi_session(
    config: PiLaunchConfig,
    session: &ChatExecutionSession,
) -> Result<PiLaunchConfig, PiError> {
    config.with_session(session.pi_config())
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
    let expected = store
        .get_chat(thread_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("chat thread"))?;
    let created_at_ms = now_ms().max(expected.updated_at_ms + 1);
    let mut chat = expected.clone();
    chat.messages.push(ChatMessage {
        id: message_id.to_owned(),
        role: ChatRole::Assistant,
        status: ChatMessageStatus::Aborted,
        content,
        created_at_ms,
        memory_refs: memory_trace.refs,
        observed_exact_count: memory_trace.observed_exact_count,
        refs_truncated: memory_trace.refs_truncated,
        refs_digest: memory_trace.refs_digest,
        memory_update: None,
        memory_revision,
        execution_model: Some(execution_model),
        agent_harness: Some(agent_harness),
        decision: None,
        attachments: Vec::new(),
        artifact_refs: Vec::new(),
        progress,
    });
    chat.updated_at_ms = created_at_ms;
    store.replace_chat(&expected, &chat).map_err(map_store)?;
    let _ = on_event.send(ChatStreamEvent::Aborted {
        run_id: run_id.to_owned(),
    });
    Ok(RunStarted {
        run_id: run_id.to_owned(),
    })
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
    let capability_ids = enabled_execute_capability_ids(&state, &profile.id)?;
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
    let workspace = profile_workspace(&profile)?;
    let chat_workbench = managed_guru_dir(state.inner(), &profile.id)?.join("workbench");
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
    let intended_cache = chat
        .pi_session_cache
        .clone()
        .filter(|cache| cache.matches(&harness.digest, &pi_execution.model_lock()));
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
    if let Err(error) =
        state
            .store
            .set_guru_last_model_profile(&profile.id, &configured_model.id, timestamp)
    {
        return Err(map_store(error));
    }

    let assistant_message_id = new_id("message");
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
        Err(error) => {
            return Err(map_internal(error));
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
        (
            collect_learned_memory_index(&state, &workspace, cutoff).await,
            collect_charter(&state, &workspace, cutoff).await,
        )
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
    let pi_config = pi_artifacts.launch_config(
        &state.artifacts.app_data_dir,
        &profile.id,
        &run_id,
        state
            .artifacts
            .app_data_dir
            .join("gurus")
            .join(&profile.id)
            .join("workbench"),
        broker_socket,
        broker.token().to_owned(),
    );
    #[cfg(any(test, feature = "e2e"))]
    let pi_config = match crate::app::with_live_pi_agent_data_dir_override(pi_config) {
        Ok(config) => config,
        Err(error) => {
            let _ = broker.shutdown().await;
            return Err(error);
        }
    };
    let pi_config = match pi_config
        .with_skills(skill_files)
        .and_then(|config| config.with_host_context(host_context))
        .and_then(|config| bind_chat_pi_session(config, turn_resources.pi_session()))
    {
        Ok(config) => config,
        Err(error) => {
            let _ = broker.shutdown().await;
            return Err(CommandError::new("pi_unavailable", error.to_string()));
        }
    };
    let resume = match intended_cache {
        Some(cache) => ChatPiResume::Warm { cache },
        None => ChatPiResume::Cold,
    };
    let (mut pi, mut pi_events, execution_model, _session_state, resumed) =
        match launch_chat_pi_resuming(
            pi_config.clone(),
            &pi_execution,
            turn_resources.pi_session_mut(),
            resume,
        )
        .await
        {
            Ok(launched) => launched,
            Err(error) => {
                let _ = broker.shutdown().await;
                return Err(error);
            }
        };
    let mut historical_images = Vec::new();
    let pi_prompt = match pi_chat_turn_prompt(
        &current_prompt,
        &turn_envelope,
        (!resumed).then_some(ColdChatBootstrap {
            workbench: &chat_workbench,
            thread_id: &chat.id,
            history: &chat.messages[..chat.messages.len().saturating_sub(1)],
            current_image_bytes,
            current_image_count: current_pi_images.len(),
            historical_images: &mut historical_images,
        }),
    ) {
        Ok(prompt) => prompt,
        Err(error) => {
            let _ = pi.shutdown(Duration::from_secs(1)).await;
            let _ = broker.shutdown().await;
            return Err(error);
        }
    };
    let mut pi_images = historical_images;
    pi_images.extend(current_pi_images.iter().cloned());
    if *cancel.borrow() {
        let _ = turn_resources.pi_session_mut().wipe();
        let _ = pi.shutdown(Duration::from_secs(1)).await;
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
            execution_model,
            harness.clone(),
            None,
        );
    }
    let prompt_request_id = match pi.prompt_with_images(&pi_prompt, &pi_images).await {
        Ok(id) => id,
        Err(error) => {
            let _ = turn_resources.pi_session_mut().wipe();
            let _ = pi.shutdown(Duration::from_secs(1)).await;
            let _ = broker.shutdown().await;
            return Err(CommandError::new("pi_unavailable", error.to_string()));
        }
    };
    if *cancel.borrow() {
        let _ = pi.abort().await;
        let _ = turn_resources.pi_session_mut().wipe();
        let _ = pi.shutdown(Duration::from_secs(1)).await;
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
            execution_model,
            harness.clone(),
            None,
        );
    }
    let task_run_id = run_id.clone();
    let thread_id = chat.id.clone();
    let task_message_id = assistant_message_id.clone();
    let task_guru_id = profile.id.clone();
    let task_update_memory = request.update_memory;
    let fallback_title = fallback_chat_title(if prompt.is_empty() {
        &prepared_attachments[0].record.filename
    } else {
        &prompt
    });
    tauri::async_runtime::spawn(async move {
        let mut content = String::new();
        let mut assistant_capture = AssistantTurnCapture::default();
        let mut completed = false;
        let mut aborted = false;
        let mut failure: Option<String> = None;
        let mut pending_controls = HashMap::new();
        let mut progress = ChatProgressProjection::new(now_ms());
        loop {
            tokio::select! {
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
                    let accepted = persist_chat_control_message(
                        app_state.store.as_ref(),
                        &profile.id,
                        &thread_id,
                        &prompt,
                    );
                    match accepted {
                        Ok(accepted) => {
                            let sent = pi.steer(&prompt).await;
                            match sent {
                                Ok(request_id) => {
                                    pending_controls.insert(request_id, (control, accepted));
                                }
                                Err(_) => control.complete(Err(ChatControlError::Rejected(
                                    "Pi did not accept the queued instruction".into(),
                                ))),
                            }
                        }
                        Err(error) => control.complete(Err(ChatControlError::Rejected(error.message))),
                    }
                }
                event = pi_events.recv() => {
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
                                                if content.is_empty() {
                                                    content = authoritative.to_owned();
                                                }
                                                completed = true;
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
                                        if let Some((control, accepted)) = pending_controls.remove(&request_id) {
                                            if payload.get("success").and_then(Value::as_bool) == Some(true) {
                                                control.complete(Ok(accepted));
                                            } else {
                                                control.complete(Err(ChatControlError::Rejected(
                                                    "Pi rejected the queued instruction".into(),
                                                )));
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

        let sealed_cache = if completed && !aborted {
            read_pi_entries(&pi, &mut pi_events, None)
                .await
                .ok()
                .and_then(|entries| cache_from_entries(entries, &harness, &execution_model))
        } else {
            None
        };
        if aborted || !completed {
            let _ = turn_resources.pi_session_mut().wipe();
        }
        let _ = if completed && !aborted {
            pi.shutdown_settled().await
        } else {
            pi.shutdown(Duration::from_secs(2)).await
        };
        let _ = broker.shutdown().await;
        capture.compute.shutdown().await;
        capture.shutdown_mcp().await;

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
                        Err(error) => {
                            let _ = on_event.send(ChatStreamEvent::Error {
                                run_id: task_run_id.clone(),
                                message: error.message,
                            });
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
                        Err(error) => {
                            let _ = on_event.send(ChatStreamEvent::Error {
                                run_id: task_run_id.clone(),
                                message: error.message,
                            });
                            return;
                        }
                    };
                let memory_writer =
                    match turn_resources.handoff_to_memory_write(pending_writer).await {
                        Ok(writer) => writer,
                        Err(error) => {
                            let _ = on_event.send(ChatStreamEvent::Error {
                                run_id: task_run_id.clone(),
                                message: error.message,
                            });
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
                        chat.messages.push(ChatMessage {
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
                            memory_revision,
                            execution_model: Some(execution_model.clone()),
                            agent_harness: Some(harness.clone()),
                            decision: decision.clone(),
                            attachments: Vec::new(),
                            artifact_refs,
                            progress: progress_snapshot.clone(),
                        });
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
                        Ok(title_applied)
                    },
                )
                .await;
                match finalized {
                    Ok((memory_update, title_applied)) => {
                        if !memory_trace.refs.is_empty() {
                            let _ = on_event.send(ChatStreamEvent::Memory {
                                run_id: task_run_id.clone(),
                                memories: memory_trace.refs.iter().map(memory_ref_dto).collect(),
                            });
                        }
                        if title_applied {
                            if let Some(title) = title.clone() {
                                let _ = on_event.send(ChatStreamEvent::Title {
                                    run_id: task_run_id.clone(),
                                    title,
                                });
                            }
                        }
                        if let Some(decision) = &decision {
                            let _ = on_event.send(ChatStreamEvent::Decision {
                                run_id: task_run_id.clone(),
                                decision: decision.clone(),
                            });
                        }
                        if let Some(result) = memory_update {
                            let _ = on_event.send(ChatStreamEvent::MemoryUpdate {
                                run_id: task_run_id.clone(),
                                result: Box::new(result),
                            });
                        }
                        for commit in &artifact_commits {
                            let _ = on_event.send(ChatStreamEvent::Artifact {
                                run_id: task_run_id.clone(),
                                artifact: commit
                                    .revision
                                    .artifact_ref(commit.artifact.title.clone()),
                            });
                        }
                        let _ = on_event.send(ChatStreamEvent::Completed {
                            run_id: task_run_id.clone(),
                            message_id,
                            final_text: content.clone(),
                            created_at: iso_time(created_at_ms).unwrap_or_default(),
                            execution_model: Box::new(execution_model.clone()),
                            agent_harness: Box::new(harness.clone()),
                        });
                    }
                    Err(error) => failure = Some(error.message),
                }
            }
        }
        if let Some(message) = failure {
            let _ = on_event.send(ChatStreamEvent::Error {
                run_id: task_run_id.clone(),
                message,
            });
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
    fn a_lagged_pi_subscriber_recovers_but_a_closed_stream_fails() {
        use tokio::sync::broadcast::error::RecvError;

        assert_eq!(pi_event_stream_failure(RecvError::Lagged(1)), None);
        assert_eq!(
            pi_event_stream_failure(RecvError::Closed),
            Some("Pi event stream was interrupted")
        );
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
