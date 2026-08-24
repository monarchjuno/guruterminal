use serde::{Deserialize, Serialize};

use crate::{memory_git::MemoryGitSnapshot, runtime::StagedMemoryChange};

pub const MEMORY_FINALIZATION_SCHEMA_VERSION: u32 = 3;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryFinalizationScope {
    Chat {
        thread_id: String,
        message_id: String,
    },
    StandaloneUser,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryFinalizationJournal {
    pub schema_version: u32,
    pub id: String,
    pub guru_id: String,
    pub scope: MemoryFinalizationScope,
    pub updated_at_ms: i64,
    pub git: MemoryGitSnapshot,
    pub changes: Vec<StagedMemoryChange>,
    pub commit_id: Option<String>,
}

impl MemoryFinalizationJournal {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != MEMORY_FINALIZATION_SCHEMA_VERSION {
            return Err("Memory finalization journal schema is unsupported");
        }
        for value in [&self.id, &self.guru_id] {
            if value.is_empty() || value.len() > 512 || value.contains(['\0', '\n', '\r']) {
                return Err("Memory finalization journal identity is invalid");
            }
        }
        if let MemoryFinalizationScope::Chat {
            thread_id,
            message_id,
        } = &self.scope
        {
            for value in [thread_id, message_id] {
                if value.is_empty() || value.len() > 512 || value.contains(['\0', '\n', '\r']) {
                    return Err("Memory finalization journal Chat identity is invalid");
                }
            }
        }
        if self.updated_at_ms < 0 || self.changes.is_empty() || self.changes.len() > 24 {
            return Err("Memory finalization journal bounds are invalid");
        }
        if self.changes.iter().any(|change| {
            change.guru_id != self.guru_id
                || change.session_id != self.id
                || change.relative_path.as_os_str().is_empty()
        }) {
            return Err("Memory finalization journal change scope is invalid");
        }
        if self
            .git
            .previous_head
            .as_deref()
            .is_some_and(|oid| git2::Oid::from_str(oid).is_err())
            || git2::Oid::from_str(&self.git.original_index_tree).is_err()
            || self
                .git
                .published_index_tree
                .as_deref()
                .is_some_and(|oid| git2::Oid::from_str(oid).is_err())
            || self
                .commit_id
                .as_deref()
                .is_some_and(|oid| git2::Oid::from_str(oid).is_err())
            || self.commit_id.is_some() != self.git.published_index_tree.is_some()
        {
            return Err("Memory finalization journal Git identity is invalid");
        }
        Ok(())
    }
}
