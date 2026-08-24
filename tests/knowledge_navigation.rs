use serde_json::Value;
use std::{fs, path::Path, process::Output};

mod common;

fn temp_dir(label: &str) -> std::path::PathBuf {
    let path = common::temp_dir("navigation", label);
    assert!(common::command(&["init", path.to_str().unwrap()])
        .status
        .success());
    path
}

fn command(arguments: &[&str]) -> Output {
    common::command(arguments)
}

fn record(
    root: &Path,
    relative: &str,
    id: &str,
    title: &str,
    summary: &str,
    extra: &str,
    body: &str,
) {
    common::write_relative_record(
        root,
        relative,
        id,
        title,
        summary,
        extra,
        body,
        "2026-07-30T09:00:00+09:00",
    );
}

fn json_command(arguments: &[&str]) -> Value {
    common::json_command(arguments)
}

fn issue_fields(value: &Value) -> Vec<&str> {
    value["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|issue| issue["field"].as_str())
        .collect()
}

#[test]
fn wiki_links_validate_separately_from_analytical_relationships() {
    let root = temp_dir("validation");
    record(
        &root,
        "wiki/a.md",
        "wiki:a",
        "A",
        "A concept.",
        "see_also: [wiki:b]\n",
        "# A\n\nStandalone.",
    );
    record(
        &root,
        "wiki/b.md",
        "wiki:b",
        "B",
        "B concept.",
        "",
        "# B\n\nStandalone.",
    );
    let valid = json_command(&[
        "knowledge",
        "check",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(valid["valid"], true);

    record(
        &root,
        "lens/m.md",
        "lens:m",
        "M",
        "A lens.",
        "see_also: [wiki:a]\n",
        "# Steps\n\nRun.",
    );
    record(
        &root,
        "wiki/bad.md",
        "wiki:bad",
        "Bad",
        "Invalid links.",
        "see_also: [wiki:bad, wiki:missing, lens:m, wiki:a, wiki:a]\nlegacy_link: [wiki:a]\n",
        "# Bad\n\nInvalid.",
    );
    let output = command(&[
        "knowledge",
        "check",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    assert!(!output.status.success());
    let invalid: Value = serde_json::from_slice(&output.stdout).unwrap();
    let fields = issue_fields(&invalid);
    assert!(fields.iter().filter(|field| **field == "see_also").count() >= 5);
    assert!(fields.contains(&"frontmatter"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_duplicate_keys_and_empty_prohibited_list_fields() {
    let root = temp_dir("frontmatter-presence");
    record(
        &root,
        "wiki/a.md",
        "wiki:a",
        "A",
        "A concept.",
        "",
        "# A\n\nStandalone.",
    );
    record(
        &root,
        "wiki/b.md",
        "wiki:b",
        "B",
        "B concept.",
        "",
        "# B\n\nStandalone.",
    );
    record(
        &root,
        "wiki/duplicate.md",
        "wiki:duplicate",
        "Duplicate",
        "Duplicate frontmatter fields are invalid.",
        "see_also: [wiki:a]\nsee_also: [wiki:b]\n",
        "# Duplicate\n\nStandalone.",
    );
    record(
        &root,
        "lens/empty.md",
        "lens:empty",
        "Empty prohibited fields",
        "Empty declarations still express an invalid schema.",
        "see_also: []\nuses: []\nlegacy_link: []\n",
        "# Steps\n\nRun.",
    );

    let output = command(&[
        "knowledge",
        "check",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    assert!(!output.status.success());
    let invalid: Value = serde_json::from_slice(&output.stdout).unwrap();
    let errors = invalid["errors"].as_array().unwrap();
    assert!(errors.iter().any(|issue| {
        issue["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("wiki/duplicate.md"))
            && issue["field"] == "see_also"
            && issue["message"]
                .as_str()
                .is_some_and(|message| message.contains("declared only once"))
    }));
    for field in ["see_also", "uses", "frontmatter"] {
        assert!(errors.iter().any(|issue| {
            issue["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("lens/empty.md"))
                && issue["field"] == field
        }));
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn read_surfaces_fail_closed_on_duplicate_document_ids() {
    let root = temp_dir("duplicate-exact-read");
    for relative in ["wiki/first.md", "wiki/nested/second.md"] {
        record(
            &root,
            relative,
            "wiki:duplicate",
            "Duplicate",
            "The ID must resolve to exactly one record.",
            "",
            "# Body\n\nContent.",
        );
    }
    record(
        &root,
        "wiki/source.md",
        "wiki:source",
        "Source",
        "A unique source linked to an ambiguous target.",
        "see_also: [wiki:duplicate]\n",
        "# Body\n\nSource content.",
    );

    let output = command(&[
        "knowledge",
        "read",
        "wiki:duplicate",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("document ID is ambiguous"));

    let search = json_command(&[
        "knowledge",
        "search",
        "exactly one record",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(search, serde_json::json!([]));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn candidate_search_is_compact_explained_and_supports_kind_union() {
    let root = temp_dir("candidates");
    record(
        &root,
        "wiki/target.md",
        "wiki:target",
        "Quantum systems",
        "A reusable technical map.",
        "aliases: [quantum stack]\ntags: [commercialization]\n",
        "# Scaling constraints\n\nPhysical redundancy matters.",
    );
    record(
        &root,
        "lens/target.md",
        "lens:target",
        "Quantum commercialization",
        "A lens for scaling risk.",
        "",
        "# Application\n\nCompare milestones.",
    );
    record(
        &root,
        "evidence/excluded.md",
        "evidence:excluded",
        "Quantum commercialization scaling",
        "An excluded observation.",
        "source: https://example.test/e\n",
        "# Observation\n\nCurrent result.",
    );
    record(
        &root,
        "decision/body-scatter.md",
        "decision:body-scatter",
        "Technical review",
        "A generic procedure.",
        "",
        "# Notes\n\nQuantum details are followed by commercialization details and scaling details.",
    );

    let value = json_command(&[
        "knowledge",
        "search",
        "quantum commercialization scaling",
        "--kind",
        "wiki",
        "--kind",
        "lens",
        "--kind",
        "decision",
        "--candidates",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    let candidates = value.as_array().unwrap();
    assert!(candidates.iter().all(|item| item["kind"] != "evidence"));
    assert_eq!(candidates[0]["id"], "wiki:target");
    assert_eq!(candidates[0]["match_tier"], "all_terms");
    assert_eq!(
        candidates[0]["matched_fields"],
        serde_json::json!(["aliases", "heading", "tags", "title"])
    );
    assert!(candidates[0].get("text").is_none());
    assert!(candidates[0].get("origin").is_none());
    assert!(candidates[0].get("mutable").is_none());
    assert_eq!(
        candidates[0]["aliases"],
        serde_json::json!(["quantum stack"])
    );

    let body = candidates
        .iter()
        .find(|item| item["id"] == "decision:body-scatter")
        .unwrap();
    assert_eq!(body["match_tier"], "partial");
    assert_eq!(body["matched_fields"], serde_json::json!(["body"]));

    let default = json_command(&[
        "knowledge",
        "search",
        "quantum commercialization scaling",
        "--kind",
        "wiki",
        "--kind",
        "lens",
        "--kind",
        "decision",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    assert!(default[0].get("text").is_some());
    assert!(default[0].get("match_tier").is_none());
    assert!(default[0].get("aliases").is_none());
    assert!(default[0].get("origin").is_none());
    assert!(default[0].get("mutable").is_none());
    assert_eq!(
        candidates
            .iter()
            .map(|item| (&item["id"], &item["score"]))
            .collect::<Vec<_>>(),
        default
            .as_array()
            .unwrap()
            .iter()
            .map(|item| (&item["id"], &item["score"]))
            .collect::<Vec<_>>()
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn complete_concise_metadata_match_stays_at_document_level() {
    let root = temp_dir("concise-document-match");
    record(
        &root,
        "wiki/target.md",
        "wiki:target",
        "Quantum systems",
        "A reusable technical map.",
        "tags: [commercialization]\n",
        "# Historical examples\n\nQuantum appears in this otherwise unrelated section.",
    );

    let candidates = json_command(&[
        "knowledge",
        "search",
        "quantum commercialization",
        "--kind",
        "wiki",
        "--candidates",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(candidates[0]["id"], "wiki:target");
    assert_eq!(candidates[0]["section"], "");
    assert_eq!(candidates[0]["heading_path"], serde_json::json!([]));
    assert_eq!(candidates[0]["match_tier"], "all_terms");
    assert_eq!(
        candidates[0]["matched_fields"],
        serde_json::json!(["tags", "title"])
    );

    let default = json_command(&[
        "knowledge",
        "search",
        "quantum commercialization",
        "--kind",
        "wiki",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(default[0]["section"], "");
    assert_eq!(default[0]["heading_path"], serde_json::json!([]));
    assert!(!default[0]["text"]
        .as_str()
        .unwrap()
        .contains("Historical examples"));
    assert!(!default[0]["text"]
        .as_str()
        .unwrap()
        .contains("otherwise unrelated"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn health_reports_review_bands_and_deterministic_advisories_without_failing() {
    let root = temp_dir("health");
    for index in 0..25 {
        let (relative, title, alias, extra, body) = if index == 0 {
            (
                "wiki/a/b/c/deep.md".to_owned(),
                "Duplicate title".to_owned(),
                "Shared alias",
                "see_also: [wiki:1, wiki:2, wiki:3, wiki:4, wiki:5, wiki:6]\n",
                (1..=9)
                    .map(|section| format!("# Section {section}\n\nBody."))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            )
        } else if index == 1 {
            (
                "wiki/second.md".to_owned(),
                "duplicate-title".to_owned(),
                "shared alias",
                "",
                "# Section\n\nBody.".to_owned(),
            )
        } else {
            (
                format!("wiki/{index}.md"),
                format!("Wiki {index}"),
                "",
                "",
                "# Section\n\nBody.".to_owned(),
            )
        };
        let aliases = if alias.is_empty() {
            String::new()
        } else {
            format!("aliases: [{alias}]\n")
        };
        record(
            &root,
            &relative,
            &format!("wiki:{index}"),
            &title,
            "Summary.",
            &format!("{aliases}{extra}"),
            &body,
        );
    }
    let value = json_command(&[
        "knowledge",
        "health",
        "--kind",
        "wiki",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    let wiki = &value["kinds"][0];
    assert_eq!(wiki["kind"], "wiki");
    assert_eq!(wiki["documents"], 25);
    assert_eq!(wiki["folders"], 3);
    assert_eq!(wiki["max_depth"], 3);
    assert_eq!(wiki["review_band"], "organize");
    let codes = wiki["advisories"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["code"].as_str())
        .collect::<Vec<_>>();
    for expected in [
        "deep_folder",
        "large_document",
        "duplicate_title",
        "duplicate_alias",
        "excessive_see_also",
    ] {
        assert!(codes.contains(&expected), "missing {expected}: {codes:?}");
    }

    // Health reports advisory state and does not fail because links are unresolved.
    assert!(
        command(&["knowledge", "health", "--workspace", root.to_str().unwrap()])
            .status
            .success()
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn health_flags_evidence_duplicates_and_decision_revision_forks() {
    let root = temp_dir("health-kinds");
    for id in ["a", "b"] {
        record(
            &root,
            &format!("evidence/{id}.md"),
            &format!("evidence:{id}"),
            &format!("Evidence {id}"),
            "Same observation key.",
            "source: https://example.test/source\nperiod: 2026-Q2\nentities: [ticker:EXAMPLE]\n",
            "# Observation\n\nValue.",
        );
    }
    record(
        &root,
        "decision/base.md",
        "decision:base",
        "Base",
        "Original judgment.",
        "",
        "# Decision\n\nHold.",
    );
    for id in ["left", "right"] {
        let relationship = if id == "left" {
            "updates"
        } else {
            "contradicts"
        };
        record(
            &root,
            &format!("decision/{id}.md"),
            &format!("decision:{id}"),
            &format!("Revision {id}"),
            "A later judgment.",
            &format!("{relationship}: [decision:base]\n"),
            "# Decision\n\nReview.",
        );
    }

    let value = json_command(&[
        "knowledge",
        "health",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    let evidence = value["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|kind| kind["kind"] == "evidence")
        .unwrap();
    assert!(evidence["advisories"]
        .as_array()
        .unwrap()
        .iter()
        .any(|advisory| advisory["code"] == "duplicate_evidence_candidate"));
    let decision = value["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|kind| kind["kind"] == "decision")
        .unwrap();
    let fork = decision["advisories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|advisory| advisory["code"] == "decision_revision_fork")
        .unwrap();
    assert_eq!(
        fork["ids"],
        serde_json::json!(["decision:base", "decision:left", "decision:right"])
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn health_treats_titles_and_aliases_as_one_naming_namespace() {
    let root = temp_dir("health-name-collision");
    record(
        &root,
        "wiki/title.md",
        "wiki:title",
        "Quantum error correction",
        "Canonical concept.",
        "",
        "# Concept\n\nBody.",
    );
    record(
        &root,
        "wiki/alias.md",
        "wiki:alias",
        "Logical qubit overview",
        "Adjacent concept.",
        "aliases: [quantum-error-correction]\n",
        "# Concept\n\nBody.",
    );

    let value = json_command(&[
        "knowledge",
        "health",
        "--kind",
        "wiki",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    let collision = value["kinds"][0]["advisories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|advisory| advisory["code"] == "duplicate_alias")
        .unwrap();
    assert_eq!(
        collision["ids"],
        serde_json::json!(["wiki:alias", "wiki:title"])
    );

    fs::remove_dir_all(root).unwrap();
}
