use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    io,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter},
    process::{Child, ChildStdin, Command},
    sync::{broadcast, oneshot, Mutex},
    task::JoinHandle,
    time::{sleep, timeout, Instant},
};

#[cfg(windows)]
use crate::process_lease::ChildProcessJob;
#[cfg(unix)]
use crate::process_lease::{
    signal_process_group, terminate_and_reap_process_group, wait_for_process_group_exit,
    ChildProcessLease, ProcessKind,
};
use crate::{artifact_trust::verify_executable, process_lease::ProcessLeaseError};
use uuid::Uuid;

const PROTOCOL_VERSION: &str = "1";
pub(crate) const WORKER_VERSION: &str = "1.0.0";
pub(crate) const WORKER_LOCK_SHA256: &str =
    "172ddf32098550f75ccd271220268694dc52766d60e9b7deb8ab88310e6605bf";
const MAX_FRAME_BYTES: usize = 1024 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum FinanceError {
    #[error("finance worker executable is missing")]
    MissingExecutable,
    #[error("finance worker is not a trusted app artifact")]
    UntrustedExecutable,
    #[error("finance worker I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("finance worker protocol failed")]
    Protocol,
    #[error("finance worker protocol version is unsupported")]
    VersionMismatch,
    #[error("finance worker identity is not trusted: {0}")]
    IntegrityMismatch(&'static str),
    #[error("finance worker request timed out")]
    Timeout,
    #[error("finance worker rejected the request: {0}")]
    Remote(String),
    #[error("finance worker stopped")]
    Stopped,
    #[error("finance worker frame exceeded the size limit")]
    FrameTooLarge,
    #[error("finance worker process ownership failed: {0}")]
    Lease(#[from] ProcessLeaseError),
}

#[derive(Clone, Debug)]
pub struct FinanceLaunchConfig {
    pub executable: PathBuf,
    pub private_working_dir: PathBuf,
    pub artifact_dir: PathBuf,
    pub lease_dir: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FinanceHandshake {
    pub protocol_version: String,
    pub worker_version: String,
    pub python_version: String,
    pub lock_digest: String,
    pub tools: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FinanceProgress {
    pub id: String,
    pub stage: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct FinanceToolCall {
    pub name: String,
    pub arguments: Value,
    pub context: Value,
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, FinanceError>>>>>;

pub struct FinanceWorker {
    child: Mutex<Child>,
    #[cfg(unix)]
    process_group_id: i32,
    #[cfg(unix)]
    lease: Option<ChildProcessLease>,
    #[cfg(windows)]
    job: Option<ChildProcessJob>,
    stdin: Mutex<BufWriter<ChildStdin>>,
    pending: Pending,
    progress: broadcast::Sender<FinanceProgress>,
    reader: JoinHandle<()>,
}

impl FinanceWorker {
    pub async fn spawn(
        config: FinanceLaunchConfig,
    ) -> Result<(Self, FinanceHandshake), FinanceError> {
        let _verified_executable =
            verify_executable(&config.executable).map_err(|_| FinanceError::UntrustedExecutable)?;
        std::fs::create_dir_all(&config.private_working_dir)?;
        std::fs::create_dir_all(&config.artifact_dir)?;
        let mut command = Command::new(&config.executable);
        command
            .current_dir(&config.private_working_dir)
            .env_clear()
            .env("GURUTERMINAL_ARTIFACT_DIR", &config.artifact_dir)
            .env("LANG", "C.UTF-8")
            .env("PYTHONHASHSEED", "0")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PYTHONNOUSERSITE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        ChildProcessJob::configure_command(&mut command);
        let mut child = command.spawn()?;
        #[cfg(unix)]
        let process_group_id = child.id().ok_or(FinanceError::Protocol)? as i32;
        #[cfg(windows)]
        let job = match ChildProcessJob::assign(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.start_kill();
                let _ = timeout(Duration::from_secs(2), child.wait()).await;
                return Err(error.into());
            }
        };
        let stdin = child.stdin.take().ok_or(FinanceError::Protocol)?;
        let stdout = child.stdout.take().ok_or(FinanceError::Protocol)?;
        #[cfg(unix)]
        let lease = match ChildProcessLease::register(
            &config.lease_dir,
            ProcessKind::Finance,
            process_group_id,
            process_group_id,
            &config.executable,
        ) {
            Ok(lease) => lease,
            Err(error) => {
                let _ = signal_process_group(process_group_id, libc::SIGKILL);
                let _ = timeout(Duration::from_secs(2), child.wait()).await;
                return Err(error.into());
            }
        };
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let reader_pending = pending.clone();
        let (progress, _) = broadcast::channel(32);
        let progress_writer = progress.clone();
        let reader = tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            loop {
                match read_frame(&mut stdout).await {
                    Ok(Some(frame)) => {
                        if dispatch_frame(&frame, &reader_pending, &progress_writer)
                            .await
                            .is_err()
                        {
                            fail_pending(&reader_pending, "malformed worker response").await;
                            break;
                        }
                    }
                    Ok(None) => {
                        fail_pending(&reader_pending, "worker stopped").await;
                        break;
                    }
                    Err(_) => {
                        fail_pending(&reader_pending, "worker protocol failed").await;
                        break;
                    }
                }
            }
        });
        let worker = Self {
            child: Mutex::new(child),
            #[cfg(unix)]
            process_group_id,
            #[cfg(unix)]
            lease: Some(lease),
            #[cfg(windows)]
            job: Some(job),
            stdin: Mutex::new(BufWriter::new(stdin)),
            pending,
            progress,
            reader,
        };
        let handshake = match worker
            .call("system.handshake", json!({}), STARTUP_TIMEOUT)
            .await
            .and_then(|value| {
                let handshake =
                    serde_json::from_value(value).map_err(|_| FinanceError::Protocol)?;
                validate_handshake(&handshake)?;
                Ok(handshake)
            }) {
            Ok(handshake) => handshake,
            Err(error) => {
                eprintln!("Guru Terminal finance worker failed handshake: {error}");
                let _ = worker.shutdown(Duration::from_secs(1)).await;
                return Err(error);
            }
        };
        Ok((worker, handshake))
    }

    pub fn subscribe_progress(&self) -> broadcast::Receiver<FinanceProgress> {
        self.progress.subscribe()
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        context: Value,
        deadline: Duration,
    ) -> Result<Value, FinanceError> {
        if !is_allowed_tool(name) || !arguments.is_object() || !context.is_object() {
            return Err(FinanceError::Protocol);
        }
        self.call(
            "tools.call",
            json!({ "name": name, "arguments": arguments, "context": context }),
            deadline,
        )
        .await
    }

    pub async fn call_tools_ordered(
        &self,
        calls: Vec<FinanceToolCall>,
        deadline: Duration,
    ) -> Vec<Result<Value, FinanceError>> {
        let started = Instant::now();
        let mut results = Vec::with_capacity(calls.len());
        for call in calls {
            let remaining = deadline.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                results.push(Err(FinanceError::Timeout));
                continue;
            }
            results.push(
                self.call_tool(&call.name, call.arguments, call.context, remaining)
                    .await,
            );
        }
        results
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, FinanceError> {
        let started = Instant::now();
        if !is_allowed_method(method) {
            return Err(FinanceError::Protocol);
        }
        let id = Uuid::new_v4().simple().to_string();
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut encoded = serde_json::to_vec(&request).map_err(|_| FinanceError::Protocol)?;
        if encoded.len() > MAX_FRAME_BYTES {
            return Err(FinanceError::FrameTooLarge);
        }
        encoded.push(b'\n');
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), sender);
        let write_result =
            write_worker_bytes(&self.stdin, &encoded, deadline.min(WRITE_TIMEOUT)).await;
        if let Err(error) = write_result {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        let remaining = deadline.saturating_sub(started.elapsed());
        match timeout(remaining, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(FinanceError::Stopped),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                let _ = self.send_cancel(&id).await;
                Err(FinanceError::Timeout)
            }
        }
    }

    async fn send_cancel(&self, request_id: &str) -> Result<(), FinanceError> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": Uuid::new_v4().simple().to_string(),
            "method": "system.cancel",
            "params": { "request_id": request_id },
        });
        let mut encoded = serde_json::to_vec(&request).map_err(|_| FinanceError::Protocol)?;
        encoded.push(b'\n');
        write_worker_bytes(&self.stdin, &encoded, WRITE_TIMEOUT).await
    }

    pub async fn shutdown(mut self, grace: Duration) -> Result<(), FinanceError> {
        let _ = self
            .call("system.shutdown", json!({}), Duration::from_secs(2))
            .await;
        let deadline = Instant::now() + grace;
        let mut child = self.child.lock().await;
        let mut child_exited = false;
        let mut observation_error = None;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => {
                    child_exited = true;
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    observation_error = Some(FinanceError::Io(error));
                    break;
                }
            }
            sleep(Duration::from_millis(25)).await;
        }
        let graceful: Result<(), FinanceError> = if let Some(error) = observation_error {
            Err(error)
        } else {
            async {
                #[cfg(unix)]
                {
                    signal_process_group(self.process_group_id, libc::SIGTERM)?;
                    if !child_exited {
                        timeout(Duration::from_secs(2), child.wait())
                            .await
                            .map_err(|_| FinanceError::Timeout)??;
                    }
                    timeout(
                        Duration::from_secs(2),
                        wait_for_process_group_exit(self.process_group_id),
                    )
                    .await
                    .map_err(|_| FinanceError::Timeout)??;
                }
                #[cfg(windows)]
                {
                    if let Some(job) = &self.job {
                        job.terminate_and_wait(Duration::from_secs(2)).await?;
                    }
                    if !child_exited {
                        timeout(Duration::from_secs(2), child.wait())
                            .await
                            .map_err(|_| FinanceError::Timeout)??;
                    }
                }
                #[cfg(not(any(unix, windows)))]
                if !child_exited {
                    child.start_kill()?;
                    timeout(Duration::from_secs(2), child.wait())
                        .await
                        .map_err(|_| FinanceError::Timeout)??;
                }
                Ok(())
            }
            .await
        };
        let stop_result = if graceful.is_ok() {
            graceful
        } else {
            async {
                #[cfg(unix)]
                {
                    signal_process_group(self.process_group_id, libc::SIGKILL)?;
                    // Process-group disappearance is authoritative. Even if
                    // leader observation failed above, never bypass the forced
                    // group stop and durable lease cleanup path.
                    let _ = timeout(Duration::from_secs(2), child.wait()).await;
                    timeout(
                        Duration::from_secs(2),
                        wait_for_process_group_exit(self.process_group_id),
                    )
                    .await
                    .map_err(|_| FinanceError::Timeout)??;
                }
                #[cfg(windows)]
                {
                    if let Some(job) = &self.job {
                        job.terminate_and_wait(Duration::from_secs(2)).await?;
                    } else if child.try_wait()?.is_none() {
                        child.start_kill()?;
                    }
                    if child.try_wait()?.is_none() {
                        timeout(Duration::from_secs(2), child.wait())
                            .await
                            .map_err(|_| FinanceError::Timeout)??;
                    }
                }
                #[cfg(not(any(unix, windows)))]
                {
                    if child.try_wait()?.is_none() {
                        child.start_kill()?;
                        timeout(Duration::from_secs(2), child.wait())
                            .await
                            .map_err(|_| FinanceError::Timeout)??;
                    }
                }
                Ok(())
            }
            .await
        };
        drop(child);
        self.reader.abort();
        let _ = timeout(Duration::from_secs(2), &mut self.reader).await;
        #[cfg(windows)]
        self.job.take();
        stop_result?;
        #[cfg(unix)]
        if let Some(lease) = self.lease.take() {
            lease.complete()?;
        }
        Ok(())
    }
}

async fn write_worker_bytes<W>(
    writer: &Mutex<W>,
    bytes: &[u8],
    deadline: Duration,
) -> Result<(), FinanceError>
where
    W: AsyncWrite + Unpin,
{
    timeout(deadline, async {
        let mut writer = writer.lock().await;
        writer.write_all(bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| FinanceError::Timeout)??;
    Ok(())
}

impl Drop for FinanceWorker {
    fn drop(&mut self) {
        self.reader.abort();
        #[cfg(unix)]
        {
            if let Some(lease) = self.lease.take() {
                terminate_and_reap_process_group(self.process_group_id, lease);
            } else {
                let _ = signal_process_group(self.process_group_id, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            let _ = job.terminate();
        }
    }
}

fn is_allowed_method(method: &str) -> bool {
    matches!(
        method,
        "system.handshake" | "system.cancel" | "system.shutdown" | "tools.list" | "tools.call"
    )
}

fn validate_handshake(handshake: &FinanceHandshake) -> Result<(), FinanceError> {
    if handshake.protocol_version != PROTOCOL_VERSION {
        return Err(FinanceError::VersionMismatch);
    }
    let expected_tools = BTreeSet::from([
        "compound_annual_growth_rate",
        "currency_convert",
        "dcf_sensitivity",
        "discounted_cash_flow",
        "enterprise_value_bridge",
        "internal_rate_of_return",
        "percentage_change",
        "period_aggregate",
        "point_in_time_filter",
        "ratio",
        "risk_metrics",
        "series_statistics",
        "weighted_average_cost_of_capital",
    ]);
    let actual_tools = handshake
        .tools
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if handshake.worker_version != WORKER_VERSION {
        return Err(FinanceError::IntegrityMismatch("worker_version"));
    }
    if !handshake.python_version.starts_with("3.12.") {
        return Err(FinanceError::IntegrityMismatch("python_version"));
    }
    if handshake.lock_digest != WORKER_LOCK_SHA256 {
        return Err(FinanceError::IntegrityMismatch("lock_digest"));
    }
    if actual_tools != expected_tools || handshake.tools.len() != expected_tools.len() {
        return Err(FinanceError::IntegrityMismatch("tool_set"));
    }
    Ok(())
}

fn is_allowed_tool(name: &str) -> bool {
    matches!(
        name,
        "compound_annual_growth_rate"
            | "currency_convert"
            | "dcf_sensitivity"
            | "discounted_cash_flow"
            | "enterprise_value_bridge"
            | "internal_rate_of_return"
            | "percentage_change"
            | "period_aggregate"
            | "point_in_time_filter"
            | "ratio"
            | "risk_metrics"
            | "series_statistics"
            | "weighted_average_cost_of_capital"
    )
}

async fn dispatch_frame(
    frame: &[u8],
    pending: &Pending,
    progress: &broadcast::Sender<FinanceProgress>,
) -> Result<(), FinanceError> {
    let value: Value = serde_json::from_slice(frame).map_err(|_| FinanceError::Protocol)?;
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(FinanceError::Protocol);
    }
    if value.get("method").and_then(Value::as_str) == Some("progress") {
        let event: FinanceProgress =
            serde_json::from_value(value.get("params").cloned().ok_or(FinanceError::Protocol)?)
                .map_err(|_| FinanceError::Protocol)?;
        let _ = progress.send(event);
        return Ok(());
    }
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or(FinanceError::Protocol)?
        .to_owned();
    let sender = pending.lock().await.remove(&id);
    if let Some(sender) = sender {
        let result = if let Some(result) = value.get("result") {
            Ok(result.clone())
        } else if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("worker rejected the request")
                .chars()
                .take(300)
                .collect();
            Err(FinanceError::Remote(message))
        } else {
            Err(FinanceError::Protocol)
        };
        let _ = sender.send(result);
    }
    Ok(())
}

async fn fail_pending(pending: &Pending, message: &str) {
    let entries = pending.lock().await.drain().collect::<Vec<_>>();
    for (_, sender) in entries {
        let _ = sender.send(Err(FinanceError::Remote(message.into())));
    }
}

async fn read_frame<R>(reader: &mut R) -> Result<Option<Vec<u8>>, FinanceError>
where
    R: AsyncBufRead + Unpin,
{
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(FinanceError::Protocol)
            };
        }
        let (take, complete) = match available.iter().position(|byte| *byte == b'\n') {
            Some(position) => (position, true),
            None => (available.len(), false),
        };
        if frame.len() + take > MAX_FRAME_BYTES {
            return Err(FinanceError::FrameTooLarge);
        }
        frame.extend_from_slice(&available[..take]);
        reader.consume(take + usize::from(complete));
        if complete {
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            return if frame.is_empty() {
                Err(FinanceError::Protocol)
            } else {
                Ok(Some(frame))
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use tokio::io::duplex;

    #[test]
    fn arbitrary_python_is_not_an_allowed_tool() {
        assert!(is_allowed_method("tools.call"));
        assert!(is_allowed_tool("ratio"));
        assert!(is_allowed_tool("discounted_cash_flow"));
        assert!(!is_allowed_tool("python.eval"));
        assert!(!is_allowed_tool("pip.install"));
        assert!(!is_allowed_tool("script.run"));
    }

    #[test]
    fn handshake_is_bound_to_the_worker_version_lock_and_exact_tools() {
        let trusted = FinanceHandshake {
            protocol_version: PROTOCOL_VERSION.into(),
            worker_version: WORKER_VERSION.into(),
            python_version: "3.12.11".into(),
            lock_digest: WORKER_LOCK_SHA256.into(),
            tools: vec![
                "compound_annual_growth_rate".into(),
                "currency_convert".into(),
                "dcf_sensitivity".into(),
                "discounted_cash_flow".into(),
                "enterprise_value_bridge".into(),
                "internal_rate_of_return".into(),
                "percentage_change".into(),
                "period_aggregate".into(),
                "point_in_time_filter".into(),
                "ratio".into(),
                "risk_metrics".into(),
                "series_statistics".into(),
                "weighted_average_cost_of_capital".into(),
            ],
        };
        assert!(validate_handshake(&trusted).is_ok());
        let mut tampered = trusted.clone();
        tampered.lock_digest = "0".repeat(64);
        assert!(matches!(
            validate_handshake(&tampered),
            Err(FinanceError::IntegrityMismatch("lock_digest"))
        ));
        let mut wrong_worker = trusted.clone();
        wrong_worker.worker_version = "1.0.1".into();
        assert!(matches!(
            validate_handshake(&wrong_worker),
            Err(FinanceError::IntegrityMismatch("worker_version"))
        ));
        let mut wrong_python = trusted.clone();
        wrong_python.python_version = "3.13.0".into();
        assert!(matches!(
            validate_handshake(&wrong_python),
            Err(FinanceError::IntegrityMismatch("python_version"))
        ));
        let mut extra_tool = trusted;
        extra_tool.tools.push("python.eval".into());
        assert!(validate_handshake(&extra_tool).is_err());
    }

    #[test]
    fn worker_identity_matches_the_packaged_python_source() {
        let pyproject = include_str!("../../python/pyproject.toml");
        let project_version = pyproject
            .lines()
            .skip_while(|line| line.trim() != "[project]")
            .skip(1)
            .find_map(|line| {
                line.trim()
                    .strip_prefix("version = \"")
                    .and_then(|value| value.strip_suffix('"'))
            })
            .expect("Python project version");
        assert_eq!(project_version, WORKER_VERSION);

        let lock_digest = hex::encode(Sha256::digest(include_bytes!("../../python/uv.lock")));
        assert_eq!(lock_digest, WORKER_LOCK_SHA256);

        let python_tools = include_str!("../../python/src/guruterminal_finance/calculations.py");
        for name in [
            "compound_annual_growth_rate",
            "currency_convert",
            "dcf_sensitivity",
            "discounted_cash_flow",
            "enterprise_value_bridge",
            "internal_rate_of_return",
            "percentage_change",
            "period_aggregate",
            "point_in_time_filter",
            "ratio",
            "risk_metrics",
            "series_statistics",
            "weighted_average_cost_of_capital",
        ] {
            assert!(
                python_tools.contains(&format!("name=\"{name}\"")),
                "finance worker source is missing {name}"
            );
        }
    }

    #[tokio::test]
    async fn response_dispatches_only_to_matching_request() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (sender, receiver) = oneshot::channel();
        pending.lock().await.insert("one".into(), sender);
        let (progress, _) = broadcast::channel(4);
        dispatch_frame(
            br#"{"jsonrpc":"2.0","id":"other","result":{"value":2}}"#,
            &pending,
            &progress,
        )
        .await
        .unwrap();
        assert_eq!(pending.lock().await.len(), 1);
        dispatch_frame(
            br#"{"jsonrpc":"2.0","id":"one","result":{"value":1}}"#,
            &pending,
            &progress,
        )
        .await
        .unwrap();
        assert_eq!(receiver.await.unwrap().unwrap()["value"], 1);
    }

    #[tokio::test]
    async fn worker_write_times_out_when_the_child_stops_reading() {
        let (writer, _reader) = duplex(8);
        let writer = Mutex::new(writer);
        let bytes = vec![b'x'; 64 * 1024];
        assert!(matches!(
            write_worker_bytes(&writer, &bytes, Duration::from_millis(10)).await,
            Err(FinanceError::Timeout)
        ));
    }
}
