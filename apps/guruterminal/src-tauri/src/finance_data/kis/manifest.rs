use super::*;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::finance_data) enum KisEnvironment {
    Real,
    Demo,
}

impl KisEnvironment {
    pub(in crate::finance_data) fn parse(value: &str) -> Result<Self, FinanceDataError> {
        match value {
            "real" => Ok(Self::Real),
            "demo" => Ok(Self::Demo),
            _ => Err(FinanceDataError::InvalidQuery(
                "KIS environment must be real or demo",
            )),
        }
    }

    pub(in crate::finance_data) fn api_root(self) -> &'static str {
        match self {
            Self::Real => KIS_REAL_API_ROOT,
            Self::Demo => KIS_DEMO_API_ROOT,
        }
    }

    pub(in crate::finance_data) fn as_str(self) -> &'static str {
        match self {
            Self::Real => "real",
            Self::Demo => "demo",
        }
    }
}

#[derive(Clone)]
pub(in crate::finance_data) struct KisAccessToken {
    pub(in crate::finance_data) value: String,
    pub(in crate::finance_data) environment: KisEnvironment,
    pub(in crate::finance_data) expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub(in crate::finance_data) struct KisTokenResponse {
    pub(in crate::finance_data) access_token: String,
    pub(in crate::finance_data) token_type: String,
    #[serde(deserialize_with = "deserialize_kis_expires_in")]
    pub(in crate::finance_data) expires_in: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisToolQuery {
    pub(in crate::finance_data) operation_id: String,
    pub(in crate::finance_data) params: BTreeMap<String, String>,
}

pub(in crate::finance_data) struct PreparedKisRequest<'a> {
    pub(in crate::finance_data) operation: &'a KisOperation,
    pub(in crate::finance_data) url: Url,
    pub(in crate::finance_data) tr_id: &'a str,
    pub(in crate::finance_data) tool_params: BTreeMap<String, String>,
}

#[derive(Clone, Copy)]
pub(in crate::finance_data) enum KisRequestAuthority {
    AgentRead,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisManifest {
    pub(in crate::finance_data) schema: String,
    pub(in crate::finance_data) upstream: KisManifestUpstream,
    pub(in crate::finance_data) policy: KisManifestPolicy,
    pub(in crate::finance_data) counts: KisManifestCounts,
    pub(in crate::finance_data) excluded_write_operation_ids: Vec<String>,
    pub(in crate::finance_data) operations: Vec<KisOperation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisManifestUpstream {
    pub(in crate::finance_data) repository: String,
    pub(in crate::finance_data) commit: String,
    pub(in crate::finance_data) config_root: String,
    pub(in crate::finance_data) examples_root: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisManifestPolicy {
    pub(in crate::finance_data) fixed_hosts: BTreeMap<String, String>,
    pub(in crate::finance_data) http_methods: Vec<String>,
    pub(in crate::finance_data) orders_included: bool,
    pub(in crate::finance_data) account_reads_available_in_v1: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisManifestCounts {
    pub(in crate::finance_data) read_operations: usize,
    pub(in crate::finance_data) market_reads: usize,
    pub(in crate::finance_data) account_reads: usize,
    pub(in crate::finance_data) excluded_writes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisOperation {
    pub(in crate::finance_data) id: String,
    pub(in crate::finance_data) product: String,
    pub(in crate::finance_data) category: String,
    pub(in crate::finance_data) name: String,
    pub(in crate::finance_data) scope: KisOperationScope,
    pub(in crate::finance_data) http_method: String,
    pub(in crate::finance_data) path: String,
    pub(in crate::finance_data) tr_id_rules: Vec<KisTransactionIdRule>,
    #[serde(default)]
    pub(in crate::finance_data) parameters: Vec<KisParameter>,
    #[serde(default)]
    pub(in crate::finance_data) query: Vec<KisQueryField>,
    #[serde(default)]
    pub(in crate::finance_data) continuation: Option<KisContinuation>,
    pub(in crate::finance_data) response: KisResponseContract,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(in crate::finance_data) enum KisOperationScope {
    Market,
    Account,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisTransactionIdRule {
    pub(in crate::finance_data) value: String,
    #[serde(default)]
    pub(in crate::finance_data) when: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisParameter {
    pub(in crate::finance_data) id: String,
    #[serde(rename = "type")]
    pub(in crate::finance_data) value_type: String,
    pub(in crate::finance_data) required: bool,
    #[serde(default)]
    pub(in crate::finance_data) default: Option<Value>,
    pub(in crate::finance_data) description: String,
    pub(in crate::finance_data) source: KisParameterSource,
    #[serde(default)]
    pub(in crate::finance_data) profile_key: Option<String>,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(in crate::finance_data) enum KisParameterSource {
    Tool,
    AccountProfile,
    Continuation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisQueryField {
    pub(in crate::finance_data) wire_name: String,
    #[serde(default)]
    pub(in crate::finance_data) parameter: Option<String>,
    #[serde(default)]
    pub(in crate::finance_data) literal: Option<String>,
    pub(in crate::finance_data) send: KisQuerySend,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(in crate::finance_data) enum KisQuerySend {
    Always,
    IfPresent,
    IfNonempty,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisContinuation {
    #[serde(default)]
    pub(in crate::finance_data) request_header: Option<String>,
    #[serde(default)]
    pub(in crate::finance_data) response_header: Option<String>,
    #[serde(default)]
    pub(in crate::finance_data) query_fields: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::finance_data) struct KisResponseContract {
    pub(in crate::finance_data) containers: Vec<String>,
    pub(in crate::finance_data) top_level_fields: Vec<String>,
    pub(in crate::finance_data) allowed_fields: Vec<String>,
    pub(in crate::finance_data) field_scope: String,
    pub(in crate::finance_data) source: String,
    pub(in crate::finance_data) complete: bool,
    pub(in crate::finance_data) unknown_fields: KisUnknownFieldPolicy,
    pub(in crate::finance_data) identity_checks: Vec<Value>,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(in crate::finance_data) enum KisUnknownFieldPolicy {
    Drop,
}

impl KisManifest {
    pub(in crate::finance_data) fn parse(encoded: &str) -> Result<Self, FinanceDataError> {
        if hex::encode(Sha256::digest(encoded.as_bytes())) != KIS_MANIFEST_SHA256 {
            return Err(FinanceDataError::Configuration(
                "bundled KIS operation manifest digest does not match its reviewed source"
                    .to_owned(),
            ));
        }
        let manifest: Self = serde_json::from_str(encoded).map_err(|error| {
            FinanceDataError::Configuration(format!(
                "bundled KIS operation manifest is invalid: {error}"
            ))
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub(in crate::finance_data) fn validate(&self) -> Result<(), FinanceDataError> {
        if self.schema != "guruterminal-kis-read-api/1"
            || self.upstream.repository != "https://github.com/koreainvestment/open-trading-api"
            || self.upstream.config_root != "MCP/Kis Trading MCP/configs"
            || self.upstream.examples_root != "examples_llm"
            || self.upstream.commit != KIS_UPSTREAM_COMMIT
            || self.policy.fixed_hosts.get("real").map(String::as_str) != Some(KIS_REAL_API_ROOT)
            || self.policy.fixed_hosts.get("demo").map(String::as_str) != Some(KIS_DEMO_API_ROOT)
            || self.policy.fixed_hosts.len() != 2
            || self.policy.http_methods != ["GET"]
            || self.policy.orders_included
            || !self.policy.account_reads_available_in_v1
            || self.counts.read_operations != 146
            || self.counts.market_reads != 91
            || self.counts.account_reads != 55
            || self.counts.excluded_writes != 18
            || self.excluded_write_operation_ids.len() != 18
            || self.operations.len() != 146
            || self.market_operation_count() != 91
            || self.account_operation_count() != 55
            || self.public_operation_count() != 88
            || self.profile_gated_market_operation_count() != 3
        {
            return Err(FinanceDataError::Configuration(
                "bundled KIS operation manifest does not match its pinned contract".to_owned(),
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for operation in &self.operations {
            validate_kis_operation(operation)?;
            if !ids.insert(operation.id.as_str()) {
                return Err(FinanceDataError::Configuration(
                    "bundled KIS operation IDs must be unique".to_owned(),
                ));
            }
        }
        let excluded = self
            .excluded_write_operation_ids
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        let expected_excluded = KIS_EXCLUDED_WRITE_OPERATION_IDS
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        if excluded.len() != 18
            || excluded != expected_excluded
            || excluded.iter().any(|id| !valid_kis_operation_id(id))
            || ids.iter().any(|id| excluded.contains(id))
        {
            return Err(FinanceDataError::Configuration(
                "bundled KIS write exclusions are invalid".to_owned(),
            ));
        }
        Ok(())
    }

    pub(in crate::finance_data) fn market_operation_count(&self) -> usize {
        self.operations
            .iter()
            .filter(|operation| operation.scope == KisOperationScope::Market)
            .count()
    }

    pub(in crate::finance_data) fn account_operation_count(&self) -> usize {
        self.operations
            .iter()
            .filter(|operation| operation.scope == KisOperationScope::Account)
            .count()
    }

    pub(in crate::finance_data) fn public_operation_count(&self) -> usize {
        self.operations
            .iter()
            .filter(|operation| !operation.requires_account_profile())
            .count()
    }

    pub(in crate::finance_data) fn agent_operation_count(
        &self,
        profile: Option<&KisAccountProfile>,
    ) -> usize {
        self.operations
            .iter()
            .filter(|operation| operation.available_to_agent(profile))
            .count()
    }

    pub(in crate::finance_data) fn agent_scope_operation_count(
        &self,
        profile: Option<&KisAccountProfile>,
        scope: KisOperationScope,
    ) -> usize {
        self.operations
            .iter()
            .filter(|operation| operation.scope == scope && operation.available_to_agent(profile))
            .count()
    }

    pub(in crate::finance_data) fn profile_gated_market_operation_count(&self) -> usize {
        self.operations
            .iter()
            .filter(|operation| {
                operation.scope == KisOperationScope::Market && operation.requires_account_profile()
            })
            .count()
    }

    pub(in crate::finance_data) fn operation(&self, id: &str) -> Option<&KisOperation> {
        self.operations.iter().find(|operation| operation.id == id)
    }
}

impl KisOperation {
    pub(in crate::finance_data) fn requires_account_profile(&self) -> bool {
        self.parameters
            .iter()
            .any(|parameter| parameter.source == KisParameterSource::AccountProfile)
    }

    pub(in crate::finance_data) fn available_to_agent(
        &self,
        profile: Option<&KisAccountProfile>,
    ) -> bool {
        !self.requires_account_profile()
            || profile.is_some_and(|profile| profile.has_required_values_for(self))
    }
}

impl KisOperationScope {
    pub(in crate::finance_data) fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Account => "account",
        }
    }
}

pub(in crate::finance_data) fn validate_kis_operation(
    operation: &KisOperation,
) -> Result<(), FinanceDataError> {
    if !valid_kis_operation_id(&operation.id)
        || operation.product.is_empty()
        || operation.category.is_empty()
        || operation.name.is_empty()
        || !operation.path.starts_with("/uapi/")
        || operation.path.contains("..")
        || operation.path.contains('?')
        || operation.path.to_ascii_lowercase().contains("oauth")
        || operation.http_method != "GET"
        || operation.parameters.len() > 64
        || operation.query.len() > 64
    {
        return Err(FinanceDataError::Configuration(
            "bundled KIS operation has an unsafe shape".to_owned(),
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    for parameter in &operation.parameters {
        if !valid_kis_parameter_name(&parameter.id)
            || !matches!(
                parameter.value_type.as_str(),
                "string" | "integer" | "number" | "boolean"
            )
            || parameter.description.is_empty()
            || parameter.description.len() > 4_096
            || !ids.insert(parameter.id.as_str())
        {
            return Err(FinanceDataError::Configuration(
                "bundled KIS operation parameter is invalid".to_owned(),
            ));
        }
    }
    for parameter in &operation.parameters {
        match parameter.source {
            KisParameterSource::AccountProfile
                if parameter
                    .profile_key
                    .as_deref()
                    .is_none_or(|key| !valid_kis_profile_key(key)) =>
            {
                return Err(FinanceDataError::Configuration(
                    "bundled KIS account-profile mapping is invalid".to_owned(),
                ));
            }
            KisParameterSource::Tool | KisParameterSource::Continuation
                if parameter.profile_key.is_some() =>
            {
                return Err(FinanceDataError::Configuration(
                    "bundled KIS non-profile parameter named a profile key".to_owned(),
                ));
            }
            _ => {}
        }
    }
    let mut wire_names = std::collections::BTreeSet::new();
    for field in &operation.query {
        if !valid_kis_wire_name(&field.wire_name)
            || !wire_names.insert(field.wire_name.as_str())
            || field.parameter.is_some() == field.literal.is_some()
            || field
                .parameter
                .as_ref()
                .is_some_and(|parameter| !ids.contains(parameter.as_str()))
            || field.literal.as_ref().is_some_and(|literal| {
                literal.len() > MAX_KIS_PARAMETER_CHARS || literal.chars().any(char::is_control)
            })
        {
            return Err(FinanceDataError::Configuration(
                "bundled KIS query mapping is invalid".to_owned(),
            ));
        }
    }
    match &operation.continuation {
        Some(continuation)
            if continuation.request_header.as_deref() != Some("tr_cont")
                || continuation.response_header.as_deref() != Some("tr_cont")
                || continuation.query_fields.iter().any(|wire_name| {
                    !valid_kis_wire_name(wire_name)
                        || !operation
                            .query
                            .iter()
                            .any(|field| field.wire_name == *wire_name)
                }) =>
        {
            return Err(FinanceDataError::Configuration(
                "bundled KIS continuation contract is invalid".to_owned(),
            ));
        }
        None if operation
            .parameters
            .iter()
            .any(|parameter| parameter.source == KisParameterSource::Continuation) =>
        {
            return Err(FinanceDataError::Configuration(
                "bundled KIS continuation parameters have no contract".to_owned(),
            ));
        }
        _ => {}
    }
    validate_kis_tr_ids(&operation.tr_id_rules, &ids)?;
    validate_kis_response_contract(operation)?;
    Ok(())
}

pub(in crate::finance_data) fn validate_kis_response_contract(
    operation: &KisOperation,
) -> Result<(), FinanceDataError> {
    let response = &operation.response;
    let expected_source = format!(
        "examples_llm/{}/{}/chk_{}.py:COLUMN_MAPPING",
        operation.product,
        operation
            .id
            .split_once('.')
            .map(|(_, id)| id)
            .unwrap_or_default(),
        operation
            .id
            .split_once('.')
            .map(|(_, id)| id)
            .unwrap_or_default(),
    );
    if response.containers.is_empty()
        || response.containers.len() > 8
        || response
            .containers
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || response.containers.iter().any(|container| {
            !container.starts_with("output")
                || container.len() > 16
                || !container.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        || response
            .top_level_fields
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || response.top_level_fields.iter().any(|field| {
            !valid_kis_wire_name(field)
                || response
                    .containers
                    .iter()
                    .any(|container| container == field)
                || matches!(field.as_str(), "rt_cd" | "msg_cd" | "msg1")
        })
        || response.allowed_fields.is_empty()
        || response.allowed_fields.len() > 1_024
        || response
            .allowed_fields
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || response.allowed_fields.iter().any(|field| {
            !valid_kis_wire_name(field) || field.len() > 128 || field.chars().any(char::is_control)
        })
        || response.field_scope != "operation_union"
        || response.source != expected_source
        || !response.complete
        || response.unknown_fields != KisUnknownFieldPolicy::Drop
        || !response.identity_checks.is_empty()
    {
        return Err(FinanceDataError::Configuration(
            "bundled KIS response contract is invalid".to_owned(),
        ));
    }
    Ok(())
}

pub(in crate::finance_data) fn valid_kis_operation_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.contains('.')
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

pub(in crate::finance_data) fn validate_kis_tr_ids(
    rules: &[KisTransactionIdRule],
    parameter_ids: &std::collections::BTreeSet<&str>,
) -> Result<(), FinanceDataError> {
    let valid = |value: &str| {
        !value.is_empty()
            && value.len() <= 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    };
    if rules.len() < 2
        || rules.len() > 32
        || rules.iter().any(|rule| {
            !valid(&rule.value)
                || rule
                    .when
                    .get("environment")
                    .is_none_or(|value| !matches!(value.as_str(), "real" | "demo"))
                || rule.when.keys().any(|parameter| {
                    parameter != "environment" && !parameter_ids.contains(parameter.as_str())
                })
        })
        || !["real", "demo"].into_iter().all(|environment| {
            rules
                .iter()
                .any(|rule| rule.when.get("environment").map(String::as_str) == Some(environment))
        })
    {
        return Err(FinanceDataError::Configuration(
            "bundled KIS transaction rules are invalid".to_owned(),
        ));
    }
    Ok(())
}

pub(in crate::finance_data) fn prepare_kis_request<'a>(
    manifest: &'a KisManifest,
    query: &KisToolQuery,
    environment: KisEnvironment,
    profile: Option<&KisAccountProfile>,
    authority: KisRequestAuthority,
) -> Result<PreparedKisRequest<'a>, FinanceDataError> {
    let operation =
        manifest
            .operation(&query.operation_id)
            .ok_or(FinanceDataError::InvalidQuery(
                "KIS operation ID is not in the reviewed manifest",
            ))?;
    let KisRequestAuthority::AgentRead = authority;
    if !operation.available_to_agent(profile) {
        return Err(FinanceDataError::InvalidQuery(
            "KIS operation requires a configured account profile",
        ));
    }

    let tool_parameter_ids = operation
        .parameters
        .iter()
        .filter(|parameter| parameter.source == KisParameterSource::Tool)
        .map(|parameter| parameter.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if query.params.len() > 64
        || query
            .params
            .keys()
            .any(|parameter| !tool_parameter_ids.contains(parameter.as_str()))
    {
        return Err(FinanceDataError::InvalidQuery(
            "KIS parameters do not match the reviewed operation",
        ));
    }

    let mut resolved = BTreeMap::new();
    let mut tool_params = BTreeMap::new();
    for parameter in &operation.parameters {
        let supplied = match parameter.source {
            KisParameterSource::Tool => query.params.get(&parameter.id).map(String::as_str),
            KisParameterSource::AccountProfile => profile.and_then(|profile| {
                parameter
                    .profile_key
                    .as_deref()
                    .and_then(|key| profile.get(key))
            }),
            KisParameterSource::Continuation => Some(""),
        };
        let default = parameter.default.as_ref().and_then(Value::as_str);
        let value = supplied.or(default);
        if parameter.required
            && parameter.source != KisParameterSource::Continuation
            && value.is_none_or(str::is_empty)
        {
            return Err(FinanceDataError::InvalidQuery(
                "KIS operation is missing one required parameter",
            ));
        }
        if let Some(value) = value {
            validate_kis_parameter_value(parameter, value)?;
            resolved.insert(parameter.id.as_str(), value.to_owned());
            if parameter.source == KisParameterSource::Tool {
                tool_params.insert(parameter.id.clone(), value.to_owned());
            }
        }
    }

    let matching_rules = operation
        .tr_id_rules
        .iter()
        .filter(|rule| {
            rule.when.iter().all(|(key, expected)| {
                if key == "environment" {
                    expected == environment.as_str()
                } else {
                    resolved.get(key.as_str()).map(String::as_str) == Some(expected.as_str())
                }
            })
        })
        .collect::<Vec<_>>();
    let [rule] = matching_rules.as_slice() else {
        return Err(FinanceDataError::InvalidQuery(
            "KIS operation parameters did not select one transaction ID",
        ));
    };

    let mut url =
        Url::parse(environment.api_root()).map_err(|_| FinanceDataError::InvalidResponse)?;
    url.set_path(&operation.path);
    url.set_query(None);
    {
        let mut query_pairs = url.query_pairs_mut();
        for field in &operation.query {
            let value = match (&field.parameter, &field.literal) {
                (Some(parameter), None) => resolved.get(parameter.as_str()).map(String::as_str),
                (None, Some(literal)) => Some(literal.as_str()),
                _ => return Err(FinanceDataError::InvalidResponse),
            };
            let should_send = match field.send {
                KisQuerySend::Always => true,
                KisQuerySend::IfPresent => value.is_some(),
                KisQuerySend::IfNonempty => value.is_some_and(|value| !value.is_empty()),
            };
            if should_send {
                query_pairs.append_pair(
                    &field.wire_name,
                    value.ok_or(FinanceDataError::InvalidQuery(
                        "KIS operation is missing one required wire value",
                    ))?,
                );
            }
        }
    }
    Ok(PreparedKisRequest {
        operation,
        url,
        tr_id: &rule.value,
        tool_params,
    })
}

pub(in crate::finance_data) fn validate_kis_parameter_value(
    parameter: &KisParameter,
    value: &str,
) -> Result<(), FinanceDataError> {
    if value.len() > MAX_KIS_PARAMETER_CHARS || value.chars().any(char::is_control) {
        return Err(FinanceDataError::InvalidQuery(
            "KIS operation parameter is invalid",
        ));
    }
    let valid_type = match parameter.value_type.as_str() {
        "string" => true,
        "integer" => value.parse::<i64>().is_ok(),
        "number" => value.parse::<f64>().is_ok_and(f64::is_finite),
        "boolean" => matches!(value, "true" | "false"),
        _ => false,
    };
    if !valid_type {
        return Err(FinanceDataError::InvalidQuery(
            "KIS operation parameter type is invalid",
        ));
    }
    Ok(())
}

pub(in crate::finance_data) fn kis_operation_descriptor(operation: &KisOperation) -> Value {
    let parameters = operation
        .parameters
        .iter()
        .filter(|parameter| parameter.source == KisParameterSource::Tool)
        .map(|parameter| {
            json!({
                "id": parameter.id,
                "type": parameter.value_type,
                "required": parameter.required,
                "default": parameter.default,
                "description": parameter.description
            })
        })
        .collect::<Vec<_>>();
    json!({
        "operation_id": operation.id,
        "product": operation.product,
        "category": operation.category,
        "name": operation.name,
        "scope": operation.scope.as_str(),
        "parameters": parameters
    })
}
