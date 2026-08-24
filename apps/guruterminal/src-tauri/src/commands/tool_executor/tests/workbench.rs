use super::*;
use std::path::PathBuf;

fn workbench_executor(temporary: &tempfile::TempDir) -> (AppToolExecutor, PathBuf, ToolPolicy) {
    let state = AppState::for_test(temporary.path().join("app"));
    let workspace = temporary.path().join("memory");
    fs::create_dir_all(&workspace).unwrap();
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();
    let workbench = state
        .artifacts
        .deletion_root
        .absolute_path(Path::new("gurus/guru-a"))
        .unwrap()
        .join("workbench");
    fs::create_dir_all(&workbench).unwrap();
    let executor = AppToolExecutor {
        capability_ids: captured_capabilities(&state, "guru-a"),
        state,
        capture: Arc::new(ToolCapture::default()),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };
    let policy = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: false,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    (executor, workbench, policy)
}

#[tokio::test]
async fn workbench_broker_tools_round_trip_revision_and_reject_attachments() {
    let temporary = tempfile::tempdir().unwrap();
    let (executor, workbench, policy) = workbench_executor(&temporary);
    let created = executor
        .execute(
            &policy,
            ToolMethod::WorkbenchWrite,
            json!({
                "path": "notes/idea.md",
                "content": "durable insight"
            }),
        )
        .await
        .unwrap();
    assert_eq!(created["status"], "ok");
    let revision = created["revision"].as_str().unwrap().to_owned();
    let read = executor
        .execute(
            &policy,
            ToolMethod::WorkbenchRead,
            json!({ "path": "notes/idea.md" }),
        )
        .await
        .unwrap();
    assert_eq!(read["content"], "durable insight");
    assert_eq!(read["revision"], revision);
    let edited = executor
        .execute(
            &policy,
            ToolMethod::WorkbenchEdit,
            json!({
                "path": "notes/idea.md",
                "old_text": "durable",
                "new_text": "revised",
                "expected_revision": revision
            }),
        )
        .await
        .unwrap();
    assert_eq!(edited["status"], "ok");
    assert_eq!(
        fs::read_to_string(workbench.join("notes/idea.md")).unwrap(),
        "revised insight"
    );

    let attachment = workbench.join("attachments/chat-a/message-a");
    fs::create_dir_all(&attachment).unwrap();
    fs::write(attachment.join("file"), "immutable").unwrap();
    let denied = executor
        .execute(
            &policy,
            ToolMethod::WorkbenchWrite,
            json!({
                "path": "attachments/chat-a/message-a/file",
                "content": "no"
            }),
        )
        .await
        .unwrap_err();
    match denied {
        BrokerError::Execution(message) => {
            assert!(message.contains("attachment snapshots are read-only"));
        }
        other => panic!("expected execution error, got {other:?}"),
    }
}

#[tokio::test]
async fn workbench_broker_rejects_malformed_edit_without_revision() {
    let temporary = tempfile::tempdir().unwrap();
    let (executor, _, policy) = workbench_executor(&temporary);
    let error = executor
        .execute(
            &policy,
            ToolMethod::WorkbenchEdit,
            json!({
                "path": "note.md",
                "old_text": "a",
                "new_text": "b"
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, BrokerError::Malformed));
}

#[tokio::test]
async fn workbench_queries_are_delivery_backed_run_results_with_distinct_producers() {
    let temporary = tempfile::tempdir().unwrap();
    let (executor, workbench, policy) = workbench_executor(&temporary);
    fs::create_dir_all(workbench.join("notes")).unwrap();
    fs::write(workbench.join("notes/alpha.md"), "one\nmatch here\nthree\n").unwrap();
    fs::write(workbench.join("notes/data.bin"), [0_u8, 1, 2]).unwrap();

    let list_request = json!({"path": ".", "limit": 10});
    let listed = executor
        .execute_for_delivery(
            &policy,
            ToolMethod::WorkbenchList,
            list_request.clone(),
            "delivery-list",
        )
        .await
        .unwrap();
    let list_ref = listed["result_ref"].as_str().unwrap();
    assert!(listed["text"].as_str().unwrap().contains("dir \tnotes"));
    assert!(executor.capture.run_result(list_ref).await.is_none());

    executor.commit_delivery(&policy, "delivery-list").await;
    let captured = executor.capture.run_result(list_ref).await.unwrap();
    assert_eq!(captured.producer.runtime_id, "workbench");
    assert_eq!(captured.producer.tool_name, "ls");
    assert_eq!(captured.producer.provider, None);
    assert_eq!(captured.payload["count"], 1);
    assert!(captured.payload.get("result_ref").is_none());
    assert_eq!(
        captured.request_digest,
        sha256(&serde_json::to_vec(&list_request).unwrap())
    );
    assert_eq!(
        captured.response_digest,
        sha256(&serde_json::to_vec(&captured.payload).unwrap())
    );
    assert!(!captured.retrieved_at.is_empty());
    assert!(captured.upstream_result_refs.is_empty());

    let found = executor
        .execute(
            &policy,
            ToolMethod::WorkbenchFind,
            json!({"pattern": "**/*.md"}),
        )
        .await
        .unwrap();
    assert!(found["text"].as_str().unwrap().contains("notes/alpha.md"));
    let find_result = executor
        .capture
        .run_result(found["result_ref"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(find_result.producer.runtime_id, "workbench");
    assert_eq!(find_result.producer.tool_name, "find");

    let searched = executor
        .execute(
            &policy,
            ToolMethod::WorkbenchGrep,
            json!({
                "pattern": "match",
                "context": 1
            }),
        )
        .await
        .unwrap();
    let text = searched["text"].as_str().unwrap();
    assert!(text.contains("notes/alpha.md-1-one"));
    assert!(text.contains("notes/alpha.md:2:match here"));
    assert!(text.contains("notes/alpha.md-3-three"));
    assert_eq!(searched["skipped_binary"], 1);
    assert!(searched["warnings"][0]
        .as_str()
        .unwrap()
        .contains("Skipped 1 binary file"));
    let grep_result = executor
        .capture
        .run_result(searched["result_ref"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(grep_result.producer.runtime_id, "workbench");
    assert_eq!(grep_result.producer.tool_name, "grep");
    assert_eq!(
        grep_result.warnings,
        ["Skipped 1 binary file: notes/data.bin"]
    );
}

#[tokio::test]
async fn workbench_query_result_is_discarded_without_delivery_ack() {
    let temporary = tempfile::tempdir().unwrap();
    let (executor, workbench, policy) = workbench_executor(&temporary);
    fs::write(workbench.join("note.md"), "bounded").unwrap();

    let result = executor
        .execute_for_delivery(
            &policy,
            ToolMethod::WorkbenchFind,
            json!({"pattern": "*.md"}),
            "delivery-discarded",
        )
        .await
        .unwrap();
    let result_ref = result["result_ref"].as_str().unwrap().to_owned();
    assert!(executor.capture.run_result(&result_ref).await.is_none());

    executor
        .discard_delivery(&policy, "delivery-discarded")
        .await;
    assert!(executor.capture.run_result(&result_ref).await.is_none());
    assert_eq!(
        executor.capture.run_results.lock().await.values().count(),
        0
    );
}

#[tokio::test]
async fn workbench_query_broker_ack_commits_the_visible_result_ref() {
    let temporary = tempfile::tempdir().unwrap();
    let (executor, workbench, policy) = workbench_executor(&temporary);
    fs::write(workbench.join("note.md"), "bounded").unwrap();
    let capture = executor.capture.clone();
    let socket = tool_broker_endpoint(temporary.path().join("workbench-query.sock"));
    let broker = start_tool_broker(socket.clone(), policy, Arc::new(executor))
        .await
        .unwrap();

    let response = broker_request(
        &socket,
        broker.token(),
        "workbench.ls",
        json!({"limit": 10}),
    )
    .await;
    assert_eq!(response["ok"], true);
    let result_ref = response["result"]["result_ref"].as_str().unwrap();
    let captured = capture.run_result(result_ref).await.unwrap();
    assert_eq!(captured.producer.runtime_id, "workbench");
    assert_eq!(captured.producer.tool_name, "ls");
    broker.shutdown().await.unwrap();
}

#[tokio::test]
async fn workbench_queries_enforce_paths_patterns_and_output_bounds() {
    let temporary = tempfile::tempdir().unwrap();
    let (executor, workbench, policy) = workbench_executor(&temporary);
    fs::write(
        workbench.join("large.md"),
        format!("match {}", "é".repeat(40_000)),
    )
    .unwrap();

    let searched = executor
        .execute(
            &policy,
            ToolMethod::WorkbenchGrep,
            json!({"pattern": "match"}),
        )
        .await
        .unwrap();
    let text = searched["text"].as_str().unwrap();
    assert!(text.ends_with("[Output truncated at 50KB]"));
    assert_eq!(searched["truncated"], true);
    assert!(text.len() <= crate::workbench::MAX_TOOL_OUTPUT_BYTES + 64);

    for (method, request) in [
        (ToolMethod::WorkbenchList, json!({"path": "../outside"})),
        (ToolMethod::WorkbenchFind, json!({"pattern": ""})),
        (
            ToolMethod::WorkbenchGrep,
            json!({"pattern": "(?=unsupported)"}),
        ),
        (
            ToolMethod::WorkbenchGrep,
            json!({"pattern": "match", "context": 4}),
        ),
    ] {
        assert!(executor.execute(&policy, method, request).await.is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn workbench_queries_never_follow_symlinks() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let (executor, workbench, policy) = workbench_executor(&temporary);
    let outside = temporary.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.md"), "outside match").unwrap();
    symlink(&outside, workbench.join("portal")).unwrap();

    let root = executor
        .execute(
            &policy,
            ToolMethod::WorkbenchGrep,
            json!({"pattern": "match"}),
        )
        .await
        .unwrap();
    assert!(!root["text"].as_str().unwrap().contains("outside match"));
    assert!(!root["text"].as_str().unwrap().contains("portal"));

    let escaped = executor
        .execute(
            &policy,
            ToolMethod::WorkbenchList,
            json!({"path": "portal"}),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        escaped,
        BrokerError::Execution(message) if message.contains("symlink")
    ));
}
