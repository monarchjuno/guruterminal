use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct UnusedExecutor;

#[async_trait]
impl ToolExecutor for UnusedExecutor {
    async fn execute(
        &self,
        _policy: &ToolPolicy,
        _method: ToolMethod,
        _params: Value,
    ) -> Result<Value, BrokerError> {
        panic!("unauthenticated connection must not reach the executor")
    }
}

fn policy() -> ToolPolicy {
    ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "session-a".into(),
        use_memory: false,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    }
}

#[test]
fn broker_endpoint_matches_the_platform_transport_namespace() {
    let logical = PathBuf::from("run").join("broker.sock");
    let endpoint = tool_broker_endpoint(logical.clone());
    #[cfg(unix)]
    assert_eq!(endpoint, logical);
    #[cfg(windows)]
    {
        let text = endpoint.to_string_lossy();
        assert!(text.starts_with(r"\\.\pipe\guruterminal-tool-"));
        assert_eq!(text.len(), r"\\.\pipe\guruterminal-tool-".len() + 32);
    }
}

#[tokio::test]
async fn connection_that_never_authenticates_is_bounded() {
    let (server, _stalled_client) = tokio::io::duplex(64);
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        handle_connection(
            server,
            "secret",
            &policy(),
            Arc::new(StdMutex::new(TransactionCardinality::default())),
            Arc::new(UnusedExecutor),
        ),
    )
    .await
    .expect("broker did not enforce authentication deadline")
    .unwrap_err();
    assert!(matches!(
        result,
        BrokerError::Io(ref error) if error.kind() == io::ErrorKind::TimedOut
    ));
}

#[tokio::test]
async fn successful_request_commits_after_delivery_ack_before_eof() {
    #[derive(Default)]
    struct Counters {
        executed: AtomicUsize,
        committed: AtomicUsize,
        discarded: AtomicUsize,
    }
    struct Executor(Arc<Counters>);

    #[async_trait]
    impl ToolExecutor for Executor {
        async fn execute(
            &self,
            _policy: &ToolPolicy,
            method: ToolMethod,
            params: Value,
        ) -> Result<Value, BrokerError> {
            assert_eq!(method, ToolMethod::FinanceSources);
            self.0.executed.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"received": params}))
        }

        async fn commit_delivery(&self, _policy: &ToolPolicy, _delivery_id: &str) {
            self.0.committed.fetch_add(1, Ordering::SeqCst);
        }

        async fn discard_delivery(&self, _policy: &ToolPolicy, _delivery_id: &str) {
            self.0.discarded.fetch_add(1, Ordering::SeqCst);
        }
    }

    let counters = Arc::new(Counters::default());
    let executor = Arc::new(Executor(counters.clone()));
    let (server, mut client) = tokio::io::duplex(4096);
    let policy = policy();
    let server_task = tokio::spawn(async move {
        handle_connection(
            server,
            "secret",
            &policy,
            Arc::new(StdMutex::new(TransactionCardinality::default())),
            executor,
        )
        .await
    });
    let request = json!({
        "protocol": PROTOCOL,
        "id": "request-1",
        "token": "secret",
        "method": "finance.sources",
        "params": {"provider": "all"}
    });
    client
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();

    let mut reader = BufReader::new(client);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["protocol"], PROTOCOL);
    assert_eq!(response["id"], "request-1");
    assert_eq!(response["ok"], true);
    assert_eq!(response["result"]["received"]["provider"], "all");
    assert!(response.get("phase").is_none());
    assert!(response.get("delivery_token").is_none());
    assert_eq!(counters.executed.load(Ordering::SeqCst), 1);
    assert_eq!(counters.committed.load(Ordering::SeqCst), 0);
    assert_eq!(counters.discarded.load(Ordering::SeqCst), 0);

    let ack = json!({
        "protocol": PROTOCOL,
        "id": "request-1",
        "delivered": true
    });
    reader
        .get_mut()
        .write_all(format!("{ack}\n").as_bytes())
        .await
        .unwrap();
    reader.get_mut().shutdown().await.unwrap();

    line.clear();
    reader.read_line(&mut line).await.unwrap();
    let committed: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        committed,
        json!({
            "protocol": PROTOCOL,
            "id": "request-1",
            "committed": true
        })
    );
    assert_eq!(counters.committed.load(Ordering::SeqCst), 1);
    assert_eq!(counters.discarded.load(Ordering::SeqCst), 0);
    line.clear();
    assert_eq!(reader.read_line(&mut line).await.unwrap(), 0);
    server_task.await.unwrap().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn node_client_interoperates_with_the_real_broker_commit_barrier() {
    #[derive(Default)]
    struct Counters {
        committed: AtomicUsize,
        discarded: AtomicUsize,
    }
    struct Executor(Arc<Counters>);

    #[async_trait]
    impl ToolExecutor for Executor {
        async fn execute(
            &self,
            _policy: &ToolPolicy,
            method: ToolMethod,
            params: Value,
        ) -> Result<Value, BrokerError> {
            assert_eq!(method, ToolMethod::FinanceSources);
            Ok(json!({"received": params}))
        }

        async fn commit_delivery(&self, _policy: &ToolPolicy, _delivery_id: &str) {
            self.0.committed.fetch_add(1, Ordering::SeqCst);
        }

        async fn discard_delivery(&self, _policy: &ToolPolicy, _delivery_id: &str) {
            self.0.discarded.fetch_add(1, Ordering::SeqCst);
        }
    }

    let temporary = tempfile::tempdir().unwrap();
    let socket_path = temporary.path().join("broker.sock");
    let counters = Arc::new(Counters::default());
    let broker = start_tool_broker(
        socket_path.clone(),
        policy(),
        Arc::new(Executor(counters.clone())),
    )
    .await
    .unwrap();
    let client_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../agent/broker-client.mjs")
        .canonicalize()
        .unwrap();
    let script = r#"
        import { pathToFileURL } from "node:url";
        const { requestBroker } = await import(pathToFileURL(process.env.GURU_BROKER_CLIENT).href);
        const result = await requestBroker(
          "finance.sources",
          { provider: "all" },
          new AbortController().signal,
        );
        if (result?.received?.provider !== "all") process.exitCode = 2;
    "#;
    let output = tokio::process::Command::new("node")
        .args(["--input-type=module", "--eval", script])
        .env("GURU_BROKER_CLIENT", client_path)
        .env("GURUTERMINAL_BROKER_SOCKET", &socket_path)
        .env("GURUTERMINAL_BROKER_TOKEN", broker.token())
        .output()
        .await
        .unwrap();

    assert!(
        output.status.success(),
        "Node broker client failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(counters.committed.load(Ordering::SeqCst), 1);
    assert_eq!(counters.discarded.load(Ordering::SeqCst), 0);
    broker.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn node_client_settles_a_slow_executor_before_the_commit_barrier() {
    struct Executor;

    #[async_trait]
    impl ToolExecutor for Executor {
        async fn execute(
            &self,
            _policy: &ToolPolicy,
            method: ToolMethod,
            params: Value,
        ) -> Result<Value, BrokerError> {
            assert_eq!(method, ToolMethod::FinanceSources);
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok(json!({"received": params}))
        }
    }

    let temporary = tempfile::tempdir().unwrap();
    let socket_path = temporary.path().join("broker-slow.sock");
    let broker = start_tool_broker(socket_path.clone(), policy(), Arc::new(Executor))
        .await
        .unwrap();
    let client_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../agent/broker-client.mjs")
        .canonicalize()
        .unwrap();
    let script = r#"
        import { pathToFileURL } from "node:url";
        const { requestBroker } = await import(pathToFileURL(process.env.GURU_BROKER_CLIENT).href);
        const result = await requestBroker(
          "finance.sources",
          { provider: "all" },
          new AbortController().signal,
        );
        if (result?.received?.provider !== "all") process.exitCode = 2;
    "#;
    let output = tokio::process::Command::new("node")
        .args(["--input-type=module", "--eval", script])
        .env("GURU_BROKER_CLIENT", client_path)
        .env("GURUTERMINAL_BROKER_SOCKET", &socket_path)
        .env("GURUTERMINAL_BROKER_TOKEN", broker.token())
        .output()
        .await
        .unwrap();

    assert!(
        output.status.success(),
        "Node broker client failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    broker.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_delivery_ack_discards_the_staged_result_without_a_commit_barrier() {
    #[derive(Default)]
    struct Counters {
        committed: AtomicUsize,
        discarded: AtomicUsize,
    }
    struct Executor(Arc<Counters>);

    #[async_trait]
    impl ToolExecutor for Executor {
        async fn execute(
            &self,
            _policy: &ToolPolicy,
            _method: ToolMethod,
            _params: Value,
        ) -> Result<Value, BrokerError> {
            Ok(json!({"staged": true}))
        }

        async fn commit_delivery(&self, _policy: &ToolPolicy, _delivery_id: &str) {
            self.0.committed.fetch_add(1, Ordering::SeqCst);
        }

        async fn discard_delivery(&self, _policy: &ToolPolicy, _delivery_id: &str) {
            self.0.discarded.fetch_add(1, Ordering::SeqCst);
        }
    }

    let counters = Arc::new(Counters::default());
    let (server, mut client) = tokio::io::duplex(4096);
    let policy = policy();
    let executor = Arc::new(Executor(counters.clone()));
    let server_task = tokio::spawn(async move {
        handle_connection(
            server,
            "secret",
            &policy,
            Arc::new(StdMutex::new(TransactionCardinality::default())),
            executor,
        )
        .await
    });
    let request = json!({
        "protocol": PROTOCOL,
        "id": "request-no-ack",
        "token": "secret",
        "method": "finance.sources",
        "params": {}
    });
    client
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let mut reader = BufReader::new(client);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["ok"], true);
    reader.get_mut().shutdown().await.unwrap();

    assert!(matches!(
        server_task.await.unwrap(),
        Err(BrokerError::Malformed)
    ));
    assert_eq!(counters.committed.load(Ordering::SeqCst), 0);
    assert_eq!(counters.discarded.load(Ordering::SeqCst), 1);
    line.clear();
    assert_eq!(reader.read_line(&mut line).await.unwrap(), 0);
}

#[tokio::test]
async fn malformed_delivery_ack_discards_the_staged_result() {
    #[derive(Default)]
    struct Counters {
        committed: AtomicUsize,
        discarded: AtomicUsize,
    }
    struct Executor(Arc<Counters>);

    #[async_trait]
    impl ToolExecutor for Executor {
        async fn execute(
            &self,
            _policy: &ToolPolicy,
            _method: ToolMethod,
            _params: Value,
        ) -> Result<Value, BrokerError> {
            Ok(json!({"staged": true}))
        }

        async fn commit_delivery(&self, _policy: &ToolPolicy, _delivery_id: &str) {
            self.0.committed.fetch_add(1, Ordering::SeqCst);
        }

        async fn discard_delivery(&self, _policy: &ToolPolicy, _delivery_id: &str) {
            self.0.discarded.fetch_add(1, Ordering::SeqCst);
        }
    }

    let counters = Arc::new(Counters::default());
    let (server, mut client) = tokio::io::duplex(4096);
    let policy = policy();
    let executor = Arc::new(Executor(counters.clone()));
    let server_task = tokio::spawn(async move {
        handle_connection(
            server,
            "secret",
            &policy,
            Arc::new(StdMutex::new(TransactionCardinality::default())),
            executor,
        )
        .await
    });
    let request = json!({
        "protocol": PROTOCOL,
        "id": "request-bad-ack",
        "token": "secret",
        "method": "finance.sources",
        "params": {}
    });
    client
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let mut reader = BufReader::new(client);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    reader
        .get_mut()
        .write_all(b"{\"protocol\":\"guru-tool-broker/1\",\"id\":\"wrong\",\"delivered\":true}\n")
        .await
        .unwrap();

    assert!(matches!(
        server_task.await.unwrap(),
        Err(BrokerError::Malformed)
    ));
    assert_eq!(counters.committed.load(Ordering::SeqCst), 0);
    assert_eq!(counters.discarded.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_evidence_create_does_not_consume_the_turn_budget() {
    let policy = policy();
    let mut cardinality = TransactionCardinality::default();
    cardinality
        .consume(ToolMethod::EvidenceCreate, &policy)
        .unwrap();
    cardinality.rollback(ToolMethod::EvidenceCreate);
    for _ in 0..3 {
        cardinality
            .consume(ToolMethod::EvidenceCreate, &policy)
            .unwrap();
    }
    assert!(matches!(
        cardinality.consume(ToolMethod::EvidenceCreate, &policy),
        Err(BrokerError::BudgetExceeded)
    ));
}

#[test]
fn a_run_may_publish_several_artifacts_but_not_past_the_turn_bound() {
    let policy = policy();
    let mut cardinality = TransactionCardinality::default();
    for _ in 0..MAX_CHAT_TURN_ARTIFACTS {
        cardinality
            .consume(ToolMethod::ChartPublish, &policy)
            .unwrap();
    }
    assert!(matches!(
        cardinality.consume(ToolMethod::ArtifactPublish, &policy),
        Err(BrokerError::BudgetExceeded)
    ));
    cardinality
        .consume(ToolMethod::DecisionSubmit, &policy)
        .unwrap();
    assert!(matches!(
        cardinality.consume(ToolMethod::DecisionSubmit, &policy),
        Err(BrokerError::BudgetExceeded)
    ));
}

#[test]
fn response_encoding_is_one_bounded_newline_delimited_frame() {
    let bytes = encode_response(&BrokerResponse {
        protocol: PROTOCOL,
        id: "request-1".into(),
        ok: true,
        result: Some(json!({"ok": true})),
        error: None,
    })
    .unwrap();
    assert!(bytes.len() <= MAX_FRAME_BYTES);
    assert_eq!(bytes.last(), Some(&b'\n'));
    let decoded: Value = serde_json::from_slice(&bytes[..bytes.len() - 1]).unwrap();
    assert_eq!(decoded["ok"], true);
    assert!(decoded.get("phase").is_none());
}
