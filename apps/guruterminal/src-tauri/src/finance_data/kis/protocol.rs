use super::*;

pub(in crate::finance_data) fn kis_credentials(
    credentials: &BTreeMap<String, String>,
) -> Result<(&str, &str), FinanceDataError> {
    if credentials.len() != 2 {
        return Err(FinanceDataError::InvalidQuery(
            "KIS requires one app key and one app secret",
        ));
    }
    let app_key = credentials
        .get("app_key")
        .map(String::as_str)
        .ok_or(FinanceDataError::InvalidQuery("KIS app key is required"))?;
    let app_secret = credentials
        .get("app_secret")
        .map(String::as_str)
        .ok_or(FinanceDataError::InvalidQuery("KIS app secret is required"))?;
    validate_api_key(app_key, KIS_SOURCE_ID)?;
    validate_api_key(app_secret, KIS_SOURCE_ID)?;
    Ok((app_key, app_secret))
}

pub(in crate::finance_data) fn valid_kis_parameter_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(in crate::finance_data) fn valid_kis_profile_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

pub(in crate::finance_data) fn valid_kis_wire_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(in crate::finance_data) fn kis_credential_fingerprint(
    environment: KisEnvironment,
    app_key: &str,
    app_secret: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"guruterminal/kis-credential-cache/v1\0");
    hasher.update(environment.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(app_key.as_bytes());
    hasher.update(b"\0");
    hasher.update(app_secret.as_bytes());
    hex::encode(hasher.finalize())
}

pub(in crate::finance_data) fn deserialize_kis_expires_in<'de, D>(
    deserializer: D,
) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom("KIS token expiry is invalid")),
        Value::String(value) => value
            .parse::<i64>()
            .map_err(|_| serde::de::Error::custom("KIS token expiry is invalid")),
        _ => Err(serde::de::Error::custom("KIS token expiry is invalid")),
    }
}

pub(in crate::finance_data) fn parse_kis_token_response(
    bytes: &[u8],
    environment: KisEnvironment,
    app_key: &str,
    app_secret: &str,
) -> Result<KisAccessToken, FinanceDataError> {
    let response: KisTokenResponse = serde_json::from_slice(bytes).map_err(|_| {
        let root = serde_json::from_slice::<Value>(bytes).ok();
        if root.as_ref().is_some_and(|root| {
            root.get("error_code").is_some()
                || root.get("error_description").is_some()
                || root.get("msg_cd").is_some()
        }) {
            classify_kis_token_error(StatusCode::OK, bytes, app_key, app_secret)
        } else {
            FinanceDataError::InvalidResponse
        }
    })?;
    if response.token_type != "Bearer"
        || response.access_token.len() < 16
        || response.access_token.len() > 4_096
        || response
            .access_token
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        || !(60..=172_800).contains(&response.expires_in)
    {
        return Err(FinanceDataError::InvalidResponse);
    }
    Ok(KisAccessToken {
        value: response.access_token,
        environment,
        expires_at: Utc::now() + ChronoDuration::seconds(response.expires_in),
    })
}

pub(in crate::finance_data) fn classify_kis_token_error(
    status: StatusCode,
    bytes: &[u8],
    app_key: &str,
    app_secret: &str,
) -> FinanceDataError {
    let diagnostic = safe_kis_token_diagnostic(bytes, app_key, app_secret);
    let summary = diagnostic.summary().to_ascii_lowercase();
    if status == StatusCode::TOO_MANY_REQUESTS
        || summary.contains("egw00133")
        || summary.contains("1분당")
        || summary.contains("too many")
        || summary.contains("rate limit")
    {
        FinanceDataError::KisRateLimited(diagnostic)
    } else if matches!(
        status,
        StatusCode::OK | StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        FinanceDataError::KisCredentialRejected(diagnostic)
    } else {
        FinanceDataError::Provider(format!("{KIS_SOURCE_ID} returned HTTP {}", status.as_u16()))
    }
}

pub(in crate::finance_data) fn safe_kis_token_diagnostic(
    bytes: &[u8],
    app_key: &str,
    app_secret: &str,
) -> SafeProviderDiagnostic {
    let root = serde_json::from_slice::<Value>(bytes).ok();
    let code = root
        .as_ref()
        .and_then(|value| value.get("error_code").or_else(|| value.get("msg_cd")))
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .map(str::to_owned);
    let message = root
        .as_ref()
        .and_then(|value| value.get("error_description").or_else(|| value.get("msg1")))
        .and_then(Value::as_str)
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 512
                && !value.contains(app_key)
                && !value.contains(app_secret)
                && !value.chars().any(char::is_control)
        });
    SafeProviderDiagnostic { code, message }
}

pub(in crate::finance_data) fn normalize_kis_market_response(
    prepared: &PreparedKisRequest<'_>,
    raw: &RawProviderResponse,
    profile: Option<&KisAccountProfile>,
) -> Result<Value, FinanceDataError> {
    let root = validate_kis_response(&raw.bytes, prepared.operation)?;
    let (response, dropped_unknown_fields) = sanitize_kis_response(root, prepared.operation);
    if contains_sensitive_kis_response_key(&response)
        || profile.is_some_and(|profile| contains_kis_profile_value(&response, profile))
    {
        return Err(FinanceDataError::InvalidResponse);
    }
    let returned = kis_response_record_count(&response);
    if returned == 0 {
        return Err(FinanceDataError::NoData(KIS_SOURCE_ID));
    }
    let truncated = raw
        .continuation
        .as_deref()
        .is_some_and(|value| matches!(value, "F" | "M"));
    let data = json!({
        "operation": {
            "id": prepared.operation.id,
            "product": prepared.operation.product,
            "category": prepared.operation.category,
            "name": prepared.operation.name
        },
        "response": response
    });
    let warnings = {
        let mut warnings = vec![
            "KIS V1 validates the reviewed request, provider success envelope, and operation-level response-field allowlist, but not request-to-response identity. Treat fields that depend on response identity with caution."
                .to_owned(),
        ];
        if truncated {
            warnings.push(
                "The bounded KIS response has another provider page; V1 returned only the first page."
                    .to_owned(),
            );
        }
        if dropped_unknown_fields {
            warnings.push(
                "KIS returned fields outside the reviewed response schema; those fields were omitted."
                    .to_owned(),
            );
        }
        warnings
    };
    Ok(json!({
        "schema_version": "guruterminal-kis-result/1",
        "tool": "finance_market_data",
        "operation": "market.operation",
        "source_id": KIS_SOURCE_ID,
        "query": {
            "provider": KIS_SOURCE_ID,
            "operation_id": prepared.operation.id,
            "params": prepared.tool_params
        },
        "data": data,
        "provenance": {
            "data_authority": "Korea Investment & Securities",
            "provider": "Korea Investment Open Trading API",
            "official_source": true,
            "source_origin": format!(
                "{}{}",
                prepared.url.origin().ascii_serialization(),
                prepared.operation.path
            ),
            "retrieved_at": raw.retrieved_at,
            "normalized_sha256": normalized_sha256(&data)?,
            "raw_persisted": false,
            "revision_semantics": "latest_only"
        },
        "quality": {
            "source_class": "official",
            "status": "warn",
            "checks": [
                {"code": "provider_success", "status": "pass"},
                {"code": "response_schema_allowlist", "status": "pass"},
                {"code": "response_identity", "status": "warn"}
            ],
            "warnings": warnings
        },
        "warnings": warnings,
        "page": {
            "returned": returned,
            "available": returned,
            "truncated": truncated
        }
    }))
}

pub(in crate::finance_data) fn normalize_kis_account_response(
    prepared: &PreparedKisRequest<'_>,
    raw: &RawProviderResponse,
    profile: Option<&KisAccountProfile>,
) -> Result<Value, FinanceDataError> {
    let root = validate_kis_response(&raw.bytes, prepared.operation)?;
    let (response, dropped_unknown_fields) = sanitize_kis_response(root, prepared.operation);
    if contains_sensitive_kis_response_key(&response)
        || profile.is_some_and(|profile| contains_kis_profile_value(&response, profile))
    {
        return Err(FinanceDataError::InvalidResponse);
    }
    let returned = kis_response_record_count(&response);
    if returned == 0 {
        return Err(FinanceDataError::NoData(KIS_SOURCE_ID));
    }
    let truncated = raw
        .continuation
        .as_deref()
        .is_some_and(|value| matches!(value, "F" | "M"));
    let data = json!({
        "operation": {
            "id": prepared.operation.id,
            "product": prepared.operation.product,
            "category": prepared.operation.category,
            "name": prepared.operation.name
        },
        "response": response
    });
    let warnings = {
        let mut warnings = vec![
            "This account result is projected through an operation-level response allowlist and strips account-shaped fields and reflected profile values, but it has no request-to-response identity rule."
                .to_owned(),
        ];
        if truncated {
            warnings.push(
                "The bounded KIS account response has another provider page; only the first page was normalized."
                    .to_owned(),
            );
        }
        if dropped_unknown_fields {
            warnings.push(
                "KIS returned fields outside the reviewed response schema; those fields were omitted."
                    .to_owned(),
            );
        }
        warnings
    };
    Ok(json!({
        "schema_version": "guruterminal-kis-account-result/1",
        "tool": "finance_market_data",
        "source_id": KIS_SOURCE_ID,
        "operation": "account.read",
        "query": {
            "provider": KIS_SOURCE_ID,
            "operation_id": prepared.operation.id,
            "params": prepared.tool_params
        },
        "data": data,
        "provenance": {
            "data_authority": "Korea Investment & Securities",
            "provider": "Korea Investment Open Trading API",
            "official_source": true,
            "source_origin": format!(
                "{}{}",
                prepared.url.origin().ascii_serialization(),
                prepared.operation.path
            ),
            "retrieved_at": raw.retrieved_at,
            "normalized_sha256": normalized_sha256(&data)?,
            "raw_persisted": false
        },
        "quality": {
            "status": "warn",
            "checks": [
                {"code": "provider_success", "status": "pass"},
                {"code": "response_schema_allowlist", "status": "pass"},
                {"code": "response_identity", "status": "warn"},
                {"code": "account_identifiers_redacted", "status": "warn"}
            ],
            "warnings": warnings
        },
        "page": {
            "returned": returned,
            "available": returned,
            "truncated": truncated
        }
    }))
}

pub(in crate::finance_data) fn validate_kis_response(
    bytes: &[u8],
    operation: &KisOperation,
) -> Result<Value, FinanceDataError> {
    let mismatch = |detail: String| FinanceDataError::KisResponseContract {
        operation: operation.id.clone(),
        detail,
    };
    let root: Value = serde_json::from_slice(bytes)
        .map_err(|_| mismatch("provider body was not valid JSON".to_owned()))?;
    let object = root
        .as_object()
        .ok_or_else(|| mismatch("top-level value was not an object".to_owned()))?;
    let status = object
        .get("rt_cd")
        .and_then(Value::as_str)
        .ok_or_else(|| mismatch("rt_cd was missing or not a string".to_owned()))?;
    if status == "0" {
        validate_kis_success_shape(object, operation)?;
        return Ok(root);
    }
    let code = object
        .get("msg_cd")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let message = object
        .get("msg1")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if [code.as_str(), message.as_str()].iter().any(|value| {
        value.contains("token")
            || value.contains("appkey")
            || value.contains("app key")
            || value.contains("인증")
            || value.contains("접근토큰")
    }) {
        return Err(FinanceDataError::CredentialRejected(KIS_SOURCE_ID));
    }
    if [code.as_str(), message.as_str()].iter().any(|value| {
        value.contains("rate")
            || value.contains("초당")
            || value.contains("호출 횟수")
            || value.contains("too many")
    }) {
        return Err(FinanceDataError::RateLimited(KIS_SOURCE_ID));
    }
    Err(FinanceDataError::Provider(format!(
        "{KIS_SOURCE_ID} rejected reviewed operation {}",
        operation.id
    )))
}

pub(in crate::finance_data) fn validate_kis_success_shape(
    object: &Map<String, Value>,
    operation: &KisOperation,
) -> Result<(), FinanceDataError> {
    let contract = &operation.response;
    let mismatch = |detail: String| FinanceDataError::KisResponseContract {
        operation: operation.id.clone(),
        detail,
    };
    let safe_names = |names: Vec<&str>| {
        let unsupported_kind = names.first().map(|name| {
            if name.is_empty() {
                "blank field name"
            } else if name.chars().all(char::is_whitespace) {
                "whitespace-only field name"
            } else if !name.is_ascii() {
                "non-ASCII field name"
            } else if name.len() > 64 {
                "overlong field name"
            } else {
                "field name with unsupported punctuation"
            }
        });
        let names = names
            .into_iter()
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= 64
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            })
            .take(8)
            .collect::<Vec<_>>();
        if names.is_empty() {
            unsupported_kind
                .unwrap_or("unsupported field name")
                .to_owned()
        } else {
            names.join(", ")
        }
    };
    let allowed_fields = contract
        .allowed_fields
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let unknown_top_level = object
        .keys()
        .filter(|key| {
            !matches!(key.as_str(), "rt_cd" | "msg_cd" | "msg1")
                && !contract
                    .containers
                    .iter()
                    .any(|container| container == *key)
                && !contract.top_level_fields.iter().any(|field| field == *key)
        })
        .map(String::as_str)
        .collect::<Vec<_>>();
    if unknown_top_level
        .iter()
        .any(|field| sensitive_kis_response_key(field))
    {
        return Err(mismatch(format!(
            "unexpected sensitive top-level field(s): {}",
            safe_names(
                unknown_top_level
                    .into_iter()
                    .filter(|field| sensitive_kis_response_key(field))
                    .collect(),
            )
        )));
    }
    let missing_containers = contract
        .containers
        .iter()
        .filter(|container| !object.contains_key(*container))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if !missing_containers.is_empty() {
        return Err(mismatch(format!(
            "missing response container(s): {}",
            safe_names(missing_containers)
        )));
    }
    for field in &contract.top_level_fields {
        if object.get(field).is_none_or(|value| {
            !matches!(
                value,
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
            )
        }) {
            return Err(mismatch(format!(
                "top-level field {} was missing or non-scalar",
                safe_names(vec![field])
            )));
        }
    }

    for container in &contract.containers {
        let value = object
            .get(container)
            .expect("the required response containers were checked above");
        let rows = match value {
            Value::Array(rows) => rows.iter().collect::<Vec<_>>(),
            Value::Object(row) if row.is_empty() => Vec::new(),
            Value::Object(_) => vec![value],
            _ => {
                return Err(mismatch(format!(
                    "container {} was not an object or array",
                    safe_names(vec![container])
                )));
            }
        };
        for row in rows {
            let row = row.as_object().ok_or_else(|| {
                mismatch(format!(
                    "container {} included a non-object row",
                    safe_names(vec![container])
                ))
            })?;
            if row.is_empty() {
                return Err(mismatch(format!(
                    "container {} included an empty row",
                    safe_names(vec![container])
                )));
            }
            let unknown_fields = row
                .keys()
                .filter(|key| !allowed_fields.contains(key.as_str()))
                .map(String::as_str)
                .collect::<Vec<_>>();
            if unknown_fields
                .iter()
                .any(|field| sensitive_kis_response_key(field))
            {
                return Err(mismatch(format!(
                    "container {} included unexpected sensitive field(s): {}",
                    safe_names(vec![container]),
                    safe_names(
                        unknown_fields
                            .into_iter()
                            .filter(|field| sensitive_kis_response_key(field))
                            .collect(),
                    )
                )));
            }
            if row.iter().any(|(field, value)| {
                allowed_fields.contains(field.as_str())
                    && !matches!(
                        value,
                        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
                    )
            }) {
                return Err(mismatch(format!(
                    "container {} included a non-scalar field value",
                    safe_names(vec![container])
                )));
            }
        }
    }
    Ok(())
}

pub(in crate::finance_data) fn reject_kis_credential_echo(
    bytes: &[u8],
    app_key: &str,
    app_secret: &str,
    access_token: &str,
) -> Result<(), FinanceDataError> {
    fn canonical_key(key: &str) -> String {
        key.bytes()
            .filter(|byte| byte.is_ascii_alphanumeric())
            .map(|byte| byte.to_ascii_lowercase() as char)
            .collect()
    }

    fn contains_credential(
        value: &Value,
        app_key: &str,
        app_secret: &str,
        access_token: &str,
    ) -> bool {
        match value {
            Value::Array(values) => values
                .iter()
                .any(|value| contains_credential(value, app_key, app_secret, access_token)),
            Value::Object(object) => object.iter().any(|(key, value)| {
                matches!(
                    canonical_key(key).as_str(),
                    "appkey"
                        | "appsecret"
                        | "accesstoken"
                        | "authorization"
                        | "bearertoken"
                        | "token"
                ) || contains_credential(value, app_key, app_secret, access_token)
            }),
            Value::String(value) => {
                value.contains(app_key)
                    || value.contains(app_secret)
                    || value.contains(access_token)
                    || value.contains(&format!("Bearer {access_token}"))
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => false,
        }
    }

    let root: Value =
        serde_json::from_slice(bytes).map_err(|_| FinanceDataError::InvalidResponse)?;
    if contains_credential(&root, app_key, app_secret, access_token) {
        return Err(FinanceDataError::InvalidResponse);
    }
    Ok(())
}

pub(in crate::finance_data) fn sanitize_kis_response(
    mut value: Value,
    operation: &KisOperation,
) -> (Value, bool) {
    fn retain_allowed_row(
        value: &mut Value,
        allowed_fields: &std::collections::BTreeSet<&str>,
    ) -> bool {
        match value {
            Value::Array(rows) => {
                let mut dropped = false;
                for row in rows {
                    dropped |= retain_allowed_row(row, allowed_fields);
                }
                dropped
            }
            Value::Object(row) => {
                let before = row.len();
                row.retain(|key, _| {
                    allowed_fields.contains(key.as_str()) && !sensitive_kis_response_key(key)
                });
                row.len() != before
            }
            _ => false,
        }
    }

    let allowed_fields = operation
        .response
        .allowed_fields
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let mut dropped_unknown_fields = false;
    if let Some(object) = value.as_object_mut() {
        let before = object.len();
        object.retain(|key, _| {
            operation
                .response
                .containers
                .iter()
                .any(|container| container == key)
                || operation
                    .response
                    .top_level_fields
                    .iter()
                    .any(|field| field == key)
        });
        dropped_unknown_fields |= object.len() + 3 < before;
        for container in &operation.response.containers {
            if let Some(rows) = object.get_mut(container) {
                dropped_unknown_fields |= retain_allowed_row(rows, &allowed_fields);
            }
        }
    }
    (value, dropped_unknown_fields)
}

pub(in crate::finance_data) fn sensitive_kis_response_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let canonical = key
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect::<String>();
    matches!(
        key.as_str(),
        "cano"
            | "acnt_prdt_cd"
            | "acnt_pwd"
            | "acnt_no"
            | "account_no"
            | "acct_no"
            | "acnt_name"
            | "cust_name"
            | "cust_rncno25"
            | "user_id"
            | "hts_id"
            | "hmid"
            | "odno"
            | "orgn_odno"
            | "ord_gno_brno"
            | "inqr_ip_addr"
            | "ctac_tlno"
    ) || matches!(
        canonical.as_str(),
        "appkey" | "appsecret" | "accesstoken" | "authorization" | "bearertoken" | "token"
    ) || key.contains("address")
        || key.contains("addr")
        || key.contains("email")
        || key.ends_with("_tlno")
}

pub(in crate::finance_data) fn contains_sensitive_kis_response_key(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(contains_sensitive_kis_response_key),
        Value::Object(object) => object.iter().any(|(key, value)| {
            sensitive_kis_response_key(key) || contains_sensitive_kis_response_key(value)
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

pub(in crate::finance_data) fn contains_kis_profile_value(
    value: &Value,
    profile: &KisAccountProfile,
) -> bool {
    match value {
        Value::Array(values) => values
            .iter()
            .any(|value| contains_kis_profile_value(value, profile)),
        Value::Object(object) => object
            .values()
            .any(|value| contains_kis_profile_value(value, profile)),
        Value::String(value) => profile
            .sensitive_echo_values()
            .any(|sensitive| value.contains(sensitive)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

pub(in crate::finance_data) fn kis_response_record_count(response: &Value) -> usize {
    response
        .as_object()
        .into_iter()
        .flat_map(|object| object.iter())
        .filter(|(key, _)| key.starts_with("output"))
        .map(|(_, value)| match value {
            Value::Array(values) => values.len(),
            Value::Object(object) if !object.is_empty() => 1,
            Value::String(value) if !value.is_empty() => 1,
            Value::Number(_) | Value::Bool(_) => 1,
            _ => 0,
        })
        .sum()
}
