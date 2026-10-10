// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

/// Tokenize as HTML, including entity decoding, rather than treating escaped
/// attribute-like source text as a real DOM attribute.
fn assert_inert_html(html: &str) {
    use html5ever::tokenizer::{BufferQueue, Token, TokenSink, TokenSinkResult, Tokenizer};
    struct Sink;
    impl TokenSink for Sink {
        type Handle = ();
        fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
            if let Token::TagToken(tag) = token {
                assert!(!matches!(
                    tag.name.as_ref(),
                    "script" | "iframe" | "object" | "embed"
                ));
                for attr in tag.attrs {
                    let name = attr.name.local.as_ref();
                    assert!(!name.starts_with("on"), "event attribute: {attr:?}");
                    if name == "href" || name == "src" {
                        assert!(
                            attr.value.as_ref() == "cid:kyomi_logo"
                                || attr.value.starts_with("cid:chart_")
                                || kyomi_types::text::safe_markdown_url(&attr.value, name == "src"),
                            "unsafe destination after HTML parsing: {attr:?}"
                        );
                    }
                }
            }
            TokenSinkResult::Continue
        }
    }
    let input = BufferQueue::default();
    input.push_back(html.into());
    let tokenizer = Tokenizer::new(Sink, Default::default());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
}

#[test]
fn agent_markup_is_text_in_email_markdown_and_tables() {
    for input in [
        "<script>void(0)</script><img src=x onerror=\"void(0)\">",
        "`<img src=x onerror=\"void(0)\">`",
        "| Column |\n|---|\n| <img src=x onerror=\"void(0)\"> |",
    ] {
        let html = markdown_to_simple_html(input);
        assert_inert_html(&html);
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<img"), "{html}");
        assert!(html.contains("&lt;"), "{html}");
    }
}

#[test]
fn unsafe_email_links_do_not_become_destinations() {
    for url in [
        "javascript:alert",
        "JaVaScRiPt:alert",
        "data:text/html,payload",
        "vbscript:alert",
        "java\tscript:alert",
    ] {
        let html = markdown_to_simple_html(&format!("[label]({url})"));
        assert_inert_html(&html);
        assert_eq!(html, "<p>label</p>", "{url}");
    }
    let html = markdown_to_simple_html("[label](https://example.com/\" onclick=\"void(0))");
    assert_inert_html(&html);
    assert!(!html.contains("href="), "{html}");
    let html = markdown_to_simple_html("[label](javascript&#58;void)");
    assert_inert_html(&html);
    assert!(
        html.contains("javascript&amp;#58;void"),
        "entities must not decode twice: {html}"
    );
}

#[test]
fn trusted_chart_fragments_survive_untrusted_message_conversion() {
    let block = "```chartml\ncontent\n```";
    let message = format!("<img src=x onerror=\"void(0)\">\n\n{block}\n\n**after**");
    let start = message.find(block).unwrap();
    let spec = serde_json::json!({"visualize": {"type": "metric", "label": "<img src=x onerror=\"void(0)\">", "value": "value"}, "data": {"rows": [{"value": "<script>void(0)</script>"}]}});
    let metric = render_metric_email_html(&spec);
    let img = build_chart_img_html(
        "chart_light",
        Some("chart_dark"),
        "[caption](javascript:alert) **literal** <img src=x>",
    );
    let trusted = format!("{metric}{img}");
    let html = email_html_with_charts(
        &message,
        vec![(start..start + block.len(), trusted.clone())],
    );
    assert_inert_html(&html);
    assert!(
        html.contains(&trusted),
        "generated charts must remain byte-identical"
    );
    assert!(html.contains("src=\"cid:chart_light\""));
    assert!(html.contains("class=\"chart-dark\""));
    assert!(html.contains("<strong>after</strong>"));
    assert!(!html.contains("<script"));
    assert!(!html.contains("<img src=x"));
    assert!(!html.contains("href=\"javascript:"));
}

#[test]
fn email_template_escapes_agent_titles_and_watch_metadata() {
    let payload = "<img src=x onerror=\"void(0)\">";
    let (subject, html) = build_watch_alert_email(
        "recipient@example.com",
        payload,
        payload,
        "<p>safe message</p>",
        1,
        "https://app.example.com",
        Some(payload),
        WatchMode::Alert,
    );
    assert_inert_html(&html);
    assert!(subject.contains(payload), "subject is plain text");
    assert!(!html.contains(payload));
    assert!(html.contains("&lt;img src=x onerror=&quot;void(0)&quot;&gt;"));
    assert!(html.contains("<p>safe message</p>"));
}

#[tokio::test]
async fn email_processing_entry_point_escapes_agent_output() {
    let context = super::tests::dummy_query_ctx();
    let (html, images) = process_message_for_email(
        "<img src=x onerror=\"void(0)\"> [label](javascript:alert) **visible**",
        &context,
    )
    .await;
    assert_inert_html(&html);
    assert!(images.is_empty());
    assert!(!html.contains("<img"));
    assert!(!html.contains("href="));
    assert!(html.contains("<strong>visible</strong>"));
}

#[test]
fn generated_charts_follow_source_order_with_unicode_and_reversed_ranges() {
    let first = "```chartml\nfirst\n```";
    let second = "```chartml\nsecond\n```";
    let message = format!("雪 **before**\n{first}\n<script>void(0)</script>\n{second}\n**after**");
    let first_start = message.find(first).unwrap();
    let second_start = message.find(second).unwrap();
    let html = email_html_with_charts(
        &message,
        vec![
            (
                second_start..second_start + second.len(),
                "<p>second chart</p>".to_string(),
            ),
            (
                first_start..first_start + first.len(),
                "<p>first chart</p>".to_string(),
            ),
        ],
    );
    assert_inert_html(&html);
    assert!(html.contains("雪 <strong>before</strong>"));
    assert!(html.find("first chart").unwrap() < html.find("second chart").unwrap());
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("<strong>after</strong>"));
    assert!(!html.contains("chartml"));
}
