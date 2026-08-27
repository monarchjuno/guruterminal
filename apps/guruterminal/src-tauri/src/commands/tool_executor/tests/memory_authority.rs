use super::*;

async fn delivered_result(executor: &AppToolExecutor, payload: Value) -> String {
    let delivery_id = new_id("test-result");
    let result_ref = executor
        .capture
        .stage_run_result(
            &delivery_id,
            RunResultProducer {
                runtime_id: "test-runtime".into(),
                tool_name: "test_read".into(),
                provider: Some("test.provider".into()),
            },
            &json!({"query": "test"}),
            payload,
            Vec::new(),
        )
        .await
        .unwrap();
    executor.capture.commit_delivery(&delivery_id).await;
    result_ref
}

#[tokio::test]
async fn run_result_becomes_visible_only_after_delivery_commit() {
    let capture = ToolCapture::default();
    for delivery_id in ["failed-delivery", "cancelled-delivery"] {
        let result_ref = capture
            .stage_run_result(
                delivery_id,
                RunResultProducer {
                    runtime_id: "test".into(),
                    tool_name: "read".into(),
                    provider: None,
                },
                &json!({"input": 1}),
                json!({"output": 2}),
                Vec::new(),
            )
            .await
            .unwrap();
        assert!(capture.run_result(&result_ref).await.is_none());
        capture.discard_delivery(delivery_id).await;
        assert!(capture.run_result(&result_ref).await.is_none());
    }

    let result_ref = capture
        .stage_run_result(
            "delivery-two",
            RunResultProducer {
                runtime_id: "test".into(),
                tool_name: "read".into(),
                provider: None,
            },
            &json!({"input": 1}),
            json!({"output": 2}),
            Vec::new(),
        )
        .await
        .unwrap();
    capture.commit_delivery("delivery-two").await;
    assert!(capture.run_result(&result_ref).await.is_some());
}

#[tokio::test]
async fn evidence_rejects_result_refs_from_another_guru_capture() {
    let first_temporary = tempfile::tempdir().unwrap();
    let first = bare_executor(&first_temporary);
    let result_ref = delivered_result(&first, json!({"value": 42})).await;

    let second_temporary = tempfile::tempdir().unwrap();
    let mut second = bare_executor(&second_temporary);
    second.guru_id = "guru-b".into();
    let result = second
        .create_evidence(evidence(
            "Cross-Guru result",
            &result_ref,
            "The value is 42.",
        ))
        .await;
    assert!(
        matches!(result, Err(BrokerError::Execution(message)) if message.contains("delivered result"))
    );
}

#[tokio::test]
async fn decision_allows_empty_evidence_only_for_abstention() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    executor
        .seal_decision(decision("abstain", Vec::new()))
        .await
        .unwrap();
    assert!(matches!(
        executor
            .seal_decision(decision("neutral", Vec::new()))
            .await,
        Err(BrokerError::Malformed)
    ));
}

#[tokio::test]
async fn decision_accepts_staged_evidence_and_rejects_raw_result_refs() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    let result_ref = delivered_result(&executor, json!({"value": 42})).await;
    let raw = executor
        .seal_decision(decision("neutral", vec![result_ref.clone()]))
        .await;
    assert!(
        matches!(raw, Err(BrokerError::Execution(message)) if message.contains("created in this turn"))
    );

    let created = executor
        .create_evidence(evidence("Exact value", &result_ref, "The value is 42."))
        .await
        .unwrap();
    executor
        .seal_decision(decision(
            "neutral",
            vec![created["evidence_id"].as_str().unwrap().to_owned()],
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn evidence_create_keeps_receipts_without_result_payloads() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    let mut payload = serde_json::Map::new();
    payload.insert("bulk".into(), Value::String("x".repeat(2 * 1024 * 1024)));
    payload.insert("value".into(), json!(42));
    let result_ref = delivered_result(&executor, Value::Object(payload)).await;

    executor
        .create_evidence(evidence(
            "Large result",
            &result_ref,
            "The cited result supports a utilization of 42.",
        ))
        .await
        .unwrap();

    let staged = executor.capture.staged_evidence.lock().await;
    assert_eq!(staged[0].citations.len(), 1);
    assert_eq!(staged[0].citations[0].receipt.response_digest.len(), 64);
    assert_eq!(
        staged[0].markdown,
        "The cited result supports a utilization of 42."
    );
    assert!(!staged[0].markdown.contains("xxxx"));
}

#[tokio::test]
async fn decision_rejects_prior_evidence_even_when_exact_read() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    executor
        .capture_memory(MemoryRefSnapshot {
            record_id: "evidence:prior".into(),
            kind: "Evidence".into(),
            title: "Prior evidence".into(),
            excerpt: "Exact read in this turn".into(),
            as_of: Some("2026-08-23T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    let result = executor
        .seal_decision(decision("neutral", vec!["evidence:prior".into()]))
        .await;
    assert!(
        matches!(result, Err(BrokerError::Execution(message)) if message.contains("created in this turn"))
    );
}

#[tokio::test]
async fn chat_update_accepts_staged_evidence_not_raw_results() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    let result_ref = delivered_result(&executor, json!({"claim": "Exact source"})).await;
    let proposal = json!({
        "kind": "lens",
        "target_id": "lens:test",
        "proposed_markdown": lens_markdown("lens:test", "Test lens"),
        "rationale": "external research",
        "source_ids": [result_ref.clone()]
    });
    assert!(matches!(
        executor.capture_proposal(proposal.clone()).await,
        Err(BrokerError::Execution(_))
    ));
    let created = executor
        .create_evidence(evidence("Exact claim", &result_ref, "The claim is exact."))
        .await
        .unwrap();
    let evidence_id = created["evidence_id"].as_str().unwrap().to_owned();
    let mut proposal = proposal;
    proposal["source_ids"] = json!([evidence_id.clone()]);
    executor.capture_proposal(proposal).await.unwrap();
    assert_eq!(
        executor
            .capture
            .proposal
            .lock()
            .await
            .first()
            .unwrap()
            .source_memory_ids,
        vec![evidence_id]
    );
}

#[tokio::test]
async fn chat_proposal_requires_an_explicit_bounded_exact_read_subset() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    for (record_id, access) in [
        ("lens:exact-source", MemoryAccess::ExactRead),
        ("lens:search-source", MemoryAccess::SearchDiscovered),
    ] {
        executor
            .capture_memory(MemoryRefSnapshot {
                record_id: record_id.into(),
                kind: "Lens".into(),
                title: record_id.into(),
                excerpt: "Current-run source".into(),
                as_of: None,
                section: None,
                access,
                full_record_digest: (access == MemoryAccess::ExactRead).then(|| "a".repeat(64)),
            })
            .await
            .unwrap();
    }
    let proposal = |source_ids: Value| {
        json!({
            "kind": "lens",
            "target_id": "lens:bounded-proposal",
            "proposed_markdown": lens_markdown("lens:bounded-proposal", "Bounded proposal"),
            "rationale": "Use only explicitly selected exact reads.",
            "source_ids": source_ids,
        })
    };
    for invalid in [json!(["lens:search-source"]), json!(["lens:not-read"])] {
        assert!(matches!(
            executor.capture_proposal(proposal(invalid)).await,
            Err(BrokerError::Execution(_))
        ));
    }
    assert!(matches!(
        executor
            .capture_proposal(proposal(json!(["lens:exact-source", "lens:exact-source"])))
            .await,
        Err(BrokerError::Malformed)
    ));
    assert!(matches!(
        executor
            .capture_proposal(proposal(json!((0..33)
                .map(|index| format!("lens:source-{index}"))
                .collect::<Vec<_>>())))
            .await,
        Err(BrokerError::Malformed)
    ));
    executor
        .capture_proposal(proposal(json!(["lens:exact-source"])))
        .await
        .unwrap();
    assert_eq!(
        executor
            .capture
            .proposal
            .lock()
            .await
            .first()
            .unwrap()
            .source_memory_ids,
        vec!["lens:exact-source"]
    );
}

#[tokio::test]
async fn proposal_tool_captures_a_candidate_without_writing_memory() {
    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let workspace = temporary.path().join("guru");
    let lens_dir = workspace.join("guruterminal/lens");
    fs::create_dir_all(&lens_dir).unwrap();
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let capture = Arc::new(ToolCapture::default());
    let guru_root = bound_root(&workspace);
    let disabled_socket = tool_broker_endpoint(temporary.path().join("proposal-disabled.sock"));
    let disabled_broker = start_tool_broker(
        disabled_socket.clone(),
        ToolPolicy {
            guru_id: "guru-a".into(),
            session_id: "chat-a".into(),
            use_memory: false,
            propose_memory_updates: false,
            memory_proposal_budget: 0,
            as_of: None,
        },
        Arc::new(AppToolExecutor {
            capability_ids: captured_capabilities(&state, "guru-a"),
            state: state.clone(),
            capture: capture.clone(),
            guru_id: "guru-a".into(),
            guru_root: guru_root.clone(),
            chat_provider: String::new(),
        }),
    )
    .await
    .unwrap();
    let denied = broker_request(
        &disabled_socket,
        disabled_broker.token(),
        "memory.patch.propose",
        json!({
            "kind": "lens",
            "target_id": "lens:test",
            "proposed_markdown": "must not be captured",
            "rationale": "disabled",
            "source_ids": []
        }),
    )
    .await;
    assert_eq!(denied["error"]["code"], "proposal_disabled");
    assert!(capture.proposal.lock().await.is_empty());
    disabled_broker.shutdown().await.unwrap();

    let socket = tool_broker_endpoint(temporary.path().join("proposal.sock"));
    capture.memories.lock().await.insert(
        "lens:source".into(),
        MemoryRefSnapshot {
            record_id: "lens:source".into(),
            kind: "Lens".into(),
            title: "Source".into(),
            excerpt: "Exact source".into(),
            as_of: Some("2026-08-12T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        },
    );
    let broker = start_tool_broker(
        socket.clone(),
        ToolPolicy {
            guru_id: "guru-a".into(),
            session_id: "chat-a".into(),
            use_memory: false,
            propose_memory_updates: true,
            memory_proposal_budget: 1,
            as_of: None,
        },
        Arc::new(AppToolExecutor {
            capability_ids: captured_capabilities(&state, "guru-a"),
            state,
            capture: capture.clone(),
            guru_id: "guru-a".into(),
            guru_root,
            chat_provider: String::new(),
        }),
    )
    .await
    .unwrap();
    let response = broker_request(
        &socket,
        broker.token(),
        "memory.patch.propose",
        json!({
            "kind": "lens",
            "target_id": "lens:test",
            "proposed_markdown": lens_markdown("lens:test", "Test lens"),
            "rationale": "test rationale",
            "source_ids": ["lens:source"]
        }),
    )
    .await;
    assert_eq!(response["ok"], true);
    assert_eq!(response["result"]["status"], "accepted_for_atomic_apply");
    assert_eq!(
        response["result"]["host_application"],
        "automatic_after_turn"
    );
    let proposal = capture
        .proposal
        .lock()
        .await
        .first()
        .cloned()
        .expect("proposal");
    assert!(proposal.proposed_markdown.contains("id: lens:test"));
    proposal.validate().unwrap();
    assert_eq!(fs::read_dir(lens_dir).unwrap().count(), 0);
    broker.shutdown().await.unwrap();
}

#[tokio::test]
async fn chat_turn_accepts_a_bounded_set_of_memory_proposals() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    executor
        .capture_memory(MemoryRefSnapshot {
            record_id: "lens:exact-source".into(),
            kind: "Lens".into(),
            title: "Source".into(),
            excerpt: "Exact source".into(),
            as_of: Some("2026-08-12T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    let proposal = |target_id: &str, body: &str| {
        json!({
            "kind": "lens",
            "target_id": target_id,
            "proposed_markdown": lens_markdown(target_id, body),
            "rationale": body,
            "source_ids": ["lens:exact-source"],
        })
    };
    executor
        .capture_proposal(proposal("lens:first", "First"))
        .await
        .unwrap();
    executor
        .capture_proposal(proposal("lens:second", "Second"))
        .await
        .unwrap();
    executor
        .capture_proposal(proposal("lens:first", "Replaced"))
        .await
        .unwrap();
    {
        let proposals = executor.capture.proposal.lock().await;
        assert_eq!(proposals.len(), 2);
        assert_eq!(proposals[0].target_record_id, "lens:first");
        assert!(proposals[0].proposed_markdown.contains("Replaced"));
        assert_eq!(proposals[1].target_record_id, "lens:second");
    }
    for index in 3..=8 {
        let title = format!("Extra {index}");
        executor
            .capture_proposal(proposal(&format!("lens:extra-{index}"), &title))
            .await
            .unwrap();
    }
    assert_eq!(executor.capture.proposal.lock().await.len(), 8);
    assert!(matches!(
        executor
            .capture_proposal(proposal("lens:ninth", "Ninth"))
            .await,
        Err(BrokerError::Malformed)
    ));
    assert_eq!(executor.capture.proposal.lock().await.len(), 8);
}

#[tokio::test]
async fn same_turn_proposals_cannot_create_duplicate_titles_or_aliases() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    executor
        .capture_memory(MemoryRefSnapshot {
            record_id: "lens:exact-source".into(),
            kind: "Lens".into(),
            title: "Source".into(),
            excerpt: "Exact source".into(),
            as_of: Some("2026-08-12T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    executor
        .capture_proposal(json!({
            "kind": "lens",
            "target_id": "lens:pricing-quality",
            "proposed_markdown": lens_markdown("lens:pricing-quality", "Pricing quality"),
            "rationale": "Keep the reusable pricing test.",
            "source_ids": ["lens:exact-source"],
        }))
        .await
        .unwrap();

    let duplicate = executor
        .capture_proposal(json!({
            "kind": "lens",
            "target_id": "lens:margin-quality",
            "proposed_markdown": lens_markdown("lens:margin-quality", "Pricing quality"),
            "rationale": "This should revise the first proposal instead.",
            "source_ids": ["lens:exact-source"],
        }))
        .await;
    assert!(
        matches!(duplicate, Err(BrokerError::Execution(ref message)) if message.contains("lens:pricing-quality") && message.contains("duplicate")),
        "{duplicate:?}"
    );
    assert_eq!(executor.capture.proposal.lock().await.len(), 1);
}

#[tokio::test]
async fn run_results_list_exposes_receipts_without_payloads() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    let result_ref = delivered_result(&executor, json!({"secret_payload": [1, 2, 3]})).await;
    let listed = executor.run_results_list(json!({})).await.unwrap();
    let results = listed["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["result_ref"], result_ref);
    assert_eq!(results[0]["tool_name"], "test_read");
    assert!(results[0].get("payload").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn guru_memory_has_no_aggregate_read_quota_and_rejects_records_after_as_of_cutoff() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "cutoff");
    let runtime_path = temporary.path().join("guruterminal-as-of-fixture");
    fs::write(
        &runtime_path,
        "#!/bin/sh\n\
         if [ \"$1\" = knowledge ] && [ \"$2\" = search ]; then\n\
           case \" $* \" in\n\
             *\" --as-of 2026-06-01 \"*)\n\
               printf '%s\\n' '[{\"id\":\"wiki:past\",\"kind\":\"wiki\",\"title\":\"Past\",\"summary\":\"Old prior\",\"as_of\":\"2026-01-15T00:00:00Z\"}]'\n\
               ;;\n\
             *)\n\
               printf '%s\\n' '[{\"id\":\"wiki:future\",\"kind\":\"wiki\",\"title\":\"Future\",\"summary\":\"Later prior\",\"as_of\":\"2026-08-15T00:00:00Z\"}]'\n\
               ;;\n\
           esac\n\
           exit 0\n\
         fi\n\
         if [ \"$1\" = knowledge ] && [ \"$2\" = read ]; then\n\
           case \"$3\" in\n\
             wiki:past)\n\
               printf '%s\\n' '{\"document\":{\"id\":\"wiki:past\",\"kind\":\"wiki\",\"title\":\"Past\",\"summary\":\"Old prior\",\"as_of\":\"2026-01-15T00:00:00Z\",\"path\":\"guruterminal/wiki/past.md\"},\"content\":\"old\"}'\n\
               ;;\n\
             wiki:future)\n\
               printf '%s\\n' '{\"document\":{\"id\":\"wiki:future\",\"kind\":\"wiki\",\"title\":\"Future\",\"summary\":\"Later prior\",\"as_of\":\"2026-08-15T00:00:00Z\",\"path\":\"guruterminal/wiki/future.md\"},\"content\":\"new\"}'\n\
               ;;\n\
             *)\n\
               printf 'unknown id\\n' >&2\n\
               exit 64\n\
               ;;\n\
           esac\n\
           exit 0\n\
         fi\n\
         printf 'unexpected arguments\\n' >&2\n\
         exit 64\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime_path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&runtime_path, permissions).unwrap();

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state,
        capture: Arc::new(ToolCapture::default()),
        guru_id: "guru-a".into(),
        guru_root: profile_workspace(&guru).unwrap(),
        chat_provider: String::new(),
    };
    for index in 0..8 {
        executor.capture.memories.lock().await.insert(
            format!("wiki:prior-{index}"),
            MemoryRefSnapshot {
                record_id: format!("wiki:prior-{index}"),
                kind: "Wiki".into(),
                title: format!("Prior {index}"),
                excerpt: "Previously exact-read context.".into(),
                as_of: Some("2026-01-01T00:00:00Z".into()),
                section: None,
                access: MemoryAccess::ExactRead,
                full_record_digest: Some("a".repeat(64)),
            },
        );
    }
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: true,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: Some("2026-06-01".into()),
    };

    let searched = executor
        .execute(&policy, ToolMethod::GuruSearch, json!({"query": "prior"}))
        .await
        .unwrap();
    let records = searched["data"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["id"], "wiki:past");

    executor
        .execute(&policy, ToolMethod::GuruRead, json!({"id": "wiki:past"}))
        .await
        .unwrap();
    assert_eq!(
        executor
            .capture
            .memories
            .lock()
            .await
            .values()
            .filter(|memory| memory.access == MemoryAccess::ExactRead)
            .count(),
        9
    );
    let rejected = executor
        .execute(&policy, ToolMethod::GuruRead, json!({"id": "wiki:future"}))
        .await;
    assert!(matches!(
        rejected,
        Err(BrokerError::Execution(message)) if message.contains("as-of cutoff")
    ));
}

#[tokio::test]
async fn chat_lens_proposal_requires_library_quality_sections() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    executor
        .capture_memory(MemoryRefSnapshot {
            record_id: "lens:exact-source".into(),
            kind: "Lens".into(),
            title: "Source".into(),
            excerpt: "Exact source".into(),
            as_of: Some("2026-08-19T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    let rejected = executor
        .capture_proposal(json!({
            "kind": "lens",
            "target_id": "lens:thin",
            "proposed_markdown": "---\nid: lens:thin\ntitle: Thin\nsummary: Thin\nas_of: 2026-08-19T00:00:00Z\n---\n\nOne observation.\n",
            "rationale": "research-only learning",
            "source_ids": ["lens:exact-source"],
        }))
        .await;
    assert!(matches!(
        rejected,
        Err(BrokerError::Execution(message)) if message.contains("Scope")
    ));
    executor
        .capture_proposal(json!({
            "kind": "lens",
            "target_id": "lens:quality",
            "proposed_markdown": lens_markdown("lens:quality", "Quality"),
            "rationale": "research-only learning",
            "source_ids": ["lens:exact-source"],
        }))
        .await
        .unwrap();
}

#[tokio::test]
async fn chat_proposal_rejects_frontmatter_id_mismatch() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    executor
        .capture_memory(MemoryRefSnapshot {
            record_id: "wiki:source".into(),
            kind: "Wiki".into(),
            title: "Source".into(),
            excerpt: "Exact source".into(),
            as_of: Some("2026-08-19T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    let rejected = executor
        .capture_proposal(json!({
            "kind": "wiki",
            "target_id": "wiki:wanted",
            "proposed_markdown": wiki_markdown("wiki:other", "Other"),
            "rationale": "research-only learning",
            "source_ids": ["wiki:source"],
        }))
        .await;
    assert!(matches!(
        rejected,
        Err(BrokerError::Execution(message)) if message.contains("target_id")
    ));
}

#[tokio::test]
async fn chat_proposal_rejects_missing_summary_before_apply() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    executor
        .capture_memory(MemoryRefSnapshot {
            record_id: "wiki:source".into(),
            kind: "Wiki".into(),
            title: "Source".into(),
            excerpt: "Exact source".into(),
            as_of: Some("2026-08-19T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    let rejected = executor
        .capture_proposal(json!({
            "kind": "wiki",
            "target_id": "wiki:cobalt",
            "proposed_markdown": "---\nid: wiki:cobalt\ntitle: WP4 cobalt-foil spare-capacity rule\nas_of: 2026-08-24T00:00:00Z\n---\n\n# Scope\n\nReusable method.\n",
            "rationale": "research-only learning",
            "source_ids": ["wiki:source"],
        }))
        .await;
    assert!(matches!(
        rejected,
        Err(BrokerError::Execution(message)) if message.contains("summary is required")
    ));
}

#[tokio::test]
async fn chat_proposal_rejects_date_only_as_of_before_apply() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    executor
        .capture_memory(MemoryRefSnapshot {
            record_id: "wiki:source".into(),
            kind: "Wiki".into(),
            title: "Source".into(),
            excerpt: "Exact source".into(),
            as_of: Some("2026-08-19T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    let rejected = executor
        .capture_proposal(json!({
            "kind": "wiki",
            "target_id": "wiki:cobalt",
            "proposed_markdown": "---\nid: wiki:cobalt\ntitle: WP4 cobalt-foil spare-capacity rule\nsummary: Standing method.\nas_of: 2026-08-24\n---\n\n# Scope\n\nReusable method.\n",
            "rationale": "research-only learning",
            "source_ids": ["wiki:source"],
        }))
        .await;
    assert!(matches!(
        rejected,
        Err(BrokerError::Execution(message)) if message.contains("RFC3339 with seconds and timezone")
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn guru_search_omits_revoked_wiki_and_lens_by_default() {
    use crate::commands::tests::write_knowledge_runtime;

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "revoked");
    fs::write(
        workspace.join("guruterminal/wiki/active.md"),
        wiki_markdown("wiki:active-claim", "Active claim"),
    )
    .unwrap();
    fs::write(
        workspace.join("guruterminal/wiki/revoked.md"),
        "---\nid: wiki:revoked-claim\ntitle: Revoked claim\nsummary: Unused.\nas_of: 2026-08-01T00:00:00Z\nstatus: revoked\nrevoked_by: evidence:later\n---\n\n# Revoked claim\n\nSuperseded.\n",
    )
    .unwrap();
    let runtime_path = temporary.path().join("guruterminal-revoked-fixture");
    write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state,
        capture: Arc::new(ToolCapture::default()),
        guru_id: "guru-a".into(),
        guru_root: profile_workspace(&guru).unwrap(),
        chat_provider: String::new(),
    };
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: true,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    let searched = executor
        .execute(&policy, ToolMethod::GuruSearch, json!({"query": "claim"}))
        .await
        .unwrap();
    let records = searched["data"].as_array().unwrap();
    assert!(records
        .iter()
        .any(|record| record["id"] == "wiki:active-claim"));
    assert!(!records
        .iter()
        .any(|record| record["id"] == "wiki:revoked-claim"));

    let cutoff_policy = ToolPolicy {
        as_of: Some("2026-08-20".into()),
        ..policy
    };
    let cutoff_search = executor
        .execute(
            &cutoff_policy,
            ToolMethod::GuruSearch,
            json!({"query": "claim"}),
        )
        .await
        .unwrap();
    let cutoff_records = cutoff_search["data"].as_array().unwrap();
    assert!(cutoff_records
        .iter()
        .any(|record| record["id"] == "wiki:active-claim"));
    let revoked = cutoff_records
        .iter()
        .find(|record| record["id"] == "wiki:revoked-claim")
        .expect("cutoff search should surface revoked Wiki");
    assert_eq!(revoked["unused"], true);
}

#[tokio::test]
async fn evidence_create_accepts_readable_markdown_and_current_result_refs() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    let result_ref = delivered_result(
        &executor,
        json!({"company": {"commentary": "3nm utilization rose on CoWoS tightness."}}),
    )
    .await;
    assert!(matches!(
        executor
            .create_evidence(json!({
                "title": "Old contract",
                "summary": "Invalid old source IDs.",
                "as_of": "2026-08-13T00:00:00Z",
                "claims": [{"text": "claim", "citations": [{"source_id": "web:1"}]}]
            }))
            .await,
        Err(BrokerError::Malformed)
    ));
    let created = executor
        .create_evidence(json!({
            "title": "TSMC 3nm capacity",
            "summary": "Packaging tightness from this research turn.",
            "as_of": "2026-08-13T15:30:00+09:00",
            "markdown": "3nm utilization rose on CoWoS tightness.",
            "source": "https://example.test/tsmc",
            "period": "2026-Q2",
            "entities": ["ticker:TSM"],
            "citations": [{
                "result_ref": result_ref,
                "note": "TSMC commentary"
            }]
        }))
        .await
        .unwrap();
    assert!(created["evidence_id"]
        .as_str()
        .unwrap()
        .starts_with("evidence:chat/"));
    {
        let evidence = executor.capture.staged_evidence.lock().await;
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].title, "TSMC 3nm capacity");
        assert_eq!(evidence[0].as_of, "2026-08-13T15:30:00+09:00");
        assert_eq!(
            evidence[0].markdown,
            "3nm utilization rose on CoWoS tightness."
        );
        assert_eq!(
            evidence[0].source.as_deref(),
            Some("https://example.test/tsmc")
        );
        assert_eq!(
            evidence[0].citations[0].note.as_deref(),
            Some("TSMC commentary")
        );
        assert_eq!(
            evidence[0].citations[0].receipt.origin.as_deref(),
            Some("test")
        );
    }
}

#[tokio::test]
async fn evidence_create_rejects_prior_run_refs_and_reserved_body() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    let result = executor
        .create_evidence(json!({
            "title": "Prior evidence",
            "summary": "A prior ID is not a current result receipt.",
            "as_of": "2026-08-13T00:00:00Z",
            "markdown": "This must be refreshed.",
            "citations": [{"result_ref": "result:prior"}]
        }))
        .await;
    assert!(
        matches!(result, Err(BrokerError::Execution(message)) if message.contains("delivered result"))
    );
    let result_ref = delivered_result(&executor, json!({"text": "exact value"})).await;
    let reserved_heading = executor
        .create_evidence(json!({
            "title": "Reserved heading",
            "summary": "The host owns Sources.",
            "as_of": "2026-08-13T00:00:00Z",
            "markdown": "Claim text.\n\n# Sources\n\n- invented",
            "citations": [{"result_ref": result_ref}]
        }))
        .await;
    assert!(
        matches!(reserved_heading, Err(BrokerError::Execution(message)) if message.contains("# Sources"))
    );
}

#[tokio::test]
async fn evidence_create_rejects_date_only_as_of_and_runtime_fields() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    let result_ref = delivered_result(&executor, json!({"value": 42})).await;
    let date_only = executor
        .create_evidence(json!({
            "title": "Date only",
            "summary": "as_of must include time.",
            "as_of": "2026-08-24",
            "markdown": "The value is 42.",
            "citations": [{"result_ref": result_ref}]
        }))
        .await;
    assert!(matches!(date_only, Err(BrokerError::Malformed)));
    let pointer = executor
        .create_evidence(json!({
            "title": "Legacy pointer",
            "summary": "Pointers are no longer accepted.",
            "as_of": "2026-08-24T00:00:00Z",
            "markdown": "The value is 42.",
            "citations": [{"result_ref": result_ref, "pointer": "/value"}]
        }))
        .await;
    assert!(matches!(pointer, Err(BrokerError::Malformed)));
    let forged_receipt = executor
        .create_evidence(json!({
            "title": "Forged receipt",
            "summary": "Receipt metadata is host-owned.",
            "as_of": "2026-08-24T00:00:00Z",
            "markdown": "The value is 42.",
            "citations": [{
                "result_ref": result_ref,
                "receipt": {"provider": "forged"}
            }]
        }))
        .await;
    assert!(matches!(forged_receipt, Err(BrokerError::Malformed)));

    let oversized = executor
        .create_evidence(json!({
            "title": "Oversized body",
            "summary": "Evidence markdown must remain bounded.",
            "as_of": "2026-08-24T00:00:00Z",
            "markdown": "x".repeat(16 * 1024 + 1),
            "citations": [{"result_ref": result_ref}]
        }))
        .await;
    assert!(matches!(oversized, Err(BrokerError::Malformed)));
}

fn learned_wiki_markdown(id: &str, title: &str) -> String {
    format!(
        "---\nid: {id}\ntitle: {title}\nsummary: Advanced packaging, not wafer starts, is the binding capacity constraint for leading-edge TSMC nodes.\nas_of: 2026-03-15T00:00:00Z\naliases:\n  - Taiwan Semiconductor\n  - 2330.TW\nentities:\n  - TSMC\ntags:\n  - foundry\n  - CoWoS\n---\n\n# Constraint\n\nCoWoS and related packaging remain tighter than leading-edge wafer starts.\n"
    )
}

#[cfg(unix)]
fn memory_runtime_executor(
    temporary: &tempfile::TempDir,
    marker: &str,
) -> (AppToolExecutor, std::path::PathBuf) {
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, marker);
    let runtime_path = temporary.path().join(format!("guruterminal-{marker}"));
    crate::commands::tests::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state,
        capture: Arc::new(ToolCapture::default()),
        guru_id: "guru-a".into(),
        guru_root: profile_workspace(&guru).unwrap(),
        chat_provider: String::new(),
    };
    (executor, workspace)
}

#[cfg(unix)]
#[tokio::test]
async fn revising_an_existing_wiki_requires_an_exact_read_of_that_page() {
    let temporary = tempfile::tempdir().unwrap();
    let (executor, workspace) = memory_runtime_executor(&temporary, "revise-exact-read");
    let existing = learned_wiki_markdown("wiki:tsmc-foundry-economics", "TSMC foundry economics");
    fs::write(workspace.join("guruterminal/wiki/tsmc.md"), &existing).unwrap();
    executor
        .capture_memory(MemoryRefSnapshot {
            record_id: "wiki:source".into(),
            kind: "Wiki".into(),
            title: "Source".into(),
            excerpt: "Exact source".into(),
            as_of: Some("2026-03-15T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    let proposal = json!({
        "kind": "wiki",
        "target_id": "wiki:tsmc-foundry-economics",
        "proposed_markdown": learned_wiki_markdown(
            "wiki:tsmc-foundry-economics",
            "TSMC foundry economics"
        ),
        "rationale": "Update the compiled foundry constraint.",
        "source_ids": ["wiki:source"],
    });
    let rejected = executor.capture_proposal(proposal.clone()).await;
    assert!(
        matches!(rejected, Err(BrokerError::Execution(ref message)) if message.contains("exact-read")),
        "{rejected:?}"
    );
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: true,
        propose_memory_updates: true,
        memory_proposal_budget: 1,
        as_of: None,
    };
    executor
        .guru_read(
            &policy,
            json!({"id": "wiki:tsmc-foundry-economics", "section": "Constraint"}),
            "section-read",
        )
        .await
        .unwrap();
    executor.capture.commit_delivery("section-read").await;
    let section_read =
        executor.capture.memories.lock().await["wiki:tsmc-foundry-economics"].clone();
    assert_eq!(section_read.section.as_deref(), Some("Constraint"));
    assert_eq!(section_read.full_record_digest, None);
    let rejected = executor.capture_proposal(proposal.clone()).await;
    assert!(
        matches!(rejected, Err(BrokerError::Execution(ref message)) if message.contains("full existing record")),
        "{rejected:?}"
    );

    executor
        .guru_read(
            &policy,
            json!({"id": "wiki:tsmc-foundry-economics"}),
            "full-read",
        )
        .await
        .unwrap();
    executor.capture.commit_delivery("full-read").await;
    let expected_digest = crate::hashing::sha256(existing.as_bytes());
    assert_eq!(
        executor.capture.memories.lock().await["wiki:tsmc-foundry-economics"]
            .full_record_digest
            .as_deref(),
        Some(expected_digest.as_str())
    );
    executor.capture_proposal(proposal).await.unwrap();
    assert_eq!(
        executor.capture.proposal.lock().await[0].target_base,
        crate::domain::MemoryProposalBase::FullRead {
            digest: expected_digest
        }
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_full_read_target_deleted_before_proposal_cannot_be_recreated_as_absent() {
    let temporary = tempfile::tempdir().unwrap();
    let (executor, workspace) = memory_runtime_executor(&temporary, "deleted-after-full-read");
    let target_id = "wiki:deleted-after-read";
    let target_path = workspace.join("guruterminal/wiki/deleted-after-read.md");
    let existing = learned_wiki_markdown(target_id, "Deleted after read");
    fs::write(&target_path, &existing).unwrap();
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: true,
        propose_memory_updates: true,
        memory_proposal_budget: 1,
        as_of: None,
    };
    executor
        .guru_read(&policy, json!({"id": target_id}), "full-read-before-delete")
        .await
        .unwrap();
    executor
        .capture
        .commit_delivery("full-read-before-delete")
        .await;
    let expected_digest = crate::hashing::sha256(existing.as_bytes());
    assert_eq!(
        executor.capture.memories.lock().await[target_id]
            .full_record_digest
            .as_deref(),
        Some(expected_digest.as_str())
    );

    fs::remove_file(target_path).unwrap();
    let rejected = executor
        .capture_proposal(json!({
            "kind": "wiki",
            "target_id": target_id,
            "proposed_markdown": existing.replace(
                "Durable fact from current research.",
                "A stale recreation must be rejected."
            ),
            "rationale": "Do not recreate a target deleted after its full read.",
            "source_ids": [target_id],
        }))
        .await;
    assert!(
        matches!(rejected, Err(BrokerError::Execution(ref message)) if message.contains("no longer exists")),
        "{rejected:?}"
    );
    assert!(executor.capture.proposal.lock().await.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn new_wiki_with_a_colliding_title_or_alias_must_revise_the_match() {
    let temporary = tempfile::tempdir().unwrap();
    let (executor, workspace) = memory_runtime_executor(&temporary, "semantic-dedup");
    fs::write(
        workspace.join("guruterminal/wiki/tsmc.md"),
        learned_wiki_markdown("wiki:tsmc-foundry-economics", "TSMC foundry economics"),
    )
    .unwrap();
    executor
        .capture_memory(MemoryRefSnapshot {
            record_id: "wiki:source".into(),
            kind: "Wiki".into(),
            title: "Source".into(),
            excerpt: "Exact source".into(),
            as_of: Some("2026-03-15T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    let rejected = executor
        .capture_proposal(json!({
            "kind": "wiki",
            "target_id": "wiki:taiwan-foundry",
            "proposed_markdown": learned_wiki_markdown(
                "wiki:taiwan-foundry",
                "Taiwan Semiconductor"
            ),
            "rationale": "Do not fork the same compiled fact.",
            "source_ids": ["wiki:source"],
        }))
        .await;
    assert!(
        matches!(
            rejected,
            Err(BrokerError::Execution(ref message))
                if message.contains("wiki:tsmc-foundry-economics") && message.contains("duplicate")
        ),
        "{rejected:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn wiki_search_cards_surface_entities_and_omit_revoked_pages() {
    let temporary = tempfile::tempdir().unwrap();
    let (executor, workspace) = memory_runtime_executor(&temporary, "later-retrieval");
    fs::write(
        workspace.join("guruterminal/wiki/tsmc.md"),
        learned_wiki_markdown("wiki:tsmc-foundry-economics", "TSMC foundry economics"),
    )
    .unwrap();
    fs::write(
        workspace.join("guruterminal/wiki/revoked.md"),
        "---\nid: wiki:old-foundry\ntitle: Old foundry note\nsummary: Unused.\nas_of: 2026-01-01T00:00:00Z\nstatus: revoked\nrevoked_by: wiki:tsmc-foundry-economics\n---\n\n# Unused\n\nSuperseded.\n",
    )
    .unwrap();
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: true,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    let searched = executor
        .execute(
            &policy,
            ToolMethod::GuruSearch,
            json!({"query": "Taiwan Semiconductor packaging bottleneck"}),
        )
        .await
        .unwrap();
    let records = searched["data"].as_array().unwrap();
    let hit = records
        .iter()
        .find(|record| record["id"] == "wiki:tsmc-foundry-economics")
        .expect("later research prompt should retrieve the learned Wiki");
    assert!(hit["entities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity == "TSMC"));
    assert!(!records
        .iter()
        .any(|record| record["id"] == "wiki:old-foundry"));
}

#[tokio::test]
async fn chat_proposal_rejects_revoked_wiki_without_a_revoked_by_pointer() {
    let temporary = tempfile::tempdir().unwrap();
    let executor = bare_executor(&temporary);
    executor
        .capture_memory(MemoryRefSnapshot {
            record_id: "wiki:source".into(),
            kind: "Wiki".into(),
            title: "Source".into(),
            excerpt: "Exact source".into(),
            as_of: Some("2026-08-19T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some("a".repeat(64)),
        })
        .await
        .unwrap();
    let rejected = executor
        .capture_proposal(json!({
            "kind": "wiki",
            "target_id": "wiki:stale",
            "proposed_markdown": "---\nid: wiki:stale\ntitle: Stale\nsummary: Unused.\nas_of: 2026-08-19T00:00:00Z\nstatus: revoked\n---\n\n# Stale\n\nHide this claim.\n",
            "rationale": "Retire the unused claim.",
            "source_ids": ["wiki:source"],
        }))
        .await;
    assert!(
        matches!(rejected, Err(BrokerError::Execution(ref message)) if message.contains("revoked_by")),
        "{rejected:?}"
    );
}
