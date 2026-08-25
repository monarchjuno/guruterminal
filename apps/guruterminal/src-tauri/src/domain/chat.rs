use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::chat_artifacts::{ChatArtifactRef, MAX_CHAT_TURN_ARTIFACTS};
use crate::chat_progress::ChatProgress;
use crate::settings::ExecutionModelLock;

use super::{
    hex_lower, memory_refs_digest, require_bounded_text, require_identifier, require_non_empty,
    required_option, validate_canonical_memory_record_id, validate_sha256_digest,
    CanonicalMemoryKind, DomainError, MemoryAccess, MemoryRefSnapshot, MAX_MEMORY_REFS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryPolicy {
    pub use_memory: bool,
    pub update_memory: bool,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            use_memory: true,
            update_memory: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageStatus {
    Complete,
    Aborted,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatDecision {
    pub payload: Value,
    pub digest: String,
    pub sealed_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryUpdateStatus {
    Applied,
    NoChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryUpdateChange {
    pub record_id: String,
    pub kind: String,
    pub operation: String,
    pub title: String,
    pub lesson: String,
    pub basis: String,
    pub future_use: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryUpdateResult {
    pub status: MemoryUpdateStatus,
    #[serde(deserialize_with = "required_option")]
    pub commit_id: Option<String>,
    pub changes: Vec<MemoryUpdateChange>,
}

impl MemoryUpdateResult {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.changes.len() > 18 {
            return Err(DomainError::Invalid("memory update has too many changes"));
        }
        let mut ids = BTreeSet::new();
        for change in &self.changes {
            validate_canonical_memory_record_id(&change.record_id, None)?;
            if CanonicalMemoryKind::from_label(&change.kind).is_none()
                || !matches!(change.operation.as_str(), "create" | "revise")
                || !ids.insert(change.record_id.as_str())
            {
                return Err(DomainError::Invalid("memory update change is invalid"));
            }
            require_bounded_text(&change.title, 512, "memory update title is invalid")?;
            require_bounded_text(&change.lesson, 2_048, "memory update lesson is invalid")?;
            require_bounded_text(&change.basis, 2_048, "memory update basis is invalid")?;
            require_bounded_text(
                &change.future_use,
                2_048,
                "memory update future use is invalid",
            )?;
        }
        match self.status {
            MemoryUpdateStatus::Applied => {
                if self.commit_id.as_deref().is_none_or(str::is_empty) || self.changes.is_empty() {
                    return Err(DomainError::Invalid("applied memory update is invalid"));
                }
            }
            MemoryUpdateStatus::NoChange => {
                if self.commit_id.is_some() || !self.changes.is_empty() {
                    return Err(DomainError::Invalid("unchanged memory update is invalid"));
                }
            }
        }
        Ok(())
    }
}

impl ChatDecision {
    pub fn validate(&self) -> Result<(), DomainError> {
        let object = self
            .payload
            .as_object()
            .filter(|object| object.len() == 8)
            .ok_or(DomainError::Invalid("chat decision payload is invalid"))?;
        let required = [
            "stance",
            "horizon",
            "probability",
            "thesis",
            "evidence_ids",
            "uses_ids",
            "risks",
            "invalidation_conditions",
        ];
        if required.iter().any(|key| !object.contains_key(*key))
            || !object
                .get("stance")
                .and_then(Value::as_str)
                .is_some_and(|value| {
                    matches!(value, "positive" | "neutral" | "negative" | "abstain")
                })
            || !object
                .get("probability")
                .and_then(Value::as_f64)
                .is_some_and(|value| (0.0..=1.0).contains(&value))
        {
            return Err(DomainError::Invalid("chat decision payload is invalid"));
        }
        for key in ["horizon", "thesis"] {
            let value = object
                .get(key)
                .and_then(Value::as_str)
                .ok_or(DomainError::Invalid("chat decision text is invalid"))?;
            require_bounded_text(value, 16 * 1024, "chat decision text is invalid")?;
        }
        for (key, limit) in [
            ("evidence_ids", 64_usize),
            ("uses_ids", 32_usize),
            ("risks", 128),
            ("invalidation_conditions", 128),
        ] {
            let values = object
                .get(key)
                .and_then(Value::as_array)
                .filter(|values| values.len() <= limit)
                .ok_or(DomainError::Invalid("chat decision list is invalid"))?;
            let mut unique = BTreeSet::new();
            for value in values {
                let value = value
                    .as_str()
                    .ok_or(DomainError::Invalid("chat decision list is invalid"))?;
                require_bounded_text(value, 16 * 1024, "chat decision list is invalid")?;
                if !unique.insert(value) {
                    return Err(DomainError::Invalid(
                        "chat decision list contains duplicates",
                    ));
                }
            }
        }
        let stance = object
            .get("stance")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let evidence_is_empty = object
            .get("evidence_ids")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty);
        if stance != "abstain" && evidence_is_empty {
            return Err(DomainError::Invalid("chat decision evidence is empty"));
        }
        let encoded = serde_json::to_vec(&self.payload)
            .map_err(|_| DomainError::Invalid("chat decision payload is invalid"))?;
        if self.sealed_at_ms < 0 || self.digest != hex_lower(&Sha256::digest(&encoded)) {
            return Err(DomainError::Invalid("chat decision seal is invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatMessage {
    pub id: String,
    pub role: ChatRole,
    pub status: ChatMessageStatus,
    pub content: String,
    pub created_at_ms: i64,
    pub memory_refs: Vec<MemoryRefSnapshot>,
    pub observed_exact_count: u64,
    pub refs_truncated: bool,
    pub refs_digest: String,
    #[serde(deserialize_with = "required_option")]
    pub memory_update: Option<MemoryUpdateResult>,
    #[serde(deserialize_with = "required_option")]
    pub memory_revision: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub execution_model: Option<ExecutionModelLock>,
    #[serde(deserialize_with = "required_option")]
    pub agent_harness: Option<crate::agent_harness::AgentHarnessSnapshot>,
    #[serde(deserialize_with = "required_option")]
    pub decision: Option<ChatDecision>,
    pub attachments: Vec<ChatAttachment>,
    pub artifact_refs: Vec<ChatArtifactRef>,
    #[serde(default)]
    pub progress: Option<ChatProgress>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatAttachment {
    pub id: String,
    pub filename: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub content_sha256: String,
}

impl ChatAttachment {
    pub fn validate(&self) -> Result<(), DomainError> {
        require_identifier(&self.id, "chat attachment id is empty or unsafe")?;
        require_bounded_text(&self.filename, 255, "chat attachment filename is invalid")?;
        if self.filename.contains(['/', '\\', '\n', '\r'])
            || self.filename.chars().any(char::is_control)
        {
            return Err(DomainError::Invalid("chat attachment filename is invalid"));
        }
        require_bounded_text(
            &self.media_type,
            127,
            "chat attachment media type is invalid",
        )?;
        if !self
            .media_type
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'+' | b'-' | b'.'))
            || !self.media_type.contains('/')
        {
            return Err(DomainError::Invalid(
                "chat attachment media type is invalid",
            ));
        }
        if self.size_bytes == 0 || self.size_bytes > 5 * 1024 * 1024 {
            return Err(DomainError::Invalid("chat attachment size is invalid"));
        }
        validate_sha256_digest(&self.content_sha256, "chat attachment digest is invalid")
    }
}

impl ChatMessage {
    pub fn validate(&self) -> Result<(), DomainError> {
        require_identifier(&self.id, "chat message id is empty or unsafe")?;
        if self.content.trim().is_empty() && self.attachments.is_empty() {
            return Err(DomainError::Invalid("chat message content is empty"));
        }
        if self.created_at_ms < 0 {
            return Err(DomainError::Invalid("chat message timestamp is invalid"));
        }
        if self.memory_refs.len() > MAX_MEMORY_REFS {
            return Err(DomainError::Invalid(
                "chat message has too many memory references",
            ));
        }
        let mut record_ids = BTreeSet::new();
        for memory in &self.memory_refs {
            memory.validate()?;
            if memory.access != MemoryAccess::ExactRead {
                return Err(DomainError::Invalid(
                    "durable chat memory references must be exact reads",
                ));
            }
            if !record_ids.insert(memory.record_id.as_str()) {
                return Err(DomainError::Invalid(
                    "chat message contains duplicate memory references",
                ));
            }
        }
        let retained_count = u64::try_from(self.memory_refs.len())
            .map_err(|_| DomainError::Invalid("chat memory reference count is invalid"))?;
        if self.observed_exact_count < retained_count
            || self.refs_truncated != (self.observed_exact_count > retained_count)
        {
            return Err(DomainError::Invalid(
                "chat memory reference summary is inconsistent",
            ));
        }
        validate_sha256_digest(&self.refs_digest, "chat memory reference digest is invalid")?;
        if !self.refs_truncated && self.refs_digest != memory_refs_digest(&self.memory_refs)? {
            return Err(DomainError::Invalid(
                "chat memory reference digest does not match retained references",
            ));
        }
        if let Some(update) = &self.memory_update {
            update.validate()?;
            if self.role != ChatRole::Assistant {
                return Err(DomainError::Invalid(
                    "only assistant chat messages may contain a memory update",
                ));
            }
        }
        if self.memory_revision.as_deref().is_some_and(|value| {
            value.is_empty() || value.len() > 256 || value.contains(['\0', '\n', '\r'])
        }) {
            return Err(DomainError::Invalid(
                "chat message memory revision is invalid",
            ));
        }
        if self
            .execution_model
            .as_ref()
            .is_some_and(|model| model.validate().is_err())
        {
            return Err(DomainError::Invalid("chat message model lock is invalid"));
        }
        if self
            .agent_harness
            .as_ref()
            .is_some_and(|harness| harness.validate().is_err())
        {
            return Err(DomainError::Invalid("chat message harness lock is invalid"));
        }
        if self.execution_model.is_some() != self.agent_harness.is_some() {
            return Err(DomainError::Invalid(
                "chat message model and harness locks must be paired",
            ));
        }
        if self.role == ChatRole::User && self.execution_model.is_some() {
            return Err(DomainError::Invalid(
                "user chat message cannot have a model lock",
            ));
        }
        if matches!(
            self.status,
            ChatMessageStatus::Aborted | ChatMessageStatus::Error
        ) {
            if self.role != ChatRole::Assistant {
                return Err(DomainError::Invalid(
                    "only assistant chat messages may be terminal failures",
                ));
            }
            if self.memory_update.is_some()
                || self.decision.is_some()
                || !self.artifact_refs.is_empty()
            {
                return Err(DomainError::Invalid(
                    "terminally failed chat messages cannot publish durable outputs",
                ));
            }
        }
        if let Some(decision) = &self.decision {
            decision.validate()?;
            if self.role != ChatRole::Assistant {
                return Err(DomainError::Invalid(
                    "only assistant chat messages may contain a decision",
                ));
            }
        }
        if self.attachments.len() > 4 {
            return Err(DomainError::Invalid(
                "chat message contains too many attachments",
            ));
        }
        let mut attachment_ids = BTreeSet::new();
        for attachment in &self.attachments {
            attachment.validate()?;
            if !attachment_ids.insert(attachment.id.as_str()) {
                return Err(DomainError::Invalid(
                    "chat message contains duplicate attachments",
                ));
            }
        }
        if self.role != ChatRole::User && !self.attachments.is_empty() {
            return Err(DomainError::Invalid(
                "only user chat messages may contain attachments",
            ));
        }
        if self.artifact_refs.len() > MAX_CHAT_TURN_ARTIFACTS {
            return Err(DomainError::Invalid(
                "chat message contains too many artifact references",
            ));
        }
        let mut artifact_ids = BTreeSet::new();
        for artifact in &self.artifact_refs {
            artifact
                .validate()
                .map_err(|_| DomainError::Invalid("chat artifact reference is invalid"))?;
            if !artifact_ids.insert(artifact.artifact_id.as_str()) {
                return Err(DomainError::Invalid(
                    "chat message contains duplicate artifact references",
                ));
            }
        }
        if self.role == ChatRole::User && !self.artifact_refs.is_empty() {
            return Err(DomainError::Invalid(
                "user chat message cannot publish an artifact",
            ));
        }
        if let Some(progress) = &self.progress {
            if self.role != ChatRole::Assistant {
                return Err(DomainError::Invalid(
                    "only assistant chat messages may contain progress",
                ));
            }
            progress.validate(true)?;
        }
        Ok(())
    }
}

/// Digest-bound derived Pi JSONL cache for one Chat thread.
/// SQLite remains the transcript authority; a mismatch forces cold rebuild.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PiSessionCache {
    pub entries_sha256: String,
    #[serde(deserialize_with = "required_option")]
    pub leaf_id: Option<String>,
    pub harness_digest: String,
    /// The stable app-owned Pi execution policy. This includes transport,
    /// CWD, extension, and session-format compatibility.
    ///
    /// Caches written by an earlier host-context seal remain readable but
    /// miss the additional surface and authority seals below, so they rebuild
    /// cold without rejecting the canonical Chat.
    #[serde(default, alias = "hostContextSha256")]
    pub runtime_policy_sha256: Option<String>,
    /// Hash of the static runtime/component surface. This makes capability
    /// routing, component descriptions, and tool mappings exact across a
    /// resumed Pi JSONL, independent of per-turn Memory flags.
    #[serde(default)]
    pub runtime_surface_sha256: Option<String>,
    /// Digest of the non-secret connector authority snapshot: every binding,
    /// global connector config revision, and active credential revision. Pi
    /// JSONL can retain tool results, so a connector authority change always
    /// forces a cold rebuild even when the component IDs remain unchanged.
    #[serde(default)]
    pub connector_authority_sha256: Option<String>,
    /// Authority carried by the cached JSONL. A fresh Pi launch reinstalls a
    /// runtime profile with these flags, so all Memory controls must match
    /// exactly before we restore its prior tool-result context. Any change
    /// rebuilds from the canonical SQLite transcript.
    #[serde(default)]
    pub memory_access_enabled: Option<bool>,
    #[serde(default)]
    pub memory_update_enabled: Option<bool>,
    /// The effective Pi/provider session ID for this derived JSONL cache.
    /// It rotates whenever the cache is rebuilt cold, so a reconstructed
    /// SQLite transcript is not sent under a stale provider cache identity.
    #[serde(default)]
    pub derived_session_id: Option<String>,
    pub execution_model: ExecutionModelLock,
}

/// The non-secret execution inputs that must match before Pi JSONL tool
/// results can be resumed for another Chat turn.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PiSessionCacheScope<'a> {
    pub harness_digest: &'a str,
    pub runtime_policy_sha256: &'a str,
    pub runtime_surface_sha256: &'a str,
    pub connector_authority_sha256: &'a str,
    pub memory_access_enabled: bool,
    pub memory_update_enabled: bool,
    pub execution_model: &'a ExecutionModelLock,
}

impl PiSessionCache {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_sha256_digest(
            &self.entries_sha256,
            "chat Pi session cache digest is invalid",
        )?;
        if let Some(leaf_id) = &self.leaf_id {
            if leaf_id.is_empty() || leaf_id.len() > 512 || leaf_id.contains('\0') {
                return Err(DomainError::Invalid(
                    "chat Pi session cache leaf is invalid",
                ));
            }
        }
        validate_sha256_digest(
            &self.harness_digest,
            "chat Pi session cache harness digest is invalid",
        )?;
        if let Some(runtime_policy_sha256) = &self.runtime_policy_sha256 {
            validate_sha256_digest(
                runtime_policy_sha256,
                "chat Pi session cache runtime policy digest is invalid",
            )?;
        }
        if let Some(runtime_surface_sha256) = &self.runtime_surface_sha256 {
            validate_sha256_digest(
                runtime_surface_sha256,
                "chat Pi session cache runtime surface digest is invalid",
            )?;
        }
        if let Some(connector_authority_sha256) = &self.connector_authority_sha256 {
            validate_sha256_digest(
                connector_authority_sha256,
                "chat Pi session cache connector authority digest is invalid",
            )?;
        }
        if let Some(derived_session_id) = &self.derived_session_id {
            if Uuid::parse_str(derived_session_id)
                .ok()
                .map(|id| id.to_string())
                .as_deref()
                != Some(derived_session_id.as_str())
            {
                return Err(DomainError::Invalid(
                    "chat Pi session cache derived session id is invalid",
                ));
            }
        }
        if self.execution_model.validate().is_err() {
            return Err(DomainError::Invalid(
                "chat Pi session cache model lock is invalid",
            ));
        }
        Ok(())
    }

    pub(crate) fn matches(&self, scope: &PiSessionCacheScope<'_>) -> bool {
        self.harness_digest == scope.harness_digest
            && self.runtime_policy_sha256.as_deref() == Some(scope.runtime_policy_sha256)
            && self.runtime_surface_sha256.as_deref() == Some(scope.runtime_surface_sha256)
            && self.connector_authority_sha256.as_deref() == Some(scope.connector_authority_sha256)
            && self.memory_access_enabled == Some(scope.memory_access_enabled)
            && self.memory_update_enabled == Some(scope.memory_update_enabled)
            && self.derived_session_id.is_some()
            && &self.execution_model == scope.execution_model
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatSession {
    pub id: String,
    pub guru_id: String,
    /// Stable identity of the derived Pi execution session for this Chat.
    /// SQLite remains canonical; Pi's JSONL is a digest-bound derived cache.
    pub pi_session_id: String,
    #[serde(default)]
    pub pi_session_cache: Option<PiSessionCache>,
    pub title: String,
    pub memory_policy: MemoryPolicy,
    pub messages: Vec<ChatMessage>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl ChatSession {
    pub fn validate(&self) -> Result<(), DomainError> {
        require_identifier(&self.id, "chat session id is empty or unsafe")?;
        require_identifier(&self.guru_id, "chat guru id is empty or unsafe")?;
        if Uuid::parse_str(&self.pi_session_id)
            .ok()
            .map(|value| value.to_string())
            .as_deref()
            != Some(self.pi_session_id.as_str())
        {
            return Err(DomainError::Invalid("chat Pi session id is invalid"));
        }
        if let Some(cache) = &self.pi_session_cache {
            cache.validate()?;
        }
        require_non_empty(&self.title, "chat title is empty")?;
        if self.created_at_ms < 0 || self.updated_at_ms < self.created_at_ms {
            return Err(DomainError::Invalid("chat timestamps are invalid"));
        }

        let mut ids = BTreeSet::new();
        for message in &self.messages {
            message.validate()?;
            if !ids.insert(message.id.as_str()) {
                return Err(DomainError::Invalid("duplicate chat message id"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ExecutionModelLock;
    use std::collections::BTreeMap;

    const DIGEST_A: &str = concat!(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    const DIGEST_B: &str = concat!(
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
    const DIGEST_C: &str = concat!(
        "cccccccccccccccccccccccccccccccc",
        "cccccccccccccccccccccccccccccccc"
    );
    const DIGEST_D: &str = concat!(
        "dddddddddddddddddddddddddddddddd",
        "dddddddddddddddddddddddddddddddd"
    );
    const DIGEST_E: &str = concat!(
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
    );
    const DIGEST_F: &str = concat!(
        "ffffffffffffffffffffffffffffffff",
        "ffffffffffffffffffffffffffffffff"
    );

    fn model_lock() -> ExecutionModelLock {
        ExecutionModelLock {
            profile_id: "openai-codex/gpt-5.6-luna".into(),
            name: "GPT 5.6 Luna".into(),
            provider: "openai-codex".into(),
            model: "gpt-5.6-luna".into(),
            thinking_level: "max".into(),
            run_options: BTreeMap::new(),
        }
    }

    fn cache_scope<'a>(execution_model: &'a ExecutionModelLock) -> PiSessionCacheScope<'a> {
        PiSessionCacheScope {
            harness_digest: DIGEST_B,
            runtime_policy_sha256: DIGEST_C,
            runtime_surface_sha256: DIGEST_D,
            connector_authority_sha256: DIGEST_F,
            memory_access_enabled: false,
            memory_update_enabled: false,
            execution_model,
        }
    }

    #[test]
    fn session_cache_matches_only_the_sealed_surface_authority_and_model() {
        let model = model_lock();
        let scope = cache_scope(&model);
        let cache = PiSessionCache {
            entries_sha256: DIGEST_A.into(),
            leaf_id: Some("leaf-1".into()),
            harness_digest: DIGEST_B.into(),
            runtime_policy_sha256: Some(DIGEST_C.into()),
            runtime_surface_sha256: Some(DIGEST_D.into()),
            connector_authority_sha256: Some(DIGEST_F.into()),
            memory_access_enabled: Some(false),
            memory_update_enabled: Some(false),
            derived_session_id: Some("123e4567-e89b-42d3-a456-426614174000".into()),
            execution_model: model.clone(),
        };
        cache.validate().unwrap();
        assert!(cache.matches(&scope));
        assert!(!cache.matches(&PiSessionCacheScope {
            harness_digest: DIGEST_E,
            ..scope
        }));
        assert!(!cache.matches(&PiSessionCacheScope {
            runtime_policy_sha256: DIGEST_E,
            ..scope
        }));
        assert!(!cache.matches(&PiSessionCacheScope {
            runtime_surface_sha256: DIGEST_E,
            ..scope
        }));
        assert!(!cache.matches(&PiSessionCacheScope {
            connector_authority_sha256: DIGEST_E,
            ..scope
        }));
        let mut other = model_lock();
        other.thinking_level = "high".into();
        assert!(!cache.matches(&PiSessionCacheScope {
            execution_model: &other,
            ..scope
        }));
        let mut legacy = cache.clone();
        legacy.connector_authority_sha256 = None;
        assert!(legacy.validate().is_ok());
        assert!(!legacy.matches(&scope));
    }

    #[test]
    fn session_cache_requires_an_exact_memory_authority_profile() {
        let profiles = [(false, false), (false, true), (true, false), (true, true)];
        for (cached_access, cached_update) in profiles {
            let model = model_lock();
            let scope = cache_scope(&model);
            let cache = PiSessionCache {
                entries_sha256: DIGEST_A.into(),
                leaf_id: Some("leaf-1".into()),
                harness_digest: DIGEST_B.into(),
                runtime_policy_sha256: Some(DIGEST_C.into()),
                runtime_surface_sha256: Some(DIGEST_D.into()),
                connector_authority_sha256: Some(DIGEST_F.into()),
                memory_access_enabled: Some(cached_access),
                memory_update_enabled: Some(cached_update),
                derived_session_id: Some("123e4567-e89b-42d3-a456-426614174000".into()),
                execution_model: model.clone(),
            };
            for (current_access, current_update) in profiles {
                assert_eq!(
                    cache.matches(&PiSessionCacheScope {
                        memory_access_enabled: current_access,
                        memory_update_enabled: current_update,
                        ..scope
                    }),
                    (cached_access, cached_update) == (current_access, current_update),
                    "cached ({cached_access}, {cached_update}) vs current ({current_access}, {current_update})",
                );
            }
        }
    }

    #[test]
    fn legacy_session_cache_rebuilds_cold_without_rejecting_the_chat() {
        let model = model_lock();
        let cache = PiSessionCache {
            entries_sha256: DIGEST_A.into(),
            leaf_id: Some("leaf-1".into()),
            harness_digest: DIGEST_B.into(),
            runtime_policy_sha256: None,
            runtime_surface_sha256: None,
            connector_authority_sha256: None,
            memory_access_enabled: None,
            memory_update_enabled: None,
            derived_session_id: None,
            execution_model: model.clone(),
        };
        cache.validate().unwrap();
        assert!(!cache.matches(&PiSessionCacheScope {
            memory_access_enabled: true,
            memory_update_enabled: true,
            ..cache_scope(&model)
        }));
    }

    #[test]
    fn previous_host_context_seal_is_loaded_then_rebuilt_cold() {
        let model = model_lock();
        let cache = PiSessionCache {
            entries_sha256: DIGEST_A.into(),
            leaf_id: Some("leaf-1".into()),
            harness_digest: DIGEST_B.into(),
            runtime_policy_sha256: Some(DIGEST_C.into()),
            runtime_surface_sha256: None,
            connector_authority_sha256: None,
            memory_access_enabled: None,
            memory_update_enabled: None,
            derived_session_id: None,
            execution_model: model.clone(),
        };
        let mut encoded = serde_json::to_value(cache).unwrap();
        let fields = encoded.as_object_mut().unwrap();
        let old_digest = fields.remove("runtimePolicySha256").unwrap();
        fields.insert("hostContextSha256".into(), old_digest);

        let restored: PiSessionCache = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored.runtime_policy_sha256, Some(DIGEST_C.into()));
        assert!(!restored.matches(&PiSessionCacheScope {
            runtime_policy_sha256: DIGEST_D,
            runtime_surface_sha256: DIGEST_E,
            memory_access_enabled: true,
            memory_update_enabled: true,
            ..cache_scope(&model)
        }));
    }
}
