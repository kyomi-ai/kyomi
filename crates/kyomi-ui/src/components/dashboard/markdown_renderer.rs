// SPDX-License-Identifier: AGPL-3.0-or-later

//! Markdown + ChartML renderer component.
//!
//! Splits content into alternating Markdown, code-block, and ChartML segments.
//! Markdown is rendered via `pulldown-cmark`, code blocks get syntax-styled
//! `<pre><code>` with a copy button, and ChartML blocks are parsed, data is
//! fetched via the datasource server function, rendered through chartml-core,
//! and the resulting `ChartElement` tree is converted to Leptos SVG views.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_core::ChartML;
use chartml_datafusion::DataFusionTransform;
use chartml_leptos::{use_chartml_configured, ChartHeaderBar, ChartMLChart};
use leptos::prelude::*;

use crate::server_fns::datasources::query_datasource_arrow;

// ---------------------------------------------------------------------------
// Content segmentation
// ---------------------------------------------------------------------------

/// A segment of dashboard content.
#[derive(Clone, Debug, PartialEq)]
enum ContentSegment {
    /// Plain markdown text to be rendered as HTML.
    Markdown(String),
    /// ChartML YAML content extracted from a ```chartml fenced code block.
    ChartML {
        yaml: String,
        block_index: usize,
        array_index: usize,
    },
    /// Non-chartml fenced code block.
    CodeBlock { language: String, code: String },
    /// A `watch-response` code block containing JSON with `message` and `watch` keys.
    WatchResponse {
        message: String,
        watch: serde_json::Value,
    },
}

/// Clean up incomplete ChartML code blocks during streaming.
///
/// When streaming content, the last `\`\`\`chartml` fence may not yet have a
/// closing `\`\`\``. This causes partial YAML to be parsed, producing errors.
/// This function removes everything from the last unclosed `\`\`\`chartml`
/// fence to the end of the content.
///
/// Matches the React implementation in MarkdownRenderer.jsx (lines 578-593).
fn clean_streaming_markdown(content: &str) -> String {
    let last_chartml_start = content.rfind("```chartml");
    match last_chartml_start {
        Some(start) => {
            // Check if there's a closing ``` after the ```chartml opening
            let after_opening = &content[start + 10..]; // Skip past "```chartml"
            let has_closing = after_opening.contains("```");
            if has_closing {
                // Block is complete, return as-is
                content.to_string()
            } else {
                // Incomplete block — strip everything from the opening fence onward
                content[..start].to_string()
            }
        }
        None => content.to_string(),
    }
}

/// Parse content into alternating Markdown, ChartML, and CodeBlock segments.
///
/// Scans for fenced code blocks (```language). ChartML blocks are extracted
/// separately from other code blocks. Everything outside fenced blocks is
/// treated as Markdown.
fn parse_segments(content: &str) -> Vec<ContentSegment> {
    let mut segments = Vec::new();
    let mut remaining = content;
    let mut chartml_block_index: usize = 0;

    loop {
        // Find the next fenced code block opening (``` at start of line)
        match find_any_fence(remaining) {
            Some((fence_start, language)) => {
                // Everything before the fence is Markdown
                let before = &remaining[..fence_start];
                if !before.trim().is_empty() {
                    segments.push(ContentSegment::Markdown(before.to_string()));
                }

                // Skip past the opening fence line
                let after_open = &remaining[fence_start..];
                let fence_end = after_open
                    .find('\n')
                    .map(|i| i + 1)
                    .unwrap_or(after_open.len());
                let inner = &after_open[fence_end..];

                // Find the closing ```
                match find_closing_fence(inner) {
                    Some(close_start) => {
                        let block_content = inner[..close_start].trim();

                        if language == "chartml" {
                            if !block_content.is_empty() {
                                // Parse the YAML to check for arrays
                                let chart_count =
                                    count_chart_array_items(block_content);
                                if chart_count > 1 {
                                    for idx in 0..chart_count {
                                        segments.push(ContentSegment::ChartML {
                                            yaml: block_content.to_string(),
                                            block_index: chartml_block_index,
                                            array_index: idx,
                                        });
                                    }
                                } else {
                                    segments.push(ContentSegment::ChartML {
                                        yaml: block_content.to_string(),
                                        block_index: chartml_block_index,
                                        array_index: 0,
                                    });
                                }
                            }
                            chartml_block_index += 1;
                        } else if !block_content.is_empty() {
                            // Check for watch-response blocks.
                            // Matches React: lang === 'json:watch-response' or
                            // className includes 'watch-response' or
                            // (lang === 'json' && content includes '"watch"' and '"message"')
                            let is_watch_response =
                                language == "json:watch-response"
                                    || language.contains("watch-response")
                                    || (language == "json"
                                        && block_content.contains("\"watch\"")
                                        && block_content.contains("\"message\""));

                            if is_watch_response {
                                if let Some(seg) =
                                    try_parse_watch_response(block_content)
                                {
                                    segments.push(seg);
                                } else {
                                    // JSON parse failed — fall through to regular code block
                                    segments.push(ContentSegment::CodeBlock {
                                        language: language.to_string(),
                                        code: block_content.to_string(),
                                    });
                                }
                            } else {
                                segments.push(ContentSegment::CodeBlock {
                                    language: language.to_string(),
                                    code: block_content.to_string(),
                                });
                            }
                        }

                        // Skip past the closing ``` line
                        let after_close = &inner[close_start..];
                        let close_fence_end = after_close
                            .find('\n')
                            .map(|i| i + 1)
                            .unwrap_or(after_close.len());
                        remaining = &after_close[close_fence_end..];
                    }
                    None => {
                        // No closing fence — treat rest as the block
                        let block_content = inner.trim();
                        if language == "chartml" {
                            if !block_content.is_empty() {
                                segments.push(ContentSegment::ChartML {
                                    yaml: block_content.to_string(),
                                    block_index: chartml_block_index,
                                    array_index: 0,
                                });
                            }
                            // Not incrementing here since we break out
                        } else if !block_content.is_empty() {
                            segments.push(ContentSegment::CodeBlock {
                                language: language.to_string(),
                                code: block_content.to_string(),
                            });
                        }
                        break;
                    }
                }
            }
            None => {
                // No more code blocks — rest is Markdown
                if !remaining.trim().is_empty() {
                    segments.push(ContentSegment::Markdown(remaining.to_string()));
                }
                break;
            }
        }
    }

    segments
}

/// Find the byte offset and language of the next fenced code block opening.
/// Returns `(offset, language)` where language may be empty.
fn find_any_fence(text: &str) -> Option<(usize, &str)> {
    let pattern = "```";
    for (idx, _) in text.match_indices(pattern) {
        // Must be at start of text or preceded by a newline
        if idx == 0 || text.as_bytes().get(idx - 1) == Some(&b'\n') {
            let after = &text[idx + 3..];
            // Extract language identifier (everything until newline or end)
            let lang_end = after.find('\n').unwrap_or(after.len());
            let lang = after[..lang_end].trim();
            // Opening fence: has a language identifier
            if !lang.is_empty() {
                return Some((idx, lang));
            }
            // A bare ``` followed by newline with no language — could be an
            // opening fence for a no-language block. We only treat it as an
            // opening fence if the content after it contains a closing fence.
            if lang_end < after.len() {
                let inner = &after[lang_end + 1..];
                if find_closing_fence(inner).is_some() {
                    return Some((idx, ""));
                }
            }
        }
    }
    None
}

/// Find the byte offset of a closing ``` fence.
fn find_closing_fence(text: &str) -> Option<usize> {
    let pattern = "```";
    for (idx, _) in text.match_indices(pattern) {
        if idx == 0 || text.as_bytes().get(idx - 1) == Some(&b'\n') {
            let after = idx + 3;
            match text.as_bytes().get(after) {
                None | Some(b'\n') | Some(b'\r') | Some(b' ') => return Some(idx),
                _ => continue,
            }
        }
    }
    None
}

/// Count how many chart-type items are in a YAML array block.
fn count_chart_array_items(yaml: &str) -> usize {
    match serde_yaml::from_str::<serde_json::Value>(yaml) {
        Ok(serde_json::Value::Array(arr)) => {
            arr.iter()
                .filter(|v| {
                    v.as_object()
                        .and_then(|o| o.get("type"))
                        .and_then(|t| t.as_str())
                        == Some("chart")
                })
                .count()
                .max(1)
        }
        _ => 1,
    }
}

/// Try to parse a JSON string as a watch-response block.
///
/// Returns `Some(ContentSegment::WatchResponse { .. })` if the JSON has the
/// expected `message` and `watch` keys, or `None` if parsing fails or the
/// structure doesn't match.
fn try_parse_watch_response(json_str: &str) -> Option<ContentSegment> {
    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let obj = parsed.as_object()?;
    if !obj.contains_key("message") || !obj.contains_key("watch") {
        return None;
    }
    let message = obj
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let watch = obj.get("watch").cloned().unwrap_or(serde_json::Value::Null);
    Some(ContentSegment::WatchResponse { message, watch })
}

// ---------------------------------------------------------------------------
// Markdown → HTML
// ---------------------------------------------------------------------------

/// Convert a markdown string to HTML using pulldown-cmark with GFM extensions.
fn markdown_to_html(markdown: &str) -> String {
    use pulldown_cmark::Options;
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let parser = pulldown_cmark::Parser::new_ext(markdown, options);
    let mut html_output = String::new();
    pulldown_cmark::html::push_html(&mut html_output, parser);
    html_output
}

// ---------------------------------------------------------------------------
// Parameter substitution
// ---------------------------------------------------------------------------

/// Replace `{{param_id}}` placeholders in SQL with parameter values.
fn substitute_params(sql: &str, params: &HashMap<String, String>) -> String {
    let mut result = sql.to_string();
    for (key, value) in params {
        result = result.replace(&format!("{{{{{key}}}}}"), value);
    }
    result
}

// ---------------------------------------------------------------------------
// ChartML spec extraction helpers
// ---------------------------------------------------------------------------

/// Extract the chart title from a parsed YAML spec.
fn extract_title(spec: &serde_json::Value) -> Option<String> {
    spec.get("title")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract the datasource slug from a parsed YAML spec.
fn extract_datasource(spec: &serde_json::Value) -> Option<String> {
    spec.get("data")
        .and_then(|d| d.get("datasource").or_else(|| d.get("endpoint")))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract the SQL query from a parsed YAML spec.
fn extract_query(spec: &serde_json::Value) -> Option<String> {
    spec.get("data")
        .and_then(|d| d.get("query").or_else(|| d.get("url")))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract the chart type from a parsed YAML spec (e.g. "bar", "line", "pie").
fn extract_chart_type(spec: &serde_json::Value) -> Option<String> {
    spec.get("visualize")
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract the chart orientation from a parsed YAML spec (e.g. "horizontal").
fn extract_chart_orientation(spec: &serde_json::Value) -> Option<String> {
    spec.get("visualize")
        .and_then(|v| v.get("orientation"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract the chart mode from a parsed YAML spec (e.g. "stacked", "grouped").
fn extract_chart_mode(spec: &serde_json::Value) -> Option<String> {
    spec.get("visualize")
        .and_then(|v| v.get("mode"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

// ---------------------------------------------------------------------------
// Kyomi chart palettes — must match apps/frontend/src/config/chartPalettes.js
// ---------------------------------------------------------------------------

fn kyomi_palette(name: &str) -> Vec<String> {
    match name {
        "vibrant" => vec![
            "#1E88C7", "#D92849", "#28C75A", "#E8B733", "#28C7A8", "#E87333",
            "#3355D9", "#A8D928", "#C728A8", "#D97328", "#28A8D9", "#73A828",
        ],
        "accessible" => vec![
            "#2D5F7A", "#A83D52", "#3D7A52", "#C9A642", "#3D8A8A", "#E89970",
            "#5C6D99", "#B8D96B", "#996B8A", "#B87752", "#85B8D9", "#85996B",
        ],
        // "balanced" (default)
        _ => vec![
            "#1A75C9", "#B8405A", "#3D8A5A", "#D9952D", "#2D7A8A", "#C9734D",
            "#4D5A8A", "#99C94D", "#8A5A7A", "#D9B370", "#70B8D9", "#6B8A4D",
        ],
    }.into_iter().map(String::from).collect()
}

// ---------------------------------------------------------------------------
// ChartML instance factory
// ---------------------------------------------------------------------------

/// Create a configured ChartML instance with all chart renderers registered
/// and the user's palette preference applied.
fn configured_chartml(palette_name: &str) -> Arc<ChartML> {
    let colors = kyomi_palette(palette_name);
    use_chartml_configured(|c| {
        c.register_renderer("bar", CartesianRenderer::new());
        c.register_renderer("line", CartesianRenderer::new());
        c.register_renderer("area", CartesianRenderer::new());
        c.register_renderer("pie", PieRenderer::new());
        c.register_renderer("donut", PieRenderer::new());
        c.register_renderer("scatter", ScatterRenderer::new());
        c.register_renderer("metric", MetricRenderer::new());
        c.register_transform(DataFusionTransform);
        c.set_default_palette(colors.clone());
    })
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

/// Renders a non-chartml fenced code block with language badge and copy button.
#[component]
fn CodeBlockView(
    #[prop(into)] language: String,
    #[prop(into)] code: String,
) -> impl IntoView {
    let code_for_copy = code.clone();
    let (copied, set_copied) = signal(false);

    let on_copy = move |_| {
        let text = code_for_copy.clone();
        leptos::task::spawn_local(async move {
            if let Some(window) = web_sys::window() {
                let clipboard = window.navigator().clipboard();
                let promise = clipboard.write_text(&text);
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                set_copied.set(true);
                gloo_timers::future::TimeoutFuture::new(2000).await;
                set_copied.set(false);
            }
        });
    };

    let lang_display = language.clone();

    view! {
        <div class="relative group my-4">
            // Language badge
            <div class="absolute top-2 left-3 z-10">
                <span class="text-xs font-mono text-muted-foreground/70 select-none">
                    {lang_display}
                </span>
            </div>
            // Copy button
            <button
                on:click=on_copy
                class="absolute top-2 right-2 p-1.5 rounded bg-accent hover:bg-accent/80 opacity-0 group-hover:opacity-100 transition-opacity z-10"
                title=move || if copied.get() { "Copied!" } else { "Copy code" }
            >
                {move || {
                    if copied.get() {
                        view! {
                            <svg class="h-4 w-4 text-green-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
                            </svg>
                        }.into_any()
                    } else {
                        view! {
                            <svg class="h-4 w-4 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 5H6a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2v-1M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 012-2h2a2 2 0 012 2m0 0h2a2 2 0 012 2v3m2 4H10m0 0l3-3m-3 3l3 3" />
                            </svg>
                        }.into_any()
                    }
                }}
            </button>
            // Code content
            <pre class="rounded-md bg-muted p-4 pt-8 overflow-x-auto text-sm">
                <code class="font-mono text-foreground">
                    {code}
                </code>
            </pre>
        </div>
    }
}

/// Renders a watch-response block: the message as markdown + a watch preview card.
///
/// Matches the React WatchPreviewCard component in `watches/WatchPreviewCard.jsx`.
/// The card shows watch name, monitoring instruction, schedule, queries, and an
/// approve/accepted button.
#[component]
fn WatchPreviewCardView(
    /// The message text to render as markdown above the card.
    #[prop(into)]
    message: String,
    /// The watch JSON object with name, prompt, schedule, queries, etc.
    #[prop(into)]
    watch: serde_json::Value,
    /// Callback when the user clicks "Accept" — receives `(watch_data, card_id)`.
    on_watch_approved: Option<Callback<(serde_json::Value, String)>>,
    /// Whether this card has already been accepted.
    is_accepted: bool,
) -> impl IntoView {
    let message_html = if message.is_empty() {
        String::new()
    } else {
        markdown_to_html(&message)
    };

    let watch_name = watch
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled Watch")
        .to_string();
    let watch_prompt = watch
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let watch_schedule = watch
        .get("schedule")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let watch_id = watch
        .get("watch_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let watch_mode = watch
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("alert")
        .to_string();
    let queries = watch
        .get("queries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let is_update = watch_id.is_some();

    let (is_creating, set_is_creating) = signal(false);
    let (created, set_created) = signal(is_accepted);

    let watch_for_cb = watch.clone();
    let approve_cb = StoredValue::new(on_watch_approved);

    // Construct the "card_id" to pass back on approval — matches React's cardId format
    let card_id_for_approve = StoredValue::new(String::new()); // set below by parent via message_id

    let handle_approve = move |_: leptos::ev::MouseEvent| {
        if created.get_untracked() || is_creating.get_untracked() {
            return;
        }
        set_is_creating.set(true);
        if let Some(cb) = approve_cb.get_value() {
            cb.run((watch_for_cb.clone(), card_id_for_approve.get_value()));
            set_created.set(true);
        }
        set_is_creating.set(false);
    };

    let mode_badge = if watch_mode == "report" {
        view! {
            <span class="inline-flex items-center gap-1 rounded-full bg-primary/10 px-2 py-0.5 text-xs font-medium text-primary">
                // ChartBarIcon equivalent
                <svg class="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6" />
                </svg>
                "Report"
            </span>
        }
        .into_any()
    } else {
        view! {
            <span class="inline-flex items-center gap-1 rounded-full bg-warning px-2 py-0.5 text-xs font-medium text-warning-foreground">
                // Bell icon
                <svg class="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 17h5l-1.405-1.405A2.032 2.032 0 0118 14.158V11a6.002 6.002 0 00-4-5.659V5a2 2 0 10-4 0v.341C7.67 6.165 6 8.388 6 11v3.159c0 .538-.214 1.055-.595 1.436L4 17h5m6 0v1a3 3 0 11-6 0v-1m6 0H9" />
                </svg>
                "Alert"
            </span>
        }
        .into_any()
    };

    let action_label = if is_update { "Update" } else { "New Watch" };

    let queries_views: Vec<AnyView> = queries
        .iter()
        .map(|q| {
            let comment = q
                .get("comment")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sql = q
                .get("sql")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let datasource = q
                .get("datasource")
                .and_then(|v| v.as_str())
                .map(String::from);

            view! {
                <div class="flex items-start gap-2 p-2 rounded bg-muted border border-border">
                    <span class="text-muted-foreground mt-0.5 shrink-0 text-xs w-4">"\u{2699}\u{fe0f}"</span>
                    <div class="flex-1 min-w-0">
                        <p class="text-xs font-medium text-foreground break-words">{comment}</p>
                        <p class="text-xs text-muted-foreground font-mono mt-1 truncate">{sql}</p>
                        {datasource.map(|ds| {
                            view! {
                                <div class="mt-1">
                                    <span class="inline-block px-1.5 py-0.5 rounded text-xs bg-accent text-foreground">
                                        {ds}
                                    </span>
                                </div>
                            }
                        })}
                    </div>
                </div>
            }
            .into_any()
        })
        .collect();

    let has_queries = !queries.is_empty();
    let queries_count = queries.len();

    view! {
        <div class="watch-response not-prose" style="font-family: var(--font-sans, ui-sans-serif, system-ui, sans-serif); white-space: normal;">
            // Render the message as markdown
            {(!message_html.is_empty()).then(|| {
                view! {
                    <div inner_html=message_html.clone()></div>
                }
            })}
            // Watch preview card — matches React Card structure
            <div class="border border-primary/30 bg-primary/5 rounded-lg my-3">
                // Card header
                <div class="px-6 pt-6 pb-2">
                    <div class="flex items-center justify-between">
                        <div class="flex items-center gap-2">
                            // Eye icon
                            <svg class="h-4 w-4 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                            </svg>
                            <h3 class="text-base font-semibold">"Watch Preview"</h3>
                        </div>
                        <div class="flex items-center gap-2">
                            {mode_badge}
                            <span class="inline-flex items-center rounded-full bg-muted px-2 py-0.5 text-xs font-medium text-muted-foreground">
                                {action_label}
                            </span>
                        </div>
                    </div>
                </div>
                // Card content
                <div class="px-6 pb-6 space-y-3">
                    // Name
                    <div>
                        <p class="text-xs font-medium text-muted-foreground uppercase tracking-wider">"Name"</p>
                        <p class="font-medium">{watch_name}</p>
                    </div>
                    // Monitoring instruction
                    {(!watch_prompt.is_empty()).then(|| {
                        view! {
                            <div>
                                <p class="text-xs font-medium text-muted-foreground uppercase tracking-wider">"Monitoring"</p>
                                <p class="text-sm text-foreground whitespace-pre-wrap">{watch_prompt.clone()}</p>
                            </div>
                        }
                    })}
                    // Queries
                    {has_queries.then(|| {
                        view! {
                            <div>
                                <p class="text-xs font-medium text-muted-foreground uppercase tracking-wider mb-2">
                                    {format!("Reference Queries ({})", queries_count)}
                                </p>
                                <div class="space-y-2 max-h-40 overflow-y-auto">
                                    {queries_views}
                                </div>
                            </div>
                        }
                    })}
                    // Schedule
                    {(!watch_schedule.is_empty()).then(|| {
                        view! {
                            <div class="flex items-center gap-2 text-sm">
                                // Clock icon
                                <svg class="h-4 w-4 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
                                </svg>
                                <span>{watch_schedule.clone()}</span>
                            </div>
                        }
                    })}
                    // Approve button
                    <div class="pt-2 border-t border-border">
                        <button
                            on:click=handle_approve
                            disabled=move || is_creating.get() || created.get()
                            class=move || {
                                if created.get() {
                                    "w-full inline-flex items-center justify-center rounded-md px-4 py-2 text-sm font-medium bg-muted text-muted-foreground cursor-not-allowed"
                                } else {
                                    "w-full inline-flex items-center justify-center rounded-md px-4 py-2 text-sm font-medium bg-primary text-primary-foreground hover:bg-primary/90 transition-colors"
                                }
                            }
                        >
                            {move || {
                                if is_creating.get() {
                                    view! {
                                        <span class="flex items-center gap-2">
                                            <svg class="animate-spin h-4 w-4" fill="none" viewBox="0 0 24 24">
                                                <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
                                                <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
                                            </svg>
                                            "Accepting..."
                                        </span>
                                    }.into_any()
                                } else if created.get() {
                                    view! {
                                        <span class="flex items-center gap-2">
                                            // CheckCircle icon
                                            <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z" />
                                            </svg>
                                            "Accepted"
                                        </span>
                                    }.into_any()
                                } else {
                                    view! {
                                        <span class="flex items-center gap-2">
                                            <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z" />
                                            </svg>
                                            "Accept"
                                        </span>
                                    }.into_any()
                                }
                            }}
                        </button>
                        <Show when=move || !created.get()>
                            <p class="text-xs text-center text-muted-foreground mt-2">
                                "Or continue chatting to refine"
                            </p>
                        </Show>
                    </div>
                </div>
            </div>
        </div>
    }
}

/// Apply type/orientation/mode overrides to a parsed ChartML spec.
///
/// Matches React's `ChartWithChrome` useMemo logic:
/// - Applies `type_override` to `visualize.type`
/// - Applies `orientation_override` (Some = set, None = remove)
/// - Applies `mode_override` (Some = set, None = remove)
/// - When type changes, strips per-row `mark` overrides so the new type applies uniformly
/// - When switching away from bar, removes incompatible `orientation`
/// - When switching away from bar/area, removes incompatible `mode`
fn apply_spec_overrides(
    yaml: &str,
    type_override: Option<&str>,
    orientation_override: Option<Option<&str>>,
    mode_override: Option<Option<&str>>,
) -> String {
    let mut spec: serde_json::Value = match serde_yaml::from_str(yaml) {
        Ok(v) => v,
        Err(_) => return yaml.to_string(),
    };

    let viz = match spec.get_mut("visualize").and_then(|v| v.as_object_mut()) {
        Some(v) => v,
        None => return yaml.to_string(),
    };

    let original_type = viz
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Apply type override
    let effective_type = if let Some(t) = type_override {
        viz.insert("type".to_string(), serde_json::Value::String(t.to_string()));
        t.to_string()
    } else {
        original_type.clone()
    };

    // Apply orientation override (Some(Some("horizontal")) = set, Some(None) = remove)
    if let Some(orient_val) = orientation_override {
        if let Some(o) = orient_val {
            viz.insert(
                "orientation".to_string(),
                serde_json::Value::String(o.to_string()),
            );
        } else {
            viz.remove("orientation");
        }
    }

    // Apply mode override (Some(Some("stacked")) = set, Some(None) = remove)
    if let Some(mode_val) = mode_override {
        if let Some(m) = mode_val {
            viz.insert("mode".to_string(), serde_json::Value::String(m.to_string()));
        } else {
            viz.remove("mode");
        }
    }

    // If type changed, clean up incompatible properties
    if type_override.is_some() && effective_type != original_type {
        // Remove orientation for non-bar charts
        if effective_type != "bar" {
            viz.remove("orientation");
        }
        // Remove mode for non-bar/area charts
        if effective_type != "bar" && effective_type != "area" {
            viz.remove("mode");
        }
        // Strip per-row mark overrides so new type applies uniformly
        if let Some(arr) = viz.get_mut("rows").and_then(|r| r.as_array_mut()) {
            for item in arr.iter_mut() {
                if let Some(obj) = item.as_object_mut() {
                    obj.remove("mark");
                }
            }
        }
    }

    // Re-serialize to YAML
    serde_yaml::to_string(&spec).unwrap_or_else(|_| yaml.to_string())
}

/// Renders a single ChartML chart with chrome (header bar, type/orientation/mode
/// overrides, refresh, and action menu).
///
/// Uses `chartml_leptos::ChartHeaderBar` (native Rust/Tailwind) instead of the
/// JS `<chart-header-bar>` web component. Type/orientation/mode overrides are
/// applied as reactive signals that derive an effective YAML spec.
///
/// For **remote datasource** charts (with `data.datasource` + `data.query`),
/// fetches data via `query_datasource_arrow`, injects it into a per-render
/// `ChartML` instance, and renders via `ChartMLChart`. For **inline data**
/// charts, passes through directly to `ChartMLChart`.
#[component]
fn ChartBlock(
    #[prop(into)] yaml: String,
    block_index: usize,
    array_index: usize,
    #[prop(into)] parameters: Signal<HashMap<String, String>>,
    /// Edit callback — passed through from MarkdownRenderer (already Option).
    on_edit_chart: Option<Callback<(usize, usize)>>,
    /// Delete callback — passed through from MarkdownRenderer (already Option).
    on_delete_chart: Option<Callback<(usize, usize)>>,
    /// Save-to-dashboard callback.
    on_save_to_dashboard: Option<Callback<String>>,
    /// Chart info callback.
    on_chart_info: Option<Callback<String>>,
    /// "Ask about this chart" callback — receives chart YAML wrapped in code fence.
    on_ask_about_chart: Option<Callback<String>>,
    /// User's chart palette preference name.
    #[prop(into, optional)]
    chart_palette: Option<String>,
) -> impl IntoView {
    let yaml_owned = yaml.clone();
    let yaml_for_info = yaml.clone();
    let yaml_for_save = yaml.clone();

    // Parse the spec to extract metadata
    let parsed_spec: Option<serde_json::Value> =
        serde_yaml::from_str(&yaml_owned).ok();

    let chart_title = parsed_spec.as_ref().and_then(extract_title);
    let initial_chart_type = parsed_spec.as_ref().and_then(extract_chart_type);
    let initial_orientation = parsed_spec.as_ref().and_then(extract_chart_orientation);
    let initial_mode = parsed_spec.as_ref().and_then(extract_chart_mode);

    let datasource_slug = parsed_spec.as_ref().and_then(extract_datasource);
    let sql_query = parsed_spec.as_ref().and_then(extract_query);
    let is_remote = datasource_slug.is_some() && sql_query.is_some();

    // -- Override signals (matches React's ChartWithChrome state) --
    let (type_override, set_type_override) = signal(None::<String>);
    let (orientation_override, set_orientation_override) = signal(None::<Option<String>>);
    let (mode_override, set_mode_override) = signal(None::<Option<String>>);

    let (refresh_count, set_refresh_count) = signal(0_u32);
    let (last_refreshed, set_last_refreshed) = signal(None::<f64>);
    let (is_refreshing, set_is_refreshing) = signal(false);

    // Derive current chart type/orientation/mode for the header bar display
    let initial_type_stored = StoredValue::new(initial_chart_type.clone());
    let initial_orient_stored = StoredValue::new(initial_orientation.clone());
    let initial_mode_stored = StoredValue::new(initial_mode.clone());

    let current_chart_type = Memo::new(move |_| {
        type_override.get().or_else(|| initial_type_stored.get_value())
    });
    let current_orientation = Memo::new(move |_| {
        match orientation_override.get() {
            Some(o) => o, // Some(Some("horizontal")) or Some(None)
            None => initial_orient_stored.get_value(), // No override
        }
    });
    let current_mode = Memo::new(move |_| {
        match mode_override.get() {
            Some(m) => m, // Some(Some("stacked")) or Some(None)
            None => initial_mode_stored.get_value(), // No override
        }
    });

    // Derive the effective YAML spec with overrides applied
    let yaml_for_spec = yaml_owned.clone();
    let effective_spec = Memo::new(move |_| {
        let t_ovr = type_override.get();
        let o_ovr = orientation_override.get();
        let m_ovr = mode_override.get();

        // No overrides — return original YAML unchanged
        if t_ovr.is_none() && o_ovr.is_none() && m_ovr.is_none() {
            return yaml_for_spec.clone();
        }

        let result = apply_spec_overrides(
            &yaml_for_spec,
            t_ovr.as_deref(),
            o_ovr.as_ref().map(|o| o.as_deref()),
            m_ovr.as_ref().map(|m| m.as_deref()),
        );

        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(&format!(
            "[ChartBlock] spec override: type={:?} orient={:?} mode={:?}\nfull_result={}",
            t_ovr, o_ovr, m_ovr, &result
        ).into());

        result
    });

    // Create the ChartML instance
    let palette = chart_palette.unwrap_or_else(|| "balanced".to_string());
    let chartml = configured_chartml(&palette);

    // -- Remote data path: fetch data, register on ChartML instance, render via ChartMLChart --
    // We store fetched remote data as a signal. When data arrives, we create a new ChartML
    // instance with the data registered as a named source ("_remote") and rewrite the YAML
    // `data:` section to reference it. This lets ChartMLChart handle everything uniformly.
    let (remote_chartml, set_remote_chartml) = signal(None::<Arc<ChartML>>);
    let (remote_error, set_remote_error) = signal(None::<String>);
    let (remote_loading, set_remote_loading) = signal(is_remote);

    if is_remote {
        let ds_slug = datasource_slug.clone();
        let sql = sql_query.clone();
        let palette_for_remote = palette.clone();

        Effect::new(move || {
            let params = parameters.get();
            let _refresh = refresh_count.get();
            let ds = ds_slug.clone();
            let q = sql.clone();
            let pal = palette_for_remote.clone();

            set_remote_loading.set(true);
            set_remote_error.set(None);
            set_is_refreshing.set(true);

            leptos::task::spawn_local(async move {
                let slug = ds.unwrap();
                let query = q.unwrap();
                let resolved_sql = substitute_params(&query, &params);

                match query_datasource_arrow(slug, resolved_sql, None).await {
                    Ok(query_result) => {
                        use base64::Engine;
                        let ipc_bytes = match base64::engine::general_purpose::STANDARD
                            .decode(&query_result.ipc_base64)
                        {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                set_remote_error.set(Some(format!("Base64 decode error: {e}")));
                                set_remote_loading.set(false);
                                set_is_refreshing.set(false);
                                return;
                            }
                        };

                        let data_table =
                            match chartml_core::data::DataTable::from_ipc_bytes(&ipc_bytes) {
                                Ok(dt) => dt,
                                Err(e) => {
                                    set_remote_error
                                        .set(Some(format!("Arrow decode error: {e}")));
                                    set_remote_loading.set(false);
                                    set_is_refreshing.set(false);
                                    return;
                                }
                            };

                        // Create a new ChartML instance with the fetched data registered
                        let instance = configured_chartml(&pal);
                        // Safety: Arc::get_mut works here because we just created it and
                        // hold the only reference. We need to register the source before
                        // wrapping it.
                        let mut chartml_mut = ChartML::new();
                        // Re-register all renderers + transform + palette on the mutable instance
                        let colors = kyomi_palette(&pal);
                        chartml_mut.register_renderer("bar", CartesianRenderer::new());
                        chartml_mut.register_renderer("line", CartesianRenderer::new());
                        chartml_mut.register_renderer("area", CartesianRenderer::new());
                        chartml_mut.register_renderer("pie", PieRenderer::new());
                        chartml_mut.register_renderer("donut", PieRenderer::new());
                        chartml_mut.register_renderer("scatter", ScatterRenderer::new());
                        chartml_mut.register_renderer("metric", MetricRenderer::new());
                        chartml_mut.register_transform(DataFusionTransform);
                        chartml_mut.set_default_palette(colors);
                        chartml_mut.register_source("_remote", data_table);
                        let _ = instance; // drop unused Arc

                        set_remote_chartml.set(Some(Arc::new(chartml_mut)));
                        let now_ms = js_sys::Date::now();
                        set_last_refreshed.set(Some(now_ms));
                        set_remote_loading.set(false);
                        set_is_refreshing.set(false);
                    }
                    Err(e) => {
                        set_remote_error.set(Some(format!("Query error: {e}")));
                        set_remote_loading.set(false);
                        set_is_refreshing.set(false);
                    }
                }
            });
        });
    }

    // Derive the spec signal for ChartMLChart, rewriting data ref for remote charts
    let effective_spec_for_chartml = Memo::new(move |_| {
        let base_spec = effective_spec.get();

        if !is_remote {
            return base_spec;
        }

        // Rewrite the data section to reference the named "_remote" source
        // so ChartMLChart resolves it from the registered sources
        match serde_yaml::from_str::<serde_json::Value>(&base_spec) {
            Ok(mut val) => {
                if let Some(obj) = val.as_object_mut() {
                    obj.insert(
                        "data".to_string(),
                        serde_json::Value::String("_remote".to_string()),
                    );
                }
                serde_yaml::to_string(&val).unwrap_or(base_spec)
            }
            Err(_) => base_spec,
        }
    });

    // Determine which callbacks are available
    let has_edit = on_edit_chart.is_some();
    let has_delete = on_delete_chart.is_some();
    let has_save = on_save_to_dashboard.is_some();
    let has_info = on_chart_info.is_some();

    // Store callbacks for use in closures
    let edit_cb = StoredValue::new(on_edit_chart);
    let delete_cb = StoredValue::new(on_delete_chart);
    let save_cb = StoredValue::new(on_save_to_dashboard);
    let info_cb = StoredValue::new(on_chart_info);
    let _ask_cb = StoredValue::new(on_ask_about_chart);
    let yaml_for_save_stored = StoredValue::new(yaml_for_save);
    let yaml_for_info_stored = StoredValue::new(yaml_for_info);

    // Build typed callbacks for ChartHeaderBar
    let on_type_change_cb = Callback::new(move |t: String| {
        set_type_override.set(Some(t));
    });
    let on_orientation_change_cb = Callback::new(move |o: Option<String>| {
        set_orientation_override.set(Some(o));
    });
    let on_mode_change_cb = Callback::new(move |m: Option<String>| {
        set_mode_override.set(Some(m));
    });
    let on_refresh_cb = Callback::new(move |()| {
        set_refresh_count.update(|c| *c += 1);
    });

    // Build typed callbacks for the header bar's action buttons.
    // Always create a callback (the header bar respects show_* flags for visibility).
    let on_edit_cb = {
        let bi = block_index;
        let ai = array_index;
        Callback::new(move |()| {
            if let Some(cb) = edit_cb.get_value() {
                cb.run((bi, ai));
            }
        })
    };
    let on_delete_cb = {
        let bi = block_index;
        let ai = array_index;
        Callback::new(move |()| {
            if let Some(cb) = delete_cb.get_value() {
                cb.run((bi, ai));
            }
        })
    };
    let on_save_cb = Callback::new(move |()| {
        if let Some(cb) = save_cb.get_value() {
            let yaml = yaml_for_save_stored.get_value();
            let chart_md = format!("```chartml\n{}\n```", yaml);
            cb.run(chart_md);
        }
    });
    let on_info_cb = Callback::new(move |()| {
        if let Some(cb) = info_cb.get_value() {
            let yaml = yaml_for_info_stored.get_value();
            cb.run(yaml);
        }
    });

    // Spec signal for ChartMLChart
    let spec_signal = Signal::derive(move || effective_spec_for_chartml.get());

    // ChartML instance signal — for remote charts, uses the instance with data registered;
    // for inline charts, uses the static configured instance.
    let chartml_for_inline = chartml.clone();

    // Store callbacks as StoredValues for use inside the reactive header closure.
    let on_type_change_stored = StoredValue::new(on_type_change_cb);
    let on_orientation_change_stored = StoredValue::new(on_orientation_change_cb);
    let on_mode_change_stored = StoredValue::new(on_mode_change_cb);
    let on_refresh_stored = StoredValue::new(on_refresh_cb);
    let on_edit_stored = StoredValue::new(on_edit_cb);
    let on_delete_stored = StoredValue::new(on_delete_cb);
    let on_save_stored = StoredValue::new(on_save_cb);
    let on_info_stored = StoredValue::new(on_info_cb);

    view! {
        <div class="my-2">
            // Native Rust ChartHeaderBar — re-renders when type/orientation/mode change
            {move || {
                let ct = current_chart_type.get();
                let co = current_orientation.get();
                let cm = current_mode.get();

                // Build the header bar view. ChartHeaderBar uses #[prop(optional, into)]
                // for string props (expects Into<String>, wraps in Some automatically)
                // and #[prop(optional)] for callbacks (expects bare Callback<T>).
                // We conditionally pass props only when values are present.
                let type_cb = on_type_change_stored.get_value();
                let orient_cb = on_orientation_change_stored.get_value();
                let mode_cb = on_mode_change_stored.get_value();
                let refresh_cb = on_refresh_stored.get_value();
                let edit_action = on_edit_stored.get_value();
                let delete_action = on_delete_stored.get_value();
                let save_action = on_save_stored.get_value();
                let info_action = on_info_stored.get_value();
                let last_sig = Signal::derive(move || last_refreshed.get());
                let refreshing_sig = Signal::derive(move || is_refreshing.get());
                let title_val = chart_title.clone();

                view! {
                    <ChartHeaderBar
                        title=title_val.unwrap_or_default()
                        chart_type=ct.unwrap_or_default()
                        chart_orientation=co.unwrap_or_default()
                        chart_mode=cm.unwrap_or_default()
                        show_type_selector=true
                        show_refresh=true
                        show_edit=has_edit
                        show_delete=has_delete
                        show_save_to_dashboard=has_save
                        show_info=has_info
                        on_type_change=type_cb
                        on_orientation_change=orient_cb
                        on_mode_change=mode_cb
                        on_refresh=refresh_cb
                        on_edit=edit_action
                        on_delete=delete_action
                        on_save_to_dashboard=save_action
                        on_info=info_action
                        last_updated=last_sig
                        is_refreshing=refreshing_sig
                    />
                }
            }}
            // Chart content area — unified through ChartMLChart
            {if is_remote {
                // Remote path: show loading/error states, or ChartMLChart once data arrives
                view! {
                    <div class="w-full">
                        {move || {
                            if let Some(err) = remote_error.get() {
                                view! {
                                    <div class="p-4 bg-destructive/10 border border-destructive/20 rounded-lg">
                                        <div class="flex items-start gap-3">
                                            <svg class="w-5 h-5 text-destructive flex-shrink-0 mt-0.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                                            </svg>
                                            <div class="flex-1">
                                                <h3 class="text-sm font-semibold text-destructive mb-1">"Chart Error"</h3>
                                                <p class="text-sm text-destructive/90">{err}</p>
                                                <button
                                                    on:click=move |_| set_refresh_count.update(|c| *c += 1)
                                                    class="mt-2 text-xs text-primary hover:text-primary/80 underline"
                                                >
                                                    "Retry"
                                                </button>
                                            </div>
                                        </div>
                                    </div>
                                }.into_any()
                            } else if remote_loading.get() {
                                view! {
                                    <div class="flex items-center justify-center py-12">
                                        <svg class="animate-spin h-6 w-6 text-muted-foreground" fill="none" viewBox="0 0 24 24">
                                            <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
                                            <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
                                        </svg>
                                        <span class="ml-2 text-sm text-muted-foreground">"Loading chart..."</span>
                                    </div>
                                }.into_any()
                            } else if let Some(remote_instance) = remote_chartml.get() {
                                view! {
                                    <ChartMLChart
                                        spec=spec_signal
                                        chartml=remote_instance
                                    />
                                }.into_any()
                            } else {
                                // Should not happen — either loading, error, or data
                                view! { <div /> }.into_any()
                            }
                        }}
                    </div>
                }.into_any()
            } else {
                // Inline data path — delegate to ChartMLChart directly
                view! {
                    <ChartMLChart
                        spec=spec_signal
                        chartml=chartml_for_inline
                    />
                }.into_any()
            }}
        </div>
    }
}

// ---------------------------------------------------------------------------
// Main component
// ---------------------------------------------------------------------------

/// Renders dashboard markdown content, handling embedded ChartML code blocks.
///
/// Markdown segments are converted to HTML via `pulldown-cmark` and rendered
/// with Tailwind prose classes. Code blocks get styled `<pre><code>` with a
/// copy button. ChartML blocks are parsed, data is fetched, and charts are
/// rendered as SVG using chartml-core.
#[component]
pub fn MarkdownRenderer(
    /// The markdown content to render (may contain ```chartml blocks)
    #[prop(into)]
    content: Signal<String>,
    /// Parameter values for SQL template substitution
    #[prop(into, optional)]
    parameters: Signal<HashMap<String, String>>,
    /// Whether content is currently being streamed (incomplete markdown cleanup)
    #[prop(optional)]
    is_streaming: Option<Signal<bool>>,
    /// Callback when a chart's "edit" action is clicked (block_index, array_index)
    #[prop(optional)]
    on_edit_chart: Option<Callback<(usize, usize)>>,
    /// Callback when a chart's "delete" action is clicked
    #[prop(optional)]
    on_delete_chart: Option<Callback<(usize, usize)>>,
    /// Callback to save a chart to another dashboard (receives chart YAML wrapped in code fence)
    #[prop(optional)]
    on_save_to_dashboard: Option<Callback<String>>,
    /// Callback to show chart info/spec (receives chart YAML)
    #[prop(optional)]
    on_chart_info: Option<Callback<String>>,
    /// Callback for "ask about this chart" — receives chart YAML wrapped in code fence
    #[prop(optional)]
    on_ask_about_chart: Option<Callback<String>>,
    /// Callback when a watch preview card is approved — receives `(watch_data, card_id)`
    #[prop(optional)]
    on_watch_approved: Option<Callback<(serde_json::Value, String)>>,
    /// Set of card IDs that have already been accepted (for dimming approved cards)
    #[prop(optional)]
    accepted_card_ids: Option<Signal<HashSet<String>>>,
    /// Message ID used to generate stable card IDs for watch preview cards
    #[prop(optional, into)]
    message_id: Option<String>,
    /// User's chart palette preference (e.g. "balanced", "vibrant", "accessible")
    #[prop(optional, into)]
    chart_palette: Option<String>,
) -> impl IntoView {
    let palette_name = StoredValue::new(chart_palette.unwrap_or_else(|| "balanced".to_string()));
    let msg_id = StoredValue::new(message_id.unwrap_or_default());

    let segments = Memo::new(move |_| {
        let raw = content.get();
        // When streaming, strip incomplete chartml blocks to prevent YAML parse errors
        let cleaned = match is_streaming {
            Some(streaming_sig) if streaming_sig.get() => {
                clean_streaming_markdown(&raw)
            }
            _ => raw,
        };
        parse_segments(&cleaned)
    });

    // Store callbacks in StoredValue so they can be cloned into the For loop closure.
    let edit_cb = StoredValue::new(on_edit_chart);
    let delete_cb = StoredValue::new(on_delete_chart);
    let save_cb = StoredValue::new(on_save_to_dashboard);
    let info_cb = StoredValue::new(on_chart_info);
    let ask_cb = StoredValue::new(on_ask_about_chart);
    let watch_cb = StoredValue::new(on_watch_approved);
    let accepted_ids = StoredValue::new(accepted_card_ids);

    view! {
        <div class="prose prose-sm dark:prose-invert max-w-none">
            <For
                each=move || {
                    segments.get().into_iter().enumerate().collect::<Vec<_>>()
                }
                key=|(i, _)| *i
                children=move |(_, segment)| {
                    match segment {
                        ContentSegment::Markdown(md) => {
                            let html = markdown_to_html(&md);
                            view! {
                                <div inner_html=html></div>
                            }.into_any()
                        }
                        ContentSegment::CodeBlock { language, code } => {
                            view! {
                                <CodeBlockView language=language code=code />
                            }.into_any()
                        }
                        ContentSegment::ChartML { yaml, block_index, array_index } => {
                            let edit = edit_cb.get_value();
                            let delete = delete_cb.get_value();
                            let save = save_cb.get_value();
                            let info = info_cb.get_value();
                            let ask = ask_cb.get_value();
                            view! {
                                <ChartBlock
                                    yaml=yaml
                                    block_index=block_index
                                    array_index=array_index
                                    parameters=parameters
                                    on_edit_chart=edit
                                    on_delete_chart=delete
                                    on_save_to_dashboard=save
                                    on_chart_info=info
                                    on_ask_about_chart=ask
                                    chart_palette=palette_name.get_value()
                                />
                            }.into_any()
                        }
                        ContentSegment::WatchResponse { message, watch } => {
                            let watch_approve = watch_cb.get_value();
                            let mid = msg_id.get_value();

                            // Generate stable card ID: {message_id}-{watch.name}-{watch.schedule}
                            let watch_name = watch
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let watch_schedule = watch
                                .get("schedule")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let card_id =
                                format!("{}-{}-{}", mid, watch_name, watch_schedule);

                            let is_accepted = accepted_ids
                                .get_value()
                                .map(|sig| sig.get_untracked().contains(&card_id))
                                .unwrap_or(false);

                            view! {
                                <WatchPreviewCardView
                                    message=message
                                    watch=watch
                                    on_watch_approved=watch_approve
                                    is_accepted=is_accepted
                                />
                            }.into_any()
                        }
                    }
                }
            />
        </div>
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_markdown_only() {
        let segments = parse_segments("# Hello\n\nSome text.");
        assert_eq!(segments.len(), 1);
        assert!(matches!(&segments[0], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_chartml_block_extraction() {
        let input = "# Title\n\n```chartml\ntype: bar\ndata: test\n```\n\nMore text.";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 3);
        assert!(matches!(&segments[0], ContentSegment::Markdown(_)));
        assert!(
            matches!(&segments[1], ContentSegment::ChartML { yaml, .. } if yaml.contains("type: bar"))
        );
        assert!(matches!(&segments[2], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_multiple_chartml_blocks() {
        let input =
            "Text\n```chartml\nchart1\n```\nMiddle\n```chartml\nchart2\n```\nEnd";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 5);
        assert!(matches!(&segments[0], ContentSegment::Markdown(_)));
        assert!(
            matches!(&segments[1], ContentSegment::ChartML { yaml, .. } if yaml == "chart1")
        );
        assert!(matches!(&segments[2], ContentSegment::Markdown(_)));
        assert!(
            matches!(&segments[3], ContentSegment::ChartML { yaml, .. } if yaml == "chart2")
        );
        assert!(matches!(&segments[4], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_chartml_at_start() {
        let input = "```chartml\nchart_data\n```\nAfter.";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 2);
        assert!(
            matches!(&segments[0], ContentSegment::ChartML { yaml, .. } if yaml == "chart_data")
        );
        assert!(matches!(&segments[1], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_empty_content() {
        let segments = parse_segments("");
        assert!(segments.is_empty());
    }

    #[test]
    fn test_non_chartml_code_blocks_are_code_segments() {
        let input = "```python\nprint('hello')\n```";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 1);
        assert!(matches!(
            &segments[0],
            ContentSegment::CodeBlock { language, code }
            if language == "python" && code == "print('hello')"
        ));
    }

    #[test]
    fn test_markdown_to_html_basic() {
        let html = markdown_to_html("**bold** and *italic*");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
    }

    #[test]
    fn test_substitute_params_basic() {
        let sql = "SELECT * FROM users WHERE region = '{{region}}' AND year = {{year}}";
        let mut params = HashMap::new();
        params.insert("region".to_string(), "US".to_string());
        params.insert("year".to_string(), "2024".to_string());
        let result = substitute_params(sql, &params);
        assert_eq!(
            result,
            "SELECT * FROM users WHERE region = 'US' AND year = 2024"
        );
    }

    #[test]
    fn test_substitute_params_empty() {
        let sql = "SELECT 1";
        let params = HashMap::new();
        let result = substitute_params(sql, &params);
        assert_eq!(result, "SELECT 1");
    }

    #[test]
    fn test_chartml_block_indices() {
        let input = "```chartml\nfirst\n```\ntext\n```chartml\nsecond\n```";
        let segments = parse_segments(input);
        assert!(matches!(
            &segments[0],
            ContentSegment::ChartML { block_index: 0, array_index: 0, .. }
        ));
        assert!(matches!(
            &segments[2],
            ContentSegment::ChartML { block_index: 1, array_index: 0, .. }
        ));
    }

    #[test]
    fn test_mixed_code_blocks() {
        let input =
            "```sql\nSELECT 1;\n```\n\n```chartml\ntype: chart\n```\n\n```python\nprint(1)\n```";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 3);
        assert!(matches!(
            &segments[0],
            ContentSegment::CodeBlock { language, .. } if language == "sql"
        ));
        assert!(matches!(
            &segments[1],
            ContentSegment::ChartML { yaml, .. } if yaml.contains("type: chart")
        ));
        assert!(matches!(
            &segments[2],
            ContentSegment::CodeBlock { language, .. } if language == "python"
        ));
    }

    #[test]
    fn test_extract_title() {
        let spec: serde_json::Value =
            serde_json::json!({"type": "chart", "title": "Revenue", "visualize": {"type": "bar"}});
        assert_eq!(extract_title(&spec), Some("Revenue".to_string()));
    }

    #[test]
    fn test_extract_datasource() {
        let spec: serde_json::Value = serde_json::json!({
            "type": "chart",
            "data": {"datasource": "main_db", "query": "SELECT 1"}
        });
        assert_eq!(extract_datasource(&spec), Some("main_db".to_string()));
    }

    #[test]
    fn test_extract_query() {
        let spec: serde_json::Value = serde_json::json!({
            "type": "chart",
            "data": {"datasource": "main_db", "query": "SELECT * FROM users"}
        });
        assert_eq!(
            extract_query(&spec),
            Some("SELECT * FROM users".to_string())
        );
    }

    // -----------------------------------------------------------------------
    // Streaming markdown cleanup tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_clean_streaming_incomplete_chartml() {
        let input = "Some text\n```chartml\ntype: bar\ndata:";
        let result = clean_streaming_markdown(input);
        assert_eq!(result, "Some text\n");
    }

    #[test]
    fn test_clean_streaming_complete_chartml() {
        let input = "Some text\n```chartml\ntype: bar\n```\nAfter.";
        let result = clean_streaming_markdown(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_clean_streaming_no_chartml() {
        let input = "Just regular markdown with **bold**.";
        let result = clean_streaming_markdown(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_clean_streaming_multiple_chartml_last_incomplete() {
        let input =
            "```chartml\nchart1\n```\nMiddle\n```chartml\nchart2_incomplete";
        let result = clean_streaming_markdown(input);
        assert_eq!(result, "```chartml\nchart1\n```\nMiddle\n");
    }

    #[test]
    fn test_clean_streaming_multiple_chartml_all_complete() {
        let input =
            "```chartml\nchart1\n```\nMiddle\n```chartml\nchart2\n```\nEnd";
        let result = clean_streaming_markdown(input);
        assert_eq!(result, input);
    }

    // -----------------------------------------------------------------------
    // Watch-response parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_watch_response_block_explicit_lang() {
        let json_content = r#"{"message": "I created a watch.", "watch": {"name": "Sales Alert", "schedule": "0 9 * * *"}}"#;
        let input = format!("```json:watch-response\n{}\n```", json_content);
        let segments = parse_segments(&input);
        assert_eq!(segments.len(), 1);
        match &segments[0] {
            ContentSegment::WatchResponse { message, watch } => {
                assert_eq!(message, "I created a watch.");
                assert_eq!(
                    watch.get("name").unwrap().as_str().unwrap(),
                    "Sales Alert"
                );
            }
            other => panic!("Expected WatchResponse, got {:?}", other),
        }
    }

    #[test]
    fn test_watch_response_block_heuristic_detection() {
        let json_content = r#"{"message": "Here is your watch.", "watch": {"name": "Test", "schedule": "*/5 * * * *"}}"#;
        let input = format!("```json\n{}\n```", json_content);
        let segments = parse_segments(&input);
        assert_eq!(segments.len(), 1);
        assert!(matches!(
            &segments[0],
            ContentSegment::WatchResponse { .. }
        ));
    }

    #[test]
    fn test_watch_response_invalid_json_falls_through() {
        let input = "```json:watch-response\n{not valid json}\n```";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 1);
        assert!(matches!(
            &segments[0],
            ContentSegment::CodeBlock { .. }
        ));
    }

    #[test]
    fn test_watch_response_missing_keys_falls_through() {
        // Has "message" but no "watch" key
        let input = "```json:watch-response\n{\"message\": \"hello\"}\n```";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 1);
        assert!(matches!(
            &segments[0],
            ContentSegment::CodeBlock { .. }
        ));
    }

    #[test]
    fn test_try_parse_watch_response_valid() {
        let json_str = r#"{"message": "Watch ready.", "watch": {"name": "W1", "schedule": "0 * * * *", "prompt": "Check sales"}}"#;
        let seg = try_parse_watch_response(json_str).unwrap();
        match seg {
            ContentSegment::WatchResponse { message, watch } => {
                assert_eq!(message, "Watch ready.");
                assert_eq!(
                    watch.get("prompt").unwrap().as_str().unwrap(),
                    "Check sales"
                );
            }
            _ => panic!("Expected WatchResponse"),
        }
    }

    #[test]
    fn test_try_parse_watch_response_invalid() {
        assert!(try_parse_watch_response("not json").is_none());
        assert!(try_parse_watch_response(r#"{"foo": 1}"#).is_none());
    }
}
