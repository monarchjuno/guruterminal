use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::{
    app::{AppState, CommandError, QuarantineSource},
    domain::{MemoryChangeAuthority, MemoryChangeTarget, MemoryWrite},
    guru_root::profile_workspace,
    hashing::sha256,
    memory_finalization::{
        MemoryFinalizationJournal, MemoryFinalizationScope, MEMORY_FINALIZATION_SCHEMA_VERSION,
    },
    run_coordinator::{RunRegistration, RunTarget},
    runtime::StagedMemoryChange,
    store::GuruTerminalStore,
};

use super::{
    iso_time, json_text_from_markdown, map_internal, map_runtime, map_store, new_id, now_ms,
};

#[cfg(test)]
thread_local! {
    static AFTER_MEMORY_GIT_FINALIZE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn after_memory_git_finalize_for_test(hook: impl FnOnce() + 'static) {
    AFTER_MEMORY_GIT_FINALIZE.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_after_memory_git_finalize_for_test() {
    AFTER_MEMORY_GIT_FINALIZE.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn run_after_memory_git_finalize_for_test() {}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryWriteResult {
    pub commit_id: String,
    pub targets: Vec<MemoryWriteTargetDto>,
    pub written_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryWriteTargetDto {
    pub record_id: String,
    pub title: String,
}

pub(in crate::commands) struct RegisteredMemoryTransaction {
    write: MemoryWrite,
    write_id: String,
    _memory_writer: RunRegistration,
    scope: MemoryFinalizationScope,
}

impl RegisteredMemoryTransaction {
    pub(in crate::commands) fn standalone(
        write: MemoryWrite,
        write_id: String,
        memory_writer: RunRegistration,
    ) -> Self {
        Self {
            write,
            write_id,
            _memory_writer: memory_writer,
            scope: MemoryFinalizationScope::StandaloneUser,
        }
    }

    pub(in crate::commands) fn chat(
        write: MemoryWrite,
        write_id: String,
        memory_writer: RunRegistration,
        thread_id: &str,
        message_id: &str,
    ) -> Self {
        Self {
            write,
            write_id,
            _memory_writer: memory_writer,
            scope: MemoryFinalizationScope::Chat {
                thread_id: thread_id.to_owned(),
                message_id: message_id.to_owned(),
            },
        }
    }
}

async fn compensate_memory_transaction(
    state: &AppState,
    workspace: &crate::guru_root::BoundGuruRoot,
    runtime: &crate::runtime::GuruTerminalRuntime,
    changes: &[StagedMemoryChange],
    snapshot: &crate::memory_git::MemoryGitSnapshot,
    expected_commit_id: Option<&str>,
    journal: Option<&MemoryFinalizationJournal>,
) -> Result<(), String> {
    // Preflight every file before changing Git or any target. A third-party
    // edit is neither side of this journal and must never be overwritten.
    Box::pin(preflight_memory_rollback(workspace, changes))
        .await
        .map_err(|error| format!("files={error}"))?;
    crate::memory_git::rollback_memory_snapshot(workspace.path(), snapshot, expected_commit_id)
        .map_err(|error| format!("git={error}"))?;
    Box::pin(rollback_memory_changes_idempotent(
        workspace, runtime, changes,
    ))
    .await
    .map_err(|error| format!("files={error}"))?;
    let mut failures = Vec::new();
    // The journal is the recovery recipe. Keep it until both durable stores
    // have been restored so a quarantined Guru can retry idempotently.
    if failures.is_empty() {
        if let Some(journal) = journal {
            if let Err(error) = state.store.delete_memory_finalization_journal(journal) {
                failures.push(format!("journal={error}"));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

async fn preflight_memory_rollback(
    workspace: &crate::guru_root::BoundGuruRoot,
    changes: &[StagedMemoryChange],
) -> Result<(), String> {
    for change in changes {
        let current = workspace
            .read_memory_record(&change.relative_path)
            .map_err(|error| error.to_string())?;
        let current_digest = current.as_deref().map(sha256);
        let before_digest = change.before_sha256.as_deref();
        let proposed_digest = (!change.delete).then_some(change.proposed_sha256.as_str());
        if current_digest.as_deref() != before_digest
            && current_digest.as_deref() != proposed_digest
        {
            return Err(format!(
                "Memory target {} is neither its journaled before nor proposed state",
                change.relative_path.display()
            ));
        }
    }
    Ok(())
}

async fn verify_published_memory(
    workspace: &crate::guru_root::BoundGuruRoot,
    runtime: &crate::runtime::GuruTerminalRuntime,
    changes: &[StagedMemoryChange],
) -> Result<(), String> {
    for change in changes {
        let current = workspace
            .read_memory_record(&change.relative_path)
            .map_err(|error| error.to_string())?;
        let current_digest = current.as_deref().map(sha256);
        let proposed_digest = (!change.delete).then_some(change.proposed_sha256.as_str());
        if current_digest.as_deref() != proposed_digest {
            return Err(format!(
                "Memory target {} changed after its journaled write",
                change.relative_path.display()
            ));
        }
    }
    workspace
        .validate(runtime)
        .await
        .map_err(|error| error.to_string())
}

pub(super) async fn rollback_memory_changes_idempotent(
    workspace: &crate::guru_root::BoundGuruRoot,
    runtime: &crate::runtime::GuruTerminalRuntime,
    changes: &[StagedMemoryChange],
) -> Result<(), String> {
    let mut applied = Vec::new();
    for change in changes {
        let current = workspace
            .read_memory_record(&change.relative_path)
            .map_err(|error| error.to_string())?;
        let current_digest = current.as_deref().map(sha256);
        let before_digest = change.before_sha256.as_deref();
        let proposed_digest = (!change.delete).then_some(change.proposed_sha256.as_str());
        let target_is_proposed = match current_digest.as_deref() {
            current if current == before_digest => false,
            current if current == proposed_digest => true,
            _ => {
                return Err(format!(
                    "Memory target {} is neither its journaled before nor proposed state",
                    change.relative_path.display()
                ));
            }
        };

        // A process can stop after staging the replacement or immediately
        // after an atomic exchange. Reconcile that journal-owned artifact
        // before invoking normal validation or rollback, both of which reject
        // pending transaction files by design.
        workspace
            .reconcile_memory_artifact(runtime, change, target_is_proposed)
            .map_err(|error| error.to_string())?;

        let reconciled = workspace
            .read_memory_record(&change.relative_path)
            .map_err(|error| error.to_string())?;
        let reconciled_digest = reconciled.as_deref().map(sha256);
        match reconciled_digest.as_deref() {
            current if current == before_digest => {}
            current if current == proposed_digest => applied.push(change.clone()),
            _ => {
                return Err(format!(
                    "Memory target {} changed while reconciling its journaled artifact",
                    change.relative_path.display()
                ));
            }
        }
    }
    if applied.is_empty() {
        workspace
            .validate(runtime)
            .await
            .map_err(|error| error.to_string())
    } else {
        workspace
            .rollback_memory_markdown_set(runtime, &applied)
            .await
            .map_err(|error| error.to_string())
    }
}

pub(in crate::commands) async fn apply_memory_targets(
    state: &AppState,
    guru_id: &str,
    authority: MemoryChangeAuthority,
    targets: Vec<MemoryChangeTarget>,
    rationale: &str,
) -> Result<MemoryWriteResult, CommandError> {
    let write = MemoryWrite {
        guru_id: guru_id.to_owned(),
        authority,
        targets,
        rationale: rationale.to_owned(),
    };
    write.validate().map_err(map_internal)?;
    state.ensure_guru_available(guru_id)?;
    let write_id = new_id("memory-write");
    let _memory_writer = state
        .register_memory_write(
            write_id.clone(),
            guru_id.to_owned(),
            RunTarget::MemoryWriteSession(write_id.clone()),
        )
        .await?;
    apply_memory_targets_registered(
        state,
        RegisteredMemoryTransaction::standalone(write, write_id, _memory_writer),
        |_| Ok(()),
    )
    .await
    .map(|(written, ())| written)
}

pub(in crate::commands) async fn apply_memory_targets_registered<T>(
    state: &AppState,
    transaction: RegisteredMemoryTransaction,
    finalize: impl FnOnce(&MemoryWriteResult) -> Result<T, CommandError>,
) -> Result<(MemoryWriteResult, T), CommandError> {
    let RegisteredMemoryTransaction {
        write,
        write_id,
        _memory_writer,
        scope,
    } = transaction;
    let guru_id = write.guru_id.as_str();
    let authority = write.authority;
    let rationale = write.rationale.as_str();
    // This registration may have waited behind another same-Guru writer.
    // Recheck after owning the writer lease so a predecessor's quarantine is
    // observed before this transaction reads the workspace or touches Git.
    state.ensure_guru_available(guru_id)?;
    write.validate().map_err(map_internal)?;
    let profile = state
        .store
        .get_guru(guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let workspace = profile_workspace(&profile)?;
    let runtime = state.runtime()?;
    workspace.validate(&runtime).await.map_err(map_internal)?;
    let runtime_changes = staged_memory_changes(guru_id, &write_id, authority, &write.targets);
    let written_at = iso_time(now_ms())?;
    let mut git_snapshot =
        crate::memory_git::begin_memory_transaction(workspace.path()).map_err(map_internal)?;
    let mut journal = MemoryFinalizationJournal {
        schema_version: MEMORY_FINALIZATION_SCHEMA_VERSION,
        id: write_id.clone(),
        guru_id: guru_id.to_owned(),
        scope,
        updated_at_ms: now_ms(),
        git: git_snapshot.clone(),
        changes: runtime_changes.clone(),
        commit_id: None,
    };
    if let Err(error) = state.store.create_memory_finalization_journal(&journal) {
        // SQLite can report an error after the INSERT became durable. Do not
        // admit a second writer until recovery has determined whether this
        // untouched transaction journal exists and removed it idempotently.
        state.quarantine_guru(
            guru_id,
            QuarantineSource::MemoryWrite,
            format!("Memory transaction journal creation was indeterminate: {error}"),
        );
        return Err(CommandError::internal(
            "Memory write recovery is required before this Guru can be used",
        ));
    }
    if let Err(apply_error) = workspace
        .apply_memory_markdown_set(&runtime, &runtime_changes)
        .await
    {
        // An I/O error can be reported after an atomic rename/exchange became
        // visible. Treat every apply failure as an indeterminate transaction;
        // the durable journal must survive until Git and files are both proven
        // restored.
        if let Err(recovery_error) = Box::pin(compensate_memory_transaction(
            state,
            &workspace,
            &runtime,
            &runtime_changes,
            &git_snapshot,
            None,
            Some(&journal),
        ))
        .await
        {
            state.quarantine_guru(
                guru_id,
                QuarantineSource::MemoryWrite,
                format!(
                    "Memory apply failed and compensation was incomplete: apply={apply_error}; recovery={recovery_error}"
                ),
            );
            return Err(CommandError::internal(
                "Memory write recovery is required before this Guru can be used",
            ));
        }
        return Err(map_runtime(apply_error));
    }
    let git_changes = runtime_changes
        .iter()
        .map(|change| crate::memory_git::MemoryGitChange {
            relative_path: change.relative_path.clone(),
            contents: (!change.delete).then(|| change.proposed_markdown.as_bytes().to_vec()),
        })
        .collect::<Vec<_>>();
    let prepared = match crate::memory_git::prepare_memory_commit_exact(
        workspace.path(),
        &commit_message(authority, rationale, &write.targets),
        &git_snapshot,
        &git_changes,
    ) {
        Ok(prepared) => prepared,
        Err(commit_error) => {
            let recovery = Box::pin(compensate_memory_transaction(
                state,
                &workspace,
                &runtime,
                &runtime_changes,
                &git_snapshot,
                None,
                Some(&journal),
            ))
            .await;
            if commit_error.recovery_required() || recovery.is_err() {
                state.quarantine_guru(
                    guru_id,
                    QuarantineSource::MemoryWrite,
                    format!(
                        "Memory git transaction could not be recovered: commit={commit_error}; recovery={}",
                        recovery.err().unwrap_or_else(|| "restored".into())
                    ),
                );
                return Err(CommandError::internal(
                    "Memory write recovery is required before this Guru can be used",
                ));
            }
            return Err(map_internal(commit_error));
        }
    };
    git_snapshot.published_index_tree = Some(prepared.index_tree_id.clone());
    let expected = journal.clone();
    journal.commit_id = Some(prepared.commit_id.clone());
    journal.git = git_snapshot.clone();
    journal.updated_at_ms = now_ms().max(expected.updated_at_ms.saturating_add(1));
    if let Err(store_error) = state
        .store
        .replace_memory_finalization_journal(&expected, &journal)
    {
        let recovery = Box::pin(compensate_memory_transaction(
            state,
            &workspace,
            &runtime,
            &runtime_changes,
            &git_snapshot,
            None,
            Some(&expected),
        ))
        .await;
        if let Err(recovery_error) = recovery {
            state.quarantine_guru(
                guru_id,
                QuarantineSource::MemoryWrite,
                format!(
                    "Memory commit intent failed and compensation was incomplete: intent={store_error}; recovery={recovery_error}"
                ),
            );
            return Err(CommandError::internal(
                "Memory write recovery is required before this Guru can be used",
            ));
        }
        return Err(map_store(store_error));
    }
    let commit = match crate::memory_git::finalize_memory_commit(workspace.path(), prepared) {
        Ok(commit) => commit,
        Err(commit_error) => {
            let expected_commit = journal.commit_id.as_deref();
            let recovery = Box::pin(compensate_memory_transaction(
                state,
                &workspace,
                &runtime,
                &runtime_changes,
                &git_snapshot,
                expected_commit,
                Some(&journal),
            ))
            .await;
            if commit_error.recovery_required() || recovery.is_err() {
                state.quarantine_guru(
                    guru_id,
                    QuarantineSource::MemoryWrite,
                    format!(
                        "Memory ref finalization could not be recovered: commit={commit_error}; recovery={}",
                        recovery.err().unwrap_or_else(|| "restored".into())
                    ),
                );
                return Err(CommandError::internal(
                    "Memory write recovery is required before this Guru can be used",
                ));
            }
            return Err(map_internal(commit_error));
        }
    };
    run_after_memory_git_finalize_for_test();
    if let Err(error) = Box::pin(verify_published_memory(
        &workspace,
        &runtime,
        &runtime_changes,
    ))
    .await
    {
        // Git contains the exact journaled bytes, but the user-owned worktree
        // changed before SQLite could become canonical. Preserve every state
        // and the recovery journal; never overwrite the competing edit.
        state.quarantine_guru(
            guru_id,
            QuarantineSource::MemoryWrite,
            format!("Memory changed during Chat finalization: {error}"),
        );
        return Err(CommandError::internal(
            "Memory write recovery is required before this Guru can be used",
        ));
    }
    let written = MemoryWriteResult {
        commit_id: commit.commit_id.clone(),
        targets: write
            .targets
            .iter()
            .map(|target| MemoryWriteTargetDto {
                record_id: target.record_id.clone(),
                title: json_text_from_markdown(&target.proposed_markdown, "title")
                    .unwrap_or_else(|| target.record_id.clone()),
            })
            .collect(),
        written_at,
    };
    match finalize(&written) {
        Ok(finalized) => {
            if let Err(error) = state.store.delete_memory_finalization_journal(&journal) {
                // Chat/standalone completion and Memory are both canonical.
                // Keep the durable intent for idempotent recovery and block
                // later writes until startup/recovery removes it.
                state.quarantine_guru(
                    guru_id,
                    QuarantineSource::MemoryWrite,
                    format!("finalized Memory journal cleanup failed: {error}"),
                );
            }
            Ok((written, finalized))
        }
        Err(finalize_error) => {
            if matches!(&journal.scope, MemoryFinalizationScope::Chat { .. }) {
                // SQLite can report an I/O error after COMMIT became durable.
                // Resolve that indeterminate outcome from the durable journal
                // and the exact canonical Chat instead of assuming rollback.
                state.quarantine_guru(
                    guru_id,
                    QuarantineSource::MemoryWrite,
                    format!(
                        "Chat finalization returned an indeterminate outcome: {finalize_error}"
                    ),
                );
                match Box::pin(recover_memory_finalizations_for_guru(state, guru_id)).await {
                    Ok(()) => {
                        state.clear_guru_quarantine(guru_id, QuarantineSource::MemoryWrite);
                        Err(finalize_error)
                    }
                    Err(recovery_error) => {
                        // A read failure is indeterminate too. Recovery leaves
                        // the journal in place so startup or an explicit retry
                        // can determine whether Chat or rollback is canonical.
                        state.quarantine_guru(
                            guru_id,
                            QuarantineSource::MemoryWrite,
                            format!(
                                "Chat finalization outcome could not be resolved: sqlite={finalize_error}; recovery={}",
                                recovery_error.message
                            ),
                        );
                        Err(CommandError::internal(
                            "Memory write recovery is required before this Guru can be used",
                        ))
                    }
                }
            } else {
                let recovery = Box::pin(compensate_memory_transaction(
                    state,
                    &workspace,
                    &runtime,
                    &runtime_changes,
                    &git_snapshot,
                    Some(&commit.commit_id),
                    Some(&journal),
                ))
                .await;
                if let Err(recovery_error) = recovery {
                    state.quarantine_guru(
                        guru_id,
                        QuarantineSource::MemoryWrite,
                        format!(
                            "Memory finalization failed and compensation was incomplete: finalization={finalize_error}; recovery={recovery_error}"
                        ),
                    );
                    return Err(CommandError::internal(
                        "Memory write recovery is required before this Guru can be used",
                    ));
                }
                Err(finalize_error)
            }
        }
    }
}

fn staged_memory_changes(
    guru_id: &str,
    session_id: &str,
    authority: MemoryChangeAuthority,
    targets: &[MemoryChangeTarget],
) -> Vec<StagedMemoryChange> {
    targets
        .iter()
        .map(|target| {
            let existed = !target.before_markdown.is_empty();
            StagedMemoryChange {
                guru_id: guru_id.to_owned(),
                session_id: session_id.to_owned(),
                relative_path: PathBuf::from(&target.relative_path),
                before_sha256: existed.then(|| sha256(target.before_markdown.as_bytes())),
                before_markdown: existed.then(|| target.before_markdown.clone()),
                proposed_sha256: sha256(target.proposed_markdown.as_bytes()),
                proposed_markdown: target.proposed_markdown.clone(),
                delete: authority == MemoryChangeAuthority::User
                    && target.proposed_markdown.is_empty(),
            }
        })
        .collect()
}

fn commit_message(
    authority: MemoryChangeAuthority,
    rationale: &str,
    targets: &[MemoryChangeTarget],
) -> String {
    let source = match authority {
        MemoryChangeAuthority::Chat => "chat",
        MemoryChangeAuthority::User => "user",
    };
    let records = targets
        .iter()
        .map(|target| target.record_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let summary = rationale.lines().next().unwrap_or(rationale).trim();
    let summary = if summary.is_empty() {
        "update memory"
    } else {
        summary
    };
    let mut message = format!("{source}: {summary}");
    if !records.is_empty() {
        message.push_str(" (");
        message.push_str(&records);
        message.push(')');
    }
    if message.len() > 240 {
        message.truncate(240);
    }
    message
}

pub(crate) fn interrupted_memory_finalization_quarantines(
    store: &dyn GuruTerminalStore,
) -> Result<HashMap<String, String>, CommandError> {
    let mut quarantines = HashMap::new();
    for record in store
        .list_memory_finalization_journals()
        .map_err(map_store)?
    {
        match record {
            crate::store::MemoryFinalizationJournalRecord::Valid(journal) => {
                quarantines.insert(
                    journal.guru_id,
                    format!(
                        "Interrupted Memory finalization {} requires recovery",
                        journal.id
                    ),
                );
            }
            crate::store::MemoryFinalizationJournalRecord::Invalid {
                id,
                guru_id,
                reason,
            } => {
                quarantines.insert(
                    guru_id,
                    format!("Invalid Memory finalization journal {id}: {reason}"),
                );
            }
        }
    }
    Ok(quarantines)
}

async fn recover_memory_finalizations_for_guru(
    state: &AppState,
    guru_id: &str,
) -> Result<(), CommandError> {
    let records = state
        .store
        .list_memory_finalization_journals()
        .map_err(map_store)?;
    let profile = state
        .store
        .get_guru(guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let workspace = profile_workspace(&profile)?;
    let runtime = state.runtime()?;
    for record in records {
        let journal = match record {
            crate::store::MemoryFinalizationJournalRecord::Valid(journal)
                if journal.guru_id == guru_id =>
            {
                journal
            }
            crate::store::MemoryFinalizationJournalRecord::Valid(_) => continue,
            crate::store::MemoryFinalizationJournalRecord::Invalid {
                guru_id: invalid_guru,
                ..
            } if invalid_guru == guru_id => {
                return Err(CommandError::internal(
                    "Memory finalization journal is invalid and requires manual recovery",
                ));
            }
            crate::store::MemoryFinalizationJournalRecord::Invalid { .. } => continue,
        };
        let finalized_commit = match &journal.scope {
            MemoryFinalizationScope::Chat {
                thread_id,
                message_id,
            } => state
                .store
                .get_chat(thread_id)
                .map_err(map_store)?
                .and_then(|chat| {
                    chat.messages
                        .into_iter()
                        .find(|message| message.id == *message_id)
                })
                .is_some_and(|message| {
                    message.status == crate::domain::ChatMessageStatus::Complete
                        && message.memory_update.and_then(|update| update.commit_id)
                            == journal.commit_id
                })
                .then(|| journal.commit_id.clone())
                .flatten(),
            MemoryFinalizationScope::StandaloneUser => journal
                .commit_id
                .as_deref()
                .filter(|commit_id| {
                    crate::memory_git::verify_finalized_memory_commit(workspace.path(), commit_id)
                        .is_ok()
                })
                .map(str::to_owned),
        };
        if let Some(commit_id) = finalized_commit {
            crate::memory_git::verify_finalized_memory_commit(workspace.path(), &commit_id)
                .map_err(map_internal)?;
            Box::pin(verify_published_memory(
                &workspace,
                &runtime,
                &journal.changes,
            ))
            .await
            .map_err(CommandError::internal)?;
            state
                .store
                .delete_memory_finalization_journal(&journal)
                .map_err(map_store)?;
            continue;
        }
        Box::pin(compensate_memory_transaction(
            state,
            &workspace,
            &runtime,
            &journal.changes,
            &journal.git,
            journal.commit_id.as_deref(),
            Some(&journal),
        ))
        .await
        .map_err(|error| {
            CommandError::internal(format!(
                "Interrupted Memory finalization could not be recovered: {error}"
            ))
        })?;
    }
    Ok(())
}

pub(super) async fn retry_quarantined_guru_recovery(
    state: &AppState,
    guru_id: &str,
) -> Result<(), CommandError> {
    if !state.is_guru_quarantined(guru_id) {
        return Ok(());
    }
    let profile = state
        .store
        .get_guru(guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let workspace = profile_workspace(&profile)?;
    recover_memory_finalizations_for_guru(state, guru_id).await?;
    if let Some(runtime) = state.runtime.clone() {
        workspace.validate(&runtime).await.map_err(map_internal)?;
    }
    state.clear_guru_quarantine(guru_id, QuarantineSource::MemoryWrite);
    Ok(())
}
