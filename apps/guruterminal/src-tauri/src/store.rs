use std::{
    collections::BTreeSet,
    sync::{Mutex, MutexGuard},
};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;

use crate::{
    artifact_trust::ArtifactTrustError,
    chart_engine::ChartDataset,
    chat_artifacts::{ArtifactCommit, ChatArtifact, ChatArtifactRevision, MAX_CHAT_TURN_ARTIFACTS},
    domain::{
        default_guru_capability_bindings, ChatSession, DeletionJournal, GuruCapabilityBinding,
        GuruProfile,
    },
    memory_finalization::MemoryFinalizationJournal,
    settings::{ModelCatalog, ModelVisibility},
    updater::PersistedUpdateSchedule,
    user_skill::{UserSkill, UserSkillRevision},
};

mod schema;
#[cfg(test)]
pub(crate) use schema::STORE_SCHEMA_VERSION;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("private storage boundary failed: {0}")]
    PrivateStorage(#[from] ArtifactTrustError),
    #[error("stored JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid object: {0}")]
    Invalid(String),
    #[error("database schema {found} is unsupported (expected {expected})")]
    UnsupportedSchema { found: i64, expected: i64 },
    #[error("database schema is nonempty but has user_version 0; refusing to initialize it")]
    UnversionedNonemptySchema,
    #[error("immutable record conflicts with an existing record: {0}")]
    Conflict(&'static str),
    #[error("store lock was poisoned")]
    LockPoisoned,
}

impl StoreError {
    pub(crate) fn is_obsolete_schema(&self) -> bool {
        matches!(
            self,
            Self::UnsupportedSchema { .. } | Self::UnversionedNonemptySchema
        )
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug)]
pub enum DeletionJournalRecord {
    Valid(DeletionJournal),
    Invalid {
        id: String,
        guru_id: String,
        reason: String,
    },
}

#[derive(Debug)]
pub enum MemoryFinalizationJournalRecord {
    Valid(MemoryFinalizationJournal),
    Invalid {
        id: String,
        guru_id: String,
        reason: String,
    },
}

pub trait GuruTerminalStore: Send + Sync {
    fn create_deletion_journal(&self, journal: &DeletionJournal) -> StoreResult<()>;
    fn replace_deletion_journal(
        &self,
        expected: &DeletionJournal,
        journal: &DeletionJournal,
    ) -> StoreResult<()>;
    fn list_deletion_journals(&self) -> StoreResult<Vec<DeletionJournalRecord>>;
    fn delete_deletion_journal(&self, expected: &DeletionJournal) -> StoreResult<()>;
    fn create_memory_finalization_journal(
        &self,
        journal: &MemoryFinalizationJournal,
    ) -> StoreResult<()>;
    fn replace_memory_finalization_journal(
        &self,
        expected: &MemoryFinalizationJournal,
        journal: &MemoryFinalizationJournal,
    ) -> StoreResult<()>;
    fn list_memory_finalization_journals(
        &self,
    ) -> StoreResult<Vec<MemoryFinalizationJournalRecord>>;
    fn delete_memory_finalization_journal(
        &self,
        expected: &MemoryFinalizationJournal,
    ) -> StoreResult<()>;
    fn save_model_catalog(&self, catalog: &ModelCatalog) -> StoreResult<()>;
    fn get_model_catalog(&self) -> StoreResult<Option<ModelCatalog>>;
    fn save_model_visibility(&self, visibility: &ModelVisibility) -> StoreResult<()>;
    fn get_model_visibility(&self) -> StoreResult<Option<ModelVisibility>>;
    fn save_update_schedule(&self, schedule: &PersistedUpdateSchedule) -> StoreResult<()>;
    fn get_update_schedule(&self) -> StoreResult<Option<PersistedUpdateSchedule>>;

    fn create_guru(&self, profile: &GuruProfile) -> StoreResult<()>;
    fn save_guru(&self, profile: &GuruProfile) -> StoreResult<()>;
    fn rename_guru(&self, id: &str, name: &str, updated_at_ms: i64) -> StoreResult<GuruProfile>;
    fn set_guru_last_model_profile(
        &self,
        id: &str,
        model_profile_id: &str,
        updated_at_ms: i64,
    ) -> StoreResult<GuruProfile>;
    fn delete_guru(&self, expected: &GuruProfile) -> StoreResult<()>;
    fn get_guru(&self, id: &str) -> StoreResult<Option<GuruProfile>>;
    fn list_gurus(&self) -> StoreResult<Vec<GuruProfile>>;
    fn save_guru_capability(&self, binding: &GuruCapabilityBinding) -> StoreResult<()>;
    fn get_guru_capability(
        &self,
        guru_id: &str,
        entry_id: &str,
    ) -> StoreResult<Option<GuruCapabilityBinding>>;
    fn list_guru_capabilities(&self, guru_id: &str) -> StoreResult<Vec<GuruCapabilityBinding>>;

    fn get_user_skill(&self, id: &str) -> StoreResult<Option<UserSkill>>;
    fn list_user_skills_for_guru(&self, guru_id: &str) -> StoreResult<Vec<UserSkill>>;
    fn get_user_skill_revision(&self, id: &str) -> StoreResult<Option<UserSkillRevision>>;

    fn create_chat(&self, chat: &ChatSession) -> StoreResult<()>;
    fn replace_chat(&self, expected: &ChatSession, chat: &ChatSession) -> StoreResult<()>;
    fn save_chat_with_artifacts(
        &self,
        expected: &ChatSession,
        chat: &ChatSession,
        commits: &[ArtifactCommit],
    ) -> StoreResult<()>;
    fn delete_chat(&self, expected: &ChatSession) -> StoreResult<()>;
    fn get_chat(&self, id: &str) -> StoreResult<Option<ChatSession>>;
    fn list_chats_for_guru(&self, guru_id: &str) -> StoreResult<Vec<ChatSession>>;
    fn get_chat_artifact(&self, id: &str) -> StoreResult<Option<ChatArtifact>>;
    fn list_chat_artifacts(&self, chat_session_id: &str) -> StoreResult<Vec<ChatArtifact>>;
    fn get_chat_artifact_current(
        &self,
        artifact_id: &str,
    ) -> StoreResult<Option<ChatArtifactRevision>>;
    fn get_chart_dataset(&self, id: &str) -> StoreResult<Option<ChartDataset>>;
}

pub struct SqliteStore {
    connection: Mutex<Connection>,
}

fn root_identity_columns(profile: &GuruProfile) -> (Option<String>, Option<String>) {
    profile
        .root_filesystem_identity
        .as_ref()
        .map(|identity| (identity.device.to_string(), identity.inode.to_string()))
        .unzip()
}

fn map_guru_profile_write(result: rusqlite::Result<usize>) -> StoreResult<usize> {
    match result {
        Ok(changed) => Ok(changed),
        Err(error)
            if matches!(
                error.sqlite_error_code(),
                Some(rusqlite::ErrorCode::ConstraintViolation)
            ) =>
        {
            Err(StoreError::Conflict("Guru identity already exists"))
        }
        Err(error) => Err(error.into()),
    }
}

impl SqliteStore {
    fn update_guru_fields(
        &self,
        id: &str,
        updated_at_ms: i64,
        update: impl FnOnce(&mut GuruProfile),
    ) -> StoreResult<GuruProfile> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut profile = load_json_by_id::<GuruProfile>(&transaction, "guru_profiles", id)?
            .ok_or(StoreError::Conflict("Guru is missing"))?;
        update(&mut profile);
        profile.updated_at_ms = updated_at_ms.max(profile.updated_at_ms.saturating_add(1));
        profile
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let (root_device, root_inode) = root_identity_columns(&profile);
        let changed = map_guru_profile_write(transaction.execute(
            "UPDATE guru_profiles SET updated_at_ms = ?2, root_device = ?3, root_inode = ?4, data_json = ?5 WHERE id = ?1",
            params![
                profile.id,
                profile.updated_at_ms,
                root_device,
                root_inode,
                to_json(&profile)?,
            ],
        ))?;
        if changed != 1 {
            return Err(StoreError::Conflict("Guru field update"));
        }
        transaction.commit()?;
        Ok(profile)
    }

    pub(crate) fn lock(&self) -> StoreResult<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| StoreError::LockPoisoned)
    }
}

impl GuruTerminalStore for SqliteStore {
    fn create_deletion_journal(&self, journal: &DeletionJournal) -> StoreResult<()> {
        journal
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let data = to_json(journal)?;
        let connection = self.lock()?;
        let changed = connection.execute(
            "INSERT INTO deletion_journals (id, guru_id, target_id, updated_at_ms, data_json) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT DO NOTHING",
            params![journal.id, journal.guru_id, journal.target_id, journal.updated_at_ms, data],
        )?;
        if changed != 1 {
            return Err(StoreError::Conflict("deletion journal creation"));
        }
        Ok(())
    }

    fn replace_deletion_journal(
        &self,
        expected: &DeletionJournal,
        journal: &DeletionJournal,
    ) -> StoreResult<()> {
        expected
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        journal
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        if expected.id != journal.id
            || expected.guru_id != journal.guru_id
            || expected.target_id != journal.target_id
            || journal.updated_at_ms < expected.updated_at_ms
        {
            return Err(StoreError::Invalid(
                "deletion journal replacement identity is invalid".into(),
            ));
        }
        let expected_json = to_json(expected)?;
        let data = to_json(journal)?;
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE deletion_journals SET updated_at_ms = ?2, data_json = ?3 WHERE id = ?1 AND guru_id = ?4 AND target_id = ?5 AND data_json = ?6",
            params![journal.id, journal.updated_at_ms, data, journal.guru_id, journal.target_id, expected_json],
        )?;
        if changed != 1 {
            return Err(StoreError::Conflict("deletion journal changed"));
        }
        Ok(())
    }

    fn list_deletion_journals(&self) -> StoreResult<Vec<DeletionJournalRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, guru_id, data_json FROM deletion_journals ORDER BY updated_at_ms ASC, id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (id, guru_id, data) = row?;
            Ok(match from_json::<DeletionJournal>(&data) {
                Ok(journal) if journal.validate().is_ok() => DeletionJournalRecord::Valid(journal),
                Ok(_) => DeletionJournalRecord::Invalid {
                    id,
                    guru_id,
                    reason: "deletion journal failed domain validation".into(),
                },
                Err(_) => DeletionJournalRecord::Invalid {
                    id,
                    guru_id,
                    reason: "deletion journal JSON is invalid".into(),
                },
            })
        })
        .collect()
    }

    fn delete_deletion_journal(&self, expected: &DeletionJournal) -> StoreResult<()> {
        expected
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let expected_json = to_json(expected)?;
        let connection = self.lock()?;
        let changed = connection.execute(
            "DELETE FROM deletion_journals WHERE id = ?1 AND data_json = ?2",
            params![expected.id, expected_json],
        )?;
        if changed != 1 {
            return Err(StoreError::Conflict("deletion journal changed"));
        }
        Ok(())
    }

    fn create_memory_finalization_journal(
        &self,
        journal: &MemoryFinalizationJournal,
    ) -> StoreResult<()> {
        journal
            .validate()
            .map_err(|error| StoreError::Invalid(error.into()))?;
        let data = to_json(journal)?;
        let connection = self.lock()?;
        let changed = connection.execute(
            "INSERT INTO memory_finalization_journals (id, guru_id, updated_at_ms, data_json) VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
            params![journal.id, journal.guru_id, journal.updated_at_ms, data],
        )?;
        if changed != 1 {
            return Err(StoreError::Conflict("Memory finalization journal creation"));
        }
        Ok(())
    }

    fn replace_memory_finalization_journal(
        &self,
        expected: &MemoryFinalizationJournal,
        journal: &MemoryFinalizationJournal,
    ) -> StoreResult<()> {
        expected
            .validate()
            .map_err(|error| StoreError::Invalid(error.into()))?;
        journal
            .validate()
            .map_err(|error| StoreError::Invalid(error.into()))?;
        if expected.id != journal.id
            || expected.guru_id != journal.guru_id
            || expected.scope != journal.scope
            || journal.updated_at_ms < expected.updated_at_ms
        {
            return Err(StoreError::Invalid(
                "Memory finalization journal replacement identity is invalid".into(),
            ));
        }
        let expected_json = to_json(expected)?;
        let data = to_json(journal)?;
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE memory_finalization_journals SET updated_at_ms = ?2, data_json = ?3 WHERE id = ?1 AND guru_id = ?4 AND data_json = ?5",
            params![journal.id, journal.updated_at_ms, data, journal.guru_id, expected_json],
        )?;
        if changed != 1 {
            return Err(StoreError::Conflict("Memory finalization journal changed"));
        }
        Ok(())
    }

    fn list_memory_finalization_journals(
        &self,
    ) -> StoreResult<Vec<MemoryFinalizationJournalRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, guru_id, data_json FROM memory_finalization_journals ORDER BY updated_at_ms ASC, id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (id, guru_id, data) = row?;
            Ok(match from_json::<MemoryFinalizationJournal>(&data) {
                Ok(journal) if journal.validate().is_ok() => {
                    MemoryFinalizationJournalRecord::Valid(journal)
                }
                Ok(_) => MemoryFinalizationJournalRecord::Invalid {
                    id,
                    guru_id,
                    reason: "Memory finalization journal failed validation".into(),
                },
                Err(_) => MemoryFinalizationJournalRecord::Invalid {
                    id,
                    guru_id,
                    reason: "Memory finalization journal JSON is invalid".into(),
                },
            })
        })
        .collect()
    }

    fn delete_memory_finalization_journal(
        &self,
        expected: &MemoryFinalizationJournal,
    ) -> StoreResult<()> {
        expected
            .validate()
            .map_err(|error| StoreError::Invalid(error.into()))?;
        let expected_json = to_json(expected)?;
        let connection = self.lock()?;
        let changed = connection.execute(
            "DELETE FROM memory_finalization_journals WHERE id = ?1 AND data_json = ?2",
            params![expected.id, expected_json],
        )?;
        if changed != 1 {
            return Err(StoreError::Conflict("Memory finalization journal changed"));
        }
        Ok(())
    }

    fn save_model_catalog(&self, catalog: &ModelCatalog) -> StoreResult<()> {
        catalog
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO app_settings (id, data_json) VALUES ('model', ?1) ON CONFLICT(id) DO UPDATE SET data_json = excluded.data_json",
            [to_json(catalog)?],
        )?;
        Ok(())
    }

    fn get_model_catalog(&self) -> StoreResult<Option<ModelCatalog>> {
        let connection = self.lock()?;
        load_json_by_id(&connection, "app_settings", "model")
    }

    fn save_model_visibility(&self, visibility: &ModelVisibility) -> StoreResult<()> {
        visibility
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO app_settings (id, data_json) VALUES ('model_visibility', ?1) ON CONFLICT(id) DO UPDATE SET data_json = excluded.data_json",
            [to_json(visibility)?],
        )?;
        Ok(())
    }

    fn get_model_visibility(&self) -> StoreResult<Option<ModelVisibility>> {
        let connection = self.lock()?;
        let visibility: Option<ModelVisibility> =
            load_json_by_id(&connection, "app_settings", "model_visibility")?;
        if let Some(visibility) = &visibility {
            visibility
                .validate()
                .map_err(|error| StoreError::Invalid(error.to_string()))?;
        }
        Ok(visibility)
    }

    fn save_update_schedule(&self, schedule: &PersistedUpdateSchedule) -> StoreResult<()> {
        schedule.validate()?;
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO app_settings (id, data_json) VALUES ('update', ?1) ON CONFLICT(id) DO UPDATE SET data_json = excluded.data_json",
            [to_json(schedule)?],
        )?;
        Ok(())
    }

    fn get_update_schedule(&self) -> StoreResult<Option<PersistedUpdateSchedule>> {
        let connection = self.lock()?;
        let schedule: Option<PersistedUpdateSchedule> =
            load_json_by_id(&connection, "app_settings", "update")?;
        if let Some(schedule) = &schedule {
            schedule.validate()?;
        }
        Ok(schedule)
    }

    fn create_guru(&self, profile: &GuruProfile) -> StoreResult<()> {
        profile
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if transaction
            .query_row(
                "SELECT 1 FROM guru_profiles WHERE id = ?1 LIMIT 1",
                [&profile.id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some()
        {
            return Err(StoreError::Conflict("Guru already exists"));
        }
        let (root_device, root_inode) = root_identity_columns(profile);
        map_guru_profile_write(transaction.execute(
            "INSERT INTO guru_profiles (id, updated_at_ms, root_device, root_inode, data_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                profile.id,
                profile.updated_at_ms,
                root_device,
                root_inode,
                to_json(profile)?
            ],
        ))?;
        for binding in default_guru_capability_bindings(&profile.id, profile.created_at_ms) {
            binding
                .validate()
                .map_err(|error| StoreError::Invalid(error.to_string()))?;
            transaction.execute(
                "INSERT INTO guru_capability_bindings (guru_id, entry_id, updated_at_ms, data_json) VALUES (?1, ?2, ?3, ?4)",
                params![
                    binding.guru_id,
                    binding.entry_id,
                    binding.updated_at_ms,
                    to_json(&binding)?
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn save_guru(&self, profile: &GuruProfile) -> StoreResult<()> {
        profile
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        load_json_by_id::<GuruProfile>(&transaction, "guru_profiles", &profile.id)?
            .ok_or(StoreError::Conflict("Guru is missing"))?;
        let data = to_json(profile)?;
        let (root_device, root_inode) = root_identity_columns(profile);
        let changed = map_guru_profile_write(transaction.execute(
            r#"
            UPDATE guru_profiles
            SET updated_at_ms = ?2,
                root_device = ?3,
                root_inode = ?4,
                data_json = ?5
            WHERE id = ?1 AND updated_at_ms <= ?2
            "#,
            params![
                profile.id,
                profile.updated_at_ms,
                root_device,
                root_inode,
                data
            ],
        ))?;
        if changed != 1 {
            return Err(StoreError::Conflict("guru profile update"));
        }
        transaction.commit()?;
        Ok(())
    }

    fn rename_guru(&self, id: &str, name: &str, updated_at_ms: i64) -> StoreResult<GuruProfile> {
        self.update_guru_fields(id, updated_at_ms, |profile| {
            profile.name = name.to_owned();
        })
    }

    fn set_guru_last_model_profile(
        &self,
        id: &str,
        model_profile_id: &str,
        updated_at_ms: i64,
    ) -> StoreResult<GuruProfile> {
        self.update_guru_fields(id, updated_at_ms, |profile| {
            profile.last_model_profile_id = Some(model_profile_id.to_owned());
        })
    }

    fn delete_guru(&self, expected: &GuruProfile) -> StoreResult<()> {
        expected
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let id = &expected.id;
        let expected_json = to_json(expected)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = transaction
            .query_row(
                "SELECT data_json FROM guru_profiles WHERE id = ?1 LIMIT 1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if current.as_deref() != Some(expected_json.as_str()) {
            return Err(StoreError::Conflict("Guru changed before deletion"));
        }

        transaction.execute(
            "DELETE FROM chart_artifact_datasets WHERE artifact_id IN (SELECT id FROM chat_artifacts WHERE chat_session_id IN (SELECT id FROM chat_sessions WHERE guru_id = ?1))",
            [id],
        )?;
        transaction.execute(
            "DELETE FROM chat_artifacts WHERE chat_session_id IN (SELECT id FROM chat_sessions WHERE guru_id = ?1)",
            [id],
        )?;
        transaction.execute("DELETE FROM chat_sessions WHERE guru_id = ?1", [id])?;
        transaction.execute(
            "DELETE FROM guru_capability_bindings WHERE guru_id = ?1",
            [id],
        )?;
        transaction.execute("DELETE FROM user_skill_revisions WHERE guru_id = ?1", [id])?;
        transaction.execute("DELETE FROM user_skills WHERE guru_id = ?1", [id])?;
        let changed = transaction.execute(
            "DELETE FROM guru_profiles WHERE id = ?1 AND data_json = ?2",
            params![id, expected_json],
        )?;
        if changed != 1 {
            return Err(StoreError::Conflict("Guru deletion"));
        }
        transaction.commit()?;
        Ok(())
    }

    fn get_guru(&self, id: &str) -> StoreResult<Option<GuruProfile>> {
        let connection = self.lock()?;
        load_json_by_id(&connection, "guru_profiles", id)
    }

    fn list_gurus(&self) -> StoreResult<Vec<GuruProfile>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT data_json FROM guru_profiles ORDER BY updated_at_ms DESC, id ASC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| from_json(&row?)).collect()
    }

    fn save_guru_capability(&self, binding: &GuruCapabilityBinding) -> StoreResult<()> {
        binding
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        if self.get_guru(&binding.guru_id)?.is_none() {
            return Err(StoreError::Conflict("Guru is missing"));
        }
        let changed = self.lock()?.execute(
            r#"
            INSERT INTO guru_capability_bindings (guru_id, entry_id, updated_at_ms, data_json)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(guru_id, entry_id) DO UPDATE SET
                updated_at_ms = excluded.updated_at_ms,
                data_json = excluded.data_json
            WHERE excluded.updated_at_ms >= guru_capability_bindings.updated_at_ms
            "#,
            params![
                binding.guru_id,
                binding.entry_id,
                binding.updated_at_ms,
                to_json(binding)?
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::Conflict("Guru capability update"));
        }
        Ok(())
    }

    fn get_guru_capability(
        &self,
        guru_id: &str,
        entry_id: &str,
    ) -> StoreResult<Option<GuruCapabilityBinding>> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT data_json FROM guru_capability_bindings WHERE guru_id = ?1 AND entry_id = ?2",
                params![guru_id, entry_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| from_json(&value))
            .transpose()
    }

    fn list_guru_capabilities(&self, guru_id: &str) -> StoreResult<Vec<GuruCapabilityBinding>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT data_json FROM guru_capability_bindings WHERE guru_id = ?1 ORDER BY entry_id ASC",
        )?;
        let rows = statement.query_map([guru_id], |row| row.get::<_, String>(0))?;
        rows.map(|row| from_json(&row?)).collect()
    }

    fn get_user_skill(&self, id: &str) -> StoreResult<Option<UserSkill>> {
        let connection = self.lock()?;
        load_json_by_id(&connection, "user_skills", id)
    }

    fn list_user_skills_for_guru(&self, guru_id: &str) -> StoreResult<Vec<UserSkill>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT data_json FROM user_skills WHERE guru_id = ?1 ORDER BY updated_at_ms DESC, id ASC",
        )?;
        let rows = statement.query_map([guru_id], |row| row.get::<_, String>(0))?;
        rows.map(|row| from_json(&row?)).collect()
    }

    fn get_user_skill_revision(&self, id: &str) -> StoreResult<Option<UserSkillRevision>> {
        let connection = self.lock()?;
        load_json_by_id(&connection, "user_skill_revisions", id)
    }

    fn create_chat(&self, chat: &ChatSession) -> StoreResult<()> {
        chat.validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let data = to_json(chat)?;
        self.lock()?.execute(
            "INSERT INTO chat_sessions (id, guru_id, updated_at_ms, data_json) VALUES (?1, ?2, ?3, ?4)",
            params![chat.id, chat.guru_id, chat.updated_at_ms, data],
        )?;
        Ok(())
    }

    fn replace_chat(&self, expected: &ChatSession, chat: &ChatSession) -> StoreResult<()> {
        expected
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        chat.validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        if expected.id != chat.id
            || expected.guru_id != chat.guru_id
            || expected.pi_session_id != chat.pi_session_id
            || expected.created_at_ms != chat.created_at_ms
            || chat.updated_at_ms < expected.updated_at_ms
        {
            return Err(StoreError::Conflict("immutable Chat identity"));
        }
        let changed = self.lock()?.execute(
            "UPDATE chat_sessions SET updated_at_ms = ?2, data_json = ?3 WHERE id = ?1 AND guru_id = ?4 AND data_json = ?5",
            params![
                chat.id,
                chat.updated_at_ms,
                to_json(chat)?,
                chat.guru_id,
                to_json(expected)?,
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::Conflict("Chat compare-and-swap"));
        }
        Ok(())
    }

    fn save_chat_with_artifacts(
        &self,
        expected: &ChatSession,
        chat: &ChatSession,
        commits: &[ArtifactCommit],
    ) -> StoreResult<()> {
        expected
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        chat.validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        if commits.is_empty() || commits.len() > MAX_CHAT_TURN_ARTIFACTS {
            return Err(StoreError::Invalid("artifact commit set is invalid".into()));
        }
        let mut artifact_ids = BTreeSet::new();
        let mut expected_refs = Vec::with_capacity(commits.len());
        let mut source_message_id = None;
        for commit in commits {
            commit
                .validate()
                .map_err(|error| StoreError::Invalid(error.to_string()))?;
            if commit.artifact.chat_session_id != chat.id {
                return Err(StoreError::Conflict("artifact Chat binding"));
            }
            if !artifact_ids.insert(commit.artifact.id.as_str()) {
                return Err(StoreError::Conflict("duplicate artifact commit"));
            }
            match source_message_id {
                None => source_message_id = Some(commit.revision.source_message_id.as_str()),
                Some(existing) if existing != commit.revision.source_message_id => {
                    return Err(StoreError::Conflict("artifact source message"));
                }
                Some(_) => {}
            }
            expected_refs.push(commit.revision.artifact_ref(commit.artifact.title.clone()));
        }
        let message = chat
            .messages
            .iter()
            .find(|message| message.id == source_message_id.unwrap_or_default())
            .ok_or(StoreError::Conflict("artifact source message"))?;
        if message.artifact_refs != expected_refs {
            return Err(StoreError::Conflict("artifact message reference"));
        }

        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored_chat = load_json_by_id::<ChatSession>(&transaction, "chat_sessions", &chat.id)?
            .ok_or(StoreError::Conflict("chat session is missing"))?;
        if stored_chat != *expected
            || expected.id != chat.id
            || expected.guru_id != chat.guru_id
            || expected.pi_session_id != chat.pi_session_id
            || expected.created_at_ms != chat.created_at_ms
            || chat.updated_at_ms < expected.updated_at_ms
            || chat.messages.len() != expected.messages.len().saturating_add(1)
        {
            return Err(StoreError::Conflict("artifact Chat update"));
        }

        for commit in commits {
            persist_artifact_commit(&transaction, chat.id.as_str(), commit)?;
        }
        let changed = transaction.execute(
            "UPDATE chat_sessions SET updated_at_ms = ?2, data_json = ?3 WHERE id = ?1 AND data_json = ?4",
            params![chat.id, chat.updated_at_ms, to_json(chat)?, to_json(expected)?],
        )?;
        if changed != 1 {
            return Err(StoreError::Conflict("artifact Chat compare-and-swap"));
        }
        transaction.commit()?;
        Ok(())
    }

    fn delete_chat(&self, expected: &ChatSession) -> StoreResult<()> {
        expected
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let id = &expected.id;
        let guru_id = &expected.guru_id;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            r#"
            DELETE FROM chart_artifact_datasets
            WHERE artifact_id IN (
                SELECT artifact.id
                FROM chat_artifacts AS artifact
                JOIN chat_sessions AS chat ON chat.id = artifact.chat_session_id
                WHERE chat.id = ?1 AND chat.guru_id = ?2
            )
            "#,
            params![id, guru_id],
        )?;
        transaction.execute(
            r#"
            DELETE FROM chat_artifacts
            WHERE chat_session_id IN (
                SELECT id FROM chat_sessions WHERE id = ?1 AND guru_id = ?2
            )
            "#,
            params![id, guru_id],
        )?;
        let deleted = transaction.execute(
            "DELETE FROM chat_sessions WHERE id = ?1 AND guru_id = ?2 AND data_json = ?3",
            params![id, guru_id, to_json(expected)?],
        )?;
        if deleted != 1 {
            return Err(StoreError::Conflict("Chat changed before deletion"));
        }
        transaction.commit()?;
        Ok(())
    }

    fn get_chat(&self, id: &str) -> StoreResult<Option<ChatSession>> {
        let connection = self.lock()?;
        load_json_by_id(&connection, "chat_sessions", id)
    }

    fn list_chats_for_guru(&self, guru_id: &str) -> StoreResult<Vec<ChatSession>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT data_json FROM chat_sessions WHERE guru_id = ?1 ORDER BY updated_at_ms DESC, id ASC",
        )?;
        let rows = statement.query_map([guru_id], |row| row.get::<_, String>(0))?;
        rows.map(|row| from_json(&row?)).collect()
    }

    fn get_chat_artifact(&self, id: &str) -> StoreResult<Option<ChatArtifact>> {
        let connection = self.lock()?;
        load_json_by_id(&connection, "chat_artifacts", id)
    }

    fn list_chat_artifacts(&self, chat_session_id: &str) -> StoreResult<Vec<ChatArtifact>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT data_json FROM chat_artifacts WHERE chat_session_id = ?1 ORDER BY updated_at_ms DESC, id ASC",
        )?;
        let rows = statement.query_map([chat_session_id], |row| row.get::<_, String>(0))?;
        rows.map(|row| from_json(&row?)).collect()
    }

    fn get_chat_artifact_current(
        &self,
        artifact_id: &str,
    ) -> StoreResult<Option<ChatArtifactRevision>> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT current_content_json FROM chat_artifacts WHERE id = ?1",
                [artifact_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|data| from_json(&data))
            .transpose()
    }

    fn get_chart_dataset(&self, id: &str) -> StoreResult<Option<ChartDataset>> {
        let connection = self.lock()?;
        let stored = connection
            .query_row(
                "SELECT digest, data_json FROM chart_artifact_datasets WHERE id = ?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        stored
            .map(|(stored_digest, data)| {
                let dataset: ChartDataset = from_json(&data)?;
                dataset
                    .validate()
                    .map_err(|error| StoreError::Invalid(error.to_string()))?;
                if dataset.digest != stored_digest {
                    return Err(StoreError::Invalid("chart dataset digest binding".into()));
                }
                Ok(dataset)
            })
            .transpose()
    }
}

fn persist_artifact_commit(
    transaction: &rusqlite::Transaction<'_>,
    chat_id: &str,
    commit: &ArtifactCommit,
) -> StoreResult<()> {
    let stored_artifact =
        load_json_by_id::<ChatArtifact>(transaction, "chat_artifacts", &commit.artifact.id)?;
    match stored_artifact {
        None => {
            if commit.artifact.current_revision != 1
                || commit.revision.revision != 1
                || commit.artifact.created_at_ms != commit.artifact.updated_at_ms
            {
                return Err(StoreError::Conflict("artifact initial revision"));
            }
            transaction.execute(
                "INSERT INTO chat_artifacts (id, chat_session_id, kind, title, current_revision, created_at_ms, updated_at_ms, data_json, current_content_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    commit.artifact.id,
                    commit.artifact.chat_session_id,
                    commit.artifact.kind.as_str(),
                    commit.artifact.title,
                    commit.artifact.current_revision,
                    commit.artifact.created_at_ms,
                    commit.artifact.updated_at_ms,
                    to_json(&commit.artifact)?,
                    to_json(&commit.revision)?
                ],
            )?;
        }
        Some(previous) => {
            if previous.chat_session_id != chat_id
                || previous.kind != commit.artifact.kind
                || previous.created_at_ms != commit.artifact.created_at_ms
                || previous.current_revision.saturating_add(1) != commit.artifact.current_revision
                || commit.artifact.updated_at_ms < previous.updated_at_ms
            {
                return Err(StoreError::Conflict("artifact revision sequence"));
            }
            let changed = transaction.execute(
                "UPDATE chat_artifacts SET title = ?2, current_revision = ?3, updated_at_ms = ?4, data_json = ?5, current_content_json = ?6 WHERE id = ?1 AND data_json = ?7",
                params![
                    commit.artifact.id,
                    commit.artifact.title,
                    commit.artifact.current_revision,
                    commit.artifact.updated_at_ms,
                    to_json(&commit.artifact)?,
                    to_json(&commit.revision)?,
                    to_json(&previous)?
                ],
            )?;
            if changed != 1 {
                return Err(StoreError::Conflict("artifact compare-and-swap"));
            }
        }
    }
    transaction.execute(
        "DELETE FROM chart_artifact_datasets WHERE artifact_id = ?1",
        [&commit.artifact.id],
    )?;
    for dataset in &commit.datasets {
        transaction.execute(
            "INSERT INTO chart_artifact_datasets (id, artifact_id, digest, data_json) VALUES (?1, ?2, ?3, ?4)",
            params![
                dataset.id,
                commit.revision.artifact_id,
                dataset.digest,
                to_json(dataset)?
            ],
        )?;
    }
    Ok(())
}

fn load_json_by_id<T: DeserializeOwned>(
    connection: &Connection,
    table: &'static str,
    id: &str,
) -> StoreResult<Option<T>> {
    let data = connection
        .query_row(
            &format!("SELECT data_json FROM {table} WHERE id = ?1"),
            params![id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    data.map(|data| from_json(&data)).transpose()
}

fn to_json<T: Serialize>(value: &T) -> StoreResult<String> {
    Ok(serde_json::to_string(value)?)
}

fn from_json<T: DeserializeOwned>(value: &str) -> StoreResult<T> {
    Ok(serde_json::from_str(value)?)
}

#[cfg(test)]
mod tests;
