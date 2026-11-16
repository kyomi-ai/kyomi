// SPDX-License-Identifier: AGPL-3.0-or-later

//! Linkup backend (search + fetch). Wire format read from docs.linkup.so on
//! 2026-11-16: bearer auth; `POST /v1/search` with `outputType: searchResults`
//! -> `results[{type,name,url,content,publishedAt}]` (image results are
//! skipped); `POST /v1/fetch` -> `markdown`. HTTP 429 covers both rate limiting
//! and exhausted credits (the API does not distinguish them), 402 is an
//! unauthenticated request.

use async_trait::async_trait;
use kyomi_core::web_tools::WebBackendConfig;
use serde_json::{Value, json};

use super::http::{merge_options, post_json};
use super::{
    FetchRequest, FetchedPage, SearchRequest, SearchResults, WebError, WebFetchProvider, WebSearchProvider,
    WebSource, WebUsage, truncate_chars,
};

/// Keys Kyomi sets that options cannot override.
const PROTECTED: &[&str] = &["q", "maxResults", "url", "outputType"];
const DEFAULT_BASE_URL: &str = "https://api.linkup.so";

pub struct LinkupProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    options: serde_json::Map<String, Value>,
}

impl LinkupProvider {
    pub fn new(cfg: &WebBackendConfig) -> kyomi_core::Result<Self> {
        let api_key = cfg
            .api_key
            .clone()
            .ok_or_else(|| kyomi_core::Error::Internal("linkup requires an API key".into()))?;
        Ok(Self {
            client: kyomi_datasource_server::http_client()?,
            api_key,
            base_url: cfg.base_url.clone().unwrap_or_else(|| DEFAULT_BASE_URL.into()).trim_end_matches('/').to_string(),
            options: cfg.options.clone(),
        })
    }

    pub(crate) fn search_body(&self, req: &SearchRequest) -> Value {
        let mut body = json!({
            "q": req.query,
            "depth": "fast",
            "outputType": "searchResults",
            "maxResults": req.max_results.max(1),
        });
        if !req.allowed_domains.is_empty() {
            body["includeDomains"] = json!(req.allowed_domains);
        }
        if !req.blocked_domains.is_empty() {
            body["excludeDomains"] = json!(req.blocked_domains);
        }
        if let Some(days) = req.recency_days {
            let from = chrono::Utc::now().date_naive() - chrono::Duration::days(i64::from(days));
            body["fromDate"] = json!(from.format("%Y-%m-%d").to_string());
        }
        merge_options(&mut body, &self.options, PROTECTED);
        body
    }
}

fn map_linkup(err: WebError) -> WebError {
    // 402: request without a usable key.
    match err {
        WebError::Upstream(m) if m.starts_with("HTTP 402") => WebError::Unauthorized,
        other => other,
    }
}

#[async_trait]
impl WebSearchProvider for LinkupProvider {
    fn name(&self) -> &str {
        "linkup"
    }

    async fn search(&self, request: &SearchRequest) -> Result<SearchResults, WebError> {
        let body = self.search_body(request);
        let resp = post_json(&self.client, &format!("{}/v1/search", self.base_url), &self.api_key, &body, &[])
            .await
            .map_err(map_linkup)?;
        let sources = resp
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| WebError::Upstream("linkup response has no `results` array".into()))?
            .iter()
            .filter(|r| r.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|r| {
                Some(WebSource {
                    title: r.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                    url: r.get("url").and_then(Value::as_str)?.to_string(),
                    snippet: r.get("content").and_then(Value::as_str).unwrap_or_default().to_string(),
                    score: None,
                    published: r.get("publishedAt").and_then(Value::as_str).map(str::to_string),
                })
            })
            .collect();
        Ok(SearchResults { query: request.query.clone(), sources, usage: WebUsage { calls: 1, credits: None } })
    }
}

#[async_trait]
impl WebFetchProvider for LinkupProvider {
    fn name(&self) -> &str {
        "linkup"
    }

    async fn fetch(&self, request: &FetchRequest) -> Result<FetchedPage, WebError> {
        let mut body = json!({ "url": request.url, "mode": "standard" });
        merge_options(&mut body, &self.options, PROTECTED);
        let resp = post_json(&self.client, &format!("{}/v1/fetch", self.base_url), &self.api_key, &body, &[])
            .await
            .map_err(map_linkup)?;
        let markdown = resp
            .get("markdown")
            .and_then(Value::as_str)
            .ok_or_else(|| WebError::Upstream("linkup fetch response has no `markdown`".into()))?;
        let (content, truncated) = truncate_chars(markdown, request.max_chars);
        Ok(FetchedPage { url: request.url.clone(), title: None, content, truncated, usage: WebUsage { calls: 1, credits: None } })
    }
}

#[cfg(test)]
mod tests {
    use super::super::http::test_server::serve;
    use super::*;
    use kyomi_core::web_tools::WebBackendKind;

    // Shaped from the schema/examples in docs.linkup.so (2026-11-16).
    const SEARCH_OK: &str = r#"{"results":[
        {"type":"text","name":"Microsoft 2024 Annual Report","url":"https://www.microsoft.com/investor/reports/ar24/index.html","content":"Highlights","favicon":"https://x/f.ico","publishedAt":"2024-07-30"},
        {"type":"image","name":"logo","url":"https://x/logo.png"}]}"#;

    fn provider(base: &str) -> LinkupProvider {
        LinkupProvider::new(&WebBackendConfig {
            kind: WebBackendKind::Linkup,
            api_key: Some("lk-test".into()),
            base_url: Some(base.into()),
            options: serde_json::Map::from_iter([("depth".to_string(), json!("deep"))]),
            ..Default::default()
        })
        .unwrap()
    }

    #[tokio::test]
    async fn search_body_and_mapping_skip_images() {
        let (base, mut rx) = serve(vec![(200, SEARCH_OK)]).await;
        let req = SearchRequest {
            query: "ms revenue".into(),
            max_results: 3,
            blocked_domains: vec!["spam.com".into()],
            ..Default::default()
        };
        let out = provider(&base).search(&req).await.unwrap();
        let seen = rx.recv().await.unwrap();
        assert!(seen.head.starts_with("POST /v1/search"));
        let body: Value = serde_json::from_str(&seen.body).unwrap();
        assert_eq!(body["q"], "ms revenue");
        assert_eq!(body["outputType"], "searchResults");
        assert_eq!(body["depth"], "deep", "options may tune non-protected fields");
        assert_eq!(body["excludeDomains"], json!(["spam.com"]));
        assert_eq!(out.sources.len(), 1);
        assert_eq!(out.sources[0].published.as_deref(), Some("2024-07-30"));
    }

    #[tokio::test]
    async fn statuses_map_to_normalised_errors() {
        for (status, expected) in [
            (401, WebError::Unauthorized),
            (402, WebError::Unauthorized),
            (429, WebError::RateLimited { retry_after: None }),
        ] {
            let (base, _rx) = serve(vec![(status, "{}")]).await;
            let err = provider(&base).search(&SearchRequest { query: "q".into(), max_results: 1, ..Default::default() }).await.unwrap_err();
            assert_eq!(err, expected, "status {status}");
        }
    }

    #[tokio::test]
    async fn fetch_returns_markdown_truncated() {
        let (base, mut rx) = serve(vec![(200, r##"{"markdown":"# Hello world"}"##)]).await;
        let page = provider(&base).fetch(&FetchRequest { url: "https://e.com".into(), max_chars: 7 }).await.unwrap();
        assert!(rx.recv().await.unwrap().head.starts_with("POST /v1/fetch"));
        assert_eq!(page.content, "# Hello");
        assert!(page.truncated);
    }
}
