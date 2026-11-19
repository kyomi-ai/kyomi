// SPDX-License-Identifier: AGPL-3.0-or-later

//! Provider-executed web tools (`WEB_*_PROVIDER=anthropic` / `openrouter`).
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
/// each one: the monthly budget once per turn, the per-turn allowance and privacy
/// before each request that is not a `pause_turn` continuation.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerTool {
    pub kind: WebToolKind,
    /// Which provider executes it; selects the response parser.
    pub backend: kyomi_core::web_tools::WebBackendKind,
    pub entry: Value,
}

impl ServerTool {
    /// The entry with this request's `max_uses` set to the allowance still left
    /// for the run (never above what the entry already allows).
    pub fn with_max_uses(&self, remaining: u32) -> Value {
        let mut entry = self.entry.clone();
        let slot = match self.backend {
            kyomi_core::web_tools::WebBackendKind::OpenRouter => entry.get_mut("parameters"),
            _ => Some(&mut entry),
        };
        if let Some(obj) = slot.and_then(Value::as_object_mut) {
            let current = obj.get("max_uses").and_then(Value::as_u64).unwrap_or(u64::from(remaining));
            obj.insert("max_uses".into(), json!(current.min(u64::from(remaining))));
        }
        entry
    }
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

/// Build the Chat Completions `tools` entry for OpenRouter's server tools
/// (`openrouter:web_search` / `openrouter:web_fetch`).
///
/// Verified with a live call on 2026-11-16: both are accepted on
/// `/api/v1/chat/completions` (not only on the Responses API). `parameters` is
/// the opaque options object (`engine`, `allowed_domains`, `max_results`, ...);
/// `max_uses` is the per-turn cap and options may lower it, never raise it.
pub fn openrouter_tool_entry(tool: WebToolKind, cfg: &WebBackendConfig, caps: &WebCaps) -> Value {
    let cap = match tool {
        WebToolKind::Search => caps.per_turn_search,
        WebToolKind::Fetch => caps.per_turn_fetch,
    };
    let mut parameters = cfg.options.clone();
    let requested = parameters.get("max_uses").and_then(Value::as_u64).unwrap_or(u64::from(cap));
    parameters.insert("max_uses".into(), json!(requested.min(u64::from(cap))));
    json!({ "type": format!("openrouter:{}", tool.tool_name()), "parameters": parameters })
}

/// Parse the server-tool activity out of a raw OpenRouter Chat Completions
/// response. Citations are `message.annotations[{type:"url_citation",
/// url_citation:{url,title,content}}]` (search only: fetch returns none);
/// usage is `usage.server_tool_use_details` (`web_search_requests`,
/// `tool_calls_executed`). No block needs verbatim replay on this API.
pub fn parse_openrouter(raw: &Value) -> ServerToolActivity {
    let mut out = ServerToolActivity::default();
    let details = raw.pointer("/usage/server_tool_use_details");
    let n = |k: &str| details.and_then(|d| d.get(k)).and_then(Value::as_u64).unwrap_or(0) as u32;
    out.searches = n("web_search_requests");
    // Fetches are not reported separately: whatever executed beyond the searches.
    out.fetches = n("tool_calls_executed").saturating_sub(out.searches);
    let mut sources = Vec::new();
    for ann in raw
        .pointer("/choices/0/message/annotations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|a| a.get("type").and_then(Value::as_str) == Some("url_citation"))
    {
        let Some(c) = ann.get("url_citation") else { continue };
        if let Some(url) = c.get("url").and_then(Value::as_str) {
            sources.push(WebSource {
                title: c.get("title").and_then(Value::as_str).unwrap_or(url).to_string(),
                url: url.to_string(),
                // The `content` field is the whole page text; not a snippet.
                snippet: String::new(),
                score: None,
                published: None,
            });
        }
    }
    out.sources = dedupe(sources);
    // OpenRouter reports citations for the response as a whole (search only), not
    // per call, and not the query: one display entry without an id or subject.
    if !out.sources.is_empty() {
        out.calls.push(ServerToolCall { id: None, tool: WebToolKind::Search, subject: String::new(), sources: out.sources.clone() });
    }
    out
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
    /// One entry per provider-executed call whose results are known, for the UI's
    /// source cards (the same cards Kyomi-executed calls produce).
    pub calls: Vec<ServerToolCall>,
    /// Calls this response announced with a `server_tool_use` block but did not
    /// finish (a `pause_turn` response ends with exactly such a block). The
    /// continuation carries their result blocks without repeating the
    /// `server_tool_use`, so it must be parsed with these to card them.
    pub pending: Vec<ServerToolCall>,
}

/// One provider-executed search or fetch, normalised for display.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerToolCall {
    /// The provider's id for the call (`srvtoolu_...`); `None` when the backend
    /// does not give one.
    pub id: Option<String>,
    pub tool: WebToolKind,
    /// The query (search) or URL (fetch) the model used; empty when not reported.
    pub subject: String,
    pub sources: Vec<WebSource>,
}

/// A server tool's result block type (`web_search_tool_result`, ...); the
/// client-tool `tool_result` is not one.
fn is_server_result_type(kind: &str) -> bool {
    kind != "tool_result" && kind.ends_with("_tool_result")
}

fn is_server_block(block: &Value) -> bool {
    match block.get("type").and_then(Value::as_str) {
        Some("server_tool_use") => true,
        Some(t) => is_server_result_type(t),
        None => false,
    }
}

/// Parse the server-tool activity out of a raw Anthropic response.
pub fn parse_anthropic(raw: &Value) -> ServerToolActivity {
    parse_anthropic_with_pending(raw, &[])
}

/// Parse a response that continues a paused server-tool turn. `pending` are the
/// calls the paused response announced without a result: the continuation
/// returns their result blocks without repeating the `server_tool_use` block.
pub fn parse_anthropic_with_pending(raw: &Value, pending: &[ServerToolCall]) -> ServerToolActivity {
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
            Some("web_search_tool_result") => {
                // An error result carries an object with `error_code` instead of a list.
                if let Some(code) = block.pointer("/content/error_code").and_then(Value::as_str) {
                    out.errors.push(format!("web_search: {code}"));
                }
                results.extend(result_sources(block));
            }
            Some("web_fetch_tool_result") => {
                if let Some(code) = block.pointer("/content/error_code").and_then(Value::as_str) {
                    out.errors.push(format!("web_fetch: {code}"));
                } else {
                    results.extend(result_sources(block));
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
    let (calls, unfinished) = anthropic_calls(blocks, pending);
    out.calls = calls;
    out.pending = unfinished;
    out
}

/// The tool and its subject (query or URL) of a `server_tool_use` block.
fn server_tool_intent(block: &Value) -> Option<(&str, WebToolKind, &str)> {
    let id = block.get("id").and_then(Value::as_str)?;
    let (tool, subject) = match block.get("name").and_then(Value::as_str)? {
        "web_search" => (WebToolKind::Search, block.pointer("/input/query")),
        "web_fetch" => (WebToolKind::Fetch, block.pointer("/input/url")),
        _ => return None,
    };
    Some((id, tool, subject.and_then(Value::as_str).unwrap_or_default()))
}

/// Pair each result block with its call — the `server_tool_use` in this response,
/// or a `pending` call from the paused response it continues — and normalise the
/// result's pages. A call whose result is an error or not in `blocks` is left
/// out: there is nothing to show; an unfinished call is returned as `pending`.
fn anthropic_calls(blocks: &[Value], pending: &[ServerToolCall]) -> (Vec<ServerToolCall>, Vec<ServerToolCall>) {
    let mut calls = Vec::new();
    let mut unfinished = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("server_tool_use") => {
                let Some((id, tool, subject)) = server_tool_intent(block) else { continue };
                let finished = blocks.iter().any(|result| {
                    result.get("tool_use_id").and_then(Value::as_str) == Some(id)
                        && result.get("type").and_then(Value::as_str).is_some_and(is_server_result_type)
                });
                if !finished {
                    unfinished.push(ServerToolCall {
                        id: Some(id.to_string()),
                        tool,
                        subject: subject.to_string(),
                        sources: Vec::new(),
                    });
                }
            }
            Some(kind) if is_server_result_type(kind) => {
                let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else { continue };
                let started = blocks.iter().find(|b| {
                    b.get("type").and_then(Value::as_str) == Some("server_tool_use")
                        && b.get("id").and_then(Value::as_str) == Some(id)
                });
                let intent = if let Some(started) = started {
                    server_tool_intent(started).map(|(_, tool, subject)| (tool, subject))
                } else {
                    pending.iter().find(|call| call.id.as_deref() == Some(id)).map(|call| (call.tool, call.subject.as_str()))
                };
                let Some((tool, subject)) = intent else { continue };
                let sources = result_sources(block);
                if sources.is_empty() { continue; }
                calls.push(ServerToolCall {
                    id: Some(id.to_string()),
                    tool,
                    subject: subject.to_string(),
                    sources,
                });
            }
            _ => {}
        }
    }
    (calls, unfinished)
}

/// The pages a `web_search_tool_result` / `web_fetch_tool_result` block returned.
fn result_sources(block: &Value) -> Vec<WebSource> {
    match block.get("type").and_then(Value::as_str) {
        Some("web_search_tool_result") => block
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|i| i.get("type").and_then(Value::as_str) == Some("web_search_result"))
            .filter_map(|item| {
                let url = item.get("url").and_then(Value::as_str)?;
                Some(WebSource {
                    title: item.get("title").and_then(Value::as_str).unwrap_or(url).to_string(),
                    url: url.to_string(),
                    snippet: String::new(),
                    score: None,
                    published: item.get("page_age").and_then(Value::as_str).map(str::to_string),
                })
            })
            .collect(),
        Some("web_fetch_tool_result") => {
            let content = block.get("content");
            let Some(url) = content.and_then(|c| c.get("url")).and_then(Value::as_str) else { return Vec::new() };
            let title = content.and_then(|c| c.pointer("/content/title")).and_then(Value::as_str).unwrap_or(url);
            vec![WebSource { title: title.to_string(), url: url.to_string(), snippet: String::new(), score: None, published: None }]
        }
        _ => Vec::new(),
    }
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
    let mut out = String::new();
    for s in sources.iter().take(MAX) {
        // Provider-supplied text is untrusted: only a screened http(s) URL becomes a
        // link, and the title is reduced to plain text (no markdown, HTML, control or
        // invisible characters; capped) so it cannot restyle the assistant's answer.
        let Some(url) = super::safe_url(&s.url) else { continue };
        let title = kyomi_types::web_text::plain_title(&s.title);
        let title = if title.is_empty() { url.to_string() } else { title };
        out.push_str(&format!("- [{title}]({url})\n"));
    }
    if out.is_empty() {
        return String::new();
    }
    format!("\n\n**Sources**\n{}", out.trim_end())
}

/// Append `footer` to an answer so it always starts a fresh block: if the answer
/// ends inside an open code fence (CommonMark: up to 3 spaces of indent, a run of 3+
/// backticks or tildes, closed by a run of the same character at least as long) the
/// fence is closed first.
pub fn append_footer(content: &str, footer: &str) -> String {
    if footer.is_empty() {
        return content.to_string();
    }
    let mut open: Option<(char, usize)> = None;
    for line in content.lines() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent > 3 {
            continue;
        }
        let rest = &line[indent..];
        let Some(marker) = rest.chars().next().filter(|c| *c == '`' || *c == '~') else { continue };
        let run = rest.chars().take_while(|c| *c == marker).count();
        if run < 3 {
            continue;
        }
        // A backtick opener whose info string contains a backtick is an inline code
        // span, not a fence (CommonMark).
        if marker == '`' && open.is_none() && rest[run..].contains('`') {
            continue;
        }
        match open {
            None => open = Some((marker, run)),
            Some((m, len)) if m == marker && run >= len && rest[run..].trim().is_empty() => open = None,
            Some(_) => {}
        }
    }
    match open {
        Some((marker, len)) => format!("{content}\n{}{footer}", marker.to_string().repeat(len)),
        None => format!("{content}{footer}"),
    }
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
        WebBackendConfig { kind, api_key: None, base_url: None, options: options.as_object().cloned().unwrap_or_default(), ..Default::default() }
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

    fn fixture(name: &str) -> Value {
        let path = format!("{}/src/web/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// LIVE capture (claude-haiku-4-5, web_search_20250305, 2026-11-16), id-redacted.
    /// Note the extra `caller` field on the result block: verbatim replay must
    /// not depend on knowing every field.
    #[test]
    fn anthropic_live_search_response_parses_and_replays_verbatim() {
        let raw = fixture("anthropic_search_message.json");
        let a = parse_anthropic(&raw);
        assert_eq!(a.provider_content.as_ref().unwrap(), raw["content"].as_array().unwrap());
        assert_eq!((a.searches, a.fetches), (1, 0));
        assert!(!a.sources.is_empty() && a.sources.iter().all(|s| s.url.starts_with("http")));
        assert!(raw["content"][1].get("caller").is_some(), "fixture must keep the live `caller` field");
        let enc = raw["content"][1]["content"][0]["encrypted_content"].as_str().unwrap();
        assert!(enc.len() > 100, "real opaque payload");
        // Replayed through the real adapter conversion, byte-for-byte.
        let msg = crate::types::Message::assistant("x").with_provider_content(a.provider_content);
        let (_, replayed) = crate::anthropic::AnthropicClient::convert_messages_to_anthropic(&[msg], &Default::default());
        assert_eq!(replayed[0]["content"], raw["content"]);
    }

    /// LIVE capture (web_fetch_20250910): fetch usage, source and document title.
    #[test]
    fn anthropic_live_fetch_response_parses() {
        let raw = fixture("anthropic_fetch_message.json");
        let a = parse_anthropic(&raw);
        assert_eq!((a.searches, a.fetches), (0, 1));
        assert!(a.provider_content.is_some());
        assert_eq!(a.sources[0].url, "https://www.rust-lang.org/");
        assert_eq!(a.sources[0].title, "Rust Programming Language");
    }

    /// LIVE capture: a real `pause_turn` (haiku, 10 sequential web_fetch calls, the
    /// server loop's iteration limit). The paused content ends in a
    /// `server_tool_use` with no result yet, exactly the shape the docs describe.
    #[test]
    fn anthropic_live_pause_turn_response_parses_and_replays_verbatim() {
        let raw = fixture("anthropic_pause_turn_message.json");
        assert_eq!(raw["stop_reason"], "pause_turn");
        let blocks = raw["content"].as_array().unwrap();
        assert_eq!(blocks.last().unwrap()["type"], "server_tool_use", "unresolved trailing server_tool_use");
        let a = parse_anthropic(&raw);
        assert_eq!(a.fetches, 10, "the loop stopped at its iteration limit");
        assert_eq!(a.provider_content.as_ref().unwrap(), blocks);
        let msg = crate::types::Message::assistant("x").with_provider_content(a.provider_content);
        let (_, replayed) = crate::anthropic::AnthropicClient::convert_messages_to_anthropic(&[msg], &Default::default());
        assert_eq!(replayed[0]["content"], raw["content"], "continuation sends the paused content unchanged");
    }

    /// KYO-979: a paused response reports the call it did not finish as pending;
    /// the live continuation carries that call's result without repeating the
    /// `server_tool_use`, so it cards only when parsed with the pending call.
    #[test]
    fn anthropic_live_pause_continuation_cards_the_pending_fetch() {
        let paused = fixture("anthropic_pause_turn_message.json");
        let continuation = fixture("anthropic_pause_turn_continuation_message.json");
        let paused = parse_anthropic(&paused);
        assert_eq!(paused.pending.len(), 1, "the trailing call has no result yet");
        let pending = &paused.pending[0];
        assert_eq!(pending.tool, WebToolKind::Fetch);
        assert_eq!(pending.subject, "https://httpbin.org/get?n=7");
        assert_eq!(pending.id.as_deref(), Some("srvtoolu_012xukXbd42Crxsb9Xav9PqW"));

        let alone = parse_anthropic(&continuation);
        assert_eq!(alone.calls.len(), 9, "without the paused call the orphan result cannot pair");
        let resumed = parse_anthropic_with_pending(&continuation, &paused.pending);
        assert_eq!(resumed.calls.len(), 10, "every result in the continuation is now a card");
        assert!(resumed.pending.is_empty(), "nothing is left pending");
        let orphan = resumed.calls.iter().find(|call| call.id.as_deref() == pending.id.as_deref()).expect("the pending call's card");
        assert_eq!(orphan.tool, WebToolKind::Fetch);
        assert_eq!(orphan.subject, pending.subject);
        assert_eq!(orphan.sources[0].url, "https://httpbin.org/get?n=7");
    }

    #[test]
    fn a_paused_search_result_keeps_the_query_from_its_paused_call() {
        let pending = ServerToolCall {
            id: Some("srvtoolu_search".into()),
            tool: WebToolKind::Search,
            subject: "latest stable rust".into(),
            sources: Vec::new(),
        };
        let raw = json!({
            "content": [{"type": "web_search_tool_result", "tool_use_id": "srvtoolu_search", "content": [
                {"type": "web_search_result", "url": "https://blog.rust-lang.org/", "title": "Rust Blog", "encrypted_content": "E", "page_age": null}
            ]}],
            "usage": {"server_tool_use": {"web_search_requests": 1}}
        });
        let a = parse_anthropic_with_pending(&raw, &[pending]);
        assert_eq!(a.calls.len(), 1);
        assert_eq!(a.calls[0].subject, "latest stable rust");
        assert_eq!(a.calls[0].sources[0].url, "https://blog.rust-lang.org/");
        assert!(a.pending.is_empty());
    }

    #[test]
    fn an_error_result_resolves_a_call_instead_of_leaving_it_pending() {
        let raw = json!({
            "content": [
                {"type": "server_tool_use", "id": "srvtoolu_2", "name": "web_fetch", "input": {"url": "https://c.example"}},
                {"type": "web_fetch_tool_result", "tool_use_id": "srvtoolu_2", "content": {"error_code": "url_not_accessible"}}
            ],
            "usage": {"server_tool_use": {"web_fetch_requests": 1}}
        });
        let a = parse_anthropic(&raw);
        assert!(a.calls.is_empty(), "an error result has nothing to show");
        assert!(a.pending.is_empty(), "the call finished, with an error");
        assert_eq!(a.errors, ["web_fetch: url_not_accessible"]);
    }

    #[test]
    fn per_request_allowance_never_exceeds_the_entry_and_applies_to_both_backends() {
        use kyomi_core::web_tools::WebBackendKind::{Anthropic as A, OpenRouter as O};
        let a = ServerTool { kind: WebToolKind::Search, backend: A, entry: anthropic_tool_entry(WebToolKind::Search, &cfg(A, json!({})), &caps()) };
        assert_eq!(a.with_max_uses(2)["max_uses"], 2);
        assert_eq!(a.with_max_uses(50)["max_uses"], 5, "never raised above the entry");
        let o = ServerTool { kind: WebToolKind::Fetch, backend: O, entry: openrouter_tool_entry(WebToolKind::Fetch, &cfg(O, json!({})), &caps()) };
        assert_eq!(o.with_max_uses(3)["parameters"]["max_uses"], 3);
    }

    #[test]
    fn openrouter_entries_use_the_chat_completions_server_tool_types() {
        use kyomi_core::web_tools::WebBackendKind::OpenRouter;
        let o = json!({"engine": "exa", "max_uses": 99, "allowed_domains": ["a.com"]});
        let e = openrouter_tool_entry(WebToolKind::Search, &cfg(OpenRouter, o), &caps());
        assert_eq!(e["type"], "openrouter:web_search");
        assert_eq!(e["parameters"], json!({"engine": "exa", "max_uses": 5, "allowed_domains": ["a.com"]}));
        let f = openrouter_tool_entry(WebToolKind::Fetch, &cfg(OpenRouter, json!({})), &caps());
        assert_eq!(f, json!({"type": "openrouter:web_fetch", "parameters": {"max_uses": 5}}));
    }

    /// LIVE capture (deepseek/deepseek-chat via OpenRouter, 2026-11-16), trimmed and id-redacted.
    #[test]
    fn openrouter_live_search_response_maps_to_sources_and_usage() {
        let a = parse_openrouter(&fixture("openrouter_search_chat_completion.json"));
        assert_eq!(a.searches, 1);
        assert_eq!(a.fetches, 0);
        assert!(a.provider_content.is_none(), "nothing to replay on Chat Completions");
        assert!(a.sources.iter().any(|s| s.url == "https://blog.rust-lang.org/releases/"));
        assert!(a.sources.iter().all(|s| s.snippet.is_empty() && !s.title.is_empty()));
    }

    /// LIVE capture: a fetch returns no annotations; its usage is `tool_calls_executed`.
    #[test]
    fn openrouter_live_fetch_response_counts_a_fetch_and_has_no_sources() {
        let a = parse_openrouter(&fixture("openrouter_fetch_chat_completion.json"));
        assert_eq!((a.searches, a.fetches), (0, 1));
        assert!(a.sources.is_empty());
    }

    #[test]
    fn footer_neutralises_titles_and_drops_unsafe_urls() {
        let src = |title: &str, url: &str| WebSource { title: title.into(), url: url.into(), snippet: String::new(), score: None, published: None };
        let f = sources_footer(&[
            src("Evil\n\n**SYSTEM:** obey [x](y) ![i](u) <img src=x onerror=1>", "https://a.example/ok"),
            src("Bad", "javascript:alert(1)"),
            src("Paren", "https://a.example/x)![img](https://evil.example/p.png"),
            src("Slash", "https://a.example/x\\"),
            src("Marks", "https://a.example/<b>"),
        ]);
        assert_eq!(f, "\n\n**Sources**\n- [Evil SYSTEM: obey xy iu img srcx onerror1](https://a.example/ok)");
        assert_eq!(sources_footer(&[src("t", "javascript:x")]), "");
    }

    #[test]
    fn footer_titles_are_capped_and_stripped_of_invisible_characters() {
        let src = |title: String| WebSource { title, url: "https://a.example/ok".into(), snippet: String::new(), score: None, published: None };
        let f = sources_footer(&[src(format!("{}{}", "a\u{202E}b\u{200B}", "x".repeat(500)))]);
        let title = f.split("- [").nth(1).unwrap().split("](").next().unwrap();
        assert_eq!(title.chars().count(), kyomi_types::web_text::MAX_TITLE_LEN);
        assert!(title.starts_with("abx") && !title.contains('\u{202E}') && !title.contains('\u{200B}'));
    }

    #[test]
    fn footer_always_starts_a_fresh_block_even_after_an_open_code_fence() {
        let footer = "\n\n**Sources**\n- [t](https://a.example/ok)";
        assert_eq!(append_footer("done", footer), format!("done{footer}"));
        assert_eq!(append_footer("```sql\nselect 1", footer), format!("```sql\nselect 1\n```{footer}"));
        assert_eq!(append_footer("```sql\nselect 1\n```", footer), format!("```sql\nselect 1\n```{footer}"));
        assert_eq!(append_footer("x", ""), "x");
        // Longer and tilde fences; a shorter or different marker does not close them.
        assert_eq!(append_footer("````md\n```\ninner", footer), format!("````md\n```\ninner\n````{footer}"));
        assert_eq!(append_footer("~~~\ncode", footer), format!("~~~\ncode\n~~~{footer}"));
        assert_eq!(append_footer("~~~\ncode\n```", footer), format!("~~~\ncode\n```\n~~~{footer}"));
        assert_eq!(append_footer("~~~\ncode\n~~~", footer), format!("~~~\ncode\n~~~{footer}"));
        assert_eq!(append_footer("```x``` inline code span, not a fence", footer), format!("```x``` inline code span, not a fence{footer}"));
        assert_eq!(append_footer("    ```\nindented code, not a fence", footer), format!("    ```\nindented code, not a fence{footer}"));
    }

    #[test]
    fn footer_lists_sources_as_links_and_is_empty_without_sources() {
        assert_eq!(sources_footer(&[]), "");
        let f = sources_footer(&parse_anthropic(&search_response()).sources);
        assert_eq!(f, "\n\n**Sources**\n- [A](https://a.example/x)");
    }
}
