use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::{SqliteStore, StoreError, StoreResult};
use crate::artifact_trust::{
    ensure_private_directory, ensure_private_regular_file, harden_private_regular_file_if_exists,
    ArtifactTrustError,
};

pub(crate) const STORE_SCHEMA_VERSION: i64 = 9;
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn sqlite_side_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn harden_sqlite_side_files(path: &Path) -> StoreResult<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        harden_private_regular_file_if_exists(&sqlite_side_path(path, suffix))?;
    }
    Ok(())
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref();
        let parent = path
            .parent()
            .ok_or_else(|| StoreError::Invalid("SQLite path has no parent".into()))?;
        ensure_private_directory(parent)?;
        let file_name = path
            .file_name()
            .ok_or_else(|| StoreError::Invalid("SQLite path has no file name".into()))?;
        // SQLite's NOFOLLOW flag rejects a symlink in any path component. Use
        // the already-opened app-data directory's canonical spelling so normal
        // platform aliases such as macOS `/var` -> `/private/var` do not make a
        // private database unusable, while the database leaf still cannot be a
        // symlink.
        let canonical_path = parent
            .canonicalize()
            .map_err(ArtifactTrustError::from)?
            .join(file_name);
        ensure_private_regular_file(&canonical_path)?;
        harden_sqlite_side_files(&canonical_path)?;
        let connection = Connection::open_with_flags(
            &canonical_path,
            OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        let store = Self::from_connection(connection)?;
        ensure_private_regular_file(&canonical_path)?;
        harden_sqlite_side_files(&canonical_path)?;
        Ok(store)
    }

    pub fn open_or_replace_obsolete(path: impl AsRef<Path>) -> StoreResult<(Self, bool)> {
        let path = path.as_ref();
        let existed = path.exists();
        match Self::open(path) {
            Ok(store) => Ok((store, !existed)),
            Err(error) if error.is_obsolete_schema() => {
                eprintln!(
                    "Guru Terminal discarded obsolete local database {} ({error}); creating schema {STORE_SCHEMA_VERSION}.",
                    path.display()
                );
                Self::remove_database_files(path)?;
                Ok((Self::open(path)?, true))
            }
            Err(error) => Err(error),
        }
    }

    fn remove_database_files(path: &Path) -> StoreResult<()> {
        let path = path
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .and_then(|parent| path.file_name().map(|name| parent.join(name)))
            .unwrap_or_else(|| path.to_path_buf());
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let candidate = if suffix.is_empty() {
                path.clone()
            } else {
                sqlite_side_path(&path, suffix)
            };
            match std::fs::remove_file(&candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(StoreError::Invalid(format!(
                        "could not remove obsolete database {}: {error}",
                        candidate.display()
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> StoreResult<Self> {
        connection.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
        let version =
            connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        match version {
            0 => {
                let has_schema = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema LIMIT 1)",
                    [],
                    |row| row.get::<_, bool>(0),
                )?;
                if has_schema {
                    return Err(StoreError::UnversionedNonemptySchema);
                }
            }
            STORE_SCHEMA_VERSION => {}
            _ => {
                return Err(StoreError::UnsupportedSchema {
                    found: version,
                    expected: STORE_SCHEMA_VERSION,
                });
            }
        }
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        if version == 0 {
            Self::initialize_schema(&connection)?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn initialize_schema(connection: &Connection) -> StoreResult<()> {
        connection.execute_batch(
            r#"
            BEGIN IMMEDIATE;
            CREATE TABLE app_settings (
                id TEXT PRIMARY KEY NOT NULL,
                data_json TEXT NOT NULL,
                CHECK (id IN ('model', 'model_visibility', 'update'))
            ) STRICT;
            CREATE TABLE deletion_journals (
                id TEXT PRIMARY KEY NOT NULL,
                guru_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                data_json TEXT NOT NULL,
                UNIQUE (guru_id, target_id)
            ) STRICT;
            CREATE TABLE memory_finalization_journals (
                id TEXT PRIMARY KEY NOT NULL,
                guru_id TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                data_json TEXT NOT NULL
            ) STRICT;
            CREATE TABLE guru_profiles (
                id TEXT PRIMARY KEY NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                root_device TEXT,
                root_inode TEXT,
                data_json TEXT NOT NULL,
                CHECK ((root_device IS NULL) = (root_inode IS NULL))
            ) STRICT;
            CREATE TABLE guru_capability_bindings (
                guru_id TEXT NOT NULL REFERENCES guru_profiles(id) ON DELETE RESTRICT,
                entry_id TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                data_json TEXT NOT NULL,
                PRIMARY KEY (guru_id, entry_id)
            ) STRICT;
            CREATE TABLE user_skills (
                id TEXT PRIMARY KEY NOT NULL,
                guru_id TEXT NOT NULL REFERENCES guru_profiles(id) ON DELETE RESTRICT,
                current_revision_id TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                data_json TEXT NOT NULL,
                UNIQUE (id, guru_id),
                FOREIGN KEY (current_revision_id, id, guru_id)
                    REFERENCES user_skill_revisions(id, skill_id, guru_id)
                    ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED
            ) STRICT;
            CREATE TABLE user_skill_revisions (
                id TEXT PRIMARY KEY NOT NULL,
                skill_id TEXT NOT NULL,
                guru_id TEXT NOT NULL REFERENCES guru_profiles(id) ON DELETE RESTRICT,
                revision INTEGER NOT NULL CHECK (revision > 0),
                created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
                data_json TEXT NOT NULL,
                UNIQUE (skill_id, revision),
                UNIQUE (id, skill_id, guru_id),
                FOREIGN KEY (skill_id, guru_id)
                    REFERENCES user_skills(id, guru_id)
                    ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED
            ) STRICT;
            CREATE TRIGGER user_skill_revisions_are_immutable
                BEFORE UPDATE ON user_skill_revisions
                BEGIN
                    SELECT RAISE(ABORT, 'user Skill revisions are immutable');
                END;
            CREATE TABLE chat_sessions (
                id TEXT PRIMARY KEY NOT NULL,
                guru_id TEXT NOT NULL REFERENCES guru_profiles(id) ON DELETE RESTRICT,
                updated_at_ms INTEGER NOT NULL,
                data_json TEXT NOT NULL
            ) STRICT;
            CREATE TABLE chat_artifacts (
                id TEXT PRIMARY KEY NOT NULL,
                chat_session_id TEXT NOT NULL REFERENCES chat_sessions(id) ON DELETE RESTRICT,
                kind TEXT NOT NULL CHECK (kind IN ('markdown', 'chart')),
                title TEXT NOT NULL,
                current_revision INTEGER NOT NULL CHECK (current_revision > 0),
                created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
                updated_at_ms INTEGER NOT NULL,
                data_json TEXT NOT NULL,
                current_content_json TEXT NOT NULL
            ) STRICT;
            CREATE TABLE chart_artifact_datasets (
                id TEXT PRIMARY KEY NOT NULL,
                artifact_id TEXT NOT NULL UNIQUE REFERENCES chat_artifacts(id) ON DELETE RESTRICT,
                digest TEXT NOT NULL,
                data_json TEXT NOT NULL
            ) STRICT;
            CREATE INDEX chat_sessions_by_guru
                ON chat_sessions(guru_id, updated_at_ms DESC);
            CREATE INDEX user_skills_by_guru
                ON user_skills(guru_id, updated_at_ms DESC, id);
            CREATE UNIQUE INDEX guru_profiles_by_root_identity
                ON guru_profiles(root_device, root_inode)
                WHERE root_device IS NOT NULL AND root_inode IS NOT NULL;
            CREATE INDEX chat_artifacts_by_session
                ON chat_artifacts(chat_session_id, updated_at_ms DESC);
            PRAGMA user_version = 9;
            COMMIT;
            "#,
        )?;
        Ok(())
    }
}
