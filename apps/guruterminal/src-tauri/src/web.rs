mod official;
mod search_provider;

pub use search_provider::{
    execute_search, SearchAttemptReceipt, SearchBackend, SearchCancel, SearchDroppedCounts,
    SearchHits, SearchProviderId, SearchRequest, WebSearchPolicy,
};

use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::OnceLock,
    time::{Duration, Instant},
};

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use encoding_rs::{Encoding, UTF_8};
use futures_util::StreamExt;
use reqwest::{
    header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, LOCATION, USER_AGENT},
    redirect::Policy,
    Client, Method, StatusCode, Url,
};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::net::lookup_host;
use tokio::sync::Semaphore;

const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp?tools=web_search_exa";
pub(crate) const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const SEARCH_RETRY_DELAY: Duration = Duration::from_millis(750);
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REDIRECTS: usize = 5;
const MAX_SEARCH_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_FETCH_RESPONSE_BYTES: usize = 20 * 1024 * 1024;
const MAX_FETCH_PAGE_BYTES: usize = 256 * 1024;
pub const MAX_FETCH_EXTRACTED_BYTES: usize = crate::document::MAX_EXTRACTED_MARKDOWN_BYTES;
const MAX_ALTERNATES: usize = 8;
const FETCH_RETRY_AFTER_CAP: Duration = Duration::from_secs(3);
const FETCH_ACCEPT: &str = "text/markdown, text/plain;q=0.9, text/html;q=0.8, application/xhtml+xml;q=0.8, text/csv;q=0.7, text/tab-separated-values;q=0.7, application/json;q=0.7, application/ld+json;q=0.7, application/x-ndjson;q=0.7, application/jsonl;q=0.7, application/xml;q=0.7, text/xml;q=0.7, application/yaml;q=0.7, application/x-yaml;q=0.7, application/pdf;q=0.6, application/zip;q=0.6, application/octet-stream;q=0.5, application/vnd.openxmlformats-officedocument.wordprocessingml.document;q=0.6, application/vnd.openxmlformats-officedocument.spreadsheetml.sheet;q=0.6, application/vnd.openxmlformats-officedocument.presentationml.presentation;q=0.6, application/msword;q=0.6, application/vnd.ms-excel;q=0.6, application/vnd.ms-powerpoint;q=0.6";
const FETCH_OFFICIAL_ACCEPT: &str = "application/json, application/ld+json;q=0.9, */*;q=0.1";
const JS_REQUIRED_PHRASES: &[&str] = &[
    "enable javascript",
    "javascript required",
    "turn on javascript",
    "please enable javascript",
    "browser not supported",
    "requires javascript",
];

#[derive(Debug, Error)]
pub enum WebError {
    #[error("web request URL is not allowed")]
    UrlDenied,
    #[error("web request resolved to a non-public address")]
    AddressDenied,
    #[error("web request could not resolve its host")]
    Resolution,
    #[error("web request timed out; try the source again once or choose another source")]
    Timeout,
    #[error("web request transport failed; try the source again once or choose another source")]
    Transport,
    #[error("public web provider is rate-limited; try again shortly")]
    RateLimited { retry_after: Option<Duration> },
    #[error("web search was cancelled")]
    Cancelled,
    #[error("web search provider is unavailable")]
    ProviderUnavailable,
    #[error("{0}")]
    ProviderMessage(String),
    #[error("web provider returned HTTP status {0}")]
    ProviderStatus(u16),
    #[error("web provider request failed")]
    Provider,
    #[error("web provider returned an invalid response")]
    InvalidProviderResponse,
    #[error("web response exceeded its size limit")]
    ResponseTooLarge,
    #[error("web response content type is not supported")]
    UnsupportedContent,
    #[error("web response bytes do not match the declared content type")]
    TypeMismatch,
    #[error("web document has no extractable text layer")]
    NoTextLayer,
    #[error("web document content offset is outside the extracted text")]
    InvalidContentOffset,
    #[error("web request exceeded its redirect limit")]
    RedirectLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebRecency {
    Day,
    Week,
    Month,
    Year,
}

impl WebRecency {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "year" => Some(Self::Year),
            _ => None,
        }
    }

    fn start_published_date(self) -> NaiveDate {
        let days = match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
            Self::Year => 365,
        };
        Utc::now().date_naive() - chrono::Duration::days(days)
    }
}

#[derive(Clone, Debug)]
pub struct WebSearchQuery {
    pub query: String,
    pub limit: u8,
    pub recency: Option<WebRecency>,
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSource {
    pub source_id: String,
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WebSearchOutput {
    pub selected_provider: String,
    pub retrieved_at: String,
    pub untrusted_content: bool,
    pub results: Vec<WebSourceOutput>,
    pub attempts: Vec<SearchAttemptReceipt>,
    pub received_count: usize,
    pub returned_count: usize,
    pub dropped: SearchDroppedCounts,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WebSourceOutput {
    pub source_id: String,
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub published_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WebAlternate {
    pub url: String,
    pub kind: &'static str,
}

#[derive(Debug, Serialize)]
pub struct WebFetchOutput {
    pub source_id: String,
    pub content_sha256: String,
    pub title: String,
    pub url: String,
    pub requested_url: String,
    pub representation_url: String,
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub charset: Option<String>,
    pub document_kind: String,
    pub extraction_method: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub representation_provider: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_id: Option<String>,
    pub page_count: Option<u32>,
    pub content: String,
    pub content_offset: usize,
    pub content_bytes: usize,
    pub extracted_bytes: usize,
    pub next_offset: Option<usize>,
    pub extraction_truncated: bool,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub quality_warnings: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alternates: Vec<WebAlternate>,
    pub untrusted_content: bool,
    pub content_class: &'static str,
    pub retrieved_at: String,
    pub published_at: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct WebFetchSnapshot {
    source_id: String,
    content_sha256: String,
    title: String,
    url: String,
    requested_url: String,
    representation_url: String,
    final_url: String,
    status: u16,
    content_type: String,
    charset: Option<String>,
    document_kind: String,
    extraction_method: &'static str,
    representation_provider: Option<&'static str>,
    record_id: Option<String>,
    page_count: Option<u32>,
    content: String,
    extraction_truncated: bool,
    quality_warnings: Vec<&'static str>,
    alternates: Vec<WebAlternate>,
    retrieved_at: String,
    published_at: Option<String>,
}

impl WebFetchSnapshot {
    pub(crate) fn page(&self, content_offset: usize) -> Result<WebFetchOutput, WebError> {
        let extracted_bytes = self.content.len();
        let (content, next_offset) = content_page(&self.content, content_offset)?;
        let content_bytes = content.len();
        Ok(WebFetchOutput {
            source_id: self.source_id.clone(),
            content_sha256: self.content_sha256.clone(),
            title: self.title.clone(),
            url: self.url.clone(),
            requested_url: self.requested_url.clone(),
            representation_url: self.representation_url.clone(),
            final_url: self.final_url.clone(),
            status: self.status,
            content_type: self.content_type.clone(),
            charset: self.charset.clone(),
            document_kind: self.document_kind.clone(),
            extraction_method: self.extraction_method,
            representation_provider: self.representation_provider,
            record_id: self.record_id.clone(),
            page_count: self.page_count,
            content,
            content_offset,
            content_bytes,
            extracted_bytes,
            next_offset,
            extraction_truncated: self.extraction_truncated,
            truncated: self.extraction_truncated || next_offset.is_some(),
            quality_warnings: self.quality_warnings.clone(),
            alternates: self.alternates.clone(),
            untrusted_content: true,
            content_class: "untrusted_web",
            retrieved_at: self.retrieved_at.clone(),
            published_at: self.published_at.clone(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct JsonSearchResponse {
    results: Vec<JsonSearchResult>,
}

#[derive(Debug, Deserialize)]
struct JsonSearchResult {
    title: Option<String>,
    url: Option<String>,
    text: Option<String>,
    highlights: Option<Vec<String>>,
    #[serde(rename = "publishedDate")]
    published_date: Option<String>,
}

pub async fn search(query: WebSearchQuery) -> Result<(WebSearchOutput, Vec<WebSource>), WebError> {
    execute_search(
        SearchRequest {
            query,
            policy: WebSearchPolicy::ExaOnly,
            chat_provider: None,
            cancel: SearchCancel::never(),
        },
        &KeylessSearchBackend,
    )
    .await
}

pub(crate) struct KeylessSearchBackend;

impl SearchBackend for KeylessSearchBackend {
    async fn attempt(
        &self,
        provider: SearchProviderId,
        query: &WebSearchQuery,
        _remaining: Duration,
        cancel: &SearchCancel,
    ) -> Result<SearchHits, WebError> {
        if cancel.is_cancelled() {
            return Err(WebError::Cancelled);
        }
        match provider {
            SearchProviderId::ExaPublic => search_exa_once(query).await,
            _ => Err(WebError::ProviderUnavailable),
        }
    }
}

pub(crate) async fn search_exa_once(query: &WebSearchQuery) -> Result<SearchHits, WebError> {
    let _permit = web_search_semaphore()
        .acquire()
        .await
        .map_err(|_| WebError::Provider)?;
    let endpoint = Url::parse(EXA_MCP_URL).map_err(|_| WebError::UrlDenied)?;
    // Exa's anonymous `web_search_exa` schema currently accepts only query and
    // numResults. Express optional filters in the discovery query, then enforce
    // every check that can be proven from returned URL/date metadata locally.
    let arguments = json!({
        "query": adapted_search_query(query),
        "numResults": provider_result_limit(query)
    });
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "web_search_exa",
            "arguments": arguments
        }
    });
    let response = request_bounded(
        Method::POST,
        endpoint,
        Some(body),
        SEARCH_TIMEOUT,
        MAX_SEARCH_RESPONSE_BYTES,
        "application/json, text/event-stream",
    )
    .await?;
    if !response.status.is_success() {
        return Err(status_error_with_retry(
            response.status,
            response.retry_after,
        ));
    }
    let text = String::from_utf8(response.bytes).map_err(|_| WebError::InvalidProviderResponse)?;
    let provider_text = mcp_text(&text)?;
    let mut sources = parse_search_results(&provider_text)?;
    let mut seen = HashSet::new();
    sources.retain(|source| seen.insert(source.url.clone()));
    Ok(SearchHits {
        sources,
        model: None,
        search_request_count: None,
        retry_after: None,
    })
}

fn status_error_with_retry(status: StatusCode, retry_after: Option<Duration>) -> WebError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        WebError::RateLimited { retry_after }
    } else {
        status_error(status)
    }
}

fn web_search_semaphore() -> &'static Semaphore {
    static SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();
    SEMAPHORE.get_or_init(|| Semaphore::new(1))
}

fn retryable_search_error(error: &WebError) -> bool {
    match error {
        WebError::Resolution
        | WebError::Timeout
        | WebError::Transport
        | WebError::RateLimited { .. } => true,
        WebError::ProviderStatus(status) => {
            matches!(*status, 408 | 425) || (500..=599).contains(status)
        }
        _ => false,
    }
}

pub(crate) fn fallbackable_search_error(error: &WebError) -> bool {
    retryable_search_error(error)
        || matches!(
            error,
            WebError::Provider
                | WebError::ProviderMessage(_)
                | WebError::ProviderStatus(_)
                | WebError::ProviderUnavailable
                | WebError::InvalidProviderResponse
        )
}

fn status_error(status: StatusCode) -> WebError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        WebError::RateLimited { retry_after: None }
    } else {
        WebError::ProviderStatus(status.as_u16())
    }
}

pub(crate) fn retry_after_from_error(error: &WebError) -> Option<Duration> {
    match error {
        WebError::RateLimited { retry_after } => *retry_after,
        _ => None,
    }
}

fn classify_transport_error(error: reqwest::Error) -> WebError {
    if error.is_timeout() {
        WebError::Timeout
    } else {
        WebError::Transport
    }
}

fn provider_result_limit(query: &WebSearchQuery) -> u8 {
    if query.recency.is_some()
        || !query.include_domains.is_empty()
        || !query.exclude_domains.is_empty()
    {
        10
    } else {
        query.limit
    }
}

fn adapted_search_query(query: &WebSearchQuery) -> String {
    let mut adapted = query.query.trim().to_owned();
    if let Some(recency) = query.recency {
        adapted.push_str(". Prefer sources published on or after ");
        adapted.push_str(&recency.start_published_date().to_string());
    }
    if !query.include_domains.is_empty() {
        adapted.push_str(". Include only sources from: ");
        adapted.push_str(&query.include_domains.join(", "));
    }
    if !query.exclude_domains.is_empty() {
        adapted.push_str(". Exclude sources from: ");
        adapted.push_str(&query.exclude_domains.join(", "));
    }
    adapted
}

pub(crate) fn apply_search_filters_counted(
    query: &WebSearchQuery,
    sources: &mut Vec<WebSource>,
) -> SearchDroppedCounts {
    let mut dropped = SearchDroppedCounts::default();
    sources.retain(|source| {
        let Ok(url) = Url::parse(&source.url) else {
            dropped.invalid_url += 1;
            return false;
        };
        if validate_source_url(url.as_str()).is_err() {
            dropped.invalid_url += 1;
            return false;
        }
        let Some(host) = url.host_str() else {
            dropped.invalid_url += 1;
            return false;
        };
        let included = query.include_domains.is_empty()
            || query
                .include_domains
                .iter()
                .any(|domain| host_matches_domain(host, domain));
        if !included {
            dropped.include_domain += 1;
            return false;
        }
        let excluded = query
            .exclude_domains
            .iter()
            .any(|domain| host_matches_domain(host, domain));
        if excluded {
            dropped.exclude_domain += 1;
            return false;
        }
        let recent = query.recency.is_none_or(|recency| {
            source
                .published_at
                .as_deref()
                .and_then(parse_published_date)
                .is_none_or(|published| published >= recency.start_published_date())
        });
        if !recent {
            dropped.recency += 1;
            return false;
        }
        true
    });
    dropped
}

fn host_matches_domain(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn parse_published_date(value: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.date_naive())
        .or_else(|| {
            value
                .get(..10)
                .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        })
}

pub(crate) async fn fetch(source: &WebSource) -> Result<WebFetchSnapshot, WebError> {
    let requested = validate_source_url(&source.url)?;
    if let Some(target) = official::match_official(&requested) {
        match fetch_official(source, &target).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(error) if official_allows_generic_fallback(&error) => {
                let mut snapshot = fetch_generic(source, &requested).await?;
                push_warning(&mut snapshot.quality_warnings, "official_fallback");
                return Ok(snapshot);
            }
            Err(error) => return Err(error),
        }
    }
    fetch_generic(source, &requested).await
}

fn official_allows_generic_fallback(error: &WebError) -> bool {
    matches!(
        error,
        WebError::Resolution
            | WebError::Timeout
            | WebError::Transport
            | WebError::RateLimited { .. }
            | WebError::ProviderStatus(_)
            | WebError::Provider
            | WebError::ProviderUnavailable
            | WebError::InvalidProviderResponse
            | WebError::UnsupportedContent
            | WebError::TypeMismatch
            | WebError::NoTextLayer
    )
}

async fn fetch_official(
    source: &WebSource,
    target: &official::OfficialTarget,
) -> Result<WebFetchSnapshot, WebError> {
    let response =
        get_with_429_retry(target.representation_url.clone(), FETCH_OFFICIAL_ACCEPT).await?;
    if !response.status.is_success() {
        return Err(status_error(response.status));
    }
    let record_id = official::verified_record_id(target, &response.bytes)
        .ok_or(WebError::InvalidProviderResponse)?;
    let markdown = official::project_markdown(target, &response.bytes)
        .ok_or(WebError::InvalidProviderResponse)?;
    if markdown.trim().is_empty() {
        return Err(WebError::NoTextLayer);
    }
    let (content, extraction_truncated) = truncate_utf8(markdown.trim(), MAX_FETCH_EXTRACTED_BYTES);
    let content_sha256 = hex::encode(Sha256::digest(&response.bytes));
    Ok(WebFetchSnapshot {
        source_id: source.source_id.clone(),
        content_sha256,
        title: markdown_heading(&content).unwrap_or_else(|| source.title.clone()),
        url: source.url.clone(),
        requested_url: source.url.clone(),
        representation_url: target.representation_url.to_string(),
        final_url: response.final_url.to_string(),
        status: response.status.as_u16(),
        content_type: media_type(&response.content_type),
        charset: Some("utf-8".into()),
        document_kind: "json".into(),
        extraction_method: "official_api",
        representation_provider: Some(target.provider.as_str()),
        record_id: Some(record_id.clone()),
        page_count: None,
        content: content.clone(),
        extraction_truncated,
        quality_warnings: quality_warnings(&content, false),
        alternates: Vec::new(),
        retrieved_at: now_rfc3339(),
        published_at: source.published_at.clone(),
    })
}

async fn fetch_generic(source: &WebSource, requested: &Url) -> Result<WebFetchSnapshot, WebError> {
    let response = get_with_429_retry(requested.clone(), FETCH_ACCEPT).await?;
    if !response.status.is_success() {
        return Err(status_error(response.status));
    }
    let content_type = media_type(&response.content_type);
    let content_sha256 = hex::encode(Sha256::digest(&response.bytes));
    let bytes = response.bytes;
    let raw_content_type = response.content_type.clone();
    let decode_content_type = content_type.clone();
    let fallback_title = source.title.clone();
    let base_url = response.final_url.clone();
    let decoded = tokio::time::timeout(
        EXTRACT_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            decode_fetched_content(
                &bytes,
                &raw_content_type,
                &decode_content_type,
                &fallback_title,
                &base_url,
            )
        }),
    )
    .await
    .map_err(|_| WebError::Timeout)?
    .map_err(|_| WebError::UnsupportedContent)??;
    Ok(WebFetchSnapshot {
        source_id: source.source_id.clone(),
        content_sha256,
        title: decoded.title,
        url: source.url.clone(),
        requested_url: source.url.clone(),
        representation_url: response.final_url.to_string(),
        final_url: response.final_url.to_string(),
        status: response.status.as_u16(),
        content_type,
        charset: decoded.charset,
        document_kind: decoded.document_kind,
        extraction_method: decoded.extraction_method,
        representation_provider: None,
        record_id: None,
        page_count: decoded.page_count,
        content: decoded.content.clone(),
        extraction_truncated: decoded.extraction_truncated,
        quality_warnings: quality_warnings(
            &decoded.content,
            decoded.extraction_method == "readability",
        ),
        alternates: decoded.alternates,
        retrieved_at: now_rfc3339(),
        published_at: source.published_at.clone(),
    })
}

async fn get_with_429_retry(url: Url, accept: &str) -> Result<BoundedResponse, WebError> {
    let started = Instant::now();
    let response = request_bounded(
        Method::GET,
        url.clone(),
        None,
        FETCH_TIMEOUT,
        MAX_FETCH_RESPONSE_BYTES,
        accept,
    )
    .await?;
    if response.status != StatusCode::TOO_MANY_REQUESTS {
        return Ok(response);
    }
    let remaining = FETCH_TIMEOUT.saturating_sub(started.elapsed());
    let Some(delay) = retry_after_delay(response.retry_after, remaining) else {
        return Err(WebError::RateLimited {
            retry_after: response.retry_after,
        });
    };
    tokio::time::sleep(delay).await;
    let remaining = FETCH_TIMEOUT.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(WebError::Timeout);
    }
    request_bounded(
        Method::GET,
        url,
        None,
        remaining,
        MAX_FETCH_RESPONSE_BYTES,
        accept,
    )
    .await
}

fn retry_after_delay(retry_after: Option<Duration>, remaining: Duration) -> Option<Duration> {
    if remaining.is_zero() {
        return None;
    }
    let parsed = retry_after.unwrap_or(Duration::from_secs(1));
    if parsed > remaining && remaining < FETCH_RETRY_AFTER_CAP {
        return None;
    }
    Some(parsed.min(FETCH_RETRY_AFTER_CAP).min(remaining))
}

#[cfg(test)]
fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(
        date.duration_since(std::time::SystemTime::now())
            .unwrap_or(Duration::ZERO),
    )
}

fn media_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn push_warning(warnings: &mut Vec<&'static str>, warning: &'static str) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

fn markdown_heading(markdown: &str) -> Option<String> {
    markdown.lines().find_map(|line| {
        line.strip_prefix("# ")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

struct BoundedResponse {
    status: StatusCode,
    content_type: String,
    final_url: Url,
    bytes: Vec<u8>,
    retry_after: Option<Duration>,
}

async fn request_bounded(
    method: Method,
    mut url: Url,
    json_body: Option<Value>,
    timeout: Duration,
    max_bytes: usize,
    accept: &str,
) -> Result<BoundedResponse, WebError> {
    for redirect_count in 0..=MAX_REDIRECTS {
        #[cfg(test)]
        if let Some(scripted) = script::take(&url, accept) {
            if (300..400).contains(&scripted.status) && scripted.status != 304 {
                if method != Method::GET {
                    return Err(WebError::Provider);
                }
                if redirect_count == MAX_REDIRECTS {
                    return Err(WebError::RedirectLimit);
                }
                let location = scripted.location.ok_or(WebError::Provider)?;
                url = url.join(&location).map_err(|_| WebError::UrlDenied)?;
                validate_url_shape(&url)?;
                continue;
            }
            return Ok(BoundedResponse {
                status: StatusCode::from_u16(scripted.status).map_err(|_| WebError::Provider)?,
                content_type: scripted.content_type,
                final_url: url,
                bytes: scripted.body,
                retry_after: parse_retry_after(scripted.retry_after.as_deref()),
            });
        }
        let (host, addresses) = resolve_public(&url).await?;
        let client = Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
            .resolve_to_addrs(&host, &addresses)
            .build()
            .map_err(|_| WebError::Provider)?;
        let mut request = client
            .request(method.clone(), url.clone())
            .header(
                USER_AGENT,
                "GuruTerminal-Web/1.0 (+pi-web-access-compatible)",
            )
            .header(ACCEPT, accept);
        if let Some(body) = &json_body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(classify_transport_error)?;
        if response.status().is_redirection() {
            if method != Method::GET {
                return Err(WebError::Provider);
            }
            if redirect_count == MAX_REDIRECTS {
                return Err(WebError::RedirectLimit);
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(WebError::Provider)?;
            url = url.join(location).map_err(|_| WebError::UrlDenied)?;
            validate_url_shape(&url)?;
            continue;
        }
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > max_bytes)
        {
            return Err(WebError::ResponseTooLarge);
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let status = response.status();
        let retry_after =
            parse_retry_after_header(response.headers().get(reqwest::header::RETRY_AFTER));
        let final_url = response.url().clone();
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(classify_transport_error)?;
            if bytes.len().saturating_add(chunk.len()) > max_bytes {
                return Err(WebError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(BoundedResponse {
            status,
            content_type,
            final_url,
            bytes,
            retry_after,
        });
    }
    Err(WebError::RedirectLimit)
}

async fn resolve_public(url: &Url) -> Result<(String, Vec<SocketAddr>), WebError> {
    validate_url_shape(url)?;
    let host = url.host_str().ok_or(WebError::UrlDenied)?.to_owned();
    let port = url.port_or_known_default().ok_or(WebError::UrlDenied)?;
    let addresses = lookup_host((host.as_str(), port))
        .await
        .map_err(|_| WebError::Resolution)?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(WebError::Resolution);
    }
    if addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(WebError::AddressDenied);
    }
    Ok((host, addresses))
}

pub fn validate_source_url(value: &str) -> Result<Url, WebError> {
    let url = Url::parse(value).map_err(|_| WebError::UrlDenied)?;
    validate_url_shape(&url)?;
    Ok(url)
}

fn validate_url_shape(url: &Url) -> Result<(), WebError> {
    let port_is_allowed = matches!(
        (url.scheme(), url.port()),
        ("http", None | Some(80)) | ("https", None | Some(443))
    );
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || !port_is_allowed
    {
        return Err(WebError::UrlDenied);
    }
    Ok(())
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    let segments = ip.segments();
    // Accept only global-unicast space, then remove documentation and IPv4
    // transition prefixes whose embedded address can bypass IPv4 policy.
    (segments[0] & 0xe000) == 0x2000
        && segments[0] != 0x2002
        && !(segments[0] == 0x2001 && matches!(segments[1], 0x0000 | 0x0db8))
}

fn mcp_text(body: &str) -> Result<String, WebError> {
    let candidates = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .chain(std::iter::once(body.trim()));
    for candidate in candidates {
        let Ok(value) = serde_json::from_str::<Value>(candidate) else {
            continue;
        };
        if value.get("error").is_some() || value["result"]["isError"] == Value::Bool(true) {
            return Err(WebError::Provider);
        }
        if let Some(text) = value["result"]["content"].as_array().and_then(|items| {
            items.iter().find_map(|item| {
                (item["type"].as_str() == Some("text"))
                    .then(|| item["text"].as_str())
                    .flatten()
                    .filter(|text| !text.trim().is_empty())
            })
        }) {
            return Ok(text.to_owned());
        }
    }
    Err(WebError::InvalidProviderResponse)
}

fn parse_search_results(text: &str) -> Result<Vec<WebSource>, WebError> {
    if let Ok(parsed) = serde_json::from_str::<JsonSearchResponse>(text) {
        let sources = parsed
            .results
            .into_iter()
            .filter_map(|result| {
                let url = result.url?;
                validate_source_url(&url).ok()?;
                let title = result
                    .title
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("Untitled source");
                let title = bounded_text(title, 512);
                let snippet = result
                    .highlights
                    .filter(|items| !items.is_empty())
                    .map(|items| items.join(" "))
                    .or(result.text)
                    .unwrap_or_default();
                let published_at = result
                    .published_date
                    .as_deref()
                    .filter(|value| !value.trim().is_empty() && *value != "N/A")
                    .map(|value| bounded_text(value, 128));
                Some(web_source(
                    title,
                    url,
                    bounded_text(&snippet, 2_000),
                    published_at,
                ))
            })
            .collect::<Vec<_>>();
        return Ok(sources);
    }

    let mut sources = Vec::new();
    for block in text.split("\nTitle: ") {
        let block = block.strip_prefix("Title: ").unwrap_or(block);
        let mut lines = block.lines();
        let title = lines.next().unwrap_or("Untitled source").trim();
        let url = block
            .lines()
            .find_map(|line| line.strip_prefix("URL: "))
            .map(str::trim);
        let Some(url) = url else { continue };
        if validate_source_url(url).is_err() {
            continue;
        }
        let snippet = block
            .split_once("\nText: ")
            .map(|(_, value)| value)
            .or_else(|| block.split_once("\nHighlights:\n").map(|(_, value)| value))
            .unwrap_or("")
            .trim_end_matches("\n---")
            .trim();
        let published_at = block
            .lines()
            .find_map(|line| line.strip_prefix("Published: "))
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "N/A")
            .map(|value| bounded_text(value, 128));
        sources.push(web_source(
            bounded_text(title, 512),
            url.to_owned(),
            bounded_text(snippet, 2_000),
            published_at,
        ));
    }
    let normalized = text.trim().to_ascii_lowercase();
    if sources.is_empty()
        && (normalized.contains("no results") || normalized.contains("no relevant results"))
    {
        Ok(Vec::new())
    } else if sources.is_empty() {
        Err(WebError::InvalidProviderResponse)
    } else {
        Ok(sources)
    }
}

pub fn source_from_url(value: &str) -> Result<WebSource, WebError> {
    let url = validate_source_url(value)?;
    let title = url
        .host_str()
        .filter(|value| !value.is_empty())
        .unwrap_or("Untitled source")
        .to_owned();
    Ok(web_source(title, url.to_string(), String::new(), None))
}

pub fn validate_search_domain(value: &str) -> Result<String, WebError> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 253
        || value.contains('/')
        || value.contains(':')
        || value.contains('@')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        || value.starts_with('.')
        || value.ends_with('.')
        || !value.contains('.')
    {
        return Err(WebError::UrlDenied);
    }
    Ok(value)
}

struct DecodedContent {
    title: String,
    content: String,
    charset: Option<String>,
    document_kind: String,
    extraction_method: &'static str,
    page_count: Option<u32>,
    extraction_truncated: bool,
    alternates: Vec<WebAlternate>,
}

fn decode_fetched_content(
    bytes: &[u8],
    raw_content_type: &str,
    content_type: &str,
    fallback_title: &str,
    base_url: &Url,
) -> Result<DecodedContent, WebError> {
    if crate::document::looks_like_pdf(bytes)
        || crate::document::looks_like_zip(bytes)
        || crate::document::looks_like_ole(bytes)
        || is_document_content_type(content_type)
    {
        let extracted = crate::document::extract(bytes, content_type, MAX_FETCH_EXTRACTED_BYTES)
            .map_err(document_error)?;
        let extraction_method = if extracted.kind == crate::document::DocumentKind::Pdf {
            "pdf"
        } else if extracted.kind == crate::document::DocumentKind::Html {
            "readability"
        } else if extracted.kind == crate::document::DocumentKind::PlainText {
            if content_type == "text/markdown" {
                "direct_markdown"
            } else {
                "direct_text"
            }
        } else {
            "office"
        };
        return Ok(DecodedContent {
            title: fallback_title.to_owned(),
            content: extracted.markdown.trim().to_owned(),
            charset: None,
            document_kind: extracted.kind.as_str().to_owned(),
            extraction_method,
            page_count: extracted.page_count,
            extraction_truncated: extracted.truncated,
            alternates: Vec::new(),
        });
    }
    if !is_text_content_type(content_type)
        && !matches!(content_type, "" | "application/octet-stream")
    {
        return Err(WebError::UnsupportedContent);
    }
    let (text, charset) = decode_web_text(bytes, raw_content_type)?;
    if !is_probably_text(&text) {
        return Err(WebError::UnsupportedContent);
    }
    let declared_html = matches!(content_type, "text/html" | "application/xhtml+xml");
    let sniff_html = matches!(content_type, "" | "application/octet-stream")
        && crate::document::looks_like_html(bytes);
    let html = declared_html || sniff_html;
    let (title, content, extraction_method, alternates) = if html {
        let (title, content) = crate::document::extract_html(&text, fallback_title);
        (
            title,
            content,
            "readability",
            collect_alternates(&text, base_url),
        )
    } else {
        let extraction_method = if content_type == "text/markdown" {
            "direct_markdown"
        } else {
            "direct_text"
        };
        (
            fallback_title.to_owned(),
            text,
            extraction_method,
            Vec::new(),
        )
    };
    if content.trim().is_empty() {
        return Err(WebError::NoTextLayer);
    }
    let (content, extraction_truncated) = truncate_utf8(content.trim(), MAX_FETCH_EXTRACTED_BYTES);
    Ok(DecodedContent {
        title,
        content,
        charset: Some(charset),
        document_kind: if html {
            "html".to_owned()
        } else {
            text_document_kind(content_type).to_owned()
        },
        extraction_method,
        page_count: None,
        extraction_truncated,
        alternates,
    })
}

fn quality_warnings(content: &str, html_extract: bool) -> Vec<&'static str> {
    let mut warnings = Vec::new();
    let trimmed = content.trim();
    let non_whitespace = trimmed
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    if non_whitespace < 100 {
        warnings.push("very_short");
    }
    let lower = trimmed.to_ascii_lowercase();
    if JS_REQUIRED_PHRASES
        .iter()
        .any(|phrase| lower.contains(phrase))
    {
        warnings.push("javascript_required");
    }
    if html_extract && is_navigation_heavy(trimmed) {
        warnings.push("navigation_heavy");
    }
    warnings
}

fn is_navigation_heavy(content: &str) -> bool {
    let lines = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.len() > 10 {
        let short_lines = lines
            .iter()
            .filter(|line| line.trim().chars().count() < 40)
            .count();
        if (short_lines as f64) / (lines.len() as f64) > 0.7 {
            return true;
        }
    }
    let link_count = content.matches("](").count();
    let words = content.split_whitespace().count().max(1);
    link_count.saturating_mul(4) > words
}

fn collect_alternates(html: &str, base_url: &Url) -> Vec<WebAlternate> {
    let document = Html::parse_document(html);
    let mut seen = HashSet::new();
    let mut markdown = Vec::new();
    let mut documents = Vec::new();
    if let Ok(selector) = Selector::parse("link[rel][href]") {
        for element in document.select(&selector) {
            let rel = element.value().attr("rel").unwrap_or("");
            if !rel
                .split_whitespace()
                .any(|token| token.eq_ignore_ascii_case("alternate"))
            {
                continue;
            }
            let Some(href) = element.value().attr("href") else {
                continue;
            };
            let type_attr = element
                .value()
                .attr("type")
                .unwrap_or("")
                .to_ascii_lowercase();
            let kind =
                if type_attr.contains("markdown") || href.to_ascii_lowercase().ends_with(".md") {
                    Some("markdown")
                } else {
                    alternate_document_kind(href, &type_attr)
                };
            if let Some(kind) = kind {
                if let Some(alternate) = resolve_alternate(base_url, href, kind, &mut seen) {
                    if kind == "markdown" {
                        markdown.push(alternate);
                    } else {
                        documents.push(alternate);
                    }
                }
            }
        }
    }
    if let Ok(selector) = Selector::parse("a[href]") {
        for element in document.select(&selector) {
            let Some(href) = element.value().attr("href") else {
                continue;
            };
            let type_attr = element
                .value()
                .attr("type")
                .unwrap_or("")
                .to_ascii_lowercase();
            if let Some(kind) = alternate_document_kind(href, &type_attr) {
                if let Some(alternate) = resolve_alternate(base_url, href, kind, &mut seen) {
                    documents.push(alternate);
                }
            }
        }
    }
    markdown
        .into_iter()
        .chain(documents)
        .take(MAX_ALTERNATES)
        .collect()
}

fn alternate_document_kind(href: &str, type_attr: &str) -> Option<&'static str> {
    let path = href
        .split(['?', '#'])
        .next()
        .unwrap_or(href)
        .to_ascii_lowercase();
    if type_attr.contains("markdown") || path.ends_with(".md") {
        Some("markdown")
    } else if type_attr.contains("pdf") || path.ends_with(".pdf") {
        Some("pdf")
    } else if type_attr.contains("word")
        || type_attr.contains("excel")
        || type_attr.contains("powerpoint")
        || type_attr.contains("officedocument")
        || path.ends_with(".doc")
        || path.ends_with(".docx")
        || path.ends_with(".xls")
        || path.ends_with(".xlsx")
        || path.ends_with(".ppt")
        || path.ends_with(".pptx")
    {
        Some("office")
    } else {
        None
    }
}

fn resolve_alternate(
    base_url: &Url,
    href: &str,
    kind: &'static str,
    seen: &mut HashSet<String>,
) -> Option<WebAlternate> {
    let resolved = base_url.join(href).ok()?;
    validate_url_shape(&resolved).ok()?;
    let url = resolved.to_string();
    if url == base_url.as_str() || !seen.insert(url.clone()) {
        return None;
    }
    Some(WebAlternate { url, kind })
}

fn document_error(error: crate::document::DocumentError) -> WebError {
    match error {
        crate::document::DocumentError::NoTextLayer => WebError::NoTextLayer,
        crate::document::DocumentError::ArchiveLimit => WebError::ResponseTooLarge,
        crate::document::DocumentError::TypeMismatch => WebError::TypeMismatch,
        _ => WebError::UnsupportedContent,
    }
}

fn decode_web_text(bytes: &[u8], content_type: &str) -> Result<(String, String), WebError> {
    let declared = content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim().eq_ignore_ascii_case("charset").then(|| {
            value
                .trim()
                .trim_matches(|character| matches!(character, '\'' | '"'))
        })
    });
    let declared_encoding = match declared {
        Some(label) => {
            Some(Encoding::for_label(label.as_bytes()).ok_or(WebError::UnsupportedContent)?)
        }
        None => None,
    };
    let (encoding, bom_length) = Encoding::for_bom(bytes)
        .or_else(|| declared_encoding.map(|encoding| (encoding, 0)))
        .unwrap_or((UTF_8, 0));
    let text = encoding
        .decode_without_bom_handling_and_without_replacement(&bytes[bom_length..])
        .ok_or(WebError::UnsupportedContent)?;
    Ok((text.into_owned(), encoding.name().to_ascii_lowercase()))
}

fn is_probably_text(value: &str) -> bool {
    !value.contains('\0')
        && !value
            .chars()
            .any(|character| character.is_control() && !character.is_whitespace())
}

fn is_text_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || matches!(
            content_type,
            "application/json"
                | "application/ld+json"
                | "application/x-ndjson"
                | "application/jsonl"
                | "application/xml"
                | "application/csv"
                | "application/yaml"
                | "application/x-yaml"
        )
        || content_type.ends_with("+json")
        || content_type.ends_with("+xml")
}

fn text_document_kind(content_type: &str) -> &'static str {
    if matches!(content_type, "text/csv" | "application/csv") {
        "csv"
    } else if content_type == "text/tab-separated-values" {
        "tsv"
    } else if content_type == "text/markdown" {
        "markdown"
    } else if content_type == "application/json"
        || matches!(
            content_type,
            "application/ld+json" | "application/x-ndjson" | "application/jsonl"
        )
        || content_type.ends_with("+json")
    {
        "json"
    } else if matches!(content_type, "application/xml" | "text/xml")
        || content_type.ends_with("+xml")
    {
        "xml"
    } else if matches!(
        content_type,
        "application/yaml" | "application/x-yaml" | "text/yaml"
    ) {
        "yaml"
    } else {
        "text"
    }
}

fn content_page(content: &str, offset: usize) -> Result<(String, Option<usize>), WebError> {
    if offset >= content.len() || !content.is_char_boundary(offset) {
        return Err(WebError::InvalidContentOffset);
    }
    let mut end = offset
        .saturating_add(MAX_FETCH_PAGE_BYTES)
        .min(content.len());
    while end > offset && !content.is_char_boundary(end) {
        end -= 1;
    }
    let next_offset = (end < content.len()).then_some(end);
    Ok((content[offset..end].to_owned(), next_offset))
}

fn is_document_content_type(content_type: &str) -> bool {
    matches!(
        content_type,
        "application/pdf"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "application/msword"
            | "application/vnd.ms-excel"
            | "application/vnd.ms-powerpoint"
    )
}

pub(crate) fn parse_retry_after_header(
    value: Option<&reqwest::header::HeaderValue>,
) -> Option<Duration> {
    let raw = value?.to_str().ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(raw).ok().map(|date| {
        date.duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
    })
}

pub(crate) fn clamp_retry_after(requested: Duration, remaining: Duration) -> Option<Duration> {
    const MAX_WAIT: Duration = Duration::from_secs(3);
    let budget = MAX_WAIT.min(remaining);
    (requested <= budget).then_some(requested.min(remaining))
}

pub(crate) fn web_source(
    title: String,
    url: String,
    snippet: String,
    published_at: Option<String>,
) -> WebSource {
    let digest = Sha256::digest(url.as_bytes());
    WebSource {
        source_id: format!("web:{}", hex::encode(&digest[..12])),
        title,
        url,
        snippet,
        published_at,
    }
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    truncate_utf8(value.trim(), max_bytes).0
}

fn truncate_utf8(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_owned(), false);
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), true)
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod script {
    use super::*;
    use std::sync::Mutex;

    #[derive(Clone)]
    pub struct ScriptedResponse {
        pub status: u16,
        pub content_type: String,
        pub body: Vec<u8>,
        pub retry_after: Option<String>,
        pub location: Option<String>,
    }

    struct Route {
        needle: String,
        responses: Vec<ScriptedResponse>,
        next: usize,
    }

    struct ScriptState {
        routes: Vec<Route>,
        requests: Vec<(String, String)>,
    }

    static STATE: Mutex<Option<ScriptState>> = Mutex::new(None);
    static RUN: Mutex<()> = Mutex::new(());

    pub struct Guard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

    impl Drop for Guard {
        fn drop(&mut self) {
            *STATE.lock().unwrap_or_else(|error| error.into_inner()) = None;
        }
    }

    pub fn install(routes: Vec<(&str, Vec<ScriptedResponse>)>) -> Guard {
        let run = RUN.lock().unwrap_or_else(|error| error.into_inner());
        *STATE.lock().unwrap_or_else(|error| error.into_inner()) = Some(ScriptState {
            routes: routes
                .into_iter()
                .map(|(needle, responses)| Route {
                    needle: needle.to_owned(),
                    responses,
                    next: 0,
                })
                .collect(),
            requests: Vec::new(),
        });
        Guard(run)
    }

    pub fn take(url: &Url, accept: &str) -> Option<ScriptedResponse> {
        let mut state = STATE.lock().ok()?;
        let state = state.as_mut()?;
        state
            .requests
            .push((url.as_str().to_owned(), accept.to_owned()));
        let url_s = url.as_str().to_owned();
        for route in &mut state.routes {
            if url_s.contains(&route.needle) {
                if route.next >= route.responses.len() {
                    return Some(ScriptedResponse {
                        status: 500,
                        content_type: "text/plain".into(),
                        body: b"script exhausted".to_vec(),
                        retry_after: None,
                        location: None,
                    });
                }
                let response = route.responses[route.next].clone();
                route.next += 1;
                return Some(response);
            }
        }
        None
    }

    pub fn requests() -> Vec<(String, String)> {
        STATE
            .lock()
            .ok()
            .and_then(|state| state.as_ref().map(|state| state.requests.clone()))
            .unwrap_or_default()
    }

    pub fn ok(content_type: &str, body: impl Into<Vec<u8>>) -> ScriptedResponse {
        ScriptedResponse {
            status: 200,
            content_type: content_type.to_owned(),
            body: body.into(),
            retry_after: None,
            location: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::join_all;

    #[test]
    fn rejects_private_and_special_addresses() {
        for value in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.1.1",
            "192.168.1.1",
            "100.64.0.1",
            "192.0.2.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2002:c0a8:0101::1",
            "2001:0:c0a8:0101::1",
            "::ffff:192.168.1.1",
            "64:ff9b::192.168.1.1",
        ] {
            assert!(!is_public_ip(value.parse().unwrap()), "{value}");
        }
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn source_urls_are_http_without_credentials_or_unusual_ports() {
        assert!(validate_source_url("https://example.com/report").is_ok());
        assert!(validate_source_url("file:///etc/passwd").is_err());
        assert!(validate_source_url("https://user:secret@example.com/").is_err());
        assert!(validate_source_url("https://example.com:8443/").is_err());
        assert!(validate_source_url("http://example.com:443/").is_err());
        assert!(validate_source_url("https://example.com:80/").is_err());
    }

    #[test]
    fn parses_pi_web_access_exa_text_blocks() {
        let parsed = parse_search_results(
            "Title: First report\nURL: https://example.com/one\nText: First snippet\n---\nTitle: Second report\nURL: https://example.org/two\nHighlights:\nSecond snippet\n---",
        )
        .unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].title, "First report");
        assert_eq!(parsed[0].snippet, "First snippet");
        assert!(parsed[0].source_id.starts_with("web:"));
    }

    #[test]
    fn accepts_a_valid_empty_search_response() {
        assert!(parse_search_results(r#"{"results":[]}"#)
            .unwrap()
            .is_empty());
        assert!(parse_search_results("No relevant results found.")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn adapts_and_enforces_keyless_search_filters_locally() {
        let query = WebSearchQuery {
            query: "quarterly report".into(),
            limit: 2,
            recency: Some(WebRecency::Year),
            include_domains: vec!["example.com".into()],
            exclude_domains: vec!["blocked.example.com".into()],
        };
        let adapted = adapted_search_query(&query);
        assert!(adapted.contains("Prefer sources published on or after"));
        assert!(adapted.contains("Include only sources from: example.com"));
        assert!(adapted.contains("Exclude sources from: blocked.example.com"));
        assert_eq!(provider_result_limit(&query), 10);

        let mut sources = vec![
            web_source(
                "kept".into(),
                "https://news.example.com/report".into(),
                String::new(),
                None,
            ),
            web_source(
                "blocked".into(),
                "https://blocked.example.com/report".into(),
                String::new(),
                None,
            ),
            web_source(
                "wrong domain".into(),
                "https://example.org/report".into(),
                String::new(),
                None,
            ),
            web_source(
                "too old".into(),
                "https://example.com/old".into(),
                String::new(),
                Some("2000-01-01T00:00:00Z".into()),
            ),
        ];
        apply_search_filters_counted(&query, &mut sources);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].title, "kept");
    }

    #[test]
    fn retries_only_transient_search_failures() {
        assert!(matches!(
            status_error(StatusCode::TOO_MANY_REQUESTS),
            WebError::RateLimited { .. }
        ));
        assert!(retryable_search_error(&WebError::RateLimited {
            retry_after: None
        }));
        assert!(retryable_search_error(&WebError::ProviderStatus(503)));
        assert!(!retryable_search_error(&WebError::ProviderStatus(403)));
        assert!(!retryable_search_error(&WebError::UrlDenied));
        // Codex GPT-5.6 Luna used to 400 on remapped hosted tool_choice; that
        // is a provider error, not a retryable transport failure. A 200
        // completion without a hosted search call is InvalidProviderResponse.
        assert!(!retryable_search_error(&WebError::ProviderStatus(400)));
        assert!(fallbackable_search_error(&WebError::ProviderStatus(400)));
        assert!(fallbackable_search_error(
            &WebError::InvalidProviderResponse
        ));
    }

    #[test]
    fn extracts_bounded_readable_html_without_script_text() {
        let (title, content) = crate::document::extract_html(
            "<html><head><title> Report </title><script>ignore()</script></head><body><main><h1>Heading</h1><p>Useful <b>text</b>.</p><table><tr><th>Year</th><th>Revenue</th></tr><tr><td>2024</td><td>10</td></tr></table></main></body></html>",
            "fallback",
        );
        assert_eq!(title, "Report");
        assert!(content.contains("Heading"));
        assert!(content.contains("Useful"));
        assert!(content.contains("text"));
        assert!(!content.contains("ignore"));
        assert!(content.contains('|') || content.contains("Revenue"));
    }

    #[test]
    fn decodes_pdf_bytes_without_utf8_conversion() {
        let bytes = crate::document::extract(b"%PDF-not-valid", "application/pdf", 1024);
        assert!(bytes.is_err());
        let html = decode_fetched_content(
            b"<html><head><title>Note</title></head><body><p>Hello</p></body></html>",
            "text/html; charset=utf-8",
            "text/html",
            "fallback",
            &Url::parse("https://example.com/note").unwrap(),
        )
        .unwrap();
        assert!(html.content.contains("Hello"));
        assert_eq!(html.document_kind, "html");
        assert_eq!(html.extraction_method, "readability");
    }

    #[test]
    fn decodes_web_charset_and_conservative_octet_stream_text() {
        let base = Url::parse("https://example.com/report").unwrap();
        let legacy = decode_fetched_content(
            b"caf\xe9",
            "text/plain; charset=windows-1252",
            "text/plain",
            "fallback",
            &base,
        )
        .unwrap();
        assert_eq!(legacy.content, "café");
        assert_eq!(legacy.charset.as_deref(), Some("windows-1252"));
        assert_eq!(legacy.extraction_method, "direct_text");

        let markdown = decode_fetched_content(
            b"<note>\n# Heading\n\nMarkdown that starts with a tag stays Markdown.",
            "text/markdown; charset=utf-8",
            "text/markdown",
            "note",
            &base,
        )
        .unwrap();
        assert_eq!(markdown.document_kind, "markdown");
        assert_eq!(markdown.extraction_method, "direct_markdown");
        assert!(markdown.content.starts_with("<note>"));

        let xml = decode_fetched_content(
            b"<rss version=\"2.0\"><channel><title>Feed</title></channel></rss>",
            "application/xml",
            "application/xml",
            "feed",
            &base,
        )
        .unwrap();
        assert_eq!(xml.document_kind, "xml");
        assert_eq!(xml.extraction_method, "direct_text");

        let download = decode_fetched_content(
            b"symbol,value\nGURU,42",
            "application/octet-stream",
            "application/octet-stream",
            "report.csv",
            &base,
        )
        .unwrap();
        assert!(download.content.contains("GURU,42"));
        assert_eq!(download.document_kind, "text");
        assert_eq!(download.extraction_method, "direct_text");
        assert!(decode_fetched_content(
            b"\x01\x02\x03",
            "application/octet-stream",
            "application/octet-stream",
            "binary",
            &base,
        )
        .is_err());
    }

    #[test]
    fn pages_large_extracted_text_on_utf8_boundaries() {
        let content = "한".repeat(100_000);
        let (first, next) = content_page(&content, 0).unwrap();
        let next = next.expect("large content has another page");
        assert!(first.len() <= MAX_FETCH_PAGE_BYTES);
        assert!(content.is_char_boundary(next));
        let (second, _) = content_page(&content, next).unwrap();
        assert!(!second.is_empty());
        assert!(matches!(
            content_page(&content, 1),
            Err(WebError::InvalidContentOffset)
        ));
    }

    #[test]
    fn reads_json_rpc_or_sse_mcp_payloads() {
        let rpc = r#"{"result":{"content":[{"type":"text","text":"Title: A\nURL: https://example.com"}]}}"#;
        assert!(mcp_text(rpc).unwrap().starts_with("Title: A"));
        let sse = format!("event: message\ndata: {rpc}\n\n");
        assert!(mcp_text(&sse).unwrap().starts_with("Title: A"));
    }

    #[tokio::test]
    #[ignore = "requires access to the public Exa MCP service"]
    async fn live_exa_search_smoke() {
        let (output, sources) = search(WebSearchQuery {
            query: "IANA example domain".into(),
            limit: 2,
            recency: None,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
        })
        .await
        .unwrap();
        assert_eq!(output.selected_provider, "exa_public");
        assert!(output.untrusted_content);
        assert!(!sources.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires access to the public Exa MCP service"]
    async fn live_concurrent_exa_searches_are_serialized() {
        let searches = (0..5).map(|index| {
            search(WebSearchQuery {
                query: format!("IANA example domain {index}"),
                limit: 2,
                recency: None,
                include_domains: vec!["iana.org".into()],
                exclude_domains: Vec::new(),
            })
        });
        for result in join_all(searches).await {
            let (output, sources) = result.unwrap();
            assert_eq!(output.selected_provider, "exa_public");
            assert!(!sources.is_empty());
            assert!(sources.iter().all(|source| {
                Url::parse(&source.url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .is_some_and(|host| host_matches_domain(&host, "iana.org"))
            }));
        }
    }

    #[tokio::test]
    #[ignore = "requires public network access"]
    async fn live_public_fetch_smoke() {
        let source = web_source(
            "Example Domain".into(),
            "https://example.com/".into(),
            String::new(),
            None,
        );
        let output = fetch(&source).await.unwrap().page(0).unwrap();
        assert!(output.content.contains("Example Domain"));
        assert_eq!(output.document_kind, "html");
        assert_eq!(output.extraction_method, "readability");
        assert!(output.untrusted_content);
        assert_eq!(output.content_class, "untrusted_web");
    }

    #[tokio::test]
    #[ignore = "requires public network access"]
    async fn live_public_pdf_fetch_extracts_text_and_format_metadata() {
        let source = web_source(
            "W3C dummy PDF".into(),
            "https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf".into(),
            String::new(),
            None,
        );
        let output = fetch(&source).await.unwrap().page(0).unwrap();
        assert_eq!(output.document_kind, "pdf");
        assert_eq!(output.extraction_method, "pdf");
        assert_eq!(output.page_count, Some(1));
        assert!(output.content.contains("Dummy PDF file"));
    }

    #[tokio::test]
    #[ignore = "requires public network access"]
    async fn live_compressed_json_is_streamed_under_the_decoded_body_budget() {
        let source = web_source(
            "Compressed JSON".into(),
            "https://httpbingo.org/gzip".into(),
            String::new(),
            None,
        );
        let output = fetch(&source).await.unwrap().page(0).unwrap();
        assert_eq!(output.document_kind, "json");
        assert_eq!(output.extraction_method, "direct_text");
        assert!(output.content.contains("gzipped"));
        assert!(output.content.len() < MAX_FETCH_RESPONSE_BYTES);
    }

    #[tokio::test]
    #[ignore = "requires public network access"]
    async fn live_generic_mime_legacy_doc_uses_cfb_stream_detection() {
        let source = web_source(
            "Apache POI sample DOC".into(),
            "https://github.com/apache/poi/raw/refs/heads/trunk/test-data/document/SampleDoc.doc"
                .into(),
            String::new(),
            None,
        );
        let output = fetch(&source).await.unwrap().page(0).unwrap();
        assert_eq!(output.document_kind, "doc");
        assert_eq!(output.extraction_method, "office");
        assert!(output
            .content
            .to_ascii_lowercase()
            .contains("test document"));
    }

    #[tokio::test]
    #[ignore = "requires public network access"]
    async fn live_large_text_file_uses_rust_issued_paging_offsets() {
        let source = web_source(
            "RFC 9110".into(),
            "https://www.rfc-editor.org/rfc/rfc9110.txt".into(),
            String::new(),
            None,
        );
        let snapshot = fetch(&source).await.unwrap();
        let first = snapshot.page(0).unwrap();
        let next = first.next_offset.expect("RFC 9110 exceeds one page");
        assert_eq!(first.document_kind, "text");
        assert_eq!(first.extraction_method, "direct_text");
        assert_eq!(first.charset.as_deref(), Some("utf-8"));
        assert_eq!(first.content_bytes, next);

        let second = snapshot.page(next).unwrap();
        assert_eq!(second.content_offset, next);
        assert!(!second.content.is_empty());
        assert_eq!(second.content_sha256, first.content_sha256);
    }

    #[test]
    fn prefers_markdown_in_fetch_accept() {
        let markdown = FETCH_ACCEPT.find("text/markdown").expect("markdown");
        let plain = FETCH_ACCEPT.find("text/plain").expect("plain");
        let html = FETCH_ACCEPT.find("text/html").expect("html");
        assert!(markdown < plain);
        assert!(plain < html);
    }

    #[test]
    fn retry_after_parses_delta_seconds_and_http_date() {
        assert_eq!(parse_retry_after(Some("2")), Some(Duration::from_secs(2)));
        let past = httpdate::fmt_http_date(std::time::SystemTime::UNIX_EPOCH);
        assert_eq!(parse_retry_after(Some(&past)), Some(Duration::ZERO));
        assert_eq!(
            retry_after_delay(Some(Duration::ZERO), Duration::from_secs(15)),
            Some(Duration::ZERO)
        );
        assert_eq!(
            retry_after_delay(Some(Duration::from_secs(30)), Duration::from_secs(1)),
            None
        );
        assert_eq!(
            retry_after_delay(Some(Duration::from_secs(30)), Duration::from_secs(15)),
            Some(FETCH_RETRY_AFTER_CAP)
        );
    }

    #[test]
    fn quality_warnings_flag_short_js_and_navigation_pages() {
        assert_eq!(
            quality_warnings("Please enable JavaScript to continue.", true),
            vec!["very_short", "javascript_required"]
        );
        let nav = (0..20)
            .map(|index| format!("[Home {index}](/p{index})"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(quality_warnings(&nav, true).contains(&"navigation_heavy"));
        assert!(quality_warnings(&"Useful paragraph. ".repeat(40), true).is_empty());
    }

    #[test]
    fn alternates_are_resolved_without_fetching_and_capped_at_eight() {
        let mut links = String::from(
            r#"<html><head><link rel="alternate" type="text/markdown" href="/page.md"></head><body>"#,
        );
        for index in 0..10 {
            links.push_str(&format!(r#"<a href="/files/doc-{index}.pdf">PDF</a>"#));
        }
        links.push_str("</body></html>");
        let alternates =
            collect_alternates(&links, &Url::parse("https://example.com/article").unwrap());
        assert_eq!(alternates.len(), MAX_ALTERNATES);
        assert_eq!(alternates[0].kind, "markdown");
        assert_eq!(alternates[0].url, "https://example.com/page.md");
        assert!(alternates.iter().skip(1).all(|item| item.kind == "pdf"));
        assert!(alternates
            .iter()
            .all(|item| item.url.starts_with("https://")));
    }

    #[tokio::test]
    async fn markdown_content_negotiation_seals_exact_bytes() {
        let body = b"# Note\n\nExact markdown bytes.";
        let _guard = script::install(vec![(
            "https://example.com/note",
            vec![script::ok("text/markdown; charset=utf-8", body.to_vec())],
        )]);
        let source = web_source(
            "Note".into(),
            "https://example.com/note".into(),
            String::new(),
            None,
        );
        let output = fetch(&source).await.unwrap().page(0).unwrap();
        assert_eq!(output.extraction_method, "direct_markdown");
        assert_eq!(output.content_sha256, hex::encode(Sha256::digest(body)));
        let requests = script::requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].1.starts_with("text/markdown"));
    }

    #[tokio::test]
    async fn html_alternates_are_returned_and_not_auto_fetched() {
        let html = r#"<html><head><title>Spec</title><link rel="alternate" type="text/markdown" href="https://example.com/spec.md"></head><body><article><h1>Spec</h1><p>The specification text is long enough to avoid a short warning.</p><p><a href="/spec.pdf">PDF</a></p></article></body></html>"#;
        let _guard = script::install(vec![(
            "https://example.com/spec",
            vec![script::ok("text/html", html.as_bytes().to_vec())],
        )]);
        let source = web_source(
            "Spec".into(),
            "https://example.com/spec".into(),
            String::new(),
            None,
        );
        let output = fetch(&source).await.unwrap().page(0).unwrap();
        assert_eq!(output.extraction_method, "readability");
        assert_eq!(output.alternates.len(), 2);
        assert_eq!(output.alternates[0].url, "https://example.com/spec.md");
        assert_eq!(output.alternates[1].kind, "pdf");
        assert_eq!(script::requests().len(), 1);
    }

    #[tokio::test]
    async fn get_429_retries_once_honoring_retry_after() {
        let body = b"ok after retry";
        let _guard = script::install(vec![(
            "https://example.com/limited",
            vec![
                script::ScriptedResponse {
                    status: 429,
                    content_type: "text/plain".into(),
                    body: b"slow down".to_vec(),
                    retry_after: Some("0".into()),
                    location: None,
                },
                script::ok("text/plain; charset=utf-8", body.to_vec()),
            ],
        )]);
        let source = web_source(
            "Limited".into(),
            "https://example.com/limited".into(),
            String::new(),
            None,
        );
        let output = fetch(&source).await.unwrap().page(0).unwrap();
        assert_eq!(output.content, "ok after retry");
        assert_eq!(script::requests().len(), 2);
    }

    #[tokio::test]
    async fn wikipedia_official_parse_seals_representation_url_and_bytes() {
        let api = official::match_official(
            &Url::parse("https://en.wikipedia.org/wiki/Ada_Lovelace").unwrap(),
        )
        .unwrap();
        let body = br#"{"parse":{"title":"Ada Lovelace","displaytitle":"Ada Lovelace","text":"<p>Ada Lovelace wrote the first published computer algorithm for the Analytical Engine.</p>"}}"#;
        let _guard = script::install(vec![(
            "en.wikipedia.org/w/api.php",
            vec![script::ok("application/json", body.to_vec())],
        )]);
        let source = web_source(
            "Ada Lovelace".into(),
            "https://en.wikipedia.org/wiki/Ada_Lovelace".into(),
            String::new(),
            None,
        );
        let output = fetch(&source).await.unwrap().page(0).unwrap();
        assert_eq!(output.extraction_method, "official_api");
        assert_eq!(output.representation_provider, Some("wikipedia"));
        assert_eq!(output.record_id.as_deref(), Some("Ada Lovelace"));
        assert_eq!(
            output.requested_url,
            "https://en.wikipedia.org/wiki/Ada_Lovelace"
        );
        assert_eq!(output.representation_url, api.representation_url.as_str());
        assert_eq!(output.content_sha256, hex::encode(Sha256::digest(body)));
        assert!(output.content.contains("Analytical Engine"));
        assert_eq!(script::requests().len(), 1);
        assert_eq!(script::requests()[0].1, FETCH_OFFICIAL_ACCEPT);
    }

    #[tokio::test]
    async fn official_identity_mismatch_falls_back_once_to_generic_fetch() {
        let html = r#"<html><head><title>USA</title></head><body><article><p>The original article body stays visible after official identity mismatch.</p></article></body></html>"#;
        let _guard = script::install(vec![
            (
                "en.wikipedia.org/w/api.php",
                vec![script::ok(
                    "application/json",
                    br#"{"parse":{"title":"United States","text":"<p>Wrong page</p>"}}"#.to_vec(),
                )],
            ),
            (
                "https://en.wikipedia.org/wiki/USA",
                vec![script::ok("text/html", html.as_bytes().to_vec())],
            ),
        ]);
        let source = web_source(
            "USA".into(),
            "https://en.wikipedia.org/wiki/USA".into(),
            String::new(),
            None,
        );
        let output = fetch(&source).await.unwrap().page(0).unwrap();
        assert_eq!(output.extraction_method, "readability");
        assert!(output.quality_warnings.contains(&"official_fallback"));
        assert!(output.content.contains("original article body"));
        assert!(!output.content.contains("Wrong page"));
        let urls = script::requests()
            .into_iter()
            .map(|(url, _)| url)
            .collect::<Vec<_>>();
        assert_eq!(urls.len(), 2);
        assert!(urls[0].contains("action=parse"));
        assert_eq!(urls[1], "https://en.wikipedia.org/wiki/USA");
    }

    #[tokio::test]
    async fn official_security_failure_does_not_fallback_to_generic_fetch() {
        let _guard = script::install(vec![
            (
                "en.wikipedia.org/w/api.php",
                vec![script::ScriptedResponse {
                    status: 302,
                    content_type: String::new(),
                    body: Vec::new(),
                    retry_after: None,
                    location: Some("http://10.0.0.1/private".into()),
                }],
            ),
            (
                "https://en.wikipedia.org/wiki/USA",
                vec![script::ok("text/html", b"must not be fetched".to_vec())],
            ),
        ]);
        let source = web_source(
            "USA".into(),
            "https://en.wikipedia.org/wiki/USA".into(),
            String::new(),
            None,
        );
        assert!(matches!(fetch(&source).await, Err(WebError::AddressDenied)));
        let urls = script::requests()
            .into_iter()
            .map(|(url, _)| url)
            .collect::<Vec<_>>();
        assert_eq!(urls.len(), 2);
        assert!(urls[0].contains("action=parse"));
        assert_eq!(urls[1], "http://10.0.0.1/private");
    }

    #[tokio::test]
    async fn unrecognized_urls_skip_official_handlers() {
        let html = r#"<html><head><title>News</title></head><body><article><p>Ordinary news article with enough text for extraction.</p></article></body></html>"#;
        let _guard = script::install(vec![(
            "https://example.com/news",
            vec![script::ok("text/html", html.as_bytes().to_vec())],
        )]);
        let source = web_source(
            "News".into(),
            "https://example.com/news".into(),
            String::new(),
            None,
        );
        let output = fetch(&source).await.unwrap().page(0).unwrap();
        assert_eq!(output.extraction_method, "readability");
        assert!(output.representation_provider.is_none());
        assert_eq!(script::requests().len(), 1);
    }

    #[tokio::test]
    async fn wikidata_and_doi_official_paths_round_trip_record_ids() {
        let wikidata = br#"{"entities":{"Q42":{"id":"Q42","labels":{"en":{"value":"Douglas Adams"}},"descriptions":{"en":{"value":"English writer"}}}}}"#;
        let crossref = br#"{"message":{"DOI":"10.1038/nature12373","title":["A paper"],"abstract":"Observed result."}}"#;
        let _guard = script::install(vec![
            (
                "Special:EntityData/Q42.json",
                vec![script::ok("application/json", wikidata.to_vec())],
            ),
            (
                "api.crossref.org/works/",
                vec![script::ok("application/json", crossref.to_vec())],
            ),
        ]);
        let wiki_output = fetch(&web_source(
            "Q42".into(),
            "https://www.wikidata.org/wiki/Q42".into(),
            String::new(),
            None,
        ))
        .await
        .unwrap()
        .page(0)
        .unwrap();
        assert_eq!(wiki_output.representation_provider, Some("wikidata"));
        assert_eq!(wiki_output.record_id.as_deref(), Some("Q42"));
        assert!(wiki_output.content.contains("Douglas Adams"));

        let doi_output = fetch(&web_source(
            "DOI".into(),
            "https://doi.org/10.1038/nature12373".into(),
            String::new(),
            None,
        ))
        .await
        .unwrap()
        .page(0)
        .unwrap();
        assert_eq!(doi_output.representation_provider, Some("crossref"));
        assert_eq!(doi_output.record_id.as_deref(), Some("10.1038/nature12373"));
        assert!(doi_output.content.contains("Observed result"));
    }

    #[tokio::test]
    async fn scripted_redirect_to_private_address_is_denied() {
        let _guard = script::install(vec![(
            "https://example.com/bounce",
            vec![script::ScriptedResponse {
                status: 302,
                content_type: String::new(),
                body: Vec::new(),
                retry_after: None,
                location: Some("http://10.0.0.1/secret".into()),
            }],
        )]);
        let source = web_source(
            "Bounce".into(),
            "https://example.com/bounce".into(),
            String::new(),
            None,
        );
        assert!(matches!(fetch(&source).await, Err(WebError::AddressDenied)));
    }
}
