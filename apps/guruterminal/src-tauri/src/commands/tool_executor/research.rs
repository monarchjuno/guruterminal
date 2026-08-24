use std::collections::BTreeSet;

use super::*;
use crate::{broker::MAX_MEMORY_PROPOSALS, domain::MAX_MEMORY_REFS};

struct GuruSearchBackend<'a> {
    state: &'a crate::app::AppState,
}

impl crate::web::SearchBackend for GuruSearchBackend<'_> {
    async fn attempt(
        &self,
        provider: crate::web::SearchProviderId,
        query: &crate::web::WebSearchQuery,
        remaining: std::time::Duration,
        cancel: &crate::web::SearchCancel,
    ) -> Result<crate::web::SearchHits, crate::web::WebError> {
        match provider {
            crate::web::SearchProviderId::ExaPublic => crate::web::search_exa_once(query).await,
            _ => {
                crate::provider_connection::run_native_web_search(
                    self.state, provider, query, remaining, cancel,
                )
                .await
            }
        }
    }
}

impl AppToolExecutor {
    pub(super) async fn web_search(
        &self,
        policy: &ToolPolicy,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        self.require_capability("community.web-research")?;
        let cutoff = effective_as_of(policy, &params)?;
        let object = exact_object(
            &params,
            &["query"],
            &[
                "limit",
                "as_of",
                "recency",
                "include_domains",
                "exclude_domains",
            ],
        )?;
        let query = object
            .get("query")
            .and_then(Value::as_str)
            .filter(|value| {
                !value.trim().is_empty()
                    && value.len() <= 4_096
                    && !value.contains('\0')
                    && !value
                        .chars()
                        .any(|character| character.is_control() && !character.is_whitespace())
            })
            .ok_or(BrokerError::Malformed)?;
        let limit = object.get("limit").and_then(Value::as_u64).unwrap_or(5);
        if !(1..=10).contains(&limit) {
            return Err(BrokerError::Malformed);
        }
        let recency = object
            .get("recency")
            .and_then(Value::as_str)
            .map(|value| crate::web::WebRecency::parse(value).ok_or(BrokerError::Malformed))
            .transpose()?;
        let include_domains = parse_search_domains(object.get("include_domains"))?;
        let exclude_domains = parse_search_domains(object.get("exclude_domains"))?;
        let (mut output, sources) = crate::web::execute_search(
            crate::web::SearchRequest {
                query: crate::web::WebSearchQuery {
                    query: query.trim().to_owned(),
                    limit: limit as u8,
                    recency,
                    include_domains,
                    exclude_domains,
                },
                policy: self.capture.web_search_policy,
                chat_provider: (!self.chat_provider.is_empty()).then(|| self.chat_provider.clone()),
                cancel: self.capture.search_cancel.clone(),
            },
            &GuruSearchBackend { state: &self.state },
        )
        .await
        .map_err(|error| BrokerError::Execution(error.to_string()))?;
        let retained: Vec<_> = if let Some(cutoff) = cutoff {
            output.results.retain(|result| {
                result
                    .published_at
                    .as_deref()
                    .is_none_or(|published| !is_after_cutoff(Some(published), cutoff))
            });
            sources
                .into_iter()
                .filter(|source| {
                    source
                        .published_at
                        .as_deref()
                        .is_none_or(|published| !is_after_cutoff(Some(published), cutoff))
                })
                .collect()
        } else {
            sources
        };
        self.capture
            .stage(delivery_id, |pending| {
                for source in retained {
                    pending.web_sources.insert(source.source_id.clone(), source);
                }
            })
            .await;
        let mut value = serde_json::to_value(output)
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        if let Some(cutoff) = cutoff {
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "as_of".into(),
                    json!({
                        "cutoff": cutoff.to_string(),
                        "undated_results_kept": true
                    }),
                );
            }
        }
        Ok(value)
    }

    pub(super) async fn web_fetch(
        &self,
        policy: &ToolPolicy,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        self.require_capability("community.web-research")?;
        let cutoff = effective_as_of(policy, &params)?;
        let object = exact_object(&params, &[], &["source_id", "url", "as_of", "offset"])?;
        let source_id = object
            .get("source_id")
            .and_then(Value::as_str)
            .filter(|value| value.starts_with("web:") && value.len() <= 128);
        let url = object
            .get("url")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty() && value.len() <= 2_048);
        let content_offset = object
            .get("offset")
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|offset| usize::try_from(offset).ok())
                    .filter(|offset| *offset < crate::web::MAX_FETCH_EXTRACTED_BYTES)
                    .ok_or(BrokerError::Malformed)
            })
            .transpose()?
            .unwrap_or(0);
        let source = match (source_id, url) {
            (Some(source_id), None) => self
                .capture
                .web_sources
                .lock()
                .await
                .get(source_id)
                .cloned()
                .ok_or(BrokerError::MethodDenied)?,
            (None, Some(url)) => {
                let source = crate::web::source_from_url(url)
                    .map_err(|error| BrokerError::Execution(error.to_string()))?;
                self.capture
                    .stage(delivery_id, |pending| {
                        pending
                            .web_sources
                            .insert(source.source_id.clone(), source.clone());
                    })
                    .await;
                source
            }
            _ => return Err(BrokerError::Malformed),
        };
        if cutoff.is_some_and(|cutoff| {
            source
                .published_at
                .as_deref()
                .is_some_and(|published| is_after_cutoff(Some(published), cutoff))
        }) {
            return Err(BrokerError::Execution(
                "web source is after the requested as-of cutoff".into(),
            ));
        }
        let existing_snapshot = self
            .capture
            .web_fetch_snapshots
            .lock()
            .await
            .get(&source.source_id)
            .cloned();
        let mut run_snapshot = match existing_snapshot {
            Some(snapshot) => snapshot,
            None if content_offset == 0 => RunWebFetchSnapshot {
                fetched: crate::web::fetch(&source)
                    .await
                    .map_err(|error| BrokerError::Execution(error.to_string()))?,
                issued_offsets: BTreeSet::from([0]),
            },
            None => return Err(BrokerError::Malformed),
        };
        require_issued_web_offset(&run_snapshot.issued_offsets, content_offset)?;
        let page = run_snapshot
            .fetched
            .page(content_offset)
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        if let Some(next_offset) = page.next_offset {
            run_snapshot.issued_offsets.insert(next_offset);
        }
        let mut output = serde_json::to_value(page)
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        if let Some(cutoff) = cutoff {
            if let Some(object) = output.as_object_mut() {
                object.insert(
                    "as_of".into(),
                    json!({
                        "cutoff": cutoff.to_string(),
                        "publication_date": source.published_at,
                        "publication_date_unknown": source.published_at.is_none()
                    }),
                );
            }
        }
        let snapshot_source_id = source.source_id.clone();
        self.capture
            .stage(delivery_id, |pending| {
                pending
                    .web_fetch_snapshots
                    .insert(snapshot_source_id, run_snapshot);
            })
            .await;
        Ok(output)
    }

    pub(super) async fn stage_proposal(
        &self,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let object = exact_object(
            &params,
            &[
                "kind",
                "target_id",
                "proposed_markdown",
                "rationale",
                "source_ids",
            ],
            &[],
        )?;
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .filter(|kind| matches!(*kind, "wiki" | "lens"))
            .ok_or(BrokerError::Malformed)?;
        let target_id = object
            .get("target_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 512)
            .ok_or(BrokerError::Malformed)?;
        let proposed = object
            .get("proposed_markdown")
            .and_then(Value::as_str)
            .filter(|value| {
                !value.is_empty() && value.len() <= 2 * 1024 * 1024 && !value.contains('\0')
            })
            .ok_or(BrokerError::Malformed)?;
        let rationale = object
            .get("rationale")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 8_192)
            .ok_or(BrokerError::Malformed)?;
        let raw_source_ids = object
            .get("source_ids")
            .and_then(Value::as_array)
            .filter(|source_ids| !source_ids.is_empty() && source_ids.len() <= MAX_MEMORY_REFS)
            .ok_or(BrokerError::Malformed)?;
        let memories = self.capture.memories.lock().await;
        let staged_evidence = self.capture.staged_evidence.lock().await;
        let staged_evidence_ids = staged_evidence
            .iter()
            .map(|evidence| evidence.evidence_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut source_memory_ids = Vec::with_capacity(raw_source_ids.len());
        let mut unique_source_ids = BTreeSet::new();
        for source_id in raw_source_ids {
            let source_id = source_id
                .as_str()
                .filter(|value| {
                    !value.is_empty() && value.len() <= 512 && !value.contains(['\0', '\n', '\r'])
                })
                .ok_or(BrokerError::Malformed)?;
            if !unique_source_ids.insert(source_id.to_owned()) {
                return Err(BrokerError::Malformed);
            }
            if !memories
                .get(source_id)
                .is_some_and(|memory| memory.access == MemoryAccess::ExactRead)
                && !staged_evidence_ids.contains(source_id)
            {
                return Err(BrokerError::Execution(
                    "memory update source_ids must name staged Evidence or exact-read Memory from this turn"
                        .into(),
                ));
            }
            source_memory_ids.push(source_id.to_owned());
        }
        let target_full_read_digest = memories
            .get(target_id)
            .filter(|memory| memory.access == MemoryAccess::ExactRead && memory.section.is_none())
            .and_then(|memory| memory.full_record_digest.clone());
        drop(staged_evidence);
        drop(memories);
        if crate::domain::markdown_frontmatter_id(proposed).as_deref() != Some(target_id) {
            return Err(BrokerError::Execution(
                "proposed_markdown frontmatter id must equal target_id".into(),
            ));
        }
        if kind == "lens" && !crate::domain::lens_proposal_has_required_sections(proposed) {
            return Err(BrokerError::Execution(
                "Lens proposals require non-empty # Scope, # Assumptions, # Counterexamples, # Limits, and # Invalidation conditions".into(),
            ));
        }
        if let Some(status) = crate::domain::markdown_frontmatter_scalar(proposed, "status") {
            match status.as_str() {
                "active" => {
                    if crate::domain::markdown_frontmatter_scalar(proposed, "revoked_by").is_some()
                    {
                        return Err(BrokerError::Execution(
                            "revoked_by is permitted only when status is revoked".into(),
                        ));
                    }
                }
                "revoked" => {
                    let Some(revoked_by) =
                        crate::domain::markdown_frontmatter_scalar(proposed, "revoked_by")
                    else {
                        return Err(BrokerError::Execution(
                            "revoked Wiki or Lens requires revoked_by".into(),
                        ));
                    };
                    if revoked_by == target_id {
                        return Err(BrokerError::Execution(
                            "revoked_by must not reference the same record".into(),
                        ));
                    }
                    if crate::domain::CanonicalMemoryKind::parse_record_id(&revoked_by).is_none() {
                        return Err(BrokerError::Execution(
                            "revoked_by must use a canonical Memory record ID".into(),
                        ));
                    }
                }
                _ => {
                    return Err(BrokerError::Execution(
                        "status must be active or revoked".into(),
                    ));
                }
            }
        } else if crate::domain::markdown_frontmatter_scalar(proposed, "revoked_by").is_some() {
            return Err(BrokerError::Execution(
                "revoked_by is permitted only when status is revoked".into(),
            ));
        }
        let proposed_title = crate::domain::markdown_frontmatter_scalar(proposed, "title")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                BrokerError::Execution("proposed_markdown frontmatter title is required".into())
            })?;
        let proposed_aliases = crate::domain::markdown_frontmatter_list(proposed, "aliases");
        let mut identity_records = self
            .capture
            .proposal
            .lock()
            .await
            .iter()
            .filter_map(|proposal| memory_identity_proposal(proposal, kind))
            .collect::<Vec<_>>();
        for pending in self.capture.pending_deliveries.lock().await.values() {
            identity_records.extend(
                pending
                    .proposal
                    .iter()
                    .filter_map(|proposal| memory_identity_proposal(proposal, kind)),
            );
        }
        let target_base = target_full_read_digest
            .clone()
            .map(|digest| crate::domain::MemoryProposalBase::FullRead { digest })
            .unwrap_or(crate::domain::MemoryProposalBase::Absent);
        if let Ok((runtime, workspace)) = self.runtime_scope(&self.guru_id) {
            let listed = workspace
                .knowledge_list(&runtime, Some(kind))
                .await
                .map_err(|error| BrokerError::Execution(error.to_string()))?;
            let persisted_records = listed
                .as_array()
                .map(|records| {
                    records
                        .iter()
                        .filter_map(memory_identity_record)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let target_matches = persisted_records
                .iter()
                .filter(|record| record.id == target_id)
                .count();
            match (target_matches, &target_base) {
                (0, crate::domain::MemoryProposalBase::Absent) => {}
                (0, crate::domain::MemoryProposalBase::FullRead { .. }) => {
                    return Err(BrokerError::Execution(
                        "the full-read Memory target no longer exists; do not recreate it from a stale proposal"
                            .into(),
                    ));
                }
                (1, crate::domain::MemoryProposalBase::Absent) => {
                    return Err(BrokerError::Execution(
                        "exact-read the full existing record before revising it".into(),
                    ));
                }
                (1, crate::domain::MemoryProposalBase::FullRead { .. }) => {}
                _ => {
                    return Err(BrokerError::Execution(
                        "the Memory target is ambiguous".into(),
                    ));
                }
            }
            identity_records.extend(persisted_records);
        }
        if let Some(existing_id) = crate::domain::colliding_active_memory_id(
            target_id,
            &proposed_title,
            &proposed_aliases,
            &identity_records,
        ) {
            return Err(BrokerError::Execution(format!(
                "an active {kind} already uses this title or alias; revise {existing_id} instead of creating a duplicate"
            )));
        }
        let target_kind = match kind {
            "wiki" => "Wiki",
            "lens" => "Lens",
            _ => unreachable!("validated proposal kind"),
        }
        .to_owned();
        let proposal = MemoryProposal::new(
            new_id("proposal"),
            target_kind,
            target_id.into(),
            target_base,
            proposed.into(),
            rationale.into(),
            source_memory_ids,
            None,
        )
        .map_err(|error| BrokerError::Execution(error.to_string()))?;
        let mut staged = self.capture.proposal.lock().await.clone();
        for pending in self.capture.pending_deliveries.lock().await.values() {
            for pending_proposal in &pending.proposal {
                upsert_turn_proposal(&mut staged, pending_proposal.clone())?;
            }
        }
        upsert_turn_proposal(&mut staged, proposal.clone())?;
        self.capture
            .stage(delivery_id, |pending| {
                let _ = upsert_turn_proposal(&mut pending.proposal, proposal.clone());
            })
            .await;
        Ok(json!({
            "proposal_id": proposal.id,
            "proposal_digest": proposal.digest,
            "status": "accepted_for_atomic_apply",
            "host_application": "automatic_after_turn"
        }))
    }

    pub(super) async fn stage_decision(
        &self,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        validate_decision_shape(&params)?;
        let staged_evidence = self.capture.staged_evidence.lock().await;
        let memories = self.capture.memories.lock().await;
        validate_decision_references(&params, &staged_evidence, &memories)?;
        drop(memories);
        drop(staged_evidence);
        let encoded = serde_json::to_vec(&params).map_err(|_| BrokerError::Malformed)?;
        let decision = ChatDecision {
            payload: params,
            digest: sha256(&encoded),
            sealed_at_ms: now_ms(),
        };
        decision
            .validate()
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        if self.capture.decision.lock().await.is_some()
            || self
                .capture
                .pending_deliveries
                .lock()
                .await
                .values()
                .any(|pending| pending.decision.is_some())
        {
            return Err(BrokerError::Execution(
                "a Chat turn may seal only one decision".into(),
            ));
        }
        self.capture
            .stage(delivery_id, |pending| {
                pending.decision = Some(decision.clone());
            })
            .await;
        Ok(json!({
            "status": "sealed",
            "decision_digest": decision.digest,
            "sealed_at": DateTime::<Utc>::from_timestamp_millis(decision.sealed_at_ms)
                .ok_or(BrokerError::Malformed)?
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        }))
    }
}

fn require_issued_web_offset(
    issued_offsets: &BTreeSet<usize>,
    content_offset: usize,
) -> Result<(), BrokerError> {
    issued_offsets
        .contains(&content_offset)
        .then_some(())
        .ok_or(BrokerError::Malformed)
}

fn parse_search_domains(value: Option<&Value>) -> Result<Vec<String>, BrokerError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value.as_array().ok_or(BrokerError::Malformed)?;
    if items.is_empty() || items.len() > 10 {
        return Err(BrokerError::Malformed);
    }
    let mut domains = Vec::with_capacity(items.len());
    let mut seen = BTreeSet::new();
    for item in items {
        let domain = item
            .as_str()
            .ok_or(BrokerError::Malformed)
            .and_then(|value| {
                crate::web::validate_search_domain(value).map_err(|_| BrokerError::Malformed)
            })?;
        if !seen.insert(domain.clone()) {
            return Err(BrokerError::Malformed);
        }
        domains.push(domain);
    }
    Ok(domains)
}

fn memory_identity_record(value: &Value) -> Option<crate::domain::MemoryIdentityRecord> {
    Some(crate::domain::MemoryIdentityRecord {
        id: value.get("id")?.as_str()?.to_owned(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        aliases: value
            .get("aliases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn memory_identity_proposal(
    proposal: &MemoryProposal,
    kind: &str,
) -> Option<crate::domain::MemoryIdentityRecord> {
    if !proposal.target_kind.eq_ignore_ascii_case(kind) {
        return None;
    }
    Some(crate::domain::MemoryIdentityRecord {
        id: proposal.target_record_id.clone(),
        title: crate::domain::markdown_frontmatter_scalar(&proposal.proposed_markdown, "title")?,
        aliases: crate::domain::markdown_frontmatter_list(&proposal.proposed_markdown, "aliases"),
        status: crate::domain::markdown_frontmatter_scalar(&proposal.proposed_markdown, "status"),
    })
}

fn upsert_turn_proposal(
    proposals: &mut Vec<MemoryProposal>,
    proposal: MemoryProposal,
) -> Result<(), BrokerError> {
    if let Some(existing) = proposals
        .iter_mut()
        .find(|existing| existing.target_record_id == proposal.target_record_id)
    {
        *existing = proposal;
        return Ok(());
    }
    if proposals.len() >= MAX_MEMORY_PROPOSALS as usize {
        return Err(BrokerError::Malformed);
    }
    proposals.push(proposal);
    Ok(())
}

#[cfg(test)]
mod paging_tests {
    use super::*;

    #[test]
    fn accepts_only_offsets_issued_for_the_cached_web_snapshot() {
        let issued = BTreeSet::from([0, 262_143]);
        assert!(require_issued_web_offset(&issued, 0).is_ok());
        assert!(require_issued_web_offset(&issued, 262_143).is_ok());
        assert!(matches!(
            require_issued_web_offset(&issued, 1),
            Err(BrokerError::Malformed)
        ));
    }
}
