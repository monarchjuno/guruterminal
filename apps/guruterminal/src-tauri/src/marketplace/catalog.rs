use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::app::CommandError;

pub(super) const MARKETPLACE_SCHEMA_VERSION: &str = "guruterminal-marketplace/1";
pub(super) const CATALOG_SCHEMA_VERSION: &str = "guruterminal-marketplace-catalog/1";
pub(super) const SNAPSHOT_SCHEMA_VERSION: &str = "guruterminal-marketplace-snapshot/1";
const BUNDLED_MARKETPLACE: &str =
    include_str!(concat!(env!("OUT_DIR"), "/marketplace_bundle.json"));
pub(super) const MAX_CATALOG_ENTRIES: usize = 128;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceCatalogDto {
    pub schema_version: String,
    pub entries: Vec<MarketplaceEntryDto>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceEntryDto {
    pub id: String,
    #[serde(default)]
    pub plugin: String,
    pub name: String,
    pub summary: String,
    pub publisher: String,
    pub data_authority: String,
    pub kind: MarketplaceEntryKind,
    pub free_state: MarketplaceFreeState,
    pub trust: MarketplaceTrust,
    pub runtime: MarketplaceRuntimeDto,
    pub release_stage: MarketplaceReleaseStage,
    pub featured: bool,
    pub markets: Vec<String>,
    pub asset_classes: Vec<String>,
    pub capabilities: Vec<String>,
    pub freshness: Vec<String>,
    pub attribution: String,
    pub terms_url: Option<String>,
    pub permissions: MarketplacePermissionsDto,
    #[serde(default)]
    pub setup: Option<MarketplaceSetupDto>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceRuntimeDto {
    pub kind: MarketplaceRuntimeKind,
    pub server_id: Option<String>,
    #[serde(default)]
    pub worker_id: Option<String>,
    #[serde(default)]
    pub provider_ids: Vec<String>,
    #[serde(default)]
    pub credential_mapping: BTreeMap<String, String>,
    #[serde(default)]
    pub config_mapping: BTreeMap<String, String>,
    #[serde(default)]
    pub verification_probe: Option<MarketplaceVerificationProbeDto>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceRuntimeKind {
    Native,
    LocalWorker,
    #[serde(rename = "mcp", alias = "bundled_mcp")]
    BundledMcp,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceVerificationProbeDto {
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceSetupDto {
    #[serde(default)]
    pub config_fields: Vec<MarketplaceSetupFieldDto>,
    #[serde(default)]
    pub credential_fields: Vec<MarketplaceSetupFieldDto>,
    #[serde(default)]
    pub credential_scope_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceSetupFieldDto {
    pub id: String,
    pub kind: MarketplaceSetupFieldKind,
    pub options: Vec<String>,
    pub label: String,
    pub required: bool,
    pub min_length: usize,
    pub max_length: usize,
    pub help_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceSetupFieldKind {
    ApiKey,
    Email,
    Select,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceEntryKind {
    DataSource,
    AnalysisTool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceFreeState {
    Keyless,
    FreeAccount,
    Local,
    Paid,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceTrust {
    FirstParty,
    ReviewedCommunity,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceReleaseStage {
    Available,
    Preview,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MarketplacePermissionsDto {
    pub network_hosts: Vec<String>,
    pub credential_kinds: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MarketplaceInstalledDto {
    pub entry_id: String,
    pub configured: bool,
    pub health: MarketplaceHealth,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceHealth {
    Ready,
    NeedsConfiguration,
    Disabled,
    Error,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MarketplaceSnapshotDto {
    pub schema_version: String,
    pub sources: Vec<MarketplaceSourceDto>,
    pub plugins: Vec<MarketplacePluginDto>,
    pub catalog: MarketplaceCatalogDto,
    pub installed: Vec<MarketplaceInstalledDto>,
    pub connectors: Vec<MarketplaceConnectorStatusDto>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MarketplaceSourceDto {
    pub id: String,
    pub display_name: String,
    pub status: MarketplaceSourceStatus,
    pub summary: String,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceSourceStatus {
    Ready,
    ComingSoon,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MarketplacePluginDto {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: MarketplaceAuthorDto,
    pub interface: MarketplacePluginInterfaceDto,
    pub policy: MarketplacePluginPolicyDto,
    pub category: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceAuthorDto {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MarketplacePluginInterfaceDto {
    pub display_name: String,
    pub short_description: String,
    pub category: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MarketplacePluginPolicyDto {
    pub installation: MarketplaceInstallationPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<MarketplaceAuthenticationPolicy>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MarketplaceInstallationPolicy {
    InstalledByDefault,
    Available,
    NotAvailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MarketplaceAuthenticationPolicy {
    OnInstall,
    OnUse,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct GuruCapabilityBindingDto {
    pub entry_id: String,
    pub enabled: bool,
    pub granted_permissions: Vec<String>,
    pub available: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MarketplaceConnectorStatusDto {
    pub entry_id: String,
    pub config: BTreeMap<String, String>,
    pub config_state: MarketplaceConfigState,
    pub credentials: Vec<MarketplaceCredentialStatusDto>,
    pub readiness: MarketplaceConnectorReadiness,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceConfigState {
    NotRequired,
    Missing,
    Valid,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceConnectorReadiness {
    Ready,
    NeedsConfiguration,
    RuntimeUnavailable,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceCredentialVerification {
    Never,
    Verified,
    Rejected,
    TemporarilyUnavailable,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MarketplaceCredentialStatusDto {
    pub entry_id: String,
    pub credential_id: String,
    pub stored: bool,
    pub active: bool,
    pub pending: bool,
    pub verification: MarketplaceCredentialVerification,
    pub verified_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuruCapabilityRequest {
    pub guru_id: String,
    pub entry_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceConnectorConfigureRequest {
    pub entry_id: String,
    pub config: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceCredentialRequest {
    pub entry_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceCredentialSaveRequest {
    pub entry_id: String,
    pub secrets: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketplaceBundleDto {
    files: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketplaceIndexDto {
    schema_version: String,
    name: String,
    interface: MarketplaceIndexInterfaceDto,
    plugins: Vec<MarketplaceIndexPluginDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MarketplaceIndexInterfaceDto {
    display_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketplaceIndexPluginDto {
    name: String,
    source: MarketplaceIndexSourceDto,
    policy: MarketplacePluginPolicyDto,
    category: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketplaceIndexSourceDto {
    source: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "ref")]
    git_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginManifestDto {
    name: String,
    version: String,
    description: String,
    author: MarketplaceAuthorDto,
    interface: MarketplacePluginInterfaceDto,
    connectors: String,
    #[serde(default, rename = "mcpServers")]
    mcp_servers: Option<String>,
    #[serde(default)]
    skills: Option<String>,
    #[serde(default)]
    memory: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct BundledMarketplace {
    pub catalog: MarketplaceCatalogDto,
    pub plugins: Vec<MarketplacePluginDto>,
    pub official_display_name: String,
}

pub(crate) fn bundled_catalog() -> Result<MarketplaceCatalogDto, CommandError> {
    Ok(bundled_marketplace()?.catalog.clone())
}

pub(super) fn bundled_marketplace() -> Result<&'static BundledMarketplace, CommandError> {
    static BUNDLED: OnceLock<Result<BundledMarketplace, String>> = OnceLock::new();
    match BUNDLED.get_or_init(load_bundled_marketplace) {
        Ok(marketplace) => Ok(marketplace),
        Err(message) => Err(CommandError::internal(format!(
            "bundled Marketplace is invalid: {message}"
        ))),
    }
}

fn load_bundled_marketplace() -> Result<BundledMarketplace, String> {
    let bundle: MarketplaceBundleDto = serde_json::from_str(BUNDLED_MARKETPLACE)
        .map_err(|error| format!("marketplace bundle is invalid: {error}"))?;
    let index: MarketplaceIndexDto = serde_json::from_str(
        bundle
            .files
            .get("marketplace.json")
            .ok_or("marketplace.json is missing from the bundle")?,
    )
    .map_err(|error| format!("marketplace.json is invalid: {error}"))?;
    if index.schema_version != MARKETPLACE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported marketplace schema {}",
            index.schema_version
        ));
    }
    if index.name.trim().is_empty() || index.interface.display_name.trim().is_empty() {
        return Err("marketplace identity is empty".to_owned());
    }
    if index.plugins.is_empty() {
        return Err("marketplace has no plugins".to_owned());
    }

    let mut plugins = Vec::new();
    let mut entries = Vec::new();
    let mut plugin_names = BTreeSet::new();
    for listing in &index.plugins {
        if !valid_plugin_name(&listing.name) || !plugin_names.insert(listing.name.as_str()) {
            return Err(format!("plugin {} is not canonical", listing.name));
        }
        if listing.source.source != "local" {
            return Err(format!(
                "bundled loader rejects non-local source for {}",
                listing.name
            ));
        }
        if listing.source.url.is_some() || listing.source.git_ref.is_some() {
            return Err(format!(
                "bundled loader rejects remote selectors for {}",
                listing.name
            ));
        }
        let plugin_root = local_source_path(listing.source.path.as_deref(), &listing.name)?;
        let manifest_path = format!("{plugin_root}/.guruterminal-plugin/plugin.json");
        let manifest: PluginManifestDto = serde_json::from_str(
            bundle
                .files
                .get(&manifest_path)
                .ok_or_else(|| format!("{manifest_path} is missing"))?,
        )
        .map_err(|error| format!("{manifest_path} is invalid: {error}"))?;
        if manifest.name != listing.name {
            return Err(format!(
                "plugin manifest name {} does not match listing {}",
                manifest.name, listing.name
            ));
        }
        if !valid_component_path(&manifest.connectors)
            || manifest
                .mcp_servers
                .as_deref()
                .is_some_and(|path| !valid_component_path(path))
            || manifest
                .skills
                .as_deref()
                .is_some_and(|path| !valid_component_path(path))
            || manifest
                .memory
                .as_deref()
                .is_some_and(|path| !valid_component_path(path))
        {
            return Err(format!(
                "plugin {} has an invalid component path",
                listing.name
            ));
        }
        if listing.policy.installation != MarketplaceInstallationPolicy::InstalledByDefault {
            return Err(format!(
                "bundled plugin {} must be installed by default",
                listing.name
            ));
        }
        let connector_prefix = format!(
            "{}/{}",
            plugin_root,
            manifest
                .connectors
                .trim_start_matches("./")
                .trim_end_matches('/')
        );
        let mut connector_files = bundle
            .files
            .iter()
            .filter(|(path, _)| {
                path.starts_with(&format!("{connector_prefix}/")) && path.ends_with(".json")
            })
            .collect::<Vec<_>>();
        connector_files.sort_by(|left, right| left.0.cmp(right.0));
        if connector_files.is_empty() {
            return Err(format!("plugin {} has no connectors", listing.name));
        }
        for (path, contents) in connector_files {
            let mut connector: MarketplaceEntryDto = serde_json::from_str(contents)
                .map_err(|error| format!("{path} is invalid: {error}"))?;
            connector.plugin = listing.name.clone();
            let file_stem = path
                .rsplit('/')
                .next()
                .and_then(|name| name.strip_suffix(".json"))
                .unwrap_or_default();
            if file_stem != connector.id {
                return Err(format!("{path} name does not match connector id"));
            }
            entries.push(connector);
        }
        plugins.push(MarketplacePluginDto {
            name: manifest.name,
            version: manifest.version,
            description: manifest.description,
            author: manifest.author,
            interface: manifest.interface,
            policy: listing.policy.clone(),
            category: listing.category.clone(),
        });
    }

    let catalog = MarketplaceCatalogDto {
        schema_version: CATALOG_SCHEMA_VERSION.to_owned(),
        entries,
    };
    validate_catalog(&catalog)?;
    Ok(BundledMarketplace {
        catalog,
        plugins,
        official_display_name: index.interface.display_name,
    })
}

fn local_source_path(path: Option<&str>, plugin: &str) -> Result<String, String> {
    let path = path.ok_or_else(|| format!("plugin {plugin} is missing a local path"))?;
    let expected = format!("./plugins/{plugin}");
    if path != expected || !valid_component_path(path) {
        return Err(format!("plugin {plugin} source path must be {expected}"));
    }
    Ok(path.trim_start_matches("./").to_owned())
}

fn valid_component_path(path: &str) -> bool {
    let path = path.strip_suffix('/').unwrap_or(path);
    let Some(rest) = path.strip_prefix("./") else {
        return false;
    };
    !rest.is_empty()
        && !rest.contains('\\')
        && !rest.contains("..")
        && rest
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn valid_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

pub(super) fn marketplace_sources(official_display_name: &str) -> Vec<MarketplaceSourceDto> {
    vec![
        MarketplaceSourceDto {
            id: "official".to_owned(),
            display_name: official_display_name.to_owned(),
            status: MarketplaceSourceStatus::Ready,
            summary: "Bundled data sources and local analysis tools.".to_owned(),
        },
        MarketplaceSourceDto {
            id: "community".to_owned(),
            display_name: "Community".to_owned(),
            status: MarketplaceSourceStatus::ComingSoon,
            summary: "Reviewed community plugins will appear here.".to_owned(),
        },
        MarketplaceSourceDto {
            id: "libraries".to_owned(),
            display_name: "Libraries".to_owned(),
            status: MarketplaceSourceStatus::ComingSoon,
            summary: "Shared Wiki and Lens libraries will appear here.".to_owned(),
        },
    ]
}

pub(super) fn validate_catalog(catalog: &MarketplaceCatalogDto) -> Result<(), String> {
    if catalog.schema_version != CATALOG_SCHEMA_VERSION {
        return Err(format!(
            "unsupported catalog schema {}",
            catalog.schema_version
        ));
    }
    if catalog.entries.is_empty() || catalog.entries.len() > MAX_CATALOG_ENTRIES {
        return Err(format!(
            "catalog entry count must be between 1 and {MAX_CATALOG_ENTRIES}"
        ));
    }

    let mut ids = BTreeSet::new();
    for entry in &catalog.entries {
        if !valid_entry_id(&entry.id) || !valid_plugin_name(&entry.plugin) {
            return Err(format!("entry id {} is not canonical", entry.id));
        }
        if !ids.insert(entry.id.as_str()) {
            return Err(format!("entry id {} is duplicated", entry.id));
        }
        if entry.name.trim().is_empty()
            || entry.summary.trim().is_empty()
            || entry.publisher.trim().is_empty()
            || entry.data_authority.trim().is_empty()
            || entry.attribution.trim().is_empty()
        {
            return Err(format!("entry {} has an empty required field", entry.id));
        }
        if entry.markets.is_empty()
            || entry.asset_classes.is_empty()
            || entry.capabilities.is_empty()
            || entry.freshness.is_empty()
        {
            return Err(format!("entry {} has an empty capability field", entry.id));
        }
        if entry.trust != MarketplaceTrust::FirstParty {
            return Err(format!("bundled entry {} must be first-party", entry.id));
        }
        if let Some(terms_url) = &entry.terms_url {
            if !terms_url.starts_with("https://") {
                return Err(format!("entry {} terms URL must use HTTPS", entry.id));
            }
        }
        for host in &entry.permissions.network_hosts {
            if host.is_empty()
                || host.contains('/')
                || host.contains(':')
                || host.chars().any(char::is_whitespace)
            {
                return Err(format!("entry {} has an invalid network host", entry.id));
            }
        }
        match entry.free_state {
            MarketplaceFreeState::FreeAccount if entry.permissions.credential_kinds.is_empty() => {
                return Err(format!(
                    "free-account entry {} must declare a credential kind",
                    entry.id
                ));
            }
            MarketplaceFreeState::Keyless | MarketplaceFreeState::Local
                if !entry.permissions.credential_kinds.is_empty() =>
            {
                return Err(format!(
                    "keyless or local entry {} cannot require a credential",
                    entry.id
                ));
            }
            _ => {}
        }
        let setup = entry.setup.as_ref();
        let credential_fields = setup
            .map(|setup| setup.credential_fields.as_slice())
            .unwrap_or_default();
        if credential_fields.len() != entry.permissions.credential_kinds.len()
            || credential_fields.iter().any(|field| {
                !entry
                    .permissions
                    .credential_kinds
                    .iter()
                    .any(|kind| kind == &field.id)
            })
        {
            return Err(format!(
                "entry {} setup credentials do not match permissions",
                entry.id
            ));
        }
        validate_runtime(entry)?;
        if let Some(setup) = setup {
            let mut field_ids = BTreeSet::new();
            for field in setup
                .config_fields
                .iter()
                .chain(setup.credential_fields.iter())
            {
                let unique_options = field.options.iter().collect::<BTreeSet<_>>();
                let valid_options = match field.kind {
                    MarketplaceSetupFieldKind::Select => {
                        !field.options.is_empty()
                            && unique_options.len() == field.options.len()
                            && field.options.iter().all(|option| {
                                (field.min_length..=field.max_length).contains(&option.len())
                                    && option.chars().all(|character| {
                                        character.is_ascii_lowercase()
                                            || character.is_ascii_digit()
                                            || matches!(character, '-' | '_')
                                    })
                            })
                    }
                    MarketplaceSetupFieldKind::ApiKey | MarketplaceSetupFieldKind::Email => {
                        field.options.is_empty()
                    }
                };
                if !field_ids.insert(field.id.as_str())
                    || field.id.is_empty()
                    || field.label.trim().is_empty()
                    || field.min_length == 0
                    || field.max_length < field.min_length
                    || field.max_length > 4_096
                    || field
                        .help_url
                        .as_ref()
                        .is_some_and(|url| !url.starts_with("https://"))
                    || !valid_options
                {
                    return Err(format!("entry {} has invalid setup fields", entry.id));
                }
            }
            if setup
                .credential_fields
                .iter()
                .any(|field| field.kind == MarketplaceSetupFieldKind::Select)
            {
                return Err(format!(
                    "entry {} cannot declare a select credential field",
                    entry.id
                ));
            }
            if setup.credential_scope_fields.iter().any(|scope_id| {
                !setup
                    .config_fields
                    .iter()
                    .any(|field| &field.id == scope_id)
            }) {
                return Err(format!(
                    "entry {} credential scope fields must reference config fields",
                    entry.id
                ));
            }
            if entry.id == crate::finance_data::KIS_SOURCE_ID {
                let environment = setup.config_fields.first();
                if setup.config_fields.len() != 1
                    || setup.credential_scope_fields != ["environment"]
                    || environment.is_none_or(|field| {
                        field.id != "environment"
                            || field.kind != MarketplaceSetupFieldKind::Select
                            || !field.required
                            || field.min_length != 4
                            || field.max_length != 4
                            || field.options.len() != 2
                            || field.options[0] != "real"
                            || field.options[1] != "demo"
                    })
                {
                    return Err(
                        "Korea Investment setup must declare the exact real/demo environment allowlist"
                            .to_owned(),
                    );
                }
                let expected_credentials = [
                    ("app_key", true, 8, 512),
                    ("app_secret", true, 8, 512),
                    ("account_number", false, 8, 8),
                    ("account_product_code", false, 2, 2),
                    ("hts_id", false, 1, 128),
                ];
                if setup.credential_fields.len() != expected_credentials.len()
                    || setup
                        .credential_fields
                        .iter()
                        .zip(expected_credentials)
                        .any(|(field, (id, required, min_length, max_length))| {
                            field.id != id
                                || field.kind != MarketplaceSetupFieldKind::ApiKey
                                || field.required != required
                                || field.min_length != min_length
                                || field.max_length != max_length
                        })
                {
                    return Err(
                        "Korea Investment setup must declare the exact app credential and optional account profile fields"
                            .to_owned(),
                    );
                }
            }
            if entry.id == "community.web-research" {
                let policy = setup.config_fields.first();
                if setup.config_fields.len() != 1
                    || !setup.credential_fields.is_empty()
                    || !setup.credential_scope_fields.is_empty()
                    || policy.is_none_or(|field| {
                        field.id != "search_policy"
                            || field.kind != MarketplaceSetupFieldKind::Select
                            || field.required
                            || field.min_length != 8
                            || field.max_length != 10
                            || field.options != ["automatic", "model_only", "exa_only"]
                    })
                {
                    return Err(
                        "Web Research setup must declare the exact optional routing allowlist"
                            .to_owned(),
                    );
                }
            }
        }
    }
    Ok(())
}

fn validate_runtime(entry: &MarketplaceEntryDto) -> Result<(), String> {
    let runtime = &entry.runtime;
    let valid_provider_id = |value: &str| {
        !value.is_empty()
            && value.len() <= 64
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
    };
    if runtime.provider_ids.len() > 64
        || runtime.provider_ids.iter().any(|id| !valid_provider_id(id))
        || runtime.provider_ids.iter().collect::<BTreeSet<_>>().len() != runtime.provider_ids.len()
    {
        return Err(format!("entry {} has invalid runtime providers", entry.id));
    }
    let credential_fields = entry
        .setup
        .as_ref()
        .map(|setup| setup.credential_fields.as_slice())
        .unwrap_or_default();
    let config_fields = entry
        .setup
        .as_ref()
        .map(|setup| setup.config_fields.as_slice())
        .unwrap_or_default();
    match runtime.kind {
        MarketplaceRuntimeKind::BundledMcp => {
            let Some(server_id) = runtime.server_id.as_deref() else {
                return Err(format!("entry {} has no MCP server id", entry.id));
            };
            if !valid_entry_id(server_id)
                || runtime.worker_id.is_some()
                || runtime.provider_ids.is_empty()
                || runtime.credential_mapping.len() != credential_fields.len()
                || runtime.credential_mapping.iter().any(|(field, target)| {
                    !credential_fields
                        .iter()
                        .any(|candidate| candidate.id == *field)
                        || !valid_provider_id(target)
                })
                || runtime.config_mapping.len() != config_fields.len()
                || runtime.config_mapping.iter().any(|(field, target)| {
                    !config_fields.iter().any(|candidate| candidate.id == *field)
                        || !valid_provider_id(target)
                })
            {
                return Err(format!(
                    "entry {} has invalid MCP runtime metadata",
                    entry.id
                ));
            }
            if !credential_fields.is_empty() && runtime.verification_probe.is_none() {
                return Err(format!(
                    "entry {} must declare an MCP credential verification probe",
                    entry.id
                ));
            }
        }
        MarketplaceRuntimeKind::LocalWorker => {
            let Some(worker_id) = runtime.worker_id.as_deref() else {
                return Err(format!("entry {} has no local worker id", entry.id));
            };
            if !valid_entry_id(worker_id)
                || runtime.server_id.is_some()
                || !runtime.provider_ids.is_empty()
                || !runtime.credential_mapping.is_empty()
                || !runtime.config_mapping.is_empty()
                || runtime.verification_probe.is_some()
            {
                return Err(format!(
                    "entry {} has invalid local runtime metadata",
                    entry.id
                ));
            }
        }
        MarketplaceRuntimeKind::Native => {
            if runtime.server_id.is_some()
                || runtime.worker_id.is_some()
                || !runtime.provider_ids.is_empty()
                || !runtime.credential_mapping.is_empty()
                || !runtime.config_mapping.is_empty()
                || runtime.verification_probe.is_some()
            {
                return Err(format!(
                    "entry {} has invalid native runtime metadata",
                    entry.id
                ));
            }
        }
    }
    if let Some(probe) = &runtime.verification_probe {
        if probe.tool_name.is_empty()
            || probe.tool_name.len() > 128
            || !probe.tool_name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/')
            })
            || !probe.arguments.is_object()
            || serde_json::to_vec(&probe.arguments)
                .map_err(|_| format!("entry {} has an invalid verification probe", entry.id))?
                .len()
                > 16 * 1024
        {
            return Err(format!(
                "entry {} has an invalid verification probe",
                entry.id
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_snapshot(snapshot: &MarketplaceSnapshotDto) -> Result<(), String> {
    if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
        return Err(format!(
            "unsupported snapshot schema {}",
            snapshot.schema_version
        ));
    }
    if snapshot.sources.len() != 3
        || snapshot.sources[0].id != "official"
        || snapshot.sources[0].status != MarketplaceSourceStatus::Ready
        || snapshot.sources[1].id != "community"
        || snapshot.sources[1].status != MarketplaceSourceStatus::ComingSoon
        || snapshot.sources[2].id != "libraries"
        || snapshot.sources[2].status != MarketplaceSourceStatus::ComingSoon
    {
        return Err("snapshot sources must declare official, community, and libraries".to_owned());
    }
    let catalog_ids = snapshot
        .catalog
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<BTreeSet<_>>();
    let plugin_names = snapshot
        .plugins
        .iter()
        .map(|plugin| plugin.name.as_str())
        .collect::<BTreeSet<_>>();
    if plugin_names.len() != snapshot.plugins.len() {
        return Err("snapshot plugins are duplicated".to_owned());
    }
    for entry in &snapshot.catalog.entries {
        if !plugin_names.contains(entry.plugin.as_str()) {
            return Err(format!(
                "capability {} references unknown plugin {}",
                entry.id, entry.plugin
            ));
        }
    }
    let mut installed_ids = BTreeSet::new();
    for installed in &snapshot.installed {
        if !catalog_ids.contains(installed.entry_id.as_str()) {
            return Err(format!(
                "configured entry {} is absent from the catalog",
                installed.entry_id
            ));
        }
        if !installed_ids.insert(installed.entry_id.as_str()) {
            return Err(format!(
                "configured entry {} is duplicated",
                installed.entry_id
            ));
        }
    }
    let mut connector_ids = BTreeSet::new();
    for connector in &snapshot.connectors {
        if !catalog_ids.contains(connector.entry_id.as_str()) {
            return Err(format!(
                "connector {} does not reference a catalog capability",
                connector.entry_id
            ));
        }
        if !connector_ids.insert(connector.entry_id.as_str()) {
            return Err(format!("connector {} is duplicated", connector.entry_id));
        }
    }
    Ok(())
}

pub(super) fn valid_entry_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
        && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
}
