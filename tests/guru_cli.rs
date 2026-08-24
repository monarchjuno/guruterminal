use serde_json::Value;
use std::{fs, path::Path, process::Output};

mod common;

fn temp_dir(label: &str) -> std::path::PathBuf {
    common::temp_dir("core", label)
}

fn command(arguments: &[&str]) -> Output {
    common::command(arguments)
}

fn init(root: &Path) {
    common::init(root);
}

fn record(kind: &str, id: &str, title: &str) -> String {
    let source = if kind == "evidence" {
        "source: https://example.test/source\n"
    } else {
        ""
    };
    format!(
        "---\nid: {id}\ntitle: {title}\nsummary: {title} summary.\nas_of: 2026-08-09T00:00:00Z\n{source}---\n\n# Core\n\nDurable content.\n"
    )
}

#[test]
fn init_creates_only_the_guruterminal_v1_layout() {
    let root = temp_dir("init");
    init(&root);

    for kind in ["wiki", "lens", "evidence", "decision"] {
        assert!(root.join("guruterminal").join(kind).is_dir());
    }
    assert!(!root.join("guruterminal/method").exists());
    assert!(!root.join(".guruterminal/packs").exists());
    assert!(!root.join(".guruterminal/mode").exists());

    let metadata: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".guruterminal/workspace.json")).expect("metadata"),
    )
    .expect("valid metadata");
    assert_eq!(metadata["schema_version"], 1);
    fs::remove_dir_all(root).expect("cleanup");
}

#[cfg(unix)]
#[test]
fn dangling_current_workspace_marker_is_rejected_before_any_write() {
    use std::os::unix::fs::symlink;

    for marker in ["guruterminal", ".guruterminal"] {
        let root = temp_dir("dangling-current");
        symlink(root.join("missing-current-target"), root.join(marker))
            .expect("dangling current fixture");
        let output = command(&["init", root.to_str().expect("utf-8 path"), "--json"]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("already contains"));
        assert!(fs::symlink_metadata(root.join(marker)).is_ok());
        let other = if marker == "guruterminal" {
            ".guruterminal"
        } else {
            "guruterminal"
        };
        assert!(fs::symlink_metadata(root.join(other)).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }
}

#[test]
fn partial_guruterminal_state_is_rejected_without_completion() {
    let root = temp_dir("partial");
    fs::create_dir_all(root.join("guruterminal/wiki")).expect("partial fixture");
    let sentinel = root.join("guruterminal/wiki/keep.txt");
    fs::write(&sentinel, "unchanged\n").expect("sentinel");

    let output = command(&["init", root.to_str().expect("utf-8 path"), "--json"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already contains"));
    assert_eq!(
        fs::read_to_string(sentinel).expect("sentinel remains"),
        "unchanged\n"
    );
    assert!(!root.join("guruterminal/lens").exists());
    assert!(!root.join(".guruterminal").exists());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn unsupported_workspace_schema_is_rejected_without_rewrite() {
    let root = temp_dir("schema");
    init(&root);
    let metadata = root.join(".guruterminal/workspace.json");
    fs::write(&metadata, "{\"schema_version\":2}\n").expect("unsupported metadata");

    let repeated_init = command(&["init", root.to_str().expect("utf-8 path"), "--json"]);
    assert!(!repeated_init.status.success());
    assert_eq!(
        fs::read_to_string(&metadata).expect("metadata remains after init"),
        "{\"schema_version\":2}\n"
    );

    let output = command(&[
        "knowledge",
        "list",
        "--workspace",
        root.to_str().expect("utf-8 path"),
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported"));
    assert_eq!(
        fs::read_to_string(metadata).expect("metadata remains"),
        "{\"schema_version\":2}\n"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn incomplete_or_removed_memory_layout_is_rejected() {
    let root = temp_dir("layout");
    init(&root);
    fs::remove_dir(root.join("guruterminal/lens")).expect("remove required directory");

    let output = command(&[
        "knowledge",
        "list",
        "--workspace",
        root.to_str().expect("utf-8 path"),
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not initialized"));

    fs::create_dir(root.join("guruterminal/lens")).expect("restore required directory");
    fs::create_dir(root.join("guruterminal/method")).expect("removed kind fixture");
    let output = command(&[
        "knowledge",
        "list",
        "--workspace",
        root.to_str().expect("utf-8 path"),
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not initialized"));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn internal_cli_exposes_memory_operations_but_not_plugins_or_packs() {
    let help = command(&["--help"]);
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("INTERNAL USE ONLY"));
    assert!(stdout.contains("knowledge"));
    assert!(!stdout.contains(" pack "));
    assert!(!stdout.contains(" mode "));
    assert!(!stdout.contains("hook"));

    for obsolete in ["pack", "mode", "hook", "graph"] {
        assert!(!command(&[obsolete]).status.success());
    }
    assert!(!command(&["knowledge", "neighbors"]).status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("neighbors"));
    assert!(!String::from_utf8_lossy(&help.stdout).contains("--entity"));
}

#[test]
fn list_search_read_and_check_use_the_four_kind_contract() {
    let root = temp_dir("knowledge");
    init(&root);
    for (kind, id, title) in [
        ("wiki", "wiki:company", "Company context"),
        ("lens", "lens:quality/earnings-quality", "Quality lens"),
        ("evidence", "evidence:filing", "Filing evidence"),
        ("decision", "decision:position", "Position decision"),
    ] {
        fs::write(
            root.join("guruterminal")
                .join(kind)
                .join(format!("{kind}.md")),
            record(kind, id, title),
        )
        .expect("record");
    }
    fs::write(
        root.join("guruterminal/wiki/revoked.md"),
        "---\nid: wiki:retired\ntitle: Retired quality claim\nsummary: Unused quality claim.\nas_of: 2026-08-01T00:00:00Z\nstatus: revoked\nrevoked_by: evidence:filing\n---\n\n# Core\n\nSuperseded quality claim.\n",
    )
    .expect("revoked wiki");

    let workspace = root.to_str().expect("utf-8 path");
    let list = command(&["knowledge", "list", "--workspace", workspace, "--json"]);
    assert!(list.status.success());
    let documents: Value = serde_json::from_slice(&list.stdout).expect("list json");
    assert_eq!(documents.as_array().expect("documents").len(), 5);
    assert!(documents
        .as_array()
        .expect("documents")
        .iter()
        .all(|item| item["kind"] != "method"
            && item.get("origin").is_none()
            && item.get("mutable").is_none()
            && item.get("published_at").is_none()));

    let search = command(&[
        "knowledge",
        "search",
        "quality",
        "--workspace",
        workspace,
        "--json",
    ]);
    assert!(search.status.success());
    let search_body = String::from_utf8_lossy(&search.stdout);
    assert!(search_body.contains("lens:quality/earnings-quality"));
    assert!(!search_body.contains("wiki:retired"));

    let historical_search = command(&[
        "knowledge",
        "search",
        "quality",
        "--as-of",
        "2026-08-08",
        "--workspace",
        workspace,
        "--json",
    ]);
    assert!(historical_search.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&historical_search.stdout).unwrap(),
        serde_json::json!([])
    );

    let include_revoked = command(&[
        "knowledge",
        "search",
        "quality",
        "--include-revoked",
        "--workspace",
        workspace,
        "--json",
    ]);
    assert!(include_revoked.status.success());
    assert!(String::from_utf8_lossy(&include_revoked.stdout).contains("wiki:retired"));

    let read = command(&[
        "knowledge",
        "read",
        "wiki:company",
        "--section",
        "Core",
        "--workspace",
        workspace,
        "--json",
    ]);
    assert!(read.status.success());
    assert!(String::from_utf8_lossy(&read.stdout).contains("Durable content"));

    let check = command(&["knowledge", "check", "--workspace", workspace, "--json"]);
    assert!(check.status.success());
    fs::remove_dir_all(root).expect("cleanup");
}
