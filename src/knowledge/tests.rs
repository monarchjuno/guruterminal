use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "guruterminal-knowledge-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT_TEMP_ROOT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn write(root: &Path, relative: &str, value: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

fn search_with_workers_for_test(
    root: &Path,
    query: &str,
    kinds: &[String],
    limit: usize,
    explain: bool,
    workers: usize,
) -> Vec<SearchResult> {
    let paths = local_markdown_files_for_kinds(root, kinds);
    let query = SearchText::new(query);
    let ranked = search_paths_with_workers(root, &paths, &query, limit, explain, workers).unwrap();
    finalize_search_results(ranked, limit)
}

#[test]
fn check_reports_required_time_evidence_and_duplicates() {
    let root = root();
    write(
        &root,
        "guruterminal/lens/a.md",
        "---\nid: duplicate\ntitle: A\nsummary: A\nas_of: 2026-01-01T00:00Z\n---",
    );
    write(
        &root,
        "guruterminal/decision/b.md",
        "---\nid: duplicate\ntitle: B\nsummary: B\nas_of: 2026-01-01T00:00:00Z\n---",
    );
    write(
        &root,
        "guruterminal/evidence/e.md",
        "---\nid: e\ntitle: E\nsummary: E\nas_of: 2026-01-01T00:00:00Z\n---",
    );
    write(&root, "guruterminal/wiki/plain.md", "# no frontmatter\n");
    let check = check(&root);
    assert!(!check.valid);
    assert!(check
        .errors
        .iter()
        .any(|i| i.field == "id" && i.message.contains("duplicates")));
    assert!(check.errors.iter().any(|i| i.field == "as_of"));
    assert!(check.errors.iter().any(|i| i.field == "id"));
    assert!(check.errors.iter().any(|i| i.field == "frontmatter"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn evidence_dossier_omits_record_source_and_search_text_carries_entity_period() {
    let root = root();
    write(
        &root,
        "guruterminal/evidence/tsmc-capacity.md",
        "---\nid: evidence:theme/tsmc-3nm\ntitle: TSMC 3nm capacity\nsummary: Packaging tightness claims from this research turn.\nas_of: 2026-08-19T00:00:00Z\nentities:\n  - TSMC\nperiod: 2026-Q2\n---\n\n# Claims\n\n- Utilization rose on packaging tightness.\n\n# Sources\n\n- `https://example.test/tsmc`\n",
    );
    let check = check(&root);
    assert!(check.valid, "{:?}", check.errors);
    let hits = search_with_kinds(&root, "utilization packaging", &[], 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "evidence:theme/tsmc-3nm");
    assert!(hits[0].text.contains("TSMC"));
    assert!(hits[0].text.contains("2026-Q2"));
    assert_eq!(hits[0].period.as_deref(), Some("2026-Q2"));
    assert!(!hits[0].text.contains("Utilization rose"));
    assert_eq!(
        search_with_kinds(&root, "TSMC", &[], 5).unwrap()[0].id,
        "evidence:theme/tsmc-3nm"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn source_less_evidence_requires_claims_and_sources() {
    let root = root();
    write(
        &root,
        "guruterminal/evidence/incomplete.md",
        "---\nid: evidence:theme/incomplete\ntitle: Incomplete dossier\nsummary: Missing sources.\nas_of: 2026-08-19T00:00:00Z\n---\n\n# Claims\n\n- A claim.\n",
    );
    let result = check(&root);
    assert!(!result.valid);
    assert!(result
        .errors
        .iter()
        .any(|issue| { issue.field == "body" && issue.message.contains("# Sources") }));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn exact_read_selects_one_local_element_section() {
    let root = root();
    write(
        &root,
        "guruterminal/wiki/a.md",
        "---\nid: wiki:a\ntitle: A\nsummary: A\nas_of: 2026-01-01T00:00:00Z\n---\n# Chosen\nBody",
    );
    let result = read(&root, "wiki:a", Some("Chosen")).unwrap();
    assert_eq!(result.section.unwrap().text, "Body");
    let _ = fs::remove_dir_all(root);
}
#[test]
fn search_normalizes_compounds_and_scores_financial_metadata() {
    let root = root();
    write(
        &root,
        "guruterminal/evidence/cash-flow.md",
        "---\nid: evidence:cash-flow\ntitle: Cash generation\nsummary: Quarterly observation.\nas_of: 2026-07-27T09:00:00Z\nsource: https://example.com/report\nperiod: 2026-Q2\ntags:\n  - free-cash-flow\n---\n\n# Observation\nConversion improved.",
    );

    let compound = search_with_kinds(&root, "free cash flow", &[], 5).unwrap();
    assert_eq!(compound.len(), 1);
    assert_eq!(compound[0].id, "evidence:cash-flow");

    let period = search_with_kinds(&root, "2026 Q2", &[], 5).unwrap();
    assert_eq!(period.len(), 1);
    assert_eq!(period[0].id, "evidence:cash-flow");
    let _ = fs::remove_dir_all(root);
}
#[test]
fn search_returns_one_section_per_record_and_breaks_ties_by_as_of() {
    let root = root();
    write(
        &root,
        "guruterminal/wiki/older.md",
        "---\nid: wiki:older\ntitle: Older signal\nsummary: Cash flow signal.\nas_of: 2026-01-01T00:00:00Z\n---\n\n# First\nCash flow.\n\n# Second\nCash flow.",
    );
    write(
        &root,
        "guruterminal/wiki/newer.md",
        "---\nid: wiki:newer\ntitle: Newer signal\nsummary: Cash flow signal.\nas_of: 2026-07-01T00:00:00Z\n---\n\n# First\nCash flow.\n\n# Second\nCash flow.",
    );

    let results = search_with_kinds(&root, "cash flow signal", &[], 10).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].id, "wiki:newer");
    assert_eq!(results[1].id, "wiki:older");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn as_of_filters_before_the_search_result_limit() {
    let root = root();
    for index in 0..7 {
        write(
            &root,
            &format!("guruterminal/wiki/future-{index}.md"),
            &format!(
                "---\nid: wiki:future-{index}\ntitle: Future signal {index}\nsummary: Shared cutoff signal.\nas_of: 2026-08-0{}T00:00:00Z\n---\n\n# Signal\nShared cutoff signal.",
                index + 1
            ),
        );
    }
    write(
        &root,
        "guruterminal/wiki/past.md",
        "---\nid: wiki:past\ntitle: Past signal\nsummary: Shared cutoff signal.\nas_of: 2026-01-15T00:00:00Z\n---\n\n# Signal\nShared cutoff signal.",
    );

    let cutoff = chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let results =
        search_with_kinds_opts(&root, "shared cutoff signal", &[], 6, false, Some(cutoff)).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "wiki:past");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn sequential_and_parallel_scans_preserve_search_candidates_and_reads() {
    let root = root();
    for index in 0..320 {
        let special = if index == 257 {
            "aliases: [durable conversion signal]\nentities: [ticker:FILT]\nsee_also: [wiki:record-001]\n"
        } else {
            ""
        };
        let numeric = if index == 257 {
            "The signed operating-margin change was +5.0%."
        } else {
            "The record has no signed operating-margin observation."
        };
        write(
            &root,
            &format!("guruterminal/wiki/record-{index:03}.md"),
            &format!(
                "---\nid: wiki:record-{index:03}\ntitle: Record {index:03}\nsummary: Common liquidity observation.\nas_of: 2026-01-01T00:00:00Z\n{special}---\n\n# Common liquidity\nBroad liquidity context for record {index}.\n\n# Detail\nRecord-specific detail {index}. {numeric}"
            ),
        );
    }
    write(
        &root,
        "guruterminal/lens/union.md",
        "---\nid: lens:union\ntitle: Union signal lens\nsummary: A repeated-kind filter target.\nas_of: 2026-01-01T00:00:00Z\n---\n\n# Application\nUnion signal.",
    );
    write(
        &root,
        "guruterminal/evidence/excluded.md",
        "---\nid: evidence:excluded\ntitle: Union signal evidence\nsummary: An excluded repeated-kind target.\nas_of: 2026-01-01T00:00:00Z\nsource: https://example.test/excluded\n---\n\n# Observation\nUnion signal.",
    );
    write(
        &root,
        "guruterminal/lens/legacy.md",
        "---\nid: custom:legacy\ntitle: Legacy identifier\nsummary: Exact-read fallback fixture.\nas_of: 2026-01-01T00:00:00Z\n---\n\n# Detail\nFallback body.",
    );
    write(
        &root,
        "guruterminal/lens/misplaced.md",
        "---\nid: wiki:misplaced\ntitle: Misplaced identifier\nsummary: Known-prefix read fallback fixture.\nas_of: 2026-01-01T00:00:00Z\n---\n\n# Detail\nKnown-prefix fallback body.",
    );
    write(
        &root,
        "guruterminal/lens/removed-method.md",
        "---\nid: method:removed\ntitle: Removed Method identifier\nsummary: Removed Memory kind fixture.\nas_of: 2026-01-01T00:00:00Z\n---\n\n# Detail\nRemoved Method body.",
    );
    write(
        &root,
        "guruterminal/wiki/a-duplicate.md",
        "---\nid: wiki:duplicate\ntitle: Duplicate loser\nsummary: A duplicate winner query appears only in summary.\nas_of: 2026-01-01T00:00:00Z\n---\n\n# Notes\nDuplicate winner.",
    );
    write(
        &root,
        "guruterminal/wiki/z-duplicate.md",
        "---\nid: wiki:duplicate\ntitle: Duplicate winner\nsummary: The higher-ranked duplicate.\nas_of: 2026-01-01T00:00:00Z\n---\n\n# Notes\nWinner body.",
    );

    assert_eq!(scan_worker_count(PARALLEL_SCAN_MIN_FILES - 1), 1);
    assert!((1..=MAX_SCAN_WORKERS).contains(&scan_worker_count(320)));

    let no_kinds = Vec::<String>::new();
    for query in [
        "common liquidity",
        "durable conversion signal",
        "+5.0%",
        "wiki:record-257",
        "duplicate winner",
    ] {
        let sequential = search_with_workers_for_test(&root, query, &no_kinds, 50, false, 1);
        let parallel = search_with_workers_for_test(&root, query, &no_kinds, 50, false, 8);
        assert_eq!(parallel, sequential, "default search parity for {query:?}");
        assert_eq!(
            search_with_kinds(&root, query, &no_kinds, 50).unwrap(),
            sequential,
            "automatic scan parity for {query:?}"
        );

        let sequential = search_with_workers_for_test(&root, query, &no_kinds, 50, true, 1);
        let parallel = search_with_workers_for_test(&root, query, &no_kinds, 50, true, 8);
        assert_eq!(
            parallel, sequential,
            "candidate search parity for {query:?}"
        );
        assert_eq!(
            search_candidates_with_kinds(&root, query, &no_kinds, 50).unwrap(),
            candidate_results(&sequential),
            "automatic candidate parity for {query:?}"
        );
    }
    assert!(search_with_kinds(&root, "duplicate winner", &no_kinds, 50)
        .unwrap()
        .is_empty());
    assert!(search_with_kinds(&root, "-5.0%", &no_kinds, 50)
        .unwrap()
        .is_empty());

    let all_paths = local_markdown_files(&root);
    let broad_query = SearchText::new("common liquidity");
    assert!(
        search_paths_with_workers(&root, &all_paths, &broad_query, 7, false, 1)
            .unwrap()
            .len()
            <= 7
    );
    assert!(
        search_paths_with_workers(&root, &all_paths, &broad_query, 7, false, 8)
            .unwrap()
            .len()
            <= 8 * 7
    );

    let repeated_kinds = vec!["wiki".to_owned(), "lens".to_owned(), "wiki".to_owned()];
    let unique_kinds = vec!["wiki".to_owned(), "lens".to_owned()];
    assert_eq!(
        local_markdown_files_for_kinds(&root, &repeated_kinds),
        local_markdown_files_for_kinds(&root, &unique_kinds)
    );
    let sequential =
        search_with_workers_for_test(&root, "union signal", &repeated_kinds, 50, true, 1);
    let parallel =
        search_with_workers_for_test(&root, "union signal", &repeated_kinds, 50, true, 8);
    assert_eq!(parallel, sequential);
    assert!(parallel.iter().all(|item| item.kind != "evidence"));
    assert_eq!(
        search_candidates_with_kinds(&root, "union signal", &repeated_kinds, 50).unwrap(),
        candidate_results(&sequential)
    );

    let sequential = search_with_workers_for_test(&root, "ticker:FILT", &no_kinds, 50, true, 1);
    let parallel = search_with_workers_for_test(&root, "ticker:FILT", &no_kinds, 50, true, 8);
    assert_eq!(parallel, sequential);
    assert_eq!(parallel.len(), 1);
    assert_eq!(parallel[0].id, "wiki:record-257");

    let sequential = read_internal(&root, "wiki:record-257", Some("Detail"), Some(1)).unwrap();
    let parallel = read_internal(&root, "wiki:record-257", Some("Detail"), Some(8)).unwrap();
    assert_eq!(parallel, sequential);
    assert_eq!(parallel.document.see_also, vec!["wiki:record-001"]);
    assert!(parallel.section.unwrap().text.contains("+5.0%"));
    assert_eq!(
        read(&root, "wiki:record-257", Some("Detail")).unwrap(),
        sequential
    );

    for invalid_id in ["custom:legacy", "wiki:misplaced", "method:removed"] {
        assert!(read_internal(&root, invalid_id, None, Some(1)).is_err());
        assert!(read_internal(&root, invalid_id, None, Some(8)).is_err());
    }
    assert!(
        search_with_kinds(&root, "removed method body", &no_kinds, 50)
            .unwrap()
            .is_empty()
    );
    let invalid_id_errors = check(&root)
        .errors
        .into_iter()
        .filter(|issue| issue.field == "id")
        .collect::<Vec<_>>();
    assert!(invalid_id_errors
        .iter()
        .any(|issue| issue.path.ends_with("legacy.md")));
    assert!(invalid_id_errors
        .iter()
        .any(|issue| issue.path.ends_with("misplaced.md")));
    assert!(invalid_id_errors
        .iter()
        .any(|issue| issue.path.ends_with("removed-method.md")));

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn catalog_does_not_follow_symlinked_files_directories_or_loops() {
    use std::os::unix::fs::symlink;

    let root = root();
    let outside = root.join("outside");
    write(
        &root,
        "outside/escaped.md",
        "---\nid: escaped\ntitle: Escaped\nsummary: Escaped\nas_of: 2026-01-01T00:00:00Z\n---",
    );
    let wiki = root.join("guruterminal/wiki");
    fs::create_dir_all(&wiki).unwrap();
    symlink(&outside, wiki.join("escaped-directory")).unwrap();
    symlink(outside.join("escaped.md"), wiki.join("escaped-file.md")).unwrap();
    symlink(&wiki, wiki.join("loop")).unwrap();
    fs::create_dir_all(root.join("guruterminal")).unwrap();
    symlink(&outside, root.join("guruterminal/lens")).unwrap();

    assert!(catalog_local(&root).is_empty());
    assert_eq!(check(&root).errors.len(), 4);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn check_enforces_the_minimal_relationship_matrix() {
    let root = root();
    write(
        &root,
        "guruterminal/wiki/w.md",
        "---\nid: wiki:w\ntitle: W\nsummary: W\nas_of: 2026-01-01T00:00:00Z\nuses:\n  - lens:l\n---",
    );
    write(
        &root,
        "guruterminal/lens/l.md",
        "---\nid: lens:l\ntitle: L\nsummary: L\nas_of: 2026-01-01T00:00:00Z\n---",
    );
    write(
        &root,
        "guruterminal/evidence/e.md",
        "---\nid: evidence:e\ntitle: E\nsummary: E\nas_of: 2026-01-01T00:00:00Z\nsource: https://example.test/e\nupdates:\n  - lens:l\n---",
    );
    write(
        &root,
        "guruterminal/decision/d.md",
        "---\nid: decision:d\ntitle: D\nsummary: D\nas_of: 2026-01-01T00:00:00Z\nuses:\n  - lens:l\nsupports:\n  - evidence:e\nupdates:\n  - decision:missing\n---",
    );
    let result = check(&root);
    assert!(!result.valid);
    assert!(result
        .errors
        .iter()
        .any(|issue| issue.path.ends_with("wiki/w.md") && issue.field == "uses"));
    assert!(result
        .errors
        .iter()
        .any(|issue| issue.path.ends_with("evidence/e.md")
            && issue.message.contains("disallowed kind")));
    assert!(result
        .errors
        .iter()
        .any(|issue| issue.message.contains("decision:missing")));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn default_search_excludes_revoked_wiki_and_lens_but_exact_read_still_works() {
    let root = root();
    write(
        &root,
        "guruterminal/wiki/active.md",
        "---\nid: wiki:active-claim\ntitle: Active claim\nsummary: Still used.\nas_of: 2026-08-19T00:00:00Z\nstatus: active\n---\n\n# Active claim\n\nReusable fact.\n",
    );
    write(
        &root,
        "guruterminal/wiki/revoked.md",
        "---\nid: wiki:revoked-claim\ntitle: Revoked claim\nsummary: No longer used.\nas_of: 2026-08-01T00:00:00Z\nstatus: revoked\nrevoked_by: evidence:later\n---\n\n# Revoked claim\n\nSuperseded fact.\n",
    );
    write(
        &root,
        "guruterminal/evidence/later.md",
        "---\nid: evidence:later\ntitle: Later observation\nsummary: Contradicts the old claim.\nas_of: 2026-08-19T00:00:00Z\nsource: https://example.test/later\n---\n\n# Later observation\n\nNew fact.\n",
    );
    let hits = search_with_kinds(&root, "claim", &[], 10).unwrap();
    assert!(hits.iter().any(|hit| hit.id == "wiki:active-claim"));
    assert!(!hits.iter().any(|hit| hit.id == "wiki:revoked-claim"));
    let with_revoked = search_with_kinds_opts(&root, "claim", &[], 10, true, None).unwrap();
    assert!(with_revoked
        .iter()
        .any(|hit| hit.id == "wiki:revoked-claim" && hit.status.as_deref() == Some("revoked")));
    let read = read(&root, "wiki:revoked-claim", None).unwrap();
    assert_eq!(read.document.status.as_deref(), Some("revoked"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn revoked_wiki_requires_revoked_by_and_rejects_active_revoked_by() {
    let root = root();
    write(
        &root,
        "guruterminal/wiki/bare-revoked.md",
        "---\nid: wiki:bare-revoked\ntitle: Bare revoked\nsummary: Missing pointer.\nas_of: 2026-08-19T00:00:00Z\nstatus: revoked\n---\n\n# Bare revoked\n\nUnused.\n",
    );
    write(
        &root,
        "guruterminal/wiki/active-pointer.md",
        "---\nid: wiki:active-pointer\ntitle: Active pointer\nsummary: Active with pointer.\nas_of: 2026-08-19T00:00:00Z\nstatus: active\nrevoked_by: evidence:later\n---\n\n# Active pointer\n\nStill used.\n",
    );
    let check = check(&root);
    assert!(!check.valid);
    assert!(check.errors.iter().any(|issue| {
        issue.field == "revoked_by" && issue.message.contains("requires revoked_by")
    }));
    assert!(check.errors.iter().any(|issue| {
        issue.field == "revoked_by" && issue.message.contains("only when status is revoked")
    }));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn revoked_by_target_must_exist_and_not_reference_itself() {
    let root = root();
    write(
        &root,
        "guruterminal/wiki/missing.md",
        "---\nid: wiki:missing-target\ntitle: Missing target\nsummary: Invalid pointer.\nas_of: 2026-08-19T00:00:00Z\nstatus: revoked\nrevoked_by: evidence:missing\n---\n\n# Claim\n\nUnused.\n",
    );
    write(
        &root,
        "guruterminal/wiki/self.md",
        "---\nid: wiki:self\ntitle: Self pointer\nsummary: Invalid pointer.\nas_of: 2026-08-19T00:00:00Z\nstatus: revoked\nrevoked_by: wiki:self\n---\n\n# Claim\n\nUnused.\n",
    );
    let result = check(&root);
    assert!(!result.valid);
    assert!(result.errors.iter().any(|issue| {
        issue.field == "revoked_by" && issue.message.contains("evidence:missing")
    }));
    assert!(result.errors.iter().any(|issue| {
        issue.field == "revoked_by" && issue.message.contains("must not reference itself")
    }));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn written_wiki_is_searchable_and_readable_in_the_same_host_cycle() {
    let root = root();
    write(
        &root,
        "guruterminal/wiki/learned.md",
        "---\nid: wiki:ev-industry\ntitle: EV industry\nsummary: Durable EV industry facts.\nas_of: 2026-08-19T00:00:00Z\n---\n\n# EV industry\n\nCompiled from current research.\n",
    );
    let hits = search_with_kinds(&root, "EV industry", &["wiki".into()], 8).unwrap();
    assert!(hits.iter().any(|hit| hit.id == "wiki:ev-industry"));
    let read = read(&root, "wiki:ev-industry", None).unwrap();
    assert!(read
        .document
        .content
        .contains("Compiled from current research"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn later_english_research_prompts_retrieve_learned_wiki_and_lens() {
    let root = root();
    write(
        &root,
        "guruterminal/wiki/tsmc-foundry.md",
        "---\nid: wiki:tsmc-foundry-economics\ntitle: TSMC foundry economics\nsummary: Advanced packaging, not wafer starts, is the binding capacity constraint for leading-edge TSMC nodes.\nas_of: 2026-03-15T00:00:00Z\naliases:\n  - Taiwan Semiconductor\n  - 2330.TW\nentities:\n  - TSMC\ntags:\n  - foundry\n  - CoWoS\n---\n\n# Constraint\n\nCoWoS and related packaging remain tighter than leading-edge wafer starts.\n",
    );
    write(
        &root,
        "guruterminal/lens/pricing-power.md",
        "---\nid: lens:pricing-power-quality\ntitle: Pricing power quality\nsummary: Price realization must survive volume pressure before it changes a quality bar.\nas_of: 2026-04-01T00:00:00Z\naliases:\n  - margin quality\ntags:\n  - quality\n  - margins\n---\n\n# Scope\n\nConsumer franchises where price mix is used as evidence of quality.\n\n# Assumptions\n\nOne strong quarter is not a structural change.\n\n# Counterexamples\n\nA competitor match on list price with rising churn falsifies the claim.\n\n# Limits\n\nDoes not apply to regulated tariffs or one-off surcharges.\n\n# Invalidation conditions\n\nSustained unit losses after a price increase.\n",
    );
    write(
        &root,
        "guruterminal/wiki/unrelated.md",
        "---\nid: wiki:airline-loyalty\ntitle: Airline loyalty breakage\nsummary: Deferred revenue from unused miles is not the same as pricing power.\nas_of: 2026-05-01T00:00:00Z\nentities:\n  - Delta\n---\n\n# Note\n\nBreakage accounting is a liability release, not a foundry constraint.\n",
    );

    let wiki_prompts = [
        "What is the binding constraint on leading-edge foundry supply?",
        "Taiwan Semiconductor packaging bottleneck",
        "How should I frame TSMC capacity risk in a later review?",
        "2330.TW advanced packaging",
        "Does CoWoS remaining tight change the leading foundry story?",
    ];
    for query in wiki_prompts {
        let hits = search_with_kinds(&root, query, &["wiki".into()], 5).unwrap();
        assert!(
            hits.iter()
                .any(|hit| hit.id == "wiki:tsmc-foundry-economics"),
            "wiki paraphrase missed {query:?}: {hits:?}"
        );
    }

    let lens_prompts = [
        "Should one quarter of better pricing change my quality bar?",
        "When would I throw out a margin-quality interpretation?",
        "How do I keep pricing-power claims falsifiable?",
    ];
    for query in lens_prompts {
        let hits = search_with_kinds(&root, query, &["lens".into()], 5).unwrap();
        assert!(
            hits.iter()
                .any(|hit| hit.id == "lens:pricing-power-quality"),
            "lens paraphrase missed {query:?}: {hits:?}"
        );
    }

    let unrelated = search_with_kinds(
        &root,
        "How should I think about airline loyalty-program breakage?",
        &["wiki".into()],
        5,
    )
    .unwrap();
    assert!(
        unrelated.iter().any(|hit| hit.id == "wiki:airline-loyalty"),
        "{unrelated:?}"
    );
    assert!(
        !unrelated
            .iter()
            .any(|hit| hit.id == "wiki:tsmc-foundry-economics"),
        "foundry wiki leaked into an unrelated later prompt: {unrelated:?}"
    );

    let cutoff = chrono::NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
    let as_of_hits = search_with_kinds_opts(
        &root,
        "foundry capacity constraint",
        &["wiki".into()],
        5,
        false,
        Some(cutoff),
    )
    .unwrap();
    assert!(as_of_hits
        .iter()
        .any(|hit| hit.id == "wiki:tsmc-foundry-economics"));
    assert!(!as_of_hits
        .iter()
        .any(|hit| hit.id == "wiki:airline-loyalty"));

    write(
        &root,
        "guruterminal/wiki/tsmc-foundry.md",
        "---\nid: wiki:tsmc-foundry-economics\ntitle: TSMC foundry economics\nsummary: Superseded packaging claim.\nas_of: 2026-06-01T00:00:00Z\nstatus: revoked\nrevoked_by: wiki:tsmc-packaging-constraint\naliases:\n  - Taiwan Semiconductor\nentities:\n  - TSMC\n---\n\n# Constraint\n\nUnused.\n",
    );
    write(
        &root,
        "guruterminal/wiki/tsmc-packaging.md",
        "---\nid: wiki:tsmc-packaging-constraint\ntitle: TSMC packaging constraint\nsummary: Later evidence replaces the earlier packaging constraint.\nas_of: 2026-06-01T00:00:00Z\nentities:\n  - TSMC\n---\n\n# Constraint\n\nCurrent replacement.\n",
    );
    let after_revoke = search_with_kinds(&root, "TSMC foundry", &["wiki".into()], 5).unwrap();
    assert!(!after_revoke
        .iter()
        .any(|hit| hit.id == "wiki:tsmc-foundry-economics"));
    let read = read(&root, "wiki:tsmc-foundry-economics", None).unwrap();
    assert_eq!(read.document.status.as_deref(), Some("revoked"));
    let _ = fs::remove_dir_all(root);
}
