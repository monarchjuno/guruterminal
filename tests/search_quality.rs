use std::{fs, path::Path, process::Output};

mod common;

fn temp_dir(label: &str) -> std::path::PathBuf {
    common::temp_dir("search", label)
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
        "2026-07-29T09:00:00+09:00",
    );
}

fn search(root: &Path, query: &str) -> Vec<serde_json::Value> {
    let output = command(&[
        "knowledge",
        "search",
        query,
        "--workspace",
        root.to_str().unwrap(),
        "--limit",
        "50",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn candidate_search(root: &Path, query: &str) -> Vec<serde_json::Value> {
    let output = command(&[
        "knowledge",
        "search",
        query,
        "--workspace",
        root.to_str().unwrap(),
        "--limit",
        "50",
        "--candidates",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn result<'a>(results: &'a [serde_json::Value], id: &str) -> Option<&'a serde_json::Value> {
    results.iter().find(|item| item["id"] == id)
}

fn assert_top(root: &Path, query: &str, expected: &str) {
    assert_eq!(
        search(root, query)
            .first()
            .and_then(|item| item["id"].as_str()),
        Some(expected),
        "unexpected top result for {query:?}"
    );
}

#[test]
fn ranking_context_and_relationship_recall_regression() {
    let root = temp_dir("ranking");
    assert!(command(&["init", root.to_str().unwrap()]).status.success());

    write_record(
        &root,
        "wiki",
        "wiki:cash-conversion-durability",
        "Cash conversion durability",
        "Tests whether accounting earnings become distributable cash.",
        "",
        "# Evidence quality\n\nFree cash flow should persist after normalized reinvestment.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:liquidity-signals",
        "Free flow and cash checklist",
        "Reviews cash timing, free float, and transaction flow.",
        "tags: [free, flow, cash]\n",
        "# Trading liquidity\n\nMarket depth and turnover conventions.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:net-revenue-retention",
        "Net revenue retention",
        "Separates expansion, contraction, and churn.",
        "aliases: [net dollar retention, NRR]\n",
        "# Interpretation\n\nCohort revenue retained after expansion and churn.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:operating-profit-resilience",
        "영업 이익 회복력",
        "영업 이익이 매출 충격 이후 회복되는 조건.",
        "",
        "# 판단 기준\n\n영업 이익의 회복 속도와 비용 구조를 함께 본다.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:capital-allocation-guardrail",
        "Capital allocation guardrail",
        "Requires returns above the cost of capital.",
        "",
        "",
    );
    write_record(
        &root,
        "wiki",
        "wiki:pricing-power-framework",
        "Pricing power framework",
        "Evaluates durable price realization.",
        "",
        "# Power history\n\nHistorical examples.\n\n# Appendix\n\nFormatting conventions.",
    );
    write_record(
        &root,
        "decision",
        "decision:original",
        "Original margin review",
        "Initial operating margin judgment.",
        "",
        "# Decision\n\nHold while unit economics remain intact.",
    );
    write_record(
        &root,
        "decision",
        "decision:revision",
        "Revised margin review",
        "Later judgment after costs increased.",
        "updates: [decision:original]\n",
        "# Decision\n\nReduce exposure after the margin break.",
    );

    assert_top(&root, "free cash flow", "wiki:cash-conversion-durability");
    assert_top(&root, "net dollar retention", "wiki:net-revenue-retention");
    assert_top(&root, "영업이익", "wiki:operating-profit-resilience");
    assert_top(&root, "decision:original", "decision:original");
    assert!(result(&search(&root, "decision:original"), "decision:revision").is_some());

    for (query, id) in [
        (
            "capital allocation guardrail",
            "wiki:capital-allocation-guardrail",
        ),
        ("pricing power framework", "wiki:pricing-power-framework"),
    ] {
        let results = search(&root, query);
        let item = result(&results, id).unwrap();
        assert_eq!(item["section"], "");
        assert_eq!(item["heading_path"], serde_json::json!([]));
    }
    assert!(!search(&root, "pricing power framework")[0]["text"]
        .as_str()
        .unwrap()
        .contains("Historical examples"));
    assert!(search(&root, "cocoa basis risk").is_empty());
    assert!(search(&root, "가격 전가 능력").is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn precision_numeric_and_multilingual_regression() {
    let root = temp_dir("precision");
    assert!(command(&["init", root.to_str().unwrap()]).status.success());

    write_record(
        &root,
        "wiki",
        "wiki:model-infrastructure",
        "AI infrastructure capex",
        "Tracks accelerator and data-center investment.",
        "",
        "# Capacity\n\nAI accelerator deployments are expanding.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:subscriber-growth",
        "Paid subscriber growth",
        "Tracks subscription additions and churn.",
        "",
        "# Cohorts\n\nSubscriber retention by cohort.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:earnings-quality",
        "Earnings quality",
        "Compares accounting earnings with cash generation.",
        "",
        "# Accounting\n\nNet income should be reconciled with cash flow.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:internet-business",
        "Internet income trends",
        "Reviews digital subscription economics.",
        "",
        "# Business model\n\nDigital distribution and subscriber acquisition.",
    );
    write_record(
        &root,
        "evidence",
        "evidence:margin-change",
        "Operating margin update",
        "Operating margin changed +5.0%.",
        "source: https://example.com/margin\n",
        "# Observation\n\nThe reported margin improved.",
    );
    write_record(
        &root,
        "evidence",
        "evidence:unsigned-margin",
        "Unsigned margin notation",
        "The table reports a 5.0% margin without a direction sign.",
        "source: https://example.com/unsigned-margin\n",
        "# Observation\n\nThe source does not state a direction.",
    );
    write_record(
        &root,
        "evidence",
        "evidence:forecast-range",
        "Forecast period",
        "Forecast covers 2025-2026.",
        "source: https://example.com/forecast-range\n",
        "# Period\n\nThe forecast spans two calendar years.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:japanese-profit",
        "営業 利益の持続性",
        "利益の質を評価する。",
        "",
        "# 判断\n\n再投資後の利益を確認する。",
    );
    write_record(
        &root,
        "wiki",
        "wiki:chinese-cash-flow",
        "自由 现金流质量",
        "评估现金转换的持续性。",
        "",
        "# 判断\n\n检查再投资后的现金流。",
    );
    write_record(
        &root,
        "wiki",
        "wiki:cyrillic-cash-flow",
        "Денежный Поток",
        "Проверяет устойчивость денежных потоков.",
        "",
        "# Интерпретация\n\nСравнивает прибыль и денежный поток.",
    );

    let ai = search(&root, "ai");
    assert_eq!(ai[0]["id"], "wiki:model-infrastructure");
    assert!(result(&ai, "wiki:subscriber-growth").is_none());
    assert_top(&root, "net income", "wiki:earnings-quality");

    assert_top(&root, "+5.0%", "evidence:margin-change");
    assert!(search(&root, "-5.0%").is_empty());
    assert!(search(&root, "−5.0%").is_empty());
    assert!(search(&root, "+5.1%").is_empty());
    assert_top(&root, "5.0%", "evidence:unsigned-margin");
    assert_top(&root, "2025-2026", "evidence:forecast-range");
    assert!(search(&root, "-2026").is_empty());

    assert_top(&root, "営業利益", "wiki:japanese-profit");
    assert_top(&root, "自由现金流", "wiki:chinese-cash-flow");
    assert_top(&root, "ДЕНЕЖНЫЙ ПОТОК", "wiki:cyrillic-cash-flow");
    assert_eq!(search(&root, "ai"), search(&root, "  ai  "));

    let blank = command(&[
        "knowledge",
        "search",
        " \t ",
        "--workspace",
        root.to_str().unwrap(),
        "--json",
    ]);
    assert!(!blank.status.success());
    assert!(String::from_utf8_lossy(&blank.stderr).contains("query must not be empty"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn authored_anchors_recover_verbose_queries_without_loosening_precision() {
    let root = temp_dir("authored-anchor");
    assert!(command(&["init", root.to_str().unwrap()]).status.success());

    write_record(
        &root,
        "wiki",
        "wiki:cash-before-working-capital",
        "Cash before working capital",
        "Explains recurring operating conversion before working-capital movements.",
        "entities: [NYSE:ACME]\naliases: [CBWC]\n",
        "# Interpretation\n\nCurrency translation effects are separated from recurring conversion.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:translation-review",
        "Currency translation cash generation review",
        "Reviews cash generation after currency translation effects.",
        "",
        "# Review\n\nTranslation and cash generation are compared.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:body-only",
        "Inventory cycle notes",
        "Describes a conventional inventory cycle.",
        "",
        "# Detail\n\nCash appears once in an otherwise unrelated body.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:model-infrastructure-anchor",
        "Model infrastructure",
        "Describes accelerator deployment.",
        "aliases: [AI]\n",
        "# Capacity\n\nAccelerator clusters are expanding.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:positive-margin-anchor",
        "Positive margin observation",
        "The observed margin change was +5.0%.",
        "aliases: [MARGIN]\n",
        "# Observation\n\nThe reported change was positive.",
    );

    let korean = candidate_search(&root, "CBWC 환산 효과 현금 창출");
    let korean_target = result(&korean, "wiki:cash-before-working-capital").unwrap();
    assert_eq!(korean[0]["id"], "wiki:cash-before-working-capital");
    assert_eq!(korean_target["section"], "");
    assert_eq!(korean_target["match_tier"], "partial");
    assert!(korean_target["matched_fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field == "aliases"));

    let verbose = candidate_search(&root, "CBWC currency translation effect cash generation");
    assert_eq!(verbose[0]["id"], "wiki:cash-before-working-capital");
    assert_eq!(verbose[0]["section"], "");
    assert_eq!(verbose[0]["match_tier"], "partial");

    for query in [
        "Please revisit wiki:cash-before-working-capital before the meeting",
        "What changed for NYSE:ACME after the release",
    ] {
        let candidates = candidate_search(&root, query);
        assert_eq!(candidates[0]["id"], "wiki:cash-before-working-capital");
        assert_eq!(candidates[0]["section"], "");
        assert_eq!(candidates[0]["match_tier"], "partial");
    }

    assert!(result(
        &search(&root, "please explain cash tornado"),
        "wiki:body-only"
    )
    .is_none());
    assert!(result(
        &search(&root, "paid subscriber discussion"),
        "wiki:model-infrastructure-anchor"
    )
    .is_none());
    assert!(result(
        &search(&root, "MARGIN changed -5.0%"),
        "wiki:positive-margin-anchor"
    )
    .is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn paraphrase_recall_ranks_packaging_above_coffee_and_keeps_alias_hits() {
    let root = temp_dir("semantic-recall");
    assert!(command(&["init", root.to_str().unwrap()]).status.success());

    write_record(
        &root,
        "wiki",
        "wiki:ai-advanced-packaging",
        "AI accelerator advanced packaging bottleneck",
        "CoWoS-class packaging, not leading-edge wafers, is the binding constraint for AI accelerator supply.",
        "aliases: [HBM packaging]\nentities: [TSMC, CoWoS]\ntags: [semiconductors, packaging]\n",
        "# Advanced packaging\n\nCoWoS-class capacity remains the industry bottleneck for AI accelerators. Later peer analysis should inspect packaging allocation before assuming wafer starts translate into shipments.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:coffee-unit-economics",
        "Specialty coffee unit economics",
        "Store-level contribution after rent and labor for cafe chains.",
        "entities: [CoffeeCo]\n",
        "# Specialty coffee\n\nRent, barista hours, and average ticket explain store contribution. This prior is about cafe chains, not listed-company financial statements.",
    );
    write_record(
        &root,
        "wiki",
        "wiki:samsung-capital-allocation",
        "Samsung Electronics capital allocation",
        "Returns excess cash through dividends and buybacks only after foundry and memory reinvestment.",
        "aliases: [삼성전자, Samsung Electronics, Samsung]\nentities: [Samsung Electronics, 005930.KS]\n",
        "# Capital allocation\n\nSamsung Electronics funds advanced foundry and memory capacity first. Shareholder returns are residual after those reinvestment needs, not a fixed payout rule.",
    );

    let paraphrase = search(&root, "How should I analyze NVIDIA accelerator shipments?");
    let packaging = result(&paraphrase, "wiki:ai-advanced-packaging");
    assert!(
        packaging.is_some(),
        "paraphrase missed the learned packaging wiki: {paraphrase:?}"
    );
    let coffee_rank = paraphrase
        .iter()
        .position(|item| item["id"] == "wiki:coffee-unit-economics");
    let packaging_rank = paraphrase
        .iter()
        .position(|item| item["id"] == "wiki:ai-advanced-packaging")
        .unwrap();
    assert!(
        coffee_rank.is_none_or(|rank| rank > packaging_rank),
        "coffee wiki ranked above packaging for an accelerator paraphrase: {paraphrase:?}"
    );

    let packaging_query = search(&root, "AI accelerator packaging for foundry supply");
    assert_eq!(
        packaging_query.first().and_then(|item| item["id"].as_str()),
        Some("wiki:ai-advanced-packaging"),
        "coffee wiki ranked above the foundry/packaging wiki: {packaging_query:?}"
    );

    assert_top(
        &root,
        "What did this Guru learn about 삼성전자 capital allocation?",
        "wiki:samsung-capital-allocation",
    );
    assert_top(
        &root,
        "Samsung Electronics shareholder returns after foundry reinvestment",
        "wiki:samsung-capital-allocation",
    );

    fs::remove_dir_all(root).unwrap();
}
