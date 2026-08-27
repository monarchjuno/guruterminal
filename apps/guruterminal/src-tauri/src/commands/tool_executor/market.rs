use super::*;

pub(super) fn bounded_chart_query_end(
    rows: &[Vec<Value>],
    offset: usize,
    requested_end: usize,
) -> Result<usize, BrokerError> {
    let mut bytes = 2_usize;
    let mut end = offset;
    for row in &rows[offset..requested_end] {
        let row_bytes = serde_json::to_vec(row)
            .map_err(|_| BrokerError::Execution("chart rows could not be serialized".into()))?
            .len();
        let separator = usize::from(end > offset);
        let next = bytes
            .checked_add(separator)
            .and_then(|value| value.checked_add(row_bytes))
            .ok_or_else(|| BrokerError::Execution("chart row window is too large".into()))?;
        if next > MAX_CHART_QUERY_BYTES {
            break;
        }
        bytes = next;
        end += 1;
    }
    if end == offset && offset < requested_end {
        return Err(BrokerError::Execution(
            "one chart row exceeds the query response limit".into(),
        ));
    }
    Ok(end)
}

pub(super) fn chart_edit_token(revision: &ChatArtifactRevision) -> String {
    sha256(
        format!(
            "{}:{}:{}",
            revision.artifact_id, revision.revision, revision.digest
        )
        .as_bytes(),
    )
}

pub(super) fn exact_object<'a>(
    value: &'a Value,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, BrokerError> {
    let object = value.as_object().ok_or(BrokerError::Malformed)?;
    if required.iter().any(|key| !object.contains_key(*key))
        || object
            .keys()
            .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(BrokerError::Malformed);
    }
    Ok(object)
}

pub(super) fn without_route_fields(
    mut value: Value,
    fields: &[&str],
) -> Result<Value, BrokerError> {
    let object = value.as_object_mut().ok_or(BrokerError::Malformed)?;
    for field in fields {
        if object.remove(*field).is_none() {
            return Err(BrokerError::Malformed);
        }
    }
    Ok(value)
}

pub(super) fn parse_as_of_date(
    value: Option<&str>,
) -> Result<Option<chrono::NaiveDate>, BrokerError> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(Some)
        .map_err(|_| BrokerError::Malformed)
}

pub(super) fn effective_as_of(
    policy: &ToolPolicy,
    params: &Value,
) -> Result<Option<chrono::NaiveDate>, BrokerError> {
    let tool = parse_as_of_date(params.get("as_of").and_then(Value::as_str))?;
    let turn = parse_as_of_date(policy.as_of.as_deref())?;
    Ok(match (turn, tool) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    })
}

pub(super) fn take_as_of(params: &mut Value) -> Result<Option<chrono::NaiveDate>, BrokerError> {
    let object = params.as_object_mut().ok_or(BrokerError::Malformed)?;
    let value = object.remove("as_of");
    parse_as_of_date(value.as_ref().and_then(Value::as_str))
}

pub(super) fn observation_date(value: &Value) -> Option<chrono::NaiveDate> {
    const KEYS: &[&str] = &[
        "date",
        "period",
        "filed_at",
        "filing_date",
        "available_at",
        "end",
        "report_date",
        "rcept_dt",
    ];
    for key in KEYS {
        let Some(text) = value.get(*key).and_then(Value::as_str) else {
            continue;
        };
        if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
            return Some(date);
        }
        if text.len() == 4 {
            if let Ok(year) = text.parse::<i32>() {
                return chrono::NaiveDate::from_ymd_opt(year, 12, 31);
            }
        }
        if text.len() == 8 {
            if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y%m%d") {
                return Some(date);
            }
        }
    }
    None
}

pub(super) fn record_date(value: Option<&str>) -> Option<chrono::NaiveDate> {
    let value = value?;
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.date_naive())
        .or_else(|| observation_date(&json!({ "date": value })))
}

pub(super) fn is_after_cutoff(as_of: Option<&str>, cutoff: chrono::NaiveDate) -> bool {
    record_date(as_of).is_none_or(|date| date > cutoff)
}

pub(super) fn apply_as_of_cutoff(
    output: &mut Value,
    cutoff: chrono::NaiveDate,
) -> Result<(), BrokerError> {
    let mut excluded = 0_usize;
    if let Some(data) = output.get_mut("data") {
        for field in ["observations", "rows", "bars", "facts", "filings"] {
            let Some(rows) = data.get_mut(field).and_then(Value::as_array_mut) else {
                continue;
            };
            let before = rows.len();
            rows.retain(|row| observation_date(row).is_none_or(|date| date <= cutoff));
            excluded += before.saturating_sub(rows.len());
        }
    }
    if let Some(page) = output.get_mut("page").and_then(Value::as_object_mut) {
        if let Some(returned) = page.get("returned").and_then(Value::as_u64) {
            page.insert(
                "returned".into(),
                json!(returned.saturating_sub(excluded as u64)),
            );
        }
    }
    let quality = output
        .get_mut("quality")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| BrokerError::Execution("provider result had no quality".into()))?;
    quality.insert(
        "as_of".into(),
        json!({
            "cutoff": cutoff.to_string(),
            "post_cutoff_excluded": excluded,
            "action": "excluded"
        }),
    );
    if let Some(checks) = quality.get_mut("checks").and_then(Value::as_array_mut) {
        checks.push(json!({
            "code": "as_of_cutoff",
            "status": if excluded == 0 { "pass" } else { "warn" },
            "detail": format!("{excluded} observations after {cutoff} were excluded")
        }));
    }
    if excluded > 0 {
        if let Some(warnings) = output.get_mut("warnings").and_then(Value::as_array_mut) {
            warnings.push(json!(format!(
                "excluded {excluded} observations after as-of {cutoff}"
            )));
        }
    }
    let empty = output
        .get("data")
        .and_then(|data| {
            ["observations", "rows", "bars", "facts", "filings"]
                .iter()
                .find_map(|field| data.get(*field).and_then(Value::as_array))
        })
        .is_some_and(|rows| rows.is_empty());
    if empty {
        return Err(BrokerError::Execution(
            "no observations remain at the requested as-of cutoff".into(),
        ));
    }
    Ok(())
}

pub(super) fn finance_context_from_sources(cutoff: DateTime<Utc>, sources: Vec<Value>) -> Value {
    json!({
        "data_cutoff": cutoff.to_rfc3339_opts(SecondsFormat::Secs, true),
        "timeout_ms": 30_000,
        "sources": sources
    })
}

pub(super) fn validate_decision_shape(params: &Value) -> Result<(), BrokerError> {
    let object = exact_object(
        params,
        &[
            "title",
            "stance",
            "horizon",
            "probability",
            "thesis",
            "evidence_ids",
            "uses_ids",
            "risks",
            "invalidation_conditions",
        ],
        &["summary"],
    )?;
    if !object
        .get("title")
        .and_then(Value::as_str)
        .is_some_and(|value| {
            let chars = value.chars().count();
            (1..=180).contains(&chars) && !value.contains('\0')
        })
        || object.get("summary").is_some_and(|value| {
            !value.as_str().is_some_and(|summary| {
                let chars = summary.chars().count();
                (1..=400).contains(&chars) && !summary.contains('\0')
            })
        })
        || !object
            .get("stance")
            .and_then(Value::as_str)
            .is_some_and(|value| matches!(value, "positive" | "neutral" | "negative" | "abstain"))
        || !object
            .get("probability")
            .and_then(Value::as_f64)
            .is_some_and(|value| (0.0..=1.0).contains(&value))
        || !object
            .get("horizon")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        || !object
            .get("thesis")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        || [
            "evidence_ids",
            "uses_ids",
            "risks",
            "invalidation_conditions",
        ]
        .iter()
        .any(|key| object.get(*key).and_then(Value::as_array).is_none())
    {
        return Err(BrokerError::Malformed);
    }
    Ok(())
}

pub(super) fn validate_decision_references(
    params: &Value,
    staged_evidence: &[StagedEvidence],
    memories: &HashMap<String, MemoryRefSnapshot>,
) -> Result<(), BrokerError> {
    let stance = params
        .get("stance")
        .and_then(Value::as_str)
        .ok_or(BrokerError::Malformed)?;
    let evidence_ids = params
        .get("evidence_ids")
        .and_then(Value::as_array)
        .filter(|ids| ids.len() <= 64)
        .ok_or(BrokerError::Malformed)?;
    if stance != "abstain" && evidence_ids.is_empty() {
        return Err(BrokerError::Malformed);
    }
    let staged_evidence_ids = staged_evidence
        .iter()
        .map(|evidence| evidence.evidence_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut unique = BTreeSet::new();
    for evidence_id in evidence_ids {
        let evidence_id = evidence_id
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 512 && !id.contains('\0'))
            .ok_or(BrokerError::Malformed)?;
        if !unique.insert(evidence_id) {
            return Err(BrokerError::Execution(
                "decision evidence IDs must be unique".into(),
            ));
        }
        if !staged_evidence_ids.contains(evidence_id) {
            return Err(BrokerError::Execution(
                "decision evidence_ids must name Evidence created in this turn".into(),
            ));
        }
    }
    let uses_ids = params
        .get("uses_ids")
        .and_then(Value::as_array)
        .filter(|ids| ids.len() <= crate::domain::MAX_MEMORY_REFS)
        .ok_or(BrokerError::Malformed)?;
    let mut unique = BTreeSet::new();
    for memory_id in uses_ids {
        let memory_id = memory_id
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 512 && !id.contains('\0'))
            .ok_or(BrokerError::Malformed)?;
        if !unique.insert(memory_id) {
            return Err(BrokerError::Execution(
                "decision uses_ids must be unique".into(),
            ));
        }
        if !memories.get(memory_id).is_some_and(|memory| {
            memory.access == MemoryAccess::ExactRead
                && matches!(memory.kind.as_str(), "Wiki" | "Lens")
        }) {
            return Err(BrokerError::Execution(
                "decision uses_ids must name exact-read Wiki or Lens records from this turn".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_provider_result(
    output: &Value,
    expected_tool: &str,
    expected_source_id: &str,
) -> Result<(), BrokerError> {
    if output.get("schema_version").and_then(Value::as_str) != Some("guruterminal-finance-result/1")
        || output.get("tool").and_then(Value::as_str) != Some(expected_tool)
        || output.get("source_id").and_then(Value::as_str) != Some(expected_source_id)
    {
        return Err(BrokerError::Execution(
            "provider result did not match its declared contract".into(),
        ));
    }
    output
        .get("provenance")
        .and_then(Value::as_object)
        .ok_or_else(|| BrokerError::Execution("provider result had no provenance".into()))?;
    output
        .get("quality")
        .and_then(|quality| quality.get("status"))
        .and_then(Value::as_str)
        .filter(|status| matches!(*status, "pass" | "warn"))
        .ok_or_else(|| {
            BrokerError::Execution("provider result quality status was invalid".into())
        })?;
    Ok(())
}

#[cfg(test)]
mod as_of_tests {
    use super::*;

    #[test]
    fn as_of_cutoff_excludes_later_observations_and_keeps_unknown_dates() {
        let mut output = json!({
            "data": {
                "observations": [
                    {"date": "2024-06-01", "value": 1},
                    {"date": "2024-07-01", "value": 2},
                    {"value": 3}
                ]
            },
            "page": {"returned": 3, "available": 3},
            "quality": {"status": "pass", "checks": []},
            "warnings": []
        });
        apply_as_of_cutoff(
            &mut output,
            chrono::NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
        )
        .unwrap();
        assert_eq!(output["data"]["observations"].as_array().unwrap().len(), 2);
        assert_eq!(output["quality"]["as_of"]["post_cutoff_excluded"], 1);
        assert_eq!(output["page"]["returned"], 2);
    }

    #[test]
    fn memory_cutoff_treats_rfc3339_and_date_only() {
        let cutoff = chrono::NaiveDate::from_ymd_opt(2024, 6, 30).unwrap();
        assert!(!is_after_cutoff(Some("2024-06-30T23:59:59Z"), cutoff));
        assert!(is_after_cutoff(Some("2024-07-01T00:00:00Z"), cutoff));
        assert!(is_after_cutoff(Some("2025-01-01"), cutoff));
        assert!(is_after_cutoff(None, cutoff));
    }
}
