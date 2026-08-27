use std::{collections::BTreeSet, fs, sync::Arc};

use super::*;
use crate::{
    app::AppState,
    domain::{ChatMessage, ChatMessageStatus, ChatRole},
};
use serde_json::{json, Value};

const WIKI_MARKDOWN: &str = "---\nid: wiki:quality\ntitle: Quality\nsummary: Stable quality context.\nas_of: 2026-08-12T00:00:00Z\n---\n\n# Quality\n\nDurable fact.\n";

#[tokio::test]
async fn no_change_chat_finalize_remains_exclusive_until_sqlite_returns() {
    use crate::{
        commands::{
            memory_updates::apply_chat_memory_update_with_finalize, tool_executor::ToolCapture,
        },
        run_coordinator::{RunKind, RunTarget},
    };

    let temporary = tempfile::tempdir().unwrap();
    let state = AppState::for_test(temporary.path().join("app"));
    let capture = ToolCapture::default();
    let result = apply_chat_memory_update_with_finalize(
        &state,
        "guru-a",
        "chat-a",
        "message-a",
        false,
        &capture,
        |_| {
            for (run_id, thread_id) in [("same-thread", "chat-a"), ("other-thread", "chat-b")] {
                assert!(state
                    .register_run(
                        run_id.into(),
                        "guru-a".into(),
                        RunKind::Chat,
                        RunTarget::ChatThread(thread_id.into()),
                    )
                    .is_err());
            }
            Ok(())
        },
    )
    .await
    .unwrap();
    assert!(result.0.is_none());

    let admitted = state
        .register_run(
            "after-finalize".into(),
            "guru-a".into(),
            RunKind::Chat,
            RunTarget::ChatThread("chat-a".into()),
        )
        .unwrap();
    drop(admitted);
}

#[cfg(unix)]
#[tokio::test]
async fn chat_memory_write_commits_git_and_previous_version_is_readable() {
    use crate::{
        domain::{MemoryChangeAuthority, MemoryChangeTarget},
        memory_git,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "memory");
    let runtime_path = temporary.path().join("guruterminal-core-memory-fixture");
    super::write_knowledge_runtime(&runtime_path);

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    let written = memory_write::apply_memory_targets(
        &state,
        "guru-a",
        MemoryChangeAuthority::Chat,
        vec![MemoryChangeTarget {
            record_id: "wiki:quality".into(),
            relative_path: "guruterminal/wiki/quality.md".into(),
            before_markdown: String::new(),
            proposed_markdown: WIKI_MARKDOWN.into(),
        }],
        "Keep one reusable Wiki fact.",
    )
    .await
    .unwrap();
    let target = workspace.join("guruterminal/wiki/quality.md");
    assert_eq!(fs::read_to_string(&target).unwrap(), WIKI_MARKDOWN);
    assert!(!written.commit_id.is_empty());
    assert_eq!(
        memory_git::read_previous_markdown(&workspace, "guruterminal/wiki/quality.md").unwrap(),
        None
    );

    memory_write::apply_memory_targets(
        &state,
        "guru-a",
        MemoryChangeAuthority::Chat,
        vec![MemoryChangeTarget {
            record_id: "wiki:quality".into(),
            relative_path: "guruterminal/wiki/quality.md".into(),
            before_markdown: WIKI_MARKDOWN.into(),
            proposed_markdown: WIKI_MARKDOWN.replace("Durable fact.", "Revised fact."),
        }],
        "Revise the Wiki.",
    )
    .await
    .unwrap();
    let previous = memory_git::read_previous_markdown(&workspace, "guruterminal/wiki/quality.md")
        .unwrap()
        .expect("prior version");
    assert_eq!(previous.markdown, WIKI_MARKDOWN);
}

#[cfg(unix)]
#[tokio::test]
async fn failed_chat_finalization_compensates_memory_head_index_and_files() {
    use crate::{
        commands::{
            memory_updates::apply_chat_memory_update_with_finalize, tool_executor::ToolCapture,
        },
        domain::MemoryProposal,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "atomic-finalization");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-atomic-finalization");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));

    let capture = ToolCapture::default();
    capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-atomic".into(),
            "Wiki".into(),
            "wiki:atomic".into(),
            crate::domain::MemoryProposalBase::Absent,
            wiki_markdown("wiki:atomic", "Atomic finalization"),
            "Only persist this Wiki with the completed Chat.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );
    let error = apply_chat_memory_update_with_finalize(
        &state,
        "guru-a",
        "chat-a",
        "message-a",
        true,
        &capture,
        |_| {
            Err::<(), _>(crate::app::CommandError::internal(
                "injected SQLite failure",
            ))
        },
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("injected SQLite failure"));
    assert!(!workspace.join("guruterminal/wiki/atomic.md").exists());
    let repo = git2::Repository::open(&workspace).unwrap();
    assert!(
        repo.head().is_err(),
        "the compensated repository must remain unborn"
    );
    assert_eq!(repo.index().unwrap().len(), 0);
    assert_eq!(state.run_coordinator.active_count(), 0);
    assert!(!state.is_guru_quarantined("guru-a"));
}

#[cfg(unix)]
#[tokio::test]
async fn competing_worktree_edit_after_git_publish_is_preserved_and_quarantined() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::{
        commands::{
            memory_updates::apply_chat_memory_update_with_finalize, tool_executor::ToolCapture,
        },
        domain::MemoryProposal,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "worktree-race");
    let runtime_path = temporary.path().join("guruterminal-core-worktree-race");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));

    let proposed = wiki_markdown("wiki:race", "Journal-owned value");
    let competing = wiki_markdown("wiki:race", "User-owned competing value");
    let target = workspace.join("guruterminal/wiki/race.md");
    let capture = ToolCapture::default();
    capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-race".into(),
            "Wiki".into(),
            "wiki:race".into(),
            crate::domain::MemoryProposalBase::Absent,
            proposed.clone(),
            "Exercise the post-publication worktree CAS.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );
    crate::commands::memory_write::after_memory_git_finalize_for_test({
        let target = target.clone();
        let competing = competing.clone();
        move || fs::write(target, competing).unwrap()
    });
    let finalized = Arc::new(AtomicBool::new(false));
    let finalized_in_callback = finalized.clone();
    let error = apply_chat_memory_update_with_finalize(
        &state,
        "guru-a",
        "chat-a",
        "message-a",
        true,
        &capture,
        move |_| {
            finalized_in_callback.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .await
    .unwrap_err();

    assert_eq!(error.code, "internal");
    assert!(!finalized.load(Ordering::SeqCst));
    assert!(state.is_guru_quarantined("guru-a"));
    assert_eq!(fs::read_to_string(&target).unwrap(), competing);
    assert_eq!(
        state
            .store
            .list_memory_finalization_journals()
            .unwrap()
            .len(),
        1
    );

    let repo = git2::Repository::open(&workspace).unwrap();
    let commit = repo.head().unwrap().peel_to_commit().unwrap();
    let tree = commit.tree().unwrap();
    let entry = tree
        .get_path(std::path::Path::new("guruterminal/wiki/race.md"))
        .unwrap();
    let blob = repo.find_blob(entry.id()).unwrap();
    assert_eq!(std::str::from_utf8(blob.content()).unwrap(), proposed);
}

#[cfg(unix)]
#[tokio::test]
async fn indeterminate_sqlite_error_keeps_an_already_canonical_chat_and_memory() {
    use crate::{
        commands::{
            memory_updates::apply_chat_memory_update_with_finalize, tool_executor::ToolCapture,
        },
        domain::MemoryProposal,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "indeterminate-sqlite-finalization");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-indeterminate-sqlite-finalization");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();

    let capture = ToolCapture::default();
    capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-canonical".into(),
            "Wiki".into(),
            "wiki:canonical".into(),
            crate::domain::MemoryProposalBase::Absent,
            wiki_markdown("wiki:canonical", "Canonical despite error"),
            "Exercise an error reported after SQLite became durable.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );

    let error = apply_chat_memory_update_with_finalize(
        &state,
        "guru-a",
        "chat-a",
        "message-a",
        true,
        &capture,
        |memory_update| {
            let expected = state.store.get_chat("chat-a").unwrap().unwrap();
            let mut finalized = expected.clone();
            finalized.messages.push(ChatMessage {
                id: "message-a".into(),
                role: ChatRole::Assistant,
                status: ChatMessageStatus::Complete,
                content: "The canonical response".into(),
                created_at_ms: 2,
                memory_refs: Vec::new(),
                observed_exact_count: 0,
                refs_truncated: false,
                refs_digest: memory_refs_digest(&[]).unwrap(),
                memory_update,
                memory_revision: None,
                execution_model: None,
                agent_harness: None,
                decision: None,
                attachments: Vec::new(),
                artifact_refs: Vec::new(),
                progress: None,
            });
            finalized.updated_at_ms = 2;
            state.store.replace_chat(&expected, &finalized).unwrap();
            Err::<(), _>(crate::app::CommandError::internal(
                "injected error after durable SQLite commit",
            ))
        },
    )
    .await
    .unwrap_err();

    assert!(error.message.contains("after durable SQLite commit"));
    let finalized = state.store.get_chat("chat-a").unwrap().unwrap();
    let commit_id = finalized.messages[0]
        .memory_update
        .as_ref()
        .and_then(|update| update.commit_id.as_deref())
        .unwrap();
    assert_eq!(
        git2::Repository::open(&workspace)
            .unwrap()
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        commit_id
    );
    assert!(workspace.join("guruterminal/wiki/canonical.md").exists());
    assert!(state
        .store
        .list_memory_finalization_journals()
        .unwrap()
        .is_empty());
    assert!(!state.is_guru_quarantined("guru-a"));
}

#[cfg(unix)]
#[tokio::test]
async fn apply_error_after_publication_compensates_before_deleting_the_journal() {
    use crate::{
        commands::{
            memory_updates::apply_chat_memory_update_with_finalize, tool_executor::ToolCapture,
        },
        domain::MemoryProposal,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "post-publication-apply-error");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-post-publication-apply-error");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));

    let capture = ToolCapture::default();
    capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-post-publish".into(),
            "Wiki".into(),
            "wiki:post-publish".into(),
            crate::domain::MemoryProposalBase::Absent,
            wiki_markdown("wiki:post-publish", "Post-publish failure"),
            "Exercise the rename-before-fsync failure window.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );
    crate::runtime::fail_next_memory_sync_after_publish_for_test();
    let error = apply_chat_memory_update_with_finalize(
        &state,
        "guru-a",
        "chat-a",
        "message-a",
        true,
        &capture,
        |_| Ok::<_, crate::app::CommandError>(()),
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("injected directory sync failure"));
    assert!(!workspace.join("guruterminal/wiki/post-publish.md").exists());
    assert!(state
        .store
        .list_memory_finalization_journals()
        .unwrap()
        .is_empty());
    assert!(!state.is_guru_quarantined("guru-a"));
}

#[cfg(unix)]
#[tokio::test]
async fn failed_git_index_recovery_quarantines_without_splitting_git_and_files() {
    use crate::{
        domain::{MemoryChangeAuthority, MemoryChangeTarget},
        memory_finalization::MemoryFinalizationScope,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "git-recovery");
    let runtime_path = temporary.path().join("guruterminal-core-git-recovery");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    crate::memory_git::fail_next_commit_and_index_restore_for_test();

    let error = memory_write::apply_memory_targets(
        &state,
        "guru-a",
        MemoryChangeAuthority::User,
        vec![MemoryChangeTarget {
            record_id: "wiki:recovery".into(),
            relative_path: "guruterminal/wiki/recovery.md".into(),
            before_markdown: String::new(),
            proposed_markdown: wiki_markdown("wiki:recovery", "Recovery sentinel"),
        }],
        "Exercise recovery quarantine.",
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "internal");
    assert!(state.is_guru_quarantined("guru-a"));
    assert!(workspace.join("guruterminal/wiki/recovery.md").exists());
    let journals = state.store.list_memory_finalization_journals().unwrap();
    assert_eq!(journals.len(), 1);
    let crate::store::MemoryFinalizationJournalRecord::Valid(journal) = &journals[0] else {
        panic!("standalone journal must be valid");
    };
    assert_eq!(journal.scope, MemoryFinalizationScope::StandaloneUser);

    // The first compensation was interrupted after the proposed index was
    // published. Retrying in-process must use the journal's exact CAS recipe,
    // restore HEAD/index/files, and only then remove the journal.
    memory_write::retry_quarantined_guru_recovery(&state, "guru-a")
        .await
        .unwrap();
    assert!(!state.is_guru_quarantined("guru-a"));
    assert!(!workspace.join("guruterminal/wiki/recovery.md").exists());
    let repo = git2::Repository::open(&workspace).unwrap();
    assert!(repo.head().is_err());
    assert_eq!(repo.index().unwrap().len(), 0);
    assert!(state
        .store
        .list_memory_finalization_journals()
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn queued_memory_writers_recheck_quarantine_before_registered_apply() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::{
        commands::{memory_updates, tool_executor::ToolCapture},
        domain::{MemoryChangeAuthority, MemoryChangeTarget, MemoryWrite},
        run_coordinator::RunTarget,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "queued-writer-quarantine");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-queued-writer-quarantine");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));

    let first_id = "memory-write:queued-first".to_owned();
    let first_writer = state
        .register_memory_write(
            first_id.clone(),
            "guru-a".into(),
            RunTarget::MemoryWriteSession(first_id.clone()),
        )
        .await
        .unwrap();
    let second_id = "memory-write:queued-second".to_owned();
    let second_pending = state
        .reserve_memory_write(
            second_id.clone(),
            "guru-a".into(),
            RunTarget::MemoryWriteSession(second_id.clone()),
        )
        .unwrap();
    let (chat_write_id, chat_pending) =
        memory_updates::reserve_chat_memory_finalization(&state, "guru-a").unwrap();
    assert_eq!(state.run_coordinator.active_count(), 3);

    crate::memory_git::fail_next_commit_and_index_restore_for_test();
    let first_error = memory_write::apply_memory_targets_registered(
        &state,
        memory_write::RegisteredMemoryTransaction::standalone(
            MemoryWrite {
                guru_id: "guru-a".into(),
                authority: MemoryChangeAuthority::User,
                targets: vec![MemoryChangeTarget {
                    record_id: "wiki:queued-first".into(),
                    relative_path: "guruterminal/wiki/queued-first.md".into(),
                    before_markdown: String::new(),
                    proposed_markdown: wiki_markdown("wiki:queued-first", "First queued writer"),
                }],
                rationale: "Quarantine after publishing the first writer's index.".into(),
            },
            first_id,
            first_writer,
        ),
        |_| Ok(()),
    )
    .await
    .unwrap_err();
    assert_eq!(first_error.code, "internal");
    assert!(state.is_guru_quarantined("guru-a"));

    let second_writer = second_pending.wait().await.unwrap();
    let second_error = memory_write::apply_memory_targets_registered(
        &state,
        memory_write::RegisteredMemoryTransaction::standalone(
            MemoryWrite {
                guru_id: "guru-a".into(),
                authority: MemoryChangeAuthority::User,
                targets: vec![MemoryChangeTarget {
                    record_id: "wiki:queued-second".into(),
                    relative_path: "guruterminal/wiki/queued-second.md".into(),
                    before_markdown: String::new(),
                    proposed_markdown: wiki_markdown("wiki:queued-second", "Second queued writer"),
                }],
                rationale: "This queued writer must not start after quarantine.".into(),
            },
            second_id,
            second_writer,
        ),
        |_| Ok(()),
    )
    .await
    .unwrap_err();
    assert_eq!(second_error.code, "guru_recovery_required");

    let chat_writer = chat_pending.wait().await.unwrap();
    let capture = ToolCapture::default();
    let finalized = Arc::new(AtomicBool::new(false));
    let finalized_in_callback = finalized.clone();
    let chat_error = memory_updates::apply_chat_memory_update_with_registered_finalize(
        &state,
        "guru-a",
        "chat-a",
        "message-a",
        false,
        &capture,
        chat_write_id,
        chat_writer,
        move |_| {
            finalized_in_callback.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .await
    .unwrap_err();
    assert_eq!(chat_error.code, "guru_recovery_required");
    assert!(!finalized.load(Ordering::SeqCst));

    assert!(workspace.join("guruterminal/wiki/queued-first.md").exists());
    assert!(!workspace
        .join("guruterminal/wiki/queued-second.md")
        .exists());
    let repo = git2::Repository::open(&workspace).unwrap();
    assert!(repo.head().is_err());
    assert_eq!(repo.index().unwrap().len(), 1);
    assert_eq!(
        state
            .store
            .list_memory_finalization_journals()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(state.run_coordinator.active_count(), 0);

    memory_write::retry_quarantined_guru_recovery(&state, "guru-a")
        .await
        .unwrap();
    assert!(!state.is_guru_quarantined("guru-a"));
    assert!(!workspace.join("guruterminal/wiki/queued-first.md").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn restart_recovers_standalone_failure_after_index_publish_before_head_update() {
    use crate::domain::{MemoryChangeAuthority, MemoryChangeTarget};

    let temporary = tempfile::tempdir().unwrap();
    let app_data = temporary.path().join("app");
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "standalone-index-publish-crash");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-standalone-index-publish-crash");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_persistent_test(app_data.clone());
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path.clone()).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    crate::memory_git::fail_next_commit_and_index_restore_for_test();

    let error = memory_write::apply_memory_targets(
        &state,
        "guru-a",
        MemoryChangeAuthority::User,
        vec![MemoryChangeTarget {
            record_id: "wiki:restart-recovery".into(),
            relative_path: "guruterminal/wiki/restart-recovery.md".into(),
            before_markdown: String::new(),
            proposed_markdown: wiki_markdown("wiki:restart-recovery", "Restart recovery sentinel"),
        }],
        "Exercise restart recovery after index publication.",
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "internal");
    let target = workspace.join("guruterminal/wiki/restart-recovery.md");
    assert!(target.exists());
    let repo = git2::Repository::open(&workspace).unwrap();
    assert!(
        repo.head().is_err(),
        "HEAD publication must not have happened"
    );
    assert_eq!(
        repo.index().unwrap().len(),
        1,
        "the proposed index survived"
    );
    drop(repo);
    state.close_for_restart_test();

    let mut restarted = AppState::for_persistent_test(app_data);
    restarted.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    assert!(restarted.is_guru_quarantined("guru-a"));
    memory_write::retry_quarantined_guru_recovery(&restarted, "guru-a")
        .await
        .unwrap();
    assert!(!restarted.is_guru_quarantined("guru-a"));
    assert!(!target.exists());
    let repo = git2::Repository::open(&workspace).unwrap();
    assert!(repo.head().is_err());
    assert_eq!(repo.index().unwrap().len(), 0);
    assert!(restarted
        .store
        .list_memory_finalization_journals()
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn restart_accepts_a_finalized_standalone_memory_journal() {
    use crate::{
        hashing::sha256,
        memory_finalization::{
            MemoryFinalizationJournal, MemoryFinalizationScope, MEMORY_FINALIZATION_SCHEMA_VERSION,
        },
        memory_git::MemoryGitChange,
        runtime::StagedMemoryChange,
    };

    let temporary = tempfile::tempdir().unwrap();
    let app_data = temporary.path().join("app");
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "standalone-finalized-crash");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-standalone-finalized-crash");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_persistent_test(app_data.clone());
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path.clone()).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));

    let write_id = "memory-write:standalone-finalized";
    let relative_path = std::path::PathBuf::from("guruterminal/wiki/standalone-finalized.md");
    let proposed = wiki_markdown("wiki:standalone-finalized", "Finalized standalone write");
    let changes = vec![StagedMemoryChange {
        guru_id: "guru-a".into(),
        session_id: write_id.into(),
        relative_path: relative_path.clone(),
        before_sha256: None,
        before_markdown: None,
        proposed_sha256: sha256(proposed.as_bytes()),
        proposed_markdown: proposed.clone(),
        delete: false,
    }];
    let snapshot = crate::memory_git::begin_memory_transaction(&workspace).unwrap();
    let mut journal = MemoryFinalizationJournal {
        schema_version: MEMORY_FINALIZATION_SCHEMA_VERSION,
        id: write_id.into(),
        guru_id: "guru-a".into(),
        scope: MemoryFinalizationScope::StandaloneUser,
        updated_at_ms: 1,
        git: snapshot.clone(),
        changes: changes.clone(),
        commit_id: None,
    };
    state
        .store
        .create_memory_finalization_journal(&journal)
        .unwrap();
    let runtime = state.runtime().unwrap();
    bound_root(&workspace)
        .apply_memory_markdown_set(&runtime, &changes)
        .await
        .unwrap();
    let prepared = crate::memory_git::prepare_memory_commit_exact(
        &workspace,
        "user: finalized standalone transaction",
        &snapshot,
        &[MemoryGitChange {
            relative_path,
            contents: Some(proposed.as_bytes().to_vec()),
        }],
    )
    .unwrap();
    let expected = journal.clone();
    journal.commit_id = Some(prepared.commit_id.clone());
    journal.git.published_index_tree = Some(prepared.index_tree_id.clone());
    journal.updated_at_ms = 2;
    state
        .store
        .replace_memory_finalization_journal(&expected, &journal)
        .unwrap();
    crate::memory_git::finalize_memory_commit(&workspace, prepared).unwrap();
    state.close_for_restart_test();

    let mut restarted = AppState::for_persistent_test(app_data);
    restarted.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    assert!(restarted.is_guru_quarantined("guru-a"));
    memory_write::retry_quarantined_guru_recovery(&restarted, "guru-a")
        .await
        .unwrap();
    assert!(!restarted.is_guru_quarantined("guru-a"));
    assert_eq!(
        fs::read_to_string(workspace.join("guruterminal/wiki/standalone-finalized.md")).unwrap(),
        proposed
    );
    assert_eq!(
        git2::Repository::open(&workspace)
            .unwrap()
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        journal.commit_id.unwrap()
    );
    assert!(restarted
        .store
        .list_memory_finalization_journals()
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn restart_quarantines_and_recovers_a_crash_after_memory_commit_before_chat_finalize() {
    use crate::{
        hashing::sha256,
        memory_finalization::{
            MemoryFinalizationJournal, MemoryFinalizationScope, MEMORY_FINALIZATION_SCHEMA_VERSION,
        },
        runtime::StagedMemoryChange,
    };

    let temporary = tempfile::tempdir().unwrap();
    let app_data = temporary.path().join("app");
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "crash-finalization");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-crash-finalization");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_persistent_test(app_data.clone());
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path.clone()).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));

    let write_id = "memory-write:crash";
    let proposed = wiki_markdown("wiki:crash", "Crash recovery");
    let changes = vec![StagedMemoryChange {
        guru_id: "guru-a".into(),
        session_id: write_id.into(),
        relative_path: "guruterminal/wiki/crash.md".into(),
        before_sha256: None,
        before_markdown: None,
        proposed_sha256: sha256(proposed.as_bytes()),
        proposed_markdown: proposed,
        delete: false,
    }];
    let snapshot = crate::memory_git::begin_memory_transaction(&workspace).unwrap();
    let mut journal = MemoryFinalizationJournal {
        schema_version: MEMORY_FINALIZATION_SCHEMA_VERSION,
        id: write_id.into(),
        guru_id: "guru-a".into(),
        scope: MemoryFinalizationScope::Chat {
            thread_id: "chat-a".into(),
            message_id: "message-a".into(),
        },
        updated_at_ms: 1,
        git: snapshot.clone(),
        changes: changes.clone(),
        commit_id: None,
    };
    state
        .store
        .create_memory_finalization_journal(&journal)
        .unwrap();
    let runtime = state.runtime().unwrap();
    let bound = bound_root(&workspace);
    bound
        .apply_memory_markdown_set(&runtime, &changes)
        .await
        .unwrap();
    let prepared = crate::memory_git::prepare_memory_commit(
        &workspace,
        "chat: interrupted finalization",
        &snapshot,
    )
    .unwrap();
    let previous = journal.clone();
    journal.commit_id = Some(prepared.commit_id.clone());
    journal.git.published_index_tree = Some(prepared.index_tree_id.clone());
    journal.updated_at_ms = 2;
    state
        .store
        .replace_memory_finalization_journal(&previous, &journal)
        .unwrap();
    crate::memory_git::finalize_memory_commit(&workspace, prepared).unwrap();
    assert!(workspace.join("guruterminal/wiki/crash.md").exists());
    state.close_for_restart_test();

    let mut restarted = AppState::for_persistent_test(app_data.clone());
    restarted.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path.clone()).unwrap(),
    ));
    assert!(restarted.is_guru_quarantined("guru-a"));
    memory_write::retry_quarantined_guru_recovery(&restarted, "guru-a")
        .await
        .unwrap();
    assert!(!restarted.is_guru_quarantined("guru-a"));
    assert!(!workspace.join("guruterminal/wiki/crash.md").exists());
    assert!(git2::Repository::open(&workspace).unwrap().head().is_err());
    assert!(restarted
        .store
        .list_memory_finalization_journals()
        .unwrap()
        .is_empty());

    // A crash immediately after the durable intent, before any file was
    // published, is also idempotently recoverable.
    let before_apply_snapshot = crate::memory_git::begin_memory_transaction(&workspace).unwrap();
    let before_apply_change = StagedMemoryChange {
        guru_id: "guru-a".into(),
        session_id: "memory-write:before-apply".into(),
        relative_path: "guruterminal/wiki/before-apply.md".into(),
        before_sha256: None,
        before_markdown: None,
        proposed_sha256: sha256(b"proposed"),
        proposed_markdown: "proposed".into(),
        delete: false,
    };
    restarted
        .store
        .create_memory_finalization_journal(&MemoryFinalizationJournal {
            schema_version: MEMORY_FINALIZATION_SCHEMA_VERSION,
            id: "memory-write:before-apply".into(),
            guru_id: "guru-a".into(),
            scope: MemoryFinalizationScope::Chat {
                thread_id: "chat-b".into(),
                message_id: "message-b".into(),
            },
            updated_at_ms: 3,
            git: before_apply_snapshot,
            changes: vec![before_apply_change],
            commit_id: None,
        })
        .unwrap();
    restarted.close_for_restart_test();
    let mut restarted = AppState::for_persistent_test(app_data);
    restarted.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    assert!(restarted.is_guru_quarantined("guru-a"));
    memory_write::retry_quarantined_guru_recovery(&restarted, "guru-a")
        .await
        .unwrap();
    assert!(!restarted.is_guru_quarantined("guru-a"));
    assert!(restarted
        .store
        .list_memory_finalization_journals()
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn restart_finishes_a_crash_after_sqlite_finalize_without_rolling_memory_back() {
    use crate::{
        domain::{MemoryUpdateChange, MemoryUpdateResult, MemoryUpdateStatus},
        hashing::sha256,
        memory_finalization::{
            MemoryFinalizationJournal, MemoryFinalizationScope, MEMORY_FINALIZATION_SCHEMA_VERSION,
        },
        runtime::StagedMemoryChange,
    };

    let temporary = tempfile::tempdir().unwrap();
    let app_data = temporary.path().join("app");
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "post-sqlite-finalization");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-post-sqlite-finalization");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_persistent_test_stage(
        app_data.clone(),
        "post-SQLite-finalization initial instance",
    );
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path.clone()).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();

    let proposed = wiki_markdown("wiki:finalized", "Finalized crash");
    let changes = vec![StagedMemoryChange {
        guru_id: "guru-a".into(),
        session_id: "memory-write:finalized".into(),
        relative_path: "guruterminal/wiki/finalized.md".into(),
        before_sha256: None,
        before_markdown: None,
        proposed_sha256: sha256(proposed.as_bytes()),
        proposed_markdown: proposed.clone(),
        delete: false,
    }];
    let snapshot = crate::memory_git::begin_memory_transaction(&workspace).unwrap();
    let mut journal = MemoryFinalizationJournal {
        schema_version: MEMORY_FINALIZATION_SCHEMA_VERSION,
        id: "memory-write:finalized".into(),
        guru_id: "guru-a".into(),
        scope: MemoryFinalizationScope::Chat {
            thread_id: "chat-a".into(),
            message_id: "message-a".into(),
        },
        updated_at_ms: 1,
        git: snapshot.clone(),
        changes: changes.clone(),
        commit_id: None,
    };
    state
        .store
        .create_memory_finalization_journal(&journal)
        .unwrap();
    let runtime = state.runtime().unwrap();
    bound_root(&workspace)
        .apply_memory_markdown_set(&runtime, &changes)
        .await
        .unwrap();
    let prepared = crate::memory_git::prepare_memory_commit(
        &workspace,
        "chat: finalized before journal cleanup",
        &snapshot,
    )
    .unwrap();
    let previous_journal = journal.clone();
    journal.commit_id = Some(prepared.commit_id.clone());
    journal.git.published_index_tree = Some(prepared.index_tree_id.clone());
    journal.updated_at_ms = 2;
    state
        .store
        .replace_memory_finalization_journal(&previous_journal, &journal)
        .unwrap();
    crate::memory_git::finalize_memory_commit(&workspace, prepared).unwrap();

    let expected_chat = state.store.get_chat("chat-a").unwrap().unwrap();
    let mut finalized_chat = expected_chat.clone();
    finalized_chat.messages.push(ChatMessage {
        id: "message-a".into(),
        role: ChatRole::Assistant,
        status: ChatMessageStatus::Complete,
        content: "Durably finalized response".into(),
        created_at_ms: 2,
        memory_refs: Vec::new(),
        observed_exact_count: 0,
        refs_truncated: false,
        refs_digest: memory_refs_digest(&[]).unwrap(),
        memory_update: Some(MemoryUpdateResult {
            status: MemoryUpdateStatus::Applied,
            commit_id: journal.commit_id.clone(),
            changes: vec![MemoryUpdateChange {
                record_id: "wiki:finalized".into(),
                kind: "Wiki".into(),
                operation: "create".into(),
                title: "Finalized crash".into(),
                lesson: "Keep the finalized Memory transaction.".into(),
                basis: "The exact Chat finalization journal.".into(),
                future_use: "Verify restart recovery.".into(),
            }],
        }),
        memory_revision: None,
        execution_model: None,
        agent_harness: None,
        decision: None,
        attachments: Vec::new(),
        artifact_refs: Vec::new(),
        progress: None,
    });
    finalized_chat.updated_at_ms = 2;
    state
        .store
        .replace_chat(&expected_chat, &finalized_chat)
        .unwrap();
    state.close_for_restart_test();

    let mut restarted = AppState::for_persistent_test_stage(
        app_data,
        "post-SQLite-finalization restarted instance",
    );
    restarted.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    assert!(restarted.is_guru_quarantined("guru-a"));
    memory_write::retry_quarantined_guru_recovery(&restarted, "guru-a")
        .await
        .unwrap();
    assert!(!restarted.is_guru_quarantined("guru-a"));
    assert_eq!(
        fs::read_to_string(workspace.join("guruterminal/wiki/finalized.md")).unwrap(),
        proposed
    );
    assert_eq!(
        git2::Repository::open(&workspace)
            .unwrap()
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        journal.commit_id.unwrap()
    );
    assert!(restarted
        .store
        .list_memory_finalization_journals()
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn memory_recovery_reconciles_both_atomic_exchange_crash_windows() {
    use crate::{hashing::sha256, runtime::StagedMemoryChange};

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "artifact-crash-windows");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-artifact-crash-windows");
    super::write_knowledge_runtime(&runtime_path);
    let runtime = crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap();
    let bound = bound_root(&workspace);

    // Crash before publication: the canonical target is still its before
    // state while the transaction artifact contains the proposal.
    let proposed = wiki_markdown("wiki:pre-publish", "Pre-publish crash");
    let pre_publish = StagedMemoryChange {
        guru_id: "guru-a".into(),
        session_id: "memory-write:pre-publish".into(),
        relative_path: "guruterminal/wiki/pre-publish.md".into(),
        before_sha256: None,
        before_markdown: None,
        proposed_sha256: sha256(proposed.as_bytes()),
        proposed_markdown: proposed.clone(),
        delete: false,
    };
    bound
        .stage_memory_artifact_for_test(&runtime, &pre_publish, proposed.as_bytes())
        .unwrap();
    memory_write::rollback_memory_changes_idempotent(
        &bound,
        &runtime,
        std::slice::from_ref(&pre_publish),
    )
    .await
    .unwrap();
    // A clean apply proves the orphaned artifact was removed.
    bound
        .apply_memory_markdown_set(&runtime, std::slice::from_ref(&pre_publish))
        .await
        .unwrap();
    bound
        .rollback_memory_markdown_set(&runtime, std::slice::from_ref(&pre_publish))
        .await
        .unwrap();

    // Crash after EXCHANGE: target contains the proposal and the artifact
    // contains the displaced before image.
    let before = wiki_markdown("wiki:post-exchange", "Before exchange");
    let after = wiki_markdown("wiki:post-exchange", "After exchange");
    let target = workspace.join("guruterminal/wiki/post-exchange.md");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, &after).unwrap();
    let post_exchange = StagedMemoryChange {
        guru_id: "guru-a".into(),
        session_id: "memory-write:post-exchange".into(),
        relative_path: "guruterminal/wiki/post-exchange.md".into(),
        before_sha256: Some(sha256(before.as_bytes())),
        before_markdown: Some(before.clone()),
        proposed_sha256: sha256(after.as_bytes()),
        proposed_markdown: after,
        delete: false,
    };
    bound
        .stage_memory_artifact_for_test(&runtime, &post_exchange, before.as_bytes())
        .unwrap();
    memory_write::rollback_memory_changes_idempotent(
        &bound,
        &runtime,
        std::slice::from_ref(&post_exchange),
    )
    .await
    .unwrap();
    assert_eq!(fs::read_to_string(target).unwrap(), before);
}

#[cfg(unix)]
#[tokio::test]
async fn failed_recovery_retains_its_durable_journal_for_retry() {
    use crate::{
        hashing::sha256,
        memory_finalization::{
            MemoryFinalizationJournal, MemoryFinalizationScope, MEMORY_FINALIZATION_SCHEMA_VERSION,
        },
        runtime::StagedMemoryChange,
    };

    let temporary = tempfile::tempdir().unwrap();
    let app_data = temporary.path().join("app");
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "retained-recovery-journal");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-retained-recovery-journal");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_persistent_test(app_data.clone());
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path.clone()).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));

    let proposed = wiki_markdown("wiki:retained", "Retained journal");
    let change = StagedMemoryChange {
        guru_id: "guru-a".into(),
        session_id: "memory-write:retained".into(),
        relative_path: "guruterminal/wiki/retained.md".into(),
        before_sha256: None,
        before_markdown: None,
        proposed_sha256: sha256(proposed.as_bytes()),
        proposed_markdown: proposed,
        delete: false,
    };
    let journal = MemoryFinalizationJournal {
        schema_version: MEMORY_FINALIZATION_SCHEMA_VERSION,
        id: "memory-write:retained".into(),
        guru_id: "guru-a".into(),
        scope: MemoryFinalizationScope::Chat {
            thread_id: "chat-a".into(),
            message_id: "message-a".into(),
        },
        updated_at_ms: 1,
        git: crate::memory_git::begin_memory_transaction(&workspace).unwrap(),
        changes: vec![change],
        commit_id: None,
    };
    state
        .store
        .create_memory_finalization_journal(&journal)
        .unwrap();
    let target = workspace.join("guruterminal/wiki/retained.md");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "concurrent bytes outside the journal").unwrap();
    state.close_for_restart_test();

    let mut restarted = AppState::for_persistent_test(app_data);
    restarted.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    assert!(restarted.is_guru_quarantined("guru-a"));
    assert!(
        memory_write::retry_quarantined_guru_recovery(&restarted, "guru-a")
            .await
            .is_err()
    );
    assert_eq!(
        restarted
            .store
            .list_memory_finalization_journals()
            .unwrap()
            .len(),
        1,
        "failed compensation must retain its recovery recipe"
    );
    assert!(restarted.is_guru_quarantined("guru-a"));

    fs::remove_file(target).unwrap();
    memory_write::retry_quarantined_guru_recovery(&restarted, "guru-a")
        .await
        .unwrap();
    assert!(!restarted.is_guru_quarantined("guru-a"));
    assert!(restarted
        .store
        .list_memory_finalization_journals()
        .unwrap()
        .is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn memory_revert_restores_exact_prior_bytes() {
    use crate::{
        domain::{MemoryChangeAuthority, MemoryChangeTarget},
        hashing::sha256,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "memory-revert");
    let runtime_path = temporary.path().join("guruterminal-core-memory-revert");
    super::write_knowledge_runtime(&runtime_path);

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    memory_write::apply_memory_targets(
        &state,
        "guru-a",
        MemoryChangeAuthority::Chat,
        vec![MemoryChangeTarget {
            record_id: "wiki:quality".into(),
            relative_path: "guruterminal/wiki/quality.md".into(),
            before_markdown: String::new(),
            proposed_markdown: WIKI_MARKDOWN.into(),
        }],
        "Keep one reusable Wiki fact.",
    )
    .await
    .unwrap();
    let revised = WIKI_MARKDOWN.replace("Durable fact.", "Revised fact.");
    memory_write::apply_memory_targets(
        &state,
        "guru-a",
        MemoryChangeAuthority::Chat,
        vec![MemoryChangeTarget {
            record_id: "wiki:quality".into(),
            relative_path: "guruterminal/wiki/quality.md".into(),
            before_markdown: WIKI_MARKDOWN.into(),
            proposed_markdown: revised.clone(),
        }],
        "Revise the Wiki.",
    )
    .await
    .unwrap();

    crate::commands::memory_crud::revert_memory_record(
        &state,
        "guru-a",
        "wiki:quality",
        &sha256(revised.as_bytes()),
    )
    .await
    .unwrap();
    let target = workspace.join("guruterminal/wiki/quality.md");
    assert_eq!(fs::read_to_string(&target).unwrap(), WIKI_MARKDOWN);
}

#[cfg(unix)]
#[tokio::test]
async fn memory_revert_rejects_concurrent_modification() {
    use crate::{
        domain::{MemoryChangeAuthority, MemoryChangeTarget},
        hashing::sha256,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "memory-revert-conflict");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-memory-revert-conflict");
    super::write_knowledge_runtime(&runtime_path);

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    memory_write::apply_memory_targets(
        &state,
        "guru-a",
        MemoryChangeAuthority::Chat,
        vec![MemoryChangeTarget {
            record_id: "wiki:quality".into(),
            relative_path: "guruterminal/wiki/quality.md".into(),
            before_markdown: String::new(),
            proposed_markdown: WIKI_MARKDOWN.into(),
        }],
        "Keep one reusable Wiki fact.",
    )
    .await
    .unwrap();
    let revised = WIKI_MARKDOWN.replace("Durable fact.", "Revised fact.");
    memory_write::apply_memory_targets(
        &state,
        "guru-a",
        MemoryChangeAuthority::Chat,
        vec![MemoryChangeTarget {
            record_id: "wiki:quality".into(),
            relative_path: "guruterminal/wiki/quality.md".into(),
            before_markdown: WIKI_MARKDOWN.into(),
            proposed_markdown: revised.clone(),
        }],
        "Revise the Wiki.",
    )
    .await
    .unwrap();

    let target = workspace.join("guruterminal/wiki/quality.md");
    let tampered = revised.replace("Revised fact.", "Tampered fact.");
    fs::write(&target, &tampered).unwrap();
    let error = crate::commands::memory_crud::revert_memory_record(
        &state,
        "guru-a",
        "wiki:quality",
        &sha256(revised.as_bytes()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "conflict");
    assert_eq!(fs::read_to_string(&target).unwrap(), tampered);
}

#[cfg(unix)]
#[tokio::test]
async fn applied_research_wiki_is_searchable_without_a_decision() {
    use crate::{
        commands::{memory_updates::apply_chat_memory_update, tool_executor::ToolCapture},
        domain::{ChatMessage, ChatMessageStatus, ChatRole, MemoryProposal, MemoryUpdateStatus},
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "two-cycle");
    let runtime_path = temporary.path().join("guruterminal-core-two-cycle");
    super::write_knowledge_runtime(&runtime_path);

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    let mut session = chat("chat-a", "guru-a", 1);
    session.messages.push(ChatMessage {
        id: "message-a".into(),
        role: ChatRole::Assistant,
        status: ChatMessageStatus::Complete,
        content: "Research the EV industry and keep the reusable facts.".into(),
        created_at_ms: 1,
        memory_refs: Vec::new(),
        observed_exact_count: 0,
        refs_truncated: false,
        refs_digest: memory_refs_digest(&[]).unwrap(),
        memory_update: None,
        memory_revision: None,
        execution_model: None,
        agent_harness: None,
        decision: None,
        attachments: Vec::new(),
        artifact_refs: Vec::new(),
        progress: None,
    });
    state.store.create_chat(&session).unwrap();

    let capture = ToolCapture::default();
    let proposal = MemoryProposal::new(
        "proposal-ev".into(),
        "Wiki".into(),
        "wiki:ev-industry".into(),
        crate::domain::MemoryProposalBase::Absent,
        wiki_markdown("wiki:ev-industry", "EV industry"),
        "Compile durable EV industry facts from current research.".into(),
        Vec::new(),
        None,
    )
    .unwrap();
    capture.proposal.lock().await.push(proposal);

    let result = apply_chat_memory_update(&state, "guru-a", "chat-a", "message-a", true, &capture)
        .await
        .unwrap()
        .expect("research-learn apply");
    assert_eq!(result.status, MemoryUpdateStatus::Applied);
    assert!(result.changes.iter().any(|change| change.kind == "Wiki"));
    assert!(!result
        .changes
        .iter()
        .any(|change| change.kind == "Decision"));

    let runtime = state.runtime().unwrap();
    let bound = bound_root(&workspace);
    let search = bound
        .knowledge_search(&runtime, "EV industry", Some("wiki"), 8, false, None)
        .await
        .unwrap();
    assert!(
        search
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "wiki:ev-industry"),
        "{search}"
    );
    let read = bound
        .knowledge_read(&runtime, "wiki:ev-industry", None)
        .await
        .unwrap();
    assert_eq!(read["document"]["id"], "wiki:ev-industry");
}

#[cfg(unix)]
#[tokio::test]
async fn applied_evidence_dossier_is_searchable_without_update_memory() {
    use crate::{
        commands::{
            memory_updates::apply_chat_memory_update,
            tool_executor::{
                EvidenceCitation, RunResult, RunResultProducer, StagedEvidence, ToolCapture,
            },
        },
        domain::{ChatMessage, ChatMessageStatus, ChatRole, MemoryUpdateStatus},
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "evidence-dossier");
    let runtime_path = temporary.path().join("guruterminal-core-evidence-dossier");
    super::write_knowledge_runtime(&runtime_path);

    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    let mut session = chat("chat-a", "guru-a", 1);
    session.messages.push(ChatMessage {
        id: "message-a".into(),
        role: ChatRole::Assistant,
        status: ChatMessageStatus::Complete,
        content: "TSMC packaging is tight this quarter.".into(),
        created_at_ms: 1,
        memory_refs: Vec::new(),
        observed_exact_count: 0,
        refs_truncated: false,
        refs_digest: memory_refs_digest(&[]).unwrap(),
        memory_update: None,
        memory_revision: None,
        execution_model: None,
        agent_harness: None,
        decision: None,
        attachments: Vec::new(),
        artifact_refs: Vec::new(),
        progress: None,
    });
    state.store.create_chat(&session).unwrap();

    let capture = ToolCapture::default();
    let receipt = RunResult {
        result_ref: "result:tsmc".into(),
        producer: RunResultProducer {
            runtime_id: "native-web".into(),
            tool_name: "web_fetch".into(),
            provider: Some("example.test".into()),
        },
        origin: Some("https://example.test/tsmc".into()),
        request_digest: "a".repeat(64),
        response_digest: "b".repeat(64),
        retrieved_at: "2026-08-13T15:30:00Z".into(),
        payload: serde_json::json!({"utilization": "rose"}),
        warnings: Vec::new(),
        upstream_result_refs: Vec::new(),
    };
    let receipt = receipt.receipt();
    capture.staged_evidence.lock().await.push(StagedEvidence {
        evidence_id: "evidence:chat/tsmc".into(),
        title: "TSMC 3nm capacity".into(),
        summary: "Packaging tightness from this research turn.".into(),
        as_of: "2026-08-13T15:30:00Z".into(),
        markdown: "3nm utilization rose on CoWoS tightness.".into(),
        source: Some("https://example.test/tsmc".into()),
        period: None,
        entities: Vec::new(),
        citations: vec![EvidenceCitation {
            result_ref: receipt.result_ref.clone(),
            note: Some("TSMC filing".into()),
            receipt,
        }],
    });

    let result = apply_chat_memory_update(&state, "guru-a", "chat-a", "message-a", false, &capture)
        .await
        .unwrap()
        .expect("evidence dossier apply");
    assert_eq!(result.status, MemoryUpdateStatus::Applied);
    assert!(result
        .changes
        .iter()
        .any(|change| change.kind == "Evidence"));
    assert!(!result.changes.iter().any(|change| change.kind == "Wiki"));
    let record_id = result
        .changes
        .iter()
        .find(|change| change.kind == "Evidence")
        .unwrap()
        .record_id
        .clone();
    assert_eq!(record_id, "evidence:chat/tsmc");

    let runtime = state.runtime().unwrap();
    let bound = bound_root(&workspace);
    let search = bound
        .knowledge_search(&runtime, "TSMC", Some("evidence"), 8, false, None)
        .await
        .unwrap();
    assert!(
        search
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == record_id),
        "{search}"
    );
    let read = bound
        .knowledge_read(&runtime, &record_id, None)
        .await
        .unwrap();
    assert!(read["content"]
        .as_str()
        .unwrap()
        .contains("3nm utilization rose on CoWoS tightness."));
    assert!(read["content"].as_str().unwrap().contains("# Sources"));
    assert!(!read["content"].as_str().unwrap().contains("result:tsmc"));
}

fn assistant_message(id: &str, content: &str) -> ChatMessage {
    ChatMessage {
        id: id.into(),
        role: ChatRole::Assistant,
        status: ChatMessageStatus::Complete,
        content: content.into(),
        created_at_ms: 1,
        memory_refs: Vec::new(),
        observed_exact_count: 0,
        refs_truncated: false,
        refs_digest: memory_refs_digest(&[]).unwrap(),
        memory_update: None,
        memory_revision: None,
        execution_model: None,
        agent_harness: None,
        decision: None,
        attachments: Vec::new(),
        artifact_refs: Vec::new(),
        progress: None,
    }
}

fn append_assistant_message(state: &AppState, chat_id: &str, id: &str, content: &str) {
    let expected = state.store.get_chat(chat_id).unwrap().unwrap();
    let mut updated = expected.clone();
    updated.messages.push(assistant_message(id, content));
    updated.updated_at_ms = updated.updated_at_ms.saturating_add(1);
    state.store.replace_chat(&expected, &updated).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn research_learn_without_a_proposal_is_no_durable_lesson() {
    use crate::{
        commands::{memory_updates::apply_chat_memory_update, tool_executor::ToolCapture},
        domain::MemoryUpdateStatus,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "no-lesson");
    let runtime_path = temporary.path().join("guruterminal-core-no-lesson");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    let mut session = chat("chat-a", "guru-a", 1);
    session.messages.push(assistant_message(
        "message-a",
        "One quarter of better pricing is not enough to rewrite the quality bar.",
    ));
    state.store.create_chat(&session).unwrap();

    let result = apply_chat_memory_update(
        &state,
        "guru-a",
        "chat-a",
        "message-a",
        true,
        &ToolCapture::default(),
    )
    .await
    .unwrap()
    .expect("update-memory turn");
    assert_eq!(result.status, MemoryUpdateStatus::NoChange);
    assert!(result.changes.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn applied_wiki_is_in_the_learned_index_and_later_english_search() {
    use crate::{
        agent_harness,
        commands::{memory_updates::apply_chat_memory_update, tool_executor::ToolCapture},
        domain::{MemoryProposal, MemoryUpdateStatus},
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "later-use");
    let runtime_path = temporary.path().join("guruterminal-core-later-use");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    let mut session = chat("chat-a", "guru-a", 1);
    session.messages.push(assistant_message(
        "message-a",
        "Compile the durable foundry constraint and reuse it in later capacity reviews.",
    ));
    state.store.create_chat(&session).unwrap();

    let capture = ToolCapture::default();
    capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-foundry".into(),
            "Wiki".into(),
            "wiki:tsmc-foundry-economics".into(),
            crate::domain::MemoryProposalBase::Absent,
            "---\nid: wiki:tsmc-foundry-economics\ntitle: TSMC foundry economics\nsummary: Advanced packaging, not wafer starts, is the binding capacity constraint for leading-edge TSMC nodes.\nas_of: 2026-03-15T00:00:00Z\naliases:\n  - Taiwan Semiconductor\n  - 2330.TW\nentities:\n  - TSMC\n---\n\n# Constraint\n\nCoWoS remains tighter than leading-edge wafer starts.\n".into(),
            "Keep a reusable foundry constraint for later reviews.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );
    let result = apply_chat_memory_update(&state, "guru-a", "chat-a", "message-a", true, &capture)
        .await
        .unwrap()
        .expect("research-learn apply");
    assert_eq!(result.status, MemoryUpdateStatus::Applied);

    let runtime = state.runtime().unwrap();
    let bound = bound_root(&workspace);
    let listed = bound.knowledge_list(&runtime, None).await.unwrap();
    let index = agent_harness::learned_memory_index_from_records(
        listed.as_array().unwrap(),
        &["wiki:tsmc-foundry-economics".into()],
        None,
    );
    assert_eq!(index[0].id, "wiki:tsmc-foundry-economics");

    for query in [
        "Taiwan Semiconductor packaging bottleneck",
        "How should I frame TSMC capacity risk in a later review?",
        "2330.TW advanced packaging",
    ] {
        let search = bound
            .knowledge_search(&runtime, query, Some("wiki"), 8, false, None)
            .await
            .unwrap();
        assert!(
            search
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == "wiki:tsmc-foundry-economics"),
            "later prompt missed the learned Wiki: {query} -> {search}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn applied_wiki_is_exact_readable_and_denied_when_memory_is_off() {
    use crate::{
        broker::{start_tool_broker, tool_broker_endpoint, ToolExecutor, ToolPolicy},
        commands::{
            chat_runtime::collect_learned_memory_index,
            memory_updates::apply_chat_memory_update,
            tool_executor::{AppToolExecutor, ToolCapture},
        },
        domain::{MemoryProposal, MemoryUpdateStatus},
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "later-read");
    let runtime_path = temporary.path().join("guruterminal-core-later-read");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let guru = profile("guru-a", &workspace, 1);
    seed_profile(state.store.as_ref(), &guru);
    let mut session = chat("chat-a", "guru-a", 1);
    session.messages.push(assistant_message(
        "message-a",
        "Compile the durable foundry constraint and reuse it in later capacity reviews.",
    ));
    state.store.create_chat(&session).unwrap();

    let wiki_body = "CoWoS remains tighter than leading-edge wafer starts.";
    let capture = ToolCapture::default();
    capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-foundry".into(),
            "Wiki".into(),
            "wiki:tsmc-foundry-economics".into(),
            crate::domain::MemoryProposalBase::Absent,
            format!(
                "---\nid: wiki:tsmc-foundry-economics\ntitle: TSMC foundry economics\nsummary: Advanced packaging, not wafer starts, is the binding capacity constraint for leading-edge TSMC nodes.\nas_of: 2026-03-15T00:00:00Z\naliases:\n  - Taiwan Semiconductor\n  - 2330.TW\nentities:\n  - TSMC\n---\n\n# Constraint\n\n{wiki_body}\n"
            ),
            "Keep a reusable foundry constraint for later reviews.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );
    capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-lens".into(),
            "Lens".into(),
            "lens:packaging-before-wafer-starts".into(),
            crate::domain::MemoryProposalBase::Absent,
            lens_markdown(
                "lens:packaging-before-wafer-starts",
                "Inspect packaging before wafer starts",
            ),
            "Keep a reusable packaging-before-wafer-starts method.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );
    let result = apply_chat_memory_update(&state, "guru-a", "chat-a", "message-a", true, &capture)
        .await
        .unwrap()
        .expect("research-learn apply");
    assert_eq!(result.status, MemoryUpdateStatus::Applied);

    let bound = bound_root(&workspace);
    let index = collect_learned_memory_index(&state, &bound, None).await;
    assert!(
        index
            .iter()
            .any(|entry| entry.id == "wiki:tsmc-foundry-economics"),
        "learned index missed the applied Wiki: {index:?}"
    );
    assert!(
        index
            .iter()
            .any(|entry| entry.id == "lens:packaging-before-wafer-starts"),
        "learned index missed the applied Lens: {index:?}"
    );

    let runtime = state.runtime().unwrap();
    let exact = bound
        .knowledge_read(&runtime, "wiki:tsmc-foundry-economics", None)
        .await
        .unwrap();
    let content = exact["content"].as_str().unwrap_or_default();
    assert!(
        content.contains(wiki_body),
        "exact knowledge read must return the applied body: {exact}"
    );

    let executor = AppToolExecutor {
        capability_ids: BTreeSet::new(),
        state: state.clone(),
        capture: Arc::new(ToolCapture::default()),
        guru_id: "guru-a".into(),
        guru_root: bound_root(&workspace),
        chat_provider: String::new(),
    };
    let memory_on = ToolPolicy {
        guru_id: "guru-a".into(),
        session_id: "chat-a".into(),
        use_memory: true,
        propose_memory_updates: false,
        memory_proposal_budget: 0,
        as_of: None,
    };
    let read = executor
        .execute(
            &memory_on,
            crate::broker::ToolMethod::GuruRead,
            json!({"id": "wiki:tsmc-foundry-economics"}),
        )
        .await
        .unwrap();
    let read_content = read
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        read_content.contains(wiki_body),
        "memory_read must return the applied body: {read}"
    );

    let denied_socket = tool_broker_endpoint(temporary.path().join("memory-off.sock"));
    let denied_broker = start_tool_broker(
        denied_socket.clone(),
        ToolPolicy {
            use_memory: false,
            ..memory_on
        },
        Arc::new(executor),
    )
    .await
    .unwrap();
    for (method, params) in [
        (
            "guru.search",
            json!({"query": "Taiwan Semiconductor packaging bottleneck"}),
        ),
        ("guru.read", json!({"id": "wiki:tsmc-foundry-economics"})),
    ] {
        let denied =
            memory_off_broker_request(&denied_socket, denied_broker.token(), method, params).await;
        assert_eq!(
            denied["error"]["code"], "memory_disabled",
            "{method} must be denied when Use memory is off: {denied}"
        );
    }
    denied_broker.shutdown().await.unwrap();
}

#[cfg(unix)]
async fn memory_off_broker_request(
    socket: &std::path::Path,
    token: &str,
    method: &str,
    params: Value,
) -> Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut stream = tokio::net::UnixStream::connect(socket).await.unwrap();
    let mut request = serde_json::to_vec(&json!({
        "protocol": "guruterminal-tool/1",
        "id": "request-1",
        "token": token,
        "method": method,
        "params": params,
    }))
    .unwrap();
    request.push(b'\n');
    stream.write_all(&request).await.unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    let ack = json!({
        "protocol": "guruterminal-tool/1",
        "id": "request-1",
        "delivered": true,
    });
    reader
        .get_mut()
        .write_all(format!("{ack}\n").as_bytes())
        .await
        .unwrap();
    reader.get_mut().shutdown().await.unwrap();
    line.clear();
    reader.read_line(&mut line).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({
            "protocol": "guruterminal-tool/1",
            "id": "request-1",
            "committed": true,
        })
    );
    line.clear();
    assert_eq!(reader.read_line(&mut line).await.unwrap(), 0);
    response
}

#[cfg(unix)]
#[tokio::test]
async fn turn_envelope_loads_existing_charter_page_when_memory_is_on() {
    use chrono::{TimeZone, Utc};

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "charter-envelope");
    let runtime_path = temporary.path().join("guruterminal-core-charter-envelope");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));
    fs::write(
        workspace.join("guruterminal/lens/charter.md"),
        "---\nid: lens:charter\ntitle: How this Guru invests\nsummary: Standing philosophy.\nas_of: 2026-01-01T00:00:00Z\n---\n\n# Scope\n\nPrefer cash-flow durability over narrative.\n",
    )
    .unwrap();

    let bound = bound_root(&workspace);
    let charter = super::super::chat_runtime::collect_charter(&state, &bound, None)
        .await
        .expect("reserved charter page");
    assert!(charter.contains("Prefer cash-flow durability over narrative."));

    let now = Utc.with_ymd_and_hms(2026, 8, 10, 1, 2, 3).unwrap();
    let with_memory =
        crate::agent_harness::turn_envelope_block(now, true, &[], Some(charter.as_str())).unwrap();
    let memory: serde_json::Value = serde_json::from_str(&with_memory).unwrap();
    assert_eq!(memory["memory_protocol"]["charter"], charter);
    let without_memory =
        crate::agent_harness::turn_envelope_block(now, false, &[], Some(charter.as_str())).unwrap();
    let omitted: serde_json::Value = serde_json::from_str(&without_memory).unwrap();
    assert!(omitted.get("memory_protocol").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn applied_wiki_does_not_leak_across_gurus_or_after_an_as_of_cutoff() {
    use crate::{
        commands::{memory_updates::apply_chat_memory_update, tool_executor::ToolCapture},
        domain::MemoryProposal,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace_a = temporary.path().join("guru-a");
    let workspace_b = temporary.path().join("guru-b");
    initialized_workspace(&workspace_a, "iso-a");
    initialized_workspace(&workspace_b, "iso-b");
    let runtime_path = temporary.path().join("guruterminal-core-isolation");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace_a, 1));
    seed_profile(state.store.as_ref(), &profile("guru-b", &workspace_b, 2));
    let mut session = chat("chat-a", "guru-a", 1);
    session.messages.push(assistant_message(
        "message-a",
        "Keep the foundry constraint inside this Guru only.",
    ));
    state.store.create_chat(&session).unwrap();

    let capture = ToolCapture::default();
    capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-iso".into(),
            "Wiki".into(),
            "wiki:tsmc-foundry-economics".into(),
            crate::domain::MemoryProposalBase::Absent,
            wiki_markdown("wiki:tsmc-foundry-economics", "TSMC foundry economics"),
            "Guru-scoped compiled fact.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );
    apply_chat_memory_update(&state, "guru-a", "chat-a", "message-a", true, &capture)
        .await
        .unwrap()
        .expect("apply");

    let runtime = state.runtime().unwrap();
    let other = bound_root(&workspace_b);
    let leaked = other
        .knowledge_search(
            &runtime,
            "TSMC foundry economics",
            Some("wiki"),
            8,
            false,
            None,
        )
        .await
        .unwrap();
    assert!(
        leaked.as_array().map(Vec::is_empty).unwrap_or(false),
        "Guru B must not see Guru A Memory: {leaked}"
    );
    let cutoff = bound_root(&workspace_a)
        .knowledge_search(
            &runtime,
            "TSMC foundry economics",
            Some("wiki"),
            8,
            false,
            Some("2026-01-01"),
        )
        .await
        .unwrap();
    assert!(
        cutoff.as_array().map(Vec::is_empty).unwrap_or(false),
        "as-of cutoff must hide the later compiled Wiki: {cutoff}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn concurrent_stale_revisions_of_the_same_memory_target_cannot_overwrite_each_other() {
    use crate::{
        commands::{memory_updates::apply_chat_memory_update, tool_executor::ToolCapture},
        domain::{MemoryProposal, MemoryProposalBase},
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "proposal-target-cas");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-proposal-target-cas");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));

    let record_id = "wiki:shared-cas-target";
    let initial = wiki_markdown(record_id, "Shared CAS target");
    let initial_capture = ToolCapture::default();
    initial_capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-initial".into(),
            "Wiki".into(),
            record_id.into(),
            MemoryProposalBase::Absent,
            initial.clone(),
            "Create the initial shared record.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );
    apply_chat_memory_update(
        &state,
        "guru-a",
        "chat-initial",
        "message-initial",
        true,
        &initial_capture,
    )
    .await
    .unwrap();

    let base = MemoryProposalBase::FullRead {
        digest: crate::hashing::sha256(initial.as_bytes()),
    };
    let first_markdown = initial.replace(
        "Durable fact from current research.",
        "First concurrent revision.",
    );
    let second_markdown = initial.replace(
        "Durable fact from current research.",
        "Second concurrent revision.",
    );
    let first_capture = ToolCapture::default();
    first_capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-first".into(),
            "Wiki".into(),
            record_id.into(),
            base.clone(),
            first_markdown.clone(),
            "Apply the first concurrent revision.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );
    let second_capture = ToolCapture::default();
    second_capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-second".into(),
            "Wiki".into(),
            record_id.into(),
            base,
            second_markdown.clone(),
            "Apply the second concurrent revision.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );

    let (first, second) = tokio::join!(
        Box::pin(apply_chat_memory_update(
            &state,
            "guru-a",
            "chat-first",
            "message-first",
            true,
            &first_capture,
        )),
        Box::pin(apply_chat_memory_update(
            &state,
            "guru-a",
            "chat-second",
            "message-second",
            true,
            &second_capture,
        )),
    );
    let outcomes = [first, second];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    let conflict = outcomes
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("one stale writer must lose");
    assert_eq!(conflict.code, "conflict");
    assert!(conflict
        .message
        .contains("changed after its full-record read"));

    let stored = bound_root(&workspace)
        .knowledge_read(&state.runtime().unwrap(), record_id, None)
        .await
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(stored == first_markdown || stored == second_markdown);
}

#[cfg(unix)]
#[tokio::test]
async fn concurrent_distinct_ids_with_the_same_title_cannot_create_duplicates() {
    use crate::{
        commands::{memory_updates::apply_chat_memory_update, tool_executor::ToolCapture},
        domain::{MemoryProposal, MemoryProposalBase},
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    initialized_workspace(&workspace, "proposal-identity-cas");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-proposal-identity-cas");
    super::write_knowledge_runtime(&runtime_path);
    let mut state = AppState::for_test(temporary.path().join("app"));
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));

    let proposal = |proposal_id: &str, record_id: &str| {
        MemoryProposal::new(
            proposal_id.into(),
            "Wiki".into(),
            record_id.into(),
            MemoryProposalBase::Absent,
            wiki_markdown(record_id, "One semantic identity"),
            "Create one canonical record for this identity.".into(),
            Vec::new(),
            None,
        )
        .unwrap()
    };
    let first_capture = ToolCapture::default();
    first_capture
        .proposal
        .lock()
        .await
        .push(proposal("proposal-first-id", "wiki:first-identity"));
    let second_capture = ToolCapture::default();
    second_capture
        .proposal
        .lock()
        .await
        .push(proposal("proposal-second-id", "wiki:second-identity"));

    let (first, second) = tokio::join!(
        Box::pin(apply_chat_memory_update(
            &state,
            "guru-a",
            "chat-first-id",
            "message-first-id",
            true,
            &first_capture,
        )),
        Box::pin(apply_chat_memory_update(
            &state,
            "guru-a",
            "chat-second-id",
            "message-second-id",
            true,
            &second_capture,
        )),
    );
    let outcomes = [first, second];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    let conflict = outcomes
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("one duplicate identity writer must lose");
    assert_eq!(conflict.code, "conflict");
    assert!(conflict.message.contains("title or alias"));

    let listed = bound_root(&workspace)
        .knowledge_list(&state.runtime().unwrap(), Some("wiki"))
        .await
        .unwrap();
    let matching = listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["title"] == "One semantic identity")
        .count();
    assert_eq!(matching, 1);
}

#[cfg(unix)]
async fn run_accumulated_chat_memory_restart_case() {
    use crate::{
        agent_harness,
        commands::{memory_updates::apply_chat_memory_update, tool_executor::ToolCapture},
        domain::{MemoryAccess, MemoryProposal, MemoryRefSnapshot, MemoryUpdateStatus},
    };

    const INITIAL_RECORDS: usize = 25;
    const EARLY_ID: &str = "wiki:accumulated-lesson-00";

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("guru");
    let app_data = temporary.path().join("app");
    initialized_workspace(&workspace, "accumulated-memory");
    let runtime_path = temporary
        .path()
        .join("guruterminal-core-accumulated-memory");
    super::write_knowledge_runtime(&runtime_path);

    let mut state = AppState::for_persistent_test(app_data.clone());
    state.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path.clone()).unwrap(),
    ));
    seed_profile(state.store.as_ref(), &profile("guru-a", &workspace, 1));

    state
        .store
        .create_chat(&chat("chat-a", "guru-a", 1))
        .unwrap();

    for index in 0..INITIAL_RECORDS {
        append_assistant_message(
            &state,
            "chat-a",
            &format!("message-{index:02}"),
            &format!("Complete accumulated investment research task {index:02}."),
        );
        let id = format!("wiki:accumulated-lesson-{index:02}");
        let title = if index == 0 {
            "Countercyclical substrate lead-time sentinel".to_owned()
        } else {
            format!("Accumulated operating lesson {index:02}")
        };
        let capture = ToolCapture::default();
        capture.proposal.lock().await.push(
            MemoryProposal::new(
                format!("proposal-{index:02}"),
                "Wiki".into(),
                id.clone(),
                crate::domain::MemoryProposalBase::Absent,
                wiki_markdown(&id, &title),
                format!("Keep reusable finding {index:02} from this completed task."),
                Vec::new(),
                None,
            )
            .unwrap(),
        );
        let result = Box::pin(apply_chat_memory_update(
            &state,
            "guru-a",
            "chat-a",
            &format!("message-{index:02}"),
            true,
            &capture,
        ))
        .await
        .unwrap()
        .expect("accumulated create");
        assert_eq!(result.status, MemoryUpdateStatus::Applied);
    }

    let runtime = state.runtime().unwrap();
    let bound = bound_root(&workspace);
    let listed_before_update = bound.knowledge_list(&runtime, None).await.unwrap();
    let recent_before_update = crate::memory_git::recent_wiki_lens_ids(&workspace, 24);
    let index_before_update = agent_harness::learned_memory_index_from_records(
        listed_before_update.as_array().unwrap(),
        &recent_before_update,
        None,
    );
    assert_eq!(index_before_update.len(), 24);
    assert!(
        index_before_update.iter().all(|entry| entry.id != EARLY_ID),
        "the oldest Wiki should leave the bounded discovery index after enough later work"
    );

    let updated_markdown = "---\nid: wiki:accumulated-lesson-00\ntitle: Countercyclical substrate lead-time sentinel\nsummary: Treat confirmed order cancellations as the decisive reset signal after substrate lead times expand.\nas_of: 2026-08-20T00:00:00Z\n---\n\n# Updated lesson\n\nAfter repeated research, confirmed order cancellations now matter more than a single distributor inventory print.\n";
    let update_capture = ToolCapture::default();
    update_capture.memories.lock().await.insert(
        EARLY_ID.into(),
        MemoryRefSnapshot {
            record_id: EARLY_ID.into(),
            kind: "Wiki".into(),
            title: "Countercyclical substrate lead-time sentinel".into(),
            excerpt: "Durable fact from current research.".into(),
            as_of: Some("2026-08-19T00:00:00Z".into()),
            section: None,
            access: MemoryAccess::ExactRead,
            full_record_digest: Some(crate::hashing::sha256(
                wiki_markdown(EARLY_ID, "Countercyclical substrate lead-time sentinel").as_bytes(),
            )),
        },
    );
    update_capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-update-early".into(),
            "Wiki".into(),
            EARLY_ID.into(),
            crate::domain::MemoryProposalBase::FullRead {
                digest: crate::hashing::sha256(
                    wiki_markdown(EARLY_ID, "Countercyclical substrate lead-time sentinel")
                        .as_bytes(),
                ),
            },
            updated_markdown.into(),
            "Revise the early lesson after repeated counterexamples.".into(),
            vec![EARLY_ID.into()],
            None,
        )
        .unwrap(),
    );
    append_assistant_message(
        &state,
        "chat-a",
        &format!("message-{INITIAL_RECORDS:02}"),
        "Revise an early investment lesson after repeated counterexamples.",
    );
    let update_result = Box::pin(apply_chat_memory_update(
        &state,
        "guru-a",
        "chat-a",
        &format!("message-{INITIAL_RECORDS:02}"),
        true,
        &update_capture,
    ))
    .await
    .unwrap()
    .expect("accumulated update");
    assert_eq!(update_result.status, MemoryUpdateStatus::Applied);

    let listed_after_update = bound.knowledge_list(&runtime, None).await.unwrap();
    let recent_after_update = crate::memory_git::recent_wiki_lens_ids(&workspace, 24);
    let index_after_update = agent_harness::learned_memory_index_from_records(
        listed_after_update.as_array().unwrap(),
        &recent_after_update,
        None,
    );
    assert_eq!(index_after_update.first().unwrap().id, EARLY_ID);

    state.close_for_restart_test();

    let mut restarted = AppState::for_persistent_test(app_data);
    restarted.runtime = Some(Arc::new(
        crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
    ));
    let runtime = restarted.runtime().unwrap();
    let bound = bound_root(&workspace);
    let listed = bound.knowledge_list(&runtime, None).await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), INITIAL_RECORDS);
    let early = bound
        .knowledge_read(&runtime, EARLY_ID, None)
        .await
        .unwrap();
    assert!(early["content"]
        .as_str()
        .unwrap()
        .contains("confirmed order cancellations"));
    let recalled = bound
        .knowledge_search(
            &runtime,
            "substrate lead-time cancellation signal",
            Some("wiki"),
            8,
            false,
            None,
        )
        .await
        .unwrap();
    assert!(recalled
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == EARLY_ID));

    let continued_id = "wiki:accumulated-after-restart";
    let continued_capture = ToolCapture::default();
    continued_capture.proposal.lock().await.push(
        MemoryProposal::new(
            "proposal-after-restart".into(),
            "Wiki".into(),
            continued_id.into(),
            crate::domain::MemoryProposalBase::Absent,
            wiki_markdown(continued_id, "Post-restart accumulated lesson"),
            "Keep learning after reopening the persisted Guru.".into(),
            Vec::new(),
            None,
        )
        .unwrap(),
    );
    append_assistant_message(
        &restarted,
        "chat-a",
        &format!("message-{:02}", INITIAL_RECORDS + 1),
        "Continue investment research after reopening this Guru.",
    );
    let continued = Box::pin(apply_chat_memory_update(
        &restarted,
        "guru-a",
        "chat-a",
        &format!("message-{:02}", INITIAL_RECORDS + 1),
        true,
        &continued_capture,
    ))
    .await
    .unwrap()
    .expect("post-restart apply");
    assert_eq!(continued.status, MemoryUpdateStatus::Applied);
    assert!(bound
        .knowledge_read(&runtime, continued_id, None)
        .await
        .unwrap()["content"]
        .as_str()
        .is_some());
    assert!(bound
        .knowledge_read(&runtime, EARLY_ID, None)
        .await
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .contains("confirmed order cancellations"));
    let previous = crate::memory_git::read_previous_markdown(
        &workspace,
        "guruterminal/wiki/accumulated-lesson-00.md",
    )
    .unwrap()
    .expect("prior wiki version");
    assert!(!previous.markdown.contains("confirmed order cancellations"));
}

#[cfg(unix)]
#[test]
fn accumulated_chat_memory_survives_revision_growth_and_restart() {
    // This test intentionally drives many large memory-write futures in one async
    // state machine. Give that test-only thread room without changing product
    // runtime stack sizing or weakening the above-index-cap scenario.
    std::thread::Builder::new()
        .name("accumulated-memory-test".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(Box::pin(run_accumulated_chat_memory_restart_case()));
        })
        .unwrap()
        .join()
        .unwrap();
}
