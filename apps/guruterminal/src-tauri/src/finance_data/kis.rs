use super::*;

mod manifest;
mod protocol;

pub(super) use manifest::*;
pub(super) use protocol::*;

/// Native-only values required by KIS account and profile-scoped reads.
///
/// This type intentionally has no `Debug` or serialization implementation.
/// React and Pi cannot construct it; the broker may derive it only from the
/// active credential bundle already verified by the native Marketplace flow.
pub(crate) struct KisAccountProfile {
    values: BTreeMap<String, String>,
}

impl KisAccountProfile {
    pub(crate) fn from_values(values: BTreeMap<String, String>) -> Result<Self, FinanceDataError> {
        const KEYS: [&str; 6] = [
            "account_number",
            "account_product_code",
            "account_password",
            "customer_identity_number",
            "home_net_id",
            "hts_id",
        ];
        if values.is_empty()
            || values.len() > KEYS.len()
            || values.keys().any(|key| !KEYS.contains(&key.as_str()))
            || values.values().any(|value| {
                value.len() > 128 || value.chars().any(|character| character.is_control())
            })
            || values.get("account_number").is_some_and(|value| {
                value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit())
            })
            || values.get("account_product_code").is_some_and(|value| {
                value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
        {
            return Err(FinanceDataError::InvalidQuery(
                "KIS account profile is invalid",
            ));
        }
        Ok(Self { values })
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn has_required_values_for(&self, operation: &KisOperation) -> bool {
        operation.parameters.iter().all(|parameter| {
            if parameter.source != KisParameterSource::AccountProfile || !parameter.required {
                return true;
            }
            parameter
                .profile_key
                .as_deref()
                .and_then(|key| self.get(key))
                .or_else(|| parameter.default.as_ref().and_then(Value::as_str))
                .is_some_and(|value| !value.is_empty())
        })
    }

    fn sensitive_echo_values(&self) -> impl Iterator<Item = &str> {
        self.values.iter().filter_map(|(key, value)| {
            (key != "account_product_code" && value.len() >= 4).then_some(value.as_str())
        })
    }
}
impl FinanceDataService {
    pub(crate) fn kis_operation_search(
        &self,
        params: Value,
        profile: Option<&KisAccountProfile>,
    ) -> Result<Value, FinanceDataError> {
        let object = params.as_object().ok_or(FinanceDataError::InvalidQuery(
            "expected the KIS operation search schema",
        ))?;
        if object.keys().any(|key| {
            !matches!(key.as_str(), "query" | "product" | "limit")
                || !object.get(key).is_some_and(Value::is_string)
        }) {
            return Err(FinanceDataError::InvalidQuery(
                "expected the KIS operation search schema",
            ));
        }
        let query = object
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if query.len() > 128 || query.chars().any(char::is_control) {
            return Err(FinanceDataError::InvalidQuery(
                "KIS operation search text is invalid",
            ));
        }
        let product = object
            .get("product")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        if product.is_some_and(|value| {
            value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        }) {
            return Err(FinanceDataError::InvalidQuery(
                "KIS operation search product is invalid",
            ));
        }
        let limit = object
            .get("limit")
            .and_then(Value::as_str)
            .unwrap_or("10")
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAX_KIS_OPERATION_SEARCH_RESULTS).contains(limit))
            .ok_or(FinanceDataError::InvalidQuery(
                "KIS operation search limit must be from 1 through 20",
            ))?;
        let query_terms = query
            .split_whitespace()
            .map(|term| term.to_lowercase())
            .collect::<Vec<_>>();
        let matches = self
            .kis_manifest
            .operations
            .iter()
            .filter(|operation| operation.available_to_agent(profile))
            .filter(|operation| product.is_none_or(|product| operation.product == product))
            .filter(|operation| {
                let haystack = format!(
                    "{} {} {} {}",
                    operation.id, operation.product, operation.category, operation.name
                )
                .to_lowercase();
                query_terms.iter().all(|term| haystack.contains(term))
            })
            .take(limit)
            .map(kis_operation_descriptor)
            .collect::<Vec<_>>();
        let returned = matches.len();
        Ok(json!({
            "schema_version": "guruterminal-kis-operation-search/1",
            "source_id": KIS_SOURCE_ID,
            "query": query,
            "product": product,
            "operations": matches,
            "page": {
                "returned": returned,
                "limit": limit
            },
            "policy": {
                "public_reads_available_v1": self.kis_manifest.public_operation_count(),
                "profile_gated_market_reads": self.kis_manifest.profile_gated_market_operation_count(),
                "account_reads_available_v1": self.kis_manifest.agent_scope_operation_count(profile, KisOperationScope::Account),
                "agent_reads_available": self.kis_manifest.agent_operation_count(profile),
                "orders_available": false
            }
        }))
    }

    pub(crate) async fn clear_kis_token_cache(&self) {
        self.kis_tokens.lock().await.clear();
    }

    pub(crate) async fn kis_read_data(
        &self,
        params: Value,
        environment: &str,
        app_key: &str,
        app_secret: &str,
        profile: Option<&KisAccountProfile>,
    ) -> Result<Value, FinanceDataError> {
        let environment = KisEnvironment::parse(environment)?;
        let query: KisToolQuery = strict_query(params, "expected the exact KIS read schema")?;
        // Reject operations and parameters at the native authority boundary
        // before credentials are used for a provider token request.
        prepare_kis_request(
            &self.kis_manifest,
            &query,
            environment,
            profile,
            KisRequestAuthority::AgentRead,
        )?;
        let token = self
            .kis_access_token(environment, app_key, app_secret)
            .await?;
        let prepared = prepare_kis_request(
            &self.kis_manifest,
            &query,
            token.environment,
            profile,
            KisRequestAuthority::AgentRead,
        )?;
        let raw = self
            .fetch_kis_request(&prepared, &token, app_key, app_secret)
            .await?;
        reject_kis_credential_echo(&raw.bytes, app_key, app_secret, &token.value)?;
        match prepared.operation.scope {
            KisOperationScope::Market => normalize_kis_market_response(&prepared, &raw, profile),
            KisOperationScope::Account => normalize_kis_account_response(&prepared, &raw, profile),
        }
    }

    async fn fetch_kis_request(
        &self,
        prepared: &PreparedKisRequest<'_>,
        token: &KisAccessToken,
        app_key: &str,
        app_secret: &str,
    ) -> Result<RawProviderResponse, FinanceDataError> {
        let mut request = self
            .client
            .get(prepared.url.clone())
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", token.value),
            )
            .header("appkey", app_key)
            .header("appsecret", app_secret)
            .header("tr_id", prepared.tr_id)
            .header("custtype", "P");
        if let Some(header) = prepared
            .operation
            .continuation
            .as_ref()
            .and_then(|continuation| continuation.request_header.as_deref())
        {
            request = request.header(header, "");
        }
        self.fetch_provider(
            request,
            KIS_SOURCE_ID,
            format!(
                "{}{}",
                prepared.url.origin().ascii_serialization(),
                prepared.operation.path
            ),
            MAX_KIS_RESPONSE_BYTES,
        )
        .await
    }

    async fn kis_access_token(
        &self,
        environment: KisEnvironment,
        app_key: &str,
        app_secret: &str,
    ) -> Result<KisAccessToken, FinanceDataError> {
        validate_api_key(app_key, KIS_SOURCE_ID)?;
        validate_api_key(app_secret, KIS_SOURCE_ID)?;
        let fingerprint = kis_credential_fingerprint(environment, app_key, app_secret);
        let mut tokens = self.kis_tokens.lock().await;
        if let Some(token) = tokens
            .get(&fingerprint)
            .filter(|token| token.expires_at > Utc::now() + ChronoDuration::seconds(30))
        {
            return Ok(token.clone());
        }
        tokens.remove(&fingerprint);

        let token = self
            .issue_kis_token(environment, app_key, app_secret)
            .await?;
        if tokens.len() >= MAX_KIS_TOKEN_CACHE_ENTRIES {
            tokens.clear();
        }
        tokens.insert(fingerprint, token.clone());
        Ok(token)
    }

    pub(super) async fn issue_kis_token(
        &self,
        environment: KisEnvironment,
        app_key: &str,
        app_secret: &str,
    ) -> Result<KisAccessToken, FinanceDataError> {
        let mut url =
            Url::parse(environment.api_root()).map_err(|_| FinanceDataError::InvalidResponse)?;
        url.set_path(KIS_TOKEN_PATH);
        url.set_query(None);
        let (status, raw) = self
            .send_provider_bounded(
                self.client.post(url.clone()).json(&json!({
                    "grant_type": "client_credentials",
                    "appkey": app_key,
                    "appsecret": app_secret
                })),
                KIS_SOURCE_ID,
                url.to_string(),
                64 * 1024,
            )
            .await?;
        if !status.is_success() {
            return Err(classify_kis_token_error(
                status, &raw.bytes, app_key, app_secret,
            ));
        }
        parse_kis_token_response(&raw.bytes, environment, app_key, app_secret)
    }
}
#[cfg(test)]
mod tests;
