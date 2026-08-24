use std::{
    fs,
    path::Path,
    process::Command,
    process::Output,
    time::{Duration, Instant},
};

mod common;

fn bin() -> &'static str {
    common::bin()
}

fn temp_dir(label: &str) -> std::path::PathBuf {
    common::temp_dir("retrieval-eval", label)
}

fn command(arguments: &[&str]) -> Output {
    common::command(arguments)
}

fn write_record(
    root: &Path,
    kind: &str,
    id: &str,
    title: &str,
    summary: &str,
    extra_frontmatter: &str,
    body: &str,
) {
    common::write_record(
        root,
        kind,
        id,
        title,
        summary,
        extra_frontmatter,
        body,
        "2026-07-30T09:00:00+09:00",
    );
}

fn search(root: &Path, query: &str, candidates: bool, limit: usize) -> Output {
    search_with_binary(bin(), root, query, candidates, limit)
}

fn search_with_binary(
    binary: &str,
    root: &Path,
    query: &str,
    candidates: bool,
    limit: usize,
) -> Output {
    let limit = limit.to_string();
    let mut arguments = vec![
        "knowledge",
        "search",
        query,
        "--workspace",
        root.to_str().unwrap(),
        "--limit",
        &limit,
        "--json",
    ];
    if candidates {
        arguments.push("--candidates");
    }
    Command::new(binary).args(arguments).output().unwrap()
}

fn candidate_ids(root: &Path, query: &str) -> Vec<String> {
    let output = search(root, query, true, 5);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
        .unwrap()
        .into_iter()
        .filter_map(|item| item["id"].as_str().map(str::to_owned))
        .collect()
}

fn timed_search(binary: &str, root: &Path, query: &str, candidates: bool) -> Duration {
    let started = Instant::now();
    let output = search_with_binary(binary, root, query, candidates, 5);
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    elapsed
}

fn p95(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    let rank = (samples.len() * 95).div_ceil(100);
    samples[rank.saturating_sub(1)]
}

#[test]
fn representative_candidate_recall_and_no_match_precision() {
    let root = temp_dir("quality");
    assert!(command(&["init", root.to_str().unwrap()]).status.success());

    let long_context = "Logical and physical constraints remain distinct. ".repeat(40);
    let records = [
        (
            "wiki",
            "wiki:quantum-error-correction",
            "Quantum error correction",
            "Explains physical redundancy, logical qubits, and commercialization constraints.",
            "aliases: [QEC, 양자 오류 정정]\ntags: [quantum-computing, commercialization]\n",
            format!("# Mechanism\n\n{long_context}"),
        ),
        (
            "wiki",
            "wiki:advanced-packaging",
            "Semiconductor advanced packaging",
            "Explains chiplets, interposers, and high-bandwidth memory integration.",
            "aliases: [HBM packaging, 첨단 패키징]\ntags: [semiconductors, value-chain]\n",
            "# Value chain\n\nPackaging links foundries, memory, substrates, and equipment.".into(),
        ),
        (
            "lens",
            "lens:pricing-power-quality",
            "Pricing power quality",
            "Tests whether price realization survives volume pressure and competition.",
            "aliases: [가격 결정력]\ntags: [quality, margins]\n",
            "# Failure indicators\n\nUnit losses and rising churn can falsify the interpretation.".into(),
        ),
        (
            "lens",
            "lens:reverse-dcf",
            "Reverse DCF expectations review",
            "Infers the growth and margin expectations embedded in a market price.",
            "aliases: [implied expectations, 역산 DCF]\ntags: [valuation, scenario-analysis]\n",
            "# Procedure\n\nSolve for the operating assumptions implied by enterprise value.".into(),
        ),
        (
            "evidence",
            "evidence:atlas-cloud-capex-2026q2",
            "Atlas Cloud 2026 Q2 capex guidance",
            "Management retained full-year infrastructure spending guidance.",
            "source: https://example.test/atlas-q2\nperiod: 2026-Q2\nentities: [ticker:ATLS]\ntags: [cloud, capex]\n",
            "# Observation\n\nGuidance was unchanged at the information cutoff.".into(),
        ),
        (
            "decision",
            "decision:atlas-cloud-allocation",
            "Atlas Cloud allocation review",
            "Maintains the position while return-on-capital guardrails hold.",
            "entities: [ticker:ATLS]\ntags: [allocation, cloud]\n",
            "# Judgment\n\nHold within the stated risk budget.".into(),
        ),
        (
            "wiki",
            "wiki:japanese-operating-profit",
            "営業 利益の持続性",
            "再投資後の利益とコスト構造を評価する。",
            "aliases: [営業利益]\ntags: [収益性]\n",
            "# 判断\n\n利益率と再投資を比較する。".into(),
        ),
        (
            "wiki",
            "wiki:chinese-cash-conversion",
            "自由 现金流质量",
            "评估利润转换为现金的持续性。",
            "aliases: [自由现金流]\ntags: [现金转换]\n",
            "# 判断\n\n检查营运资本和再投资。".into(),
        ),
        (
            "wiki",
            "wiki:cyrillic-cash-flow",
            "Денежный Поток",
            "Проверяет устойчивость денежных потоков.",
            "aliases: [свободный денежный поток]\ntags: [качество прибыли]\n",
            "# Интерпретация\n\nСравнивает прибыль и денежный поток.".into(),
        ),
        (
            "lens",
            "lens:modified-duration",
            "Modified duration price sensitivity",
            "Estimates approximate bond-price sensitivity to a yield change.",
            "aliases: [수정 듀레이션]\ntags: [fixed-income, spread-risk]\n",
            "# Inputs\n\nUse yield, cash-flow timing, and the relevant spread shock.".into(),
        ),
        (
            "lens",
            "lens:commodity-basis-risk",
            "Commodity basis-risk lens",
            "Separates local inventory and delivery constraints from headline futures moves.",
            "aliases: [베이시스 위험]\ntags: [commodities, logistics]\n",
            "# Limits\n\nContract specification and delivery location must match.".into(),
        ),
        (
            "evidence",
            "evidence:eurusd-carry-window",
            "EUR/USD carry observation",
            "Records a positive carry differential over the reviewed daily window.",
            "source: https://example.test/eurusd\nperiod: 2026-07\nentities: [fx:EURUSD]\ntags: [fx, carry]\n",
            "# Observation\n\nThe result excludes transaction costs and funding constraints.".into(),
        ),
    ];
    let representative_section_context =
        "Additional reviewed context elaborates boundaries and caveats for future reuse. "
            .repeat(96);
    for (kind, id, title, summary, extra, body) in records {
        write_record(
            &root,
            kind,
            id,
            title,
            summary,
            extra,
            &format!("{body}\n\n{representative_section_context}"),
        );
    }

    let cases = [
        ("Quantum error correction", "wiki:quantum-error-correction"),
        ("QEC", "wiki:quantum-error-correction"),
        ("양자 오류 정정", "wiki:quantum-error-correction"),
        ("quantum commercialization", "wiki:quantum-error-correction"),
        (
            "wiki:quantum-error-correction",
            "wiki:quantum-error-correction",
        ),
        (
            "Semiconductor advanced packaging",
            "wiki:advanced-packaging",
        ),
        ("HBM packaging", "wiki:advanced-packaging"),
        ("첨단패키징", "wiki:advanced-packaging"),
        ("semiconductors value-chain", "wiki:advanced-packaging"),
        ("chiplets interposers", "wiki:advanced-packaging"),
        ("Pricing power quality", "lens:pricing-power-quality"),
        ("가격결정력", "lens:pricing-power-quality"),
        ("price realization volume", "lens:pricing-power-quality"),
        ("quality margins", "lens:pricing-power-quality"),
        ("rising churn", "lens:pricing-power-quality"),
        ("Reverse DCF expectations review", "lens:reverse-dcf"),
        ("implied expectations", "lens:reverse-dcf"),
        ("역산DCF", "lens:reverse-dcf"),
        ("valuation scenario-analysis", "lens:reverse-dcf"),
        ("enterprise value assumptions", "lens:reverse-dcf"),
        (
            "Atlas Cloud 2026 Q2 capex guidance",
            "evidence:atlas-cloud-capex-2026q2",
        ),
        ("ticker:ATLS 2026-Q2", "evidence:atlas-cloud-capex-2026q2"),
        ("cloud capex", "evidence:atlas-cloud-capex-2026q2"),
        (
            "full-year infrastructure spending",
            "evidence:atlas-cloud-capex-2026q2",
        ),
        (
            "evidence:atlas-cloud-capex-2026q2",
            "evidence:atlas-cloud-capex-2026q2",
        ),
        (
            "Atlas Cloud allocation review",
            "decision:atlas-cloud-allocation",
        ),
        ("ticker:ATLS allocation", "decision:atlas-cloud-allocation"),
        ("risk budget", "decision:atlas-cloud-allocation"),
        (
            "return-on-capital guardrails",
            "decision:atlas-cloud-allocation",
        ),
        (
            "decision:atlas-cloud-allocation",
            "decision:atlas-cloud-allocation",
        ),
        ("営業利益", "wiki:japanese-operating-profit"),
        ("営業利益の持続性", "wiki:japanese-operating-profit"),
        ("再投資後の利益", "wiki:japanese-operating-profit"),
        ("収益性", "wiki:japanese-operating-profit"),
        (
            "wiki:japanese-operating-profit",
            "wiki:japanese-operating-profit",
        ),
        ("自由现金流", "wiki:chinese-cash-conversion"),
        ("自由现金流质量", "wiki:chinese-cash-conversion"),
        ("现金转换", "wiki:chinese-cash-conversion"),
        ("营运资本 再投资", "wiki:chinese-cash-conversion"),
        (
            "wiki:chinese-cash-conversion",
            "wiki:chinese-cash-conversion",
        ),
        ("ДЕНЕЖНЫЙ ПОТОК", "wiki:cyrillic-cash-flow"),
        ("свободный денежный поток", "wiki:cyrillic-cash-flow"),
        ("качество прибыли", "wiki:cyrillic-cash-flow"),
        ("прибыль денежный", "wiki:cyrillic-cash-flow"),
        ("wiki:cyrillic-cash-flow", "wiki:cyrillic-cash-flow"),
        (
            "Modified duration price sensitivity",
            "lens:modified-duration",
        ),
        ("수정듀레이션", "lens:modified-duration"),
        ("fixed-income spread-risk", "lens:modified-duration"),
        ("bond-price yield change", "lens:modified-duration"),
        ("lens:modified-duration", "lens:modified-duration"),
        ("Commodity basis-risk lens", "lens:commodity-basis-risk"),
        ("베이시스위험", "lens:commodity-basis-risk"),
        ("commodities logistics", "lens:commodity-basis-risk"),
        (
            "inventory delivery constraints",
            "lens:commodity-basis-risk",
        ),
        ("lens:commodity-basis-risk", "lens:commodity-basis-risk"),
        ("EUR/USD carry observation", "evidence:eurusd-carry-window"),
        ("fx:EURUSD 2026-07", "evidence:eurusd-carry-window"),
        ("fx carry", "evidence:eurusd-carry-window"),
        ("transaction costs funding", "evidence:eurusd-carry-window"),
        (
            "evidence:eurusd-carry-window",
            "evidence:eurusd-carry-window",
        ),
    ];

    let hits = cases
        .iter()
        .filter(|(query, expected)| candidate_ids(&root, query).iter().any(|id| id == expected))
        .count();
    assert!(
        hits * 100 >= cases.len() * 95,
        "Recall@5 was {hits}/{}",
        cases.len()
    );

    let no_match_queries = [
        "orbital potato yield",
        "marine insurance salvage",
        "lithium brine evaporation",
        "municipal tax lien",
        "satellite spectrum auction",
        "biotech trial enrollment",
        "aircraft lease residual",
        "forest carbon registry",
        "steel scrap premium",
        "container demurrage index",
        "uranium conversion outage",
        "mortgage prepayment convexity",
        "sovereign election polling",
        "water utility leakage",
        "rare earth separation",
        "pharmaceutical rebate",
        "cocoa grinding ratio",
        "grid interconnection backlog",
        "shipping canal closure",
        "fertilizer ammonia benchmark",
    ];
    let false_positives = no_match_queries
        .iter()
        .filter(|query| !candidate_ids(&root, query).is_empty())
        .count();
    assert!(
        false_positives * 100 <= no_match_queries.len() * 5,
        "no-match false positives were {false_positives}/{}",
        no_match_queries.len()
    );

    let baseline_binary = std::env::var("GURUTERMINAL_RETRIEVAL_BASELINE_BIN").ok();
    let full_binary = baseline_binary.as_deref().unwrap_or(bin());
    let (full_bytes, compact_bytes) = cases.iter().fold(
        (0_usize, 0_usize),
        |(full_total, compact_total), (query, _)| {
            let full = search_with_binary(full_binary, &root, query, false, 5);
            let compact = search(&root, query, true, 5);
            assert!(full.status.success());
            assert!(compact.status.success());
            (
                full_total + full.stdout.len(),
                compact_total + compact.stdout.len(),
            )
        },
    );
    if baseline_binary.is_some() {
        assert!(
            compact_bytes * 100 <= full_bytes * 40,
            "candidate payload averaged {} bytes versus {} baseline full bytes across {} queries",
            compact_bytes / cases.len(),
            full_bytes / cases.len(),
            cases.len()
        );
    } else {
        assert!(
            compact_bytes < full_bytes,
            "candidate payload averaged {} bytes versus {} current full bytes across {} queries",
            compact_bytes / cases.len(),
            full_bytes / cases.len(),
            cases.len()
        );
    }
    eprintln!(
        "Recall@5 {hits}/{}, no-match false positives {false_positives}/{}, candidate payload {} versus {} {} full bytes on average",
        cases.len(),
        no_match_queries.len(),
        compact_bytes / cases.len(),
        full_bytes / cases.len(),
        if baseline_binary.is_some() {
            "baseline"
        } else {
            "current"
        }
    );

    let compact = search(&root, "physical constraints", true, 5);
    assert!(compact.status.success());
    let candidate_json: Vec<serde_json::Value> = serde_json::from_slice(&compact.stdout).unwrap();
    assert!(candidate_json[0].get("text").is_none());
    for omitted in ["path", "tags", "relationships"] {
        assert!(
            candidate_json[0].get(omitted).is_none(),
            "candidate cards must not duplicate exact-read field {omitted}"
        );
    }
    assert!(candidate_json[0].get("match_tier").is_some());
    assert!(candidate_json[0].get("matched_fields").is_some());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn five_hundred_document_search_stays_bounded() {
    let root = temp_dir("latency");
    assert!(command(&["init", root.to_str().unwrap()]).status.success());
    for index in 0..500 {
        write_record(
            &root,
            "wiki",
            &format!("wiki:synthetic-{index:03}"),
            &format!("Synthetic operating constraint {index:03}"),
            &format!("A deterministic corpus record for mechanism {index:03}."),
            &format!("aliases: [fixture-{index:03}]\ntags: [synthetic, mechanism]\n"),
            &format!(
                "# Mechanism {index:03}\n\nConstraint {index:03} has a distinct boundary condition."
            ),
        );
    }

    let queries = [
        "fixture-000",
        "fixture-055",
        "fixture-111",
        "fixture-166",
        "fixture-222",
        "fixture-277",
        "fixture-333",
        "fixture-388",
        "fixture-444",
        "fixture-499",
    ];
    let baseline_binary = std::env::var("GURUTERMINAL_RETRIEVAL_BASELINE_BIN").ok();
    timed_search(bin(), &root, queries[0], true);
    if let Some(baseline_binary) = baseline_binary {
        timed_search(&baseline_binary, &root, queries[0], false);
        let mut candidate_samples = Vec::new();
        let mut baseline_samples = Vec::new();
        for round in 0..3 {
            for (index, query) in queries.iter().enumerate() {
                if (round + index) % 2 == 0 {
                    candidate_samples.push(timed_search(bin(), &root, query, true));
                    baseline_samples.push(timed_search(&baseline_binary, &root, query, false));
                } else {
                    baseline_samples.push(timed_search(&baseline_binary, &root, query, false));
                    candidate_samples.push(timed_search(bin(), &root, query, true));
                }
            }
        }
        let candidate_p95 = p95(candidate_samples);
        let baseline_p95 = p95(baseline_samples);
        eprintln!(
            "500-document warmed/interleaved search p95: candidate {candidate_p95:?}, baseline {baseline_p95:?}"
        );
        assert!(
            candidate_p95.as_nanos() * 100 <= baseline_p95.as_nanos() * 120,
            "candidate search p95 {candidate_p95:?} exceeded baseline {baseline_p95:?} by more than 20%"
        );
    } else {
        let candidate_p95 = p95(queries
            .iter()
            .map(|query| timed_search(bin(), &root, query, true))
            .collect());
        assert!(
            candidate_p95 < Duration::from_secs(2),
            "500-document candidate search p95 was {candidate_p95:?}"
        );
    }

    fs::remove_dir_all(root).unwrap();
}
