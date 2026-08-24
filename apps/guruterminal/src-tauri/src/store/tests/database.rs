use super::support::*;
use super::*;
use crate::artifact_trust::ArtifactTrustError;
use crate::settings::ModelVisibility;
use crate::store::STORE_SCHEMA_VERSION;
use rusqlite::Connection;

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
    for version in [1_i64, 2, 5, 16, 17] {
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
