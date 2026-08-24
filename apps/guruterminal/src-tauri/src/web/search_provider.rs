use std::{future::pending, time::Duration};

use serde::Serialize;
use tokio::{sync::watch, time::Instant};

use super::{
    apply_search_filters_counted, clamp_retry_after, fallbackable_search_error, now_rfc3339,
    retry_after_from_error, retryable_search_error, WebError, WebSearchOutput, WebSearchQuery,
    WebSource, WebSourceOutput, SEARCH_RETRY_DELAY,
};

const SEARCH_OPERATION_TIMEOUT: Duration = Duration::from_secs(60);
// Keep native attempts short enough that Automatic still has one complete
// anonymous-search window within the bounded operation.
const NATIVE_SEARCH_TIMEOUT: Duration = Duration::from_secs(39);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SearchProviderId {
    OpenaiCodex,
    Anthropic,
    Xai,
    ExaPublic,
}

impl SearchProviderId {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "openai-codex" => Some(Self::OpenaiCodex),
            "anthropic" => Some(Self::Anthropic),
            "xai" => Some(Self::Xai),
            "exa_public" => Some(Self::ExaPublic),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCodex => "openai-codex",
            Self::Anthropic => "anthropic",
            Self::Xai => "xai",
            Self::ExaPublic => "exa_public",
        }
    }

    pub fn kind(self) -> &'static str {
        match self {
            Self::OpenaiCodex | Self::Anthropic | Self::Xai => "model_native",
            Self::ExaPublic => "keyless",
        }
    }

    pub fn is_native(&self) -> bool {
        matches!(self, Self::OpenaiCodex | Self::Anthropic | Self::Xai)
    }

    pub fn timeout(self) -> Duration {
        if self.is_native() {
            NATIVE_SEARCH_TIMEOUT
        } else {
            super::SEARCH_TIMEOUT
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WebSearchPolicy {
    #[default]
    Automatic,
    ModelOnly,
    ExaOnly,
}

impl WebSearchPolicy {
    pub fn from_config_value(value: Option<&str>) -> Option<Self> {
        match value {
            None | Some("automatic") => Some(Self::Automatic),
            Some("model_only") => Some(Self::ModelOnly),
            Some("exa_only") => Some(Self::ExaOnly),
            Some(_) => None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct SearchDroppedCounts {
    pub invalid_url: usize,
    pub include_domain: usize,
    pub exclude_domain: usize,
    pub recency: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SearchAttemptReceipt {
    pub provider: String,
    pub kind: &'static str,
    pub status: &'static str,
    pub retry_count: u8,
    pub result_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_request_count: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct SearchHits {
    pub sources: Vec<WebSource>,
    pub model: Option<String>,
    pub search_request_count: Option<u32>,
    pub retry_after: Option<Duration>,
}

#[derive(Clone, Debug)]
pub struct SearchCancel {
    inner: Option<watch::Receiver<bool>>,
}

impl Default for SearchCancel {
    fn default() -> Self {
        Self::never()
    }
}

impl SearchCancel {
    pub fn never() -> Self {
        Self { inner: None }
    }

    pub fn from_watch(receiver: watch::Receiver<bool>) -> Self {
        Self {
            inner: Some(receiver),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|receiver| *receiver.borrow())
    }

    pub async fn cancelled(&self) {
        let Some(mut receiver) = self.inner.clone() else {
            pending::<()>().await;
            return;
        };
        if *receiver.borrow_and_update() {
            return;
        }
        let _ = receiver.changed().await;
    }
}

pub struct SearchRequest {
    pub query: WebSearchQuery,
    pub policy: WebSearchPolicy,
    pub chat_provider: Option<String>,
    pub cancel: SearchCancel,
}

pub trait SearchBackend {
    fn attempt(
        &self,
        provider: SearchProviderId,
        query: &WebSearchQuery,
        remaining: Duration,
        cancel: &SearchCancel,
    ) -> impl std::future::Future<Output = Result<SearchHits, WebError>> + Send;
}

pub fn resolve_search_candidates(
    policy: WebSearchPolicy,
    chat_provider: Option<&str>,
) -> Result<Vec<SearchProviderId>, WebError> {
    match policy {
        WebSearchPolicy::ModelOnly => {
            let Some(provider) = chat_provider.and_then(SearchProviderId::parse) else {
                return Err(WebError::ProviderMessage(
                    "the current Chat provider does not support native web search".into(),
                ));
            };
            if !provider.is_native() {
                return Err(WebError::ProviderMessage(
                    "the current Chat provider does not support native web search".into(),
                ));
            }
            Ok(vec![provider])
        }
        WebSearchPolicy::ExaOnly => Ok(vec![SearchProviderId::ExaPublic]),
        WebSearchPolicy::Automatic => {
            let mut candidates = Vec::new();
            if let Some(provider) =
                chat_provider
                    .and_then(SearchProviderId::parse)
                    .filter(|provider| {
                        matches!(
                            provider,
                            SearchProviderId::OpenaiCodex | SearchProviderId::Anthropic
                        )
                    })
            {
                candidates.push(provider);
            }
            candidates.push(SearchProviderId::ExaPublic);
            Ok(candidates)
        }
    }
}

pub async fn execute_search<B: SearchBackend>(
    request: SearchRequest,
    backend: &B,
) -> Result<(WebSearchOutput, Vec<WebSource>), WebError> {
    let deadline = Instant::now() + SEARCH_OPERATION_TIMEOUT;
    let candidates = resolve_search_candidates(request.policy, request.chat_provider.as_deref())?;
    let mut attempts = Vec::new();
    let mut warnings = Vec::new();
    let mut last_error: Option<WebError> = None;

    for (index, provider) in candidates.iter().copied().enumerate() {
        if request.cancel.is_cancelled() {
            return Err(WebError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            last_error = Some(WebError::Timeout);
            break;
        }
        match attempt_provider(
            provider,
            &request.query,
            remaining.min(provider.timeout()),
            &request.cancel,
            backend,
        )
        .await
        {
            (Ok(hits), retry_count) => {
                let received_count = hits.sources.len();
                let mut sources = hits.sources;
                let dropped = apply_search_filters_counted(&request.query, &mut sources);
                sources.truncate(request.query.limit as usize);
                if sources.iter().any(|source| source.published_at.is_none()) {
                    warnings.push("undated_result".into());
                }
                if hits.search_request_count.is_none()
                    && hits.model.is_none()
                    && provider.is_native()
                {
                    warnings.push("usage_metadata_unavailable".into());
                }
                if index > 0 {
                    warnings.push("provider_fallback".into());
                }
                attempts.push(SearchAttemptReceipt {
                    provider: provider.as_str().to_owned(),
                    kind: provider.kind(),
                    status: "ok",
                    retry_count,
                    result_count: received_count,
                    model: hits.model,
                    search_request_count: hits.search_request_count,
                });
                let returned_count = sources.len();
                let output = WebSearchOutput {
                    selected_provider: provider.as_str().to_owned(),
                    retrieved_at: now_rfc3339(),
                    untrusted_content: true,
                    results: sources
                        .iter()
                        .map(|source| WebSourceOutput {
                            source_id: source.source_id.clone(),
                            title: source.title.clone(),
                            url: source.url.clone(),
                            snippet: source.snippet.clone(),
                            published_at: source.published_at.clone(),
                        })
                        .collect(),
                    attempts,
                    received_count,
                    returned_count,
                    dropped,
                    warnings,
                };
                return Ok((output, sources));
            }
            (Err(error), _) if matches!(error, WebError::Cancelled) => return Err(error),
            (Err(error), retry_count)
                if fallbackable_search_error(&error) && index + 1 < candidates.len() =>
            {
                attempts.push(SearchAttemptReceipt {
                    provider: provider.as_str().to_owned(),
                    kind: provider.kind(),
                    status: "error",
                    retry_count,
                    result_count: 0,
                    model: None,
                    search_request_count: None,
                });
                last_error = Some(error);
            }
            (Err(error), _) => return Err(error),
        }
    }

    Err(last_error.unwrap_or(WebError::ProviderUnavailable))
}

async fn attempt_provider<B: SearchBackend>(
    provider: SearchProviderId,
    query: &WebSearchQuery,
    timeout: Duration,
    cancel: &SearchCancel,
    backend: &B,
) -> (Result<SearchHits, WebError>, u8) {
    let started = Instant::now();
    let remaining = || timeout.saturating_sub(started.elapsed());
    let first = {
        let attempt = backend.attempt(provider, query, remaining(), cancel);
        tokio::pin!(attempt);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return (Err(WebError::Cancelled), 0),
            result = tokio::time::timeout(remaining(), attempt) => {
                result.unwrap_or(Err(WebError::Timeout))
            },
        }
    };
    match first {
        Ok(hits) => (Ok(hits), 0),
        Err(error) if matches!(error, WebError::Cancelled) => (Err(error), 0),
        Err(error) if retryable_search_error(&error) => {
            if remaining().is_zero() {
                return (Err(error), 0);
            }
            let wait = retry_after_from_error(&error)
                .and_then(|requested| clamp_retry_after(requested, remaining()))
                .or_else(|| {
                    if retry_after_from_error(&error).is_some() {
                        None
                    } else {
                        Some(SEARCH_RETRY_DELAY.min(remaining()))
                    }
                });
            let Some(wait) = wait else {
                return (Err(error), 0);
            };
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return (Err(WebError::Cancelled), 0),
                _ = tokio::time::sleep(wait) => {}
            }
            let retry = {
                let attempt = backend.attempt(provider, query, remaining(), cancel);
                tokio::pin!(attempt);
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return (Err(WebError::Cancelled), 1),
                    result = tokio::time::timeout(remaining(), attempt) => {
                        result.unwrap_or(Err(WebError::Timeout))
                    },
                }
            };
            match retry {
                Ok(hits) => (Ok(hits), 1),
                Err(error) => (Err(error), 1),
            }
        }
        Err(error) => (Err(error), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::{web_source, WebRecency};
    use std::sync::Mutex;

    struct ScriptedBackend {
        calls: Mutex<Vec<SearchProviderId>>,
        outcomes: Mutex<Vec<Result<SearchHits, WebError>>>,
    }

    impl ScriptedBackend {
        fn new(outcomes: Vec<Result<SearchHits, WebError>>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                outcomes: Mutex::new(outcomes),
            }
        }
    }

    impl SearchBackend for ScriptedBackend {
        async fn attempt(
            &self,
            provider: SearchProviderId,
            _query: &WebSearchQuery,
            _remaining: Duration,
            cancel: &SearchCancel,
        ) -> Result<SearchHits, WebError> {
            if cancel.is_cancelled() {
                return Err(WebError::Cancelled);
            }
            self.calls.lock().unwrap().push(provider);
            self.outcomes.lock().unwrap().remove(0)
        }
    }

    fn query() -> WebSearchQuery {
        WebSearchQuery {
            query: "latest public report".into(),
            limit: 5,
            recency: None,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
        }
    }

    fn hit(url: &str) -> SearchHits {
        SearchHits {
            sources: vec![web_source(
                "Report".into(),
                url.into(),
                "snippet".into(),
                Some("2024-01-01T00:00:00Z".into()),
            )],
            model: Some("gpt-5.6-luna".into()),
            search_request_count: Some(1),
            retry_after: None,
        }
    }

    #[test]
    fn policies_resolve_deterministically_from_the_current_chat_provider() {
        assert_eq!(
            resolve_search_candidates(WebSearchPolicy::Automatic, Some("openai-codex")).unwrap(),
            vec![SearchProviderId::OpenaiCodex, SearchProviderId::ExaPublic]
        );
        assert_eq!(
            resolve_search_candidates(WebSearchPolicy::Automatic, Some("google")).unwrap(),
            vec![SearchProviderId::ExaPublic]
        );
        assert_eq!(
            resolve_search_candidates(WebSearchPolicy::Automatic, Some("xai")).unwrap(),
            vec![SearchProviderId::ExaPublic]
        );
        assert!(resolve_search_candidates(WebSearchPolicy::ModelOnly, Some("google")).is_err());
        assert_eq!(
            resolve_search_candidates(WebSearchPolicy::ModelOnly, Some("xai")).unwrap(),
            vec![SearchProviderId::Xai]
        );
        assert_eq!(
            resolve_search_candidates(WebSearchPolicy::ExaOnly, Some("anthropic")).unwrap(),
            vec![SearchProviderId::ExaPublic]
        );
        assert_eq!(
            WebSearchPolicy::from_config_value(None),
            Some(WebSearchPolicy::Automatic)
        );
        assert_eq!(
            WebSearchPolicy::from_config_value(Some("model_only")),
            Some(WebSearchPolicy::ModelOnly)
        );
        assert_eq!(WebSearchPolicy::from_config_value(Some("model")), None);
    }

    #[test]
    fn automatic_reserves_a_complete_exa_fallback_window() {
        assert!(NATIVE_SEARCH_TIMEOUT + super::super::SEARCH_TIMEOUT < SEARCH_OPERATION_TIMEOUT);
        for provider in [
            SearchProviderId::OpenaiCodex,
            SearchProviderId::Anthropic,
            SearchProviderId::Xai,
        ] {
            assert_eq!(provider.timeout(), NATIVE_SEARCH_TIMEOUT);
        }
    }

    #[tokio::test]
    async fn empty_native_success_does_not_fallback() {
        let backend = ScriptedBackend::new(vec![Ok(SearchHits::default())]);
        let (output, sources) = execute_search(
            SearchRequest {
                query: query(),
                policy: WebSearchPolicy::Automatic,
                chat_provider: Some("anthropic".into()),
                cancel: SearchCancel::never(),
            },
            &backend,
        )
        .await
        .unwrap();
        assert!(sources.is_empty());
        assert_eq!(output.selected_provider, "anthropic");
        assert_eq!(output.received_count, 0);
        assert_eq!(output.returned_count, 0);
        assert_eq!(
            backend.calls.lock().unwrap().as_slice(),
            &[SearchProviderId::Anthropic]
        );
    }

    #[tokio::test]
    async fn unavailable_native_falls_back_to_exa_public() {
        let backend = ScriptedBackend::new(vec![
            Err(WebError::ProviderUnavailable),
            Ok(hit("https://example.com/report")),
        ]);
        let (output, sources) = execute_search(
            SearchRequest {
                query: query(),
                policy: WebSearchPolicy::Automatic,
                chat_provider: Some("openai-codex".into()),
                cancel: SearchCancel::never(),
            },
            &backend,
        )
        .await
        .unwrap();
        assert_eq!(output.selected_provider, "exa_public");
        assert_eq!(sources.len(), 1);
        assert!(output
            .warnings
            .iter()
            .any(|warning| warning == "provider_fallback"));
        assert_eq!(
            backend.calls.lock().unwrap().as_slice(),
            &[SearchProviderId::OpenaiCodex, SearchProviderId::ExaPublic]
        );
    }

    #[tokio::test]
    async fn failed_retry_is_recorded_before_provider_fallback() {
        let backend = ScriptedBackend::new(vec![
            Err(WebError::Timeout),
            Err(WebError::Transport),
            Ok(hit("https://example.com/report")),
        ]);
        let (output, _) = execute_search(
            SearchRequest {
                query: query(),
                policy: WebSearchPolicy::Automatic,
                chat_provider: Some("openai-codex".into()),
                cancel: SearchCancel::never(),
            },
            &backend,
        )
        .await
        .unwrap();
        assert_eq!(output.selected_provider, "exa_public");
        assert_eq!(output.attempts[0].status, "error");
        assert_eq!(output.attempts[0].retry_count, 1);
        assert_eq!(backend.calls.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn malformed_and_missing_hosted_search_fall_back() {
        let backend = ScriptedBackend::new(vec![
            Err(WebError::InvalidProviderResponse),
            Ok(hit("https://example.com/report")),
        ]);
        let (output, _) = execute_search(
            SearchRequest {
                query: query(),
                policy: WebSearchPolicy::Automatic,
                chat_provider: Some("openai-codex".into()),
                cancel: SearchCancel::never(),
            },
            &backend,
        )
        .await
        .unwrap();
        assert_eq!(output.selected_provider, "exa_public");
    }

    #[tokio::test]
    async fn long_retry_after_does_not_block_fallback() {
        let backend = ScriptedBackend::new(vec![
            Err(WebError::RateLimited {
                retry_after: Some(Duration::from_secs(30)),
            }),
            Ok(hit("https://example.com/report")),
        ]);
        let (output, _) = execute_search(
            SearchRequest {
                query: query(),
                policy: WebSearchPolicy::Automatic,
                chat_provider: Some("openai-codex".into()),
                cancel: SearchCancel::never(),
            },
            &backend,
        )
        .await
        .unwrap();
        assert_eq!(output.selected_provider, "exa_public");
        assert_eq!(backend.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn transient_failure_retries_once_then_succeeds() {
        let backend = ScriptedBackend::new(vec![
            Err(WebError::Timeout),
            Ok(hit("https://example.com/report")),
        ]);
        let (output, _) = execute_search(
            SearchRequest {
                query: query(),
                policy: WebSearchPolicy::ExaOnly,
                chat_provider: None,
                cancel: SearchCancel::never(),
            },
            &backend,
        )
        .await
        .unwrap();
        assert_eq!(output.selected_provider, "exa_public");
        assert_eq!(output.attempts[0].retry_count, 1);
        assert_eq!(backend.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn cancellation_stops_retry_sleep() {
        let (tx, rx) = watch::channel(false);
        let backend = ScriptedBackend::new(vec![Err(WebError::RateLimited {
            retry_after: Some(Duration::from_secs(3)),
        })]);
        let search = execute_search(
            SearchRequest {
                query: query(),
                policy: WebSearchPolicy::ExaOnly,
                chat_provider: None,
                cancel: SearchCancel::from_watch(rx),
            },
            &backend,
        );
        tx.send_replace(true);
        let error = search.await.unwrap_err();
        assert!(matches!(error, WebError::Cancelled));
    }

    #[test]
    fn dropped_counts_separate_invalid_include_exclude_and_recency() {
        let query = WebSearchQuery {
            query: "report".into(),
            limit: 5,
            recency: Some(WebRecency::Year),
            include_domains: vec!["example.com".into()],
            exclude_domains: vec!["blocked.example.com".into()],
        };
        let mut sources = vec![
            web_source("bad".into(), "not-a-url".into(), String::new(), None),
            web_source(
                "blocked".into(),
                "https://blocked.example.com/a".into(),
                String::new(),
                None,
            ),
            web_source(
                "other".into(),
                "https://example.org/a".into(),
                String::new(),
                None,
            ),
            web_source(
                "old".into(),
                "https://example.com/old".into(),
                String::new(),
                Some("2000-01-01T00:00:00Z".into()),
            ),
            web_source(
                "kept".into(),
                "https://news.example.com/a".into(),
                String::new(),
                None,
            ),
        ];
        let dropped = apply_search_filters_counted(&query, &mut sources);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].title, "kept");
        assert_eq!(dropped.invalid_url, 1);
        assert_eq!(dropped.exclude_domain, 1);
        assert_eq!(dropped.include_domain, 1);
        assert_eq!(dropped.recency, 1);
    }

    #[test]
    fn retry_after_http_date_and_delta_seconds_are_clamped() {
        let seconds = crate::web::parse_retry_after_header(Some(
            &reqwest::header::HeaderValue::from_static("2"),
        ));
        assert_eq!(seconds, Some(Duration::from_secs(2)));
        let past = httpdate::fmt_http_date(std::time::SystemTime::UNIX_EPOCH);
        assert_eq!(
            crate::web::parse_retry_after_header(Some(
                &reqwest::header::HeaderValue::from_str(&past).unwrap(),
            )),
            Some(Duration::ZERO)
        );
        assert_eq!(
            clamp_retry_after(Duration::from_secs(2), Duration::from_secs(10)),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            clamp_retry_after(Duration::from_secs(30), Duration::from_secs(10)),
            None
        );
        let http_date =
            httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(2));
        let parsed = crate::web::parse_retry_after_header(Some(
            &reqwest::header::HeaderValue::from_str(&http_date).unwrap(),
        ));
        assert!(parsed.is_some_and(|duration| duration <= Duration::from_secs(3)));
    }
}
