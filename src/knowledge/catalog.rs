use super::*;
use sha2::{Digest, Sha256};

pub(super) struct CatalogCapture {
    pub revision: String,
    pub documents: Vec<Document>,
    pub layout_issues: Vec<KnowledgeIssue>,
}

fn infer_kind(path: &Path, root: &Path) -> String {
    let base = root.join("guruterminal");
    path.strip_prefix(&base)
        .ok()
        .and_then(|p| p.components().next())
        .and_then(|p| CanonicalMemoryKind::from_slug(p.as_os_str().to_str()?))
        .map(CanonicalMemoryKind::slug)
        .expect("scanned markdown lives under a canonical Memory kind folder")
        .to_owned()
}

pub fn catalog_local(root: &Path) -> Vec<Document> {
    catalog_local_unchecked(root)
        .into_iter()
        .filter(document_is_valid)
        .collect()
}

pub(super) fn catalog_local_unchecked(root: &Path) -> Vec<Document> {
    let mut docs: Vec<_> = local_markdown_files(root)
        .into_iter()
        .map(|p| load_local(root, &p))
        .collect();
    docs.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    docs
}

/// Reads each Markdown record once and derives the desktop-compatible tree
/// revision from those exact bytes. The surrounding Runtime boundary scan is
/// still authoritative for file count, byte limits, and unsupported entries.
pub(super) fn capture_local_catalog(root: &Path) -> Result<CatalogCapture, String> {
    let memory_root = root.join("guruterminal");
    let mut tree = Sha256::new();
    let mut documents = Vec::new();
    let mut layout_issues = Vec::new();
    let mut paths = Vec::new();
    for kind in CanonicalMemoryKind::ALL {
        let kind = kind.slug();
        let directory = memory_root.join(kind);
        if let Ok(metadata) = fs::symlink_metadata(&directory) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                layout_issues.push(KnowledgeIssue {
                    path: display_path(&directory, root),
                    field: "layout".into(),
                    message: format!("{kind} collection must be a non-symlink directory"),
                });
                continue;
            }
        }
        collect_context_paths(root, kind, &directory, &mut paths, &mut layout_issues);
    }
    paths.sort();
    for path in paths {
        let bytes = fs::read(&path)
            .map_err(|error| format!("cannot capture {}: {error}", display_path(&path, root)))?;
        let relative = path
            .strip_prefix(&memory_root)
            .map_err(|_| "knowledge record is outside the Memory tree".to_string())?;
        let relative = relative
            .to_str()
            .ok_or_else(|| "knowledge record path is not UTF-8".to_string())?
            .replace(std::path::MAIN_SEPARATOR, "/");
        tree.update((relative.len() as u64).to_be_bytes());
        tree.update(relative.as_bytes());
        tree.update((bytes.len() as u64).to_be_bytes());
        tree.update(&bytes);
        documents.push(match String::from_utf8(bytes) {
            Ok(content) => document_from_content(root, &path, content),
            Err(error) => unreadable_document(root, &path, error.to_string()),
        });
    }
    documents.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    Ok(CatalogCapture {
        revision: hex::encode(tree.finalize()),
        documents,
        layout_issues,
    })
}

fn collect_context_paths(
    root: &Path,
    kind: &str,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
    issues: &mut Vec<KnowledgeIssue>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let relative = display_path(&path, root);
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            issues.push(KnowledgeIssue {
                path: relative,
                field: "layout".into(),
                message: format!("{kind} elements and directories must not be symlinks"),
            });
        } else if metadata.is_dir() {
            collect_context_paths(root, kind, &path, paths, issues);
        } else if metadata.is_file() && is_markdown(&path) {
            paths.push(path);
        } else {
            issues.push(KnowledgeIssue {
                path: relative,
                field: "layout".into(),
                message: format!("{kind} elements must be non-symlink Markdown files"),
            });
        }
    }
}
pub fn read(root: &Path, id: &str, section: Option<&str>) -> Result<ReadResult, String> {
    read_internal(root, id, section, None)
}

pub(super) fn read_internal(
    root: &Path,
    id: &str,
    section: Option<&str>,
    workers: Option<usize>,
) -> Result<ReadResult, String> {
    let document = find_document_by_id(root, id, workers)?
        .ok_or_else(|| format!("knowledge document not found: {id}"))?;
    let selected = if let Some(wanted) = section {
        let wanted = wanted.to_ascii_lowercase();
        let matches: Vec<_> = split_sections(&document.body)
            .into_iter()
            .filter(|item| {
                item.heading.eq_ignore_ascii_case(&wanted)
                    || item.heading_path.join(" / ").eq_ignore_ascii_case(&wanted)
            })
            .collect();
        match matches.len() {
            0 => return Err(format!("section not found in {id}: {section:?}")),
            1 => matches.into_iter().next(),
            _ => return Err(format!("section is ambiguous in {id}: {section:?}")),
        }
    } else {
        None
    };
    Ok(ReadResult {
        document,
        section: selected,
    })
}

pub(super) fn find_document_by_id(
    root: &Path,
    id: &str,
    workers: Option<usize>,
) -> Result<Option<Document>, String> {
    let Some(kind) = canonical_id_kind(id) else {
        return Ok(None);
    };
    let paths = local_markdown_files_for_kinds(root, &[kind.to_owned()]);
    find_document_in_paths(root, &paths, id, workers)
}

pub(super) fn find_document_in_paths(
    root: &Path,
    paths: &[PathBuf],
    id: &str,
    workers: Option<usize>,
) -> Result<Option<Document>, String> {
    let chunks = if let Some(workers) = workers {
        scan_chunks_with_workers(paths, workers, "read", |chunk| {
            find_document_in_chunk(root, chunk, id)
        })?
    } else {
        scan_chunks(paths, "read", |chunk| {
            find_document_in_chunk(root, chunk, id)
        })?
    };
    let matches = chunks.into_iter().flatten().collect::<Vec<_>>();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => Err(format!("knowledge document ID is ambiguous: {id}")),
    }
}

pub(super) fn find_document_in_chunk(root: &Path, paths: &[PathBuf], id: &str) -> Vec<Document> {
    paths
        .iter()
        .map(|path| load_local(root, path))
        .filter(|document| document.id == id && document_is_valid(document))
        .collect()
}

pub(super) fn split_sections(body: &str) -> Vec<Section> {
    let mut out = Vec::new();
    let mut stack: Vec<(usize, String)> = vec![];
    let mut heading = String::new();
    let mut path = vec![];
    let mut buf = Vec::new();
    let flush = |out: &mut Vec<Section>, heading: &str, path: &[String], buf: &mut Vec<&str>| {
        let text = buf.join("\n").trim().to_owned();
        if !text.is_empty() || !heading.is_empty() || out.is_empty() {
            out.push(Section {
                heading: heading.into(),
                heading_path: path.into(),
                text,
            });
        }
        buf.clear();
    };
    for line in body.lines() {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && line.as_bytes().get(hashes) == Some(&b' ') {
            flush(&mut out, &heading, &path, &mut buf);
            while stack.last().is_some_and(|(level, _)| *level >= hashes) {
                stack.pop();
            }
            let name = line[hashes + 1..].trim().to_owned();
            stack.push((hashes, name.clone()));
            heading = name;
            path = stack.iter().map(|(_, n)| n.clone()).collect();
        } else {
            buf.push(line);
        }
    }
    flush(&mut out, &heading, &path, &mut buf);
    out.into_iter()
        .filter(|s| !s.text.is_empty() || !s.heading.is_empty())
        .collect()
}

pub(super) fn is_rfc3339_seconds(value: &str) -> bool {
    static RFC3339_SECONDS: OnceLock<Regex> = OnceLock::new();
    RFC3339_SECONDS
        .get_or_init(|| {
            Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2})$")
                .expect("RFC 3339 seconds regex is valid")
        })
        .is_match(value)
        && DateTime::parse_from_rfc3339(value).is_ok()
}
pub(super) fn document_is_valid(doc: &Document) -> bool {
    validate_document(doc).is_empty()
}

pub(super) fn load_local(root: &Path, path: &Path) -> Document {
    match load_document(root, path) {
        Ok(doc) => doc,
        Err(error) => unreadable_document(root, path, error.to_string()),
    }
}
pub(super) fn load_document(root: &Path, path: &Path) -> Result<Document, std::io::Error> {
    let content = fs::read_to_string(path)?;
    Ok(document_from_content(root, path, content))
}

fn document_from_content(root: &Path, path: &Path, content: String) -> Document {
    let parsed = parse_frontmatter(&content);
    let relationships = RELATIONSHIP_TYPES
        .iter()
        .flat_map(|kind| {
            parsed
                .metadata
                .lists
                .get(*kind)
                .into_iter()
                .flatten()
                .map(move |target| Relationship {
                    kind: (*kind).into(),
                    target: target.clone(),
                })
        })
        .collect();
    let m = parsed.metadata;
    Document {
        id: m.scalar.get("id").cloned().unwrap_or_default(),
        kind: infer_kind(path, root),
        title: m.scalar.get("title").cloned().unwrap_or_default(),
        summary: m.scalar.get("summary").cloned().unwrap_or_default(),
        as_of: m.scalar.get("as_of").cloned().unwrap_or_default(),
        path: display_path(path, root),
        entities: m.lists.get("entities").cloned().unwrap_or_default(),
        aliases: m.lists.get("aliases").cloned().unwrap_or_default(),
        tags: m.lists.get("tags").cloned().unwrap_or_default(),
        see_also: m.lists.get("see_also").cloned().unwrap_or_default(),
        source: m.scalar.get("source").cloned(),
        period: m.scalar.get("period").cloned(),
        status: m.scalar.get("status").cloned(),
        revoked_by: m.scalar.get("revoked_by").cloned(),
        relationships,
        content,
        body: parsed.body,
        read_error: parsed.error,
        declared_fields: m.declared_fields,
        duplicate_fields: m.duplicate_fields,
    }
}
pub(super) fn unreadable_document(root: &Path, path: &Path, message: String) -> Document {
    Document {
        id: String::new(),
        kind: infer_kind(path, root),
        title: String::new(),
        summary: String::new(),
        as_of: String::new(),
        path: display_path(path, root),
        entities: vec![],
        aliases: vec![],
        tags: vec![],
        see_also: vec![],
        source: None,
        period: None,
        status: None,
        revoked_by: None,
        relationships: vec![],
        content: String::new(),
        body: String::new(),
        read_error: Some(format!("cannot read Markdown: {message}")),
        declared_fields: BTreeSet::new(),
        duplicate_fields: BTreeSet::new(),
    }
}
pub(super) fn validate_document(doc: &Document) -> Vec<KnowledgeIssue> {
    let mut out = vec![];
    if let Some(error) = &doc.read_error {
        out.push(issue(doc, "frontmatter", error));
    }
    for field in &doc.duplicate_fields {
        out.push(issue(
            doc,
            field,
            "frontmatter field must be declared only once",
        ));
    }
    for (field, value) in [
        ("id", &doc.id),
        ("title", &doc.title),
        ("summary", &doc.summary),
        ("as_of", &doc.as_of),
    ] {
        if value.trim().is_empty() {
            out.push(issue(doc, field, "is required"));
        }
    }
    if !doc.id.is_empty() && !canonical_id_matches_kind(&doc.id, &doc.kind) {
        out.push(issue(
            doc,
            "id",
            &format!(
                "must use a canonical {}: ID matching its collection",
                doc.kind
            ),
        ));
    }
    if !doc.as_of.is_empty() && !is_rfc3339_seconds(&doc.as_of) {
        out.push(issue(
            doc,
            "as_of",
            "must be RFC3339 with seconds and timezone",
        ));
    }
    if doc.kind == "evidence" {
        if doc
            .source
            .as_deref()
            .is_some_and(|source| source.trim().is_empty())
        {
            out.push(issue(
                doc,
                "source",
                "evidence source must be a non-empty locator when declared",
            ));
        }
        if doc.body.trim().is_empty() {
            out.push(issue(doc, "body", "Evidence requires a non-empty body"));
        }
        let has_source = doc
            .source
            .as_deref()
            .is_some_and(|source| !source.trim().is_empty());
        let has_sources_section = split_sections(&doc.body).iter().any(|section| {
            section.heading.eq_ignore_ascii_case("Sources") && !section.text.trim().is_empty()
        });
        if !has_source && !has_sources_section {
            out.push(issue(
                doc,
                "body",
                "Evidence requires a non-empty source or # Sources section",
            ));
        }
    }
    if doc.declared_fields.contains("see_also") && doc.kind != "wiki" {
        out.push(issue(
            doc,
            "see_also",
            "see_also is permitted only for Wiki memory",
        ));
    }
    if (doc.declared_fields.contains("status") || doc.declared_fields.contains("revoked_by"))
        && !matches!(doc.kind.as_str(), "wiki" | "lens")
    {
        out.push(issue(
            doc,
            "status",
            "status and revoked_by are permitted only for Wiki and Lens",
        ));
    }
    if let Some(status) = &doc.status {
        if !matches!(status.as_str(), "active" | "revoked") {
            out.push(issue(doc, "status", "must be active or revoked"));
        }
        if status == "revoked"
            && doc
                .revoked_by
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            out.push(issue(
                doc,
                "revoked_by",
                "revoked Wiki or Lens requires revoked_by",
            ));
        }
        if status != "revoked"
            && doc
                .revoked_by
                .as_deref()
                .is_some_and(|value| !value.is_empty())
        {
            out.push(issue(
                doc,
                "revoked_by",
                "revoked_by is permitted only when status is revoked",
            ));
        }
    } else if doc
        .revoked_by
        .as_deref()
        .is_some_and(|value| !value.is_empty())
    {
        out.push(issue(
            doc,
            "revoked_by",
            "revoked_by is permitted only when status is revoked",
        ));
    }
    if let Some(revoked_by) = &doc.revoked_by {
        if !revoked_by.trim().is_empty() && canonical_id_kind(revoked_by).is_none() {
            out.push(issue(
                doc,
                "revoked_by",
                "must use a canonical Memory record ID",
            ));
        }
    }
    let mut seen = BTreeSet::new();
    for target in &doc.see_also {
        if target == &doc.id {
            out.push(issue(doc, "see_also", "must not reference itself"));
        }
        if !target.starts_with("wiki:") {
            out.push(issue(
                doc,
                "see_also",
                &format!("target must use a wiki: ID: {target}"),
            ));
        }
        if !seen.insert(target) {
            out.push(issue(
                doc,
                "see_also",
                &format!("duplicate target: {target}"),
            ));
        }
    }
    for relationship in RELATIONSHIP_TYPES {
        if doc.declared_fields.contains(*relationship)
            && !relationship_field_allowed(&doc.kind, relationship)
        {
            out.push(issue(
                doc,
                relationship,
                "relationship is not permitted for this memory kind",
            ));
        }
    }
    out
}

pub(super) fn canonical_id_kind(id: &str) -> Option<&str> {
    let (kind, _) = id.split_once(':')?;
    canonical_id_matches_kind(id, kind).then_some(kind)
}

pub(super) fn canonical_id_matches_kind(id: &str, expected_kind: &str) -> bool {
    CanonicalMemoryKind::parse_record_id(id).is_some_and(|(kind, _)| kind.slug() == expected_kind)
}
pub(super) fn issue(doc: &Document, field: &str, message: &str) -> KnowledgeIssue {
    KnowledgeIssue {
        path: doc.path.clone(),
        field: field.into(),
        message: message.into(),
    }
}

pub(super) fn relationship_field_allowed(source: &str, relationship: &str) -> bool {
    match source {
        "evidence" => matches!(relationship, "updates" | "contradicts"),
        "decision" => matches!(
            relationship,
            "uses" | "supports" | "updates" | "contradicts"
        ),
        _ => false,
    }
}

pub(super) fn relationship_target_allowed(source: &str, relationship: &str, target: &str) -> bool {
    match (source, relationship) {
        ("evidence", "updates" | "contradicts") => target == "evidence",
        ("decision", "uses") => matches!(target, "wiki" | "lens"),
        ("decision", "supports") => target == "evidence",
        ("decision", "updates" | "contradicts") => target == "decision",
        _ => false,
    }
}

pub(super) fn layout_issues(root: &Path) -> Vec<KnowledgeIssue> {
    let mut issues = Vec::new();
    for kind in CanonicalMemoryKind::ALL {
        let kind = kind.slug();
        let directory = root.join("guruterminal").join(kind);
        if let Ok(metadata) = fs::symlink_metadata(&directory) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                issues.push(KnowledgeIssue {
                    path: display_path(&directory, root),
                    field: "layout".into(),
                    message: format!("{kind} collection must be a non-symlink directory"),
                });
                continue;
            }
        }
        collect_layout_issues(root, kind, &directory, &mut issues);
    }
    issues
}

pub(super) fn collect_layout_issues(
    root: &Path,
    kind: &str,
    directory: &Path,
    issues: &mut Vec<KnowledgeIssue>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let relative = display_path(&path, root);
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            issues.push(KnowledgeIssue {
                path: relative,
                field: "layout".into(),
                message: format!("{kind} elements and directories must not be symlinks"),
            });
        } else if metadata.is_dir() {
            collect_layout_issues(root, kind, &path, issues);
        } else if !metadata.is_file() || !is_markdown(&path) {
            issues.push(KnowledgeIssue {
                path: relative,
                field: "layout".into(),
                message: format!("{kind} elements must be non-symlink Markdown files"),
            });
        }
    }
}

pub(super) fn local_markdown_files(root: &Path) -> Vec<PathBuf> {
    local_markdown_files_for_kinds(root, &[])
}

pub(super) fn local_markdown_files_for_kinds(root: &Path, kinds: &[String]) -> Vec<PathBuf> {
    let mut out = vec![];
    let directories = if kinds.is_empty() {
        CanonicalMemoryKind::ALL
            .iter()
            .map(|kind| kind.slug())
            .collect::<BTreeSet<_>>()
    } else {
        kinds.iter().map(String::as_str).collect::<BTreeSet<_>>()
    };
    for dir in directories {
        collect_markdown_files(&root.join("guruterminal").join(dir), &mut out);
    }
    out.sort();
    out
}

pub(super) fn collect_markdown_files(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(metadata) = fs::symlink_metadata(directory) else {
        return;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_markdown_files(&path, out);
        } else if metadata.is_file() && is_markdown(&path) {
            out.push(path);
        }
    }
}

pub(super) fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("md")
}
pub(super) fn display_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
