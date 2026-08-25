use std::{collections::HashMap, path::PathBuf};

use crate::{
    app::CommandError,
    commands::map_store,
    domain::{
        ChatSession, DeletionJournal, DeletionKind, DeletionPhase, GuruProfile,
        RootFilesystemIdentity,
    },
    secure_delete::SecureDeletionRoot,
    store::{DeletionJournalRecord, GuruTerminalStore},
};

#[derive(Debug)]
struct DeletionPath {
    source: PathBuf,
    tombstone: PathBuf,
    required: bool,
    expected_identity: Option<RootFilesystemIdentity>,
}

#[derive(Debug, Default)]
pub struct DeletionRecovery {
    pub quarantined_gurus: HashMap<String, String>,
}

pub fn delete_chat(
    store: &dyn GuruTerminalStore,
    root: &SecureDeletionRoot,
    expected: &ChatSession,
    timestamp_ms: i64,
) -> Result<(), CommandError> {
    let journal = DeletionJournal {
        id: format!("delete-chat-{}", expected.id),
        kind: DeletionKind::Chat,
        guru_id: expected.guru_id.clone(),
        target_id: expected.id.clone(),
        expected_source_identity: None,
        phase: DeletionPhase::Prepared,
        created_at_ms: timestamp_ms,
        updated_at_ms: timestamp_ms,
    };
    execute_deletion(store, root, journal, |store| store.delete_chat(expected))
}

pub fn delete_guru(
    store: &dyn GuruTerminalStore,
    root: &SecureDeletionRoot,
    expected: &GuruProfile,
    timestamp_ms: i64,
) -> Result<(), CommandError> {
    let source = PathBuf::from("gurus").join(&expected.id);
    let expected_workspace_identity = expected
        .root_filesystem_identity
        .as_ref()
        .ok_or_else(|| CommandError::conflict("Guru workspace identity is not sealed"))?;
    let observed_workspace_identity = root
        .directory_identity(&source.join("workspace"))?
        .ok_or_else(|| CommandError::conflict("Guru workspace disappeared before deletion"))?;
    if &observed_workspace_identity != expected_workspace_identity {
        return Err(CommandError::conflict(
            "Guru workspace identity changed before deletion",
        ));
    }
    let source_identity = root
        .directory_identity(&source)?
        .ok_or_else(|| CommandError::conflict("Guru storage disappeared before deletion"))?;
    let journal = DeletionJournal {
        id: format!("delete-guru-{}", expected.id),
        kind: DeletionKind::Guru,
        guru_id: expected.id.clone(),
        target_id: expected.id.clone(),
        expected_source_identity: Some(source_identity),
        phase: DeletionPhase::Prepared,
        created_at_ms: timestamp_ms,
        updated_at_ms: timestamp_ms,
    };
    execute_deletion(store, root, journal, |store| store.delete_guru(expected))
}

fn execute_deletion(
    store: &dyn GuruTerminalStore,
    root: &SecureDeletionRoot,
    prepared: DeletionJournal,
    delete_target: impl FnOnce(&dyn GuruTerminalStore) -> Result<(), crate::store::StoreError>,
) -> Result<(), CommandError> {
    prepared.validate().map_err(map_internal)?;
    store
        .create_deletion_journal(&prepared)
        .map_err(map_store)?;
    let paths = journal_paths(&prepared)?;
    if let Err(error) = detach_paths(root, &paths) {
        // Reconcile the complete path set even when the local rollback inside
        // `detach_paths` failed partway. The durable pointer may be removed only
        // after every required/live entry is proven restored.
        if rollback_paths(root, &paths).is_ok() {
            store
                .delete_deletion_journal(&prepared)
                .map_err(map_store)?;
        }
        return Err(error);
    }

    let mut detached = prepared.clone();
    detached.phase = DeletionPhase::Detached;
    detached.updated_at_ms = detached.updated_at_ms.saturating_add(1);
    if let Err(error) = store.replace_deletion_journal(&prepared, &detached) {
        if rollback_paths(root, &paths).is_ok() {
            store
                .delete_deletion_journal(&prepared)
                .map_err(map_store)?;
        }
        return Err(map_store(error));
    }

    if let Err(error) = delete_target(store) {
        if rollback_paths(root, &paths).is_ok() {
            store
                .delete_deletion_journal(&detached)
                .map_err(map_store)?;
        }
        return Err(map_store(error));
    }

    // Once SQLite commits absence, restoration would resurrect bytes without
    // authority. Cleanup therefore remains retryable in the durable journal.
    cleanup_paths(root, &paths)?;
    store.delete_deletion_journal(&detached).map_err(map_store)
}

pub fn recover(
    store: &dyn GuruTerminalStore,
    root: &SecureDeletionRoot,
) -> Result<DeletionRecovery, CommandError> {
    let mut recovery = DeletionRecovery::default();
    let records = store.list_deletion_journals().map_err(map_store)?;
    for record in records {
        let journal = match record {
            DeletionJournalRecord::Valid(journal) => journal,
            DeletionJournalRecord::Invalid {
                id,
                guru_id,
                reason,
            } => {
                let reason = format!("pending deletion journal {id} is invalid: {reason}");
                if store.get_guru(&guru_id).map_err(map_store)?.is_some() {
                    recovery.quarantined_gurus.insert(guru_id, reason);
                }
                continue;
            }
        };
        let paths = match journal_paths(&journal) {
            Ok(paths) => paths,
            Err(error) => {
                if store
                    .get_guru(&journal.guru_id)
                    .map_err(map_store)?
                    .is_some()
                {
                    recovery
                        .quarantined_gurus
                        .insert(journal.guru_id.clone(), error.message);
                }
                continue;
            }
        };
        let target_exists = match journal.kind {
            DeletionKind::Chat => match store.get_chat(&journal.target_id) {
                Ok(Some(chat)) if chat.guru_id == journal.guru_id => true,
                Ok(Some(_)) => {
                    recovery.quarantined_gurus.insert(
                        journal.guru_id.clone(),
                        "pending Chat deletion belongs to another Guru".into(),
                    );
                    continue;
                }
                Ok(None) => false,
                Err(error) => {
                    recovery
                        .quarantined_gurus
                        .insert(journal.guru_id.clone(), error.to_string());
                    continue;
                }
            },
            DeletionKind::Guru => match store.get_guru(&journal.guru_id) {
                Ok(profile) => profile.is_some(),
                Err(error) => {
                    recovery
                        .quarantined_gurus
                        .insert(journal.guru_id.clone(), error.to_string());
                    continue;
                }
            },
        };

        let reconciled = if target_exists {
            rollback_paths(root, &paths)
        } else {
            cleanup_paths(root, &paths)
        };
        if let Err(error) = reconciled {
            if target_exists {
                recovery
                    .quarantined_gurus
                    .insert(journal.guru_id.clone(), error.message);
            }
            // A committed deletion with cleanup failure remains absent and is
            // retried on a later startup; it must not block unrelated Gurus.
            continue;
        }
        if let Err(error) = store.delete_deletion_journal(&journal) {
            if target_exists {
                recovery
                    .quarantined_gurus
                    .insert(journal.guru_id.clone(), error.to_string());
            }
        }
    }
    Ok(recovery)
}

pub fn has_pending_for(
    store: &dyn GuruTerminalStore,
    guru_id: &str,
    target_id: &str,
) -> Result<bool, CommandError> {
    Ok(store
        .list_deletion_journals()
        .map_err(map_store)?
        .into_iter()
        .any(|record| match record {
            DeletionJournalRecord::Valid(journal) => {
                journal.guru_id == guru_id && journal.target_id == target_id
            }
            DeletionJournalRecord::Invalid {
                guru_id: stored_guru,
                ..
            } => stored_guru == guru_id,
        }))
}

fn journal_paths(journal: &DeletionJournal) -> Result<Vec<DeletionPath>, CommandError> {
    journal.validate().map_err(map_internal)?;
    let mut paths = Vec::new();
    match journal.kind {
        DeletionKind::Chat => {
            let guru = PathBuf::from("gurus").join(&journal.guru_id);
            for parent in [
                guru.join("pi-sessions"),
                guru.join("pi-runtime"),
                guru.join("workbench").join("attachments"),
            ] {
                paths.push(DeletionPath {
                    source: parent.join(&journal.target_id),
                    tombstone: parent.join(format!(".deleting-{}", journal.target_id)),
                    required: false,
                    expected_identity: None,
                });
            }
        }
        DeletionKind::Guru => {
            let gurus = PathBuf::from("gurus");
            paths.push(DeletionPath {
                source: gurus.join(&journal.guru_id),
                tombstone: gurus.join(format!(".deleting-{}", journal.guru_id)),
                required: true,
                expected_identity: journal.expected_source_identity.clone(),
            });
            let runs = PathBuf::from("runs");
            paths.push(DeletionPath {
                source: runs.join(&journal.guru_id),
                tombstone: runs.join(format!(".deleting-{}", journal.guru_id)),
                required: false,
                expected_identity: None,
            });
        }
    }
    Ok(paths)
}

fn detach_paths(root: &SecureDeletionRoot, paths: &[DeletionPath]) -> Result<(), CommandError> {
    let mut detached = Vec::new();
    for path in paths {
        if root.entry_exists(&path.tombstone)? {
            rollback_detached(root, &detached)?;
            return Err(CommandError::conflict(
                "private-storage deletion tombstone already exists",
            ));
        }
        match root.rename_sibling_expected(
            &path.source,
            &path.tombstone,
            path.expected_identity.as_ref(),
        ) {
            Ok(true) => detached.push(path),
            Ok(false) if !path.required => {}
            Ok(false) => {
                rollback_detached(root, &detached)?;
                return Err(CommandError::conflict(
                    "required private storage disappeared during deletion",
                ));
            }
            Err(error) => {
                rollback_detached(root, &detached)?;
                return Err(error);
            }
        }
    }
    Ok(())
}

fn rollback_detached(
    root: &SecureDeletionRoot,
    detached: &[&DeletionPath],
) -> Result<(), CommandError> {
    for path in detached.iter().rev() {
        if !root.rename_sibling_expected(
            &path.tombstone,
            &path.source,
            path.expected_identity.as_ref(),
        )? {
            return Err(CommandError::internal(
                "deletion rollback lost a detached private-storage entry",
            ));
        }
    }
    Ok(())
}

fn rollback_paths(root: &SecureDeletionRoot, paths: &[DeletionPath]) -> Result<(), CommandError> {
    for path in paths.iter().rev() {
        let live_exists = root.entry_exists(&path.source)?;
        let tombstone_exists = root.entry_exists(&path.tombstone)?;
        if live_exists {
            if let Some(expected) = &path.expected_identity {
                let observed = root.directory_identity(&path.source)?.ok_or_else(|| {
                    CommandError::internal(
                        "required private storage disappeared during identity recovery",
                    )
                })?;
                if &observed != expected {
                    return Err(CommandError::conflict(
                        "required private storage identity changed during recovery",
                    ));
                }
            }
        }
        match (live_exists, tombstone_exists) {
            (true, true) => {
                return Err(CommandError::internal(
                    "deletion rollback found both live and tombstone storage",
                ));
            }
            (false, true) => {
                if !root.rename_sibling_expected(
                    &path.tombstone,
                    &path.source,
                    path.expected_identity.as_ref(),
                )? {
                    return Err(CommandError::internal(
                        "deletion rollback lost a tombstone storage entry",
                    ));
                }
            }
            (false, false) if path.required => {
                return Err(CommandError::internal(
                    "required private storage is missing during deletion recovery",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn cleanup_paths(root: &SecureDeletionRoot, paths: &[DeletionPath]) -> Result<(), CommandError> {
    for path in paths {
        root.remove_tree_expected(&path.tombstone, path.expected_identity.as_ref())?;
    }
    Ok(())
}

fn map_internal(error: impl std::fmt::Display) -> CommandError {
    CommandError::internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        artifact_trust::ensure_private_directory,
        domain::{GuruStorageKind, MemoryPolicy},
        store::SqliteStore,
    };
    use std::fs;

    fn root(path: &std::path::Path) -> SecureDeletionRoot {
        SecureDeletionRoot::open(&path.canonicalize().unwrap()).unwrap()
    }

    fn tree_contains(path: &std::path::Path, needle: &[u8]) -> bool {
        let Ok(entries) = fs::read_dir(path) else {
            return false;
        };
        entries.filter_map(Result::ok).any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                tree_contains(&path, needle)
            } else {
                fs::read(path)
                    .is_ok_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
            }
        })
    }

    fn chat(guru_id: &str, id: &str) -> ChatSession {
        ChatSession {
            id: id.into(),
            guru_id: guru_id.into(),
            pi_session_id: "123e4567-e89b-42d3-a456-426614174000".into(),
            pi_session_cache: None,
            title: "Chat".into(),
            memory_policy: MemoryPolicy::default(),
            messages: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn seed_guru(store: &SqliteStore, app_data: &std::path::Path, id: &str) -> GuruProfile {
        ensure_private_directory(&app_data.join("gurus").join(id).join("workspace")).unwrap();
        let deletion_root = root(app_data);
        let workspace_identity = deletion_root
            .directory_identity(&PathBuf::from("gurus").join(id).join("workspace"))
            .unwrap()
            .unwrap();
        let profile = GuruProfile {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            storage_kind: GuruStorageKind::Managed,
            memory_root: app_data
                .join("gurus")
                .join(id)
                .join("workspace")
                .display()
                .to_string(),
            root_filesystem_identity: Some(workspace_identity),
            last_model_profile_id: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        store.create_guru(&profile).unwrap();
        profile
    }

    #[test]
    fn chat_deletion_removes_crashed_scratch_and_attachments_before_finishing_journal() {
        let temporary = tempfile::tempdir().unwrap();
        let root = root(temporary.path());
        let store = SqliteStore::open_in_memory().unwrap();
        seed_guru(&store, temporary.path(), "guru-a");
        let chat = chat("guru-a", "chat-a");
        store.create_chat(&chat).unwrap();
        for relative in [
            "gurus/guru-a/pi-sessions/chat-a",
            "gurus/guru-a/pi-runtime/chat-a",
            "gurus/guru-a/workbench/attachments/chat-a",
        ] {
            ensure_private_directory(&temporary.path().join(relative)).unwrap();
            fs::write(temporary.path().join(relative).join("secret"), b"secret").unwrap();
        }

        delete_chat(&store, &root, &chat, 10).unwrap();

        assert!(store.get_chat("chat-a").unwrap().is_none());
        assert!(store.list_deletion_journals().unwrap().is_empty());
        assert!(!temporary
            .path()
            .join("gurus/guru-a/pi-sessions/chat-a")
            .exists());
        assert!(!temporary
            .path()
            .join("gurus/guru-a/pi-runtime/chat-a")
            .exists());
        assert!(!temporary
            .path()
            .join("gurus/guru-a/workbench/attachments/chat-a")
            .exists());
    }

    #[test]
    fn fresh_chat_deletion_allows_missing_optional_storage_parents() {
        let temporary = tempfile::tempdir().unwrap();
        let root = root(temporary.path());
        let store = SqliteStore::open_in_memory().unwrap();
        seed_guru(&store, temporary.path(), "guru-a");
        let chat = chat("guru-a", "chat-empty");
        store.create_chat(&chat).unwrap();

        delete_chat(&store, &root, &chat, 10).unwrap();

        assert!(store.get_chat("chat-empty").unwrap().is_none());
        assert!(store.list_deletion_journals().unwrap().is_empty());
    }

    #[test]
    fn guru_deletion_removes_guru_owned_run_scratch() {
        let temporary = tempfile::tempdir().unwrap();
        let root = root(temporary.path());
        let store = SqliteStore::open_in_memory().unwrap();
        let profile = seed_guru(&store, temporary.path(), "guru-a");
        let scratch = temporary.path().join("runs/guru-a/stale-run");
        ensure_private_directory(&scratch).unwrap();
        fs::write(scratch.join("SKILL.md"), b"unique-sensitive-skill-marker").unwrap();

        delete_guru(&store, &root, &profile, 10).unwrap();

        assert!(store.get_guru("guru-a").unwrap().is_none());
        assert!(!temporary.path().join("runs/guru-a").exists());
        assert!(!tree_contains(
            temporary.path(),
            b"unique-sensitive-skill-marker"
        ));
    }

    #[test]
    fn guru_deletion_refuses_a_swapped_second_guru_tree() {
        let temporary = tempfile::tempdir().unwrap();
        let root = root(temporary.path());
        let store = SqliteStore::open_in_memory().unwrap();
        let profile_a = seed_guru(&store, temporary.path(), "guru-a");
        seed_guru(&store, temporary.path(), "guru-b");
        fs::write(
            temporary.path().join("gurus/guru-b/second-guru-sentinel"),
            b"keep",
        )
        .unwrap();
        fs::rename(
            temporary.path().join("gurus/guru-a"),
            temporary.path().join("gurus/guru-a-original"),
        )
        .unwrap();
        fs::rename(
            temporary.path().join("gurus/guru-b"),
            temporary.path().join("gurus/guru-a"),
        )
        .unwrap();

        assert!(delete_guru(&store, &root, &profile_a, 10).is_err());
        assert_eq!(
            fs::read(temporary.path().join("gurus/guru-a/second-guru-sentinel")).unwrap(),
            b"keep"
        );
        assert!(store.get_guru("guru-a").unwrap().is_some());
        assert!(store.list_deletion_journals().unwrap().is_empty());
    }

    #[test]
    fn recovery_rolls_back_one_live_target_and_continues_cleaning_another() {
        let temporary = tempfile::tempdir().unwrap();
        let root = root(temporary.path());
        let store = SqliteStore::open_in_memory().unwrap();
        seed_guru(&store, temporary.path(), "guru-a");
        let chat = chat("guru-a", "chat-live");
        store.create_chat(&chat).unwrap();
        let live_journal = DeletionJournal {
            id: "delete-chat-chat-live".into(),
            kind: DeletionKind::Chat,
            guru_id: "guru-a".into(),
            target_id: "chat-live".into(),
            expected_source_identity: None,
            phase: DeletionPhase::Detached,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        let absent_journal = DeletionJournal {
            id: "delete-chat-chat-absent".into(),
            kind: DeletionKind::Chat,
            guru_id: "guru-b".into(),
            target_id: "chat-absent".into(),
            expected_source_identity: None,
            phase: DeletionPhase::Detached,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        store.create_deletion_journal(&live_journal).unwrap();
        store.create_deletion_journal(&absent_journal).unwrap();
        for relative in [
            "gurus/guru-a/pi-sessions/.deleting-chat-live",
            "gurus/guru-b/pi-sessions/.deleting-chat-absent",
        ] {
            ensure_private_directory(&temporary.path().join(relative)).unwrap();
        }

        let recovery = recover(&store, &root).unwrap();

        assert!(recovery.quarantined_gurus.is_empty());
        assert!(temporary
            .path()
            .join("gurus/guru-a/pi-sessions/chat-live")
            .is_dir());
        assert!(!temporary
            .path()
            .join("gurus/guru-b/pi-sessions/.deleting-chat-absent")
            .exists());
        assert!(store.list_deletion_journals().unwrap().is_empty());
    }

    #[test]
    fn required_guru_storage_missing_is_quarantined_without_blocking_other_recovery() {
        let temporary = tempfile::tempdir().unwrap();
        let root = root(temporary.path());
        let store = SqliteStore::open_in_memory().unwrap();
        let profile = GuruProfile {
            id: "guru-bad".into(),
            name: "Bad".into(),
            description: String::new(),
            storage_kind: GuruStorageKind::Managed,
            memory_root: temporary
                .path()
                .join("gurus/guru-bad/workspace")
                .display()
                .to_string(),
            root_filesystem_identity: None,
            last_model_profile_id: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        store.create_guru(&profile).unwrap();
        let journal = DeletionJournal {
            id: "delete-guru-guru-bad".into(),
            kind: DeletionKind::Guru,
            guru_id: "guru-bad".into(),
            target_id: "guru-bad".into(),
            expected_source_identity: Some(RootFilesystemIdentity {
                device: 1,
                inode: 1,
            }),
            phase: DeletionPhase::Detached,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        store.create_deletion_journal(&journal).unwrap();

        let recovery = recover(&store, &root).unwrap();

        assert!(recovery.quarantined_gurus.contains_key("guru-bad"));
        assert_eq!(store.list_deletion_journals().unwrap().len(), 1);
    }
}
