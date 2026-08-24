use std::collections::BTreeSet;

use chrono::{DateTime, SecondsFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::{
    app::{AppState, CommandError},
    domain::{CanonicalMemoryKind, MemoryChangeAuthority, MemoryChangeTarget},
    guru_root::profile_workspace,
    hashing::sha256,
    maintenance::MaintenanceActivityKind,
    store::GuruTerminalStore,
};

use super::{map_internal, map_store, memory_write, require_text};

const MAX_BODY_BYTES: usize = 128 * 1024;
const MAX_LIST_ITEMS: usize = 64;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryDraftDto {
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub as_of: String,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub see_also: Vec<String>,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub assumptions: String,
    #[serde(default)]
    pub counterexamples: String,
    #[serde(default)]
    pub limits: String,
    #[serde(default)]
    pub invalidation_conditions: String,
    pub body_markdown: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryMemoryCreateRequest {
    pub guru_id: String,
    pub draft: LibraryDraftDto,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryMemoryUpdateRequest {
    pub guru_id: String,
    pub record_id: String,
    pub draft: LibraryDraftDto,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryMemoryDeleteRequest {
    pub guru_id: String,
    pub record_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryMemoryRevertRequest {
    pub guru_id: String,
    pub record_id: String,
    #[serde(default)]
    pub expected_markdown: Option<String>,
    #[serde(default)]
    pub commit_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LibraryMemoryMutationDto {
    pub commit_id: String,
    pub record_id: String,
}

fn yaml_scalar(value: &str) -> String {
    format!(
        "'{}'",
        value.replace('\'', "''").replace(['\n', '\r', '\0'], " ")
    )
}

fn validate_items(values: &[String], label: &str) -> Result<Vec<String>, CommandError> {
    if values.len() > MAX_LIST_ITEMS {
        return Err(CommandError::invalid(format!(
            "{label} has too many values"
        )));
    }
    let mut unique = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        let value = require_text(value, label, 512)?;
        if value.contains(['\n', '\r']) {
            return Err(CommandError::invalid(format!(
                "{label} contains an invalid value"
            )));
        }
        if unique.insert(value.clone()) {
            out.push(value);
        }
    }
    Ok(out)
}

fn yaml_list(markdown: &mut String, key: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    markdown.push_str(key);
    markdown.push_str(":\n");
    for value in values {
        markdown.push_str("  - ");
        markdown.push_str(&yaml_scalar(value));
        markdown.push('\n');
    }
}

fn draft_markdown(
    draft: &LibraryDraftDto,
    record_id: &str,
) -> Result<(CanonicalMemoryKind, String), CommandError> {
    let kind = CanonicalMemoryKind::from_label(&draft.kind)
        .filter(|kind| matches!(kind, CanonicalMemoryKind::Wiki | CanonicalMemoryKind::Lens))
        .ok_or_else(|| CommandError::invalid("Only Wiki and Lens can be edited in Library"))?;
    let title = require_text(&draft.title, "Memory title", 512)?;
    let summary = require_text(&draft.summary, "Memory summary", 2_048)?;
    let as_of = DateTime::parse_from_rfc3339(draft.as_of.trim())
        .map_err(|_| CommandError::invalid("Memory as_of must be an RFC3339 timestamp"))?
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    let entities = validate_items(&draft.entities, "Memory entities")?;
    let aliases = validate_items(&draft.aliases, "Memory aliases")?;
    let tags = validate_items(&draft.tags, "Memory tags")?;
    let see_also = validate_items(&draft.see_also, "Memory see_also")?;
    if kind != CanonicalMemoryKind::Wiki && !see_also.is_empty() {
        return Err(CommandError::invalid("Only Wiki may use see_also"));
    }
    require_text(&draft.body_markdown, "Memory body", MAX_BODY_BYTES)?;
    if draft.body_markdown.contains('\0') {
        return Err(CommandError::invalid("Memory body is too large or invalid"));
    }
    if kind == CanonicalMemoryKind::Lens {
        for (value, label) in [
            (&draft.scope, "Lens scope"),
            (&draft.assumptions, "Lens assumptions"),
            (&draft.counterexamples, "Lens counterexamples"),
            (&draft.limits, "Lens limits"),
            (
                &draft.invalidation_conditions,
                "Lens invalidation conditions",
            ),
        ] {
            require_text(value, label, 16 * 1024)?;
        }
    }

    let mut markdown = format!(
        "---\nid: {record_id}\ntitle: {}\nsummary: {}\nas_of: {}\n",
        yaml_scalar(&title),
        yaml_scalar(&summary),
        as_of
    );
    yaml_list(&mut markdown, "entities", &entities);
    yaml_list(&mut markdown, "aliases", &aliases);
    yaml_list(&mut markdown, "tags", &tags);
    yaml_list(&mut markdown, "see_also", &see_also);
    markdown.push_str("---\n\n");
    if kind == CanonicalMemoryKind::Lens {
        for (heading, value) in [
            ("Scope", &draft.scope),
            ("Assumptions", &draft.assumptions),
            ("Counterexamples", &draft.counterexamples),
            ("Limits", &draft.limits),
            ("Invalidation conditions", &draft.invalidation_conditions),
        ] {
            markdown.push_str("# ");
            markdown.push_str(heading);
            markdown.push_str("\n\n");
            markdown.push_str(value.trim());
            markdown.push_str("\n\n");
        }
    }
    if !draft.body_markdown.trim().is_empty() {
        markdown.push_str(draft.body_markdown.trim());
        markdown.push('\n');
    }
    Ok((kind, markdown))
}

fn slug(value: &str) -> String {
    let mut out = String::new();
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_lowercase());
            separator = false;
        } else if !separator && !out.is_empty() {
            out.push('-');
            separator = true;
        }
        if out.len() >= 72 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "memory".into()
    } else {
        out
    }
}

fn allocate_library_id(kind: &str, title: &str, existing: &BTreeSet<String>) -> String {
    let base_slug = slug(title);
    let base = format!("{kind}:{base_slug}");
    if !existing.contains(&base) {
        return base;
    }
    let mut index = 2_u32;
    loop {
        let candidate = format!("{kind}:{base_slug}-{index}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        index = index.saturating_add(1);
        if index > 9_999 {
            return format!("{kind}:{base_slug}-overflow");
        }
    }
}

async fn workspace_and_runtime(
    state: &AppState,
    guru_id: &str,
) -> Result<
    (
        crate::guru_root::BoundGuruRoot,
        std::sync::Arc<crate::runtime::GuruTerminalRuntime>,
    ),
    CommandError,
> {
    state.ensure_guru_available(guru_id)?;
    let profile = state
        .store
        .get_guru(guru_id)
        .map_err(map_store)?
        .ok_or_else(|| CommandError::not_found("Guru"))?;
    Ok((profile_workspace(&profile)?, state.runtime()?))
}

async fn apply_user_change(
    state: &AppState,
    guru_id: &str,
    target: MemoryChangeTarget,
    rationale: String,
) -> Result<LibraryMemoryMutationDto, CommandError> {
    let written = memory_write::apply_memory_targets(
        state,
        guru_id,
        MemoryChangeAuthority::User,
        vec![target.clone()],
        &rationale,
    )
    .await?;
    Ok(LibraryMemoryMutationDto {
        commit_id: written.commit_id,
        record_id: target.record_id,
    })
}

fn list_record<'a>(records: &'a [Value], record_id: &str) -> Result<&'a Value, CommandError> {
    let matches = records
        .iter()
        .filter(|record| record.get("id").and_then(Value::as_str) == Some(record_id))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => Ok(record),
        [] => Err(CommandError::not_found("Memory record")),
        _ => Err(CommandError::conflict("Memory record id is ambiguous")),
    }
}

fn incoming_blockers(records: &[Value], record_id: &str) -> Vec<String> {
    let mut blockers = records
        .iter()
        .filter_map(|candidate| {
            let candidate_id = candidate.get("id")?.as_str()?;
            if candidate_id == record_id {
                return None;
            }
            let linked = candidate
                .get("relationships")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|relation| {
                    relation.get("target").and_then(Value::as_str) == Some(record_id)
                        || relation.get("target_id").and_then(Value::as_str) == Some(record_id)
                })
                || candidate
                    .get("see_also")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|target| target.as_str() == Some(record_id))
                || candidate.get("revoked_by").and_then(Value::as_str) == Some(record_id);
            linked.then(|| candidate_id.to_owned())
        })
        .collect::<Vec<_>>();
    blockers.sort();
    blockers.dedup();
    blockers
}

#[tauri::command(rename_all = "snake_case")]
pub async fn library_memory_create(
    request: LibraryMemoryCreateRequest,
    state: State<'_, AppState>,
) -> Result<LibraryMemoryMutationDto, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::MemoryMutation)?;
    let (workspace, runtime) = workspace_and_runtime(state.inner(), &request.guru_id).await?;
    let kind = CanonicalMemoryKind::from_label(&request.draft.kind)
        .filter(|kind| matches!(kind, CanonicalMemoryKind::Wiki | CanonicalMemoryKind::Lens))
        .ok_or_else(|| CommandError::invalid("Only Wiki and Lens can be created in Library"))?;
    let records = workspace
        .knowledge_list(&runtime, None)
        .await
        .map_err(map_internal)?;
    let existing = records
        .as_array()
        .ok_or_else(|| CommandError::internal("Runtime list result is invalid"))?
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect::<BTreeSet<_>>();
    let record_id = allocate_library_id(kind.slug(), &request.draft.title, &existing);
    let (_, markdown) = draft_markdown(&request.draft, &record_id)?;
    let suffix = record_id
        .strip_prefix(&format!("{}:", kind.slug()))
        .unwrap_or(record_id.as_str());
    let target = MemoryChangeTarget {
        record_id: record_id.clone(),
        relative_path: format!("guruterminal/{}/{}.md", kind.slug(), slug(suffix)),
        before_markdown: String::new(),
        proposed_markdown: markdown,
    };
    apply_user_change(
        state.inner(),
        &request.guru_id,
        target,
        "Create user-authored Memory record.".into(),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn library_memory_update(
    request: LibraryMemoryUpdateRequest,
    state: State<'_, AppState>,
) -> Result<LibraryMemoryMutationDto, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::MemoryMutation)?;
    let (workspace, runtime) = workspace_and_runtime(state.inner(), &request.guru_id).await?;
    let records = workspace
        .knowledge_list(&runtime, None)
        .await
        .map_err(map_internal)?;
    let record = list_record(
        records
            .as_array()
            .ok_or_else(|| CommandError::internal("Runtime list result is invalid"))?,
        &request.record_id,
    )?;
    let kind = CanonicalMemoryKind::parse_record_id(&request.record_id)
        .map(|(kind, _)| kind)
        .filter(|kind| matches!(kind, CanonicalMemoryKind::Wiki | CanonicalMemoryKind::Lens))
        .ok_or_else(|| CommandError::invalid("Only Wiki and Lens can be edited in Library"))?;
    if CanonicalMemoryKind::from_label(&request.draft.kind) != Some(kind) {
        return Err(CommandError::invalid(
            "Memory kind and immutable record id do not match",
        ));
    }
    let read = workspace
        .knowledge_read(&runtime, &request.record_id, None)
        .await
        .map_err(map_internal)?;
    let before = read
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::internal("Runtime read result is invalid"))?
        .to_owned();
    let (_, markdown) = draft_markdown(&request.draft, &request.record_id)?;
    let path = record
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::internal("Runtime record path is invalid"))?
        .to_owned();
    let target = MemoryChangeTarget {
        record_id: request.record_id,
        relative_path: path,
        before_markdown: before,
        proposed_markdown: markdown,
    };
    apply_user_change(
        state.inner(),
        &request.guru_id,
        target,
        "Revise user-authored Memory record.".into(),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn library_memory_delete(
    request: LibraryMemoryDeleteRequest,
    state: State<'_, AppState>,
) -> Result<LibraryMemoryMutationDto, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::MemoryMutation)?;
    let (workspace, runtime) = workspace_and_runtime(state.inner(), &request.guru_id).await?;
    let listed = workspace
        .knowledge_list(&runtime, None)
        .await
        .map_err(map_internal)?;
    let records = listed
        .as_array()
        .ok_or_else(|| CommandError::internal("Runtime list result is invalid"))?;
    let record = list_record(records, &request.record_id)?;
    let blockers = incoming_blockers(records, &request.record_id);
    if !blockers.is_empty() {
        return Err(CommandError::conflict(format!(
            "Memory is referenced by: {}",
            blockers.join(", ")
        )));
    }
    let read = workspace
        .knowledge_read(&runtime, &request.record_id, None)
        .await
        .map_err(map_internal)?;
    let before = read
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::internal("Runtime read result is invalid"))?
        .to_owned();
    let path = record
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::internal("Runtime record path is invalid"))?
        .to_owned();
    let target = MemoryChangeTarget {
        record_id: request.record_id,
        relative_path: path,
        before_markdown: before,
        proposed_markdown: String::new(),
    };
    apply_user_change(
        state.inner(),
        &request.guru_id,
        target,
        "Delete user-selected Memory record.".into(),
    )
    .await
}

fn optional_id(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn revert_expected_digest(
    workspace: &crate::guru_root::BoundGuruRoot,
    relative_path: &str,
    request: &LibraryMemoryRevertRequest,
) -> Result<String, CommandError> {
    let from_markdown = request
        .expected_markdown
        .as_ref()
        .map(|markdown| sha256(markdown.as_bytes()));
    let from_commit = match optional_id(&request.commit_id) {
        Some(commit_id) => {
            match crate::memory_git::read_markdown_at_commit(
                workspace.path(),
                relative_path,
                commit_id,
            )
            .map_err(map_internal)?
            {
                Some(markdown) => Some(sha256(markdown.as_bytes())),
                None => {
                    return Err(CommandError::conflict(
                        "memory changed after the write was prepared",
                    ))
                }
            }
        }
        None => None,
    };
    match (from_markdown, from_commit) {
        (None, None) => Err(CommandError::invalid(
            "Memory revert requires the current record or its commit",
        )),
        (Some(left), Some(right)) if left != right => Err(CommandError::conflict(
            "memory changed after the write was prepared",
        )),
        (Some(digest), _) | (_, Some(digest)) => Ok(digest),
    }
}

pub(crate) async fn revert_memory_record(
    state: &AppState,
    guru_id: &str,
    record_id: &str,
    expected_sha256: &str,
) -> Result<LibraryMemoryMutationDto, CommandError> {
    let (workspace, runtime) = workspace_and_runtime(state, guru_id).await?;
    let listed = workspace
        .knowledge_list(&runtime, None)
        .await
        .map_err(map_internal)?;
    let records = listed
        .as_array()
        .ok_or_else(|| CommandError::internal("Runtime list result is invalid"))?;
    let record = list_record(records, record_id)?;
    CanonicalMemoryKind::parse_record_id(record_id)
        .map(|(kind, _)| kind)
        .filter(|kind| matches!(kind, CanonicalMemoryKind::Wiki | CanonicalMemoryKind::Lens))
        .ok_or_else(|| CommandError::invalid("Only Wiki and Lens can be reverted"))?;
    let path = record
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::internal("Runtime record path is invalid"))?
        .to_owned();
    let read = workspace
        .knowledge_read(&runtime, record_id, None)
        .await
        .map_err(map_internal)?;
    let before = read
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::internal("Runtime read result is invalid"))?
        .to_owned();
    if sha256(before.as_bytes()) != expected_sha256 {
        return Err(CommandError::conflict(
            "memory changed after the write was prepared",
        ));
    }
    let previous =
        crate::memory_git::read_previous_markdown(workspace.path(), &path).map_err(map_internal)?;
    let proposed = previous.map(|version| version.markdown).unwrap_or_default();
    if proposed == before {
        return Err(CommandError::invalid("Memory has no previous version"));
    }
    if proposed.is_empty() {
        let blockers = incoming_blockers(records, record_id);
        if !blockers.is_empty() {
            return Err(CommandError::conflict(format!(
                "Memory is referenced by: {}",
                blockers.join(", ")
            )));
        }
    }
    let target = MemoryChangeTarget {
        record_id: record_id.to_owned(),
        relative_path: path,
        before_markdown: before,
        proposed_markdown: proposed,
    };
    apply_user_change(
        state,
        guru_id,
        target,
        "Revert Memory record to the previous version.".into(),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn library_memory_revert(
    request: LibraryMemoryRevertRequest,
    state: State<'_, AppState>,
) -> Result<LibraryMemoryMutationDto, CommandError> {
    let _activity = state
        .maintenance
        .admit_kind(MaintenanceActivityKind::MemoryMutation)?;
    let (workspace, runtime) = workspace_and_runtime(state.inner(), &request.guru_id).await?;
    let listed = workspace
        .knowledge_list(&runtime, None)
        .await
        .map_err(map_internal)?;
    let record = list_record(
        listed
            .as_array()
            .ok_or_else(|| CommandError::internal("Runtime list result is invalid"))?,
        &request.record_id,
    )?;
    CanonicalMemoryKind::parse_record_id(&request.record_id)
        .map(|(kind, _)| kind)
        .filter(|kind| matches!(kind, CanonicalMemoryKind::Wiki | CanonicalMemoryKind::Lens))
        .ok_or_else(|| CommandError::invalid("Only Wiki and Lens can be reverted"))?;
    let path = record
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::internal("Runtime record path is invalid"))?;
    let expected_sha256 = revert_expected_digest(&workspace, path, &request)?;
    revert_memory_record(
        state.inner(),
        &request.guru_id,
        &request.record_id,
        &expected_sha256,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(kind: &str) -> LibraryDraftDto {
        LibraryDraftDto {
            kind: kind.into(),
            title: "Quality discipline".into(),
            summary: "A reusable quality discipline.".into(),
            as_of: "2026-08-19T00:00:00Z".into(),
            entities: vec!["ticker:TEST".into()],
            aliases: vec!["quality".into()],
            tags: vec!["fundamentals".into()],
            see_also: Vec::new(),
            scope: "Profitable operating companies.".into(),
            assumptions: "Reported accounts are comparable.".into(),
            counterexamples: "Early-stage businesses without stable margins.".into(),
            limits: "Not a standalone valuation method.".into(),
            invalidation_conditions: "Accounting definitions materially change.".into(),
            body_markdown: "# Thesis\n\nPrefer durable reinvestment economics.".into(),
        }
    }

    #[test]
    fn library_drafts_serialize_canonical_wiki_and_lens_only() {
        let (_, wiki) = draft_markdown(&draft("Wiki"), "wiki:quality").unwrap();
        assert!(wiki.contains("id: wiki:quality"));
        assert!(wiki.contains("ticker:TEST"));
        let (_, lens) = draft_markdown(&draft("Lens"), "lens:quality").unwrap();
        assert!(lens.contains("# Counterexamples"));
        assert!(lens.contains("# Invalidation conditions"));
        assert!(draft_markdown(&draft("Evidence"), "evidence:quality").is_err());
        assert!(draft_markdown(&draft("Decision"), "decision:quality").is_err());
    }

    #[test]
    fn lens_draft_requires_anti_overfitting_fields() {
        let mut value = draft("Lens");
        value.counterexamples.clear();
        assert!(draft_markdown(&value, "lens:quality").is_err());
    }

    #[test]
    fn draft_requires_markdown_body() {
        let mut value = draft("Wiki");
        value.body_markdown.clear();
        assert!(draft_markdown(&value, "wiki:quality").is_err());
    }

    #[test]
    fn library_draft_writes_as_of_with_seconds_not_fractional_time() {
        let mut value = draft("Wiki");
        value.as_of = "2026-08-24T16:32:01.234Z".into();
        let (_, markdown) = draft_markdown(&value, "wiki:quality").unwrap();
        assert!(markdown.contains("as_of: 2026-08-24T16:32:01Z\n"));
        assert!(!markdown.contains("16:32:01.234"));
    }

    #[test]
    fn incoming_relationships_see_also_and_revoked_by_block_delete() {
        let records = vec![
            serde_json::json!({"id": "lens:owner", "relationships": [{"relation": "uses", "target_id": "wiki:target"}]}),
            serde_json::json!({"id": "wiki:neighbor", "see_also": ["wiki:target"]}),
            serde_json::json!({"id": "wiki:revoked", "status": "revoked", "revoked_by": "wiki:target"}),
            serde_json::json!({"id": "wiki:target"}),
        ];
        assert_eq!(
            incoming_blockers(&records, "wiki:target"),
            vec!["lens:owner", "wiki:neighbor", "wiki:revoked"]
        );
    }
}
