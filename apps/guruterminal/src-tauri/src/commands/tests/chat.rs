use super::*;

const TURN_ENVELOPE: &str = r#"{"live_time":{"current_date_utc":"2026-08-10"}}"#;

#[test]
fn chat_attachments_are_decoded_and_bounded_before_persistence() {
    let bytes = b"bounded attachment";
    let prepared = prepare_chat_attachments(&[ChatAttachmentInput {
        filename: "notes.txt".into(),
        media_type: "text/plain".into(),
        data_base64: BASE64.encode(bytes),
    }])
    .unwrap();

    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].bytes, bytes);
    assert_eq!(prepared[0].record.filename, "notes.txt");
    assert_eq!(prepared[0].record.media_type, "text/plain");
    assert_eq!(prepared[0].record.size_bytes, bytes.len() as u64);
    assert_eq!(prepared[0].record.content_sha256, sha256(bytes));

    assert!(prepare_chat_attachments(&[ChatAttachmentInput {
        filename: "../escape.txt".into(),
        media_type: "text/plain".into(),
        data_base64: BASE64.encode(bytes),
    }])
    .is_err());
    assert!(prepare_chat_attachments(&[ChatAttachmentInput {
        filename: "invalid.txt".into(),
        media_type: "text/plain".into(),
        data_base64: BASE64.encode([0xff, 0xfe]),
    }])
    .is_err());
    assert!(prepare_chat_attachments(&[ChatAttachmentInput {
        filename: "notes.txt".into(),
        media_type: "text/plain".into(),
        data_base64: "not-base64".into(),
    }])
    .is_err());

    let too_many = (0..=MAX_CHAT_ATTACHMENTS)
        .map(|index| ChatAttachmentInput {
            filename: format!("attachment-{index}.txt"),
            media_type: "text/plain".into(),
            data_base64: BASE64.encode(bytes),
        })
        .collect::<Vec<_>>();
    assert!(prepare_chat_attachments(&too_many).is_err());
}

#[test]
fn chat_attachments_extract_text_documents_and_keep_images_unextracted() {
    let notes = prepare_chat_attachments(&[ChatAttachmentInput {
        filename: "notes.txt".into(),
        media_type: "text/plain".into(),
        data_base64: BASE64.encode(b"Revenue quality improved."),
    }])
    .unwrap();
    match &notes[0].extract_status {
        AttachmentExtractStatus::Extracted(extraction) => {
            assert_eq!(extraction.kind, "text");
            assert_eq!(extraction.parts.len(), 1);
            assert_eq!(extraction.parts[0], b"Revenue quality improved.");
        }
        other => panic!("expected extracted text, got {other:?}"),
    }

    let image = prepare_chat_attachments(&[ChatAttachmentInput {
        filename: "chart.png".into(),
        media_type: "image/png".into(),
        data_base64: BASE64.encode([1_u8; 8]),
    }])
    .unwrap();
    assert!(matches!(
        image[0].extract_status,
        AttachmentExtractStatus::NotApplicable
    ));

    let oversized_image = vec![2_u8; MAX_CHAT_IMAGE_ATTACHMENT_BYTES + 1];
    assert!(prepare_chat_attachments(&[ChatAttachmentInput {
        filename: "huge.png".into(),
        media_type: "image/png".into(),
        data_base64: BASE64.encode(oversized_image),
    }])
    .is_err());
}

#[test]
fn attachment_prompt_points_at_extracted_markdown() {
    let prepared = prepare_chat_attachments(&[ChatAttachmentInput {
        filename: "notes.txt".into(),
        media_type: "text/plain".into(),
        data_base64: BASE64.encode(b"Inspect this note."),
    }])
    .unwrap();
    let prompt = attachment_prompt("Summarize", "chat-a", "message-a", &prepared);
    assert!(prompt.contains("extracted_path"));
    assert!(prompt.contains(&format!("{}.md", prepared[0].record.id)));
    assert!(prompt.contains("read and grep"));
}

#[test]
fn chat_attachment_reads_are_bound_to_the_exact_file_digest() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("attachment-1");
    let bytes = b"exact attachment";
    fs::write(&path, bytes).unwrap();
    let attachment = ChatAttachment {
        id: "attachment-1".into(),
        filename: "notes.txt".into(),
        media_type: "text/plain".into(),
        size_bytes: bytes.len() as u64,
        content_sha256: sha256(bytes),
    };

    assert_eq!(
        read_chat_attachment_file(&path, &attachment).unwrap(),
        bytes
    );
    fs::write(&path, b"other attachment").unwrap();
    assert!(read_chat_attachment_file(&path, &attachment).is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let link = temporary.path().join("attachment-link");
        symlink(&path, &link).unwrap();
        assert!(read_chat_attachment_file(&link, &attachment).is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn chat_immediate_abort_is_admitted_before_preflight_and_never_starts_pi() {
    use std::{
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tauri::Manager;

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    initialized_workspace(&workspace, "chat-abort-preflight");
    let runtime_path = temporary.path().join("slow-guruterminal-core");
    fs::write(&runtime_path, "#!/bin/sh\nsleep 0.2\nprintf '[]\\n'\n").unwrap();
    fs::set_permissions(&runtime_path, fs::Permissions::from_mode(0o700)).unwrap();

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let agent_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("agent");
    Arc::get_mut(&mut state.artifacts).unwrap().pi = Some(crate::app::PiArtifacts {
        executable: temporary.path().join("must-not-start-pi"),
        runtime_dir: temporary.path().join("unused-pi-runtime"),
        extension: agent_root.join("guruterminal-extension.mjs"),
        provider_extension: agent_root.join("guruterminal-provider-extension.mjs"),
        system_prompt: agent_root.join("SYSTEM.md"),
        provider: String::new(),
        model: String::new(),
        thinking_level: String::new(),
        run_options: std::collections::BTreeMap::new(),
        provider_credential: None,
    });
    let profile = profile("guru-chat-abort", &workspace, 1);
    seed_profile(state.store.as_ref(), &profile);
    let chat = chat("chat-abort", &profile.id, 1);
    state.store.create_chat(&chat).unwrap();

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert!(app.manage(state));
    let event_count = Arc::new(AtomicUsize::new(0));
    let captured_events = event_count.clone();
    let channel = Channel::new(move |_| {
        captured_events.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let run_id = "chat-immediate-abort".to_owned();
    let send = chat_send(
        ChatSendRequest {
            run_id: run_id.clone(),
            guru_id: profile.id.clone(),
            thread_id: chat.id.clone(),
            prompt: "This prompt must never reach Pi.".into(),
            use_memory: false,
            update_memory: false,
            as_of: None,
            model_profile_id: "fixture".into(),
            thinking_level: "medium".into(),
            run_options: std::collections::BTreeMap::new(),
            attachments: Vec::new(),
        },
        channel,
        app.state(),
    );
    let abort = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if app
                    .state::<AppState>()
                    .run_coordinator
                    .activities()
                    .unwrap()
                    .iter()
                    .any(|activity| activity.run_id == run_id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
            app.state::<AppState>()
                .cancel_run(&run_id, RunKind::Chat)
                .await
        })
        .await
        .expect("Chat admission appeared before preflight timeout")
    };
    let (started, aborted) = tokio::join!(send, abort);
    aborted.expect("immediate Chat abort addresses the reserved run");
    assert_eq!(started.unwrap().run_id, run_id);

    let state = app.state::<AppState>();
    assert_eq!(state.run_coordinator.active_count(), 0);
    assert_eq!(event_count.load(Ordering::SeqCst), 2);
    assert!(fs::read_dir(&state.artifacts.broker_dir)
        .unwrap()
        .next()
        .is_none());
    assert!(fs::read_dir(&state.artifacts.process_lease_dir)
        .unwrap()
        .next()
        .is_none());
    let stored = state.store.get_chat(&chat.id).unwrap().unwrap();
    assert_eq!(stored.messages.len(), 2);
    assert_eq!(stored.messages[0].role, ChatRole::User);
    assert_eq!(stored.messages[0].status, ChatMessageStatus::Complete);
    assert_eq!(stored.messages[1].role, ChatRole::Assistant);
    assert_eq!(stored.messages[1].status, ChatMessageStatus::Aborted);
    assert_eq!(stored.messages[1].content, "Response stopped.");
    assert_eq!(
        stored.messages[1]
            .execution_model
            .as_ref()
            .map(|model| model.thinking_level.as_str()),
        Some("medium")
    );
}

#[cfg(unix)]
#[test]
fn cold_pi_bootstrap_is_bounded_and_never_forwards_progress() {
    let history = vec![
        ChatMessage {
            id: "message-1".into(),
            role: ChatRole::User,
            status: crate::domain::ChatMessageStatus::Complete,
            content: "First question".into(),
            created_at_ms: 1,
            memory_refs: Vec::new(),
            observed_exact_count: 0,
            refs_truncated: false,
            refs_digest: memory_refs_digest(&[]).unwrap(),
            memory_update: None,
            memory_revision: None,
            execution_model: None,
            agent_harness: None,
            decision: None,
            attachments: Vec::new(),
            artifact_refs: Vec::new(),
            progress: None,
        },
        ChatMessage {
            id: "message-2".into(),
            role: ChatRole::Assistant,
            status: crate::domain::ChatMessageStatus::Complete,
            content: "First answer".into(),
            created_at_ms: 2,
            memory_refs: Vec::new(),
            observed_exact_count: 0,
            refs_truncated: false,
            refs_digest: memory_refs_digest(&[]).unwrap(),
            memory_update: None,
            memory_revision: None,
            execution_model: None,
            agent_harness: None,
            decision: None,
            attachments: Vec::new(),
            artifact_refs: Vec::new(),
            progress: None,
        },
    ];

    let temporary = tempfile::tempdir().unwrap();
    let mut historical_images = Vec::new();
    let prompt = bootstrap_pi_chat_session_from_sqlite(
        temporary.path(),
        "chat-a",
        &history,
        "Follow-up question",
        TURN_ENVELOPE,
        0,
        0,
        &mut historical_images,
    )
    .unwrap();
    assert!(prompt.contains(r#""content":"First question""#));
    assert!(prompt.contains(r#""role":"user""#));
    assert!(prompt.contains(r#""content":"First answer""#));
    assert!(prompt.contains(r#""role":"assistant""#));
    assert!(prompt.contains("<turn_envelope>"));
    assert!(prompt.contains("2026-08-10"));
    assert!(!prompt.contains("PRIVATE_PROGRESS_MARKER"));
    assert!(!prompt.contains("progress"));
    assert!(prompt.ends_with("<current_user_message>\nFollow-up question\n</current_user_message>"));
    let empty = bootstrap_pi_chat_session_from_sqlite(
        temporary.path(),
        "chat-a",
        &[],
        "New question",
        TURN_ENVELOPE,
        0,
        0,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(empty.contains("<turn_envelope>"));
    assert!(empty.contains("New question"));
    let rebuilt = pi_chat_turn_prompt(
        "Second turn only",
        TURN_ENVELOPE,
        Some(ColdChatBootstrap {
            workbench: temporary.path(),
            thread_id: "chat-a",
            history: &history,
            current_image_bytes: 0,
            current_image_count: 0,
            historical_images: &mut Vec::new(),
        }),
    )
    .unwrap();
    assert!(rebuilt.contains("First answer"));
    assert!(rebuilt.ends_with("<current_user_message>\nSecond turn only\n</current_user_message>"));
    let warm = pi_chat_turn_prompt("Warm follow-up", TURN_ENVELOPE, None).unwrap();
    assert!(warm.contains("<turn_envelope>"));
    assert!(warm.contains("Warm follow-up"));
    assert!(!warm.contains("conversation_history_jsonl"));
}

#[test]
fn cold_chat_bootstrap_restores_attachment_only_text_turn_from_canonical_files() {
    let temporary = tempfile::tempdir().unwrap();
    let workbench = temporary.path();
    let bytes = b"Revenue quality improved after low-margin churn.";
    let attachment = ChatAttachment {
        id: "attachment-a".into(),
        filename: "cohort-notes.txt".into(),
        media_type: "text/plain".into(),
        size_bytes: bytes.len() as u64,
        content_sha256: sha256(bytes),
    };
    let directory = workbench.join("attachments/chat-a/message-attachment-only");
    fs::create_dir_all(&directory).unwrap();
    let attachment_id = attachment.id.clone();
    fs::write(directory.join(&attachment_id), bytes).unwrap();
    let history = vec![ChatMessage {
        id: "message-attachment-only".into(),
        role: ChatRole::User,
        status: crate::domain::ChatMessageStatus::Complete,
        content: String::new(),
        created_at_ms: 1,
        memory_refs: Vec::new(),
        observed_exact_count: 0,
        refs_truncated: false,
        refs_digest: memory_refs_digest(&[]).unwrap(),
        memory_update: None,
        memory_revision: None,
        execution_model: None,
        agent_harness: None,
        decision: None,
        attachments: vec![attachment],
        artifact_refs: Vec::new(),
        progress: None,
    }];

    let prompt = bootstrap_pi_chat_session_from_sqlite(
        workbench,
        "chat-a",
        &history,
        "What did the attachment establish?",
        TURN_ENVELOPE,
        0,
        0,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(prompt.contains("cohort-notes.txt"));
    assert!(prompt.contains("Revenue quality improved after low-margin churn."));
    assert!(prompt.contains("content_sha256"));
    assert!(prompt.contains("message-attachment-only"));

    fs::write(
        directory.join(format!("{attachment_id}.md")),
        "Extracted prefix from the original note.",
    )
    .unwrap();
    let extracted_prompt = bootstrap_pi_chat_session_from_sqlite(
        workbench,
        "chat-a",
        &history,
        "What did the attachment establish?",
        TURN_ENVELOPE,
        0,
        0,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(extracted_prompt.contains("Extracted prefix from the original note."));
    assert!(extracted_prompt.contains("extracted_markdown"));
}

#[test]
fn cold_chat_bootstrap_degrades_invalid_historical_text_to_metadata() {
    let temporary = tempfile::tempdir().unwrap();
    let bytes = [0xff, 0xfe];
    let attachment = ChatAttachment {
        id: "attachment-invalid".into(),
        filename: "legacy.txt".into(),
        media_type: "text/plain".into(),
        size_bytes: bytes.len() as u64,
        content_sha256: sha256(&bytes),
    };
    let directory = temporary.path().join("attachments/chat-a/message-invalid");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(&attachment.id), bytes).unwrap();
    let history = vec![ChatMessage {
        id: "message-invalid".into(),
        role: ChatRole::User,
        status: crate::domain::ChatMessageStatus::Complete,
        content: "Legacy attachment".into(),
        created_at_ms: 1,
        memory_refs: Vec::new(),
        observed_exact_count: 0,
        refs_truncated: false,
        refs_digest: memory_refs_digest(&[]).unwrap(),
        memory_update: None,
        memory_revision: None,
        execution_model: None,
        agent_harness: None,
        decision: None,
        attachments: vec![attachment],
        artifact_refs: Vec::new(),
        progress: None,
    }];

    let prompt = bootstrap_pi_chat_session_from_sqlite(
        temporary.path(),
        "chat-a",
        &history,
        "Continue",
        TURN_ENVELOPE,
        0,
        0,
        &mut Vec::new(),
    )
    .unwrap();

    assert!(prompt.contains("legacy.txt"));
    assert!(prompt.contains("metadata_and_digest_only"));
    assert!(!prompt.contains("text_prefix"));
}

#[test]
fn cold_chat_historical_images_respect_the_four_image_turn_budget() {
    let temporary = tempfile::tempdir().unwrap();
    let workbench = temporary.path();
    let message_id = "message-images";
    let directory = workbench.join(format!("attachments/chat-a/{message_id}"));
    fs::create_dir_all(&directory).unwrap();
    let mut attachments = Vec::new();
    for index in 0..2 {
        let id = format!("image-{index}");
        let bytes = vec![index as u8 + 1; 8];
        fs::write(directory.join(&id), &bytes).unwrap();
        attachments.push(ChatAttachment {
            id,
            filename: format!("chart-{index}.png"),
            media_type: "image/png".into(),
            size_bytes: bytes.len() as u64,
            content_sha256: sha256(&bytes),
        });
    }
    let history = vec![ChatMessage {
        id: message_id.into(),
        role: ChatRole::User,
        status: crate::domain::ChatMessageStatus::Complete,
        content: "Compare these charts".into(),
        created_at_ms: 1,
        memory_refs: Vec::new(),
        observed_exact_count: 0,
        refs_truncated: false,
        refs_digest: memory_refs_digest(&[]).unwrap(),
        memory_update: None,
        memory_revision: None,
        execution_model: None,
        agent_harness: None,
        decision: None,
        attachments,
        artifact_refs: Vec::new(),
        progress: None,
    }];

    for (current_count, expected_historical) in [(0, 2), (2, 2), (3, 1), (4, 0)] {
        let mut historical = Vec::new();
        bootstrap_pi_chat_session_from_sqlite(
            workbench,
            "chat-a",
            &history,
            "Continue",
            TURN_ENVELOPE,
            0,
            current_count,
            &mut historical,
        )
        .unwrap();
        assert_eq!(historical.len(), expected_historical);
        assert!(historical.len() + current_count <= MAX_CHAT_ATTACHMENTS);
    }
}
