use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const RESEARCH_SKILL_ID: &str = "research";
pub const WIKI_SKILL_ID: &str = "wiki";
pub const LENS_SKILL_ID: &str = "lens";
pub const DECISION_SKILL_ID: &str = "decision";
pub const VALUATION_SKILL_ID: &str = "valuation";
pub const COMPARISON_SKILL_ID: &str = "comparison";
pub const RULE_TEST_SKILL_ID: &str = "rule-test";
pub const FILINGS_SKILL_ID: &str = "filings";
pub use crate::user_skill::USER_SKILL_PROVENANCE_BANNER;

const HARNESS_SCHEMA: &str = "guruterminal-harness/1";
const RUNTIME_SCHEMA: &str = "guruterminal-agent-runtime/1";
const SKILL_BINDING_PREFIX: &str = "skill.";
const MAX_ACTIVE_SKILLS: usize = 64;
const EXTENSION_ENTRYPOINT: &str = "guruterminal-extension.mjs";
const PROVIDER_EXTENSION_ENTRYPOINT: &str = "guruterminal-provider-extension.mjs";
const EXTENSION_FILES: &[(&str, &[u8])] = &[
    (
        EXTENSION_ENTRYPOINT,
        include_bytes!("../../agent/guruterminal-extension.mjs"),
    ),
    (
        "broker-client.mjs",
        include_bytes!("../../agent/broker-client.mjs"),
    ),
    (
        "workbench-tools.mjs",
        include_bytes!("../../agent/workbench-tools.mjs"),
    ),
    (
        "model-run-controls.mjs",
        include_bytes!("../../agent/model-run-controls.mjs"),
    ),
    (
        "guruterminal-native-search.mjs",
        include_bytes!("../../agent/guruterminal-native-search.mjs"),
    ),
    (
        "native-search/common.mjs",
        include_bytes!("../../agent/native-search/common.mjs"),
    ),
    (
        "native-search/codex.mjs",
        include_bytes!("../../agent/native-search/codex.mjs"),
    ),
    (
        "native-search/anthropic.mjs",
        include_bytes!("../../agent/native-search/anthropic.mjs"),
    ),
    (
        "native-search/xai.mjs",
        include_bytes!("../../agent/native-search/xai.mjs"),
    ),
];
const PROVIDER_EXTENSION_FILES: &[(&str, &[u8])] = &[
    (
        PROVIDER_EXTENSION_ENTRYPOINT,
        include_bytes!("../../agent/guruterminal-provider-extension.mjs"),
    ),
    (
        "guruterminal-native-search.mjs",
        include_bytes!("../../agent/guruterminal-native-search.mjs"),
    ),
    (
        "native-search/common.mjs",
        include_bytes!("../../agent/native-search/common.mjs"),
    ),
    (
        "native-search/codex.mjs",
        include_bytes!("../../agent/native-search/codex.mjs"),
    ),
    (
        "native-search/anthropic.mjs",
        include_bytes!("../../agent/native-search/anthropic.mjs"),
    ),
    (
        "native-search/xai.mjs",
        include_bytes!("../../agent/native-search/xai.mjs"),
    ),
];

const SELECTABLE_SKILLS: &[BundledSkill] = &[
    BundledSkill {
        id: RESEARCH_SKILL_ID,
        name: "Research",
        relative_path: "skills/research/SKILL.md",
        content: include_bytes!("../../agent/skills/research/SKILL.md"),
    },
    BundledSkill {
        id: WIKI_SKILL_ID,
        name: "Wiki",
        relative_path: "skills/wiki/SKILL.md",
        content: include_bytes!("../../agent/skills/wiki/SKILL.md"),
    },
    BundledSkill {
        id: LENS_SKILL_ID,
        name: "Lens",
        relative_path: "skills/lens/SKILL.md",
        content: include_bytes!("../../agent/skills/lens/SKILL.md"),
    },
    BundledSkill {
        id: DECISION_SKILL_ID,
        name: "Decision",
        relative_path: "skills/decision/SKILL.md",
        content: include_bytes!("../../agent/skills/decision/SKILL.md"),
    },
];

/// Method cards always passed to Pi. They are not catalogued, mentionable, or
/// toggleable. Load them only when the advertised description matches.
const ALWAYS_ON_SKILLS: &[BundledSkill] = &[
    BundledSkill {
        id: VALUATION_SKILL_ID,
        name: "Valuation",
        relative_path: "skills/valuation/SKILL.md",
        content: include_bytes!("../../agent/skills/valuation/SKILL.md"),
    },
    BundledSkill {
        id: COMPARISON_SKILL_ID,
        name: "Comparison",
        relative_path: "skills/comparison/SKILL.md",
        content: include_bytes!("../../agent/skills/comparison/SKILL.md"),
    },
    BundledSkill {
        id: RULE_TEST_SKILL_ID,
        name: "Rule test",
        relative_path: "skills/rule-test/SKILL.md",
        content: include_bytes!("../../agent/skills/rule-test/SKILL.md"),
    },
    BundledSkill {
        id: FILINGS_SKILL_ID,
        name: "Filings",
        relative_path: "skills/filings/SKILL.md",
        content: include_bytes!("../../agent/skills/filings/SKILL.md"),
    },
];

#[derive(Clone, Copy)]
struct BundledSkill {
    id: &'static str,
    name: &'static str,
    relative_path: &'static str,
    content: &'static [u8],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentSkillSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub ownership: String,
    pub editable: bool,
    pub current_revision_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHarnessSnapshot {
    pub schema: String,
    pub mode: String,
    pub skill_ids: Vec<String>,
    pub user_skills: Vec<UserSkillSnapshot>,
    pub capability_ids: Vec<String>,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UserSkillSnapshot {
    pub id: String,
    pub revision_id: String,
    pub content_sha256: String,
}

/// The exact Pi surface for one Chat run. Core tools start active. Bundled
/// component tools are registered with Pi but remain inactive until the agent
/// discovers and loads that component through the capability tools. Rust derives
/// both sets from this Chat run and the Guru's sealed authority snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRuntimeProfile {
    pub schema: String,
    pub mode: String,
    /// Exact enabled capability bindings represented by this run profile.
    /// Components may aggregate several providers behind one semantic tool.
    pub capability_ids: Vec<String>,
    pub core_tool_names: Vec<String>,
    pub components: Vec<AgentRuntimeComponent>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRuntimeComponent {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    pub name: String,
    pub description: String,
    pub tool_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_ids: Vec<String>,
}

impl AgentRuntimeProfile {
    pub fn new(
        mode: &str,
        use_memory: bool,
        propose_memory_updates: bool,
        capability_ids: &[String],
    ) -> Result<Self, AgentHarnessError> {
        if mode != "chat" {
            return Err(AgentHarnessError::InvalidContext);
        }
        let available_capabilities = normalize_capability_ids(capability_ids)?;
        let capabilities = available_capabilities;
        let mut core_tools = Vec::new();
        let mut add_core = |name: &str| core_tools.push(name.to_owned());

        // Pi's built-in tools are disabled. This read surface is required for
        // progressive Skill loading; mutation and discovery are Chat-only.
        add_core("read");
        for name in ["write", "edit", "ls", "find", "grep"] {
            add_core(name);
        }
        add_core("run_results_list");
        if use_memory {
            add_core("memory_search");
            add_core("memory_read");
            add_core("memory_previous");
        }
        let mut components = Vec::new();
        for capability_id in &capabilities {
            components.extend(runtime_components(capability_id));
        }
        components.extend(finance_provider_components(&capabilities));
        components.extend(mcp_runtime_components(&capabilities)?);
        for name in [
            "artifact_list",
            "artifact_read",
            "artifact_publish",
            "decision_submit",
            "evidence_create",
        ] {
            add_core(name);
        }
        components.push(AgentRuntimeComponent {
            id: "guruterminal.charting/authoring".into(),
            kind: "tool".into(),
            server_id: None,
            name: "Chart authoring".into(),
            description: "Publish charts with built-in indicators and persisted drawing overlays from run-local data references, and inspect only bounded row windows when exact values are needed.".into(),
            tool_names: vec!["chart_query".into(), "chart_publish".into()],
            provider_ids: Vec::new(),
        });
        if !components.is_empty() {
            add_core("capability_search");
            add_core("capability_load");
        }
        if propose_memory_updates {
            add_core("memory_patch_propose");
        }

        Ok(Self {
            schema: RUNTIME_SCHEMA.to_owned(),
            mode: mode.to_owned(),
            capability_ids: capabilities,
            core_tool_names: core_tools,
            components,
        })
    }

    pub fn validate(&self) -> Result<(), AgentHarnessError> {
        let expected = Self::new(
            &self.mode,
            self.core_tool_names
                .iter()
                .any(|name| name == "memory_read"),
            self.core_tool_names
                .iter()
                .any(|name| matches!(name.as_str(), "memory_patch_propose")),
            &self.capability_ids,
        )?;
        if self.schema != RUNTIME_SCHEMA || &expected != self {
            return Err(AgentHarnessError::InvalidContext);
        }
        Ok(())
    }
}

type RuntimeComponentSpec = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

fn runtime_components(capability_id: &str) -> Vec<AgentRuntimeComponent> {
    let specifications: &[RuntimeComponentSpec] = match capability_id {
        "community.web-research" => &[(
            "community.web-research/research",
            "Public web research",
            "Search the public web and materialize exact source pages. Use fetched pages for the requested analysis; do not withhold an answer only because the source is public web.",
            &["web_search", "web_fetch"],
            &["community.web-research"],
        )],
        "guruterminal.compute-python" => &[(
            "guruterminal.compute-python/python",
            "Sandboxed compute",
            "Run bounded offline Python or JavaScript analysis with a reproducibility receipt. Prefer javascript unless a listed Python package is required; adding Python packages restarts the sandbox.",
            &["compute_run"],
            &["guruterminal.compute-python"],
        )],
        "guruterminal.finance-core" => &[
            (
                "guruterminal.finance-core/source-catalog",
                "Finance source catalog",
                "Inspect the finance sources installed in this Guru Terminal build.",
                &["finance_sources"],
                &["guruterminal.finance-core"],
            ),
            (
                "guruterminal.finance-core/calculations",
                "Finance calculations",
                "Run deterministic finance calculations with reproducibility receipts.",
                &["finance_calculate"],
                &["guruterminal.finance-core"],
            ),
        ],
        _ => &[],
    };
    specifications
        .iter()
        .map(
            |(id, name, description, tool_names, provider_ids)| AgentRuntimeComponent {
                id: (*id).to_owned(),
                kind: "tool".to_owned(),
                server_id: None,
                name: (*name).to_owned(),
                description: (*description).to_owned(),
                tool_names: tool_names.iter().map(|tool| (*tool).to_owned()).collect(),
                provider_ids: provider_ids.iter().map(|id| (*id).to_owned()).collect(),
            },
        )
        .collect()
}

fn finance_provider_components(capabilities: &[String]) -> Vec<AgentRuntimeComponent> {
    let enabled = |ids: &[&str]| {
        ids.iter()
            .filter(|id| capabilities.iter().any(|enabled| enabled == **id))
            .map(|id| (*id).to_owned())
            .collect::<Vec<_>>()
    };
    let mut components = Vec::new();
    let mut add =
        |id: &str, name: &str, description: &str, tools: &[&str], providers: &[String]| {
            components.push(AgentRuntimeComponent {
                id: id.to_owned(),
                kind: "tool".to_owned(),
                server_id: None,
                name: name.to_owned(),
                description: description.to_owned(),
                tool_names: tools.iter().map(|tool| (*tool).to_owned()).collect(),
                provider_ids: providers.to_vec(),
            });
        };
    let macro_providers = enabled(&["world-bank.indicators"]);
    if !macro_providers.is_empty() {
        add(
            "guruterminal.finance-providers/macro-data",
            "Structured macro data",
            &format!(
                "Fetch macro series from enabled providers: {}.",
                macro_providers.join(", ")
            ),
            &["finance_macro_data"],
            &macro_providers,
        );
    }
    let market_providers = enabled(&["krx.market-data", "koreainvestment.market-data"]);
    if !market_providers.is_empty() {
        add(
            "guruterminal.finance-providers/market-data",
            "Structured market data",
            &format!(
                "Fetch market history from enabled providers: {}.",
                market_providers.join(", ")
            ),
            &["finance_market_data"],
            &market_providers,
        );
    }
    let disclosure_providers = enabled(&["opendart.disclosures"]);
    if !disclosure_providers.is_empty() {
        add(
            "guruterminal.finance-providers/company-disclosures",
            "Official company data and filings",
            &format!(
                "Fetch company facts, exact filings, and entity identifiers from enabled regulators: {}.",
                disclosure_providers.join(", ")
            ),
            &["finance_company_data", "finance_filings", "finance_resolve_entity"],
            &disclosure_providers,
        );
    }
    components
}

fn mcp_runtime_components(
    capabilities: &[String],
) -> Result<Vec<AgentRuntimeComponent>, AgentHarnessError> {
    let catalog =
        crate::marketplace::bundled_catalog().map_err(|_| AgentHarnessError::InvalidContext)?;
    let enabled = capabilities
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut servers = std::collections::BTreeMap::<String, (Vec<String>, Vec<String>)>::new();
    for entry in catalog.entries {
        if !enabled.contains(&entry.id)
            || entry.runtime.kind != crate::marketplace::MarketplaceRuntimeKind::BundledMcp
        {
            continue;
        }
        let Some(server_id) = entry.runtime.server_id else {
            return Err(AgentHarnessError::InvalidContext);
        };
        let server = servers.entry(server_id).or_default();
        server.0.push(entry.name);
        server.1.extend(entry.runtime.provider_ids);
    }
    let mut namespaces = std::collections::BTreeSet::new();
    if servers
        .keys()
        .map(|server_id| server_id.replace(['.', '-'], "_"))
        .any(|namespace| !namespaces.insert(namespace))
    {
        return Err(AgentHarnessError::InvalidContext);
    }
    Ok(servers
        .into_iter()
        .map(|(server_id, (names, provider_ids))| {
            let provider_ids = provider_ids
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            AgentRuntimeComponent {
                id: format!("mcp/{server_id}"),
                kind: "mcp".into(),
                server_id: Some(server_id.clone()),
                name: if names.len() == 1 {
                    names[0].clone()
                } else {
                    format!("{} providers", server_id.to_uppercase())
                },
                description: format!(
                    "Discover and activate read-only tools from the bundled {server_id} MCP runtime. Enabled providers: {}.",
                    provider_ids.join(", ")
                ),
                tool_names: Vec::new(),
                provider_ids,
            }
        })
        .collect())
}

impl AgentHarnessSnapshot {
    pub fn validate(&self) -> Result<(), AgentHarnessError> {
        if self.schema != HARNESS_SCHEMA || self.mode != "chat" {
            return Err(AgentHarnessError::InvalidContext);
        }
        let mut seen_skills = std::collections::BTreeSet::new();
        for id in &self.skill_ids {
            // Stored Chat locks are historical. They must stay readable after a
            // bundled Skill is removed; only new runs require the current catalog.
            if !historical_skill_id_is_valid(id) || !seen_skills.insert(id.as_str()) {
                return Err(AgentHarnessError::InvalidSkill);
            }
        }
        let mut seen_user_skills = std::collections::BTreeSet::new();
        for skill in &self.user_skills {
            if crate::user_skill::skill_slug(&skill.id).is_err()
                || skill.revision_id.is_empty()
                || skill.revision_id.len() > 128
                || skill.content_sha256.len() != 64
                || !skill
                    .content_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
                || !seen_user_skills.insert(skill.id.as_str())
            {
                return Err(AgentHarnessError::InvalidSkill);
            }
        }
        if normalize_capability_ids(&self.capability_ids)? != self.capability_ids
            || self.digest.len() != 64
            || !self
                .digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(AgentHarnessError::InvalidContext);
        }
        Ok(())
    }

    pub fn validate_current(&self) -> Result<(), AgentHarnessError> {
        self.validate()?;
        let expected = snapshot_with_user_skills(
            &self.mode,
            &self.skill_ids,
            &self.user_skills,
            &self.capability_ids,
        )?;
        if &expected != self {
            return Err(AgentHarnessError::InvalidContext);
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum AgentHarnessError {
    #[error("unknown or duplicate agent skill")]
    InvalidSkill,
    #[error("bundled agent skill is missing or modified: {0}")]
    InvalidSkillFile(PathBuf),
    #[error("bundled agent extension is missing or modified: {0}")]
    InvalidExtensionFile(PathBuf),
    #[error("agent harness context is malformed")]
    InvalidContext,
}

pub fn extension_bundle_sha256() -> String {
    let mut digest = Sha256::new();
    digest.update(b"guruterminal-agent-extension-bundle/1");
    for (name, content) in EXTENSION_FILES {
        digest.update([0]);
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(content);
    }
    hex::encode(digest.finalize())
}

pub fn validate_extension_bundle(entrypoint: &Path) -> Result<(), AgentHarnessError> {
    if entrypoint.file_name().and_then(|name| name.to_str()) != Some(EXTENSION_ENTRYPOINT) {
        return Err(AgentHarnessError::InvalidExtensionFile(
            entrypoint.to_path_buf(),
        ));
    }
    let root = entrypoint
        .parent()
        .ok_or_else(|| AgentHarnessError::InvalidExtensionFile(entrypoint.to_path_buf()))?;
    validate_agent_files(root, EXTENSION_FILES)
}

pub fn validate_provider_extension_bundle(entrypoint: &Path) -> Result<(), AgentHarnessError> {
    if entrypoint.file_name().and_then(|name| name.to_str()) != Some(PROVIDER_EXTENSION_ENTRYPOINT)
    {
        return Err(AgentHarnessError::InvalidExtensionFile(
            entrypoint.to_path_buf(),
        ));
    }
    let root = entrypoint
        .parent()
        .ok_or_else(|| AgentHarnessError::InvalidExtensionFile(entrypoint.to_path_buf()))?;
    validate_agent_files(root, PROVIDER_EXTENSION_FILES)
}

fn validate_agent_files(root: &Path, files: &[(&str, &[u8])]) -> Result<(), AgentHarnessError> {
    for (name, expected) in files {
        let relative = Path::new(name);
        let mut ancestor = relative.parent();
        while let Some(parent) = ancestor {
            if parent.as_os_str().is_empty() {
                break;
            }
            let path = root.join(parent);
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|_| AgentHarnessError::InvalidExtensionFile(path.clone()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(AgentHarnessError::InvalidExtensionFile(path));
            }
            ancestor = parent.parent();
        }
        let path = root.join(name);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| AgentHarnessError::InvalidExtensionFile(path.clone()))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || std::fs::read(&path).ok().as_deref() != Some(*expected)
        {
            return Err(AgentHarnessError::InvalidExtensionFile(path));
        }
    }
    Ok(())
}

pub fn default_skill_ids() -> Vec<String> {
    SELECTABLE_SKILLS
        .iter()
        .map(|skill| skill.id.to_owned())
        .collect()
}

pub fn always_on_skill_ids() -> Vec<String> {
    ALWAYS_ON_SKILLS
        .iter()
        .map(|skill| skill.id.to_owned())
        .collect()
}

/// Skills Pi receives for a Chat run: always-on method cards, then the Guru's
/// selectable Skills. Always-on IDs never enter the user catalog.
pub fn run_skill_ids(selectable: &[String]) -> Result<Vec<String>, AgentHarnessError> {
    let selectable = normalize_selectable_skill_ids(selectable)?;
    let mut ids = always_on_skill_ids();
    for id in selectable {
        if ids.iter().any(|seen| seen == &id) {
            return Err(AgentHarnessError::InvalidSkill);
        }
        ids.push(id);
    }
    Ok(ids)
}

pub fn skill_binding_id(skill_id: &str) -> Result<String, AgentHarnessError> {
    selectable_skill(skill_id)?;
    Ok(format!("{SKILL_BINDING_PREFIX}{skill_id}"))
}

pub fn skill_id_from_binding(entry_id: &str) -> Option<&'static str> {
    let id = entry_id.strip_prefix(SKILL_BINDING_PREFIX)?;
    SELECTABLE_SKILLS
        .iter()
        .find(|skill| skill.id == id)
        .map(|skill| skill.id)
}

pub fn user_skill_binding_id(skill_id: &str) -> Result<String, AgentHarnessError> {
    crate::user_skill::skill_slug(skill_id).map_err(|_| AgentHarnessError::InvalidSkill)?;
    Ok(format!(
        "user-skill.{}",
        hex::encode(Sha256::digest(skill_id.as_bytes()))
    ))
}

pub fn normalize_selectable_skill_ids(
    requested: &[String],
) -> Result<Vec<String>, AgentHarnessError> {
    let mut normalized = Vec::new();
    for skill in SELECTABLE_SKILLS {
        if requested.iter().any(|id| id == skill.id) {
            normalized.push(skill.id.to_owned());
        }
    }
    if normalized.len() != requested.len() {
        return Err(AgentHarnessError::InvalidSkill);
    }
    Ok(normalized)
}

pub fn skill_catalog(enabled_skill_ids: &[String]) -> Vec<AgentSkillSummary> {
    SELECTABLE_SKILLS
        .iter()
        .map(|skill| {
            let (_, description) = bundled_skill_frontmatter(skill);
            AgentSkillSummary {
                id: skill.id.to_owned(),
                name: skill.name.to_owned(),
                description,
                enabled: enabled_skill_ids.iter().any(|id| id == skill.id),
                ownership: "bundled".into(),
                editable: false,
                current_revision_id: None,
            }
        })
        .collect()
}

pub fn resolve_skill_paths(
    agent_root: &Path,
    skill_ids: &[String],
) -> Result<Vec<PathBuf>, AgentHarnessError> {
    let mut paths = Vec::with_capacity(skill_ids.len());
    for id in skill_ids {
        let skill = bundled_skill(id)?;
        if paths
            .iter()
            .any(|existing| existing == &agent_root.join(skill.relative_path))
        {
            return Err(AgentHarnessError::InvalidSkill);
        }
        let path = agent_root.join(skill.relative_path);
        validate_skill_path(agent_root, &path)?;
        paths.push(path);
    }
    Ok(paths)
}

pub fn validate_skill_path(agent_root: &Path, path: &Path) -> Result<(), AgentHarnessError> {
    let skill = all_skills()
        .find(|skill| agent_root.join(skill.relative_path) == path)
        .ok_or(AgentHarnessError::InvalidSkillFile(path.to_path_buf()))?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| AgentHarnessError::InvalidSkillFile(path.to_path_buf()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || std::fs::read(path).ok().as_deref() != Some(skill.content)
    {
        return Err(AgentHarnessError::InvalidSkillFile(path.to_path_buf()));
    }
    Ok(())
}

pub fn validate_user_skill_path(
    private_run_dir: &Path,
    path: &Path,
) -> Result<(), AgentHarnessError> {
    let user_root = private_run_dir.join("user-skills");
    let relative = path
        .strip_prefix(&user_root)
        .map_err(|_| AgentHarnessError::InvalidSkillFile(path.to_path_buf()))?;
    let components = relative.components().collect::<Vec<_>>();
    if components.len() != 2
        || components[1].as_os_str() != "SKILL.md"
        || !matches!(components[0], std::path::Component::Normal(_))
    {
        return Err(AgentHarnessError::InvalidSkillFile(path.to_path_buf()));
    }
    let slug = components[0]
        .as_os_str()
        .to_str()
        .ok_or_else(|| AgentHarnessError::InvalidSkillFile(path.to_path_buf()))?;
    crate::user_skill::skill_slug(&format!("skill:{slug}"))
        .map_err(|_| AgentHarnessError::InvalidSkillFile(path.to_path_buf()))?;
    for directory in [&user_root, &user_root.join(slug)] {
        let metadata = std::fs::symlink_metadata(directory)
            .map_err(|_| AgentHarnessError::InvalidSkillFile(path.to_path_buf()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AgentHarnessError::InvalidSkillFile(path.to_path_buf()));
        }
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| AgentHarnessError::InvalidSkillFile(path.to_path_buf()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len()
            > (crate::user_skill::MAX_SKILL_MARKDOWN_BYTES + USER_SKILL_PROVENANCE_BANNER.len())
                as u64
    {
        return Err(AgentHarnessError::InvalidSkillFile(path.to_path_buf()));
    }
    let markdown = std::fs::read_to_string(path)
        .map_err(|_| AgentHarnessError::InvalidSkillFile(path.to_path_buf()))?;
    let (name, _) = crate::user_skill::parse_skill_frontmatter(&markdown)
        .map_err(|_| AgentHarnessError::InvalidSkillFile(path.to_path_buf()))?;
    if name != slug {
        return Err(AgentHarnessError::InvalidSkillFile(path.to_path_buf()));
    }
    Ok(())
}

pub fn snapshot(
    mode: &str,
    skill_ids: &[String],
    capability_ids: &[String],
) -> Result<AgentHarnessSnapshot, AgentHarnessError> {
    snapshot_with_user_skills(mode, skill_ids, &[], capability_ids)
}

pub fn snapshot_with_user_skills(
    mode: &str,
    skill_ids: &[String],
    user_skills: &[UserSkillSnapshot],
    capability_ids: &[String],
) -> Result<AgentHarnessSnapshot, AgentHarnessError> {
    if mode != "chat" {
        return Err(AgentHarnessError::InvalidContext);
    }
    if skill_ids.len().saturating_add(user_skills.len()) > MAX_ACTIVE_SKILLS {
        return Err(AgentHarnessError::InvalidSkill);
    }
    let mut normalized_skills = Vec::with_capacity(skill_ids.len());
    for id in skill_ids {
        selectable_skill(id)?;
        if normalized_skills.iter().any(|seen| seen == id) {
            return Err(AgentHarnessError::InvalidSkill);
        }
        normalized_skills.push(id.clone());
    }
    let normalized_capabilities = normalize_capability_ids(capability_ids)?;
    let mut normalized_user_skills = user_skills.to_vec();
    normalized_user_skills.sort_by(|left, right| left.id.cmp(&right.id));
    if normalized_user_skills != user_skills
        || normalized_user_skills
            .windows(2)
            .any(|pair| pair[0].id == pair[1].id)
    {
        return Err(AgentHarnessError::InvalidSkill);
    }
    let mut digest = Sha256::new();
    digest.update(HARNESS_SCHEMA.as_bytes());
    digest.update([0]);
    digest.update(mode.as_bytes());
    digest.update([0]);
    digest.update(include_bytes!("../../agent/SYSTEM.md"));
    digest.update([0]);
    digest.update(extension_bundle_sha256().as_bytes());
    for skill in ALWAYS_ON_SKILLS {
        digest.update([0]);
        digest.update(skill.id.as_bytes());
        digest.update([0]);
        digest.update(skill.content);
    }
    for id in &normalized_skills {
        let skill = bundled_skill(id)?;
        digest.update([0]);
        digest.update(skill.id.as_bytes());
        digest.update([0]);
        digest.update(skill.content);
    }
    for skill in &normalized_user_skills {
        crate::user_skill::skill_slug(&skill.id).map_err(|_| AgentHarnessError::InvalidSkill)?;
        if skill.content_sha256.len() != 64
            || !skill
                .content_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(AgentHarnessError::InvalidSkill);
        }
        digest.update([0]);
        digest.update(skill.id.as_bytes());
        digest.update([0]);
        digest.update(skill.revision_id.as_bytes());
        digest.update([0]);
        digest.update(skill.content_sha256.as_bytes());
    }
    for id in &normalized_capabilities {
        digest.update([0]);
        digest.update(id.as_bytes());
    }
    Ok(AgentHarnessSnapshot {
        schema: HARNESS_SCHEMA.to_owned(),
        mode: mode.to_owned(),
        skill_ids: normalized_skills,
        user_skills: normalized_user_skills,
        capability_ids: normalized_capabilities,
        digest: hex::encode(digest.finalize()),
    })
}

fn normalize_capability_ids(capability_ids: &[String]) -> Result<Vec<String>, AgentHarnessError> {
    let mut normalized = capability_ids.to_vec();
    normalized.sort();
    if normalized.windows(2).any(|pair| pair[0] == pair[1])
        || normalized.iter().any(|id| {
            id.is_empty()
                || id.len() > 96
                || !id.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte)
                })
                || !id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                || !id.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
        })
    {
        return Err(AgentHarnessError::InvalidContext);
    }
    Ok(normalized)
}

pub fn append_snapshot_to_context(
    context: &str,
    snapshot: &AgentHarnessSnapshot,
) -> Result<String, AgentHarnessError> {
    snapshot.validate_current()?;
    let mut value: Value =
        serde_json::from_str(context).map_err(|_| AgentHarnessError::InvalidContext)?;
    let object = value
        .as_object_mut()
        .ok_or(AgentHarnessError::InvalidContext)?;
    if object.contains_key("agent_harness") {
        return Err(AgentHarnessError::InvalidContext);
    }
    object.insert(
        "agent_harness".to_owned(),
        serde_json::to_value(snapshot).map_err(|_| AgentHarnessError::InvalidContext)?,
    );
    serde_json::to_string(&value).map_err(|_| AgentHarnessError::InvalidContext)
}

pub fn append_runtime_profile_to_context(
    context: &str,
    profile: &AgentRuntimeProfile,
) -> Result<String, AgentHarnessError> {
    profile.validate()?;
    let mut value: Value =
        serde_json::from_str(context).map_err(|_| AgentHarnessError::InvalidContext)?;
    let object = value
        .as_object_mut()
        .ok_or(AgentHarnessError::InvalidContext)?;
    let harness_mode = object
        .get("agent_harness")
        .and_then(|harness| harness.get("mode"))
        .and_then(Value::as_str);
    if harness_mode != Some(profile.mode.as_str()) || object.contains_key("agent_runtime") {
        return Err(AgentHarnessError::InvalidContext);
    }
    object.insert(
        "agent_runtime".to_owned(),
        serde_json::to_value(profile).map_err(|_| AgentHarnessError::InvalidContext)?,
    );
    serde_json::to_string(&value).map_err(|_| AgentHarnessError::InvalidContext)
}

/// Rust host wall clock for the current Chat turn. Placed on the turn prompt,
/// not the system-prompt host envelope, so the cacheable prefix stays stable.
pub fn live_time_envelope(current_utc: DateTime<Utc>) -> Value {
    json!({
        "schema": "guruterminal-live-time/1",
        "authority": "rust_host_wall_clock",
        "current_utc": current_utc.to_rfc3339_opts(SecondsFormat::Millis, true),
        "current_date_utc": current_utc.format("%Y-%m-%d").to_string(),
        "applies_to": "live_chat_only",
        "not_evidence": true
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LearnedMemoryIndexEntry {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub as_of: String,
    pub kind: String,
}

const MAX_LEARNED_INDEX_ITEMS: usize = 24;
const MAX_LEARNED_INDEX_BYTES: usize = 8 * 1024;
pub const CHARTER_RECORD_ID: &str = "lens:charter";
const MAX_CHARTER_BYTES: usize = 4 * 1024;

pub fn learned_memory_index_from_records(
    records: &[Value],
    recent_ids: &[String],
    cutoff: Option<chrono::NaiveDate>,
) -> Vec<LearnedMemoryIndexEntry> {
    let mut entries = records
        .iter()
        .filter_map(|record| {
            let id = record.get("id")?.as_str()?;
            let kind_slug = record
                .get("kind")
                .and_then(Value::as_str)
                .or_else(|| id.split_once(':').map(|(kind, _)| kind))?;
            let kind = match kind_slug {
                "wiki" | "Wiki" => "Wiki",
                "lens" | "Lens" => "Lens",
                _ => return None,
            };
            if record.get("status").and_then(Value::as_str) == Some("revoked") {
                return None;
            }
            let as_of = record
                .get("as_of")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if cutoff.is_some_and(|cutoff| memory_as_of_is_after(&as_of, cutoff)) {
                return None;
            }
            Some(LearnedMemoryIndexEntry {
                id: id.to_owned(),
                title: record
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_owned(),
                summary: record
                    .get("summary")
                    .or_else(|| record.get("excerpt"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .chars()
                    .take(280)
                    .collect(),
                as_of,
                kind: kind.to_owned(),
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        let left_rank = recent_ids.iter().position(|id| id == &left.id);
        let right_rank = recent_ids.iter().position(|id| id == &right.id);
        match (left_rank, right_rank) {
            (Some(left_index), Some(right_index)) => left_index.cmp(&right_index),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => right
                .as_of
                .cmp(&left.as_of)
                .then_with(|| left.id.cmp(&right.id)),
        }
    });
    cap_learned_memory_index(entries)
}

fn memory_as_of_is_after(as_of: &str, cutoff: chrono::NaiveDate) -> bool {
    DateTime::parse_from_rfc3339(as_of)
        .ok()
        .map(|value| value.date_naive())
        .or_else(|| chrono::NaiveDate::parse_from_str(as_of, "%Y-%m-%d").ok())
        .is_none_or(|date| date > cutoff)
}

fn cap_learned_memory_index(entries: Vec<LearnedMemoryIndexEntry>) -> Vec<LearnedMemoryIndexEntry> {
    let mut out = Vec::new();
    for entry in entries.into_iter().take(MAX_LEARNED_INDEX_ITEMS) {
        out.push(entry);
        if serde_json::to_vec(&out)
            .map(|encoded| encoded.len())
            .unwrap_or(usize::MAX)
            > MAX_LEARNED_INDEX_BYTES
        {
            out.pop();
            break;
        }
    }
    out
}

pub fn charter_body_from_markdown(markdown: &str) -> String {
    bound_utf8_prefix(markdown_body(markdown), MAX_CHARTER_BYTES).to_owned()
}

pub fn charter_from_knowledge_read(
    read: &Value,
    cutoff: Option<chrono::NaiveDate>,
) -> Option<String> {
    let document = read.get("document")?;
    if document.get("id").and_then(Value::as_str) != Some(CHARTER_RECORD_ID) {
        return None;
    }
    if document.get("status").and_then(Value::as_str) == Some("revoked") {
        return None;
    }
    let as_of = document.get("as_of").and_then(Value::as_str).unwrap_or("");
    if cutoff.is_some_and(|cutoff| memory_as_of_is_after(as_of, cutoff)) {
        return None;
    }
    let content = read.get("content").and_then(Value::as_str)?;
    let body = charter_body_from_markdown(content);
    (!body.trim().is_empty()).then_some(body)
}

pub fn turn_envelope_block(
    current_utc: DateTime<Utc>,
    use_memory: bool,
    learned_index: &[LearnedMemoryIndexEntry],
    charter: Option<&str>,
) -> Result<String, AgentHarnessError> {
    let mut envelope = json!({
        "live_time": live_time_envelope(current_utc)
    });
    if use_memory {
        let mut memory_protocol = json!({
            "active": true,
            "learned_index": learned_index
        });
        if let Some(body) = charter.filter(|body| !body.is_empty()) {
            memory_protocol["charter"] = json!(body);
        }
        envelope["memory_protocol"] = memory_protocol;
    }
    serde_json::to_string(&envelope).map_err(|_| AgentHarnessError::InvalidContext)
}

fn markdown_body(markdown: &str) -> &str {
    let Some(rest) = markdown.strip_prefix("---") else {
        return markdown;
    };
    let Some(end) = rest.find("\n---") else {
        return markdown;
    };
    rest[end + 4..].trim_start_matches('\n')
}

fn bound_utf8_prefix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub fn apply_user_skill_banner(markdown: &str) -> String {
    if markdown.contains("guruterminal-user-skill/1") {
        return markdown.to_owned();
    }
    let mut seen = 0usize;
    let mut offset = 0usize;
    for line in markdown.split_inclusive('\n') {
        if line.trim() == "---" {
            seen += 1;
            offset += line.len();
            if seen == 2 {
                let mut out =
                    String::with_capacity(USER_SKILL_PROVENANCE_BANNER.len() + markdown.len());
                out.push_str(&markdown[..offset]);
                out.push_str(USER_SKILL_PROVENANCE_BANNER);
                out.push_str(&markdown[offset..]);
                return out;
            }
            continue;
        }
        offset += line.len();
    }
    format!("{USER_SKILL_PROVENANCE_BANNER}{markdown}")
}

fn bundled_skill_frontmatter(skill: &BundledSkill) -> (String, String) {
    let markdown = std::str::from_utf8(skill.content).expect("bundled skill is utf-8");
    crate::user_skill::parse_skill_frontmatter(markdown).expect("bundled skill frontmatter")
}

fn selectable_skill(id: &str) -> Result<&'static BundledSkill, AgentHarnessError> {
    SELECTABLE_SKILLS
        .iter()
        .find(|skill| skill.id == id)
        .ok_or(AgentHarnessError::InvalidSkill)
}

fn bundled_skill(id: &str) -> Result<&'static BundledSkill, AgentHarnessError> {
    all_skills()
        .find(|skill| skill.id == id)
        .ok_or(AgentHarnessError::InvalidSkill)
}

fn historical_skill_id_is_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
}

fn all_skills() -> impl Iterator<Item = &'static BundledSkill> {
    ALWAYS_ON_SKILLS.iter().chain(SELECTABLE_SKILLS.iter())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn write_agent_files(root: &Path, files: &[(&str, &[u8])]) {
        for (name, content) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
    }

    #[test]
    fn extension_bundles_bind_every_native_search_module() {
        let temporary = tempfile::tempdir().unwrap();
        write_agent_files(temporary.path(), EXTENSION_FILES);
        write_agent_files(temporary.path(), PROVIDER_EXTENSION_FILES);
        let chat = temporary.path().join(EXTENSION_ENTRYPOINT);
        let support = temporary.path().join(PROVIDER_EXTENSION_ENTRYPOINT);
        validate_extension_bundle(&chat).unwrap();
        validate_provider_extension_bundle(&support).unwrap();

        std::fs::write(
            temporary.path().join("native-search/common.mjs"),
            b"tampered",
        )
        .unwrap();
        assert!(validate_extension_bundle(&chat).is_err());
        assert!(validate_provider_extension_bundle(&support).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn extension_bundles_reject_a_symlinked_module_directory() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        write_agent_files(temporary.path(), EXTENSION_FILES);
        write_agent_files(external.path(), EXTENSION_FILES);
        std::fs::remove_dir_all(temporary.path().join("native-search")).unwrap();
        symlink(
            external.path().join("native-search"),
            temporary.path().join("native-search"),
        )
        .unwrap();
        assert!(validate_extension_bundle(&temporary.path().join(EXTENSION_ENTRYPOINT)).is_err());
    }

    #[test]
    fn turn_envelope_carries_live_time_without_system_context() {
        let now = Utc.with_ymd_and_hms(2026, 8, 10, 1, 2, 3).unwrap();
        let block = turn_envelope_block(now, false, &[], None).unwrap();
        let value: Value = serde_json::from_str(&block).unwrap();
        assert_eq!(value["live_time"]["current_date_utc"], "2026-08-10");
        assert_eq!(value["live_time"]["not_evidence"], true);
        assert!(value.get("agent_runtime").is_none());
        assert!(value.get("memory_protocol").is_none());
        let with_memory = turn_envelope_block(
            now,
            true,
            &[LearnedMemoryIndexEntry {
                id: "wiki:ev".into(),
                title: "EV industry".into(),
                summary: "Durable EV facts.".into(),
                as_of: "2026-08-19T00:00:00Z".into(),
                kind: "Wiki".into(),
            }],
            None,
        )
        .unwrap();
        let memory: Value = serde_json::from_str(&with_memory).unwrap();
        assert_eq!(memory["memory_protocol"]["active"], true);
        assert!(memory["memory_protocol"].get("instructions").is_none());
        assert!(memory["memory_protocol"].get("charter").is_none());
        assert_eq!(
            memory["memory_protocol"]["learned_index"][0]["id"],
            "wiki:ev"
        );
        assert!(memory["memory_protocol"]["learned_index"][0]
            .get("body")
            .is_none());
    }

    #[test]
    fn turn_envelope_includes_charter_body_when_memory_is_on_and_omits_it_when_off() {
        let now = Utc.with_ymd_and_hms(2026, 8, 10, 1, 2, 3).unwrap();
        let body = "Prefer cash-flow durability over narrative.";
        let with_memory = turn_envelope_block(now, true, &[], Some(body)).unwrap();
        let memory: Value = serde_json::from_str(&with_memory).unwrap();
        assert_eq!(memory["memory_protocol"]["charter"], body);
        let without_memory = turn_envelope_block(now, false, &[], Some(body)).unwrap();
        let omitted: Value = serde_json::from_str(&without_memory).unwrap();
        assert!(omitted.get("memory_protocol").is_none());
    }

    #[test]
    fn turn_envelope_includes_charter_when_lens_charter_exists() {
        let now = Utc.with_ymd_and_hms(2026, 8, 10, 1, 2, 3).unwrap();
        let read = json!({
            "document": {
                "id": CHARTER_RECORD_ID,
                "title": "How this Guru invests",
                "status": "active",
                "as_of": "2026-01-01T00:00:00Z"
            },
            "content": "---\nid: lens:charter\ntitle: How this Guru invests\nsummary: Standing philosophy.\nas_of: 2026-01-01T00:00:00Z\n---\n\n# Scope\n\nPrefer cash-flow durability over narrative.\n"
        });
        let charter = charter_from_knowledge_read(&read, None).expect("charter body");
        assert!(charter.contains("Prefer cash-flow durability over narrative."));
        let envelope = turn_envelope_block(now, true, &[], Some(charter.as_str())).unwrap();
        let value: Value = serde_json::from_str(&envelope).unwrap();
        assert_eq!(value["memory_protocol"]["charter"], charter);
        assert!(
            value["memory_protocol"]
                .get("charter")
                .and_then(Value::as_str)
                .is_some_and(|body| body.contains("Prefer cash-flow durability over narrative.")),
            "memory-on envelope must carry the reserved charter body: {envelope}"
        );
        assert!(charter_from_knowledge_read(
            &json!({
                "document": {
                    "id": CHARTER_RECORD_ID,
                    "status": "revoked",
                    "as_of": "2026-01-01T00:00:00Z"
                },
                "content": read["content"].clone()
            }),
            None
        )
        .is_none());
        let oversized = format!("{}{}", "cash ".repeat(MAX_CHARTER_BYTES / 4), "end");
        let truncated = charter_body_from_markdown(&oversized);
        assert!(truncated.len() <= MAX_CHARTER_BYTES);
        assert_ne!(truncated, oversized);
    }

    #[test]
    fn learned_index_prefers_recent_receipts_and_drops_revoked_or_post_cutoff() {
        let records = vec![
            json!({
                "id": "wiki:old",
                "kind": "wiki",
                "title": "Old",
                "summary": "Older compiled facts.",
                "as_of": "2026-01-01T00:00:00Z"
            }),
            json!({
                "id": "wiki:recent",
                "kind": "wiki",
                "title": "Recent",
                "summary": "Just learned.",
                "as_of": "2026-08-01T00:00:00Z"
            }),
            json!({
                "id": "wiki:revoked",
                "kind": "wiki",
                "title": "Revoked",
                "summary": "Unused.",
                "as_of": "2026-07-01T00:00:00Z",
                "status": "revoked"
            }),
            json!({
                "id": "wiki:future",
                "kind": "wiki",
                "title": "Future",
                "summary": "After cutoff.",
                "as_of": "2026-08-19T00:00:00Z"
            }),
            json!({
                "id": "evidence:theme",
                "kind": "evidence",
                "title": "Theme",
                "summary": "Not learned state.",
                "as_of": "2026-08-01T00:00:00Z"
            }),
        ];
        let cutoff = chrono::NaiveDate::from_ymd_opt(2026, 8, 10).unwrap();
        let index =
            learned_memory_index_from_records(&records, &["wiki:recent".into()], Some(cutoff));
        assert_eq!(
            index
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["wiki:recent", "wiki:old"]
        );
    }

    #[test]
    fn user_skill_banner_follows_frontmatter_and_keeps_name() {
        let markdown =
            "---\nname: house-style\ndescription: House tables\n---\n\n# House style\n\nUse tables.\n";
        let rendered = apply_user_skill_banner(markdown);
        assert!(rendered.starts_with("---\nname: house-style\ndescription: House tables\n---\n"));
        assert!(rendered.contains("guruterminal-user-skill/1"));
        let (name, _) = crate::user_skill::parse_skill_frontmatter(&rendered).unwrap();
        assert_eq!(name, "house-style");
        assert_eq!(apply_user_skill_banner(&rendered), rendered);
    }

    #[test]
    fn stored_harness_locks_remain_readable_after_bundled_skills_are_removed() {
        let snapshot = AgentHarnessSnapshot {
            schema: HARNESS_SCHEMA.to_owned(),
            mode: "chat".into(),
            skill_ids: vec!["finance-research".into(), "investment-postmortem".into()],
            user_skills: Vec::new(),
            capability_ids: vec!["community.web-research".into()],
            digest: "a".repeat(64),
        };
        snapshot.validate().unwrap();
        assert!(snapshot.validate_current().is_err());
        let invalid = AgentHarnessSnapshot {
            skill_ids: vec!["Finance Research".into()],
            ..snapshot.clone()
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn bundled_skill_frontmatter_is_the_catalog_source() {
        let catalog = skill_catalog(&default_skill_ids());
        assert_eq!(catalog.len(), SELECTABLE_SKILLS.len());
        for skill in SELECTABLE_SKILLS {
            let (name, description) = bundled_skill_frontmatter(skill);
            assert_eq!(name, skill.id);
            let summary = catalog
                .iter()
                .find(|entry| entry.id == skill.id)
                .expect(skill.id);
            assert_eq!(summary.name, skill.name);
            assert_eq!(summary.description, description);
        }
        for skill in ALWAYS_ON_SKILLS {
            let (name, _) = bundled_skill_frontmatter(skill);
            assert_eq!(name, skill.id);
            assert!(catalog.iter().all(|entry| entry.id != skill.id));
        }
    }

    #[test]
    fn always_on_method_skills_are_not_selectable() {
        let catalog = skill_catalog(&default_skill_ids());
        let always_on = always_on_skill_ids();
        assert!(!always_on.is_empty());
        for id in &always_on {
            assert!(default_skill_ids()
                .iter()
                .all(|selectable| selectable != id));
            assert!(catalog.iter().all(|entry| &entry.id != id));
            assert!(skill_binding_id(id).is_err());
            assert!(skill_id_from_binding(&format!("skill.{id}")).is_none());
            assert!(normalize_selectable_skill_ids(std::slice::from_ref(id)).is_err());
            assert!(snapshot("chat", std::slice::from_ref(id), &[]).is_err());
        }
        assert_eq!(catalog.len(), SELECTABLE_SKILLS.len());
        assert!(catalog.iter().any(|entry| entry.id == RESEARCH_SKILL_ID));
    }

    #[test]
    fn run_skills_keep_research_and_prepend_method_cards() {
        let research_only = run_skill_ids(&[RESEARCH_SKILL_ID.to_owned()]).unwrap();
        assert_eq!(
            research_only[always_on_skill_ids().len()..],
            [RESEARCH_SKILL_ID]
        );
        for id in always_on_skill_ids() {
            assert!(research_only.contains(&id));
        }
        assert!(research_only.contains(&RESEARCH_SKILL_ID.to_owned()));
        assert_eq!(run_skill_ids(&[]).unwrap(), always_on_skill_ids());

        let empty_catalog = skill_catalog(&[]);
        assert!(empty_catalog.iter().all(|entry| !entry.enabled));
        assert!(empty_catalog
            .iter()
            .any(|entry| entry.id == RESEARCH_SKILL_ID));
        assert!(empty_catalog
            .iter()
            .all(|entry| !always_on_skill_ids().contains(&entry.id)));
    }

    #[test]
    fn snapshot_hashes_always_on_method_skills() {
        let without_research = snapshot("chat", &[], &[]).unwrap();
        let with_research = snapshot("chat", &[RESEARCH_SKILL_ID.to_owned()], &[]).unwrap();
        assert_eq!(without_research.skill_ids, Vec::<String>::new());
        assert_eq!(with_research.skill_ids, vec![RESEARCH_SKILL_ID]);
        assert_ne!(without_research.digest, with_research.digest);
        without_research.validate_current().unwrap();
        with_research.validate_current().unwrap();
    }

    #[test]
    fn runtime_profile_keeps_run_results_core_and_provider_ids() {
        let profile = AgentRuntimeProfile::new(
            "chat",
            false,
            false,
            &[
                "guruterminal.finance-core".into(),
                "openbb.platform".into(),
                "sec.edgar".into(),
                "opendart.disclosures".into(),
            ],
        )
        .unwrap();
        assert!(profile
            .core_tool_names
            .iter()
            .any(|name| name == "run_results_list"));
        assert!(!profile
            .core_tool_names
            .iter()
            .any(|name| name == "memory_search"));
        let openbb = profile
            .components
            .iter()
            .find(|component| component.id == "mcp/openbb")
            .unwrap();
        assert_eq!(openbb.kind, "mcp");
        assert_eq!(openbb.server_id.as_deref(), Some("openbb"));
        assert!(openbb.tool_names.is_empty());
        assert!(openbb.provider_ids.contains(&"yfinance".into()));
        assert!(profile
            .components
            .iter()
            .all(|component| component.id != "guruterminal.finance-providers/market-data"));
        let disclosures = profile
            .components
            .iter()
            .find(|component| component.id == "guruterminal.finance-providers/company-disclosures")
            .unwrap();
        assert!(disclosures
            .tool_names
            .contains(&"finance_resolve_entity".into()));
    }

    #[test]
    fn compute_component_names_python_and_javascript() {
        let profile = AgentRuntimeProfile::new(
            "chat",
            false,
            false,
            &["guruterminal.compute-python".into()],
        )
        .unwrap();
        let compute = profile
            .components
            .iter()
            .find(|component| component.id == "guruterminal.compute-python/python")
            .unwrap();
        assert_eq!(compute.tool_names, vec!["compute_run".to_owned()]);
        let haystack = format!(
            "{} {} {} {}",
            compute.id,
            compute.name,
            compute.description,
            compute.tool_names.join(" ")
        )
        .to_ascii_lowercase();
        assert!(haystack.contains("python"));
        assert!(haystack.contains("javascript"));
        assert!(haystack.contains("compute_run"));
    }
}
