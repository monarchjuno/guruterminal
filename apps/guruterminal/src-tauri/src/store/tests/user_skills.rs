use super::support::*;
use super::*;

#[test]
fn user_skill_relations_are_composite_deferred_and_revisions_reject_updates() {
    let store = SqliteStore::open_in_memory().unwrap();
    let profile = guru();
    seed_guru(&store, &profile);
    let other = guru_with_root("guru-2", 33, 44);
    seed_guru(&store, &other);

    {
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(
                "INSERT INTO user_skills
                    (id, guru_id, current_revision_id, updated_at_ms, data_json)
                 VALUES ('skill:valid', 'guru-1', 'revision-valid', 1, '{}');
                 INSERT INTO user_skill_revisions
                    (id, skill_id, guru_id, revision, created_at_ms, data_json)
                 VALUES ('revision-valid', 'skill:valid', 'guru-1', 1, 1, '{}');",
            )
            .unwrap();
        transaction.commit().unwrap();
        assert!(connection
            .execute(
                "UPDATE user_skill_revisions SET data_json = '{\"tampered\":true}' WHERE id = 'revision-valid'",
                [],
            )
            .is_err());

        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(
                "INSERT INTO user_skills
                    (id, guru_id, current_revision_id, updated_at_ms, data_json)
                 VALUES
                    ('skill:swapped-a', 'guru-1', 'revision-swapped-b', 2, '{}'),
                    ('skill:swapped-b', 'guru-1', 'revision-swapped-a', 2, '{}');
                 INSERT INTO user_skill_revisions
                    (id, skill_id, guru_id, revision, created_at_ms, data_json)
                 VALUES
                    ('revision-swapped-a', 'skill:swapped-a', 'guru-1', 1, 2, '{}'),
                    ('revision-swapped-b', 'skill:swapped-b', 'guru-1', 1, 2, '{}');",
            )
            .unwrap();
        assert!(transaction.commit().is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM user_skills WHERE id LIKE 'skill:swapped-%'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );

        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(
                "INSERT INTO user_skills
                    (id, guru_id, current_revision_id, updated_at_ms, data_json)
                 VALUES ('skill:cross-guru', 'guru-1', 'revision-cross-guru', 3, '{}');
                 INSERT INTO user_skill_revisions
                    (id, skill_id, guru_id, revision, created_at_ms, data_json)
                 VALUES ('revision-cross-guru', 'skill:cross-guru', 'guru-2', 1, 3, '{}');",
            )
            .unwrap();
        assert!(transaction.commit().is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM user_skills WHERE id = 'skill:cross-guru'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    store.delete_guru(&profile).unwrap();
    assert!(store.get_guru("guru-2").unwrap().is_some());
}
