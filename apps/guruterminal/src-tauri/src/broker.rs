use async_trait::async_trait;
use futures_util::{stream::FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    future::Future,
    io,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};
use thiserror::Error;
#[cfg(windows)]
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{watch, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::timeout,
};
use uuid::Uuid;

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, HANDLE},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

use crate::chat_artifacts::MAX_CHAT_TURN_ARTIFACTS;

const PROTOCOL: &str = "guruterminal-tool/1";
// Memory proposals may contain up to 2 MiB of Markdown. Keep framing headroom
// for typed target metadata and JSON escaping.
const MAX_FRAME_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_MEMORY_PROPOSALS: u8 = 8;
const MAX_CONCURRENT_CONNECTIONS: usize = 64;
const RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const DELIVERY_ACK_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(not(test))]
const AUTHENTICATION_READ_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const AUTHENTICATION_READ_TIMEOUT: Duration = Duration::from_millis(100);

type SharedTransactionCardinality = Arc<StdMutex<TransactionCardinality>>;
type ConnectionFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

#[cfg(windows)]
struct WindowsPipeSecurity {
    descriptor: *mut std::ffi::c_void,
}

#[cfg(windows)]
// The descriptor is immutable after construction and LocalFree permits the
// owning allocation to be released from any thread.
unsafe impl Send for WindowsPipeSecurity {}

#[cfg(windows)]
impl WindowsPipeSecurity {
    fn current_user_and_system() -> io::Result<Self> {
        let user_sid = current_user_sid_string()?;
        let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;{user_sid})");
        let encoded = sddl.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: `encoded` is a live, NUL-terminated SDDL string and the output
        // receives one LocalAlloc-owned self-relative security descriptor.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                encoded.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { descriptor })
    }
}

#[cfg(windows)]
impl Drop for WindowsPipeSecurity {
    fn drop(&mut self) {
        // SAFETY: the conversion API returned this LocalAlloc allocation and
        // this value is its unique owner.
        unsafe { LocalFree(self.descriptor) };
    }
}

#[cfg(windows)]
fn current_user_sid_string() -> io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo process handle is always valid and `token` is writable.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        let mut needed = 0_u32;
        // The first call intentionally discovers the required buffer size.
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        if needed < std::mem::size_of::<TOKEN_USER>() as u32 {
            return Err(io::Error::last_os_error());
        }
        let words = (needed as usize).div_ceil(std::mem::size_of::<usize>());
        let mut buffer = vec![0_usize; words];
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetTokenInformation initialized a suitably aligned TOKEN_USER
        // and its SID remains backed by `buffer` for this conversion call.
        let sid = unsafe { (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        let mut sid_text = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut sid_text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let converted = (|| {
            let mut length = 0_usize;
            // SID strings are bounded by the SID format; retain a defensive cap
            // before creating the borrowed UTF-16 slice.
            while length <= 256 && unsafe { *sid_text.add(length) } != 0 {
                length += 1;
            }
            if length > 256 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "current user SID string is invalid",
                ));
            }
            // SAFETY: the conversion API returned at least `length + 1` UTF-16
            // units ending in NUL.
            String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, length) })
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "SID is not UTF-16"))
        })();
        // SAFETY: ConvertSidToStringSidW allocated this string with LocalAlloc.
        unsafe { LocalFree(sid_text.cast()) };
        converted
    })();
    // SAFETY: OpenProcessToken returned this uniquely owned handle.
    unsafe { CloseHandle(token) };
    result
}

#[cfg(windows)]
fn create_windows_pipe_server(
    path: &PathBuf,
    first: bool,
    security: &WindowsPipeSecurity,
) -> io::Result<NamedPipeServer> {
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.descriptor,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true);
    // Keep Tokio's Win32 `PIPE_UNLIMITED_INSTANCES` default. Each accepted
    // connection remains backed by an OS-accounted pipe handle, while the
    // broker does not make long-running independent tool calls wait behind an
    // arbitrary four-handler ceiling.
    // SAFETY: `attributes` and its immutable descriptor remain live throughout
    // CreateNamedPipeW; Windows copies the security descriptor into the object.
    unsafe {
        options.create_with_security_attributes_raw(
            path,
            (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolPolicy {
    pub guru_id: String,
    pub session_id: String,
    pub use_memory: bool,
    pub propose_memory_updates: bool,
    #[serde(default = "default_memory_proposal_budget")]
    pub memory_proposal_budget: u8,
    #[serde(default)]
    pub as_of: Option<String>,
}

const fn default_memory_proposal_budget() -> u8 {
    1
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolMethod {
    GuruSearch,
    GuruRead,
    GuruReadPrevious,
    FinanceSources,
    FinanceMacroData,
    FinanceMarketData,
    FinanceCompanyData,
    FinanceFilings,
    FinanceCalculate,
    FinanceResolveEntity,
    McpConnect,
    McpCall,
    RunResultsList,
    ComputeRun,
    WebSearch,
    WebFetch,
    DecisionSubmit,
    EvidenceCreate,
    MemoryPatchPropose,
    ArtifactList,
    ArtifactRead,
    ArtifactPublish,
    ChartQuery,
    ChartPublish,
    WorkbenchRead,
    WorkbenchWrite,
    WorkbenchEdit,
    WorkbenchList,
    WorkbenchFind,
    WorkbenchGrep,
}

impl ToolMethod {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "guru.search" => Some(Self::GuruSearch),
            "guru.read" => Some(Self::GuruRead),
            "guru.read_previous" => Some(Self::GuruReadPrevious),
            "finance.sources" => Some(Self::FinanceSources),
            "finance.macro_data" => Some(Self::FinanceMacroData),
            "finance.market_data" => Some(Self::FinanceMarketData),
            "finance.company_data" => Some(Self::FinanceCompanyData),
            "finance.filings" => Some(Self::FinanceFilings),
            "finance.calculate" => Some(Self::FinanceCalculate),
            "finance.resolve_entity" => Some(Self::FinanceResolveEntity),
            "mcp.connect" => Some(Self::McpConnect),
            "mcp.call" => Some(Self::McpCall),
            "run.results.list" => Some(Self::RunResultsList),
            "compute.run" => Some(Self::ComputeRun),
            "web.search" => Some(Self::WebSearch),
            "web.fetch" => Some(Self::WebFetch),
            "decision.submit" => Some(Self::DecisionSubmit),
            "evidence.create" => Some(Self::EvidenceCreate),
            "memory.patch.propose" => Some(Self::MemoryPatchPropose),
            "artifact.list" => Some(Self::ArtifactList),
            "artifact.read" => Some(Self::ArtifactRead),
            "artifact.publish" => Some(Self::ArtifactPublish),
            "chart.query" => Some(Self::ChartQuery),
            "chart.publish" => Some(Self::ChartPublish),
            "workbench.read" => Some(Self::WorkbenchRead),
            "workbench.write" => Some(Self::WorkbenchWrite),
            "workbench.edit" => Some(Self::WorkbenchEdit),
            "workbench.ls" => Some(Self::WorkbenchList),
            "workbench.find" => Some(Self::WorkbenchFind),
            "workbench.grep" => Some(Self::WorkbenchGrep),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("tool broker I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("tool is not in the Guru Terminal allowlist")]
    MethodDenied,
    #[error("Guru Memory use is disabled for this turn")]
    MemoryDisabled,
    #[error("memory proposals are disabled for this turn")]
    ProposalDisabled,
    #[error("tool request is malformed")]
    Malformed,
    #[error("tool broker authentication failed")]
    Authentication,
    #[error("tool frame exceeded the size limit")]
    FrameTooLarge,
    #[error("tool execution failed: {0}")]
    Execution(String),
    #[error("transaction cardinality for this run is exhausted")]
    BudgetExceeded,
}

#[async_trait]
pub trait ToolExecutor: Send + Sync + 'static {
    async fn execute(
        &self,
        policy: &ToolPolicy,
        method: ToolMethod,
        params: Value,
    ) -> Result<Value, BrokerError>;

    async fn execute_for_delivery(
        &self,
        policy: &ToolPolicy,
        method: ToolMethod,
        params: Value,
        _delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        self.execute(policy, method, params).await
    }

    async fn commit_delivery(&self, _policy: &ToolPolicy, _delivery_id: &str) {}

    async fn discard_delivery(&self, _policy: &ToolPolicy, _delivery_id: &str) {}
}

#[derive(Deserialize)]
struct BrokerRequest {
    protocol: String,
    id: String,
    token: String,
    method: String,
    params: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerDeliveryAck {
    protocol: String,
    id: String,
    delivered: bool,
}

#[derive(Serialize)]
struct BrokerCommitBarrier<'a> {
    protocol: &'static str,
    id: &'a str,
    committed: bool,
}

#[derive(Serialize)]
struct BrokerResponse {
    protocol: &'static str,
    id: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

struct BoundedJsonBuffer {
    bytes: Vec<u8>,
    limit: usize,
    overflowed: bool,
}

impl BoundedJsonBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
            overflowed: false,
        }
    }
}

impl io::Write for BoundedJsonBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|size| size > self.limit)
        {
            self.overflowed = true;
            return Err(io::Error::other("bounded broker frame exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct ToolBrokerHandle {
    pub socket_path: PathBuf,
    token: String,
    shutdown: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
    identity_lease: Option<ToolBrokerIdentityLease>,
}

/// Process-lifetime authority for sequential, turn-scoped tool brokers.
///
/// The endpoint and token remain stable so a future long-lived Pi process can
/// keep one broker identity, while each broker start still receives a fresh
/// policy, executor, and transaction-cardinality state. This type deliberately
/// does not implement `Debug` or serialization because its token is authority.
#[derive(Clone)]
pub struct ToolBrokerIdentity {
    inner: Arc<ToolBrokerIdentityInner>,
}

struct ToolBrokerIdentityInner {
    socket_path: PathBuf,
    token: String,
    leased: AtomicBool,
}

struct ToolBrokerIdentityLease {
    identity: Arc<ToolBrokerIdentityInner>,
}

impl Drop for ToolBrokerIdentityLease {
    fn drop(&mut self) {
        self.identity.leased.store(false, Ordering::Release);
    }
}

impl ToolBrokerIdentity {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            inner: Arc::new(ToolBrokerIdentityInner {
                socket_path,
                token: new_broker_token(),
                leased: AtomicBool::new(false),
            }),
        }
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.inner.socket_path
    }

    pub fn token(&self) -> &str {
        &self.inner.token
    }

    pub async fn start(
        &self,
        policy: ToolPolicy,
        executor: Arc<dyn ToolExecutor>,
    ) -> Result<ToolBrokerHandle, BrokerError> {
        start_tool_broker_with_identity(self, policy, executor).await
    }

    fn acquire(&self) -> Result<ToolBrokerIdentityLease, BrokerError> {
        self.inner
            .leased
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                BrokerError::Io(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "tool broker identity is already active",
                ))
            })?;
        Ok(ToolBrokerIdentityLease {
            identity: self.inner.clone(),
        })
    }
}

#[derive(Debug, Default)]
struct TransactionCardinality {
    decisions: u8,
    evidence_creations: u8,
    proposals: u8,
    artifact_publishes: u8,
}

impl TransactionCardinality {
    fn consume(&mut self, method: ToolMethod, policy: &ToolPolicy) -> Result<(), BrokerError> {
        match method {
            ToolMethod::DecisionSubmit if self.decisions >= 1 => {
                return Err(BrokerError::BudgetExceeded);
            }
            ToolMethod::EvidenceCreate if self.evidence_creations >= 3 => {
                return Err(BrokerError::BudgetExceeded);
            }
            ToolMethod::MemoryPatchPropose if self.proposals >= policy.memory_proposal_budget => {
                return Err(BrokerError::BudgetExceeded);
            }
            ToolMethod::ArtifactPublish | ToolMethod::ChartPublish
                if self.artifact_publishes >= MAX_CHAT_TURN_ARTIFACTS as u8 =>
            {
                return Err(BrokerError::BudgetExceeded);
            }
            _ => {}
        }
        match method {
            ToolMethod::DecisionSubmit => self.decisions += 1,
            ToolMethod::EvidenceCreate => self.evidence_creations += 1,
            ToolMethod::MemoryPatchPropose => self.proposals += 1,
            ToolMethod::ArtifactPublish | ToolMethod::ChartPublish => self.artifact_publishes += 1,
            _ => {}
        }
        Ok(())
    }

    fn rollback(&mut self, method: ToolMethod) {
        match method {
            ToolMethod::DecisionSubmit => self.decisions = self.decisions.saturating_sub(1),
            ToolMethod::EvidenceCreate => {
                self.evidence_creations = self.evidence_creations.saturating_sub(1)
            }
            ToolMethod::MemoryPatchPropose => self.proposals = self.proposals.saturating_sub(1),
            ToolMethod::ArtifactPublish | ToolMethod::ChartPublish => {
                self.artifact_publishes = self.artifact_publishes.saturating_sub(1)
            }
            _ => {}
        }
    }
}

#[derive(Debug)]
struct TransactionReservation {
    cardinality: SharedTransactionCardinality,
    method: ToolMethod,
    committed: bool,
}

#[derive(Debug)]
struct ExecutedRequest {
    id: String,
    result: Value,
    method: ToolMethod,
    reservation: TransactionReservation,
}

impl TransactionReservation {
    fn reserve(
        cardinality: SharedTransactionCardinality,
        method: ToolMethod,
        policy: &ToolPolicy,
    ) -> Result<Self, BrokerError> {
        cardinality
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .consume(method, policy)?;
        Ok(Self {
            cardinality,
            method,
            committed: false,
        })
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for TransactionReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.cardinality
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .rollback(self.method);
        }
    }
}

impl ToolBrokerHandle {
    pub fn token(&self) -> &str {
        &self.token
    }

    pub async fn shutdown(mut self) -> Result<(), BrokerError> {
        #[cfg(feature = "e2e")]
        eprintln!("Guru Terminal E2E broker explicit shutdown requested");
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.send_replace(true);
        }
        let task_result = if let Some(mut task) = self.task.take() {
            match timeout(Duration::from_secs(2), &mut task).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(BrokerError::Io(io::Error::other(format!(
                    "tool broker task failed: {error}"
                )))),
                Err(_) => {
                    // Connection handlers are owned as futures inside this task.
                    // Awaiting its abort therefore drops every in-flight tool
                    // future before shutdown reports the bounded failure.
                    task.abort();
                    let _ = task.await;
                    Err(BrokerError::Io(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "tool broker task did not drain after shutdown",
                    )))
                }
            }
        } else {
            Ok(())
        };
        let endpoint_result = remove_broker_endpoint(&self.socket_path);
        // Keep the identity leased until both the accept task and every
        // connection handler have drained and the accepting endpoint is gone.
        self.identity_lease.take();
        task_result?;
        endpoint_result
    }
}

impl Drop for ToolBrokerHandle {
    fn drop(&mut self) {
        #[cfg(feature = "e2e")]
        eprintln!("Guru Terminal E2E broker handle dropped");
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.send_replace(true);
        }
        if let Some(mut task) = self.task.take() {
            let identity_lease = self.identity_lease.take();
            let socket_path = self.socket_path.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if timeout(Duration::from_secs(2), &mut task).await.is_err() {
                        task.abort();
                        let _ = task.await;
                    }
                    let _ = remove_broker_endpoint(&socket_path);
                    drop(identity_lease);
                });
            } else {
                task.abort();
                let _ = remove_broker_endpoint(&socket_path);
                drop(identity_lease);
            }
            return;
        }
        let _ = remove_broker_endpoint(&self.socket_path);
        self.identity_lease.take();
    }
}

#[cfg(unix)]
fn remove_broker_endpoint(endpoint: &PathBuf) -> Result<(), BrokerError> {
    match std::fs::remove_file(endpoint) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BrokerError::Io(error)),
    }
}

#[cfg(windows)]
fn remove_broker_endpoint(_endpoint: &PathBuf) -> Result<(), BrokerError> {
    // Named pipes disappear when their final server handle closes.
    Ok(())
}

/// Resolves a run-scoped broker endpoint for the current platform.
///
/// Unix uses the caller-provided filesystem path. Windows named pipes live in
/// a separate namespace, so a stable digest keeps the endpoint short and free
/// of user-controlled separators. The returned path must be passed unchanged
/// both to [`start_tool_broker`] and to the Pi environment.
pub fn tool_broker_endpoint(logical_path: PathBuf) -> PathBuf {
    #[cfg(unix)]
    {
        logical_path
    }
    #[cfg(windows)]
    {
        use sha2::{Digest, Sha256};

        let digest = hex::encode(Sha256::digest(logical_path.to_string_lossy().as_bytes()));
        PathBuf::from(format!(r"\\.\pipe\guruterminal-tool-{}", &digest[..32]))
    }
}

pub async fn start_tool_broker(
    socket_path: PathBuf,
    policy: ToolPolicy,
    executor: Arc<dyn ToolExecutor>,
) -> Result<ToolBrokerHandle, BrokerError> {
    ToolBrokerIdentity::new(socket_path)
        .start(policy, executor)
        .await
}

async fn start_tool_broker_with_identity(
    identity: &ToolBrokerIdentity,
    policy: ToolPolicy,
    executor: Arc<dyn ToolExecutor>,
) -> Result<ToolBrokerHandle, BrokerError> {
    if policy.memory_proposal_budget > MAX_MEMORY_PROPOSALS
        || (policy.propose_memory_updates && policy.memory_proposal_budget == 0)
    {
        return Err(BrokerError::Malformed);
    }
    let identity_lease = identity.acquire()?;
    let socket_path = identity.socket_path().clone();
    let (shutdown, shutdown_receiver) = watch::channel(false);
    #[cfg(unix)]
    let task = {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::symlink_metadata(&socket_path) {
            Ok(_) => {
                return Err(BrokerError::Io(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "tool broker endpoint already exists",
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(BrokerError::Io(error)),
        }
        let listener = UnixListener::bind(&socket_path)?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
        let server_token = identity.token().to_owned();
        let task_token = server_token.clone();
        let task_shutdown = shutdown.clone();
        let mut shutdown_receiver = shutdown_receiver;
        let task = tokio::spawn(async move {
            let cardinality = Arc::new(StdMutex::new(TransactionCardinality::default()));
            let connection_slots = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
            let policy = Arc::new(policy);
            let mut handlers = FuturesUnordered::new();
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_requested(&mut shutdown_receiver) => break,
                    _ = handlers.next(), if !handlers.is_empty() => {}
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => {
                            if let Ok(permit) = connection_slots.clone().try_acquire_owned() {
                                handlers.push(connection_handler(
                                    stream,
                                    task_token.clone(),
                                    policy.clone(),
                                    cardinality.clone(),
                                    executor.clone(),
                                    task_shutdown.subscribe(),
                                    permit,
                                ));
                            }
                        }
                        Err(_) => break,
                    },
                }
            }
            task_shutdown.send_replace(true);
            drain_connection_handlers(&mut handlers).await;
        });
        (server_token, task)
    };

    #[cfg(windows)]
    let task = {
        if !socket_path.to_string_lossy().starts_with(r"\\.\pipe\") {
            return Err(BrokerError::Malformed);
        }
        let security = WindowsPipeSecurity::current_user_and_system()?;
        let server = create_windows_pipe_server(&socket_path, true, &security)?;
        let server_token = identity.token().to_owned();
        let task_token = server_token.clone();
        let task_path = socket_path.clone();
        let task_shutdown = shutdown.clone();
        let mut shutdown_receiver = shutdown_receiver;
        let task = tokio::spawn(async move {
            let cardinality = Arc::new(StdMutex::new(TransactionCardinality::default()));
            let connection_slots = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
            let policy = Arc::new(policy);
            let mut handlers = FuturesUnordered::new();
            let mut server = server;
            loop {
                let connected = tokio::select! {
                    biased;
                    _ = shutdown_requested(&mut shutdown_receiver) => break,
                    _ = handlers.next(), if !handlers.is_empty() => continue,
                    connected = server.connect() => connected,
                };
                if connected.is_err() {
                    break;
                }
                let next = create_windows_pipe_server(&task_path, false, &security);
                if let Ok(permit) = connection_slots.clone().try_acquire_owned() {
                    handlers.push(connection_handler(
                        server,
                        task_token.clone(),
                        policy.clone(),
                        cardinality.clone(),
                        executor.clone(),
                        task_shutdown.subscribe(),
                        permit,
                    ));
                }
                match next {
                    Ok(next) => server = next,
                    Err(_) => break,
                }
            }
            task_shutdown.send_replace(true);
            drain_connection_handlers(&mut handlers).await;
        });
        (server_token, task)
    };

    let (token, task) = task;

    Ok(ToolBrokerHandle {
        socket_path,
        token,
        shutdown: Some(shutdown),
        task: Some(task),
        identity_lease: Some(identity_lease),
    })
}

fn connection_handler<S>(
    stream: S,
    expected_token: String,
    policy: Arc<ToolPolicy>,
    cardinality: SharedTransactionCardinality,
    executor: Arc<dyn ToolExecutor>,
    shutdown: watch::Receiver<bool>,
    _permit: OwnedSemaphorePermit,
) -> ConnectionFuture
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    Box::pin(async move {
        // Do not cancel an in-flight handler when the accept loop shuts
        // down. After the client ACKs a result, dropping the write of the
        // commit barrier makes delivery indeterminate (Pi exit 71). Hung
        // tools are still bounded by ToolBrokerHandle::shutdown's abort.
        let _ = shutdown;
        let result =
            handle_connection(stream, &expected_token, &policy, cardinality, executor).await;
        #[cfg(feature = "e2e")]
        if let Err(error) = result {
            eprintln!("Guru Terminal E2E broker handler failed: {error}");
        }
        #[cfg(not(feature = "e2e"))]
        let _ = result;
    })
}

async fn shutdown_requested(shutdown: &mut watch::Receiver<bool>) {
    if *shutdown.borrow_and_update() {
        return;
    }
    loop {
        if shutdown.changed().await.is_err() || *shutdown.borrow_and_update() {
            return;
        }
    }
}

async fn drain_connection_handlers(handlers: &mut FuturesUnordered<ConnectionFuture>) {
    while handlers.next().await.is_some() {}
}

async fn handle_connection<S>(
    stream: S,
    expected_token: &str,
    policy: &ToolPolicy,
    cardinality: SharedTransactionCardinality,
    executor: Arc<dyn ToolExecutor>,
) -> Result<(), BrokerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let mut bytes = Vec::new();
    let read = timeout(
        AUTHENTICATION_READ_TIMEOUT,
        (&mut reader)
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "broker authentication timed out"))??;
    if read == 0 || bytes.last() != Some(&b'\n') {
        return Err(BrokerError::Malformed);
    }
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(BrokerError::FrameTooLarge);
    }
    bytes.pop();
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let request: BrokerRequest =
        serde_json::from_slice(&bytes).map_err(|_| BrokerError::Malformed)?;
    let delivery_id = new_broker_token();
    let execution_result = {
        let execution = execute_request(
            request,
            expected_token,
            policy,
            cardinality,
            executor.clone(),
            &delivery_id,
        );
        tokio::pin!(execution);
        tokio::select! {
            biased;
            result = &mut execution => Some(result),
            _ = wait_for_peer_disconnect(&mut reader) => None,
        }
    };
    let Some(execution_result) = execution_result else {
        executor.discard_delivery(policy, &delivery_id).await;
        return Ok(());
    };
    let (response, mut transaction) = match execution_result {
        Ok(executed) => (
            BrokerResponse {
                protocol: PROTOCOL,
                id: executed.id,
                ok: true,
                result: Some(executed.result),
                error: None,
            },
            Some((executed.method, executed.reservation)),
        ),
        Err((id, error)) => {
            executor.discard_delivery(policy, &delivery_id).await;
            (
                BrokerResponse {
                    protocol: PROTOCOL,
                    id,
                    ok: false,
                    result: None,
                    error: Some(
                        json!({ "code": error_code(&error), "message": error.to_string() }),
                    ),
                },
                None,
            )
        }
    };
    let response_id = response.id.clone();
    let (encoded, delivered_success) = match encode_response_or_bounded_error(response) {
        Ok(encoded) => encoded,
        Err(error) => {
            executor.discard_delivery(policy, &delivery_id).await;
            return Err(error);
        }
    };
    if !delivered_success {
        executor.discard_delivery(policy, &delivery_id).await;
        transaction.take();
    }
    if let Err(error) = write_response(&mut writer, &encoded).await {
        executor.discard_delivery(policy, &delivery_id).await;
        return Err(error);
    }
    if let Err(error) = read_delivery_ack(&mut reader, &response_id).await {
        executor.discard_delivery(policy, &delivery_id).await;
        return Err(error);
    }
    if let Some((_, reservation)) = transaction {
        executor.commit_delivery(policy, &delivery_id).await;
        reservation.commit();
    }
    let committed = encode_commit_barrier(&response_id)?;
    write_response(&mut writer, &committed).await?;
    // The explicit committed frame is the client-visible terminal boundary.
    // A later half-close anomaly cannot roll back the already acknowledged and
    // committed delivery, so shutdown remains best-effort.
    let _ = shutdown_writer(&mut writer).await;
    Ok(())
}

fn encode_commit_barrier(id: &str) -> Result<Vec<u8>, BrokerError> {
    let mut bytes = serde_json::to_vec(&BrokerCommitBarrier {
        protocol: PROTOCOL,
        id,
        committed: true,
    })
    .map_err(|_| BrokerError::Malformed)?;
    if bytes.len() >= MAX_FRAME_BYTES {
        return Err(BrokerError::FrameTooLarge);
    }
    bytes.push(b'\n');
    Ok(bytes)
}

async fn write_response<W>(writer: &mut W, bytes: &[u8]) -> Result<(), BrokerError>
where
    W: AsyncWrite + Unpin,
{
    timeout(RESPONSE_WRITE_TIMEOUT, async {
        writer.write_all(bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| {
        BrokerError::Io(io::Error::new(
            io::ErrorKind::TimedOut,
            "broker response write timed out",
        ))
    })??;
    Ok(())
}

async fn read_delivery_ack<R>(
    reader: &mut BufReader<R>,
    expected_id: &str,
) -> Result<(), BrokerError>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let read = timeout(
        DELIVERY_ACK_TIMEOUT,
        reader
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "broker delivery ACK timed out"))??;
    if read == 0 || bytes.last() != Some(&b'\n') {
        return Err(BrokerError::Malformed);
    }
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(BrokerError::FrameTooLarge);
    }
    bytes.pop();
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let ack: BrokerDeliveryAck =
        serde_json::from_slice(&bytes).map_err(|_| BrokerError::Malformed)?;
    if ack.protocol != PROTOCOL || ack.id != expected_id || !ack.delivered {
        return Err(BrokerError::Malformed);
    }
    Ok(())
}

async fn shutdown_writer<W>(writer: &mut W) -> Result<(), BrokerError>
where
    W: AsyncWrite + Unpin,
{
    timeout(RESPONSE_WRITE_TIMEOUT, writer.shutdown())
        .await
        .map_err(|_| {
            BrokerError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "broker response shutdown timed out",
            ))
        })??;
    Ok(())
}

async fn wait_for_peer_disconnect<R>(reader: &mut R)
where
    R: AsyncRead + Unpin,
{
    let mut byte = [0_u8; 1];
    let _ = reader.read(&mut byte).await;
}

fn new_broker_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

async fn execute_request(
    request: BrokerRequest,
    expected_token: &str,
    policy: &ToolPolicy,
    cardinality: SharedTransactionCardinality,
    executor: Arc<dyn ToolExecutor>,
    delivery_id: &str,
) -> Result<ExecutedRequest, (String, BrokerError)> {
    let id = request.id;
    if id.is_empty() || id.len() > 256 || id.contains(['\n', '\r', '\0']) {
        return Err(("invalid-request-id".into(), BrokerError::Malformed));
    }
    if request.protocol != PROTOCOL {
        return Err((id, BrokerError::Malformed));
    }
    if !constant_time_eq(request.token.as_bytes(), expected_token.as_bytes()) {
        return Err((id, BrokerError::Authentication));
    }
    let method = ToolMethod::parse(&request.method).ok_or_else(|| {
        let error = BrokerError::MethodDenied;
        (id.clone(), error)
    })?;
    enforce_policy(policy, method).map_err(|error| (id.clone(), error))?;
    let reservation = TransactionReservation::reserve(cardinality, method, policy)
        .map_err(|error| (id.clone(), error))?;
    let result = executor
        .execute_for_delivery(policy, method, request.params, delivery_id)
        .await
        .map_err(|error| (id.clone(), error))?;
    Ok(ExecutedRequest {
        id,
        result,
        method,
        reservation,
    })
}

fn encode_response(response: &BrokerResponse) -> Result<Vec<u8>, BrokerError> {
    let mut writer = BoundedJsonBuffer::new(MAX_FRAME_BYTES - 1);
    if serde_json::to_writer(&mut writer, response).is_err() {
        return Err(if writer.overflowed {
            BrokerError::FrameTooLarge
        } else {
            BrokerError::Malformed
        });
    }
    writer.bytes.push(b'\n');
    Ok(writer.bytes)
}

fn encode_response_or_bounded_error(
    response: BrokerResponse,
) -> Result<(Vec<u8>, bool), BrokerError> {
    match encode_response(&response) {
        Ok(bytes) => Ok((bytes, response.ok)),
        Err(BrokerError::FrameTooLarge) => encode_response(&BrokerResponse {
            protocol: PROTOCOL,
            id: response.id,
            ok: false,
            result: None,
            error: Some(json!({
                "code": "frame_too_large",
                "message": "tool response exceeded the bounded broker frame"
            })),
        })
        .map(|bytes| (bytes, false)),
        Err(error) => Err(error),
    }
}

fn enforce_policy(policy: &ToolPolicy, method: ToolMethod) -> Result<(), BrokerError> {
    if matches!(
        method,
        ToolMethod::GuruSearch | ToolMethod::GuruRead | ToolMethod::GuruReadPrevious
    ) && !policy.use_memory
    {
        return Err(BrokerError::MemoryDisabled);
    }
    if matches!(method, ToolMethod::MemoryPatchPropose) && !policy.propose_memory_updates {
        return Err(BrokerError::ProposalDisabled);
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn error_code(error: &BrokerError) -> &'static str {
    match error {
        BrokerError::MethodDenied => "method_denied",
        BrokerError::MemoryDisabled => "memory_disabled",
        BrokerError::ProposalDisabled => "proposal_disabled",
        BrokerError::Authentication => "authentication_failed",
        BrokerError::FrameTooLarge => "frame_too_large",
        BrokerError::Malformed => "malformed_request",
        BrokerError::Execution(_) => "execution_failed",
        BrokerError::BudgetExceeded => "transaction_limit",
        BrokerError::Io(_) => "io_failed",
    }
}

#[cfg(test)]
mod tests;
