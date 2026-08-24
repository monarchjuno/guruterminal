use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

use crate::{
    app::{AppState, CommandError},
    domain::{
        ChatDecision, MemoryChangeAuthority, MemoryChangeTarget, MemoryIdentityRecord,
        MemoryProposal, MemoryProposalBase, MemoryUpdateChange, MemoryUpdateResult,
        MemoryUpdateStatus, MemoryWrite,
    },
    guru_root::{profile_workspace, BoundGuruRoot},
    run_coordinator::{PendingMemoryWrite, RunRegistration, RunTarget},
    store::GuruTerminalStore,
};

use super::{
    json_text_from_markdown, map_internal, map_store, memory_write, new_id, now_ms, tool_executor,
    tool_executor::ToolCapture, types::memory_kind_from_id,
};

fn topic_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            slug.push(character.to_ascii_lowercase());
            separator = false;
        } else if !separator && !slug.is_empty() {
            slug.push('-');
            separator = true;
        }
        if slug.len() >= 72 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "memory".into()
    } else {
        slug
    }
}

fn derived_memory_target_path(kind: &str, record_id: &str) -> Result<String, CommandError> {
    let suffix = record_id
        .strip_prefix(&format!("{kind}:"))
        .ok_or_else(|| CommandError::invalid("Memory target kind does not match its ID"))?;
    let slug = topic_slug(suffix);
    Ok(format!("guruterminal/{kind}/{slug}.md"))
}

fn yaml_scalar(value: &str) -> String {
    format!(
        "'{}'",
        value.replace('\'', "''").replace(['\n', '\r', '\0'], " ")
    )
}

fn markdown_inline(value: &str) -> String {
    value
        .replace(['\n', '\r', '\0'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn markdown_code(value: &str) -> String {
    format!("`{}`", markdown_inline(value).replace('`', "'"))
}

fn as_of_or_now(value: Option<&str>, timestamp: i64) -> String {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
        .or_else(|| {
            value
                .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
                .and_then(|date| date.and_hms_opt(0, 0, 0))
                .map(|value| value.and_utc().to_rfc3339_opts(SecondsFormat::Secs, true))
        })
        .unwrap_or_else(|| {
            DateTime::<Utc>::from_timestamp_millis(timestamp)
                .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
                .to_rfc3339_opts(SecondsFormat::Secs, true)
        })
}

async fn normalized_target(
    state: &AppState,
    workspace: &BoundGuruRoot,
    kind: &str,
    record_id: &str,
    expected_base: Option<&MemoryProposalBase>,
    proposed_markdown: String,
) -> Result<Option<MemoryChangeTarget>, CommandError> {
    let runtime = state.runtime()?;
    let listed = workspace
        .knowledge_list(&runtime, Some(kind))
        .await
        .map_err(map_internal)?;
    let matches = listed
        .as_array()
        .ok_or_else(|| CommandError::internal("Memory list result is invalid"))?
        .iter()
        .filter(|record| record.get("id").and_then(Value::as_str) == Some(record_id))
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(CommandError::conflict("Memory target is ambiguous"));
    }
    let (relative_path, before_markdown) = if let Some(summary) = matches.first() {
        let path = summary
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| CommandError::internal("Memory target path is invalid"))?
            .to_owned();
        let read = workspace
            .knowledge_read(&runtime, record_id, None)
            .await
            .map_err(map_internal)?;
        let content = read
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| CommandError::internal("Memory target content is invalid"))?
            .to_owned();
        (path, content)
    } else {
        (derived_memory_target_path(kind, record_id)?, String::new())
    };
    if let Some(expected_base) = expected_base {
        match expected_base {
            MemoryProposalBase::Absent if matches.is_empty() => {}
            MemoryProposalBase::Absent => {
                return Err(CommandError::conflict(format!(
                    "Memory proposal target {record_id} was created after the proposal was formed"
                )));
            }
            MemoryProposalBase::FullRead { .. } if matches.is_empty() => {
                return Err(CommandError::conflict(format!(
                    "Memory proposal target {record_id} no longer exists"
                )));
            }
            MemoryProposalBase::FullRead { digest }
                if crate::hashing::sha256(before_markdown.as_bytes()) == *digest => {}
            MemoryProposalBase::FullRead { .. } => {
                return Err(CommandError::conflict(format!(
                    "Memory proposal target {record_id} changed after its full-record read"
                )));
            }
        }
    }
    if before_markdown == proposed_markdown {
        return Ok(None);
    }
    if matches!(kind, "wiki" | "lens")
        && crate::domain::markdown_frontmatter_id(&proposed_markdown).as_deref() != Some(record_id)
    {
        return Err(CommandError::invalid(
            "proposed Markdown id must equal the target record id",
        ));
    }
    if kind == "lens" && !crate::domain::lens_proposal_has_required_sections(&proposed_markdown) {
        return Err(CommandError::invalid(
            "Lens proposals require non-empty # Scope, # Assumptions, # Counterexamples, # Limits, and # Invalidation conditions",
        ));
    }
    Ok(Some(MemoryChangeTarget {
        record_id: record_id.to_owned(),
        relative_path,
        before_markdown,
        proposed_markdown,
    }))
}

fn listed_record_ids(listed: &Value) -> BTreeSet<String> {
    listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|record| record.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

fn current_memory_identity(record: &Value) -> Option<(String, MemoryIdentityRecord)> {
    let kind = record.get("kind")?.as_str()?.to_ascii_lowercase();
    if !matches!(kind.as_str(), "wiki" | "lens") {
        return None;
    }
    Some((
        kind,
        MemoryIdentityRecord {
            id: record.get("id")?.as_str()?.to_owned(),
            title: record.get("title")?.as_str()?.to_owned(),
            aliases: record
                .get("aliases")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            status: record
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
    ))
}

fn proposed_memory_identity(
    proposal: &MemoryProposal,
) -> Result<MemoryIdentityRecord, CommandError> {
    Ok(MemoryIdentityRecord {
        id: proposal.target_record_id.clone(),
        title: crate::domain::markdown_frontmatter_scalar(&proposal.proposed_markdown, "title")
            .filter(|title| !title.is_empty())
            .ok_or_else(|| CommandError::invalid("proposed Markdown title is required"))?,
        aliases: crate::domain::markdown_frontmatter_list(&proposal.proposed_markdown, "aliases"),
        status: crate::domain::markdown_frontmatter_scalar(&proposal.proposed_markdown, "status"),
    })
}

async fn validate_current_proposal_bases_and_identities(
    runtime: &crate::runtime::GuruTerminalRuntime,
    workspace: &BoundGuruRoot,
    listed: &Value,
    proposals: &[MemoryProposal],
) -> Result<(), CommandError> {
    let current = listed
        .as_array()
        .ok_or_else(|| CommandError::internal("Memory list result is invalid"))?;

    for proposal in proposals {
        proposal.validate().map_err(map_internal)?;
        let matches = current
            .iter()
            .filter(|record| {
                record.get("id").and_then(Value::as_str) == Some(proposal.target_record_id.as_str())
            })
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(CommandError::conflict(format!(
                "Memory proposal target {} is ambiguous",
                proposal.target_record_id
            )));
        }
        match &proposal.target_base {
            MemoryProposalBase::Absent if matches.is_empty() => {}
            MemoryProposalBase::Absent => {
                return Err(CommandError::conflict(format!(
                    "Memory proposal target {} was created after the proposal was formed",
                    proposal.target_record_id
                )));
            }
            MemoryProposalBase::FullRead { .. } if matches.is_empty() => {
                return Err(CommandError::conflict(format!(
                    "Memory proposal target {} no longer exists",
                    proposal.target_record_id
                )));
            }
            MemoryProposalBase::FullRead { digest } => {
                let read = workspace
                    .knowledge_read(runtime, &proposal.target_record_id, None)
                    .await
                    .map_err(map_internal)?;
                let current_markdown = read
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommandError::internal("Memory target content is invalid"))?;
                if crate::hashing::sha256(current_markdown.as_bytes()) != *digest {
                    return Err(CommandError::conflict(format!(
                        "Memory proposal target {} changed after its full-record read",
                        proposal.target_record_id
                    )));
                }
            }
        }
    }

    let current_identities = current
        .iter()
        .filter_map(current_memory_identity)
        .collect::<Vec<_>>();
    let proposed_identities = proposals
        .iter()
        .map(|proposal| {
            Ok((
                proposal.target_kind.to_ascii_lowercase(),
                proposed_memory_identity(proposal)?,
            ))
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    for (proposal, (kind, proposed)) in proposals.iter().zip(&proposed_identities) {
        let candidates = current_identities
            .iter()
            .filter(|(candidate_kind, _)| candidate_kind == kind)
            .map(|(_, record)| record.clone())
            .chain(
                proposed_identities
                    .iter()
                    .filter(|(candidate_kind, record)| {
                        candidate_kind == kind && record.id != proposed.id
                    })
                    .map(|(_, record)| record.clone()),
            )
            .collect::<Vec<_>>();
        if let Some(existing_id) = crate::domain::colliding_active_memory_id(
            &proposal.target_record_id,
            &proposed.title,
            &proposed.aliases,
            &candidates,
        ) {
            return Err(CommandError::conflict(format!(
                "an active {kind} already uses this title or alias; revise {existing_id} instead of creating a duplicate"
            )));
        }
    }
    Ok(())
}

fn allocate_slug_id(
    kind: &str,
    namespace: &str,
    title: &str,
    existing: &mut BTreeSet<String>,
) -> (String, Option<String>) {
    let slug = topic_slug(title);
    let base = format!("{kind}:{namespace}/{slug}");
    if existing.insert(base.clone()) {
        return (base, None);
    }
    let previous = base.clone();
    let mut index = 2_u32;
    loop {
        let candidate = format!("{kind}:{namespace}/{slug}-{index}");
        if existing.insert(candidate.clone()) {
            return (candidate, Some(previous));
        }
        index = index.saturating_add(1);
        if index > 9_999 {
            return (
                format!("{kind}:{namespace}/{slug}-overflow"),
                Some(previous),
            );
        }
    }
}

fn evidence_markdown(evidence: &tool_executor::StagedEvidence, timestamp: i64) -> String {
    let claims = evidence
        .claims
        .iter()
        .enumerate()
        .map(|(claim_index, claim)| {
            let citations = claim
                .citations
                .iter()
                .map(|citation| {
                    format!(
                        "{} {}",
                        markdown_code(&citation.result_ref),
                        markdown_code(&citation.pointer)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{}. {}\n   - Citations: {}",
                claim_index + 1,
                markdown_inline(&claim.text),
                citations,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let data = evidence
        .claims
        .iter()
        .enumerate()
        .flat_map(|(claim_index, claim)| {
            claim
                .citations
                .iter()
                .enumerate()
                .map(move |(citation_index, citation)| {
                    let encoded = serde_json::to_string_pretty(&citation.selected)
                        .unwrap_or_else(|_| "null".into());
                    let indented = encoded
                        .lines()
                        .map(|line| format!("    {line}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!(
                        "## Claim {} · Citation {}\n\n- Result: {}\n- JSON Pointer: {}\n- Selection: {}\n\n{}",
                        claim_index + 1,
                        citation_index + 1,
                        markdown_code(&citation.result_ref),
                        markdown_code(&citation.pointer),
                        if citation.excerpt.is_some() { "exact excerpt" } else { "exact value" },
                        indented,
                    )
                })
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut receipts = BTreeMap::new();
    for claim in &evidence.claims {
        for citation in &claim.citations {
            receipts
                .entry(citation.result_ref.as_str())
                .or_insert(&citation.receipt);
        }
    }
    let sources = receipts
        .values()
        .map(|receipt| {
            let provider = receipt.producer.provider.as_deref().unwrap_or("unspecified");
            let warnings = if receipt.warnings.is_empty() {
                "none".into()
            } else {
                receipt
                    .warnings
                    .iter()
                    .map(|warning| markdown_inline(warning))
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            format!(
                "- {}\n  - Runtime: {}\n  - Tool: {}\n  - Provider: {}\n  - Retrieved: {}\n  - Request SHA-256: {}\n  - Response SHA-256: {}\n  - Warnings: {}",
                markdown_code(&receipt.result_ref),
                markdown_code(&receipt.producer.runtime_id),
                markdown_code(&receipt.producer.tool_name),
                markdown_code(provider),
                receipt.retrieved_at,
                markdown_code(&receipt.request_digest),
                markdown_code(&receipt.response_digest),
                warnings,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "---\nid: {}\ntitle: {}\nsummary: {}\nas_of: {}\n---\n\n# Claims\n\n{}\n\n# Data\n\n{}\n\n# Sources\n\n{}\n",
        evidence.evidence_id,
        yaml_scalar(&evidence.title),
        yaml_scalar(&evidence.summary),
        as_of_or_now(Some(&evidence.as_of), timestamp),
        claims,
        data,
        sources,
    )
}

fn decision_title(thesis: &str, stance: &str) -> String {
    let from_thesis = thesis
        .lines()
        .next()
        .unwrap_or(thesis)
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if from_thesis.is_empty() {
        format!("Decision · {stance}")
    } else {
        from_thesis
    }
}

fn decision_markdown(
    decision: &ChatDecision,
    record_id: &str,
    timestamp: i64,
    supports: &[String],
    uses: &[String],
) -> String {
    let stance = decision
        .payload
        .get("stance")
        .and_then(Value::as_str)
        .unwrap_or("abstain");
    let thesis = decision
        .payload
        .get("thesis")
        .and_then(Value::as_str)
        .unwrap_or("No thesis was recorded.");
    let horizon = decision
        .payload
        .get("horizon")
        .and_then(Value::as_str)
        .unwrap_or("unspecified");
    let probability = decision
        .payload
        .get("probability")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let list = |name: &str, ids: &[String]| {
        if ids.is_empty() {
            format!("{name}: []\n")
        } else {
            format!(
                "{name}:\n{}",
                ids.iter()
                    .map(|id| format!("  - {id}\n"))
                    .collect::<String>()
            )
        }
    };
    let risks = decision
        .payload
        .get("risks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|value| format!("- {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let invalidation = decision
        .payload
        .get("invalidation_conditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|value| format!("- {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "---\nid: {record_id}\ntitle: {}\nsummary: {}\nas_of: {}\n{}{}---\n\n# Decision\n\n**Stance:** {stance}\n\n**Horizon:** {horizon}\n\n**Probability:** {probability:.4}\n\n{thesis}\n\n# Material risks\n\n{}\n\n# Invalidation conditions\n\n{}\n",
        yaml_scalar(&decision_title(thesis, stance)),
        yaml_scalar(&thesis.chars().take(240).collect::<String>()),
        as_of_or_now(None, timestamp),
        list("uses", uses),
        list("supports", supports),
        if risks.is_empty() { "- None recorded" } else { &risks },
        if invalidation.is_empty() { "- None recorded" } else { &invalidation },
    )
}

#[cfg(test)]
pub(super) async fn apply_chat_memory_update(
    state: &AppState,
    guru_id: &str,
    thread_id: &str,
    message_id: &str,
    update_memory: bool,
    capture: &ToolCapture,
) -> Result<Option<MemoryUpdateResult>, CommandError> {
    apply_chat_memory_update_with_finalize(
        state,
        guru_id,
        thread_id,
        message_id,
        update_memory,
        capture,
        |_| Ok(()),
    )
    .await
    .map(|(result, ())| result)
}

#[cfg(test)]
pub(super) async fn apply_chat_memory_update_with_finalize<T>(
    state: &AppState,
    guru_id: &str,
    thread_id: &str,
    message_id: &str,
    update_memory: bool,
    capture: &ToolCapture,
    finalize: impl FnOnce(Option<MemoryUpdateResult>) -> Result<T, CommandError>,
) -> Result<(Option<MemoryUpdateResult>, T), CommandError> {
    let (write_id, pending_writer) = reserve_chat_memory_finalization(state, guru_id)?;
    let memory_writer = pending_writer.wait().await?;
    apply_chat_memory_update_with_registered_finalize(
        state,
        guru_id,
        thread_id,
        message_id,
        update_memory,
        capture,
        write_id,
        memory_writer,
        finalize,
    )
    .await
}

pub(super) fn reserve_chat_memory_finalization(
    state: &AppState,
    guru_id: &str,
) -> Result<(String, PendingMemoryWrite), CommandError> {
    let write_id = new_id("memory-write");
    let pending_writer = state.reserve_memory_write(
        write_id.clone(),
        guru_id.to_owned(),
        RunTarget::MemoryWriteSession(write_id.clone()),
    )?;
    Ok((write_id, pending_writer))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_chat_memory_update_with_registered_finalize<T>(
    state: &AppState,
    guru_id: &str,
    thread_id: &str,
    message_id: &str,
    update_memory: bool,
    capture: &ToolCapture,
    write_id: String,
    memory_writer: RunRegistration,
    finalize: impl FnOnce(Option<MemoryUpdateResult>) -> Result<T, CommandError>,
) -> Result<(Option<MemoryUpdateResult>, T), CommandError> {
    // This lease is acquired before inspecting the captured turn and remains
    // held through the SQLite finalize callback, including no-change turns.
    // It may also have waited behind a writer that quarantined this Guru, so
    // recheck before even taking capture locks or running an artifact-only /
    // no-change finalizer.
    state.ensure_guru_available(guru_id)?;
    let proposals = if update_memory {
        capture.proposal.lock().await.clone()
    } else {
        Vec::new()
    };
    let decision = capture.decision.lock().await.clone();
    let staged_evidence = capture.staged_evidence.lock().await.clone();
    let mut selected = BTreeSet::new();
    for proposal in &proposals {
        selected.extend(proposal.source_memory_ids.iter().cloned());
    }
    if let Some(decision) = &decision {
        selected.extend(
            decision
                .payload
                .get("evidence_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
        selected.extend(
            decision
                .payload
                .get("uses_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    let memories = capture.memories.lock().await.clone();
    if proposals.is_empty() && decision.is_none() && staged_evidence.is_empty() {
        let result = update_memory.then_some(MemoryUpdateResult {
            status: MemoryUpdateStatus::NoChange,
            commit_id: None,
            changes: Vec::new(),
        });
        let finalized = finalize(result.clone())?;
        return Ok((result, finalized));
    }

    let profile = state
        .store
        .get_guru(guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    let workspace = profile_workspace(&profile)?;
    let runtime = state.runtime()?;
    workspace.validate(&runtime).await.map_err(map_internal)?;
    let timestamp = now_ms();
    let listed = workspace
        .knowledge_list(&runtime, None)
        .await
        .map_err(map_internal)?;
    validate_current_proposal_bases_and_identities(&runtime, &workspace, &listed, &proposals)
        .await?;
    let mut existing_ids = listed_record_ids(&listed);
    let mut targets = Vec::new();
    let immutable_base = MemoryProposalBase::Absent;

    for evidence in &staged_evidence {
        if !existing_ids.insert(evidence.evidence_id.clone()) {
            return Err(CommandError::conflict(
                "staged Evidence ID already exists in Guru Memory",
            ));
        }
        if let Some(target) = normalized_target(
            state,
            &workspace,
            "evidence",
            &evidence.evidence_id,
            Some(&immutable_base),
            evidence_markdown(evidence, timestamp),
        )
        .await?
        {
            targets.push(target);
        }
    }
    for proposal in &proposals {
        let kind = proposal.target_kind.to_ascii_lowercase();
        if let Some(target) = normalized_target(
            state,
            &workspace,
            &kind,
            &proposal.target_record_id,
            Some(&proposal.target_base),
            proposal.proposed_markdown.clone(),
        )
        .await?
        {
            targets.push(target);
        }
    }
    if let Some(decision) = &decision {
        let supports = decision
            .payload
            .get("evidence_ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let uses = decision
            .payload
            .get("uses_ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let thesis = decision
            .payload
            .get("thesis")
            .and_then(Value::as_str)
            .unwrap_or("Chat decision");
        let (record_id, _) = allocate_slug_id("decision", "theme", thesis, &mut existing_ids);
        if let Some(target) = normalized_target(
            state,
            &workspace,
            "decision",
            &record_id,
            Some(&immutable_base),
            decision_markdown(decision, &record_id, timestamp, &supports, &uses),
        )
        .await?
        {
            targets.push(target);
        }
    }
    if targets.is_empty() {
        let result = update_memory.then_some(MemoryUpdateResult {
            status: MemoryUpdateStatus::NoChange,
            commit_id: None,
            changes: Vec::new(),
        });
        let finalized = finalize(result.clone())?;
        return Ok((result, finalized));
    }

    let _ = (thread_id, message_id);
    let rationale = if proposals.is_empty() {
        "Persist exact evidence and the explicit Chat decision.".to_owned()
    } else {
        proposals
            .iter()
            .map(|proposal| proposal.rationale.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let basis = staged_evidence
        .iter()
        .flat_map(|evidence| evidence.claims.iter())
        .flat_map(|claim| claim.citations.iter())
        .map(|citation| {
            citation
                .receipt
                .producer
                .provider
                .as_deref()
                .unwrap_or(&citation.receipt.producer.tool_name)
        })
        .chain(
            selected
                .iter()
                .filter_map(|id| memories.get(id))
                .map(|memory| memory.title.as_str()),
        )
        .filter(|value| !value.trim().is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    let changes = targets
        .iter()
        .map(|target| {
            let kind = memory_kind_from_id(&target.record_id)?;
            Ok(MemoryUpdateChange {
                record_id: target.record_id.clone(),
                kind: kind.clone(),
                operation: if target.before_markdown.is_empty() {
                    "create"
                } else {
                    "revise"
                }
                .into(),
                title: json_text_from_markdown(&target.proposed_markdown, "title")
                    .unwrap_or_else(|| target.record_id.clone()),
                lesson: json_text_from_markdown(&target.proposed_markdown, "summary")
                    .unwrap_or_else(|| "Canonical Chat memory was updated.".into()),
                basis: if basis.is_empty() {
                    "Current Chat and its exact Memory reads".into()
                } else {
                    basis.clone()
                },
                future_use: match kind.as_str() {
                    "Wiki" => "This will shape factual context in later related research.",
                    "Lens" => "This will shape hypotheses, checks, and invalidation conditions in later related work.",
                    "Decision" => "This remains an immutable prior for later review and learning.",
                    _ => "This source can support later related research when read exactly.",
                }
                .into(),
            })
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    let (_, (result, finalized)) = memory_write::apply_memory_targets_registered(
        state,
        memory_write::RegisteredMemoryTransaction::chat(
            MemoryWrite {
                guru_id: guru_id.to_owned(),
                authority: MemoryChangeAuthority::Chat,
                targets,
                rationale,
            },
            write_id,
            memory_writer,
            thread_id,
            message_id,
        ),
        |written| {
            let result = MemoryUpdateResult {
                status: MemoryUpdateStatus::Applied,
                commit_id: Some(written.commit_id.clone()),
                changes,
            };
            let finalized = finalize(Some(result.clone()))?;
            Ok((result, finalized))
        },
    )
    .await?;
    Ok((Some(result), finalized))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web_result() -> tool_executor::RunResult {
        tool_executor::RunResult {
            result_ref: "result:one".into(),
            producer: tool_executor::RunResultProducer {
                runtime_id: "native-web".into(),
                tool_name: "web_fetch".into(),
                provider: Some("example.test".into()),
            },
            request_digest: "a".repeat(64),
            response_digest: "b".repeat(64),
            retrieved_at: "2026-08-13T00:00:00Z".into(),
            payload: serde_json::json!({"data": {"capacity": 91}}),
            warnings: Vec::new(),
            upstream_result_refs: Vec::new(),
        }
    }

    fn staged_evidence(text: &str, selected: Value) -> tool_executor::StagedEvidence {
        let result = web_result();
        let receipt = result.receipt();
        tool_executor::StagedEvidence {
            evidence_id: "evidence:chat/one".into(),
            title: "TSMC 3nm capacity".into(),
            summary: "Capacity and yield claims from the latest filing.".into(),
            as_of: "2026-08-13".into(),
            claims: vec![tool_executor::EvidenceClaim {
                text: text.into(),
                citations: vec![tool_executor::EvidenceCitation {
                    result_ref: receipt.result_ref.clone(),
                    pointer: "/data/capacity".into(),
                    excerpt: None,
                    selected,
                    receipt,
                }],
            }],
        }
    }

    #[test]
    fn evidence_contains_exact_data_and_host_receipt() {
        let evidence = staged_evidence(
            "3nm utilization rose on CoWoS tightness.",
            serde_json::json!(91),
        );
        let markdown = evidence_markdown(&evidence, 0);
        let frontmatter = markdown
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap()
            .0;
        assert!(frontmatter.contains("id: evidence:chat/one"));
        assert!(markdown.contains("# Claims"));
        assert!(markdown.contains("# Data"));
        assert!(markdown.contains("# Sources"));
        assert!(markdown.contains("result:one"));
        assert!(markdown.contains("/data/capacity"));
        assert!(markdown.contains("    91"));
        assert!(markdown.contains("Request SHA-256"));
        assert!(markdown.contains(&"a".repeat(64)));
    }

    #[test]
    fn slug_allocation_suffixes_collisions_and_records_updates() {
        let mut existing = BTreeSet::from(["evidence:theme/tsmc-3nm-capacity".into()]);
        let (first, updates) =
            allocate_slug_id("evidence", "theme", "TSMC 3nm capacity", &mut existing);
        assert_eq!(first, "evidence:theme/tsmc-3nm-capacity-2");
        assert_eq!(updates.as_deref(), Some("evidence:theme/tsmc-3nm-capacity"));
        let (fresh, none) = allocate_slug_id("evidence", "theme", "New theme", &mut existing);
        assert_eq!(fresh, "evidence:theme/new-theme");
        assert!(none.is_none());
    }

    #[test]
    fn derived_paths_are_slug_filenames() {
        assert_eq!(
            derived_memory_target_path("evidence", "evidence:theme/tsmc-3nm-capacity").unwrap(),
            "guruterminal/evidence/theme-tsmc-3nm-capacity.md"
        );
        assert_eq!(
            derived_memory_target_path("decision", "decision:theme/hold-tsmc").unwrap(),
            "guruterminal/decision/theme-hold-tsmc.md"
        );
    }

    #[test]
    fn evidence_markdown_flattens_untrusted_inline_text() {
        let markdown = evidence_markdown(
            &staged_evidence(
                "First line\n# Forged heading | cell",
                serde_json::json!("value\n# Not a heading"),
            ),
            0,
        );
        assert!(markdown.contains("First line # Forged heading | cell"));
        assert!(!markdown.contains("\n# Forged heading"));
        assert!(markdown.contains("    \"value\\n# Not a heading\""));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn immutable_target_rejects_a_record_created_after_absence_was_observed() {
        use std::{fs, sync::Arc};

        let temporary = tempfile::tempdir().unwrap();
        let workspace_path = temporary.path().join("guru");
        crate::commands::tests::initialized_workspace(&workspace_path, "immutable-target-cas");
        let runtime_path = temporary
            .path()
            .join("guruterminal-core-immutable-target-cas");
        crate::commands::tests::write_knowledge_runtime(&runtime_path);
        let mut state = AppState::for_test(temporary.path().join("app"));
        state.runtime = Some(Arc::new(
            crate::runtime::GuruTerminalRuntime::new(runtime_path).unwrap(),
        ));
        let workspace = crate::commands::tests::bound_root(&workspace_path);
        let existing =
            evidence_markdown(&staged_evidence("Existing claim.", serde_json::json!(1)), 0);
        fs::write(
            workspace_path.join("guruterminal/evidence/chat-one.md"),
            existing,
        )
        .unwrap();

        let result = normalized_target(
            &state,
            &workspace,
            "evidence",
            "evidence:chat/one",
            Some(&MemoryProposalBase::Absent),
            evidence_markdown(&staged_evidence("Later claim.", serde_json::json!(2)), 1),
        )
        .await;
        assert!(
            matches!(result, Err(ref error) if error.code == "conflict" && error.message.contains("was created")),
            "{result:?}"
        );
    }
}
