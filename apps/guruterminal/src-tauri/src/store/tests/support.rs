use super::*;

pub(super) fn guru() -> GuruProfile {
    guru_with_root("guru-1", 11, 22)
}

pub(super) fn guru_with_root(id: &str, device: u64, inode: u64) -> GuruProfile {
    GuruProfile {
        id: id.into(),
        name: format!("{id} Guru"),
        description: String::new(),
        storage_kind: GuruStorageKind::Managed,
        memory_root: format!("/tmp/{id}"),
        root_filesystem_identity: Some(RootFilesystemIdentity { device, inode }),
        last_model_profile_id: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

pub(super) fn create_guru(store: &SqliteStore, profile: &GuruProfile) -> StoreResult<()> {
    store.create_guru(profile)
}

pub(super) fn seed_guru(store: &SqliteStore, profile: &GuruProfile) {
    create_guru(store, profile).unwrap();
}
