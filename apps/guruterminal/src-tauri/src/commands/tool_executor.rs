mod artifacts;
mod authority;
mod finance;
mod market;
mod mcp_host;
mod research;
mod workbench;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::{
    app::AppState,
    broker::{BrokerError, ToolExecutor, ToolMethod, ToolPolicy},
    chat_artifacts::{
        ArtifactCommit, ChatArtifact, ChatArtifactKind, ChatArtifactPayload, ChatArtifactRevision,
    },
    domain::{ChatDecision, MemoryAccess, MemoryProposal, MemoryRefSnapshot},
    finance::{FinanceLaunchConfig, FinanceWorker},
    guru_root::BoundGuruRoot,
    hashing::sha256,
    mcp_pool::{McpProcessPool, TurnMcpServer},
    store::GuruTerminalStore,
};

use super::{chat_runtime::parse_memory_kind, new_id, now_ms, types::runtime_record_summary};

use market::*;

const DEFAULT_CHART_QUERY_ROWS: usize = 50;
const MAX_CHART_QUERY_ROWS: usize = 200;
const MAX_CHART_QUERY_BYTES: usize = 1024 * 1024;
const MAX_RUN_RESULTS: usize = 64;
const MAX_RUN_RESULT_BYTES: usize = 4 * 1024 * 1024;
const MAX_RUN_RESULT_REGISTRY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChartQueryRequest {
    artifact_id: String,
    revision: u32,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_chart_query_rows")]
    limit: usize,
}

fn default_chart_query_rows() -> usize {
    DEFAULT_CHART_QUERY_ROWS
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ChartPublishMode {
    Create,
    Revise,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChartPublishRequest {
    mode: ChartPublishMode,
    title: String,
    #[serde(default)]
    dataset: Option<crate::chart_engine::ChartDatasetInput>,
    #[serde(default)]
    view: Option<crate::chart_engine::ChartView>,
    #[serde(default)]
    studies: Option<Vec<crate::chart_engine::ChartStudy>>,
    #[serde(default)]
    drawings: Option<Vec<crate::chart_engine::ChartDrawing>>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    artifact_id: Option<String>,
    #[serde(default)]
    edit_token: Option<String>,
}

#[derive(Default)]
pub(super) struct ToolCapture {
    pub(super) memories: Mutex<HashMap<String, MemoryRefSnapshot>>,
    pub(super) web_sources: Mutex<HashMap<String, crate::web::WebSource>>,
    web_fetch_snapshots: Mutex<HashMap<String, RunWebFetchSnapshot>>,
    pub(crate) run_results: Mutex<RunResultRegistry>,
    pub(super) staged_evidence: Mutex<Vec<StagedEvidence>>,
    pub(super) decision: Mutex<Option<ChatDecision>>,
    pub(super) proposal: Mutex<Vec<MemoryProposal>>,
    pub(super) artifacts: Mutex<Vec<ArtifactCommit>>,
    artifact_reads: Mutex<BTreeSet<(String, u32)>>,
    source_message_id: Option<String>,
    pending_deliveries: Mutex<HashMap<String, PendingToolCapture>>,
    pub(super) compute: Arc<crate::compute::TurnComputeSession>,
    pub(super) web_search_policy: crate::web::WebSearchPolicy,
    pub(super) search_cancel: crate::web::SearchCancel,
    mcp_sessions: Mutex<BTreeMap<String, TurnMcpServer>>,
    pub(super) mcp_scratch_root: Option<PathBuf>,
    pub(super) mcp_pool: Option<McpProcessPool>,
}

#[derive(Default)]
struct PendingToolCapture {
    memories: HashMap<String, MemoryRefSnapshot>,
    web_sources: HashMap<String, crate::web::WebSource>,
    web_fetch_snapshots: HashMap<String, RunWebFetchSnapshot>,
    run_results: BTreeMap<String, RunResult>,
    staged_evidence: Vec<StagedEvidence>,
    decision: Option<ChatDecision>,
    proposal: Vec<MemoryProposal>,
    artifacts: Vec<ArtifactCommit>,
    artifact_reads: BTreeSet<(String, u32)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunResultProducer {
    pub(crate) runtime_id: String,
    pub(crate) tool_name: String,
    pub(crate) provider: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RunResult {
    pub(crate) result_ref: String,
    pub(crate) producer: RunResultProducer,
    pub(crate) request_digest: String,
    pub(crate) response_digest: String,
    pub(crate) retrieved_at: String,
    pub(crate) payload: Value,
    pub(crate) warnings: Vec<String>,
    pub(crate) upstream_result_refs: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunResultReceipt {
    pub(crate) result_ref: String,
    pub(crate) producer: RunResultProducer,
    pub(crate) request_digest: String,
    pub(crate) response_digest: String,
    pub(crate) retrieved_at: String,
    pub(crate) warnings: Vec<String>,
    pub(crate) upstream_result_refs: Vec<String>,
}

impl RunResult {
    pub(crate) fn receipt(&self) -> RunResultReceipt {
        RunResultReceipt {
            result_ref: self.result_ref.clone(),
            producer: self.producer.clone(),
            request_digest: self.request_digest.clone(),
            response_digest: self.response_digest.clone(),
            retrieved_at: self.retrieved_at.clone(),
            warnings: self.warnings.clone(),
            upstream_result_refs: self.upstream_result_refs.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RunResultRegistry {
    results: BTreeMap<String, RunResult>,
    bytes: usize,
}

impl RunResultRegistry {
    pub(crate) fn get(&self, result_ref: &str) -> Option<&RunResult> {
        self.results.get(result_ref)
    }

    fn insert(&mut self, result: RunResult) -> Result<(), BrokerError> {
        let bytes = run_result_bytes(&result)?;
        if bytes > MAX_RUN_RESULT_BYTES
            || self.results.len() >= MAX_RUN_RESULTS
            || self
                .bytes
                .checked_add(bytes)
                .is_none_or(|total| total > MAX_RUN_RESULT_REGISTRY_BYTES)
            || self.results.contains_key(&result.result_ref)
        {
            return Err(BrokerError::Execution(
                "run result registry capacity was exceeded".into(),
            ));
        }
        self.bytes += bytes;
        self.results.insert(result.result_ref.clone(), result);
        Ok(())
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &RunResult> {
        self.results.values()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct EvidenceCitation {
    pub(super) result_ref: String,
    pub(super) pointer: String,
    pub(super) excerpt: Option<String>,
    pub(super) selected: Value,
    pub(super) receipt: RunResultReceipt,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct EvidenceClaim {
    pub(super) text: String,
    pub(super) citations: Vec<EvidenceCitation>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct StagedEvidence {
    pub(super) evidence_id: String,
    pub(super) title: String,
    pub(super) summary: String,
    pub(super) as_of: String,
    pub(super) claims: Vec<EvidenceClaim>,
}

#[derive(Clone, Debug)]
struct RunWebFetchSnapshot {
    fetched: crate::web::WebFetchSnapshot,
    issued_offsets: BTreeSet<usize>,
}

impl ToolCapture {
    pub(super) fn for_chat(source_message_id: String) -> Self {
        Self {
            source_message_id: Some(source_message_id),
            ..Self::default()
        }
    }

    pub(crate) async fn run_result(&self, result_ref: &str) -> Option<RunResult> {
        self.run_results.lock().await.get(result_ref).cloned()
    }

    pub(crate) async fn run_result_selection(
        &self,
        result_ref: &str,
        pointer: &str,
    ) -> Option<(RunResultReceipt, Option<Value>)> {
        let registry = self.run_results.lock().await;
        let result = registry.get(result_ref)?;
        Some((result.receipt(), result.payload.pointer(pointer).cloned()))
    }

    pub(crate) async fn stage_run_result(
        &self,
        delivery_id: &str,
        producer: RunResultProducer,
        request: &Value,
        payload: Value,
        upstream_result_refs: Vec<String>,
    ) -> Result<String, BrokerError> {
        let request_bytes = serde_json::to_vec(request).map_err(|_| BrokerError::Malformed)?;
        let response_bytes = serde_json::to_vec(&payload)
            .map_err(|_| BrokerError::Execution("tool result could not be captured".into()))?;
        if response_bytes.len() > MAX_RUN_RESULT_BYTES {
            return Err(BrokerError::Execution(
                "tool result exceeded the run result capture limit".into(),
            ));
        }
        let mut unique_upstream = BTreeSet::new();
        for result_ref in &upstream_result_refs {
            if result_ref.is_empty()
                || result_ref.len() > 128
                || !unique_upstream.insert(result_ref.as_str())
                || self.run_result(result_ref).await.is_none()
            {
                return Err(BrokerError::Execution(
                    "upstream_result_refs must name unique delivered results from this turn".into(),
                ));
            }
        }
        let result_ref = format!("result:{}", uuid::Uuid::new_v4().simple());
        let warnings = result_warnings(&payload);
        let result = RunResult {
            result_ref: result_ref.clone(),
            producer,
            request_digest: sha256(&request_bytes),
            response_digest: sha256(&response_bytes),
            retrieved_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            payload,
            warnings,
            upstream_result_refs,
        };
        let result_bytes = run_result_bytes(&result)?;
        if result_bytes > MAX_RUN_RESULT_BYTES {
            return Err(BrokerError::Execution(
                "tool result exceeded the run result capture limit".into(),
            ));
        }
        let committed = self.run_results.lock().await;
        let mut pending = self.pending_deliveries.lock().await;
        let pending_count = pending
            .values()
            .map(|capture| capture.run_results.len())
            .sum::<usize>();
        let pending_bytes = pending
            .values()
            .flat_map(|capture| capture.run_results.values())
            .try_fold(0_usize, |total, result| {
                total.checked_add(run_result_bytes(result).ok()?)
            })
            .ok_or_else(|| {
                BrokerError::Execution("run result registry capacity was exceeded".into())
            })?;
        if committed.results.len() + pending_count >= MAX_RUN_RESULTS
            || committed
                .bytes
                .checked_add(pending_bytes)
                .and_then(|bytes| bytes.checked_add(result_bytes))
                .is_none_or(|bytes| bytes > MAX_RUN_RESULT_REGISTRY_BYTES)
        {
            return Err(BrokerError::Execution(
                "run result registry capacity was exceeded".into(),
            ));
        }
        pending
            .entry(delivery_id.to_owned())
            .or_default()
            .run_results
            .insert(result_ref.clone(), result);
        drop(pending);
        drop(committed);
        Ok(result_ref)
    }

    async fn stage<F>(&self, delivery_id: &str, update: F)
    where
        F: FnOnce(&mut PendingToolCapture),
    {
        let mut pending = self.pending_deliveries.lock().await;
        update(pending.entry(delivery_id.to_owned()).or_default());
    }

    async fn discard_delivery(&self, delivery_id: &str) {
        self.pending_deliveries.lock().await.remove(delivery_id);
    }

    async fn commit_delivery(&self, delivery_id: &str) {
        // Keep the reservation transfer atomic with staging: both paths acquire
        // the registry before pending deliveries, so no concurrent call can
        // reserve the slot between removal and committed insertion.
        let mut registry = self.run_results.lock().await;
        let mut pending_deliveries = self.pending_deliveries.lock().await;
        let Some(mut pending) = pending_deliveries.remove(delivery_id) else {
            return;
        };
        for (_, result) in std::mem::take(&mut pending.run_results) {
            registry
                .insert(result)
                .expect("staged run result budget was checked before delivery");
        }
        drop(pending_deliveries);
        drop(registry);

        if !pending.memories.is_empty() {
            let mut memories = self.memories.lock().await;
            for (record_id, memory) in pending.memories {
                let existing_rank = memories
                    .get(&record_id)
                    .map(memory_authority_rank)
                    .unwrap_or_default();
                if existing_rank <= memory_authority_rank(&memory) {
                    memories.insert(record_id, memory);
                }
            }
        }
        if !pending.web_sources.is_empty() {
            self.web_sources.lock().await.extend(pending.web_sources);
        }
        if !pending.web_fetch_snapshots.is_empty() {
            self.web_fetch_snapshots
                .lock()
                .await
                .extend(pending.web_fetch_snapshots);
        }
        if !pending.staged_evidence.is_empty() {
            self.staged_evidence
                .lock()
                .await
                .extend(pending.staged_evidence);
        }
        if let Some(decision) = pending.decision {
            *self.decision.lock().await = Some(decision);
        }
        if !pending.proposal.is_empty() {
            let mut proposals = self.proposal.lock().await;
            for proposal in pending.proposal {
                if let Some(existing) = proposals
                    .iter_mut()
                    .find(|existing| existing.target_record_id == proposal.target_record_id)
                {
                    *existing = proposal;
                } else {
                    proposals.push(proposal);
                }
            }
        }
        if !pending.artifacts.is_empty() {
            self.artifacts
                .lock()
                .await
                .extend(std::mem::take(&mut pending.artifacts));
        }
        self.artifact_reads
            .lock()
            .await
            .extend(pending.artifact_reads);
    }
}

fn memory_authority_rank(memory: &MemoryRefSnapshot) -> u8 {
    match (memory.access, memory.section.as_ref()) {
        (MemoryAccess::SearchDiscovered, _) => 0,
        (MemoryAccess::ExactRead, Some(_)) => 1,
        (MemoryAccess::ExactRead, None) => 2,
    }
}

fn run_result_bytes(result: &RunResult) -> Result<usize, BrokerError> {
    serde_json::to_vec(&result.payload)
        .map(|payload| {
            payload.len()
                + result.result_ref.len()
                + result.producer.runtime_id.len()
                + result.producer.tool_name.len()
                + result.producer.provider.as_deref().map_or(0, str::len)
                + result.request_digest.len()
                + result.response_digest.len()
                + result.retrieved_at.len()
                + result.warnings.iter().map(String::len).sum::<usize>()
                + result
                    .upstream_result_refs
                    .iter()
                    .map(String::len)
                    .sum::<usize>()
        })
        .map_err(|_| BrokerError::Execution("tool result could not be captured".into()))
}

fn result_warnings(payload: &Value) -> Vec<String> {
    let mut warnings = Vec::new();
    for pointer in [
        "/warnings",
        "/quality_warnings",
        "/structuredContent/warnings",
    ] {
        warnings.extend(
            payload
                .pointer(pointer)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(bounded_warning_text),
        );
    }
    if let Some(warning) = payload
        .get("quality")
        .and_then(|quality| quality.get("warning"))
        .and_then(Value::as_str)
        .filter(|warning| !warning.is_empty() && warning.len() <= 2_048)
    {
        warnings.push(warning.to_owned());
    }
    warnings
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(16)
        .collect()
}

fn bounded_warning_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(message) => message.clone(),
        Value::Object(object) => {
            let message = object.get("message")?.as_str()?;
            match object.get("category").and_then(Value::as_str) {
                Some(category) if !category.is_empty() => format!("{category}: {message}"),
                _ => message.to_owned(),
            }
        }
        _ => return None,
    };
    (!text.is_empty() && text.len() <= 2_048 && !text.contains('\0')).then_some(text)
}

#[derive(Clone)]
pub(super) struct AppToolExecutor {
    pub(super) state: AppState,
    pub(super) capture: Arc<ToolCapture>,
    pub(super) guru_id: String,
    pub(super) guru_root: BoundGuruRoot,
    pub(super) capability_ids: BTreeSet<String>,
    pub(super) chat_provider: String,
}

#[async_trait]
impl ToolExecutor for AppToolExecutor {
    async fn execute(
        &self,
        policy: &ToolPolicy,
        method: ToolMethod,
        params: Value,
    ) -> Result<Value, BrokerError> {
        let delivery_id = new_id("direct-delivery");
        let result = self
            .execute_staged(policy, method, params, &delivery_id)
            .await;
        if result.is_ok() {
            self.capture.commit_delivery(&delivery_id).await;
        } else {
            self.capture.discard_delivery(&delivery_id).await;
        }
        result
    }

    async fn execute_for_delivery(
        &self,
        policy: &ToolPolicy,
        method: ToolMethod,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        self.execute_staged(policy, method, params, delivery_id)
            .await
    }

    async fn commit_delivery(&self, _policy: &ToolPolicy, delivery_id: &str) {
        self.capture.commit_delivery(delivery_id).await;
    }

    async fn discard_delivery(&self, _policy: &ToolPolicy, delivery_id: &str) {
        self.capture.discard_delivery(delivery_id).await;
    }
}

impl AppToolExecutor {
    async fn execute_staged(
        &self,
        policy: &ToolPolicy,
        method: ToolMethod,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        self.state
            .ensure_guru_available(&self.guru_id)
            .map_err(|_| BrokerError::Execution("Guru is unavailable".into()))?;
        self.ensure_scope(policy)?;
        let request = params.clone();
        let mut result = match method {
            ToolMethod::GuruSearch => self.guru_search(policy, params, delivery_id).await,
            ToolMethod::GuruRead => self.guru_read(policy, params, delivery_id).await,
            ToolMethod::GuruReadPrevious => self.guru_read_previous(policy, params).await,
            ToolMethod::FinanceSources => self.finance_sources(params),
            ToolMethod::FinanceMacroData => {
                self.finance_macro_data(policy, params, delivery_id).await
            }
            ToolMethod::FinanceMarketData => {
                self.finance_market_data(policy, params, delivery_id).await
            }
            ToolMethod::FinanceCompanyData => {
                self.finance_company_data(policy, params, delivery_id).await
            }
            ToolMethod::FinanceFilings => self.finance_filings(policy, params, delivery_id).await,
            ToolMethod::FinanceCalculate => {
                self.finance_calculate(policy, params, delivery_id).await
            }
            ToolMethod::FinanceResolveEntity => self.finance_resolve_entity(params).await,
            ToolMethod::McpConnect => self.mcp_connect(params).await,
            ToolMethod::McpCall => self.mcp_call(params, delivery_id).await,
            ToolMethod::RunResultsList => self.run_results_list(params).await,
            ToolMethod::ComputeRun => self.compute_run(policy, params, delivery_id).await,
            ToolMethod::WebSearch => self.web_search(policy, params, delivery_id).await,
            ToolMethod::WebFetch => self.web_fetch(policy, params, delivery_id).await,
            ToolMethod::DecisionSubmit => self.stage_decision(params, delivery_id).await,
            ToolMethod::EvidenceCreate => self.stage_evidence_create(params, delivery_id).await,
            ToolMethod::MemoryPatchPropose => self.stage_proposal(params, delivery_id).await,
            ToolMethod::ArtifactList => self.artifact_list(policy, params),
            ToolMethod::ArtifactRead => self.artifact_read(policy, params, delivery_id).await,
            ToolMethod::ArtifactPublish => self.artifact_publish(policy, params, delivery_id).await,
            ToolMethod::ChartQuery => self.chart_query(policy, params).await,
            ToolMethod::ChartPublish => self.chart_publish(policy, params, delivery_id).await,
            ToolMethod::WorkbenchRead => self.workbench_read(params),
            ToolMethod::WorkbenchWrite => self.workbench_write(params),
            ToolMethod::WorkbenchEdit => self.workbench_edit(params),
            ToolMethod::WorkbenchList => self.workbench_list(params),
            ToolMethod::WorkbenchFind => self.workbench_find(params),
            ToolMethod::WorkbenchGrep => self.workbench_grep(params),
        }?;
        if let Some(producer) = run_result_producer(method, &request, &result) {
            let result_ref = self
                .capture
                .stage_run_result(delivery_id, producer, &request, result.clone(), Vec::new())
                .await?;
            attach_result_ref(&mut result, &result_ref);
        }
        Ok(result)
    }
}

fn run_result_producer(
    method: ToolMethod,
    request: &Value,
    result: &Value,
) -> Option<RunResultProducer> {
    let (runtime_id, tool_name) = match method {
        ToolMethod::GuruSearch => ("memory", "memory_search"),
        ToolMethod::GuruRead => ("memory", "memory_read"),
        ToolMethod::GuruReadPrevious => ("memory", "memory_previous"),
        ToolMethod::FinanceMacroData => ("native-finance", "finance_macro_data"),
        ToolMethod::FinanceMarketData => ("native-finance", "finance_market_data"),
        ToolMethod::FinanceCompanyData => ("native-finance", "finance_company_data"),
        ToolMethod::FinanceFilings => ("native-finance", "finance_filings"),
        ToolMethod::FinanceCalculate => ("finance-worker", "finance_calculate"),
        ToolMethod::FinanceResolveEntity => ("native-finance", "finance_resolve_entity"),
        ToolMethod::ComputeRun => ("compute", "compute_run"),
        ToolMethod::WebSearch => ("native-web", "web_search"),
        ToolMethod::WebFetch => ("native-web", "web_fetch"),
        ToolMethod::ArtifactList => ("artifact-store", "artifact_list"),
        ToolMethod::ArtifactRead => ("artifact-store", "artifact_read"),
        ToolMethod::ChartQuery => ("artifact-store", "chart_query"),
        ToolMethod::WorkbenchRead => ("workbench", "workbench_read"),
        ToolMethod::WorkbenchList => ("workbench", "ls"),
        ToolMethod::WorkbenchFind => ("workbench", "find"),
        ToolMethod::WorkbenchGrep => ("workbench", "grep"),
        _ => return None,
    };
    let provider = match method {
        ToolMethod::WebSearch => result
            .get("selected_provider")
            .and_then(Value::as_str)
            .map(str::to_owned),
        ToolMethod::WebFetch => result
            .get("representation_provider")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                result
                    .get("final_url")
                    .or_else(|| result.get("url"))
                    .and_then(Value::as_str)
                    .and_then(|url| reqwest::Url::parse(url).ok())
                    .and_then(|url| url.host_str().map(str::to_owned))
            }),
        _ => result
            .get("source_id")
            .or_else(|| result.get("provider"))
            .or_else(|| request.get("provider"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
    .filter(|value| !value.is_empty() && value.len() <= 256);
    Some(RunResultProducer {
        runtime_id: runtime_id.into(),
        tool_name: tool_name.into(),
        provider,
    })
}

fn attach_result_ref(result: &mut Value, result_ref: &str) {
    if let Some(object) = result.as_object_mut() {
        object.insert("result_ref".into(), Value::String(result_ref.to_owned()));
    } else {
        let payload = std::mem::take(result);
        *result = json!({ "result_ref": result_ref, "data": payload });
    }
}

#[cfg(test)]
mod tests;
