use super::*;

fn normalize_chart_note(note: Option<String>) -> Option<String> {
    note.and_then(|note| {
        let trimmed = note.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

impl AppToolExecutor {
    pub(super) async fn compute_run(
        &self,
        _policy: &ToolPolicy,
        params: Value,
        _delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        self.require_capability("guruterminal.compute-python")?;
        let language = params
            .get("language")
            .and_then(Value::as_str)
            .ok_or(BrokerError::Malformed)?;
        match language {
            "python" => {
                exact_object(
                    &params,
                    &["language", "source"],
                    &["inputs", "packages", "seed"],
                )?;
            }
            "javascript" => {
                exact_object(&params, &["language", "source"], &["inputs", "seed"])?;
            }
            _ => return Err(BrokerError::Malformed),
        }
        let call: crate::compute::ComputeCall =
            serde_json::from_value(params).map_err(|_| BrokerError::Malformed)?;
        call.validate().map_err(|_| BrokerError::Malformed)?;
        self.capture
            .compute
            .run(call)
            .await
            .map_err(|error| BrokerError::Execution(error.to_string()))
    }

    pub(super) fn artifact_list(
        &self,
        policy: &ToolPolicy,
        params: Value,
    ) -> Result<Value, BrokerError> {
        exact_object(&params, &[], &[])?;
        let artifacts = self
            .state
            .store
            .list_chat_artifacts(&policy.session_id)
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        Ok(json!({
            "artifacts": artifacts.into_iter().map(|artifact| json!({
                "artifact_id": artifact.id,
                "title": artifact.title,
                "kind": artifact.kind,
                "current_revision": artifact.current_revision,
                "updated_at_ms": artifact.updated_at_ms,
            })).collect::<Vec<_>>()
        }))
    }

    pub(super) async fn artifact_read(
        &self,
        policy: &ToolPolicy,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let object = exact_object(&params, &["artifact_id"], &[])?;
        let artifact_id = object
            .get("artifact_id")
            .and_then(Value::as_str)
            .ok_or(BrokerError::Malformed)?
            .to_owned();
        let artifact = self
            .state
            .store
            .get_chat_artifact(&artifact_id)
            .map_err(|error| BrokerError::Execution(error.to_string()))?
            .ok_or_else(|| BrokerError::Execution("artifact was not found".into()))?;
        if artifact.chat_session_id != policy.session_id {
            return Err(BrokerError::MethodDenied);
        }
        let revision = self
            .state
            .store
            .get_chat_artifact_current(&artifact.id)
            .map_err(|error| BrokerError::Execution(error.to_string()))?
            .ok_or_else(|| BrokerError::Execution("artifact content was not found".into()))?;
        self.capture
            .stage(delivery_id, |pending| {
                pending
                    .artifact_reads
                    .insert((artifact.id.clone(), revision.revision));
            })
            .await;
        match &revision.payload {
            ChatArtifactPayload::Markdown { .. } => Ok(json!({
                "artifact": artifact,
                "revision": revision,
            })),
            ChatArtifactPayload::Chart { chart, .. } => {
                let dataset = self
                    .state
                    .store
                    .get_chart_dataset(&chart.dataset_id)
                    .map_err(|error| BrokerError::Execution(error.to_string()))?
                    .ok_or_else(|| BrokerError::Execution("chart dataset was not found".into()))?;
                chart
                    .validate_dataset(&dataset)
                    .map_err(|error| BrokerError::Execution(error.to_string()))?;
                Ok(json!({
                    "artifact": {
                        "artifact_id": artifact.id,
                        "title": artifact.title,
                        "kind": "chart",
                        "revision": revision.revision,
                        "revision_digest": revision.digest,
                        "updated_at_ms": artifact.updated_at_ms,
                        "edit_token": chart_edit_token(&revision),
                    },
                    "chart": chart,
                    "dataset": dataset.summary(),
                    "message": "Chart data stays in native storage. Use chart_query only when exact rows are required."
                }))
            }
        }
    }

    pub(super) async fn artifact_publish(
        &self,
        policy: &ToolPolicy,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let object = exact_object(
            &params,
            &["mode", "title", "payload"],
            &["artifact_id", "expected_revision"],
        )?;
        let mode = object
            .get("mode")
            .and_then(Value::as_str)
            .ok_or(BrokerError::Malformed)?;
        let title = object
            .get("title")
            .and_then(Value::as_str)
            .ok_or(BrokerError::Malformed)?
            .trim()
            .to_owned();
        let payload = serde_json::from_value::<ChatArtifactPayload>(
            object
                .get("payload")
                .cloned()
                .ok_or(BrokerError::Malformed)?,
        )
        .map_err(|_| BrokerError::Malformed)?;
        payload
            .validate()
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        if matches!(payload, ChatArtifactPayload::Chart { .. }) {
            return Err(BrokerError::Execution(
                "financial and analytic charts must be published with chart_publish".into(),
            ));
        }
        let source_message_id = self
            .capture
            .source_message_id
            .clone()
            .ok_or(BrokerError::MethodDenied)?;
        let timestamp = now_ms();

        let commit = match mode {
            "create" => {
                if object.contains_key("artifact_id") || object.contains_key("expected_revision") {
                    return Err(BrokerError::Malformed);
                }
                let artifact_id = new_id("artifact");
                let revision = ChatArtifactRevision::new(
                    artifact_id.clone(),
                    1,
                    payload,
                    source_message_id,
                    timestamp,
                )
                .map_err(|error| BrokerError::Execution(error.to_string()))?;
                ArtifactCommit {
                    artifact: ChatArtifact {
                        id: artifact_id,
                        chat_session_id: policy.session_id.clone(),
                        kind: revision.payload.kind(),
                        title,
                        current_revision: 1,
                        created_at_ms: timestamp,
                        updated_at_ms: timestamp,
                    },
                    revision,
                    datasets: Vec::new(),
                }
            }
            "revise" => {
                let artifact_id = object
                    .get("artifact_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or(BrokerError::Malformed)?;
                let expected_revision = object
                    .get("expected_revision")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or(BrokerError::Malformed)?;
                if !self
                    .capture
                    .artifact_reads
                    .lock()
                    .await
                    .contains(&(artifact_id.clone(), expected_revision))
                {
                    return Err(BrokerError::Execution(
                        "artifact revision must be read before it can be revised".into(),
                    ));
                }
                let mut artifact = self
                    .state
                    .store
                    .get_chat_artifact(&artifact_id)
                    .map_err(|error| BrokerError::Execution(error.to_string()))?
                    .ok_or_else(|| BrokerError::Execution("artifact was not found".into()))?;
                if artifact.chat_session_id != policy.session_id {
                    return Err(BrokerError::MethodDenied);
                }
                if artifact.current_revision != expected_revision {
                    return Err(BrokerError::Execution(
                        "artifact revision is stale; read the current revision".into(),
                    ));
                }
                if artifact.kind != payload.kind() {
                    return Err(BrokerError::Execution(
                        "an artifact cannot change kind across revisions".into(),
                    ));
                }
                let created_at_ms = timestamp.max(artifact.updated_at_ms.saturating_add(1));
                artifact.title = title;
                artifact.current_revision = expected_revision.saturating_add(1);
                artifact.updated_at_ms = created_at_ms;
                let revision = ChatArtifactRevision::new(
                    artifact.id.clone(),
                    artifact.current_revision,
                    payload,
                    source_message_id,
                    created_at_ms,
                )
                .map_err(|error| BrokerError::Execution(error.to_string()))?;
                ArtifactCommit {
                    artifact,
                    revision,
                    datasets: Vec::new(),
                }
            }
            _ => return Err(BrokerError::Malformed),
        };
        self.stage_artifact(
            commit,
            delivery_id,
            "The artifact will be saved only if this Chat turn completes.",
        )
        .await
    }

    pub(super) async fn chart_query(
        &self,
        policy: &ToolPolicy,
        params: Value,
    ) -> Result<Value, BrokerError> {
        let request: ChartQueryRequest =
            serde_json::from_value(params).map_err(|_| BrokerError::Malformed)?;
        if request.limit == 0 || request.limit > MAX_CHART_QUERY_ROWS {
            return Err(BrokerError::Malformed);
        }
        let artifact = self
            .state
            .store
            .get_chat_artifact(&request.artifact_id)
            .map_err(|error| BrokerError::Execution(error.to_string()))?
            .ok_or_else(|| BrokerError::Execution("artifact was not found".into()))?;
        if artifact.chat_session_id != policy.session_id || artifact.kind != ChatArtifactKind::Chart
        {
            return Err(BrokerError::MethodDenied);
        }
        if !self
            .capture
            .artifact_reads
            .lock()
            .await
            .contains(&(artifact.id.clone(), request.revision))
        {
            return Err(BrokerError::Execution(
                "the exact chart revision must be read before its rows can be queried".into(),
            ));
        }
        let revision = self
            .state
            .store
            .get_chat_artifact_current(&artifact.id)
            .map_err(|error| BrokerError::Execution(error.to_string()))?
            .ok_or_else(|| BrokerError::Execution("artifact content was not found".into()))?;
        if revision.revision != request.revision {
            return Err(BrokerError::Execution(
                "the chart changed after it was read; read the current chart again".into(),
            ));
        }
        let ChatArtifactPayload::Chart { chart, .. } = revision.payload else {
            return Err(BrokerError::Execution("artifact is not a chart".into()));
        };
        let dataset = self
            .state
            .store
            .get_chart_dataset(&chart.dataset_id)
            .map_err(|error| BrokerError::Execution(error.to_string()))?
            .ok_or_else(|| BrokerError::Execution("chart dataset was not found".into()))?;
        chart
            .validate_dataset(&dataset)
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        if request.offset > dataset.rows.len() {
            return Err(BrokerError::Malformed);
        }
        let requested_end = request
            .offset
            .saturating_add(request.limit)
            .min(dataset.rows.len());
        let end = bounded_chart_query_end(&dataset.rows, request.offset, requested_end)?;
        Ok(json!({
            "artifact_id": artifact.id,
            "revision": revision.revision,
            "revision_digest": revision.digest,
            "dataset_id": dataset.id,
            "dataset_digest": dataset.digest,
            "columns": dataset.columns,
            "offset": request.offset,
            "rows": &dataset.rows[request.offset..end],
            "next_offset": (end < dataset.rows.len()).then_some(end),
            "total_rows": dataset.rows.len(),
        }))
    }

    async fn materialize_chart_dataset(
        &self,
        input: crate::chart_engine::ChartDatasetInput,
    ) -> Result<crate::chart_engine::ChartDataset, BrokerError> {
        match input {
            crate::chart_engine::ChartDatasetInput::FromResult(envelope) => {
                let result = self
                    .capture
                    .run_result(&envelope.from_result.result_ref)
                    .await
                    .ok_or_else(|| {
                        BrokerError::Execution(
                            "chart result_ref must name a delivered result from this turn".into(),
                        )
                    })?;
                crate::chart_engine::dataset_from_result_selection(
                    new_id("dataset"),
                    envelope.from_result,
                    &result.payload,
                    chart_result_receipt(&result),
                )
                .map_err(|error| BrokerError::Execution(error.to_string()))
            }
            crate::chart_engine::ChartDatasetInput::Inline(envelope) => {
                let mut receipts = Vec::with_capacity(envelope.inline.upstream_result_refs.len());
                for result_ref in &envelope.inline.upstream_result_refs {
                    let result = self.capture.run_result(result_ref).await.ok_or_else(|| {
                        BrokerError::Execution(
                            "chart upstream_result_refs must name delivered results from this turn"
                                .into(),
                        )
                    })?;
                    receipts.push(chart_result_receipt(&result));
                }
                crate::chart_engine::dataset_from_inline(
                    new_id("dataset"),
                    envelope.inline,
                    receipts,
                )
                .map_err(|error| BrokerError::Execution(error.to_string()))
            }
        }
    }

    pub(super) async fn chart_publish(
        &self,
        policy: &ToolPolicy,
        params: Value,
        delivery_id: &str,
    ) -> Result<Value, BrokerError> {
        let request: ChartPublishRequest =
            serde_json::from_value(params).map_err(|_| BrokerError::Malformed)?;
        let title = request.title.trim();
        if title.is_empty() || title.len() > 200 {
            return Err(BrokerError::Malformed);
        }
        let title = title.to_owned();
        let timestamp = now_ms();
        let source_message_id = self
            .capture
            .source_message_id
            .clone()
            .ok_or(BrokerError::MethodDenied)?;
        let commit = match request.mode {
            ChartPublishMode::Create => {
                if request.artifact_id.is_some() || request.edit_token.is_some() {
                    return Err(BrokerError::Malformed);
                }
                let view = request.view.ok_or(BrokerError::Malformed)?;
                let dataset = self
                    .materialize_chart_dataset(request.dataset.ok_or(BrokerError::Malformed)?)
                    .await?;
                let document = crate::chart_engine::ChartDocument {
                    dataset_id: dataset.id.clone(),
                    dataset_digest: dataset.digest.clone(),
                    view,
                    studies: request.studies.unwrap_or_default(),
                    drawings: request.drawings.unwrap_or_default(),
                    note: normalize_chart_note(request.note),
                };
                document
                    .validate()
                    .map_err(|error| BrokerError::Execution(error.to_string()))?;
                let artifact_id = new_id("artifact");
                let revision = ChatArtifactRevision::new(
                    artifact_id.clone(),
                    1,
                    ChatArtifactPayload::Chart {
                        schema: crate::chart_engine::CHART_SCHEMA.into(),
                        chart: Box::new(document),
                    },
                    source_message_id,
                    timestamp,
                )
                .map_err(|error| BrokerError::Execution(error.to_string()))?;
                ArtifactCommit {
                    artifact: ChatArtifact {
                        id: artifact_id,
                        chat_session_id: policy.session_id.clone(),
                        kind: ChatArtifactKind::Chart,
                        title,
                        current_revision: 1,
                        created_at_ms: timestamp,
                        updated_at_ms: timestamp,
                    },
                    revision,
                    datasets: vec![dataset],
                }
            }
            ChartPublishMode::Revise => {
                let artifact_id = request
                    .artifact_id
                    .as_deref()
                    .ok_or(BrokerError::Malformed)?;
                let edit_token = request
                    .edit_token
                    .as_deref()
                    .ok_or(BrokerError::Malformed)?;
                let mut artifact = self
                    .state
                    .store
                    .get_chat_artifact(artifact_id)
                    .map_err(|error| BrokerError::Execution(error.to_string()))?
                    .ok_or_else(|| BrokerError::Execution("artifact was not found".into()))?;
                if artifact.chat_session_id != policy.session_id
                    || artifact.kind != ChatArtifactKind::Chart
                {
                    return Err(BrokerError::MethodDenied);
                }
                let current = self
                    .state
                    .store
                    .get_chat_artifact_current(&artifact.id)
                    .map_err(|error| BrokerError::Execution(error.to_string()))?
                    .ok_or_else(|| {
                        BrokerError::Execution("artifact content was not found".into())
                    })?;
                if chart_edit_token(&current) != edit_token
                    || !self
                        .capture
                        .artifact_reads
                        .lock()
                        .await
                        .contains(&(artifact.id.clone(), current.revision))
                {
                    return Err(BrokerError::Execution(
                        "read the current chart before revising it".into(),
                    ));
                }
                let ChatArtifactPayload::Chart {
                    chart: current_chart,
                    ..
                } = current.payload
                else {
                    return Err(BrokerError::Execution("artifact is not a chart".into()));
                };
                let dataset = if let Some(dataset) = request.dataset {
                    self.materialize_chart_dataset(dataset).await?
                } else {
                    self.state
                        .store
                        .get_chart_dataset(&current_chart.dataset_id)
                        .map_err(|error| BrokerError::Execution(error.to_string()))?
                        .ok_or_else(|| {
                            BrokerError::Execution("chart dataset was not found".into())
                        })?
                };
                let document = crate::chart_engine::ChartDocument {
                    dataset_id: dataset.id.clone(),
                    dataset_digest: dataset.digest.clone(),
                    view: request.view.unwrap_or(current_chart.view),
                    studies: request.studies.unwrap_or(current_chart.studies),
                    drawings: request.drawings.unwrap_or(current_chart.drawings),
                    note: match request.note {
                        Some(note) => normalize_chart_note(Some(note)),
                        None => current_chart.note,
                    },
                };
                document
                    .validate()
                    .map_err(|error| BrokerError::Execution(error.to_string()))?;
                let created_at_ms = timestamp.max(artifact.updated_at_ms.saturating_add(1));
                artifact.title = title;
                artifact.current_revision = artifact.current_revision.saturating_add(1);
                artifact.updated_at_ms = created_at_ms;
                let revision = ChatArtifactRevision::new(
                    artifact.id.clone(),
                    artifact.current_revision,
                    ChatArtifactPayload::Chart {
                        schema: crate::chart_engine::CHART_SCHEMA.into(),
                        chart: Box::new(document),
                    },
                    source_message_id,
                    created_at_ms,
                )
                .map_err(|error| BrokerError::Execution(error.to_string()))?;
                ArtifactCommit {
                    artifact,
                    revision,
                    datasets: vec![dataset],
                }
            }
        };
        self.stage_artifact(
            commit,
            delivery_id,
            "The chart will be saved only if this Chat turn completes.",
        )
        .await
    }

    pub(super) async fn stage_artifact(
        &self,
        commit: ArtifactCommit,
        delivery_id: &str,
        message: &'static str,
    ) -> Result<Value, BrokerError> {
        commit
            .validate()
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        let reference = commit.revision.artifact_ref(commit.artifact.title.clone());
        let mut pending = self.capture.pending_deliveries.lock().await;
        let committed = self.capture.artifacts.lock().await;
        let mut published_ids = BTreeSet::new();
        for existing in committed.iter() {
            published_ids.insert(existing.artifact.id.clone());
        }
        for pending_capture in pending.values() {
            for existing in &pending_capture.artifacts {
                published_ids.insert(existing.artifact.id.clone());
            }
        }
        if published_ids.contains(&commit.artifact.id) {
            return Err(BrokerError::Execution(
                "a Chat turn may not publish the same artifact twice".into(),
            ));
        }
        if published_ids.len() >= crate::chat_artifacts::MAX_CHAT_TURN_ARTIFACTS {
            return Err(BrokerError::Execution(format!(
                "a Chat turn may publish at most {} artifact revisions",
                crate::chat_artifacts::MAX_CHAT_TURN_ARTIFACTS
            )));
        }
        drop(committed);
        pending
            .entry(delivery_id.to_owned())
            .or_default()
            .artifacts
            .push(commit);
        Ok(json!({
            "status": "staged",
            "artifact": reference,
            "message": message,
        }))
    }
}

fn chart_result_receipt(result: &RunResult) -> crate::chart_engine::ChartResultReceipt {
    crate::chart_engine::ChartResultReceipt {
        result_ref: result.result_ref.clone(),
        runtime_id: result.producer.runtime_id.clone(),
        tool_name: result.producer.tool_name.clone(),
        provider: result.producer.provider.clone(),
        request_digest: result.request_digest.clone(),
        response_digest: result.response_digest.clone(),
        retrieved_at: result.retrieved_at.clone(),
        warnings: result.warnings.clone(),
        upstream_result_refs: result.upstream_result_refs.clone(),
    }
}
