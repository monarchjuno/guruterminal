use super::*;

#[test]
fn tool_executor_rejects_a_guru_session_scope_mismatch() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let guru_a = temporary.path().join("guru-a");
    let guru_b = temporary.path().join("guru-b");
    fs::create_dir_all(&guru_a).unwrap();
    fs::create_dir_all(&guru_b).unwrap();
    seed_profile(state.store.as_ref(), &profile("guru-a", &guru_a, 1));
    seed_profile(state.store.as_ref(), &profile("guru-b", &guru_b, 1));
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let guru_root = bound_root(&guru_a);
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state,
        capture: Arc::new(ToolCapture::default()),
        guru_id: "guru-a".into(),
        guru_root,
        chat_provider: String::new(),
    };
    let policy = ToolPolicy {
        guru_id: "guru-b".into(),
        session_id: "chat-a".into(),
        use_memory: true,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    assert!(matches!(
        executor.ensure_scope(&policy),
        Err(BrokerError::Execution(_))
    ));
}

#[tokio::test]
async fn artifact_tools_require_an_exact_read_and_keep_thread_scope() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let workspace = temporary.path().join("guru");
    fs::create_dir(&workspace).unwrap();
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    state
        .store
        .create_chat(&chat("chat-b", "guru-a", 1))
        .unwrap();
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: false,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    let capture = Arc::new(ToolCapture::for_chat("assistant-1".into()));
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state: state.clone(),
        capture: capture.clone(),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };
    executor
        .execute(
            &policy,
            ToolMethod::ArtifactPublish,
            json!({
                "mode": "create",
                "title": "Research note",
                "payload": {
                    "kind": "markdown",
                    "schema": "guruterminal-markdown/1",
                    "markdown": "# Initial note"
                }
            }),
        )
        .await
        .unwrap();
    let commit = capture.artifacts.lock().await.last().cloned().unwrap();
    let mut stored_chat = state.store.get_chat("chat-a").unwrap().unwrap();
    let expected_chat = stored_chat.clone();
    stored_chat.messages.push(ChatMessage {
        id: "assistant-1".into(),
        role: ChatRole::Assistant,
        status: crate::domain::ChatMessageStatus::Complete,
        content: "Created the note.".into(),
        created_at_ms: commit.artifact.updated_at_ms,
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
        artifact_refs: vec![commit.revision.artifact_ref(commit.artifact.title.clone())],
        progress: None,
    });
    stored_chat.updated_at_ms = commit.artifact.updated_at_ms;
    state
        .store
        .save_chat_with_artifacts(&expected_chat, &stored_chat, std::slice::from_ref(&commit))
        .unwrap();

    let revision_capture = Arc::new(ToolCapture::for_chat("assistant-2".into()));
    let revision_executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state: state.clone(),
        capture: revision_capture.clone(),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };
    let revise = json!({
        "mode": "revise",
        "artifact_id": commit.artifact.id,
        "expected_revision": 1,
        "title": "Research note",
        "payload": {
            "kind": "markdown",
            "schema": "guruterminal-markdown/1",
            "markdown": "# Revised note"
        }
    });
    assert!(matches!(
        revision_executor
            .execute(&policy, ToolMethod::ArtifactPublish, revise.clone())
            .await,
        Err(BrokerError::Execution(_))
    ));
    revision_executor
        .execute(
            &policy,
            ToolMethod::ArtifactRead,
            json!({ "artifact_id": commit.artifact.id }),
        )
        .await
        .unwrap();
    revision_executor
        .execute(&policy, ToolMethod::ArtifactPublish, revise)
        .await
        .unwrap();
    assert_eq!(
        revision_capture
            .artifacts
            .lock()
            .await
            .last()
            .unwrap()
            .revision
            .revision,
        2
    );

    let other_thread_policy = ToolPolicy {
        session_id: "chat-b".into(),
        ..policy
    };
    assert!(matches!(
        revision_executor
            .execute(
                &other_thread_policy,
                ToolMethod::ArtifactRead,
                json!({ "artifact_id": commit.artifact.id }),
            )
            .await,
        Err(BrokerError::MethodDenied)
    ));
}

#[tokio::test]
async fn a_chat_turn_can_publish_distinct_artifacts_but_rejects_duplicates_and_the_bound() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let workspace = temporary.path().join("guru");
    fs::create_dir(&workspace).unwrap();
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: false,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    let capture = Arc::new(ToolCapture::for_chat("assistant-1".into()));
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state: state.clone(),
        capture: capture.clone(),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };
    let publish = |title: &str| {
        json!({
            "mode": "create",
            "title": title,
            "payload": {
                "kind": "markdown",
                "schema": "guruterminal-markdown/1",
                "markdown": format!("# {title}")
            }
        })
    };
    for index in 1..=crate::chat_artifacts::MAX_CHAT_TURN_ARTIFACTS {
        executor
            .execute(
                &policy,
                ToolMethod::ArtifactPublish,
                publish(&format!("Note {index}")),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        capture.artifacts.lock().await.len(),
        crate::chat_artifacts::MAX_CHAT_TURN_ARTIFACTS
    );
    assert!(matches!(
        executor
            .execute(&policy, ToolMethod::ArtifactPublish, publish("Note extra"))
            .await,
        Err(BrokerError::Execution(_))
    ));

    let first = capture.artifacts.lock().await[0].clone();
    let mut stored_chat = state.store.get_chat("chat-a").unwrap().unwrap();
    let expected_chat = stored_chat.clone();
    stored_chat.messages.push(ChatMessage {
        id: "assistant-1".into(),
        role: ChatRole::Assistant,
        status: crate::domain::ChatMessageStatus::Complete,
        content: "Created the notes.".into(),
        created_at_ms: first.artifact.updated_at_ms,
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
        artifact_refs: vec![first.revision.artifact_ref(first.artifact.title.clone())],
        progress: None,
    });
    stored_chat.updated_at_ms = first.artifact.updated_at_ms;
    state
        .store
        .save_chat_with_artifacts(&expected_chat, &stored_chat, std::slice::from_ref(&first))
        .unwrap();

    let duplicate_capture = Arc::new(ToolCapture::for_chat("assistant-2".into()));
    let duplicate_executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state,
        capture: duplicate_capture.clone(),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };
    duplicate_executor
        .execute(
            &policy,
            ToolMethod::ArtifactRead,
            json!({ "artifact_id": first.artifact.id }),
        )
        .await
        .unwrap();
    let revise = json!({
        "mode": "revise",
        "artifact_id": first.artifact.id,
        "expected_revision": 1,
        "title": first.artifact.title,
        "payload": {
            "kind": "markdown",
            "schema": "guruterminal-markdown/1",
            "markdown": "# Revised"
        }
    });
    duplicate_executor
        .execute(&policy, ToolMethod::ArtifactPublish, revise.clone())
        .await
        .unwrap();
    assert!(matches!(
        duplicate_executor
            .execute(&policy, ToolMethod::ArtifactPublish, revise)
            .await,
        Err(BrokerError::Execution(_))
    ));
    assert_eq!(duplicate_capture.artifacts.lock().await.len(), 1);
}

async fn delivered_chart_result(capture: &ToolCapture, payload: Value) -> String {
    let delivery_id = new_id("test-result-delivery");
    let result_ref = capture
        .stage_run_result(
            &delivery_id,
            RunResultProducer {
                runtime_id: "openbb".into(),
                tool_name: "equity_price_historical".into(),
                provider: Some("openbb-keyless".into()),
            },
            &json!({ "symbol": "TEST" }),
            payload,
            vec![],
        )
        .await
        .unwrap();
    capture.commit_delivery(&delivery_id).await;
    result_ref
}

#[tokio::test]
async fn chart_publish_selects_a_delivered_result_and_persists_its_lineage() {
    let temporary = tempfile::tempdir().unwrap();
    let app_data_dir = temporary.path().join("app");
    let state = AppState::for_persistent_test(app_data_dir);
    let workspace = temporary.path().join("guru");
    fs::create_dir(&workspace).unwrap();
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: false,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    let capture = Arc::new(ToolCapture::for_chat("assistant-1".into()));
    let result_ref = delivered_chart_result(
        &capture,
        json!({
            "data": {
                "rows": [
                    { "date": "2026-08-01", "close": 100 },
                    { "date": "2026-08-02", "close": 101 },
                    { "date": "2026-08-03", "close": 102 }
                ]
            },
            "warnings": ["Delayed market data"]
        }),
    )
    .await;
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state: state.clone(),
        capture: capture.clone(),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };

    executor
        .execute(
            &policy,
            ToolMethod::ChartPublish,
            json!({
                "mode": "create",
                "title": "Close price",
                "dataset": {
                    "from_result": {
                        "result_ref": result_ref,
                        "rows_pointer": "/data/rows",
                        "columns": [
                            { "id": "date", "label": "Date", "kind": "date", "pointer": "/date" },
                            { "id": "close", "label": "Close", "kind": "number", "pointer": "/close" }
                        ]
                    }
                },
                "view": {
                    "kind": "analytic",
                    "chart_type": "line",
                    "x": "date",
                    "y": ["close"]
                }
            }),
        )
        .await
        .unwrap();
    let commit = capture.artifacts.lock().await.last().cloned().unwrap();
    assert_eq!(commit.datasets[0].rows.len(), 3);
    assert_eq!(commit.datasets[0].columns[1].label, "Close");
    let crate::chart_engine::ChartDatasetLineage::FromResult {
        receipt,
        rows_pointer,
        columns,
    } = &commit.datasets[0].lineage
    else {
        panic!("result-selected chart did not persist result lineage");
    };
    assert_eq!(receipt.result_ref, result_ref);
    assert_eq!(receipt.runtime_id, "openbb");
    assert_eq!(receipt.provider.as_deref(), Some("openbb-keyless"));
    assert_eq!(receipt.warnings, ["Delayed market data"]);
    assert_eq!(rows_pointer, "/data/rows");
    assert_eq!(columns[1].pointer, "/close");

    let mut stored_chat = state.store.get_chat("chat-a").unwrap().unwrap();
    let expected_chat = stored_chat.clone();
    stored_chat.messages.push(ChatMessage {
        id: "assistant-1".into(),
        role: ChatRole::Assistant,
        status: crate::domain::ChatMessageStatus::Complete,
        content: "Created the chart.".into(),
        created_at_ms: commit.artifact.updated_at_ms,
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
        artifact_refs: vec![commit.revision.artifact_ref(commit.artifact.title.clone())],
        progress: None,
    });
    stored_chat.updated_at_ms = commit.artifact.updated_at_ms;
    state
        .store
        .save_chat_with_artifacts(&expected_chat, &stored_chat, std::slice::from_ref(&commit))
        .unwrap();

    let next_capture = Arc::new(ToolCapture::for_chat("assistant-2".into()));
    let next_executor = AppToolExecutor {
        capture: next_capture.clone(),
        ..executor
    };
    let compact = next_executor
        .execute(
            &policy,
            ToolMethod::ArtifactRead,
            json!({ "artifact_id": commit.artifact.id }),
        )
        .await
        .unwrap();
    assert_eq!(compact["dataset"]["row_count"], 3);
    assert_eq!(compact["dataset"]["lineage"]["kind"], "from_result");
    assert_eq!(
        compact["dataset"]["lineage"]["receipt"]["response_digest"]
            .as_str()
            .unwrap()
            .len(),
        64
    );

    let page = next_executor
        .execute(
            &policy,
            ToolMethod::ChartQuery,
            json!({ "artifact_id": commit.artifact.id, "revision": 1, "limit": 2 }),
        )
        .await
        .unwrap();
    assert_eq!(page["rows"].as_array().unwrap().len(), 2);
    assert_eq!(page["rows"][0][1], 100);
    assert_eq!(page["next_offset"], 2);

    next_executor
        .execute(
            &policy,
            ToolMethod::ChartPublish,
            json!({
                "mode": "revise",
                "artifact_id": commit.artifact.id,
                "edit_token": compact["artifact"]["edit_token"],
                "title": "Close price with note",
                "note": "Same immutable dataset."
            }),
        )
        .await
        .unwrap();
    let revised = next_capture.artifacts.lock().await.last().cloned().unwrap();
    assert_eq!(revised.datasets[0].id, commit.datasets[0].id);
    assert_eq!(revised.datasets[0].digest, commit.datasets[0].digest);

    assert!(matches!(
        next_executor
            .execute(
                &policy,
                ToolMethod::ChartPublish,
                json!({
                    "mode": "create",
                    "title": "Cross-turn result",
                    "dataset": {
                        "from_result": {
                            "result_ref": result_ref,
                            "rows_pointer": "/data/rows",
                            "columns": [
                                { "id": "date", "label": "Date", "kind": "date", "pointer": "/date" },
                                { "id": "close", "label": "Close", "kind": "number", "pointer": "/close" }
                            ]
                        }
                    },
                    "view": {
                        "kind": "analytic",
                        "chart_type": "line",
                        "x": "date",
                        "y": ["close"]
                    }
                }),
            )
            .await,
        Err(BrokerError::Execution(_))
    ));
}

#[tokio::test]
async fn chart_publish_accepts_inline_data_and_validates_upstream_results() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let workspace = temporary.path().join("guru");
    fs::create_dir(&workspace).unwrap();
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: false,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    let capture = Arc::new(ToolCapture::for_chat("assistant-1".into()));
    let upstream = delivered_chart_result(&capture, json!({ "value": 100 })).await;
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state: state.clone(),
        capture: capture.clone(),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };

    executor
        .execute(
            &policy,
            ToolMethod::ChartPublish,
            json!({
                "mode": "create",
                "title": "Derived returns",
                "dataset": {
                    "inline": {
                        "columns": [
                            { "id": "date", "label": "Date", "kind": "date" },
                            { "id": "return", "label": "Return", "kind": "number" }
                        ],
                        "rows": [["2026-08-01", 0.1], ["2026-08-02", 0.2]],
                        "upstream_result_refs": [upstream]
                    }
                },
                "view": {
                    "kind": "analytic",
                    "chart_type": "line",
                    "x": "date",
                    "y": ["return"]
                }
            }),
        )
        .await
        .unwrap();
    let commit = capture.artifacts.lock().await.last().cloned().unwrap();
    let crate::chart_engine::ChartDatasetLineage::AgentAuthored { upstream_receipts } =
        &commit.datasets[0].lineage
    else {
        panic!("inline chart was not marked agent-authored");
    };
    assert_eq!(upstream_receipts.len(), 1);
    assert_eq!(upstream_receipts[0].result_ref, upstream);

    let mut stored_chat = state.store.get_chat("chat-a").unwrap().unwrap();
    let expected_chat = stored_chat.clone();
    stored_chat.messages.push(ChatMessage {
        id: "assistant-1".into(),
        role: ChatRole::Assistant,
        status: crate::domain::ChatMessageStatus::Complete,
        content: "Created a derived chart.".into(),
        created_at_ms: commit.artifact.updated_at_ms,
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
        artifact_refs: vec![commit.revision.artifact_ref(commit.artifact.title.clone())],
        progress: None,
    });
    stored_chat.updated_at_ms = commit.artifact.updated_at_ms;
    state
        .store
        .save_chat_with_artifacts(&expected_chat, &stored_chat, std::slice::from_ref(&commit))
        .unwrap();
    let persisted = state
        .store
        .get_chart_dataset(&commit.datasets[0].id)
        .unwrap()
        .unwrap();
    assert_eq!(persisted.rows, commit.datasets[0].rows);
    assert_eq!(persisted.lineage, commit.datasets[0].lineage);

    assert!(matches!(
        executor
            .execute(
                &policy,
                ToolMethod::ChartPublish,
                json!({
                    "mode": "create",
                    "title": "Unknown upstream",
                    "dataset": {
                        "inline": {
                            "columns": [
                                { "id": "date", "label": "Date", "kind": "date" },
                                { "id": "value", "label": "Value", "kind": "number" }
                            ],
                            "rows": [["2026-08-01", 1]],
                            "upstream_result_refs": ["result:not-from-this-turn"]
                        }
                    },
                    "view": {
                        "kind": "analytic",
                        "chart_type": "line",
                        "x": "date",
                        "y": ["value"]
                    }
                }),
            )
            .await,
        Err(BrokerError::Execution(_))
    ));

    for (title, rows) in [
        ("Invalid typed cell", json!([["not-a-number"]])),
        (
            "Too many rows",
            Value::Array((0..10_001).map(|index| json!([index])).collect()),
        ),
    ] {
        assert!(matches!(
            executor
                .execute(
                    &policy,
                    ToolMethod::ChartPublish,
                    json!({
                        "mode": "create",
                        "title": title,
                        "dataset": {
                            "inline": {
                                "columns": [
                                    { "id": "value", "label": "Value", "kind": "number" }
                                ],
                                "rows": rows
                            }
                        },
                        "view": {
                            "kind": "analytic",
                            "chart_type": "bar",
                            "x": "value",
                            "y": ["value"]
                        }
                    }),
                )
                .await,
            Err(BrokerError::Execution(_))
        ));
    }
}

#[tokio::test]
async fn aborted_turn_capture_does_not_persist_evidence_decision_or_artifact() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let workspace = temporary.path().join("guru");
    fs::create_dir_all(workspace.join("guruterminal/evidence")).unwrap();
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: false,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    let capture = Arc::new(ToolCapture::for_chat("assistant-aborted".into()));
    let result_ref = delivered_chart_result(&capture, json!({"value": 42})).await;
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state: state.clone(),
        capture: capture.clone(),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };
    let evidence = executor
        .execute(
            &policy,
            ToolMethod::EvidenceCreate,
            json!({
                "title": "Aborted evidence",
                "summary": "This must remain staged only.",
                "as_of": "2026-08-24",
                "claims": [{
                    "text": "The value is 42.",
                    "citations": [{"result_ref": result_ref, "pointer": "/value"}]
                }]
            }),
        )
        .await
        .unwrap();
    executor
        .execute(
            &policy,
            ToolMethod::DecisionSubmit,
            decision(
                "neutral",
                vec![evidence["evidence_id"].as_str().unwrap().to_owned()],
            ),
        )
        .await
        .unwrap();
    executor
        .execute(
            &policy,
            ToolMethod::ArtifactPublish,
            json!({
                "mode": "create",
                "title": "Aborted artifact",
                "payload": {
                    "kind": "markdown",
                    "schema": "guruterminal-markdown/1",
                    "markdown": "# Staged only"
                }
            }),
        )
        .await
        .unwrap();
    assert_eq!(capture.staged_evidence.lock().await.len(), 1);
    assert!(capture.decision.lock().await.is_some());
    assert_eq!(capture.artifacts.lock().await.len(), 1);

    drop(executor);
    drop(capture);
    assert!(state
        .store
        .list_chat_artifacts("chat-a")
        .unwrap()
        .is_empty());
    let chat = state.store.get_chat("chat-a").unwrap().unwrap();
    assert!(chat.messages.is_empty());
    assert_eq!(
        fs::read_dir(workspace.join("guruterminal/evidence"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn chart_query_window_is_bounded_by_serialized_bytes() {
    let cell = "x".repeat(8 * 1024);
    let rows = (0..200)
        .map(|_| vec![Value::String(cell.clone()); 8])
        .collect::<Vec<_>>();
    let end = bounded_chart_query_end(&rows, 0, rows.len()).unwrap();

    assert!(end > 0);
    assert!(end < rows.len());
    assert!(serde_json::to_vec(&rows[..end]).unwrap().len() <= MAX_CHART_QUERY_BYTES);
    assert!(serde_json::to_vec(&rows[..=end]).unwrap().len() > MAX_CHART_QUERY_BYTES);
}

#[tokio::test]
async fn execute_capabilities_are_captured_for_the_run() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    fs::create_dir(&workspace).unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    let captured = captured_capabilities(&state, "guru-a");
    assert!(captured.contains("community.web-research"));
    let mut binding = state
        .store
        .get_guru_capability("guru-a", "community.web-research")
        .unwrap()
        .unwrap();
    binding.enabled = false;
    binding.granted_permissions.clear();
    binding.updated_at_ms += 1;
    state.store.save_guru_capability(&binding).unwrap();
    let pinned_executor = AppToolExecutor {
        capability_ids: captured,
        state: state.clone(),
        capture: Arc::new(ToolCapture::default()),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };
    assert!(pinned_executor.capability_enabled("community.web-research"));

    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state,
        capture: Arc::new(ToolCapture::default()),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };
    assert!(!executor.capability_enabled("community.web-research"));

    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: false,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    for obsolete in [
        json!({"query": "market news", "provider": "model"}),
        json!({"query": "market news", "_native_result": {}}),
    ] {
        assert!(matches!(
            pinned_executor
                .web_search(&policy, obsolete, "test-delivery")
                .await,
            Err(BrokerError::Malformed)
        ));
    }
    assert!(matches!(
        pinned_executor
            .web_fetch(
                &policy,
                json!({
                    "source_id": "web:deadbeef",
                    "url": "https://example.com/report"
                }),
                "test-delivery"
            )
            .await,
        Err(BrokerError::Malformed)
    ));
    assert!(matches!(
        pinned_executor
            .web_fetch(
                &policy,
                json!({
                    "url": "https://example.com/report",
                    "offset": 2 * 1024 * 1024
                }),
                "test-delivery"
            )
            .await,
        Err(BrokerError::Malformed)
    ));
    assert!(matches!(
        pinned_executor
            .web_fetch(
                &policy,
                json!({"url": "http://127.0.0.1/secret"}),
                "test-delivery"
            )
            .await,
        Err(BrokerError::Execution(_))
    ));
    assert!(matches!(
        executor
            .web_search(&policy, json!({"query": "market news"}), "test-delivery")
            .await,
        Err(BrokerError::MethodDenied)
    ));
    assert!(matches!(
        executor
            .web_fetch(
                &policy,
                json!({"source_id": "web:deadbeef"}),
                "test-delivery"
            )
            .await,
        Err(BrokerError::MethodDenied)
    ));
    assert!(matches!(
        executor
            .web_fetch(
                &policy,
                json!({
                    "source_id": "web:deadbeef",
                    "url": "https://example.com/report"
                }),
                "test-delivery"
            )
            .await,
        Err(BrokerError::MethodDenied | BrokerError::Malformed)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn tool_executor_keeps_the_pinned_guru_after_its_path_is_replaced() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let root_a = temporary.path().join("guru-a");
    let root_b = temporary.path().join("guru-b");
    let moved_a = temporary.path().join("guru-a-original");
    initialized_workspace(&root_a, "A");
    initialized_workspace(&root_b, "B");

    let runtime_path = temporary.path().join("guruterminal-read-fixture");
    fs::write(
        &runtime_path,
        "#!/bin/sh\n\
         if [ \"$#\" -ne 6 ] || [ \"$1\" != knowledge ] || [ \"$2\" != read ] || \
            [ \"$4\" != --workspace ] || [ \"$5\" != . ] || [ \"$6\" != --json ]; then\n\
           printf 'unexpected arguments\\n' >&2\n\
           exit 64\n\
         fi\n\
         IFS= read -r marker < runtime-marker\n\
         printf '{\"document\":{\"id\":\"lens:%s\",\"kind\":\"lens\",\"title\":\"Marker %s\",\"summary\":\"Pinned %s\",\"path\":\"guruterminal/lens/%s.md\"},\"content\":\"%s\"}\\n' \"$marker\" \"$marker\" \"$marker\" \"$marker\" \"$marker\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime_path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&runtime_path, permissions).unwrap();

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &root_a, 1);
    seed_profile(state.store.as_ref(), &guru);
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let capture = Arc::new(ToolCapture::default());
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state,
        capture: capture.clone(),
        guru_id: "guru-a".into(),
        guru_root: profile_workspace(&guru).unwrap(),
        chat_provider: String::new(),
    };

    fs::rename(&root_a, &moved_a).unwrap();
    fs::rename(&root_b, &root_a).unwrap();

    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: true,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    let result = executor
        .execute(
            &policy,
            ToolMethod::GuruRead,
            json!({"id": "lens:requested"}),
        )
        .await
        .unwrap();

    assert_eq!(result["content"], "A");
    assert_eq!(
        capture.memories.lock().await["lens:A"].access,
        MemoryAccess::ExactRead
    );
    assert_eq!(
        fs::read_to_string(root_a.join("runtime-marker")).unwrap(),
        "B\n"
    );
    assert_eq!(
        fs::read_to_string(moved_a.join("runtime-marker")).unwrap(),
        "A\n"
    );
}

#[tokio::test]
async fn memory_off_is_denied_before_the_executor_runs() {
    let temporary = tempfile::tempdir().unwrap();
    let socket = tool_broker_endpoint(temporary.path().join("broker.sock"));
    let calls = Arc::new(AtomicUsize::new(0));
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: false,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    let broker = start_tool_broker(
        socket.clone(),
        policy,
        Arc::new(CountingExecutor {
            calls: calls.clone(),
        }),
    )
    .await
    .unwrap();
    let response = broker_request(
        &socket,
        broker.token(),
        "guru.search",
        json!({"query": "quality"}),
    )
    .await;
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "memory_disabled");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    broker.shutdown().await.unwrap();
}

#[tokio::test]
async fn provenance_distinguishes_search_discovery_from_an_exact_read() {
    let temporary = tempfile::tempdir().unwrap();
    let capture = Arc::new(ToolCapture::default());
    let workspace = temporary.path().join("guru");
    fs::create_dir(&workspace).unwrap();
    let executor = AppToolExecutor {
        state: AppState::for_test(temporary.path().join("app")),
        capture: capture.clone(),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        capability_ids: BTreeSet::new(),
        chat_provider: String::new(),
    };
    let discovered = MemoryRefSnapshot {
        record_id: "lens:quality".into(),
        kind: "Lens".into(),
        title: "Quality".into(),
        excerpt: "Search hit".into(),
        as_of: Some("2026-08-01T00:00:00Z".into()),
        section: None,
        access: MemoryAccess::SearchDiscovered,
        full_record_digest: None,
    };
    executor.capture_memory(discovered.clone()).await.unwrap();
    assert_eq!(
        capture.memories.lock().await["lens:quality"].access,
        MemoryAccess::SearchDiscovered
    );

    let mut exact = discovered.clone();
    exact.excerpt = "Exact record body".into();
    exact.section = Some("Boundary".into());
    exact.access = MemoryAccess::ExactRead;
    executor.capture_memory(exact.clone()).await.unwrap();
    executor.capture_memory(discovered).await.unwrap();
    assert_eq!(capture.memories.lock().await["lens:quality"], exact);
}
