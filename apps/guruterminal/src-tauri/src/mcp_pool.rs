use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::{
    artifact_trust::{create_private_directory, ensure_private_directory},
    hashing::sha256,
    mcp::{McpError, McpSession, McpTool, ProviderlessToolPolicy},
};

const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(8 * 60);
const DEFAULT_MAX_IDLE: usize = 4;
const RESET_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const DEACTIVATE_TOOLS: &str = "deactivate_tools";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct McpPoolKey {
    pub guru_id: String,
    pub server_id: String,
    pub authority_fingerprint: String,
}

pub(crate) struct TurnMcpServer {
    pub session: McpSession,
    pub control_tool_names: BTreeSet<String>,
    pub tools: BTreeMap<String, McpTool>,
    pub enabled_provider_ids: BTreeSet<String>,
    pub providerless_tool_policy: ProviderlessToolPolicy,
    pub provider_receipt_pointer: String,
    pub sensitive_values: Vec<String>,
    pub pool_key: Option<McpPoolKey>,
    pub scratch_dir: Option<PathBuf>,
}

struct IdleMcpProcess {
    key: McpPoolKey,
    server: TurnMcpServer,
    idle_since: Instant,
}

struct McpProcessPoolInner {
    idle: VecDeque<IdleMcpProcess>,
}

impl McpProcessPoolInner {
    fn take_expired(&mut self, ttl: Duration) -> Vec<TurnMcpServer> {
        let now = Instant::now();
        let mut kept = VecDeque::new();
        let mut discarded = Vec::new();
        while let Some(entry) = self.idle.pop_front() {
            if now.saturating_duration_since(entry.idle_since) >= ttl {
                discarded.push(entry.server);
            } else {
                kept.push_back(entry);
            }
        }
        self.idle = kept;
        discarded
    }
}

#[derive(Clone)]
pub(crate) struct McpProcessPool {
    root: PathBuf,
    idle_ttl: Duration,
    max_idle: usize,
    inner: std::sync::Arc<Mutex<McpProcessPoolInner>>,
}

impl McpProcessPool {
    pub(crate) fn prepare(root: PathBuf) -> Result<Self, String> {
        ensure_private_directory(&root).map_err(|error| error.to_string())?;
        sweep_pool_root(&root);
        Ok(Self::with_limits(root, DEFAULT_IDLE_TTL, DEFAULT_MAX_IDLE))
    }

    pub(crate) fn with_limits(root: PathBuf, idle_ttl: Duration, max_idle: usize) -> Self {
        Self {
            root,
            idle_ttl,
            max_idle: max_idle.max(1),
            inner: std::sync::Arc::new(Mutex::new(McpProcessPoolInner {
                idle: VecDeque::new(),
            })),
        }
    }

    pub(crate) fn create_scratch(&self, guru_id: &str, server_id: &str) -> Result<PathBuf, String> {
        if !valid_server_id(server_id) {
            return Err("invalid MCP server id".into());
        }
        let guru = sha256(guru_id.as_bytes());
        let token = uuid::Uuid::new_v4().simple();
        let path = self
            .root
            .join(format!("{}-{server_id}-{token}", &guru[..12]));
        create_private_directory(&path).map_err(|error| error.to_string())?;
        Ok(path)
    }

    pub(crate) async fn acquire(&self, key: &McpPoolKey) -> Option<TurnMcpServer> {
        let mut discarded = Vec::new();
        let found = {
            let mut inner = self.inner.lock().await;
            discarded.extend(inner.take_expired(self.idle_ttl));
            inner
                .idle
                .iter()
                .position(|entry| entry.key == *key)
                .and_then(|index| inner.idle.remove(index))
                .map(|entry| entry.server)
        };
        for server in discarded {
            server.discard().await;
        }
        found
    }

    pub(crate) async fn release(&self, mut server: TurnMcpServer) {
        if server.pool_key.is_none() {
            server.discard().await;
            return;
        }
        match server.session.is_running().await {
            Ok(true) => {}
            _ => {
                server.discard().await;
                return;
            }
        }
        if server.reset_to_control_surface().await.is_err() {
            server.discard().await;
            return;
        }
        let key = server
            .pool_key
            .clone()
            .expect("pooled MCP server lost its key");
        let mut discarded = Vec::new();
        {
            let mut inner = self.inner.lock().await;
            discarded.extend(inner.take_expired(self.idle_ttl));
            while inner.idle.len() >= self.max_idle {
                if let Some(oldest) = inner.idle.pop_front() {
                    discarded.push(oldest.server);
                }
            }
            inner.idle.push_back(IdleMcpProcess {
                key,
                server,
                idle_since: Instant::now(),
            });
        }
        for server in discarded {
            server.discard().await;
        }
    }

    /// Quarantine turn-owned servers while they reset, then make only the
    /// successfully reset processes visible to future acquires. The owned
    /// server values stay outside `idle` for the entire reset, so a new turn
    /// cannot lease partially reset authority or tool state.
    pub(crate) fn release_in_background(&self, servers: Vec<TurnMcpServer>) {
        if servers.is_empty() {
            return;
        }
        let pool = self.clone();
        let _reset = tokio::spawn(async move {
            for server in servers {
                pool.release(server).await;
            }
        });
    }

    #[cfg(test)]
    pub(crate) async fn idle_count(&self) -> usize {
        self.inner.lock().await.idle.len()
    }

    #[cfg(test)]
    pub(crate) async fn shutdown(&self) {
        let idle = {
            let mut inner = self.inner.lock().await;
            std::mem::take(&mut inner.idle)
        };
        for entry in idle {
            entry.server.discard().await;
        }
    }
}

impl TurnMcpServer {
    pub(crate) async fn reset_to_control_surface(&mut self) -> Result<(), McpError> {
        let listed = self.session.list_tools().await?;
        let extras = listed
            .iter()
            .map(|tool| tool.name.clone())
            .filter(|name| !self.control_tool_names.contains(name))
            .collect::<Vec<_>>();
        if !extras.is_empty() {
            if !self.control_tool_names.contains(DEACTIVATE_TOOLS) {
                return Err(McpError::Protocol);
            }
            let result = self
                .session
                .call_tool(
                    DEACTIVATE_TOOLS,
                    json!({ "tool_names": extras }),
                    RESET_TIMEOUT,
                )
                .await?;
            if result.is_error {
                return Err(McpError::Protocol);
            }
        }
        let listed = self.session.list_tools().await?;
        let names = listed
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>();
        if names != self.control_tool_names {
            return Err(McpError::Protocol);
        }
        self.tools = listed
            .into_iter()
            .map(|tool| (tool.name.clone(), tool))
            .collect();
        let _ = self.session.take_tools_changed();
        Ok(())
    }

    pub(crate) async fn discard(self) {
        let Self {
            session,
            scratch_dir,
            ..
        } = self;
        let _ = session.shutdown(SHUTDOWN_GRACE).await;
        if let Some(path) = scratch_dir {
            let _ = tokio::fs::remove_dir_all(path).await;
        }
    }
}

pub(crate) fn authority_fingerprint(canonical: &Value) -> String {
    sha256(&serde_json::to_vec(canonical).expect("MCP authority fingerprint must encode"))
}

fn valid_server_id(server_id: &str) -> bool {
    let bytes = server_id.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn sweep_pool_root(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn authority_fingerprint_changes_with_credentials_or_providers() {
        let yfinance = json!({
            "allowed_categories": ["equity"],
            "allowed_network_hosts": ["query1.finance.yahoo.com"],
            "control_tool_names": ["activate_tools", "deactivate_tools"],
            "credentials": {},
            "enabled_provider_ids": ["yfinance"],
            "executable": "/runtime/openbb",
            "provider_config": {},
        });
        let rotated = json!({
            "allowed_categories": ["equity"],
            "allowed_network_hosts": ["query1.finance.yahoo.com"],
            "control_tool_names": ["activate_tools", "deactivate_tools"],
            "credentials": {"fmp_api_key": "rotated"},
            "enabled_provider_ids": ["yfinance"],
            "executable": "/runtime/openbb",
            "provider_config": {},
        });
        let fmp = json!({
            "allowed_categories": ["equity"],
            "allowed_network_hosts": ["financialmodelingprep.com"],
            "control_tool_names": ["activate_tools", "deactivate_tools"],
            "credentials": {"fmp_api_key": "rotated"},
            "enabled_provider_ids": ["fmp"],
            "executable": "/runtime/openbb",
            "provider_config": {},
        });
        let first = authority_fingerprint(&yfinance);
        assert_eq!(first, authority_fingerprint(&yfinance));
        assert_ne!(first, authority_fingerprint(&rotated));
        assert_ne!(first, authority_fingerprint(&fmp));
    }

    #[test]
    fn create_scratch_rejects_unsafe_server_ids() {
        let temporary = tempfile::tempdir().unwrap();
        let pool = McpProcessPool::with_limits(temporary.path().to_path_buf(), DEFAULT_IDLE_TTL, 1);
        assert!(pool.create_scratch("guru", "../openbb").is_err());
        assert!(pool.create_scratch("guru", "openbb/extra").is_err());
        assert!(pool.create_scratch("guru", "").is_err());
        let scratch = pool.create_scratch("guru-a", "openbb").unwrap();
        assert!(scratch.starts_with(temporary.path()));
        assert!(scratch
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("-openbb-")));
    }
}
