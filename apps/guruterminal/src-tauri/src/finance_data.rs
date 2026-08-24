mod entity;
mod kis;
mod provider_common;
mod provider_korea;
mod world_bank;

pub(crate) use kis::KisAccountProfile;
use kis::*;
use provider_common::*;

use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, SecondsFormat, Utc};
use reqwest::{redirect::Policy, Client, RequestBuilder, StatusCode, Url};
use scraper::Html;
use serde::{de::DeserializeOwned, Deserialize, Deserializer};
use serde_json::{json, Map, Value};
pub(crate) use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Mutex;

const WORLD_BANK_SOURCE_ID: &str = "world-bank.indicators";
const WORLD_BANK_API_ROOT: &str = "https://api.worldbank.org/v2";
pub(super) const OPENDART_SOURCE_ID: &str = "opendart.disclosures";
pub(super) const OPENDART_API_ROOT: &str = "https://opendart.fss.or.kr";
const KRX_SOURCE_ID: &str = "krx.market-data";
const KRX_API_ROOT: &str = "https://data-dbg.krx.co.kr";
pub(crate) const KIS_SOURCE_ID: &str = "koreainvestment.market-data";
const KIS_REAL_API_ROOT: &str = "https://openapi.koreainvestment.com:9443";
const KIS_DEMO_API_ROOT: &str = "https://openapivts.koreainvestment.com:29443";
const KIS_TOKEN_PATH: &str = "/oauth2/tokenP";
const KIS_UPSTREAM_COMMIT: &str = "b093e42ba32d1df5f5ddad7a71cb715cbc800832";
const KIS_MANIFEST_SHA256: &str =
    "c343d5681ea14f40c5ef5e1f51f543aa248e32f7ecf030df40dc381bee0f667a";
const KIS_EXCLUDED_WRITE_OPERATION_IDS: [&str; 18] = [
    "domestic_bond.buy",
    "domestic_bond.order_rvsecncl",
    "domestic_bond.sell",
    "domestic_futureoption.order",
    "domestic_futureoption.order_rvsecncl",
    "domestic_stock.order_cash",
    "domestic_stock.order_credit",
    "domestic_stock.order_resv",
    "domestic_stock.order_resv_rvsecncl",
    "domestic_stock.order_rvsecncl",
    "overseas_futureoption.order",
    "overseas_futureoption.order_rvsecncl",
    "overseas_stock.daytime_order",
    "overseas_stock.daytime_order_rvsecncl",
    "overseas_stock.order",
    "overseas_stock.order_resv",
    "overseas_stock.order_resv_ccnl",
    "overseas_stock.order_rvsecncl",
];
const KIS_MANIFEST_JSON: &str = include_str!("../../marketplace/kis-read-api-v1.json");
const MAX_KIS_PARAMETER_CHARS: usize = 1_024;
const MAX_KIS_TOKEN_CACHE_ENTRIES: usize = 4;
const MAX_KIS_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_KIS_OPERATION_SEARCH_RESULTS: usize = 20;
const MAX_PROVIDER_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_LARGE_PROVIDER_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_DECOMPRESSED_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_DOCUMENT_TEXT_CHARS: usize = 200_000;
const MAX_FACT_ROWS: usize = 500;
const MAX_FILING_ROWS: usize = 100;
const MAX_YEAR_SPAN: i32 = 120;
const FINANCE_SOURCE_IDS: [&str; 4] = [
    WORLD_BANK_SOURCE_ID,
    OPENDART_SOURCE_ID,
    KRX_SOURCE_ID,
    KIS_SOURCE_ID,
];

#[derive(Debug, Error)]
pub enum FinanceDataError {
    #[error("finance data request is invalid: {0}")]
    InvalidQuery(&'static str),
    #[error("finance data request is invalid: {0}")]
    InvalidRequest(String),
    #[error("finance data provider request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("finance data provider network request failed: {0}")]
    NetworkSafe(&'static str),
    #[error("finance data provider credential was rejected: {0}")]
    CredentialRejected(&'static str),
    #[error("KIS credential was rejected: {0}")]
    KisCredentialRejected(SafeProviderDiagnostic),
    #[error("finance data provider rate limit was reached: {0}")]
    RateLimited(&'static str),
    #[error("KIS token verification is temporarily limited: {0}")]
    KisRateLimited(SafeProviderDiagnostic),
    #[error("finance data provider returned no matching data: {0}")]
    NoData(&'static str),
    #[error("finance data provider rejected the request: {0}")]
    Provider(String),
    #[error("finance data provider returned an invalid response")]
    InvalidResponse,
    #[error("KIS response contract mismatch for {operation}: {detail}")]
    KisResponseContract { operation: String, detail: String },
    #[error("finance data configuration is invalid: {0}")]
    Configuration(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafeProviderDiagnostic {
    code: Option<String>,
    message: Option<String>,
}

impl SafeProviderDiagnostic {
    pub(crate) fn summary(&self) -> String {
        match (&self.code, &self.message) {
            (Some(code), Some(message)) => format!("{code}: {message}"),
            (Some(code), None) => code.clone(),
            (None, Some(message)) => message.clone(),
            (None, None) => "No provider diagnostic was returned.".to_owned(),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(code: &str, message: &str) -> Self {
        Self {
            code: Some(code.to_owned()),
            message: Some(message.to_owned()),
        }
    }
}

impl std::fmt::Display for SafeProviderDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.summary())
    }
}

#[derive(Clone)]
pub struct FinanceDataService {
    client: Client,
    source_inventory: Value,
    kis_manifest: Arc<KisManifest>,
    kis_tokens: Arc<Mutex<BTreeMap<String, KisAccessToken>>>,
    entity_directories: Arc<Mutex<entity::EntityDirectoryCache>>,
}

struct RawProviderResponse {
    bytes: Vec<u8>,
    source_url: String,
    retrieved_at: String,
    continuation: Option<String>,
}

fn build_finance_source_inventory(kis_manifest: &KisManifest) -> Result<Value, FinanceDataError> {
    let catalog = crate::marketplace::bundled_catalog().map_err(|error| {
        FinanceDataError::Configuration(format!(
            "bundled Marketplace catalog could not be projected into finance sources: {error}"
        ))
    })?;

    let mut sources = Vec::with_capacity(FINANCE_SOURCE_IDS.len());
    for source_id in FINANCE_SOURCE_IDS {
        let mut matches = catalog.entries.iter().filter(|entry| entry.id == source_id);
        let entry = matches.next().ok_or_else(|| {
            FinanceDataError::Configuration(format!(
                "bundled Marketplace catalog is missing finance source {source_id}"
            ))
        })?;
        if matches.next().is_some()
            || !matches!(
                entry.release_stage,
                crate::marketplace::MarketplaceReleaseStage::Available
                    | crate::marketplace::MarketplaceReleaseStage::Preview
            )
            || entry.capabilities.is_empty()
            || entry.permissions.network_hosts.is_empty()
            || entry.trust != crate::marketplace::MarketplaceTrust::FirstParty
        {
            return Err(FinanceDataError::Configuration(format!(
                "bundled Marketplace finance source {source_id} is invalid"
            )));
        }
        let source = Map::from_iter([
            ("id".to_owned(), Value::String(entry.id.clone())),
            ("name".to_owned(), Value::String(entry.name.clone())),
            (
                "data_authority".to_owned(),
                Value::String(entry.data_authority.clone()),
            ),
            (
                "status".to_owned(),
                serde_json::to_value(entry.release_stage)
                    .map_err(|error| FinanceDataError::Configuration(error.to_string()))?,
            ),
            (
                "access".to_owned(),
                Value::String(finance_source_access(entry)?),
            ),
            (
                "trust".to_owned(),
                serde_json::to_value(entry.trust)
                    .map_err(|error| FinanceDataError::Configuration(error.to_string()))?,
            ),
            (
                "official_source".to_owned(),
                Value::Bool(finance_source_is_official(source_id)),
            ),
            (
                "capabilities".to_owned(),
                serde_json::to_value(&entry.capabilities)
                    .map_err(|error| FinanceDataError::Configuration(error.to_string()))?,
            ),
            (
                "network_hosts".to_owned(),
                serde_json::to_value(&entry.permissions.network_hosts)
                    .map_err(|error| FinanceDataError::Configuration(error.to_string()))?,
            ),
            (
                "terms_url".to_owned(),
                entry
                    .terms_url
                    .as_ref()
                    .map_or(Value::Null, |url| Value::String(url.clone())),
            ),
            (
                "query_contract".to_owned(),
                finance_source_query_contract(source_id, kis_manifest),
            ),
        ]);
        sources.push(Value::Object(source));
    }
    Ok(json!({
        "schema_version": "guruterminal-finance-sources/1",
        "sources": sources
    }))
}

fn finance_source_access(
    entry: &crate::marketplace::MarketplaceEntryDto,
) -> Result<String, FinanceDataError> {
    use crate::marketplace::{MarketplaceFreeState, MarketplaceSetupFieldKind};
    let setup = entry.setup.as_ref();
    let credential_kinds = setup
        .into_iter()
        .flat_map(|setup| setup.credential_fields.iter())
        .map(|field| field.kind)
        .collect::<Vec<_>>();
    let config_kinds = setup
        .into_iter()
        .flat_map(|setup| setup.config_fields.iter())
        .map(|field| field.kind)
        .collect::<Vec<_>>();
    match (credential_kinds.as_slice(), config_kinds.as_slice()) {
        ([MarketplaceSetupFieldKind::ApiKey], []) => Ok("api_key".to_owned()),
        (credential_kinds, [MarketplaceSetupFieldKind::Select])
            if entry.id == KIS_SOURCE_ID
                && credential_kinds.len() >= 2
                && credential_kinds
                    .iter()
                    .all(|kind| *kind == MarketplaceSetupFieldKind::ApiKey) =>
        {
            Ok("app_credentials".to_owned())
        }
        ([], [MarketplaceSetupFieldKind::Email]) => Ok("contact_email".to_owned()),
        ([], []) if entry.free_state == MarketplaceFreeState::Keyless => Ok("keyless".to_owned()),
        _ => Err(FinanceDataError::Configuration(format!(
            "bundled Marketplace finance source {} has an unsupported access contract",
            entry.id
        ))),
    }
}

fn finance_source_is_official(source_id: &str) -> bool {
    FINANCE_SOURCE_IDS.contains(&source_id)
}

fn finance_source_query_contract(source_id: &str, kis_manifest: &KisManifest) -> Value {
    match source_id {
        WORLD_BANK_SOURCE_ID => json!({
            "tool": "finance_macro_data",
            "provider": WORLD_BANK_SOURCE_ID,
            "economy": "ISO 3166-1 alpha-2/alpha-3 or World Bank economy code",
            "indicator": "one World Bank indicator code",
            "year_span_limit": MAX_YEAR_SPAN
        }),
        OPENDART_SOURCE_ID => json!({
            "company_scoped": true,
            "maximum_rows": MAX_FACT_ROWS
        }),
        KRX_SOURCE_ID => json!({
            "intervals": ["1d"],
            "one_trading_date": true,
            "adjustment": "raw"
        }),
        KIS_SOURCE_ID => json!({
            "tool": "finance_market_data",
            "provider": KIS_SOURCE_ID,
            "operation_inventory": "bundled_static_manifest",
            "discovery_operation_id": "catalog.search",
            "discovery_params": ["query", "product", "limit"],
            "upstream_commit": kis_manifest.upstream.commit,
            "public_read_operations": kis_manifest.public_operation_count(),
            "profile_gated_market_operations": kis_manifest.profile_gated_market_operation_count(),
            "account_read_operations": kis_manifest.account_operation_count(),
            "account_reads_available_v1": true,
            "account_profile_required": true,
            "orders_available": false
        }),
        _ => Value::Null,
    }
}

impl FinanceDataService {
    pub fn new() -> Result<Self, FinanceDataError> {
        let kis_manifest = Arc::new(KisManifest::parse(KIS_MANIFEST_JSON)?);
        let source_inventory = build_finance_source_inventory(&kis_manifest)?;
        let client = Client::builder()
            .https_only(true)
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("GuruTerminal/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            client,
            source_inventory,
            kis_manifest,
            kis_tokens: Arc::new(Mutex::new(BTreeMap::new())),
            entity_directories: Arc::new(Mutex::new(entity::EntityDirectoryCache::default())),
        })
    }

    pub fn sources(&self) -> Value {
        self.source_inventory.clone()
    }

    pub async fn verify_credential(
        &self,
        entry_id: &str,
        credentials: &BTreeMap<String, String>,
        provider_context: Option<&str>,
    ) -> Result<(), FinanceDataError> {
        match entry_id {
            OPENDART_SOURCE_ID => {
                let secret = single_api_key(credentials)?;
                validate_api_key(secret, OPENDART_SOURCE_ID)?;
                let today = Utc::now().date_naive().format("%Y%m%d").to_string();
                let mut public = fixed_url(OPENDART_API_ROOT, &["api", "list.json"])?;
                public
                    .query_pairs_mut()
                    .append_pair("bgn_de", &today)
                    .append_pair("end_de", &today)
                    .append_pair("page_count", "1");
                let mut request = public.clone();
                request.query_pairs_mut().append_pair("crtfc_key", secret);
                let raw = self
                    .fetch_provider(
                        self.client.get(request),
                        OPENDART_SOURCE_ID,
                        public.to_string(),
                        MAX_PROVIDER_BYTES,
                    )
                    .await?;
                validate_dart_status_allow_no_data(&raw.bytes)?;
            }
            KRX_SOURCE_ID => {
                let secret = single_api_key(credentials)?;
                validate_api_key(secret, KRX_SOURCE_ID)?;
                let mut public = fixed_url(KRX_API_ROOT, &["svc", "apis", "sto", "stk_bydd_trd"])?;
                public.query_pairs_mut().append_pair(
                    "basDd",
                    &Utc::now().date_naive().format("%Y%m%d").to_string(),
                );
                let raw = self
                    .fetch_provider(
                        self.client.get(public.clone()).header("AUTH_KEY", secret),
                        KRX_SOURCE_ID,
                        public.to_string(),
                        MAX_PROVIDER_BYTES,
                    )
                    .await?;
                detect_krx_error(&raw.bytes)?;
            }
            KIS_SOURCE_ID => {
                let (app_key, app_secret) = kis_credentials(credentials)?;
                let environment = KisEnvironment::parse(provider_context.ok_or(
                    FinanceDataError::InvalidQuery("KIS environment is required"),
                )?)?;
                // Verification must always reach the selected authority. An
                // execution token cached for the same pair cannot promote a
                // newly staged credential candidate.
                self.issue_kis_token(environment, app_key, app_secret)
                    .await?;
            }
            _ => {
                return Err(FinanceDataError::InvalidQuery(
                    "credential provider is not supported",
                ))
            }
        }
        Ok(())
    }

    async fn fetch_provider(
        &self,
        request: RequestBuilder,
        provider: &'static str,
        source_url: String,
        maximum_bytes: usize,
    ) -> Result<RawProviderResponse, FinanceDataError> {
        let (status, raw) = self
            .send_provider_bounded(request, provider, source_url, maximum_bytes)
            .await?;
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                Err(FinanceDataError::CredentialRejected(provider))
            }
            StatusCode::TOO_MANY_REQUESTS => Err(FinanceDataError::RateLimited(provider)),
            status if !status.is_success() => Err(FinanceDataError::Provider(format!(
                "{provider} returned HTTP {}",
                status.as_u16()
            ))),
            _ => Ok(raw),
        }
    }

    async fn send_provider_bounded(
        &self,
        request: RequestBuilder,
        provider: &'static str,
        source_url: String,
        maximum_bytes: usize,
    ) -> Result<(StatusCode, RawProviderResponse), FinanceDataError> {
        let mut response = request
            .send()
            .await
            .map_err(|_| FinanceDataError::NetworkSafe(provider))?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > maximum_bytes as u64)
        {
            return Err(FinanceDataError::Provider(format!(
                "{provider} response exceeded the bounded size"
            )));
        }
        let continuation = response
            .headers()
            .get("tr_cont")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let mut bytes = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or(64 * 1024)
                .min(maximum_bytes as u64) as usize,
        );
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| FinanceDataError::NetworkSafe(provider))?
        {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > maximum_bytes)
            {
                return Err(FinanceDataError::Provider(format!(
                    "{provider} response exceeded the bounded size"
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((
            status,
            RawProviderResponse {
                bytes,
                source_url,
                retrieved_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                continuation,
            },
        ))
    }
}

fn strict_query<T: DeserializeOwned>(
    params: Value,
    message: &'static str,
) -> Result<T, FinanceDataError> {
    serde_json::from_value(params).map_err(|_| FinanceDataError::InvalidQuery(message))
}

pub(super) fn fixed_url(root: &str, segments: &[&str]) -> Result<Url, FinanceDataError> {
    let mut url = Url::parse(root).map_err(|_| FinanceDataError::InvalidResponse)?;
    url.path_segments_mut()
        .map_err(|_| FinanceDataError::InvalidResponse)?
        .extend(segments);
    Ok(url)
}

pub(super) fn validate_api_key(
    secret: &str,
    _provider: &'static str,
) -> Result<(), FinanceDataError> {
    if secret.len() < 8
        || secret.len() > 512
        || secret
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(FinanceDataError::InvalidQuery(
            "provider API key has an invalid shape",
        ));
    }
    Ok(())
}

fn single_api_key(credentials: &BTreeMap<String, String>) -> Result<&str, FinanceDataError> {
    if credentials.len() != 1 {
        return Err(FinanceDataError::InvalidQuery(
            "provider requires exactly one API key",
        ));
    }
    credentials
        .get("api_key")
        .map(String::as_str)
        .ok_or(FinanceDataError::InvalidQuery(
            "provider requires the api_key credential",
        ))
}

#[cfg(test)]
mod tests;
