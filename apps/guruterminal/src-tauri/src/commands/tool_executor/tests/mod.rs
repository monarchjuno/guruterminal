use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::*;
use crate::{
    broker::{start_tool_broker, tool_broker_endpoint},
    commands::{
        enabled_execute_capability_ids,
        tests::{
            bound_root, chat, initialized_workspace, lens_markdown, profile, seed_profile,
            wiki_markdown,
        },
    },
    domain::{memory_refs_digest, ChatMessage, ChatRole},
    guru_root::profile_workspace,
};

fn captured_capabilities(state: &AppState, guru_id: &str) -> BTreeSet<String> {
    enabled_execute_capability_ids(state, guru_id)
        .unwrap()
        .into_iter()
        .collect()
}

fn decision(stance: &str, evidence_ids: Vec<String>) -> Value {
    json!({
        "stance": stance,
        "horizon": "12 months",
        "probability": 0.5,
        "thesis": "Bounded test judgment",
        "evidence_ids": evidence_ids,
        "uses_ids": [],
        "risks": ["Evidence can change"],
        "invalidation_conditions": ["The cited evidence is superseded"]
    })
}

fn bare_executor(temporary: &tempfile::TempDir) -> AppToolExecutor {
    let workspace = temporary.path().join("guru");
    fs::create_dir_all(&workspace).unwrap();
    AppToolExecutor {
        state: AppState::for_test(temporary.path().join("app")),
        capture: Arc::new(ToolCapture::default()),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        capability_ids: BTreeSet::new(),
        chat_provider: String::new(),
    }
}

struct CountingExecutor {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolExecutor for CountingExecutor {
    async fn execute(
        &self,
        _policy: &ToolPolicy,
        _method: ToolMethod,
        _params: Value,
    ) -> Result<Value, BrokerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"unexpected": true}))
    }
}

async fn broker_request(socket: &Path, token: &str, method: &str, params: Value) -> Value {
    #[cfg(unix)]
    let mut stream = tokio::net::UnixStream::connect(socket).await.unwrap();
    #[cfg(windows)]
    let mut stream = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(socket)
        .unwrap();
    let mut request = serde_json::to_vec(&json!({
        "protocol": "guruterminal-tool/1",
        "id": "request-1",
        "token": token,
        "method": method,
        "params": params,
    }))
    .unwrap();
    request.push(b'\n');
    stream.write_all(&request).await.unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    let ack = json!({
        "protocol": "guruterminal-tool/1",
        "id": "request-1",
        "delivered": true,
    });
    reader
        .get_mut()
        .write_all(format!("{ack}\n").as_bytes())
        .await
        .unwrap();
    reader.get_mut().shutdown().await.unwrap();
    line.clear();
    reader.read_line(&mut line).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({
            "protocol": "guruterminal-tool/1",
            "id": "request-1",
            "committed": true,
        })
    );
    line.clear();
    assert_eq!(reader.read_line(&mut line).await.unwrap(), 0);
    response
}

#[test]
fn run_result_receipts_collect_web_and_mcp_warning_shapes() {
    assert_eq!(
        result_warnings(&json!({
            "warnings": ["top-level"],
            "quality_warnings": ["navigation-heavy"],
            "structuredContent": {"warnings": [
                "provider-delay",
                "top-level",
                {"category": "OpenBBWarning", "message": "provider fallback"}
            ]},
            "quality": {"warning": "partial"}
        })),
        [
            "OpenBBWarning: provider fallback",
            "navigation-heavy",
            "partial",
            "provider-delay",
            "top-level"
        ]
    );
}

#[test]
fn web_receipts_use_actual_search_provider_and_fetch_origin() {
    let search = run_result_producer(
        ToolMethod::WebSearch,
        &json!({}),
        &json!({"selected_provider": "exa_public", "source_id": "web:opaque"}),
    )
    .unwrap();
    assert_eq!(search.provider.as_deref(), Some("exa_public"));

    let fetch = run_result_producer(
        ToolMethod::WebFetch,
        &json!({}),
        &json!({
            "source_id": "web:opaque",
            "final_url": "https://investor.example.test/report"
        }),
    )
    .unwrap();
    assert_eq!(fetch.provider.as_deref(), Some("investor.example.test"));
}

mod finance_calculate;
mod memory_authority;
mod scope_and_artifacts;
mod workbench;
