use super::*;

pub fn check(root: &Path) -> KnowledgeCheck {
    let docs = catalog_local_unchecked(root);
    check_documents(root, &docs)
}

pub(super) fn check_documents(root: &Path, docs: &[Document]) -> KnowledgeCheck {
    check_documents_with_layout(docs, layout_issues(root))
}

pub(super) fn check_documents_with_layout(
    docs: &[Document],
    mut issues: Vec<KnowledgeIssue>,
) -> KnowledgeCheck {
    let mut ids = BTreeMap::<String, String>::new();
    for doc in docs {
        for issue in validate_document(doc) {
            issues.push(issue);
        }
        if !doc.id.is_empty() {
            if let Some(first) = ids.insert(doc.id.clone(), doc.path.clone()) {
                issues.push(KnowledgeIssue {
                    path: doc.path.clone(),
                    field: "id".into(),
                    message: format!("duplicates {first}"),
                });
            }
        }
    }
    let kinds = docs
        .iter()
        .filter(|doc| !doc.id.is_empty())
        .map(|doc| (doc.id.as_str(), doc.kind.as_str()))
        .collect::<BTreeMap<_, _>>();
    for doc in docs {
        if let Some(revoked_by) = doc
            .revoked_by
            .as_deref()
            .filter(|target| !target.trim().is_empty())
        {
            if revoked_by == doc.id {
                issues.push(issue(doc, "revoked_by", "must not reference itself"));
            } else if !kinds.contains_key(revoked_by) {
                issues.push(issue(
                    doc,
                    "revoked_by",
                    &format!("target does not exist: {revoked_by}"),
                ));
            }
        }
        for relationship in &doc.relationships {
            let Some(target_kind) = kinds.get(relationship.target.as_str()) else {
                issues.push(issue(
                    doc,
                    &relationship.kind,
                    &format!("target does not exist: {}", relationship.target),
                ));
                continue;
            };
            if !relationship_target_allowed(&doc.kind, &relationship.kind, target_kind) {
                issues.push(issue(
                    doc,
                    &relationship.kind,
                    &format!(
                        "target {} has disallowed kind {}",
                        relationship.target, target_kind
                    ),
                ));
            }
        }
        for target in &doc.see_also {
            let Some(target_kind) = kinds.get(target.as_str()) else {
                issues.push(issue(
                    doc,
                    "see_also",
                    &format!("target does not exist: {target}"),
                ));
                continue;
            };
            if *target_kind != "wiki" {
                issues.push(issue(
                    doc,
                    "see_also",
                    &format!("target {target} has disallowed kind {target_kind}"),
                ));
            }
        }
    }
    KnowledgeCheck {
        valid: issues.is_empty(),
        documents: docs.len(),
        errors: issues,
    }
}

pub(super) fn document_id_counts(documents: &[Document]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for document in documents {
        *counts.entry(document.id.clone()).or_default() += 1;
    }
    counts
}

pub fn health(root: &Path, kind: Option<&str>) -> Result<KnowledgeHealth, String> {
    if let Some(kind) = kind {
        if CanonicalMemoryKind::from_slug(kind).is_none() {
            return Err(format!("unknown knowledge kind: {kind}"));
        }
    }
    let docs = catalog_local(root);
    Ok(health_documents(&docs, kind))
}

pub(super) fn health_documents(docs: &[Document], kind: Option<&str>) -> KnowledgeHealth {
    let kinds = CanonicalMemoryKind::ALL
        .iter()
        .filter(|candidate| kind.is_none_or(|wanted| wanted == candidate.slug()))
        .map(|kind| kind_health(docs, kind.slug()))
        .collect();
    KnowledgeHealth { kinds }
}

pub(super) fn kind_health(docs: &[Document], kind: &str) -> KindHealth {
    let documents = docs
        .iter()
        .filter(|document| document.kind == kind)
        .collect::<Vec<_>>();
    let mut folder_names = BTreeSet::new();
    let mut max_depth = 0;
    let mut advisories = Vec::new();
    let mut deep = Vec::new();
    let mut large = Vec::new();
    let mut excessive_links = Vec::new();

    for document in &documents {
        let folders = subject_folders(document);
        max_depth = max_depth.max(folders.len());
        for depth in 1..=folders.len() {
            folder_names.insert(folders[..depth].join("/"));
        }
        if folders.len() > 2 {
            deep.push(document_key(document));
        }
        if document.content.chars().count() > 12_000 || split_sections(&document.body).len() > 8 {
            large.push(document_key(document));
        }
        if kind == "wiki" && document.see_also.len() > 5 {
            excessive_links.push(document_key(document));
        }
    }
    add_advisory(&mut advisories, "deep_folder", deep);
    add_advisory(&mut advisories, "large_document", large);
    add_duplicate_advisories(
        &mut advisories,
        "duplicate_title",
        documents
            .iter()
            .filter_map(|document| {
                let key = normalized_health_key(&document.title);
                (!key.is_empty()).then(|| (key, document_key(document)))
            })
            .collect(),
    );
    add_name_collision_advisories(&mut advisories, &documents);
    add_advisory(&mut advisories, "excessive_see_also", excessive_links);
    if kind == "evidence" {
        add_duplicate_advisories(
            &mut advisories,
            "duplicate_evidence_candidate",
            evidence_signatures(&documents),
        );
    }
    if kind == "decision" {
        add_decision_fork_advisories(&mut advisories, &documents);
    }
    advisories.sort();

    KindHealth {
        kind: kind.to_owned(),
        documents: documents.len(),
        folders: folder_names.len(),
        max_depth,
        review_band: review_band(kind, &documents).to_owned(),
        advisories,
    }
}

pub(super) fn subject_folders(doc: &Document) -> Vec<String> {
    let components = doc.path.split('/').collect::<Vec<_>>();
    let start = components
        .windows(2)
        .position(|window| window == ["guruterminal", doc.kind.as_str()])
        .map_or(components.len(), |index| index + 2);
    components
        .get(start..components.len().saturating_sub(1))
        .unwrap_or_default()
        .iter()
        .map(|component| (*component).to_owned())
        .collect()
}

pub(super) fn document_key(doc: &Document) -> String {
    if doc.id.is_empty() {
        doc.path.clone()
    } else {
        doc.id.clone()
    }
}

pub(super) fn normalized_health_key(value: &str) -> String {
    normalize_search_text(value)
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

pub(super) fn add_advisory(advisories: &mut Vec<HealthAdvisory>, code: &str, mut ids: Vec<String>) {
    ids.sort();
    ids.dedup();
    if !ids.is_empty() {
        advisories.push(HealthAdvisory {
            code: code.to_owned(),
            ids,
        });
    }
}

pub(super) fn add_duplicate_advisories(
    advisories: &mut Vec<HealthAdvisory>,
    code: &str,
    keyed_ids: Vec<(String, String)>,
) {
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    for (key, id) in keyed_ids {
        groups.entry(key).or_default().push(id);
    }
    for ids in groups.into_values() {
        let unique = ids.into_iter().collect::<BTreeSet<_>>();
        if unique.len() > 1 {
            add_advisory(advisories, code, unique.into_iter().collect());
        }
    }
}

pub(super) fn add_name_collision_advisories(
    advisories: &mut Vec<HealthAdvisory>,
    documents: &[&Document],
) {
    let mut groups = BTreeMap::<String, Vec<(String, bool)>>::new();
    for document in documents {
        let id = document_key(document);
        let title_key = normalized_health_key(&document.title);
        if !title_key.is_empty() {
            groups
                .entry(title_key)
                .or_default()
                .push((id.clone(), false));
        }
        for alias in &document.aliases {
            let alias_key = normalized_health_key(alias);
            if !alias_key.is_empty() {
                groups
                    .entry(alias_key)
                    .or_default()
                    .push((id.clone(), true));
            }
        }
    }
    for entries in groups.into_values() {
        let ids = entries
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>();
        if ids.len() > 1 && entries.iter().any(|(_, is_alias)| *is_alias) {
            add_advisory(advisories, "duplicate_alias", ids.into_iter().collect());
        }
    }
}

pub(super) fn evidence_signatures(documents: &[&Document]) -> Vec<(String, String)> {
    let mut signatures = Vec::new();
    for document in documents {
        let period = document
            .period
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("");
        let title = normalize_search_text(&document.title);
        if !title.is_empty() && !period.is_empty() {
            signatures.push((format!("{title}\u{0}{period}"), document_key(document)));
        }
        let Some(source) = document.source.as_deref().filter(|value| !value.is_empty()) else {
            for entity in &document.entities {
                signatures.push((
                    format!(
                        "{}\u{0}{}\u{0}{}",
                        title,
                        normalize_search_text(period),
                        normalize_search_text(entity)
                    ),
                    document_key(document),
                ));
            }
            continue;
        };
        let Some(period) = document.period.as_deref().filter(|value| !value.is_empty()) else {
            continue;
        };
        for entity in &document.entities {
            signatures.push((
                format!(
                    "{}\u{0}{}\u{0}{}",
                    normalize_search_text(source),
                    normalize_search_text(period),
                    normalize_search_text(entity)
                ),
                document_key(document),
            ));
        }
    }
    signatures
}

pub(super) fn add_decision_fork_advisories(
    advisories: &mut Vec<HealthAdvisory>,
    documents: &[&Document],
) {
    let mut revisions = BTreeMap::<String, Vec<String>>::new();
    for document in documents {
        for relationship in &document.relationships {
            if matches!(relationship.kind.as_str(), "updates" | "contradicts") {
                revisions
                    .entry(relationship.target.clone())
                    .or_default()
                    .push(document_key(document));
            }
        }
    }
    for (target, revisions) in revisions {
        let revisions = revisions.into_iter().collect::<BTreeSet<_>>();
        if revisions.len() > 1 {
            let mut ids = vec![target];
            ids.extend(revisions);
            add_advisory(advisories, "decision_revision_fork", ids);
        }
    }
}

pub(super) fn review_band(kind: &str, documents: &[&Document]) -> &'static str {
    let count = documents.len();
    let (organize, curate, scale, entity_threshold) = match kind {
        "wiki" => (25, 75, 200, None),
        "lens" => (15, 30, 100, None),
        "evidence" => (200, 500, 1_000, Some(50)),
        "decision" => (50, 150, 300, Some(10)),
        _ => (usize::MAX, usize::MAX, usize::MAX, None),
    };
    if count >= scale {
        "scale"
    } else if count >= curate {
        "curate"
    } else if count >= organize
        || entity_threshold.is_some_and(|threshold| {
            let mut counts = BTreeMap::<String, usize>::new();
            for document in documents {
                for entity in document
                    .entities
                    .iter()
                    .map(|entity| entity.to_ascii_lowercase())
                    .collect::<BTreeSet<_>>()
                {
                    *counts.entry(entity).or_default() += 1;
                }
            }
            counts.into_values().any(|count| count >= threshold)
        })
    {
        "organize"
    } else {
        "normal"
    }
}
