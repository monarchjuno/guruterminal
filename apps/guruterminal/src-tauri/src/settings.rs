use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{cmp::Ordering, collections::BTreeSet, env, fs::OpenOptions, io::Read, path::Path};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::{app::CommandError, artifact_trust::ensure_private_regular_file, hashing::sha256};

const MAX_AUTH_FILE_BYTES: u64 = 64 * 1024;
const MAX_MODEL_ID_BYTES: usize = 512;
// Pi can expose large provider catalogs (for example, routing providers). Keep
// the persisted catalog bounded without treating the old manual-profile limit
// as a provider model limit.
const MAX_CONFIGURED_MODELS: usize = 4_096;
const MAX_MODEL_RUN_CONTROLS: usize = 16;
pub const PI_THINKING_LEVELS: &[&str] =
    &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
pub const PROVIDER_CREDENTIAL_ENVIRONMENTS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANT_LING_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "DEEPSEEK_API_KEY",
    "NVIDIA_API_KEY",
    "MISTRAL_API_KEY",
    "GROQ_API_KEY",
    "CEREBRAS_API_KEY",
    "XAI_API_KEY",
    "OPENROUTER_API_KEY",
    "AI_GATEWAY_API_KEY",
    "HF_TOKEN",
    "FIREWORKS_API_KEY",
    "TOGETHER_API_KEY",
    "BASETEN_API_KEY",
    "KIMI_API_KEY",
    "MINIMAX_API_KEY",
    "MINIMAX_CN_API_KEY",
    "ZAI_API_KEY",
    "ZAI_CODING_CN_API_KEY",
    "OPENCODE_API_KEY",
    "RADIUS_API_KEY",
    "QWEN_TOKEN_PLAN_API_KEY",
    "QWEN_TOKEN_PLAN_CN_API_KEY",
    "XIAOMI_API_KEY",
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredModel {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub model: String,
    pub input: Vec<String>,
    pub reasoning: bool,
    pub context_window: u64,
    pub max_tokens: u64,
    pub thinking_levels: Vec<String>,
    pub thinking_level_map: std::collections::BTreeMap<String, Option<String>>,
    pub run_controls: Vec<ModelRunControl>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelRunControlChoice {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelRunControl {
    pub id: String,
    pub label: String,
    pub default_choice: String,
    pub choices: Vec<ModelRunControlChoice>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionModelLock {
    pub profile_id: String,
    pub name: String,
    pub provider: String,
    pub model: String,
    pub thinking_level: String,
    pub run_options: std::collections::BTreeMap<String, String>,
}

impl ExecutionModelLock {
    pub fn from_model(
        value: &ConfiguredModel,
        thinking_level: &str,
        run_options: &std::collections::BTreeMap<String, String>,
    ) -> Self {
        Self {
            profile_id: value.id.clone(),
            name: value.name.clone(),
            provider: value.provider.clone(),
            model: value.model.clone(),
            thinking_level: thinking_level.to_owned(),
            run_options: run_options.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), CommandError> {
        if !valid_model_profile_id(&self.profile_id) {
            return Err(CommandError::invalid("execution model ID is invalid"));
        }
        if self.name.trim().is_empty()
            || self.name.len() > 80
            || self.name.chars().any(char::is_control)
        {
            return Err(CommandError::invalid(
                "execution model display name is invalid",
            ));
        }
        if !provider_options()
            .iter()
            .any(|option| option.id == self.provider)
        {
            return Err(CommandError::invalid("unsupported Pi model provider"));
        }
        validate_model_identifier(&self.model)?;
        if !PI_THINKING_LEVELS.contains(&self.thinking_level.as_str()) {
            return Err(CommandError::invalid("Pi thinking level is invalid"));
        }
        validate_run_option_map(&self.run_options)?;
        Ok(())
    }
}

impl ConfiguredModel {
    pub fn validate(&self) -> Result<(), CommandError> {
        if !valid_model_profile_id(&self.id) {
            return Err(CommandError::invalid("Pi model key is invalid"));
        }
        let name = self.name.trim();
        if name.is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
            return Err(CommandError::invalid("model display name is invalid"));
        }
        if !provider_options()
            .iter()
            .any(|option| option.id == self.provider)
        {
            return Err(CommandError::invalid("unsupported Pi model provider"));
        }
        validate_model_identifier(&self.model)?;
        if self.input.is_empty()
            || self.input.iter().any(|input| {
                !matches!(input.as_str(), "text" | "image") || input.chars().any(char::is_control)
            })
            || self
                .input
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.input.len()
        {
            return Err(CommandError::invalid(
                "Pi model input capabilities are invalid",
            ));
        }
        if self.context_window == 0 || self.max_tokens == 0 {
            return Err(CommandError::invalid("Pi model limits are invalid"));
        }
        if self.thinking_levels.is_empty()
            || self.thinking_levels.iter().any(|level| {
                !PI_THINKING_LEVELS.contains(&level.as_str())
                    || level.is_empty()
                    || level.chars().any(char::is_control)
            })
            || self
                .thinking_levels
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.thinking_levels.len()
        {
            return Err(CommandError::invalid(
                "Pi model thinking levels are invalid",
            ));
        }
        if self.thinking_level_map.keys().any(|level| {
            !PI_THINKING_LEVELS.contains(&level.as_str()) || level.chars().any(char::is_control)
        }) || self.thinking_level_map.values().any(|value| {
            value
                .as_deref()
                .is_some_and(|value| value.is_empty() || value.chars().any(char::is_control))
        }) {
            return Err(CommandError::invalid(
                "Pi model thinking level map is invalid",
            ));
        }
        if self.run_controls.len() > MAX_MODEL_RUN_CONTROLS {
            return Err(CommandError::invalid("too many Pi model run controls"));
        }
        let mut control_ids = std::collections::HashSet::new();
        for control in &self.run_controls {
            if !valid_run_option_id(&control.id)
                || !valid_run_option_label(&control.label)
                || control.choices.is_empty()
                || control.choices.len() > 16
                || !control_ids.insert(control.id.as_str())
            {
                return Err(CommandError::invalid("Pi model run control is invalid"));
            }
            let mut choice_ids = std::collections::HashSet::new();
            for choice in &control.choices {
                if !valid_run_option_id(&choice.id)
                    || !valid_run_option_label(&choice.label)
                    || choice.description.len() > 200
                    || choice.description.chars().any(char::is_control)
                    || !choice_ids.insert(choice.id.as_str())
                {
                    return Err(CommandError::invalid(
                        "Pi model run control choice is invalid",
                    ));
                }
            }
            if !choice_ids.contains(control.default_choice.as_str()) {
                return Err(CommandError::invalid(
                    "Pi model run control default is invalid",
                ));
            }
        }
        Ok(())
    }

    pub fn validate_thinking_level(&self, level: &str) -> Result<(), CommandError> {
        if self
            .thinking_levels
            .iter()
            .any(|candidate| candidate == level)
        {
            Ok(())
        } else {
            Err(CommandError::invalid(
                "thinking level is not supported by the selected Pi model",
            ))
        }
    }

    pub fn default_run_options(&self) -> std::collections::BTreeMap<String, String> {
        self.run_controls
            .iter()
            .map(|control| (control.id.clone(), control.default_choice.clone()))
            .collect()
    }

    pub fn validate_run_options(
        &self,
        options: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), CommandError> {
        validate_run_option_map(options)?;
        if options.len() != self.run_controls.len()
            || self.run_controls.iter().any(|control| {
                options.get(&control.id).is_none_or(|choice| {
                    !control
                        .choices
                        .iter()
                        .any(|candidate| candidate.id == *choice)
                })
            })
        {
            return Err(CommandError::invalid(
                "run options are not supported by the selected Pi model",
            ));
        }
        Ok(())
    }
}

fn valid_run_option_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || (index > 0 && (byte.is_ascii_digit() || byte == b'_' || byte == b'-'))
        })
}

fn valid_run_option_label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 80 && !value.chars().any(char::is_control)
}

fn validate_run_option_map(
    options: &std::collections::BTreeMap<String, String>,
) -> Result<(), CommandError> {
    if options.len() > MAX_MODEL_RUN_CONTROLS
        || options
            .iter()
            .any(|(control, choice)| !valid_run_option_id(control) || !valid_run_option_id(choice))
    {
        return Err(CommandError::invalid("Pi model run options are invalid"));
    }
    Ok(())
}

pub(crate) fn valid_model_profile_id(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_MODEL_ID_BYTES
        && !value.chars().any(char::is_control)
}

fn validate_model_identifier(model: &str) -> Result<(), CommandError> {
    if model.trim().is_empty()
        || model.len() > MAX_MODEL_ID_BYTES
        || model.chars().any(char::is_control)
    {
        return Err(CommandError::invalid("Pi model ID is invalid"));
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelCatalog {
    pub models: Vec<ConfiguredModel>,
}

impl ModelCatalog {
    pub fn validate(&self) -> Result<(), CommandError> {
        if self.models.len() > MAX_CONFIGURED_MODELS {
            return Err(CommandError::invalid("too many configured models"));
        }
        let mut ids = std::collections::HashSet::new();
        for model in &self.models {
            model.validate()?;
            if !ids.insert(model.id.as_str()) {
                return Err(CommandError::invalid("Pi model keys must be unique"));
            }
        }
        Ok(())
    }

    pub fn resolve(&self, id: &str) -> Result<ConfiguredModel, CommandError> {
        self.models
            .iter()
            .find(|model| model.id == id)
            .cloned()
            .ok_or_else(|| CommandError::not_found("Pi model"))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelVisibility {
    pub hidden_model_profile_ids: BTreeSet<String>,
}

impl ModelVisibility {
    pub fn validate(&self) -> Result<(), CommandError> {
        if self.hidden_model_profile_ids.len() > MAX_CONFIGURED_MODELS
            || self
                .hidden_model_profile_ids
                .iter()
                .any(|id| !valid_model_profile_id(id))
        {
            return Err(CommandError::invalid(
                "hidden Pi model profile IDs are invalid",
            ));
        }
        Ok(())
    }

    pub fn is_visible(&self, model_profile_id: &str) -> bool {
        !self.hidden_model_profile_ids.contains(model_profile_id)
    }

    pub fn set_visible(&mut self, model_profile_id: &str, visible: bool) {
        if visible {
            self.hidden_model_profile_ids.remove(model_profile_id);
        } else {
            self.hidden_model_profile_ids
                .insert(model_profile_id.to_owned());
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ModelProviderOption {
    pub id: &'static str,
    pub label: &'static str,
    pub credential_label: &'static str,
    pub description: &'static str,
    pub api_key: bool,
    pub oauth: Option<ProviderOauth>,
    pub credential_source: CredentialSource,
    pub recommended: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct ProviderOauth {
    pub label: &'static str,
    #[serde(skip)]
    pub authorization: OauthAuthorizationRule,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OauthAuthorizationRule {
    host: &'static str,
    path: &'static str,
}

impl OauthAuthorizationRule {
    pub const fn exact(host: &'static str, path: &'static str) -> Self {
        Self { host, path }
    }

    pub fn allows(self, host: &str, path: &str) -> bool {
        self.host == host && path.starts_with('/') && self.path == path
    }
}

pub fn catalog_allows_authorization(host: &str, path: &str) -> bool {
    provider_options().iter().any(|provider| {
        provider
            .oauth
            .is_some_and(|oauth| oauth.authorization.allows(host, path))
    })
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    Saved,
    Environment,
    Missing,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfiguredModelView {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub model: String,
    pub input: Vec<String>,
    pub reasoning: bool,
    pub context_window: u64,
    pub max_tokens: u64,
    pub thinking_levels: Vec<String>,
    pub thinking_level_map: std::collections::BTreeMap<String, Option<String>>,
    pub run_controls: Vec<ModelRunControl>,
    pub credential_source: CredentialSource,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelCatalogView {
    pub models: Vec<ConfiguredModelView>,
    pub providers: Vec<ModelProviderOption>,
    pub hidden_model_profile_ids: Vec<String>,
}

pub fn provider_options() -> Vec<ModelProviderOption> {
    vec![
        oauth_provider(
            "openai-codex",
            "OpenAI with ChatGPT",
            "ChatGPT account",
            "Use your ChatGPT Plus, Pro, Business, Edu, or Enterprise account.",
            ProviderOauth {
                label: "Continue with ChatGPT",
                authorization: OauthAuthorizationRule::exact(
                    "auth.openai.com",
                    "/oauth/authorize",
                ),
            },
            false,
            true,
        ),
        oauth_provider(
            "anthropic",
            "Anthropic",
            "Anthropic API key",
            "Use Claude Pro or Max, or an Anthropic API key stored by Pi in Guru Terminal's private app data.",
            ProviderOauth {
                label: "Continue with Claude",
                authorization: OauthAuthorizationRule::exact("claude.ai", "/oauth/authorize"),
            },
            true,
            false,
        ),
        provider("ant-ling", "Ant Ling", "Ant Ling API key"),
        provider("openai", "OpenAI", "OpenAI API key"),
        provider("google", "Google Gemini", "Gemini API key"),
        provider("deepseek", "DeepSeek", "DeepSeek API key"),
        provider("nvidia", "NVIDIA NIM", "NVIDIA API key"),
        provider("mistral", "Mistral", "Mistral API key"),
        provider("groq", "Groq", "Groq API key"),
        provider("cerebras", "Cerebras", "Cerebras API key"),
        oauth_provider(
            "xai",
            "xAI",
            "xAI API key",
            "Use SuperGrok or X Premium, or an xAI API key stored by Pi in Guru Terminal's private app data.",
            ProviderOauth {
                label: "Continue with SuperGrok",
                authorization: OauthAuthorizationRule::exact(
                    "accounts.x.ai",
                    "/oauth2/device",
                ),
            },
            true,
            false,
        ),
        oauth_provider(
            "openrouter",
            "OpenRouter",
            "OpenRouter API key",
            "Sign in with OpenRouter, or store an OpenRouter API key in Guru Terminal's private app data.",
            ProviderOauth {
                label: "Continue with OpenRouter",
                authorization: OauthAuthorizationRule::exact("openrouter.ai", "/auth"),
            },
            true,
            false,
        ),
        provider(
            "vercel-ai-gateway",
            "Vercel AI Gateway",
            "AI Gateway API key",
        ),
        provider("huggingface", "Hugging Face", "Hugging Face token"),
        provider("fireworks", "Fireworks", "Fireworks API key"),
        provider("together", "Together AI", "Together API key"),
        provider("baseten", "Baseten", "Baseten API key"),
        provider("kimi-coding", "Kimi For Coding", "Kimi API key"),
        provider("minimax", "MiniMax", "MiniMax API key"),
        provider("minimax-cn", "MiniMax (China)", "MiniMax China API key"),
        provider("zai", "ZAI Coding Plan", "ZAI API key"),
        provider(
            "zai-coding-cn",
            "ZAI Coding Plan (China)",
            "ZAI China API key",
        ),
        provider("opencode", "OpenCode Zen", "OpenCode API key"),
        provider("opencode-go", "OpenCode Go", "OpenCode API key"),
        provider("radius", "Radius", "Radius API key"),
        provider(
            "qwen-token-plan",
            "Qwen Token Plan",
            "Qwen Token Plan API key",
        ),
        provider(
            "qwen-token-plan-individual",
            "Qwen Token Plan (Individual)",
            "Qwen Token Plan API key",
        ),
        provider(
            "qwen-token-plan-cn",
            "Qwen Token Plan (China)",
            "Qwen Token Plan China API key",
        ),
        provider("xiaomi", "Xiaomi MiMo", "Xiaomi API key"),
        provider(
            "xiaomi-token-plan-cn",
            "Xiaomi MiMo Token Plan (China)",
            "Xiaomi Token Plan API key",
        ),
        provider(
            "xiaomi-token-plan-ams",
            "Xiaomi MiMo Token Plan (Amsterdam)",
            "Xiaomi Token Plan API key",
        ),
        provider(
            "xiaomi-token-plan-sgp",
            "Xiaomi MiMo Token Plan (Singapore)",
            "Xiaomi Token Plan API key",
        ),
    ]
}

const fn provider(
    id: &'static str,
    label: &'static str,
    credential_label: &'static str,
) -> ModelProviderOption {
    ModelProviderOption {
        id,
        label,
        credential_label,
        description: "Connect with an API key stored by Pi in Guru Terminal's private app data.",
        api_key: true,
        oauth: None,
        credential_source: CredentialSource::Missing,
        recommended: false,
    }
}

const fn oauth_provider(
    id: &'static str,
    label: &'static str,
    credential_label: &'static str,
    description: &'static str,
    oauth: ProviderOauth,
    api_key: bool,
    recommended: bool,
) -> ModelProviderOption {
    ModelProviderOption {
        id,
        label,
        credential_label,
        description,
        api_key,
        oauth: Some(oauth),
        credential_source: CredentialSource::Missing,
        recommended,
    }
}

pub fn provider_credential_from_environment(provider: &str) -> Option<(String, String)> {
    if cfg!(feature = "e2e") {
        return None;
    }
    let name = provider_environment_name(provider)?;
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| (name.to_owned(), value))
}

fn provider_environment_name(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "anthropic" => "ANTHROPIC_API_KEY",
        "ant-ling" => "ANT_LING_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "google" => "GEMINI_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "nvidia" => "NVIDIA_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "groq" => "GROQ_API_KEY",
        "cerebras" => "CEREBRAS_API_KEY",
        "xai" => "XAI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "vercel-ai-gateway" => "AI_GATEWAY_API_KEY",
        "huggingface" => "HF_TOKEN",
        "fireworks" => "FIREWORKS_API_KEY",
        "together" => "TOGETHER_API_KEY",
        "baseten" => "BASETEN_API_KEY",
        "kimi-coding" => "KIMI_API_KEY",
        "minimax" => "MINIMAX_API_KEY",
        "minimax-cn" => "MINIMAX_CN_API_KEY",
        "zai" => "ZAI_API_KEY",
        "zai-coding-cn" => "ZAI_CODING_CN_API_KEY",
        "opencode" | "opencode-go" => "OPENCODE_API_KEY",
        "radius" => "RADIUS_API_KEY",
        "qwen-token-plan" | "qwen-token-plan-individual" => "QWEN_TOKEN_PLAN_API_KEY",
        "qwen-token-plan-cn" => "QWEN_TOKEN_PLAN_CN_API_KEY",
        "xiaomi" => "XIAOMI_API_KEY",
        "xiaomi-token-plan-cn" => "XIAOMI_TOKEN_PLAN_CN_API_KEY",
        "xiaomi-token-plan-ams" => "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
        "xiaomi-token-plan-sgp" => "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
        "openai-codex" => return None,
        _ => return None,
    })
}

pub fn catalog_view(
    catalog: &ModelCatalog,
    agent_data_dir: &Path,
    visibility: &ModelVisibility,
) -> Result<ModelCatalogView, CommandError> {
    visibility.validate()?;
    let provider_options = provider_options();
    // auth.json is bounded but may sit on a slower user-data volume. Read and
    // validate it once per view instead of once per model (provider catalogs
    // can contain thousands of entries) and once again per provider.
    let saved_auth = read_auth_map(&agent_data_dir.join("auth.json"))?;
    let credential_sources = provider_options
        .iter()
        .map(|provider| {
            (
                provider.id,
                credential_source_from_auth(&saved_auth, provider.id),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let provider_order = provider_options
        .iter()
        .enumerate()
        .map(|(index, provider)| (provider.id, index))
        .collect::<std::collections::HashMap<_, _>>();
    let mut models = catalog
        .models
        .iter()
        .map(|model| {
            let credential_source = credential_sources
                .get(model.provider.as_str())
                .copied()
                .unwrap_or(CredentialSource::Missing);
            Ok(ConfiguredModelView {
                id: model.id.clone(),
                name: model.name.clone(),
                provider: model.provider.clone(),
                model: model.model.clone(),
                input: model.input.clone(),
                reasoning: model.reasoning,
                context_window: model.context_window,
                max_tokens: model.max_tokens,
                thinking_levels: model.thinking_levels.clone(),
                thinking_level_map: model.thinking_level_map.clone(),
                run_controls: model.run_controls.clone(),
                credential_source,
            })
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    models.sort_by(|left, right| {
        provider_order
            .get(left.provider.as_str())
            .unwrap_or(&usize::MAX)
            .cmp(
                provider_order
                    .get(right.provider.as_str())
                    .unwrap_or(&usize::MAX),
            )
            .then_with(|| compare_model_recency(left, right))
    });
    let providers = provider_options
        .into_iter()
        .map(|mut provider| {
            provider.credential_source = credential_sources
                .get(provider.id)
                .copied()
                .unwrap_or(CredentialSource::Missing);
            Ok(provider)
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    Ok(ModelCatalogView {
        models,
        providers,
        hidden_model_profile_ids: visibility
            .hidden_model_profile_ids
            .iter()
            .cloned()
            .collect(),
    })
}

fn compare_model_recency(left: &ConfiguredModelView, right: &ConfiguredModelView) -> Ordering {
    numeric_runs(&right.model)
        .cmp(&numeric_runs(&left.model))
        .then_with(|| model_tier_rank(right).cmp(&model_tier_rank(left)))
        .then_with(|| right.context_window.cmp(&left.context_window))
        .then_with(|| right.max_tokens.cmp(&left.max_tokens))
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.id.cmp(&right.id))
}

fn model_tier_rank(model: &ConfiguredModelView) -> u8 {
    let value = format!("{} {}", model.name, model.model).to_ascii_lowercase();
    if ["opus", "fable", "ultra", " pro", "-pro", " sol"]
        .iter()
        .any(|marker| value.contains(marker))
    {
        3
    } else if ["sonnet", "terra", "flash", "standard"]
        .iter()
        .any(|marker| value.contains(marker))
    {
        2
    } else if ["haiku", "mini", "nano", "luna", "spark", "lite"]
        .iter()
        .any(|marker| value.contains(marker))
    {
        1
    } else {
        2
    }
}

fn numeric_runs(value: &str) -> Vec<u64> {
    let mut numbers = Vec::new();
    let mut current = None::<u64>;
    for byte in value.bytes() {
        if byte.is_ascii_digit() {
            current = Some(
                current
                    .unwrap_or_default()
                    .saturating_mul(10)
                    .saturating_add(u64::from(byte - b'0')),
            );
        } else if let Some(number) = current.take() {
            numbers.push(number);
        }
    }
    if let Some(number) = current {
        numbers.push(number);
    }
    numbers
}

/// Returns a secret-free, process-local cache generation for the credential
/// authority Pi will use for this provider. Environment credentials take the
/// same precedence as the provider support launch path. OAuth access tokens and
/// expiry timestamps are deliberately excluded: routine token refresh must not
/// invalidate a model catalog, while a refresh-token or account replacement
/// still rotates the generation.
pub(crate) fn provider_credential_generation(
    agent_data_dir: &Path,
    provider: &str,
) -> Result<Option<String>, CommandError> {
    let (source, secret) =
        if let Some((name, secret)) = provider_credential_from_environment(provider) {
            (format!("environment:{name}"), secret)
        } else {
            let auth = read_auth_map(&agent_data_dir.join("auth.json"))?;
            let Some(entry) = auth
                .get(provider)
                .filter(|entry| valid_saved_credential(entry))
            else {
                return Ok(None);
            };
            let Some(entry) = entry.as_object() else {
                return Ok(None);
            };
            match entry.get("type").and_then(Value::as_str) {
                Some("api_key") => (
                    "saved:api_key".to_owned(),
                    entry
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                ),
                Some("oauth") => (
                    "saved:oauth".to_owned(),
                    entry
                        .get("refresh")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                ),
                _ => return Ok(None),
            }
        };
    let mut material = Vec::with_capacity(provider.len() + source.len() + secret.len() + 64);
    material.extend_from_slice(b"guruterminal/provider-credential-generation/v1\0");
    material.extend_from_slice(provider.as_bytes());
    material.push(0);
    material.extend_from_slice(source.as_bytes());
    material.push(0);
    material.extend_from_slice(secret.as_bytes());
    Ok(Some(sha256(&material)))
}

fn credential_source_from_auth(auth: &Map<String, Value>, provider: &str) -> CredentialSource {
    if auth.get(provider).is_some_and(valid_saved_credential) {
        CredentialSource::Saved
    } else if provider_credential_from_environment(provider).is_some() {
        CredentialSource::Environment
    } else {
        CredentialSource::Missing
    }
}

fn valid_saved_credential(value: &Value) -> bool {
    let Some(entry) = value.as_object() else {
        return false;
    };
    match entry.get("type").and_then(Value::as_str) {
        Some("api_key") => entry
            .get("key")
            .and_then(Value::as_str)
            .is_some_and(|key| !key.is_empty()),
        Some("oauth") => {
            entry
                .get("access")
                .and_then(Value::as_str)
                .is_some_and(|token| !token.is_empty())
                && entry.get("refresh").and_then(Value::as_str).is_some()
                && entry.get("expires").and_then(Value::as_u64).is_some()
        }
        _ => false,
    }
}

fn read_auth_map(path: &Path) -> Result<Map<String, Value>, CommandError> {
    if !path.exists() {
        return Ok(Map::new());
    }
    ensure_private_regular_file(path).map_err(|error| CommandError::internal(error.to_string()))?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    if file
        .metadata()
        .map_err(|error| CommandError::internal(error.to_string()))?
        .len()
        > MAX_AUTH_FILE_BYTES
    {
        return Err(CommandError::internal("Pi auth file is too large"));
    }
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|error| CommandError::internal(error.to_string()))?;
    if contents.trim().is_empty() {
        return Ok(Map::new());
    }
    serde_json::from_str::<Value>(&contents)
        .map_err(|_| CommandError::internal("Pi auth file is invalid"))?
        .as_object()
        .cloned()
        .ok_or_else(|| CommandError::internal("Pi auth file is invalid"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn write_auth_fixture(directory: &Path, value: Value) {
        let path = directory.join("auth.json");
        ensure_private_regular_file(&path).unwrap();
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        ensure_private_regular_file(&path).unwrap();
    }

    #[test]
    fn every_exposed_provider_has_a_unique_id_and_environment_route() {
        let providers = provider_options();
        let unique = providers
            .iter()
            .map(|provider| provider.id)
            .collect::<HashSet<_>>();
        assert_eq!(unique.len(), providers.len());
        assert!(providers
            .iter()
            .all(|provider| { provider.api_key || provider.oauth.is_some() }));
        assert!(providers.iter().all(|provider| {
            !provider.api_key
                || provider_environment_name(provider.id)
                    .is_some_and(|name| PROVIDER_CREDENTIAL_ENVIRONMENTS.contains(&name))
        }));
        assert!(providers
            .iter()
            .any(|provider| provider.id == "openai-codex"
                && !provider.api_key
                && provider.oauth.is_some()));
        assert!(providers.iter().any(|provider| {
            provider.id == "xai" && provider.api_key && provider.oauth.is_some()
        }));
    }

    #[test]
    fn oauth_authorization_rules_match_only_the_declared_host_and_path() {
        let openai = OauthAuthorizationRule::exact("auth.openai.com", "/oauth/authorize");
        assert!(openai.allows("auth.openai.com", "/oauth/authorize"));
        assert!(!openai.allows("auth.openai.com", "/other"));
        assert!(!openai.allows("auth.openai.com", "oauth/authorize"));
        let xai = OauthAuthorizationRule::exact("accounts.x.ai", "/oauth2/device");
        assert!(xai.allows("accounts.x.ai", "/oauth2/device"));
        assert!(!xai.allows("accounts.x.ai", "/activate"));
        assert!(!xai.allows("auth.x.ai", "/oauth2/device"));
        assert!(!xai.allows("accounts.x.ai.evil.example", "/oauth2/device"));
        assert!(catalog_allows_authorization(
            "claude.ai",
            "/oauth/authorize"
        ));
        assert!(!catalog_allows_authorization(
            "example.com",
            "/oauth/authorize"
        ));
    }

    #[test]
    fn saved_credentials_are_masked_from_the_catalog_view() {
        let temporary = tempfile::tempdir().unwrap();
        write_auth_fixture(
            temporary.path(),
            serde_json::json!({
                "openai": { "type": "api_key", "key": "secret-value" }
            }),
        );
        let mut visibility = ModelVisibility::default();
        visibility.set_visible("fixture", false);
        let view = catalog_view(
            &ModelCatalog {
                models: vec![ConfiguredModel {
                    id: "fixture".into(),
                    name: "Fixture".into(),
                    provider: "openai".into(),
                    model: "fixture-model".into(),
                    input: vec!["text".into()],
                    reasoning: true,
                    context_window: 128_000,
                    max_tokens: 32_000,
                    thinking_levels: vec![
                        "off".into(),
                        "low".into(),
                        "medium".into(),
                        "high".into(),
                    ],
                    thinking_level_map: std::collections::BTreeMap::new(),
                    run_controls: vec![],
                }],
            },
            temporary.path(),
            &visibility,
        )
        .unwrap();
        assert_eq!(view.models[0].credential_source, CredentialSource::Saved);
        assert_eq!(view.hidden_model_profile_ids, vec!["fixture"]);
        assert!(!visibility.is_visible("fixture"));
        let serialized = serde_json::to_string(&view).unwrap();
        assert!(!serialized.contains("secret-value"));
    }

    #[test]
    fn provider_credential_generation_ignores_oauth_access_refresh_but_rotates_with_authority() {
        let temporary = tempfile::tempdir().unwrap();
        write_auth_fixture(
            temporary.path(),
            serde_json::json!({
                "openai-codex": {
                    "type": "oauth",
                    "access": "access-one",
                    "refresh": "authority-one",
                    "expires": 1_900_000_000_000_u64
                }
            }),
        );
        let first = provider_credential_generation(temporary.path(), "openai-codex")
            .unwrap()
            .unwrap();

        write_auth_fixture(
            temporary.path(),
            serde_json::json!({
                "openai-codex": {
                    "type": "oauth",
                    "access": "access-two",
                    "refresh": "authority-one",
                    "expires": 1_900_000_100_000_u64
                }
            }),
        );
        assert_eq!(
            provider_credential_generation(temporary.path(), "openai-codex")
                .unwrap()
                .unwrap(),
            first
        );

        write_auth_fixture(
            temporary.path(),
            serde_json::json!({
                "openai-codex": {
                    "type": "oauth",
                    "access": "access-three",
                    "refresh": "authority-two",
                    "expires": 1_900_000_200_000_u64
                }
            }),
        );
        assert_ne!(
            provider_credential_generation(temporary.path(), "openai-codex")
                .unwrap()
                .unwrap(),
            first
        );
    }

    #[test]
    fn pi_execution_locks_keep_the_explicitly_selected_level() {
        let model = ConfiguredModel {
            id: "anthropic/claude-sonnet".into(),
            name: "Claude Sonnet".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet".into(),
            input: vec!["text".into(), "image".into()],
            reasoning: true,
            context_window: 200_000,
            max_tokens: 64_000,
            thinking_levels: vec!["off".into(), "low".into(), "medium".into()],
            thinking_level_map: std::collections::BTreeMap::new(),
            run_controls: vec![ModelRunControl {
                id: "performance".into(),
                label: "Performance".into(),
                default_choice: "standard".into(),
                choices: vec![
                    ModelRunControlChoice {
                        id: "standard".into(),
                        label: "Standard".into(),
                        description: "Use the standard tier.".into(),
                    },
                    ModelRunControlChoice {
                        id: "fast".into(),
                        label: "Fast".into(),
                        description: "Use the priority tier.".into(),
                    },
                ],
            }],
        };
        model.validate().unwrap();
        model.validate_thinking_level("low").unwrap();
        assert!(model.validate_thinking_level("high").is_err());
        let fast = std::collections::BTreeMap::from([("performance".into(), "fast".into())]);
        model.validate_run_options(&fast).unwrap();
        assert!(model
            .validate_run_options(&std::collections::BTreeMap::from([(
                "performance".into(),
                "turbo".into()
            )]))
            .is_err());
        let lock = ExecutionModelLock::from_model(&model, "low", &fast);
        assert_eq!(lock.provider, "anthropic");
        assert_eq!(lock.model, "claude-sonnet");
        assert_eq!(lock.thinking_level, "low");
        assert_eq!(lock.run_options.get("performance").unwrap(), "fast");
        lock.validate().unwrap();
    }

    #[test]
    fn pi_provider_catalogs_can_exceed_the_old_manual_profile_limit() {
        let models = (0..128)
            .map(|index| ConfiguredModel {
                id: format!("openrouter/model-{index}"),
                name: format!("Model {index}"),
                provider: "openrouter".into(),
                model: format!("model-{index}"),
                input: vec!["text".into()],
                reasoning: false,
                context_window: 128_000,
                max_tokens: 32_000,
                thinking_levels: vec!["off".into()],
                thinking_level_map: std::collections::BTreeMap::new(),
                run_controls: vec![],
            })
            .collect();

        ModelCatalog { models }.validate().unwrap();
    }

    #[test]
    fn catalog_lists_each_providers_newest_numeric_model_version_first() {
        let temporary = tempfile::tempdir().unwrap();
        let model = |version: &str, variant: &str| ConfiguredModel {
            id: format!("openai-codex/gpt-{version}{variant}"),
            name: format!("GPT {version}{variant}"),
            provider: "openai-codex".into(),
            model: format!("gpt-{version}{variant}"),
            input: vec!["text".into()],
            reasoning: true,
            context_window: 128_000,
            max_tokens: 32_000,
            thinking_levels: vec!["off".into(), "medium".into()],
            thinking_level_map: std::collections::BTreeMap::new(),
            run_controls: vec![],
        };
        let view = catalog_view(
            &ModelCatalog {
                models: vec![
                    model("5.4", ""),
                    model("5.10", ""),
                    model("5.6", " Luna"),
                    model("5.6", " Sol"),
                    model("5.6", " Terra"),
                ],
            },
            temporary.path(),
            &ModelVisibility::default(),
        )
        .unwrap();

        assert_eq!(
            view.models
                .iter()
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>(),
            [
                "gpt-5.10",
                "gpt-5.6 Sol",
                "gpt-5.6 Terra",
                "gpt-5.6 Luna",
                "gpt-5.4"
            ]
        );
    }

    #[test]
    fn pi_models_can_explicitly_disable_the_off_thinking_level() {
        let model = ConfiguredModel {
            id: "fixture/always-thinking".into(),
            name: "Always Thinking".into(),
            provider: "openai".into(),
            model: "always-thinking".into(),
            input: vec!["text".into()],
            reasoning: true,
            context_window: 128_000,
            max_tokens: 32_000,
            thinking_levels: vec!["high".into(), "max".into()],
            thinking_level_map: std::collections::BTreeMap::from([
                ("off".into(), None),
                ("high".into(), Some("high".into())),
                ("max".into(), Some("max".into())),
            ]),
            run_controls: vec![],
        };

        model.validate().unwrap();
        model.validate_thinking_level("high").unwrap();
        model.validate_thinking_level("max").unwrap();
        assert!(model.validate_thinking_level("off").is_err());
    }

    #[test]
    fn oauth_credentials_are_recognized_without_reaching_the_renderer() {
        let temporary = tempfile::tempdir().unwrap();
        write_auth_fixture(
            temporary.path(),
            serde_json::json!({
                "openai-codex": {
                    "type": "oauth",
                    "access": "access-secret",
                    "refresh": "refresh-secret",
                    "expires": 1_900_000_000_000_u64,
                    "accountId": "account"
                }
            }),
        );
        let view = catalog_view(
            &ModelCatalog { models: vec![] },
            temporary.path(),
            &ModelVisibility::default(),
        )
        .unwrap();
        let openai = view
            .providers
            .iter()
            .find(|provider| provider.id == "openai-codex")
            .unwrap();
        assert_eq!(openai.credential_source, CredentialSource::Saved);
        let serialized = serde_json::to_string(&view).unwrap();
        assert!(!serialized.contains("access-secret"));
        assert!(!serialized.contains("refresh-secret"));
        assert!(!serialized.contains("authorization"));
        assert!(!serialized.contains("auth.openai.com"));
    }

    #[test]
    fn minted_oauth_keys_without_refresh_tokens_are_recognized() {
        let temporary = tempfile::tempdir().unwrap();
        write_auth_fixture(
            temporary.path(),
            serde_json::json!({
                "openrouter": {
                    "type": "oauth",
                    "access": "or-secret",
                    "refresh": "",
                    "expires": 9_007_199_254_740_991_u64
                }
            }),
        );
        let view = catalog_view(
            &ModelCatalog { models: vec![] },
            temporary.path(),
            &ModelVisibility::default(),
        )
        .unwrap();
        let openrouter = view
            .providers
            .iter()
            .find(|provider| provider.id == "openrouter")
            .unwrap();
        assert_eq!(openrouter.credential_source, CredentialSource::Saved);
        assert!(openrouter.api_key);
        assert_eq!(
            openrouter.oauth.as_ref().map(|oauth| oauth.label),
            Some("Continue with OpenRouter")
        );
        let serialized = serde_json::to_string(&view).unwrap();
        assert!(!serialized.contains("or-secret"));
    }
}
