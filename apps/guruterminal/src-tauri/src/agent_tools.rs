/// Single source of Pi tool names and their broker methods.
/// Product-contract tests keep this table aligned with the extension allowlist,
/// broker parser, and Chat progress presentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentToolSpec {
    pub pi_name: &'static str,
    pub broker_method: Option<&'static str>,
}

pub const AGENT_TOOLS: &[AgentToolSpec] = &[
    AgentToolSpec {
        pi_name: "read",
        broker_method: Some("workbench.read"),
    },
    AgentToolSpec {
        pi_name: "write",
        broker_method: Some("workbench.write"),
    },
    AgentToolSpec {
        pi_name: "edit",
        broker_method: Some("workbench.edit"),
    },
    AgentToolSpec {
        pi_name: "ls",
        broker_method: Some("workbench.ls"),
    },
    AgentToolSpec {
        pi_name: "find",
        broker_method: Some("workbench.find"),
    },
    AgentToolSpec {
        pi_name: "grep",
        broker_method: Some("workbench.grep"),
    },
    AgentToolSpec {
        pi_name: "capability_search",
        broker_method: None,
    },
    AgentToolSpec {
        pi_name: "capability_load",
        broker_method: None,
    },
    AgentToolSpec {
        pi_name: "memory_search",
        broker_method: Some("guru.search"),
    },
    AgentToolSpec {
        pi_name: "memory_read",
        broker_method: Some("guru.read"),
    },
    AgentToolSpec {
        pi_name: "memory_previous",
        broker_method: Some("guru.read_previous"),
    },
    AgentToolSpec {
        pi_name: "run_results_list",
        broker_method: Some("run.results.list"),
    },
    AgentToolSpec {
        pi_name: "finance_sources",
        broker_method: Some("finance.sources"),
    },
    AgentToolSpec {
        pi_name: "finance_macro_data",
        broker_method: Some("finance.macro_data"),
    },
    AgentToolSpec {
        pi_name: "finance_market_data",
        broker_method: Some("finance.market_data"),
    },
    AgentToolSpec {
        pi_name: "finance_company_data",
        broker_method: Some("finance.company_data"),
    },
    AgentToolSpec {
        pi_name: "finance_filings",
        broker_method: Some("finance.filings"),
    },
    AgentToolSpec {
        pi_name: "finance_calculate",
        broker_method: Some("finance.calculate"),
    },
    AgentToolSpec {
        pi_name: "finance_resolve_entity",
        broker_method: Some("finance.resolve_entity"),
    },
    AgentToolSpec {
        pi_name: "compute_run",
        broker_method: Some("compute.run"),
    },
    AgentToolSpec {
        pi_name: "web_search",
        broker_method: Some("web.search"),
    },
    AgentToolSpec {
        pi_name: "web_fetch",
        broker_method: Some("web.fetch"),
    },
    AgentToolSpec {
        pi_name: "artifact_list",
        broker_method: Some("artifact.list"),
    },
    AgentToolSpec {
        pi_name: "artifact_read",
        broker_method: Some("artifact.read"),
    },
    AgentToolSpec {
        pi_name: "artifact_publish",
        broker_method: Some("artifact.publish"),
    },
    AgentToolSpec {
        pi_name: "chart_query",
        broker_method: Some("chart.query"),
    },
    AgentToolSpec {
        pi_name: "chart_publish",
        broker_method: Some("chart.publish"),
    },
    AgentToolSpec {
        pi_name: "decision_submit",
        broker_method: Some("decision.submit"),
    },
    AgentToolSpec {
        pi_name: "evidence_create",
        broker_method: Some("evidence.create"),
    },
    AgentToolSpec {
        pi_name: "memory_patch_propose",
        broker_method: Some("memory.patch.propose"),
    },
];

pub fn pi_tool_names() -> impl Iterator<Item = &'static str> {
    AGENT_TOOLS.iter().map(|tool| tool.pi_name)
}
