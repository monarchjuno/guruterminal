use super::*;

impl AppToolExecutor {
    pub(super) async fn stage_evidence_create(
        &self,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let object = exact_object(&params, &["title", "summary", "as_of", "claims"], &[])?;
        let title = bounded_text(object.get("title"), 1, 180)?;
        let summary = bounded_text(object.get("summary"), 1, 400)?;
        let as_of = bounded_text(object.get("as_of"), 10, 128)?;
        if chrono::NaiveDate::parse_from_str(&as_of, "%Y-%m-%d").is_err()
            && DateTime::parse_from_rfc3339(&as_of).is_err()
        {
            return Err(BrokerError::Malformed);
        }
        let values = object
            .get("claims")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty() && values.len() <= MAX_EVIDENCE_CLAIMS)
            .ok_or(BrokerError::Malformed)?;
        let mut claims = Vec::with_capacity(values.len());
        let mut selected_bytes = 0_usize;
        for value in values {
            let claim = exact_object(value, &["text", "citations"], &[])?;
            let text = bounded_text(claim.get("text"), 1, 800)?;
            let citation_values = claim
                .get("citations")
                .and_then(Value::as_array)
                .filter(|items| !items.is_empty() && items.len() <= MAX_EVIDENCE_CITATIONS)
                .ok_or(BrokerError::Malformed)?;
            let mut citations = Vec::with_capacity(citation_values.len());
            let mut unique = BTreeSet::new();
            for value in citation_values {
                let citation = exact_object(value, &["result_ref", "pointer"], &["excerpt"])?;
                let result_ref = bounded_text(citation.get("result_ref"), 1, 128)?;
                let pointer = citation
                    .get("pointer")
                    .and_then(Value::as_str)
                    .filter(|pointer| {
                        crate::json_pointer::valid_json_pointer(
                            pointer,
                            MAX_EVIDENCE_POINTER_BYTES,
                            true,
                        )
                    })
                    .map(str::to_owned)
                    .ok_or(BrokerError::Malformed)?;
                let excerpt = match citation.get("excerpt") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(excerpt))
                        if !excerpt.is_empty()
                            && excerpt.len() <= MAX_EVIDENCE_EXCERPT_BYTES
                            && !excerpt.contains('\0') =>
                    {
                        Some(excerpt.clone())
                    }
                    _ => return Err(BrokerError::Malformed),
                };
                if !unique.insert((result_ref.clone(), pointer.clone(), excerpt.clone())) {
                    return Err(BrokerError::Execution(
                        "evidence citations must be unique within a claim".into(),
                    ));
                }
                let (receipt, pointed) = self
                    .capture
                    .run_result_selection(&result_ref, &pointer)
                    .await
                    .ok_or_else(|| {
                        BrokerError::Execution(
                            "evidence result_ref must name a delivered result from this turn"
                                .into(),
                        )
                    })?;
                let pointed = pointed.ok_or_else(|| {
                    BrokerError::Execution(
                        "evidence citation pointer did not resolve in the selected result".into(),
                    )
                })?;
                let selected = match excerpt.as_deref() {
                    Some(excerpt) => pointed
                        .as_str()
                        .filter(|value| value.contains(excerpt))
                        .map(|_| Value::String(excerpt.to_owned()))
                        .ok_or_else(|| {
                            BrokerError::Execution(
                                "evidence excerpt must be an exact substring of the selected value"
                                    .into(),
                            )
                        })?,
                    None => pointed,
                };
                selected_bytes = selected_bytes
                    .checked_add(
                        serde_json::to_vec(&selected)
                            .map_err(|_| BrokerError::Malformed)?
                            .len(),
                    )
                    .filter(|bytes| *bytes <= MAX_EVIDENCE_SELECTED_BYTES)
                    .ok_or_else(|| {
                        BrokerError::Execution("evidence selected data is too large".into())
                    })?;
                citations.push(EvidenceCitation {
                    result_ref,
                    pointer,
                    excerpt,
                    selected,
                    receipt,
                });
            }
            claims.push(EvidenceClaim { text, citations });
        }
        let evidence_id = format!("evidence:chat/{}", uuid::Uuid::new_v4().simple());
        let staged = StagedEvidence {
            evidence_id: evidence_id.clone(),
            title,
            summary,
            as_of,
            claims,
        };
        let committed = self.capture.staged_evidence.lock().await;
        let mut pending = self.capture.pending_deliveries.lock().await;
        let pending_count = pending
            .values()
            .map(|pending| pending.staged_evidence.len())
            .sum::<usize>();
        if committed.len() + pending_count >= MAX_EVIDENCE_RECORDS {
            return Err(BrokerError::Execution(
                "a Chat turn may create at most three Evidence records".into(),
            ));
        }
        pending
            .entry(delivery_id.to_owned())
            .or_default()
            .staged_evidence
            .push(staged);
        drop(pending);
        drop(committed);
        Ok(json!({
            "evidence_id": evidence_id,
            "status": "staged_for_atomic_apply"
        }))
    }

    pub(super) fn finance_sources(&self, params: Value) -> Result<Value, BrokerError> {
        exact_object(&params, &[], &[])?;
        self.require_capability("guruterminal.finance-core")?;
        let mut sources = self.state.finance_data.sources();
        if let Some(entries) = sources.get_mut("sources").and_then(Value::as_array_mut) {
            entries.retain(|source| {
                source
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|entry_id| self.capability_enabled(entry_id))
            });
        }
        Ok(sources)
    }

    pub(super) fn capability_enabled(&self, entry_id: &str) -> bool {
        self.capability_ids.contains(entry_id)
    }

    pub(super) fn require_capability(&self, entry_id: &str) -> Result<(), BrokerError> {
        if self.capability_enabled(entry_id) {
            Ok(())
        } else {
            Err(BrokerError::MethodDenied)
        }
    }

    #[cfg(test)]
    pub(super) async fn capture_memory(
        &self,
        memory: MemoryRefSnapshot,
    ) -> Result<(), BrokerError> {
        memory
            .validate()
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        let mut capture = self.capture.memories.lock().await;
        if let Some(existing) = capture.get(&memory.record_id) {
            if memory_authority_rank(existing) > memory_authority_rank(&memory) {
                return Ok(());
            }
        }
        capture.insert(memory.record_id.clone(), memory);
        Ok(())
    }

    pub(super) async fn stage_memory(
        &self,
        delivery_id: &str,
        memory: MemoryRefSnapshot,
    ) -> Result<(), BrokerError> {
        memory
            .validate()
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        self.capture
            .stage(delivery_id, |pending| {
                if pending
                    .memories
                    .get(&memory.record_id)
                    .is_some_and(|existing| {
                        memory_authority_rank(existing) > memory_authority_rank(&memory)
                    })
                {
                    return;
                }
                pending.memories.insert(memory.record_id.clone(), memory);
            })
            .await;
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn commit_test_capture<T>(
        &self,
        delivery_id: &str,
        result: Result<T, BrokerError>,
    ) -> Result<T, BrokerError> {
        if result.is_ok() {
            let _ = self.capture.commit_delivery(delivery_id).await;
        } else {
            self.capture.discard_delivery(delivery_id).await;
        }
        result
    }

    #[cfg(test)]
    pub(super) async fn seal_decision(&self, params: Value) -> Result<Value, BrokerError> {
        let delivery_id = new_id("test-delivery");
        let result = self.stage_decision(params, &delivery_id).await;
        self.commit_test_capture(&delivery_id, result).await
    }

    #[cfg(test)]
    pub(super) async fn capture_proposal(&self, params: Value) -> Result<Value, BrokerError> {
        let delivery_id = new_id("test-delivery");
        let result = self.stage_proposal(params, &delivery_id).await;
        self.commit_test_capture(&delivery_id, result).await
    }

    #[cfg(test)]
    pub(super) async fn create_evidence(&self, params: Value) -> Result<Value, BrokerError> {
        let delivery_id = new_id("test-delivery");
        let result = self.stage_evidence_create(params, &delivery_id).await;
        self.commit_test_capture(&delivery_id, result).await
    }

    pub(super) fn validate_provider_tool_output(
        &self,
        output: Value,
        expected_tool: &str,
        expected_source_id: &str,
    ) -> Result<Value, BrokerError> {
        validate_provider_result(&output, expected_tool, expected_source_id)?;
        Ok(output)
    }

    pub(super) async fn guru_read_previous(
        &self,
        policy: &ToolPolicy,
        params: Value,
    ) -> Result<Value, BrokerError> {
        let object = exact_object(&params, &["id"], &[])?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 512)
            .ok_or(BrokerError::Malformed)?;
        let (runtime, workspace) = self.runtime_scope(&policy.guru_id)?;
        let listed = workspace
            .knowledge_list(&runtime, None)
            .await
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        let path = listed
            .as_array()
            .into_iter()
            .flatten()
            .find(|record| record.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|record| record.get("path").and_then(Value::as_str))
            .ok_or_else(|| BrokerError::Execution("memory record is missing".into()))?;
        let previous = crate::memory_git::read_previous_markdown(workspace.path(), path)
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        Ok(match previous {
            Some(previous) => json!({
                "id": id,
                "path": path,
                "markdown": previous.markdown,
                "commit_id": previous.commit_id,
                "content_class": "untrusted_memory"
            }),
            None => json!({
                "id": id,
                "path": path,
                "markdown": Value::Null,
                "commit_id": Value::Null,
                "content_class": "untrusted_memory"
            }),
        })
    }

    pub(super) fn ensure_scope(&self, policy: &ToolPolicy) -> Result<(), BrokerError> {
        if policy.guru_id != self.guru_id {
            return Err(BrokerError::Execution(
                "Guru/tool root scope does not match".into(),
            ));
        }
        let chat = self
            .state
            .store
            .get_chat(&policy.session_id)
            .map_err(|error| BrokerError::Execution(error.to_string()))?
            .ok_or_else(|| BrokerError::Execution("chat session is missing".into()))?;
        if chat.guru_id != policy.guru_id {
            return Err(BrokerError::Execution(
                "Guru/session scope does not match".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn guru_search(
        &self,
        policy: &ToolPolicy,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let cutoff = effective_as_of(policy, &params)?;
        let object = exact_object(&params, &["query"], &["kind", "limit", "as_of"])?;
        let query = object
            .get("query")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 4_096)
            .ok_or(BrokerError::Malformed)?;
        let kind = object.get("kind").and_then(Value::as_str);
        if kind.is_some_and(|kind| parse_memory_kind(kind).is_err()) {
            return Err(BrokerError::Malformed);
        }
        let limit = object.get("limit").and_then(Value::as_u64).unwrap_or(6);
        if !(1..=6).contains(&limit) {
            return Err(BrokerError::Malformed);
        }
        let (runtime, workspace) = self.runtime_scope(&policy.guru_id)?;
        let cutoff_arg = cutoff.map(|value| value.format("%Y-%m-%d").to_string());
        let mut value = workspace
            .knowledge_search(
                &runtime,
                query,
                kind,
                limit as u8,
                cutoff.is_some(),
                cutoff_arg.as_deref(),
            )
            .await
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        if let Some(records) = value.as_array_mut() {
            records.retain(|record| {
                let revoked = record.get("status").and_then(Value::as_str) == Some("revoked");
                let after_cutoff = cutoff.is_some_and(|cutoff| {
                    is_after_cutoff(record.get("as_of").and_then(Value::as_str), cutoff)
                });
                if after_cutoff {
                    return false;
                }
                if revoked {
                    if cutoff.is_some() {
                        return true;
                    }
                    return false;
                }
                true
            });
            if cutoff.is_some() {
                for record in records.iter_mut() {
                    if record.get("status").and_then(Value::as_str) == Some("revoked") {
                        record["unused"] = serde_json::json!(true);
                    }
                }
            }
        }
        if let Some(records) = value.as_array() {
            for record in records {
                if let Ok(summary) = runtime_record_summary(record) {
                    let memory = MemoryRefSnapshot {
                        record_id: summary.id,
                        kind: summary.kind,
                        title: summary.title,
                        excerpt: summary.excerpt,
                        as_of: summary.as_of,
                        section: None,
                        access: MemoryAccess::SearchDiscovered,
                        full_record_digest: None,
                    };
                    self.stage_memory(delivery_id, memory).await?;
                }
            }
        }
        Ok(value)
    }

    pub(super) async fn guru_read(
        &self,
        policy: &ToolPolicy,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let cutoff = effective_as_of(policy, &params)?;
        let object = exact_object(&params, &["id"], &["section", "as_of"])?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 512)
            .ok_or(BrokerError::Malformed)?;
        let section = object.get("section").and_then(Value::as_str);
        if section.is_some_and(|value| value.is_empty() || value.len() > 512) {
            return Err(BrokerError::Malformed);
        }
        let (runtime, workspace) = self.runtime_scope(&policy.guru_id)?;
        let value = workspace
            .knowledge_read(&runtime, id, section)
            .await
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        let full_record_digest = if section.is_none() {
            Some(crate::hashing::sha256(
                value
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        BrokerError::Execution(
                            "full Memory read did not return canonical content".into(),
                        )
                    })?
                    .as_bytes(),
            ))
        } else {
            None
        };
        if let Some(document) = value.get("document") {
            let summary = runtime_record_summary(document)
                .map_err(|error| BrokerError::Execution(error.to_string()))?;
            if cutoff.is_some_and(|cutoff| is_after_cutoff(summary.as_of.as_deref(), cutoff)) {
                return Err(BrokerError::Execution(
                    "memory record is after the requested as-of cutoff".into(),
                ));
            }
            let memory = MemoryRefSnapshot {
                record_id: summary.id,
                kind: summary.kind,
                title: summary.title,
                excerpt: summary.excerpt,
                as_of: summary.as_of,
                section: section.map(str::to_owned),
                access: MemoryAccess::ExactRead,
                full_record_digest,
            };
            self.stage_memory(delivery_id, memory).await?;
        }
        let mut wrapped = value;
        if let Some(object) = wrapped.as_object_mut() {
            object.insert(
                "content_class".into(),
                Value::String("untrusted_memory".into()),
            );
        }
        Ok(wrapped)
    }

    pub(super) fn runtime_scope<'a>(
        &'a self,
        guru_id: &str,
    ) -> Result<(Arc<crate::runtime::GuruTerminalRuntime>, &'a BoundGuruRoot), BrokerError> {
        if guru_id != self.guru_id {
            return Err(BrokerError::Execution(
                "Guru/tool root scope does not match".into(),
            ));
        }
        let runtime = self
            .state
            .runtime
            .clone()
            .ok_or_else(|| BrokerError::Execution("Guru Runtime is unavailable".into()))?;
        Ok((runtime, &self.guru_root))
    }
}

fn memory_authority_rank(memory: &MemoryRefSnapshot) -> u8 {
    match (memory.access, memory.section.as_ref()) {
        (MemoryAccess::SearchDiscovered, _) => 0,
        (MemoryAccess::ExactRead, Some(_)) => 1,
        (MemoryAccess::ExactRead, None) => 2,
    }
}

const MAX_EVIDENCE_RECORDS: usize = 3;
const MAX_EVIDENCE_CLAIMS: usize = 16;
const MAX_EVIDENCE_CITATIONS: usize = 8;
const MAX_EVIDENCE_POINTER_BYTES: usize = 2 * 1024;
const MAX_EVIDENCE_EXCERPT_BYTES: usize = 8 * 1024;
const MAX_EVIDENCE_SELECTED_BYTES: usize = 256 * 1024;

fn bounded_text(value: Option<&Value>, min: usize, max: usize) -> Result<String, BrokerError> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| {
            let chars = text.chars().count();
            chars >= min && chars <= max && !text.contains('\0')
        })
        .map(str::to_owned)
        .ok_or(BrokerError::Malformed)
}
