use super::*;
use std::fs;

pub(in crate::commands) fn bound_root(workspace: &Path) -> BoundGuruRoot {
    BoundGuruRoot::open_unbound(workspace.canonicalize().unwrap()).unwrap()
}

#[cfg(unix)]
pub(in crate::commands) fn initialized_workspace(workspace: &Path, marker: &str) {
    fs::create_dir_all(workspace.join(".guruterminal")).unwrap();
    for kind in ["wiki", "lens", "evidence", "decision"] {
        fs::create_dir_all(workspace.join("guruterminal").join(kind)).unwrap();
    }
    fs::write(
        workspace.join(".guruterminal/workspace.json"),
        "{\n  \"schema_version\": 1\n}\n",
    )
    .unwrap();
    fs::write(workspace.join("runtime-marker"), format!("{marker}\n")).unwrap();
}

pub(in crate::commands) fn profile(id: &str, workspace: &Path, timestamp: i64) -> GuruProfile {
    let workspace = workspace.canonicalize().unwrap();
    let root = BoundGuruRoot::open_unbound(workspace.clone()).unwrap();
    GuruProfile {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        storage_kind: GuruStorageKind::Managed,
        memory_root: workspace.to_string_lossy().into_owned(),
        root_filesystem_identity: root.identity(),
        last_model_profile_id: None,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    }
}

pub(in crate::commands) fn seed_profile(store: &dyn GuruTerminalStore, profile: &GuruProfile) {
    store.create_guru(profile).unwrap();
}

pub(in crate::commands) fn wiki_markdown(id: &str, title: &str) -> String {
    format!(
        "---\nid: {id}\ntitle: {title}\nsummary: {title}\nas_of: 2026-08-19T00:00:00Z\n---\n\n# {title}\n\nDurable fact from current research.\n"
    )
}

pub(in crate::commands) fn lens_markdown(id: &str, title: &str) -> String {
    format!(
        "---\nid: {id}\ntitle: {title}\nsummary: {title}\nas_of: 2026-08-19T00:00:00Z\n---\n\n# Scope\n\nApplies to this research-learn test.\n\n# Assumptions\n\nCurrent-run sources are representative.\n\n# Counterexamples\n\nOne-off anecdotes.\n\n# Limits\n\nDoes not generalize outside the named industry.\n\n# Invalidation conditions\n\nA later sourced observation reverses the claim.\n"
    )
}

#[cfg(unix)]
pub(in crate::commands) fn write_knowledge_runtime(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(
        path,
        r#"#!/usr/bin/env python3
import json, sys
from pathlib import Path

def parse():
    args = sys.argv[1:]
    workspace, kind, section, positionals, include_revoked, as_of = ".", None, None, [], False, None
    i = 0
    while i < len(args):
        item = args[i]
        if item == "--workspace":
            workspace = args[i + 1]; i += 2
        elif item == "--kind":
            kind = args[i + 1]; i += 2
        elif item == "--section":
            section = args[i + 1]; i += 2
        elif item == "--limit":
            i += 2
        elif item == "--as-of":
            as_of = args[i + 1]; i += 2
        elif item in ("--json", "--candidates", "--include-revoked"):
            if item == "--include-revoked":
                include_revoked = True
            i += 1
        else:
            positionals.append(item); i += 1
    return positionals, workspace, kind, section, include_revoked, as_of

def unquote(value):
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        return value[1:-1].strip()
    return value

def frontmatter(text):
    if not text.startswith("---"):
        return {}, {}, text
    parts = text.split("---", 2)
    if len(parts) < 3:
        return {}, {}, text
    meta, lists, current = {}, {}, None
    for line in parts[1].splitlines():
        stripped = line.strip()
        if stripped.startswith("- ") and current:
            lists.setdefault(current, []).append(unquote(stripped[2:]))
            continue
        if ":" in line and not stripped.startswith("-"):
            key, value = line.split(":", 1)
            key, value = key.strip(), value.strip()
            current = None
            if not value:
                current = key
                lists[key] = []
            elif value.startswith("[") and value.endswith("]"):
                lists[key] = [unquote(item) for item in value[1:-1].split(",") if item.strip()]
            else:
                meta[key] = unquote(value)
    return meta, lists, parts[2]

def docs(root, kind=None):
    out = []
    kinds = [kind] if kind else ["wiki", "lens", "evidence", "decision"]
    base = Path(root) / "guruterminal"
    for slug in kinds:
        folder = base / slug
        if not folder.is_dir():
            continue
        for path in folder.rglob("*.md"):
            text = path.read_text()
            meta, lists, body = frontmatter(text)
            record_id = meta.get("id")
            if not record_id:
                continue
            rel = str(path.relative_to(root)).replace("\\", "/")
            out.append({
                "id": record_id,
                "kind": slug,
                "title": meta.get("title", ""),
                "summary": meta.get("summary", ""),
                "as_of": meta.get("as_of", ""),
                "path": rel,
                "status": meta.get("status") or "active",
                "revoked_by": meta.get("revoked_by"),
                "entities": lists.get("entities", []),
                "aliases": lists.get("aliases", []),
                "tags": lists.get("tags", []),
                "see_also": lists.get("see_also", []),
                "source": meta.get("source"),
                "period": meta.get("period"),
                "relationships": [],
                "text": body,
                "content": text,
            })
    return out

positionals, workspace, kind, section, include_revoked, as_of = parse()
command = positionals[0] if positionals else ""
action = positionals[1] if len(positionals) > 1 else ""
if command == "knowledge" and action == "check":
    print(json.dumps({"valid": True, "documents": 0, "errors": []}))
elif command == "knowledge" and action == "health":
    print(json.dumps({"kinds": []}))
elif command == "knowledge" and action == "list":
    records = docs(workspace, kind)
    for record in records:
        record.pop("text", None)
        record.pop("content", None)
    print(json.dumps(records))
elif command == "knowledge" and action == "search":
    query = positionals[2].lower() if len(positionals) > 2 else ""
    terms = [term for term in query.split() if term]
    hits = []
    for record in docs(workspace, kind):
        if (
            not include_revoked
            and record.get("status") == "revoked"
            and record["kind"] in ("wiki", "lens")
        ):
            continue
        record_day = (record.get("as_of") or "")[:10]
        if as_of and record_day and record_day > as_of:
            continue
        blob = " ".join([
            record["id"], record["title"], record["summary"], record.get("text", ""),
            " ".join(record.get("aliases") or []),
            " ".join(record.get("entities") or []),
            " ".join(record.get("tags") or []),
        ]).lower()
        matched = sum(1 for term in terms if term in blob)
        if terms and ((len(terms) == 1 and matched < 1) or (len(terms) > 1 and matched < 2)):
            continue
        hits.append({
            "id": record["id"],
            "kind": record["kind"],
            "title": record["title"],
            "summary": record["summary"],
            "as_of": record["as_of"],
            "path": record["path"],
            "section": "",
            "heading_path": [],
            "entities": record.get("entities") or [],
            "aliases": record.get("aliases") or [],
            "period": record.get("period"),
            "relationships": [],
            "score": 100,
            "text": record.get("text", ""),
            "status": record.get("status"),
        })
    print(json.dumps(hits[:20]))
elif command == "knowledge" and action == "read":
    record_id = positionals[2] if len(positionals) > 2 else ""
    for record in docs(workspace):
        if record["id"] == record_id:
            content = record.pop("content")
            record.pop("text", None)
            print(json.dumps({"document": record, "section": None, "content": content}))
            break
    else:
        sys.stderr.write("not found\n")
        sys.exit(64)
else:
    sys.stderr.write("unexpected\n")
    sys.exit(64)
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).unwrap();
}

pub(in crate::commands) fn chat(id: &str, guru_id: &str, timestamp: i64) -> ChatSession {
    ChatSession {
        id: id.into(),
        guru_id: guru_id.into(),
        pi_session_id: "123e4567-e89b-42d3-a456-426614174000".into(),
        pi_session_cache: None,
        title: "Test chat".into(),
        memory_policy: MemoryPolicy::default(),
        messages: Vec::new(),
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    }
}
