use super::support::*;
use super::*;

#[test]
fn profile_round_trips() {
    let store = SqliteStore::open_in_memory().unwrap();
    let profile = guru();
    assert!(matches!(
        store.save_guru(&profile),
        Err(StoreError::Conflict("Guru is missing"))
    ));
    seed_guru(&store, &profile);
    assert_eq!(store.get_guru(&profile.id).unwrap(), Some(profile));
}

#[test]
fn deleting_a_guru_removes_every_dependent_row_in_one_transaction() {
    let store = SqliteStore::open_in_memory().unwrap();
    let profile = guru();
    seed_guru(&store, &profile);
    let chat = ChatSession {
        id: "chat-delete".into(),
        guru_id: "guru-1".into(),
        pi_session_id: "123e4567-e89b-42d3-a456-426614174000".into(),
        pi_session_cache: None,
        title: "Delete fixture".into(),
        memory_policy: MemoryPolicy::default(),
        messages: Vec::new(),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    store.create_chat(&chat).unwrap();
    store.delete_guru(&profile).unwrap();
    assert!(store.delete_guru(&profile).is_err());
    assert_eq!(store.get_guru("guru-1").unwrap(), None);
    let connection = store.lock().unwrap();
    for table in ["guru_profiles", "guru_capability_bindings", "chat_sessions"] {
        let count = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(count, 0, "table {table} retained a deleted Guru row");
    }
}

#[test]
fn capability_bindings_are_seeded_and_isolated_per_guru() {
    let store = SqliteStore::open_in_memory().unwrap();
    let first = guru_with_root("guru-a", 11, 22);
    let second = guru_with_root("guru-b", 33, 44);
    seed_guru(&store, &first);
    seed_guru(&store, &second);

    let first_defaults = store.list_guru_capabilities("guru-a").unwrap();
    let expected_defaults = crate::domain::default_guru_capability_bindings("guru-a", 1);
    assert_eq!(first_defaults.len(), expected_defaults.len());
    assert_eq!(
        first_defaults
            .iter()
            .filter(|binding| binding.enabled)
            .count(),
        expected_defaults
            .iter()
            .filter(|binding| binding.enabled)
            .count()
    );
    assert!(first_defaults.iter().any(|binding| {
        binding.entry_id == "guruterminal.compute-python"
            && binding.granted_permissions == ["execute"]
    }));
    assert!(first_defaults
        .iter()
        .filter(|binding| !binding.enabled)
        .all(|binding| binding.granted_permissions.is_empty()));
    assert_eq!(
        first_defaults
            .iter()
            .filter(|binding| binding.entry_id.starts_with("skill."))
            .count(),
        crate::agent_harness::default_skill_ids().len()
    );
    assert!(first_defaults
        .iter()
        .filter(|binding| binding.entry_id.starts_with("skill."))
        .all(|binding| binding.granted_permissions == ["load"]));

    let mut disabled = store
        .get_guru_capability("guru-a", "openbb.platform")
        .unwrap()
        .unwrap();
    disabled.enabled = false;
    disabled.granted_permissions.clear();
    disabled.updated_at_ms += 1;
    store.save_guru_capability(&disabled).unwrap();

    assert!(
        !store
            .get_guru_capability("guru-a", "openbb.platform")
            .unwrap()
            .unwrap()
            .enabled
    );
    assert!(
        store
            .get_guru_capability("guru-b", "openbb.platform")
            .unwrap()
            .unwrap()
            .enabled
    );
}

#[test]
fn same_guru_can_update_while_cross_guru_root_reuse_is_rejected() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut first = guru_with_root("guru-a", 11, 22);
    seed_guru(&store, &first);
    first.name = "Updated Guru".into();
    first.memory_root = "/tmp/guru-a-renamed".into();
    first.updated_at_ms = 2;
    store.save_guru(&first).unwrap();
    assert_eq!(store.get_guru("guru-a").unwrap(), Some(first));

    assert!(matches!(
        create_guru(&store, &guru_with_root("guru-b", 11, 22)),
        Err(StoreError::Conflict("Guru identity already exists"))
    ));
}

#[test]
fn concurrent_connections_atomically_admit_only_one_guru_per_root() {
    use std::sync::{Arc, Barrier};

    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("guruterminal.sqlite3");
    drop(SqliteStore::open(&path).unwrap());
    let barrier = Arc::new(Barrier::new(3));
    let handles = ["guru-a", "guru-b"].map(|id| {
        let path = path.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            let store = SqliteStore::open(path).unwrap();
            barrier.wait();
            let profile = guru_with_root(id, 101, 202);
            store.create_guru(&profile)
        })
    });
    barrier.wait();
    let results = handles.map(|handle| handle.join().unwrap());
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(StoreError::Conflict(_))))
            .count(),
        1
    );
}
