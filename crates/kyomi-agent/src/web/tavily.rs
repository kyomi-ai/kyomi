// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tavily backend (search + extract). Wire format read from docs.tavily.com on
//! 2026-11-16: bearer auth; `POST /search` -> `results[{title,url,content,score,
//! published_date}]`; `POST /extract` -> `results[{url,raw_content}]` plus
//! `failed_results`; plan/key limits are HTTP 432/433/434.

use async_trait::async_trait;
use kyomi_core::web_tools::WebBackendConfig;
use serde_json::{Value, json};

use super::http::{merge_options, post_json};
use super::{
    FetchRequest, FetchedPage, SearchRequest, SearchResults, WebError, WebFetchProvider, WebSearchProvider,
    WebSource, WebUsage, truncate_chars,
};

const DEFAULT_BASE_URL: &str = "https://api.tavily.com";
/// Keys Kyomi sets that options cannot override.
const PROTECTED: &[&str] = &["query", "max_results", "urls"];
const QUOTA_STATUSES: &[u16] = &[432, 433, 434];

pub struct TavilyProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    options: serde_json::Map<String, Value>,
}

impl TavilyProvider {
    pub fn new(cfg: &WebBackendConfig) -> kyomi_core::Result<Self> {
        let api_key = cfg
            .api_key
            .clone()
            .ok_or_else(|| kyomi_core::Error::Internal("tavily requires an API key".into()))?;
        Ok(Self {
            client: kyomi_datasource_server::http_client()?,
            api_key,
            base_url: cfg.base_url.clone().unwrap_or_else(|| DEFAULT_BASE_URL.into()).trim_end_matches('/').to_string(),
            options: cfg.options.clone(),
        })
    }

    fn time_range(days: u32) -> &'static str {
        match days {
            0..=1 => "day",
            2..=7 => "week",
            8..=31 => "month",
            _ => "year",
        }
    }

    pub(crate) fn search_body(&self, req: &SearchRequest) -> Value {
        let mut body = json!({
            "query": req.query,
            "max_results": req.max_results.clamp(1, 20),
            "search_depth": "basic",
            "include_answer": false,
            "include_usage": true,
        });
        if let Some(days) = req.recency_days {
            body["time_range"] = json!(Self::time_range(days));
        }
        if !req.allowed_domains.is_empty() {
            body["include_domains"] = json!(req.allowed_domains);
        }
        if !req.blocked_domains.is_empty() {
            body["exclude_domains"] = json!(req.blocked_domains);
        }
        merge_options(&mut body, &self.options, PROTECTED);
        body
    }
}

#[async_trait]
impl WebSearchProvider for TavilyProvider {
    fn name(&self) -> &str {
        "tavily"
    }

    async fn search(&self, request: &SearchRequest) -> Result<SearchResults, WebError> {
        let body = self.search_body(request);
        let resp = post_json(&self.client, &format!("{}/search", self.base_url), &self.api_key, &body, QUOTA_STATUSES).await?;
        let sources = resp
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| WebError::Upstream("tavily response has no `results` array".into()))?
            .iter()
            .filter_map(|r| {
                Some(WebSource {
                    title: r.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
                    url: r.get("url").and_then(Value::as_str)?.to_string(),
                    snippet: r.get("content").and_then(Value::as_str).unwrap_or_default().to_string(),
                    score: r.get("score").and_then(Value::as_f64),
                    published: r.get("published_date").and_then(Value::as_str).map(str::to_string),
                })
            })
            .collect();
        Ok(SearchResults {
            query: request.query.clone(),
            sources,
            usage: WebUsage { calls: 1, credits: resp.pointer("/usage/credits").and_then(Value::as_f64) },
        })
    }
}

#[async_trait]
impl WebFetchProvider for TavilyProvider {
    fn name(&self) -> &str {
        "tavily"
    }

    async fn fetch(&self, request: &FetchRequest) -> Result<FetchedPage, WebError> {
        let mut body = json!({
            "urls": [request.url],
            "extract_depth": "basic",
            "format": "markdown",
            "include_usage": true,
        });
        merge_options(&mut body, &self.options, PROTECTED);
        let resp = post_json(&self.client, &format!("{}/extract", self.base_url), &self.api_key, &body, QUOTA_STATUSES).await?;
        // A 200 can carry only `failed_results`; both arrays must be checked.
        if let Some(first) = resp.get("results").and_then(Value::as_array).and_then(|a| a.first()) {
            let raw = first.get("raw_content").and_then(Value::as_str).unwrap_or_default();
            let (content, truncated) = truncate_chars(raw, request.max_chars);
            return Ok(FetchedPage {
                url: request.url.clone(),
                title: None,
                content,
                truncated,
                usage: WebUsage { calls: 1, credits: resp.pointer("/usage/credits").and_then(Value::as_f64) },
            });
        }
        let reason = resp
            .pointer("/failed_results/0/error")
            .and_then(Value::as_str)
            .unwrap_or("no content returned");
        Err(WebError::InvalidRequest(format!("could not fetch the page: {reason}")))
    }
}

#[cfg(test)]
mod tests {
    use super::super::http::test_server::serve;
    use super::*;
    use kyomi_core::web_tools::WebBackendKind;

    // Shaped from the response examples in docs.tavily.com (2026-11-16).
    const SEARCH_OK: &str = r#"{"query":"who is leo messi","images":[],"results":[
        {"title":"Lionel Messi Facts | Britannica","url":"https://www.britannica.com/facts/Lionel-Messi","content":"Messi is...","score":0.81,"published_date":"2024-01-02"},
        {"title":"No url"}],"response_time":1.67,"usage":{"credits":1},"request_id":"r"}"#;
    const EXTRACT_OK: &str = r##"{"results":[{"url":"https://e.com/a","raw_content":"# Title\nabcdefghij"}],"failed_results":[],"usage":{"credits":1}}"##;
    const EXTRACT_FAILED: &str = r#"{"results":[],"failed_results":[{"url":"https://e.com/a","error":"Failed to retrieve content"}]}"#;

    fn provider(base: &str) -> TavilyProvider {
        let cfg = WebBackendConfig {
            kind: WebBackendKind::Tavily,
            api_key: Some("tvly-test".into()),
            base_url: Some(base.into()),
            options: serde_json::Map::from_iter([("topic".to_string(), json!("news")), ("max_results".to_string(), json!(99)), ("search_depth".to_string(), json!("advanced"))]),
            ..Default::default()
        };
        TavilyProvider::new(&cfg).unwrap()
    }

    fn req() -> SearchRequest {
        SearchRequest {
            query: "who is leo messi".into(),
            max_results: 5,
            recency_days: Some(3),
            allowed_domains: vec!["britannica.com".into()],
            blocked_domains: vec![],
        }
    }

    #[tokio::test]
    async fn search_request_body_and_response_mapping() {
        let (base, mut rx) = serve(vec![(200, SEARCH_OK)]).await;
        let out = provider(&base).search(&req()).await.unwrap();
        let seen = rx.recv().await.unwrap();
        assert!(seen.head.to_lowercase().contains("authorization: bearer tvly-test"));
        assert!(seen.head.starts_with("POST /search"));
        let body: Value = serde_json::from_str(&seen.body).unwrap();
        assert_eq!(body["query"], "who is leo messi");
        assert_eq!(body["max_results"], 5, "options cannot lift the result cap");
        assert_eq!(body["time_range"], "week");
        assert_eq!(body["include_domains"], json!(["britannica.com"]));
        assert_eq!(body["topic"], "news", "opaque options pass through");
        assert_eq!(body["search_depth"], "advanced", "options may tune non-protected fields");
        assert_eq!(out.sources.len(), 1, "results without a url are dropped");
        assert_eq!(out.sources[0].title, "Lionel Messi Facts | Britannica");
        assert_eq!(out.sources[0].published.as_deref(), Some("2024-01-02"));
        assert_eq!(out.usage, WebUsage { calls: 1, credits: Some(1.0) });
    }

    #[tokio::test]
    async fn status_codes_map_to_normalised_errors() {
        for (status, expected) in [
            (401, WebError::Unauthorized),
            (432, WebError::QuotaExhausted),
            (433, WebError::QuotaExhausted),
            (429, WebError::RateLimited { retry_after: None }),
        ] {
            let (base, _rx) = serve(vec![(status, r#"{"detail":{"error":"x"}}"#)]).await;
            assert_eq!(provider(&base).search(&req()).await.unwrap_err(), expected, "status {status}");
        }
    }

    #[tokio::test]
    async fn extract_maps_content_truncates_and_reports_failed_results() {
        let (base, mut rx) = serve(vec![(200, EXTRACT_OK)]).await;
        let page = provider(&base)
            .fetch(&FetchRequest { url: "https://e.com/a".into(), max_chars: 8 })
            .await
            .unwrap();
        assert!(rx.recv().await.unwrap().head.starts_with("POST /extract"));
        assert_eq!(page.content, "# Title\n");
        assert!(page.truncated);

        let (base, _rx) = serve(vec![(200, EXTRACT_FAILED)]).await;
        let err = provider(&base)
            .fetch(&FetchRequest { url: "https://e.com/a".into(), max_chars: 100 })
            .await
            .unwrap_err();
        assert!(matches!(err, WebError::InvalidRequest(m) if m.contains("Failed to retrieve")));
    }
}
