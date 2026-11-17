// SPDX-License-Identifier: AGPL-3.0-or-later

//! `web_search` / `web_fetch` results render as normalised sources.

use super::super::*;
use serde_json::json;

fn render(schema: Value) -> String {
    render_tool_schema(schema).to_html()
}

#[test]
fn search_sources_render_as_external_links_with_host_and_date() {
    let html = render(json!({
        "tool": "web_search",
        "input": {"query": "rust release"},
        "output": {"available": true, "results": [
            {"title": "<untrusted_web_content>\nRust 1.99\n</untrusted_web_content>", "url": "https://blog.rust-lang.org/2026/x", "snippet": "<untrusted_web_content>x</untrusted_web_content>", "published": "2026-10-01"}
        ]}
    }));
    assert!(html.contains("rust release"), "{html}");
    assert!(html.contains("Rust 1.99"), "{html}");
    assert!(!html.contains("&lt;untrusted_web_content"), "fence markers are not shown to people: {html}");
    assert!(html.contains("href=\"https://blog.rust-lang.org/2026/x\""), "{html}");
    assert!(html.contains("noopener"), "{html}");
    assert!(html.contains("blog.rust-lang.org · 2026-10-01"), "{html}");
    assert!(!html.contains("untrusted_web_content"), "snippets stay out of the UI: {html}");
}

#[test]
fn unsafe_urls_from_web_content_are_never_links() {
    let html = render(json!({
        "tool": "web_search",
        "input": {"query": "q"},
        "output": {"results": [
            {"title": "Evil", "url": "javascript:alert(1)"},
            {"title": "Data", "url": "data:text/html,x"},
            {"title": "Rel", "url": "/etc/passwd"}
        ]}
    }));
    assert!(!html.contains("href=\"javascript"), "{html}");
    assert!(!html.contains("href=\"data:"), "{html}");
    assert!(!html.contains("href=\"/etc"), "{html}");
    assert!(html.contains("Evil"), "{html}");
}

#[test]
fn unavailable_result_explains_why_and_lists_no_sources() {
    let html = render(json!({
        "tool": "web_search",
        "input": {"query": "q"},
        "output": {"available": false, "reason": "this workspace's monthly web access limit has been reached"}
    }));
    assert!(html.contains("monthly web access limit"), "{html}");
    assert!(!html.contains("Sources:"), "{html}");
}

#[test]
fn fetch_renders_the_page_as_one_source() {
    let html = render(json!({
        "tool": "web_fetch",
        "input": {"url": "https://example.com/doc"},
        "output": {"available": true, "url": "https://example.com/doc", "title": "Docs", "content": "<untrusted_web_content>x</untrusted_web_content>"}
    }));
    assert!(html.contains("Fetching page:"), "{html}");
    assert!(html.contains("Docs"), "{html}");
    assert!(html.contains("href=\"https://example.com/doc\""), "{html}");
}

/// A provider-executed call (Anthropic / OpenRouter) is recorded with the same
/// payload shape as a Kyomi-executed one and shows as the same source card:
/// the query, then each page as a link with its host and age.
#[test]
fn a_provider_executed_search_shows_the_same_source_card() {
    let html = render(json!({
        "tool": "web_search",
        "input": {"query": "latest stable rust"},
        "output": {"tool": "web_search", "available": true, "query": "latest stable rust", "results": [
            {"title": "<untrusted_web_content>\nRust Versions\n</untrusted_web_content>", "url": "https://releases.rs/", "snippet": "<untrusted_web_content>\n\n</untrusted_web_content>", "published": "April 30, 2026"},
            {"title": "<untrusted_web_content>\nBlog\n</untrusted_web_content>", "url": "https://blog.rust-lang.org/", "snippet": "<untrusted_web_content>\n\n</untrusted_web_content>"}
        ]}
    }));
    assert!(html.contains("latest stable rust"), "{html}");
    assert!(html.contains("Sources:"), "{html}");
    assert!(html.contains("href=\"https://releases.rs/\""), "{html}");
    assert!(html.contains("releases.rs · April 30, 2026"), "{html}");
    assert!(html.contains("href=\"https://blog.rust-lang.org/\""), "{html}");
    assert!(!html.contains("untrusted_web_content"), "{html}");
}

/// OpenRouter reports citations for the response, not the query: the card still
/// lists the sources.
#[test]
fn a_provider_search_without_a_reported_query_still_lists_its_sources() {
    let html = render(json!({
        "tool": "web_search",
        "input": {"query": ""},
        "output": {"tool": "web_search", "available": true, "query": "", "results": [
            {"title": "Rust 1.99", "url": "https://blog.rust-lang.org/x", "snippet": ""}
        ]}
    }));
    assert!(html.contains("Sources:"), "{html}");
    assert!(html.contains("href=\"https://blog.rust-lang.org/x\""), "{html}");
}
