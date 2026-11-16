// SPDX-License-Identifier: AGPL-3.0-or-later

//! Provider-executed web tools (`WEB_*_PROVIDER=anthropic`).
//!
//! Kyomi executes nothing here. The Anthropic adapter adds a server tool to the
//! request's `tools` array and Anthropic runs it; the response carries
//! `server_tool_use` / `web_search_tool_result` / `web_fetch_tool_result`
//! blocks. Wire format read from platform.claude.com on 2026-11-16.
//!
//! Three things need care and live here:
//!
//! * **Verbatim replay.** For a multi-turn exchange these blocks (including
//!   `encrypted_content` / `encrypted_index`) must be sent back exactly as
//!   received; a modified or missing value is a 400. [`parse_anthropic`]
//!   returns the assistant `content` array untouched as
//!   [`ServerToolActivity::provider_content`], which the agent stores on the
//!   assistant [`crate::types::Message`] and the adapter replays unchanged.
//! * **Tool version.** Deliberately `web_search_20250305` / `web_fetch_20250910`:
//!   these are called directly by the model. Newer versions default
//!   `allowed_callers` to code execution (dynamic filtering), which would pull a
//!   second billable tool and different ZDR/HIPAA eligibility into every
//!   request. An operator can opt in through the opaque options
//!   (`{"type": "web_search_20260209"}`); then `allowed_callers` is pinned to
//!   `["direct"]` unless the options say otherwise.
//! * **Budget from usage.** Kyomi cannot count these calls itself, so the
//!   monthly counters are fed from the response's `usage.server_tool_use`.

use kyomi_core::web_tools::{WebBackendConfig, WebCaps, WebToolKind};
use serde_json::{Value, json};

use super::WebSource;

/// A provider-executed tool entry plus which tool it is, so the agent can gate
/// each one (privacy, monthly budget) before every request.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerTool {
    pub kind: WebToolKind,
    pub entry: Value,
}

pub const ANTHROPIC_SEARCH_VERSION: &str = "web_search_20250305";
pub const ANTHROPIC_FETCH_VERSION: &str = "web_fetch_20250910";

/// Build the Messages-API `tools` entry for a provider-executed Anthropic tool.
///
/// `max_uses` is the per-turn cap forwarded to the provider (the options may
/// lower it, never raise it). Everything else comes from the opaque options.
pub fn anthropic_tool_entry(tool: WebToolKind, cfg: &WebBackendConfig, caps: &WebCaps) -> Value {
    let (default_type, cap) = match tool {
        WebToolKind::Search => (ANTHROPIC_SEARCH_VERSION, caps.per_turn_search),
        WebToolKind::Fetch => (ANTHROPIC_FETCH_VERSION, caps.per_turn_fetch),
    };
    let mut entry = json!({ "type": default_type, "name": tool.tool_name() });
    if tool == WebToolKind::Fetch {
        // Citations are off by default for fetch; they must be shown to users.
        entry["citations"] = json!({ "enabled": true });
    }
    let map = entry.as_object_mut().expect("object literal");
    for (k, v) in &cfg.options {
        if k != "name" && k != "max_uses" {
            map.insert(k.clone(), v.clone());
        }
    }
    let requested = cfg.options.get("max_uses").and_then(Value::as_u64).unwrap_or(u64::from(cap));
    map.insert("max_uses".into(), json!(requested.min(u64::from(cap))));
    // Versions after the basic ones default to code-execution callers; keep direct calls.
    let version = map.get("type").and_then(Value::as_str).unwrap_or(default_type).to_string();
    if version != default_type && !map.contains_key("allowed_callers") {
        map.insert("allowed_callers".into(), json!(["direct"]));
    }
    entry
}

/// What the provider did during one response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerToolActivity {
    /// The assistant `content` array, verbatim, when it contains server-tool
    /// blocks (otherwise `None`: nothing needs special replay).
    pub provider_content: Option<Vec<Value>>,
    /// Normalised sources: cited ones when the text carries citations,
    /// otherwise the search results.
    pub sources: Vec<WebSource>,
    /// Billable searches / fetches, from `usage.server_tool_use`.
    pub searches: u32,
    pub fetches: u32,
    /// Error codes the tool reported (`web_search_tool_result_error`, ...).
    pub errors: Vec<String>,
}

fn is_server_block(block: &Value) -> bool {
    match block.get("type").and_then(Value::as_str) {
        Some("server_tool_use") => true,
        Some(t) => t != "tool_result" && t.ends_with("_tool_result"),
        None => false,
    }
}

/// Parse the server-tool activity out of a raw Anthropic response.
pub fn parse_anthropic(raw: &Value) -> ServerToolActivity {
    let mut out = ServerToolActivity::default();
    let usage = raw.pointer("/usage/server_tool_use");
    let count = |key: &str| usage.and_then(|u| u.get(key)).and_then(Value::as_u64).unwrap_or(0) as u32;
    out.searches = count("web_search_requests");
    out.fetches = count("web_fetch_requests");

    let Some(blocks) = raw.get("content").and_then(Value::as_array) else { return out };
    if blocks.iter().any(is_server_block) {
        out.provider_content = Some(blocks.clone());
    }
    let mut results: Vec<WebSource> = Vec::new();
    let mut cited: Vec<WebSource> = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("web_search_tool_result") => match block.get("content") {
                Some(Value::Array(items)) => {
                    for item in items.iter().filter(|i| i.get("type").and_then(Value::as_str) == Some("web_search_result")) {
                        if let Some(url) = item.get("url").and_then(Value::as_str) {
                            results.push(WebSource {
                                title: item.get("title").and_then(Value::as_str).unwrap_or(url).to_string(),
                                url: url.to_string(),
                                snippet: String::new(),
                                score: None,
                                published: item.get("page_age").and_then(Value::as_str).map(str::to_string),
                            });
                        }
                    }
                }
                Some(err) => {
                    if let Some(code) = err.get("error_code").and_then(Value::as_str) {
                        out.errors.push(format!("web_search: {code}"));
                    }
                }
                None => {}
            },
            Some("web_fetch_tool_result") => {
                let c = block.get("content");
                if let Some(code) = c.and_then(|c| c.get("error_code")).and_then(Value::as_str) {
                    out.errors.push(format!("web_fetch: {code}"));
                } else if let Some(url) = c.and_then(|c| c.get("url")).and_then(Value::as_str) {
                    let title = c.and_then(|c| c.pointer("/content/title")).and_then(Value::as_str).unwrap_or(url);
                    results.push(WebSource {
                        title: title.to_string(),
                        url: url.to_string(),
                        snippet: String::new(),
                        score: None,
                        published: None,
                    });
                }
            }
            Some("text") => {
                for c in block.get("citations").and_then(Value::as_array).into_iter().flatten() {
                    if c.get("type").and_then(Value::as_str) == Some("web_search_result_location")
                        && let Some(url) = c.get("url").and_then(Value::as_str)
                    {
                        cited.push(WebSource {
                            title: c.get("title").and_then(Value::as_str).unwrap_or(url).to_string(),
                            url: url.to_string(),
                            snippet: c.get("cited_text").and_then(Value::as_str).unwrap_or_default().to_string(),
                            score: None,
                            published: None,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out.sources = dedupe(if cited.is_empty() { results } else { cited });
    out
}

/// Drop repeated URLs, keeping the first (cited text wins when it came first).
pub fn dedupe(sources: Vec<WebSource>) -> Vec<WebSource> {
    let mut seen = std::collections::HashSet::new();
    sources.into_iter().filter(|s| seen.insert(s.url.clone())).collect()
}

/// The sources footer appended to an answer so the UI shows what was used
/// (provider terms require citations to be displayed).
pub fn sources_footer(sources: &[WebSource]) -> String {
    const MAX: usize = 8;
    if sources.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n**Sources**\n");
    for s in sources.iter().take(MAX) {
        let title = s.title.replace(['[', ']'], "");
        out.push_str(&format!("- [{title}]({})\n", s.url));
    }
    out.trim_end().to_string()
}

/// Recorded-response fixtures shared with other test modules.
#[cfg(test)]
pub(crate) mod tests_support {
    use serde_json::{Value, json};

    /// Shaped from the response examples on platform.claude.com (2026-11-16).
    pub(crate) fn search_response_with_stop(stop_reason: &str) -> Value {
        json!({
            "id": "msg_1", "type": "message", "role": "assistant", "stop_reason": stop_reason,
            "content": [
                {"type": "text", "text": "I'll search for that."},
                {"type": "server_tool_use", "id": "srvtoolu_01", "name": "web_search", "input": {"query": "rust 2026"}},
                {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_01", "content": [
                    {"type": "web_search_result", "url": "https://a.example/x", "title": "A", "encrypted_content": "ENC-A==", "page_age": "April 30, 2026"},
                    {"type": "web_search_result", "url": "https://b.example/y", "title": "B", "encrypted_content": "ENC-B==", "page_age": null}
                ]},
                {"type": "text", "text": "Rust is great.", "citations": [
                    {"type": "web_search_result_location", "url": "https://a.example/x", "title": "A", "encrypted_index": "IDX==", "cited_text": "Rust is..."}
                ]}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5, "server_tool_use": {"web_search_requests": 1}}
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(kind: kyomi_core::web_tools::WebBackendKind, options: Value) -> WebBackendConfig {
        WebBackendConfig { kind, api_key: None, base_url: None, options: options.as_object().cloned().unwrap_or_default() }
    }
    fn caps() -> WebCaps {
        WebCaps { per_turn_search: 5, per_turn_fetch: 5, workspace_monthly: 200, global_monthly: 1000 }
    }
    use kyomi_core::web_tools::WebBackendKind::Anthropic;

    #[test]
    fn default_entries_use_direct_basic_versions_with_forwarded_cap() {
        let s = anthropic_tool_entry(WebToolKind::Search, &cfg(Anthropic, json!({})), &caps());
        assert_eq!(s, json!({"type": "web_search_20250305", "name": "web_search", "max_uses": 5}));
        let f = anthropic_tool_entry(WebToolKind::Fetch, &cfg(Anthropic, json!({})), &caps());
        assert_eq!(
            f,
            json!({"type": "web_fetch_20250910", "name": "web_fetch", "max_uses": 5, "citations": {"enabled": true}})
        );
    }

    #[test]
    fn options_pass_through_but_cannot_raise_the_cap_or_rename() {
        let o = json!({"max_uses": 50, "name": "evil", "blocked_domains": ["x.com"], "user_location": {"type": "approximate", "country": "US"}});
        let e = anthropic_tool_entry(WebToolKind::Search, &cfg(Anthropic, o), &caps());
        assert_eq!(e["max_uses"], 5);
        assert_eq!(e["name"], "web_search");
        assert_eq!(e["blocked_domains"], json!(["x.com"]));
        assert_eq!(e["user_location"]["country"], "US");
        let lowered = anthropic_tool_entry(WebToolKind::Search, &cfg(Anthropic, json!({"max_uses": 2})), &caps());
        assert_eq!(lowered["max_uses"], 2);
    }

    #[test]
    fn newer_versions_are_pinned_to_direct_callers_unless_overridden() {
        let e = anthropic_tool_entry(WebToolKind::Search, &cfg(Anthropic, json!({"type": "web_search_20260209"})), &caps());
        assert_eq!(e["allowed_callers"], json!(["direct"]));
        let o = json!({"type": "web_search_20260209", "allowed_callers": ["code_execution_20260120"]});
        let e = anthropic_tool_entry(WebToolKind::Search, &cfg(Anthropic, o), &caps());
        assert_eq!(e["allowed_callers"], json!(["code_execution_20260120"]));
    }

    fn search_response() -> Value {
        tests_support::search_response_with_stop("end_turn")
    }

    #[test]
    fn parse_keeps_blocks_verbatim_and_normalises_sources_and_usage() {
        let raw = search_response();
        let a = parse_anthropic(&raw);
        assert_eq!(a.provider_content.as_ref().unwrap(), raw["content"].as_array().unwrap(), "verbatim, incl. encrypted_*");
        assert_eq!(a.searches, 1);
        assert_eq!(a.fetches, 0);
        assert_eq!(a.sources.len(), 1, "cited sources win over the raw result list");
        assert_eq!(a.sources[0].url, "https://a.example/x");
        assert_eq!(a.sources[0].snippet, "Rust is...");
    }

    #[test]
    fn without_citations_the_search_results_are_the_sources() {
        let mut raw = search_response();
        raw["content"][3]["citations"] = json!([]);
        let a = parse_anthropic(&raw);
        assert_eq!(a.sources.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(), ["https://a.example/x", "https://b.example/y"]);
    }

    #[test]
    fn plain_responses_need_no_special_replay() {
        let raw = json!({"content": [{"type": "text", "text": "hi"}, {"type": "tool_use", "id": "t", "name": "x", "input": {}}], "usage": {}});
        assert_eq!(parse_anthropic(&raw), ServerToolActivity::default());
    }

    #[test]
    fn fetch_results_and_tool_errors_are_parsed() {
        let raw = json!({
            "content": [
                {"type": "server_tool_use", "id": "srvtoolu_2", "name": "web_fetch", "input": {"url": "https://c.example"}},
                {"type": "web_fetch_tool_result", "tool_use_id": "srvtoolu_2", "content": {"type": "web_fetch_result", "url": "https://c.example",
                    "content": {"type": "document", "title": "C page", "source": {"type": "text", "media_type": "text/plain", "data": "x"}}, "retrieved_at": "t"}},
                {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_3", "content": {"type": "web_search_tool_result_error", "error_code": "max_uses_exceeded"}}
            ],
            "usage": {"server_tool_use": {"web_fetch_requests": 1}}
        });
        let a = parse_anthropic(&raw);
        assert_eq!(a.fetches, 1);
        assert_eq!(a.sources[0].title, "C page");
        assert_eq!(a.errors, ["web_search: max_uses_exceeded"]);
    }

    #[test]
    fn footer_lists_sources_as_links_and_is_empty_without_sources() {
        assert_eq!(sources_footer(&[]), "");
        let f = sources_footer(&parse_anthropic(&search_response()).sources);
        assert_eq!(f, "\n\n**Sources**\n- [A](https://a.example/x)");
    }
}
