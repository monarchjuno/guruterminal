use super::support::*;
use super::*;

#[test]
fn exact_chat_memory_update_and_provenance_round_trip_without_loss() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_guru(&store, &guru());
    let update = MemoryUpdateResult {
        status: MemoryUpdateStatus::Applied,
        commit_id: Some("commit-1".into()),
        changes: vec![MemoryUpdateChange {
            record_id: "lens:quality".into(),
            kind: "Lens".into(),
            operation: "create".into(),
            title: "Quality".into(),
            lesson: "Prefer durable earnings quality.".into(),
            basis: "Exact filing".into(),
            future_use: "Changes later earnings checks.".into(),
        }],
    };
    let memory_ref = MemoryRefSnapshot {
        record_id: "lens:source".into(),
        kind: "Lens".into(),
        title: "Source".into(),
        excerpt: "Exact section".into(),
        as_of: Some("2026-08-01T00:00:00Z".into()),
        section: Some("Boundary".into()),
        access: MemoryAccess::ExactRead,
        full_record_digest: None,
    };
    let refs_digest = memory_refs_digest(std::slice::from_ref(&memory_ref)).unwrap();
    let chat = ChatSession {
        id: "chat-1".into(),
        guru_id: "guru-1".into(),
        pi_session_id: "123e4567-e89b-42d3-a456-426614174001".into(),
        pi_session_cache: None,
        title: "Exact proposal".into(),
        memory_policy: MemoryPolicy::default(),
        messages: vec![ChatMessage {
            id: "message-1".into(),
            role: ChatRole::Assistant,
            status: crate::domain::ChatMessageStatus::Complete,
            content: "Answer".into(),
            created_at_ms: 2,
            memory_refs: vec![memory_ref],
            observed_exact_count: 1,
            refs_truncated: false,
            refs_digest,
            memory_update: Some(update.clone()),
            memory_revision: Some("tree-revision".into()),
            execution_model: None,
            agent_harness: None,
            decision: None,
            attachments: Vec::new(),
            artifact_refs: Vec::new(),
            progress: None,
        }],
        created_at_ms: 1,
        updated_at_ms: 2,
    };
    store.create_chat(&chat).unwrap();
    let loaded = store.get_chat(&chat.id).unwrap().unwrap();
    assert_eq!(loaded, chat);
    assert_eq!(loaded.messages[0].memory_update.as_ref(), Some(&update));
    assert_eq!(
        loaded.messages[0].memory_refs[0].access,
        MemoryAccess::ExactRead
    );
}

#[test]
fn aborted_chat_message_round_trips_without_becoming_complete() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_guru(&store, &guru());
    let chat = ChatSession {
        id: "chat-aborted".into(),
        guru_id: "guru-1".into(),
        pi_session_id: "123e4567-e89b-42d3-a456-426614174099".into(),
        pi_session_cache: None,
        title: "Stopped response".into(),
        memory_policy: MemoryPolicy::default(),
        messages: vec![ChatMessage {
            id: "assistant-aborted".into(),
            role: ChatRole::Assistant,
            status: crate::domain::ChatMessageStatus::Aborted,
            content: "Response stopped.".into(),
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
        }],
        created_at_ms: 1,
        updated_at_ms: 2,
    };

    store.create_chat(&chat).unwrap();
    let loaded = store.get_chat(&chat.id).unwrap().unwrap();
    assert_eq!(loaded, chat);
    assert_eq!(
        loaded.messages[0].status,
        crate::domain::ChatMessageStatus::Aborted
    );
}

#[test]
fn assistant_progress_round_trips_with_the_chat_message() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_guru(&store, &guru());
    let progress = crate::chat_progress::ChatProgress {
        started_at_ms: 10,
        finished_at_ms: Some(20),
        items: vec![crate::chat_progress::ChatProgressItem::Tool {
            id: "tool-1".into(),
            category: crate::chat_progress::ChatProgressCategory::Memory,
            operation: crate::chat_progress::ChatProgressOperation::Read,
            action: "Read Memory".into(),
            target: Some("lens:rates".into()),
            href: None,
            status: crate::chat_progress::ChatProgressStatus::Succeeded,
            started_at_ms: 12,
            finished_at_ms: Some(18),
        }],
    };
    let chat = ChatSession {
        id: "chat-progress".into(),
        guru_id: "guru-1".into(),
        pi_session_id: "123e4567-e89b-42d3-a456-426614174088".into(),
        pi_session_cache: None,
        title: "Progress".into(),
        memory_policy: MemoryPolicy::default(),
        messages: vec![ChatMessage {
            id: "assistant-progress".into(),
            role: ChatRole::Assistant,
            status: crate::domain::ChatMessageStatus::Complete,
            content: "Done".into(),
            created_at_ms: 20,
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
            progress: Some(progress.clone()),
        }],
        created_at_ms: 1,
        updated_at_ms: 20,
    };

    store.create_chat(&chat).unwrap();
    let loaded = store.get_chat(&chat.id).unwrap().unwrap();
    assert_eq!(loaded.messages[0].progress, Some(progress));
}

#[test]
fn chat_artifact_replaces_current_content_and_deletes_with_its_session() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_guru(&store, &guru());
    let mut chat = ChatSession {
        id: "chat-artifacts".into(),
        guru_id: "guru-1".into(),
        pi_session_id: "123e4567-e89b-42d3-a456-426614174002".into(),
        pi_session_cache: None,
        title: "Artifact work".into(),
        memory_policy: MemoryPolicy::default(),
        messages: Vec::new(),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    store.create_chat(&chat).unwrap();

    let revision = ChatArtifactRevision::new(
        "artifact-1".into(),
        1,
        ChatArtifactPayload::Markdown {
            schema: "guruterminal-markdown/1".into(),
            markdown: "# First revision".into(),
        },
        "assistant-1".into(),
        2,
    )
    .unwrap();
    let mut artifact = ChatArtifact {
        id: "artifact-1".into(),
        chat_session_id: chat.id.clone(),
        kind: ChatArtifactKind::Markdown,
        title: "Research note".into(),
        current_revision: 1,
        created_at_ms: 2,
        updated_at_ms: 2,
    };
    let first_expected = chat.clone();
    chat.messages.push(ChatMessage {
        id: "assistant-1".into(),
        role: ChatRole::Assistant,
        status: crate::domain::ChatMessageStatus::Complete,
        content: "I created the note.".into(),
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
        artifact_refs: vec![revision.artifact_ref(artifact.title.clone())],
        progress: None,
    });
    chat.updated_at_ms = 2;
    store
        .save_chat_with_artifacts(
            &first_expected,
            &chat,
            &[ArtifactCommit {
                artifact: artifact.clone(),
                revision: revision.clone(),
                datasets: vec![],
            }],
        )
        .unwrap();

    artifact.current_revision = 2;
    artifact.updated_at_ms = 3;
    let second = ChatArtifactRevision::new(
        artifact.id.clone(),
        2,
        ChatArtifactPayload::Markdown {
            schema: "guruterminal-markdown/1".into(),
            markdown: "# Second revision".into(),
        },
        "assistant-2".into(),
        3,
    )
    .unwrap();
    let second_expected = chat.clone();
    chat.messages.push(ChatMessage {
        id: "assistant-2".into(),
        role: ChatRole::Assistant,
        status: crate::domain::ChatMessageStatus::Complete,
        content: "I revised the note.".into(),
        created_at_ms: 3,
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
        artifact_refs: vec![second.artifact_ref(artifact.title.clone())],
        progress: None,
    });
    chat.updated_at_ms = 3;
    store
        .save_chat_with_artifacts(
            &second_expected,
            &chat,
            &[ArtifactCommit {
                artifact: artifact.clone(),
                revision: second,
                datasets: vec![],
            }],
        )
        .unwrap();

    let loaded = store.get_chat(&chat.id).unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.messages[0].artifact_refs[0].revision, 1);
    assert_eq!(loaded.messages[1].artifact_refs[0].revision, 2);
    assert_eq!(
        store.get_chat_artifact(&artifact.id).unwrap(),
        Some(artifact)
    );
    let current = store
        .get_chat_artifact_current("artifact-1")
        .unwrap()
        .unwrap();
    assert_eq!(current.revision, 2);
    assert!(matches!(
        current.payload,
        ChatArtifactPayload::Markdown { ref markdown, .. }
            if markdown == "# Second revision"
    ));

    let mut wrong_guru = chat.clone();
    wrong_guru.guru_id = "another-guru".into();
    assert!(store.delete_chat(&wrong_guru).is_err());
    assert!(store.get_chat(&chat.id).unwrap().is_some());
    store.delete_chat(&chat).unwrap();
    assert_eq!(store.get_chat(&chat.id).unwrap(), None);
    assert_eq!(store.get_chat_artifact("artifact-1").unwrap(), None);
    assert_eq!(store.get_chat_artifact_current("artifact-1").unwrap(), None);
}

#[test]
fn one_assistant_message_can_persist_distinct_artifact_commits() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_guru(&store, &guru());
    let mut chat = ChatSession {
        id: "chat-multi-artifacts".into(),
        guru_id: "guru-1".into(),
        pi_session_id: "123e4567-e89b-42d3-a456-426614174003".into(),
        pi_session_cache: None,
        title: "Paired deliverables".into(),
        memory_policy: MemoryPolicy::default(),
        messages: Vec::new(),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    store.create_chat(&chat).unwrap();

    let first_revision = ChatArtifactRevision::new(
        "artifact-note".into(),
        1,
        ChatArtifactPayload::Markdown {
            schema: "guruterminal-markdown/1".into(),
            markdown: "# Note".into(),
        },
        "assistant-1".into(),
        2,
    )
    .unwrap();
    let second_revision = ChatArtifactRevision::new(
        "artifact-chart-note".into(),
        1,
        ChatArtifactPayload::Markdown {
            schema: "guruterminal-markdown/1".into(),
            markdown: "# Second note".into(),
        },
        "assistant-1".into(),
        3,
    )
    .unwrap();
    let first = ArtifactCommit {
        artifact: ChatArtifact {
            id: "artifact-note".into(),
            chat_session_id: chat.id.clone(),
            kind: ChatArtifactKind::Markdown,
            title: "Note".into(),
            current_revision: 1,
            created_at_ms: 2,
            updated_at_ms: 2,
        },
        revision: first_revision.clone(),
        datasets: vec![],
    };
    let second = ArtifactCommit {
        artifact: ChatArtifact {
            id: "artifact-chart-note".into(),
            chat_session_id: chat.id.clone(),
            kind: ChatArtifactKind::Markdown,
            title: "Second note".into(),
            current_revision: 1,
            created_at_ms: 3,
            updated_at_ms: 3,
        },
        revision: second_revision.clone(),
        datasets: vec![],
    };
    let expected = chat.clone();
    chat.messages.push(ChatMessage {
        id: "assistant-1".into(),
        role: ChatRole::Assistant,
        status: crate::domain::ChatMessageStatus::Complete,
        content: "Published both.".into(),
        created_at_ms: 3,
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
        artifact_refs: vec![
            first_revision.artifact_ref(first.artifact.title.clone()),
            second_revision.artifact_ref(second.artifact.title.clone()),
        ],
        progress: None,
    });
    chat.updated_at_ms = 3;
    store
        .save_chat_with_artifacts(&expected, &chat, &[first, second])
        .unwrap();

    let loaded = store.get_chat(&chat.id).unwrap().unwrap();
    assert_eq!(loaded.messages[0].artifact_refs.len(), 2);
    assert_eq!(
        store
            .get_chat_artifact_current("artifact-note")
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        store
            .get_chat_artifact_current("artifact-chart-note")
            .unwrap()
            .unwrap()
            .revision,
        1
    );
}
