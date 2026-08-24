use async_trait::async_trait;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use tokio::{sync::broadcast, time::timeout};

#[cfg(windows)]
use crate::windows_fs::{ensure_no_reparse_points, metadata_is_reparse};
use crate::{
    app::{CommandError, PiArtifacts},
    pi::{PiEvent, PiProcess},
    settings::{ConfiguredModel, ExecutionModelLock},
};

const PI_CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A catalog-resolved execution choice. Every model-backed product path must
/// carry this value through process launch and verify it against live Pi state
/// before allowing the first model prompt.
#[derive(Clone)]
pub struct PiExecutionConfig {
    pub artifacts: PiArtifacts,
    pub model: ConfiguredModel,
    pub thinking_level: String,
    pub run_options: BTreeMap<String, String>,
}

impl PiExecutionConfig {
    pub fn new(
        mut artifacts: PiArtifacts,
        model: ConfiguredModel,
        thinking_level: &str,
        run_options: &BTreeMap<String, String>,
    ) -> Result<Self, CommandError> {
        model.validate()?;
        model.validate_thinking_level(thinking_level)?;
        model.validate_run_options(run_options)?;
        artifacts.provider = model.provider.clone();
        artifacts.model = model.model.clone();
        artifacts.thinking_level = thinking_level.to_owned();
        artifacts.run_options = run_options.clone();
        Ok(Self {
            artifacts,
            model,
            thinking_level: thinking_level.to_owned(),
            run_options: run_options.clone(),
        })
    }

    pub fn model_lock(&self) -> ExecutionModelLock {
        ExecutionModelLock::from_model(&self.model, &self.thinking_level, &self.run_options)
    }
}

async fn wait_for_pi_response(
    receiver: &mut broadcast::Receiver<PiEvent>,
    request_id: u64,
    operation: &'static str,
) -> Result<Value, CommandError> {
    timeout(PI_CONTROL_TIMEOUT, async {
        loop {
            match receiver.recv().await {
                Ok(PiEvent::Rpc { payload })
                    if payload.get("type").and_then(Value::as_str) == Some("response")
                        && payload.get("id").and_then(Value::as_u64) == Some(request_id) =>
                {
                    if payload.get("success").and_then(Value::as_bool) == Some(true) {
                        return Ok(payload.get("data").cloned().unwrap_or(Value::Null));
                    }
                    return Err(CommandError::new(
                        "pi_unavailable",
                        format!("Pi rejected the {operation} control"),
                    ));
                }
                Ok(PiEvent::ProtocolError { .. }) | Ok(PiEvent::Exited) => {
                    return Err(CommandError::new(
                        "pi_unavailable",
                        format!("Pi stopped during the {operation} control"),
                    ));
                }
                Ok(_) => {}
                Err(_) => {
                    return Err(CommandError::new(
                        "pi_unavailable",
                        format!("Pi event stream closed during the {operation} control"),
                    ));
                }
            }
        }
    })
    .await
    .map_err(|_| {
        CommandError::new(
            "pi_unavailable",
            format!("Pi {operation} control timed out"),
        )
    })?
}

#[async_trait]
trait PiExecutionControl {
    async fn available_thinking_levels(&mut self) -> Result<Value, CommandError>;
    async fn state(&mut self) -> Result<Value, CommandError>;
}

struct LivePiExecutionControl<'a> {
    pi: &'a PiProcess,
    events: &'a mut broadcast::Receiver<PiEvent>,
}

#[async_trait]
impl PiExecutionControl for LivePiExecutionControl<'_> {
    async fn available_thinking_levels(&mut self) -> Result<Value, CommandError> {
        let request_id = self.pi.get_available_thinking_levels().await.map_err(|_| {
            CommandError::new("pi_unavailable", "Pi could not read thinking levels")
        })?;
        wait_for_pi_response(self.events, request_id, "thinking-level discovery").await
    }

    async fn state(&mut self) -> Result<Value, CommandError> {
        let request_id = self.pi.get_state().await.map_err(|_| {
            CommandError::new("pi_unavailable", "Pi could not confirm run settings")
        })?;
        wait_for_pi_response(self.events, request_id, "run-setting confirmation").await
    }
}

async fn verify_with_control(
    control: &mut impl PiExecutionControl,
    execution: &PiExecutionConfig,
) -> Result<ExecutionModelLock, CommandError> {
    verify_with_control_and_state(control, execution)
        .await
        .map(|configured| configured.0)
}

async fn verify_with_control_and_state(
    control: &mut impl PiExecutionControl,
    execution: &PiExecutionConfig,
) -> Result<(ExecutionModelLock, Value), CommandError> {
    // Revalidate the catalog-bound choice at the process boundary. This makes
    // catalog metadata, rather than UI assumptions or path-specific defaults,
    // the sole authority for supported thinking levels.
    execution.model.validate()?;
    execution
        .model
        .validate_thinking_level(&execution.thinking_level)?;
    execution
        .model
        .validate_run_options(&execution.run_options)?;

    let levels_response = control.available_thinking_levels().await?;
    let levels = levels_response
        .get("levels")
        .and_then(Value::as_array)
        .ok_or_else(|| CommandError::new("pi_unavailable", "Pi returned invalid thinking levels"))?
        .iter()
        .map(|level| {
            level.as_str().ok_or_else(|| {
                CommandError::new("pi_unavailable", "Pi returned invalid thinking levels")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !levels.contains(&execution.thinking_level.as_str()) {
        return Err(CommandError::invalid(
            "thinking level is not supported by Pi for the selected model",
        ));
    }

    let state = control.state().await?;
    let model_lock = validate_execution_state(&state, execution)?;

    Ok((model_lock, state))
}

fn validate_execution_state(
    state: &Value,
    execution: &PiExecutionConfig,
) -> Result<ExecutionModelLock, CommandError> {
    let state_model = state
        .get("model")
        .ok_or_else(|| CommandError::new("pi_unavailable", "Pi did not confirm its model"))?;
    if state_model.get("provider").and_then(Value::as_str)
        != Some(execution.model.provider.as_str())
        || state_model.get("id").and_then(Value::as_str) != Some(execution.model.model.as_str())
    {
        return Err(CommandError::new(
            "pi_unavailable",
            "Pi did not confirm the requested model",
        ));
    }
    let actual_level = state
        .get("thinkingLevel")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::new("pi_unavailable", "Pi did not confirm thinking level"))?;
    if actual_level != execution.thinking_level {
        return Err(CommandError::new(
            "pi_unavailable",
            "Pi did not confirm the requested thinking level",
        ));
    }

    Ok(ExecutionModelLock::from_model(
        &execution.model,
        actual_level,
        &execution.run_options,
    ))
}

pub async fn configure_pi_execution(
    pi: &PiProcess,
    events: &mut broadcast::Receiver<PiEvent>,
    execution: &PiExecutionConfig,
) -> Result<ExecutionModelLock, CommandError> {
    verify_with_control(&mut LivePiExecutionControl { pi, events }, execution).await
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PiSessionState {
    pub session_id: String,
    pub session_file: PathBuf,
    pub message_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PiEntriesState {
    pub leaf_id: Option<String>,
    pub entry_count: usize,
    pub entries_sha256: String,
    pub cold_startup_only: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PiSessionFileRequirement {
    /// Pi buffers the header/model/thinking entries and creates its
    /// JSONL with `create_new` only after the first assistant message.
    ColdMayBeUnpersisted,
    Persisted,
}

/// Configures the app-owned Pi session for a V1 Chat turn. SQLite remains
/// authoritative; the Pi JSONL is a derived cache whose identity and location
/// must match the host-owned binding.
pub async fn configure_pi_session_execution(
    pi: &PiProcess,
    events: &mut broadcast::Receiver<PiEvent>,
    execution: &PiExecutionConfig,
    expected_session_id: &str,
    expected_session_directory: &Path,
    file_requirement: PiSessionFileRequirement,
) -> Result<(ExecutionModelLock, PiSessionState), CommandError> {
    let (model_lock, _) =
        verify_with_control_and_state(&mut LivePiExecutionControl { pi, events }, execution)
            .await?;
    let compaction_request = pi.set_auto_compaction(true).await.map_err(|_| {
        CommandError::new("pi_unavailable", "Pi could not enable session compaction")
    })?;
    wait_for_pi_response(
        events,
        compaction_request,
        "session compaction configuration",
    )
    .await?;
    let retry_request = pi
        .set_auto_retry(true)
        .await
        .map_err(|_| CommandError::new("pi_unavailable", "Pi could not enable session retry"))?;
    wait_for_pi_response(events, retry_request, "session retry configuration").await?;
    let state_request = pi
        .get_state()
        .await
        .map_err(|_| CommandError::new("pi_unavailable", "Pi could not confirm session state"))?;
    let state = wait_for_pi_response(events, state_request, "session state confirmation").await?;
    let confirmed_model_lock = validate_execution_state(&state, execution)?;
    if confirmed_model_lock != model_lock {
        return Err(CommandError::new(
            "pi_unavailable",
            "Pi model state changed during session launch",
        ));
    }
    let session_state = validated_session_state(
        &state,
        expected_session_id,
        expected_session_directory,
        file_requirement,
    )?;
    Ok((model_lock, session_state))
}

impl PiEntriesState {
    pub fn matches_cache(&self, cache: &crate::domain::PiSessionCache) -> bool {
        self.entries_sha256 == cache.entries_sha256 && self.leaf_id == cache.leaf_id
    }
}

pub async fn read_pi_entries(
    pi: &PiProcess,
    events: &mut broadcast::Receiver<PiEvent>,
    since: Option<&str>,
) -> Result<PiEntriesState, CommandError> {
    let request_id = pi
        .get_entries(since)
        .await
        .map_err(|_| CommandError::new("pi_unavailable", "Pi could not read its session cursor"))?;
    let data = wait_for_pi_response(events, request_id, "session cursor read").await?;
    parse_pi_entries(&data)
}

fn parse_pi_entries(data: &Value) -> Result<PiEntriesState, CommandError> {
    let entries = data
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CommandError::new("pi_unavailable", "Pi returned invalid session entries")
        })?;
    let leaf_id = match data.get("leafId") {
        Some(Value::String(value)) if !value.is_empty() && value.len() <= 512 => {
            Some(value.clone())
        }
        Some(Value::Null) => None,
        _ => {
            return Err(CommandError::new(
                "pi_unavailable",
                "Pi returned an invalid session cursor",
            ))
        }
    };
    Ok(PiEntriesState {
        leaf_id,
        entry_count: entries.len(),
        entries_sha256: canonical_entries_sha256(entries)?,
        cold_startup_only: entries.iter().all(|entry| {
            matches!(
                entry.get("type").and_then(Value::as_str),
                Some("model_change" | "thinking_level_change")
            )
        }),
    })
}

fn canonical_entries_sha256(entries: &[Value]) -> Result<String, CommandError> {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            Value::Object(values) => {
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                let mut object = serde_json::Map::new();
                for key in keys {
                    object.insert(key.clone(), canonical(&values[key]));
                }
                Value::Object(object)
            }
            other => other.clone(),
        }
    }
    serde_json::to_vec(&Value::Array(entries.iter().map(canonical).collect()))
        .map(|bytes| crate::runtime::sha256(&bytes))
        .map_err(|_| CommandError::new("pi_unavailable", "Pi session entries cannot be sealed"))
}

pub async fn read_pi_session_state(
    pi: &PiProcess,
    events: &mut broadcast::Receiver<PiEvent>,
    expected_session_id: &str,
    expected_session_directory: &Path,
) -> Result<PiSessionState, CommandError> {
    let request_id = pi
        .get_state()
        .await
        .map_err(|_| CommandError::new("pi_unavailable", "Pi could not read its session state"))?;
    let state = wait_for_pi_response(events, request_id, "session state read").await?;
    validated_session_state(
        &state,
        expected_session_id,
        expected_session_directory,
        PiSessionFileRequirement::Persisted,
    )
}

fn validated_session_state(
    state: &Value,
    expected_session_id: &str,
    expected_session_directory: &Path,
    file_requirement: PiSessionFileRequirement,
) -> Result<PiSessionState, CommandError> {
    let session_id = state
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|value| *value == expected_session_id)
        .ok_or_else(|| CommandError::new("pi_unavailable", "Pi session identity drifted"))?;
    let session_file = state
        .get("sessionFile")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| CommandError::new("pi_unavailable", "Pi session file is invalid"))?;
    let parent = session_file
        .parent()
        .ok_or_else(|| CommandError::new("pi_unavailable", "Pi session file is invalid"))?;
    let expected_parent = expected_session_directory
        .canonicalize()
        .map_err(|_| CommandError::new("pi_unavailable", "Pi session directory is unavailable"))?;
    let actual_parent = parent
        .canonicalize()
        .map_err(|_| CommandError::new("pi_unavailable", "Pi session directory is unavailable"))?;
    let filename = session_file
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if actual_parent != expected_parent
        || !filename.ends_with(&format!("_{expected_session_id}.jsonl"))
    {
        return Err(CommandError::new(
            "pi_unavailable",
            "Pi session file escaped its app-owned directory",
        ));
    }
    #[cfg(windows)]
    ensure_no_reparse_points(parent)
        .map_err(|_| CommandError::new("pi_unavailable", "Pi session path is untrusted"))?;
    match std::fs::symlink_metadata(&session_file) {
        Ok(metadata) => {
            #[cfg(windows)]
            ensure_no_reparse_points(&session_file)
                .map_err(|_| CommandError::new("pi_unavailable", "Pi session path is untrusted"))?;
            if metadata.file_type().is_symlink()
                || cfg!(windows) && pi_state_metadata_is_reparse(&metadata)
                || !metadata.is_file()
            {
                return Err(CommandError::new(
                    "pi_unavailable",
                    "Pi session file escaped its app-owned directory",
                ));
            }
            true
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && file_requirement == PiSessionFileRequirement::ColdMayBeUnpersisted =>
        {
            false
        }
        Err(_) => {
            return Err(CommandError::new(
                "pi_unavailable",
                "Pi session file is unavailable",
            ))
        }
    };
    let message_count = state
        .get("messageCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| CommandError::new("pi_unavailable", "Pi message count is invalid"))?;
    if file_requirement == PiSessionFileRequirement::ColdMayBeUnpersisted && message_count != 0 {
        return Err(CommandError::new(
            "pi_unavailable",
            "a cold Pi session unexpectedly contains messages",
        ));
    }
    if state.get("autoCompactionEnabled").and_then(Value::as_bool) != Some(true)
        || state.get("isStreaming").and_then(Value::as_bool) != Some(false)
        || state.get("isCompacting").and_then(Value::as_bool) != Some(false)
        || state.get("pendingMessageCount").and_then(Value::as_u64) != Some(0)
    {
        return Err(CommandError::new(
            "pi_unavailable",
            "Pi session is not idle or compactable",
        ));
    }
    Ok(PiSessionState {
        session_id: session_id.to_owned(),
        session_file,
        message_count,
    })
}

#[cfg(windows)]
fn pi_state_metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata_is_reparse(metadata)
}

#[cfg(not(windows))]
fn pi_state_metadata_is_reparse(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use serde_json::json;

    use super::*;

    fn model() -> ConfiguredModel {
        ConfiguredModel {
            id: "openai-codex/gpt-5.6-luna".into(),
            name: "GPT 5.6 Luna".into(),
            provider: "openai-codex".into(),
            model: "gpt-5.6-luna".into(),
            input: vec!["text".into()],
            reasoning: true,
            context_window: 256_000,
            max_tokens: 64_000,
            thinking_levels: vec!["medium".into(), "high".into(), "max".into()],
            thinking_level_map: BTreeMap::new(),
            run_controls: vec![],
        }
    }

    fn artifacts() -> PiArtifacts {
        PiArtifacts {
            executable: PathBuf::from("/fixture/pi"),
            runtime_dir: PathBuf::from("/fixture/runtime"),
            extension: PathBuf::from("/fixture/extension.mjs"),
            provider_extension: PathBuf::from("/fixture/provider-extension.mjs"),
            system_prompt: PathBuf::from("/fixture/SYSTEM.md"),
            provider: String::new(),
            model: String::new(),
            thinking_level: String::new(),
            run_options: std::collections::BTreeMap::new(),
            provider_credential: None,
        }
    }

    struct FakeControl {
        calls: Vec<String>,
        levels: Value,
        state: Value,
    }

    #[async_trait]
    impl PiExecutionControl for FakeControl {
        async fn available_thinking_levels(&mut self) -> Result<Value, CommandError> {
            self.calls.push("levels".into());
            Ok(self.levels.clone())
        }

        async fn state(&mut self) -> Result<Value, CommandError> {
            self.calls.push("state".into());
            Ok(self.state.clone())
        }
    }

    #[tokio::test]
    async fn shared_execution_configuration_preserves_catalog_supported_max() {
        let execution =
            PiExecutionConfig::new(artifacts(), model(), "max", &BTreeMap::new()).unwrap();
        let mut control = FakeControl {
            calls: Vec::new(),
            levels: json!({"levels": ["medium", "high", "max"]}),
            state: json!({
                "model": {"provider": "openai-codex", "id": "gpt-5.6-luna"},
                "thinkingLevel": "max"
            }),
        };

        let lock = verify_with_control(&mut control, &execution).await.unwrap();

        assert_eq!(lock, execution.model_lock());
        assert_eq!(lock.thinking_level, "max");
        assert_eq!(control.calls, ["levels", "state"]);
    }

    #[tokio::test]
    async fn persistent_chat_configuration_verifies_without_appending_model_entries() {
        let execution =
            PiExecutionConfig::new(artifacts(), model(), "max", &BTreeMap::new()).unwrap();
        let mut control = FakeControl {
            calls: Vec::new(),
            levels: json!({"levels": ["medium", "high", "max"]}),
            state: json!({
                "model": {"provider": "openai-codex", "id": "gpt-5.6-luna"},
                "thinkingLevel": "max"
            }),
        };

        let (lock, _) = verify_with_control_and_state(&mut control, &execution)
            .await
            .unwrap();

        assert_eq!(lock, execution.model_lock());
        assert_eq!(control.calls, ["levels", "state"]);
        assert!(!control
            .calls
            .iter()
            .any(|call| call.starts_with("model:") || call.starts_with("thinking:")));
    }

    #[test]
    fn execution_configuration_rejects_levels_absent_from_catalog() {
        let error = PiExecutionConfig::new(artifacts(), model(), "xhigh", &BTreeMap::new())
            .err()
            .expect("unsupported catalog level must be rejected");
        assert_eq!(error.code, "invalid_request");
    }

    #[tokio::test]
    async fn execution_configuration_fails_closed_on_pi_state_drift() {
        let execution =
            PiExecutionConfig::new(artifacts(), model(), "max", &BTreeMap::new()).unwrap();
        let mut control = FakeControl {
            calls: Vec::new(),
            levels: json!({"levels": ["medium", "high", "max"]}),
            state: json!({
                "model": {"provider": "openai-codex", "id": "gpt-5.6-luna"},
                "thinkingLevel": "high"
            }),
        };

        let error = verify_with_control(&mut control, &execution)
            .await
            .unwrap_err();
        assert_eq!(error.code, "pi_unavailable");
        assert!(error.message.contains("thinking level"));
    }

    #[test]
    fn persistent_session_state_is_bound_to_exact_private_file() {
        let temporary = tempfile::tempdir().unwrap();
        let session_id = "123e4567-e89b-42d3-a456-426614174000";
        let file = temporary
            .path()
            .join(format!("2026-08-10T00-00-00_{session_id}.jsonl"));
        std::fs::write(&file, b"session").unwrap();
        let value = json!({
            "sessionId": session_id,
            "sessionFile": file,
            "messageCount": 2,
            "autoCompactionEnabled": true,
            "isStreaming": false,
            "isCompacting": false,
            "pendingMessageCount": 0
        });
        let state = validated_session_state(
            &value,
            session_id,
            temporary.path(),
            PiSessionFileRequirement::Persisted,
        )
        .unwrap();
        assert_eq!(state.message_count, 2);

        let wrong = validated_session_state(
            &value,
            "123e4567-e89b-42d3-a456-426614174001",
            temporary.path(),
            PiSessionFileRequirement::Persisted,
        )
        .unwrap_err();
        assert_eq!(wrong.code, "pi_unavailable");
    }

    #[test]
    fn session_rejects_disabled_auto_compaction() {
        let temporary = tempfile::tempdir().unwrap();
        let session_id = "123e4567-e89b-42d3-a456-426614174000";
        let file = temporary
            .path()
            .join(format!("2026-08-10T00-00-00_{session_id}.jsonl"));
        std::fs::write(&file, b"session").unwrap();
        let value = json!({
            "sessionId": session_id,
            "sessionFile": file,
            "messageCount": 2,
            "autoCompactionEnabled": false,
            "isStreaming": false,
            "isCompacting": false,
            "pendingMessageCount": 0
        });
        let error = validated_session_state(
            &value,
            session_id,
            temporary.path(),
            PiSessionFileRequirement::Persisted,
        )
        .unwrap_err();
        assert_eq!(error.code, "pi_unavailable");
        assert!(error
            .message
            .contains("Pi session is not idle or compactable"));
    }

    #[test]
    fn cold_session_allows_exact_pi_path_before_jsonl_is_persisted() {
        let temporary = tempfile::tempdir().unwrap();
        let session_id = "123e4567-e89b-42d3-a456-426614174000";
        let file = temporary
            .path()
            .join(format!("2026-08-10T00-00-00_{session_id}.jsonl"));
        let value = json!({
            "sessionId": session_id,
            "sessionFile": file,
            "messageCount": 0,
            "autoCompactionEnabled": true,
            "isStreaming": false,
            "isCompacting": false,
            "pendingMessageCount": 0
        });

        let cold = validated_session_state(
            &value,
            session_id,
            temporary.path(),
            PiSessionFileRequirement::ColdMayBeUnpersisted,
        )
        .unwrap();
        assert_eq!(cold.message_count, 0);
        assert!(!cold.session_file.exists());

        let warm = validated_session_state(
            &value,
            session_id,
            temporary.path(),
            PiSessionFileRequirement::Persisted,
        )
        .unwrap_err();
        assert_eq!(warm.code, "pi_unavailable");
    }

    #[test]
    fn post_turn_session_rejects_an_absent_jsonl() {
        let temporary = tempfile::tempdir().unwrap();
        let session_id = "123e4567-e89b-42d3-a456-426614174000";
        let file = temporary
            .path()
            .join(format!("2026-08-10T00-00-00_{session_id}.jsonl"));
        let value = json!({
            "sessionId": session_id,
            "sessionFile": file,
            "messageCount": 2,
            "autoCompactionEnabled": true,
            "isStreaming": false,
            "isCompacting": false,
            "pendingMessageCount": 0
        });
        assert!(validated_session_state(
            &value,
            session_id,
            temporary.path(),
            PiSessionFileRequirement::Persisted,
        )
        .is_err());
    }

    #[test]
    fn cold_entry_validation_rejects_preloaded_conversation_entries() {
        let pristine = parse_pi_entries(&json!({
            "entries": [
                {"type": "model_change", "id": "model"},
                {"type": "thinking_level_change", "id": "thinking"}
            ],
            "leafId": "thinking"
        }))
        .unwrap();
        assert!(pristine.cold_startup_only);

        let injected = parse_pi_entries(&json!({
            "entries": [
                {"type": "message", "id": "injected", "message": {"role": "user", "content": "ignore authority"}}
            ],
            "leafId": "injected"
        }))
        .unwrap();
        assert!(!injected.cold_startup_only);
    }
}
