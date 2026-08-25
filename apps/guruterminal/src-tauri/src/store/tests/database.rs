use super::super::schema::{migration_steps_for_test, FIRST_MIGRATABLE_SCHEMA_VERSION};
use super::super::DeletionJournalRecord;
use super::super::MemoryFinalizationJournalRecord;
use super::support::*;
use super::*;
use crate::artifact_trust::ArtifactTrustError;
use crate::domain::{DeletionJournal, DeletionKind, DeletionPhase};
use crate::memory_finalization::{
    MemoryFinalizationJournal, MemoryFinalizationScope, MEMORY_FINALIZATION_SCHEMA_VERSION,
};
use crate::memory_git::MemoryGitSnapshot;
use crate::runtime::StagedMemoryChange;
use crate::settings::ModelVisibility;
use crate::store::STORE_SCHEMA_VERSION;
use rusqlite::Connection;

fn migration_chat() -> ChatSession {
    ChatSession {
        id: "chat-migration".into(),
        guru_id: "guru-1".into(),
        pi_session_id: "123e4567-e89b-42d3-a456-426614174010".into(),
        pi_session_cache: None,
        title: "Preserve this research".into(),
        memory_policy: MemoryPolicy::default(),
        messages: Vec::new(),
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn downgrade_to_private_schema_v9(store: &SqliteStore) {
    store
        .lock()
        .unwrap()
        .execute_batch(
            r#"
            DROP INDEX deletion_journals_by_updated_at;
            DROP INDEX memory_finalization_journals_by_updated_at;
            PRAGMA user_version = 9;
            "#,
        )
        .unwrap();
}

fn memory_finalization_journal() -> MemoryFinalizationJournal {
    MemoryFinalizationJournal {
        schema_version: MEMORY_FINALIZATION_SCHEMA_VERSION,
        id: "memory-write:migration".into(),
        guru_id: "guru-1".into(),
        scope: MemoryFinalizationScope::StandaloneUser,
        updated_at_ms: 3,
        git: MemoryGitSnapshot {
            previous_head: None,
            symbolic_head: None,
            original_index_tree: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            published_index_tree: None,
        },
        changes: vec![StagedMemoryChange {
            guru_id: "guru-1".into(),
            session_id: "memory-write:migration".into(),
            relative_path: "guruterminal/wiki/migration.md".into(),
            before_sha256: None,
            before_markdown: None,
            proposed_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .into(),
            proposed_markdown: "# Preserve this pending Memory write\n".into(),
            delete: false,
        }],
        commit_id: None,
    }
}

#[test]
fn schema_migrations_are_contiguous_from_private_v9_to_current() {
    let mut version = FIRST_MIGRATABLE_SCHEMA_VERSION;
    for (from, to) in migration_steps_for_test() {
        assert_eq!(from, version, "migration sequence has a gap");
        assert_eq!(to, from + 1, "migration must advance exactly one version");
        version = to;
    }
    assert_eq!(
        version, STORE_SCHEMA_VERSION,
        "a schema version change requires a contiguous migration"
    );
}

#[test]
fn private_schema_v9_migrates_losslessly_to_current_schema() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("guruterminal.sqlite3");
    let store = SqliteStore::open(&path).unwrap();
    let profile = guru();
    seed_guru(&store, &profile);
    let chat = migration_chat();
    store.create_chat(&chat).unwrap();

    let mut visibility = ModelVisibility::default();
    visibility.set_visible("openai-codex/gpt-5.6-luna", false);
    store.save_model_visibility(&visibility).unwrap();
    let journal = DeletionJournal {
        id: "delete-chat-chat-migration".into(),
        kind: DeletionKind::Chat,
        guru_id: profile.id.clone(),
        target_id: chat.id.clone(),
        expected_source_identity: None,
        phase: DeletionPhase::Prepared,
        created_at_ms: 2,
        updated_at_ms: 2,
    };
    store.create_deletion_journal(&journal).unwrap();
    let memory_journal = memory_finalization_journal();
    store
        .create_memory_finalization_journal(&memory_journal)
        .unwrap();
    downgrade_to_private_schema_v9(&store);
    drop(store);

    let (migrated, fresh) = SqliteStore::open_or_replace_obsolete(&path).unwrap();
    assert!(!fresh, "a supported private schema must not be replaced");
    assert_eq!(migrated.get_guru(&profile.id).unwrap(), Some(profile));
    assert_eq!(migrated.get_chat(&chat.id).unwrap(), Some(chat));
    assert_eq!(migrated.get_model_visibility().unwrap(), Some(visibility));
    assert!(matches!(
        migrated.list_deletion_journals().unwrap().as_slice(),
        [DeletionJournalRecord::Valid(value)] if value == &journal
    ));
    assert!(matches!(
        migrated.list_memory_finalization_journals().unwrap().as_slice(),
        [MemoryFinalizationJournalRecord::Valid(value)]
            if value.id == memory_journal.id
                && value.guru_id == memory_journal.guru_id
                && value.updated_at_ms == memory_journal.updated_at_ms
                && value.changes.len() == 1
                && value.changes[0].proposed_markdown == "# Preserve this pending Memory write\n"
    ));
    drop(migrated);

    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        STORE_SCHEMA_VERSION
    );
    for (index, table) in [
        ("deletion_journals_by_updated_at", "deletion_journals"),
        (
            "memory_finalization_journals_by_updated_at",
            "memory_finalization_journals",
        ),
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT tbl_name FROM sqlite_schema WHERE type = 'index' AND name = ?1",
                    [index],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            table
        );
    }
}

#[test]
fn failed_migration_rolls_back_prior_steps_and_preserves_private_data() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("guruterminal.sqlite3");
    let store = SqliteStore::open(&path).unwrap();
    let profile = guru();
    seed_guru(&store, &profile);
    let chat = migration_chat();
    store.create_chat(&chat).unwrap();
    let journal = DeletionJournal {
        id: "delete-chat-chat-migration".into(),
        kind: DeletionKind::Chat,
        guru_id: profile.id.clone(),
        target_id: chat.id.clone(),
        expected_source_identity: None,
        phase: DeletionPhase::Prepared,
        created_at_ms: 2,
        updated_at_ms: 2,
    };
    store.create_deletion_journal(&journal).unwrap();
    downgrade_to_private_schema_v9(&store);
    store
        .lock()
        .unwrap()
        .execute_batch(
            r#"
            CREATE INDEX memory_finalization_journals_by_updated_at
                ON guru_profiles(updated_at_ms ASC, id ASC);
            "#,
        )
        .unwrap();
    drop(store);

    assert!(matches!(
        SqliteStore::open(&path),
        Err(StoreError::Sqlite(_))
    ));

    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        FIRST_MIGRATABLE_SCHEMA_VERSION
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'index' AND name = 'deletion_journals_by_updated_at'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0,
        "the first migration statement must roll back"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT tbl_name FROM sqlite_schema WHERE type = 'index' AND name = 'memory_finalization_journals_by_updated_at'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "guru_profiles"
    );
    assert_eq!(
        connection
            .query_row("SELECT id FROM guru_profiles", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        profile.id
    );
    assert_eq!(
        connection
            .query_row("SELECT id FROM chat_sessions", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        chat.id
    );
    assert_eq!(
        connection
            .query_row("SELECT id FROM deletion_journals", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        journal.id
    );
}

#[test]
fn model_visibility_round_trips_independently_from_the_catalog() {
    let store = SqliteStore::open_in_memory().unwrap();
    assert_eq!(store.get_model_visibility().unwrap(), None);

    let mut visibility = ModelVisibility::default();
    visibility.set_visible("openai-codex/gpt-5.6-luna", false);
    store.save_model_visibility(&visibility).unwrap();

    assert_eq!(store.get_model_visibility().unwrap(), Some(visibility));
}

#[cfg(unix)]
#[test]
fn file_backed_store_hardens_database_and_rejects_side_symlinks() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("app-data");
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    let database = directory.join("guruterminal.sqlite3");
    let store = SqliteStore::open(&database).unwrap();
    seed_guru(&store, &guru());
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&database).unwrap().permissions().mode() & 0o777,
        0o600
    );
    for suffix in ["-wal", "-shm"] {
        let side = sqlite_side_path(&database, suffix);
        assert!(side.is_file());
        assert_eq!(
            std::fs::metadata(side).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(store);
    for suffix in ["-wal", "-shm", "-journal"] {
        let side = sqlite_side_path(&database, suffix);
        if side.exists() {
            assert_eq!(
                std::fs::metadata(side).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    let outside = temporary.path().join("outside");
    std::fs::write(&outside, b"outside").unwrap();
    let linked_database = directory.join("linked.sqlite3");
    symlink(&outside, &linked_database).unwrap();
    assert!(matches!(
        SqliteStore::open(&linked_database),
        Err(StoreError::PrivateStorage(ArtifactTrustError::Untrusted))
    ));

    let side_database = directory.join("side.sqlite3");
    std::fs::write(&side_database, []).unwrap();
    symlink(&outside, sqlite_side_path(&side_database, "-wal")).unwrap();
    assert!(matches!(
        SqliteStore::open(&side_database),
        Err(StoreError::PrivateStorage(ArtifactTrustError::Untrusted))
    ));
}

#[test]
fn unsupported_schema_versions_are_rejected_without_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    for version in [
        1_i64,
        2,
        FIRST_MIGRATABLE_SCHEMA_VERSION - 1,
        STORE_SCHEMA_VERSION + 1,
        STORE_SCHEMA_VERSION + 7,
    ] {
        let path = temporary
            .path()
            .join(format!("guruterminal-v{version}.sqlite3"));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE sentinel (value TEXT NOT NULL) STRICT;\
                 INSERT INTO sentinel (value) VALUES ('preserve-me');\
                 PRAGMA user_version = {version};"
            ))
            .unwrap();
        drop(connection);
        let before = std::fs::read(&path).unwrap();

        assert!(matches!(
            SqliteStore::open(&path),
            Err(StoreError::UnsupportedSchema { found, expected })
                if found == version && expected == STORE_SCHEMA_VERSION
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);

        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            version
        );
        assert_eq!(
            connection
                .query_row("SELECT value FROM sentinel", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "preserve-me"
        );
    }
}

#[test]
fn nonempty_schema_zero_is_rejected_without_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("guruterminal-schema-zero.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE sentinel (value TEXT NOT NULL) STRICT;\
             INSERT INTO sentinel (value) VALUES ('preserve-me');\
             PRAGMA user_version = 0;",
        )
        .unwrap();
    drop(connection);
    let before = std::fs::read(&path).unwrap();

    assert!(matches!(
        SqliteStore::open(&path),
        Err(StoreError::UnversionedNonemptySchema)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(!sqlite_side_path(&path, suffix).exists());
    }

    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT value FROM sentinel", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "preserve-me"
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM sqlite_schema", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn obsolete_schema_can_be_replaced_with_a_fresh_current_database() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("app-data");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("guruterminal.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE sentinel (value TEXT NOT NULL) STRICT;\
             INSERT INTO sentinel (value) VALUES ('preserve-me');\
             PRAGMA user_version = 2;",
        )
        .unwrap();
    drop(connection);
    std::fs::write(sqlite_side_path(&path, "-wal"), b"stale-wal").unwrap();

    let (store, fresh) = SqliteStore::open_or_replace_obsolete(&path).unwrap();
    assert!(fresh);
    drop(store);

    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        STORE_SCHEMA_VERSION
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'sentinel'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    assert!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'guru_profiles'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0
    );
}

#[test]
fn current_schema_is_reopened_without_replacement() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("app-data");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("guruterminal.sqlite3");
    drop(SqliteStore::open(&path).unwrap());

    let (store, fresh) = SqliteStore::open_or_replace_obsolete(&path).unwrap();
    assert!(!fresh);
    drop(store);
}
