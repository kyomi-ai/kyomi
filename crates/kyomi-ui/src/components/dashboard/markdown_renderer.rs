// SPDX-License-Identifier: AGPL-3.0-or-later

//! Markdown + ChartML renderer component.
//!
//! Splits content into alternating Markdown, code-block, and ChartML segments.
//! Markdown is rendered via `pulldown-cmark`, code blocks get syntax-styled
//! `<pre><code>` with a copy button, and ChartML blocks are parsed, data is
//! fetched via the datasource server function, rendered through chartml-core,
//! and the resulting `ChartElement` tree is converted to Leptos SVG views.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_chart_table::TableRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_core::ChartML;
use chartml_datafusion::DataFusionTransform;
use chartml_leptos::{use_chartml_configured, ChartMLChart};
// `kyomi_palette` and `kyomi_theme` live in the tiny `kyomi-chart-theme`
// crate so they're accessible from both the browser rendering path
// (kyomi-ui compiled for wasm32) and the SSR rendering path (kyomi-agent
// via chartml_factory → PDF export, email snapshots, MCP chart app). The
// crate has zero server-only dependencies, so it compiles cleanly under
// the `hydrate` feature. Re-exported here so existing callers in this
// file don't have to touch their import paths.
pub(crate) use kyomi_chart_theme::{kyomi_palette, kyomi_theme};
use super::chart_header_bar::ChartHeaderBar;
use super::source_cache::DashboardSourceCache;
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
    ///
    /// `yamls` carries ONE fully-serialized YAML spec per chart in the block.
    /// A single-chart block has `yamls.len() == 1`; a 3-chart YAML-array block
    /// has `yamls.len() == 3`, with each entry already split into its own spec.
    ChartML {
        yamls: Vec<String>,
        block_index: usize,
    },
    /// Non-chartml fenced code block.
    CodeBlock { language: String, code: String },
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
                                let yamls = split_chartml_block(block_content);
                                if !yamls.is_empty() {
                                    segments.push(ContentSegment::ChartML {
                                        yamls,
                                        block_index: chartml_block_index,
                                    });
                                }
                            }
                            chartml_block_index += 1;
                        } else if !block_content.is_empty() {
                            segments.push(ContentSegment::CodeBlock {
                                language: language.to_string(),
                                code: block_content.to_string(),
                            });
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
                                let yamls = split_chartml_block(block_content);
                                if !yamls.is_empty() {
                                    segments.push(ContentSegment::ChartML {
                                        yamls,
                                        block_index: chartml_block_index,
                                    });
                                }
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

/// Parse a ```chartml``` block. If the YAML top-level value is a Sequence,
/// return each item re-serialized to its own YAML string. If it's a Mapping
/// (or fails to parse), return a single-element vec containing the original
/// block content unchanged.
fn split_chartml_block(block_content: &str) -> Vec<String> {
    match serde_yaml::from_str::<serde_json::Value>(block_content) {
        Ok(serde_json::Value::Array(items)) => {
            let per_item: Vec<String> = items
                .iter()
                .filter_map(|v| serde_yaml::to_string(v).ok())
                .collect();
            if per_item.is_empty() {
                vec![block_content.to_string()]
            } else {
                per_item
            }
        }
        _ => vec![block_content.to_string()],
    }
}

// ---------------------------------------------------------------------------
// Markdown → HTML
// ---------------------------------------------------------------------------

/// Convert a markdown string to HTML using pulldown-cmark with GFM extensions.
fn markdown_to_html(markdown: &str) -> String {
    use pulldown_cmark::{CowStr, Event, Options};
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    // Replace horizontal rules with a centered asterism — an editorial
    // typographic ornament rendered in Instrument Serif (font-display) at
    // a muted color. See DESIGN.md "Typographic Marks".
    const ASTERISM_HTML: &str = "<div class=\"my-10 text-center text-[color:var(--color-muted-foreground)] text-xl font-display\">\u{2042}</div>";
    let parser = pulldown_cmark::Parser::new_ext(markdown, options).map(|event| match event {
        Event::Rule => Event::Html(CowStr::Borrowed(ASTERISM_HTML)),
        other => other,
    });
    let mut html_output = String::new();
    pulldown_cmark::html::push_html(&mut html_output, parser);
    // Add loading="lazy" to all images for performance
    html_output = html_output.replace("<img ", "<img loading=\"lazy\" ");
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

// ---------------------------------------------------------------------------
// Named-source dispatch helpers
// ---------------------------------------------------------------------------

/// Reserved keys that indicate a flat/inline data source. Any `data:` map whose
/// keys are all in this set is treated as the flat shape, not a named-source map.
/// Mirrors React's `RESERVED_DATA_KEYS` in `packages/chartml-transform/src/helpers.js`.
const RESERVED_DATA_KEYS: &[&str] = &[
    "datasource",
    "provider",
    "query",
    "rows",
    "url",
    "cache",
    "endpoint",
];

/// Parsed description of one entry in a named-sources map.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NamedSourceSpec {
    /// User-provided source name (the YAML key). This is what the ChartML spec
    /// ends up referencing — i.e. after we register the table under this name,
    /// rewriting `data:` to `Value::String(name)` makes `DataRef::Named` resolve.
    pub name: String,
    /// Datasource slug (`datasource:` or alias `endpoint:`).
    pub slug: String,
    /// SQL query (`query:` or alias `url:`).
    pub query: String,
    /// Cache TTL parsed from `cache.ttl`, if present. `None` = no expiry.
    pub ttl: Option<Duration>,
}

/// Detect a map-of-maps `data:` shape and return each entry as a `NamedSourceSpec`.
///
/// Returns:
/// - `None` if `data` is a string, array, a flat inline source (contains any
///   reserved key at the top level), or is missing / not an object.
/// - `None` if the map is empty.
/// - `None` if any entry is malformed (missing `datasource`/`endpoint` or
///   `query`/`url`). Caller treats this as Unknown and does not fetch.
/// - `Some(Vec)` for a well-formed named-source map.
fn extract_named_sources(spec: &serde_json::Value) -> Option<Vec<NamedSourceSpec>> {
    let data = spec.get("data")?;
    let obj = data.as_object()?;
    if obj.is_empty() {
        return None;
    }
    // If ANY reserved key is present at the top level, it's the flat shape.
    if obj.keys().any(|k| RESERVED_DATA_KEYS.contains(&k.as_str())) {
        return None;
    }

    let mut sources = Vec::with_capacity(obj.len());
    for (name, value) in obj {
        let entry = value.as_object()?;
        let slug = entry
            .get("datasource")
            .or_else(|| entry.get("endpoint"))
            .and_then(|v| v.as_str())?
            .to_string();
        let query = entry
            .get("query")
            .or_else(|| entry.get("url"))
            .and_then(|v| v.as_str())?
            .to_string();
        let ttl = entry
            .get("cache")
            .and_then(|c| c.get("ttl"))
            .and_then(|v| v.as_str())
            .and_then(parse_ttl);

        sources.push(NamedSourceSpec {
            name: name.clone(),
            slug,
            query,
            ttl,
        });
    }
    Some(sources)
}

/// Parse a human-friendly TTL string to a `Duration`.
///
/// Supported units (case-insensitive): `ms`, `s`, `m`, `h`, `d`. Integer-only
/// values — fractional TTLs are rejected.
///
/// Examples: `"6h"`, `"30m"`, `"1d"`, `"45s"`, `"500ms"`.
fn parse_ttl(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    // Order matters: "ms" must be tried before "s".
    for (suffix, unit) in [
        ("ms", DurationUnit::Millis),
        ("s", DurationUnit::Seconds),
        ("m", DurationUnit::Minutes),
        ("h", DurationUnit::Hours),
        ("d", DurationUnit::Days),
    ] {
        if let Some(num_str) = lower.strip_suffix(suffix) {
            let n: u64 = num_str.trim().parse().ok()?;
            return Some(unit.duration(n));
        }
    }
    None
}

#[derive(Copy, Clone)]
enum DurationUnit {
    Millis,
    Seconds,
    Minutes,
    Hours,
    Days,
}

impl DurationUnit {
    fn duration(self, n: u64) -> Duration {
        match self {
            DurationUnit::Millis => Duration::from_millis(n),
            DurationUnit::Seconds => Duration::from_secs(n),
            DurationUnit::Minutes => Duration::from_secs(n * 60),
            DurationUnit::Hours => Duration::from_secs(n * 3600),
            DurationUnit::Days => Duration::from_secs(n * 86_400),
        }
    }
}

/// True when `spec.transform` is a non-empty object. Matches React's truthy
/// `spec.transform` check but tightens it — an empty map means "no transform".
fn has_transform(spec: &serde_json::Value) -> bool {
    spec.get("transform")
        .and_then(|t| t.as_object())
        .map(|o| !o.is_empty())
        .unwrap_or(false)
}

/// Shape classification of a chart's `data:` section. Drives the `ChartBlock`
/// dispatch — each variant maps to either a specific render path or a specific
/// user-facing error message.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ChartDataShape {
    /// Either `data.provider == "inline"` or a literal `rows:` array — the
    /// chartml renderer handles these directly without any host-side fetch.
    Inline,
    /// `data: "<name>"` — a reference to a pre-registered source.
    StringRef,
    /// Flat single-source shape: `data: { datasource, query }` (or the `endpoint`
    /// / `url` aliases). Host fetches, registers as `"_remote"`, rewrites `data:`.
    FlatRemote,
    /// One named source with no transform — host fetches, registers under the
    /// user's chosen name, rewrites `data:` to that name. Renders cleanly.
    NamedSingleNoTransform,
    /// One named source WITH a transform block — transforms against named
    /// sources aren't supported by the Rust chartml renderer yet (KYO-90).
    NamedSingleWithTransform,
    /// Multiple named sources with no transform — invalid per React's own
    /// validation rules (no sensible single-source to render).
    NamedMultiNoTransform,
    /// Multiple named sources with a transform — would need cross-source SQL
    /// joins, also blocked on KYO-90.
    NamedMultiWithTransform,
    /// Couldn't classify the `data:` section. Treated like `Inline` (no fetch);
    /// chartml's own parser reports any real parse error downstream.
    Unknown,
}

/// Classify a ChartML spec by its `data:` shape. Pure parsing — no side effects.
fn classify_chart_data(spec: &serde_json::Value) -> ChartDataShape {
    let Some(data) = spec.get("data") else {
        return ChartDataShape::Unknown;
    };
    // String reference: `data: "my_source"`
    if data.is_string() {
        return ChartDataShape::StringRef;
    }
    // Arrays aren't a valid `data:` shape — let chartml's parser complain.
    if !data.is_object() {
        return ChartDataShape::Unknown;
    }
    let Some(obj) = data.as_object() else {
        return ChartDataShape::Unknown;
    };

    // Inline by provider or rows.
    let provider_is_inline = obj
        .get("provider")
        .and_then(|p| p.as_str())
        .map(|s| s.eq_ignore_ascii_case("inline"))
        .unwrap_or(false);
    if provider_is_inline || obj.contains_key("rows") {
        return ChartDataShape::Inline;
    }

    // Flat remote: contains datasource/endpoint + query/url at the top level.
    let has_slug = obj.contains_key("datasource") || obj.contains_key("endpoint");
    let has_query = obj.contains_key("query") || obj.contains_key("url");
    if has_slug && has_query {
        return ChartDataShape::FlatRemote;
    }

    // Named-source map detection. If none of the reserved keys appear at the
    // top level, it's a map-of-maps named-source shape.
    if obj.is_empty() || obj.keys().any(|k| RESERVED_DATA_KEYS.contains(&k.as_str())) {
        return ChartDataShape::Unknown;
    }

    // It's named-source shape; count entries and check transform.
    let named = match extract_named_sources(spec) {
        Some(v) if !v.is_empty() => v,
        _ => return ChartDataShape::Unknown,
    };
    let tx = has_transform(spec);
    match (named.len(), tx) {
        (1, false) => ChartDataShape::NamedSingleNoTransform,
        (1, true) => ChartDataShape::NamedSingleWithTransform,
        (_, false) => ChartDataShape::NamedMultiNoTransform,
        (_, true) => ChartDataShape::NamedMultiWithTransform,
    }
}

/// Extract the chart type from a parsed YAML spec (e.g. "bar", "line", "pie").
pub(crate) fn extract_chart_type(spec: &serde_json::Value) -> Option<String> {
    spec.get("visualize")
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract the chart orientation from a parsed YAML spec (e.g. "horizontal").
pub(crate) fn extract_chart_orientation(spec: &serde_json::Value) -> Option<String> {
    spec.get("visualize")
        .and_then(|v| v.get("orientation"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract the chart mode from a parsed YAML spec (e.g. "stacked", "grouped").
pub(crate) fn extract_chart_mode(spec: &serde_json::Value) -> Option<String> {
    spec.get("visualize")
        .and_then(|v| v.get("mode"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract `layout.colSpan` (or snake_case `col_span`) from a parsed spec,
/// clamped to 1..=12. Defaults to 12 when missing / invalid.
fn extract_col_span(spec: &serde_json::Value) -> u8 {
    let raw = spec
        .get("layout")
        .and_then(|l| l.get("colSpan").or_else(|| l.get("col_span")))
        .and_then(|v| v.as_u64());
    match raw {
        Some(n) if (1..=12).contains(&n) => n as u8,
        _ => 12,
    }
}

/// Map colSpan (1..=12) to static Tailwind classes. Mobile = full width,
/// `md:` breakpoint = specified span. Static strings so Tailwind's content
/// scanner picks them up.
fn chart_col_span_class(col_span: u8) -> &'static str {
    match col_span {
        1 => "col-span-12 md:col-span-1",
        2 => "col-span-12 md:col-span-2",
        3 => "col-span-12 md:col-span-3",
        4 => "col-span-12 md:col-span-4",
        5 => "col-span-12 md:col-span-5",
        6 => "col-span-12 md:col-span-6",
        7 => "col-span-12 md:col-span-7",
        8 => "col-span-12 md:col-span-8",
        9 => "col-span-12 md:col-span-9",
        10 => "col-span-12 md:col-span-10",
        11 => "col-span-12 md:col-span-11",
        _ => "col-span-12", // 12 or anything unexpected
    }
}

// ---------------------------------------------------------------------------
// ChartML instance factory
// ---------------------------------------------------------------------------
// `kyomi_palette` and `kyomi_theme` live in the `kyomi-chart-theme` crate
// (`crates/kyomi-chart-theme/src/lib.rs`). See the re-export at the top
// of this file.

/// Create a configured ChartML instance with all chart renderers registered,
/// the user's palette preference applied, and the Kyomi editorial theme wired
/// in. `is_dark` selects per-mode palette slots and chrome colors.
pub(crate) fn configured_chartml(palette_name: &str, is_dark: bool) -> Arc<ChartML> {
    let colors = kyomi_palette(palette_name, is_dark);
    let theme = kyomi_theme(is_dark);
    use_chartml_configured(|c| {
        c.register_renderer("bar", CartesianRenderer::new());
        c.register_renderer("line", CartesianRenderer::new());
        c.register_renderer("area", CartesianRenderer::new());
        c.register_renderer("pie", PieRenderer::new());
        c.register_renderer("doughnut", PieRenderer::new());
        c.register_renderer("scatter", ScatterRenderer::new());
        c.register_renderer("metric", MetricRenderer::new());
        c.register_renderer("table", TableRenderer::new());
        c.register_transform(DataFusionTransform);
        c.set_default_palette(colors.clone());
        c.set_theme(theme.clone());
    })
}

/// Build a fresh `ChartML` instance (the mutable variant) with the same wiring
/// as `configured_chartml` — all renderers, the DataFusion transform, the
/// user's palette, and the editorial theme. Used by the remote-fetch paths
/// which need to `register_source` on the instance before handing it to
/// `ChartMLChart` (Arc doesn't expose mutable access once cloned).
///
/// Any changes to the renderer/palette/theme wiring MUST also be applied to
/// `configured_chartml` so the inline-data and remote-data paths stay aligned.
fn build_chartml_mut(palette_name: &str, is_dark: bool) -> ChartML {
    let mut chartml_mut = ChartML::new();
    let colors = kyomi_palette(palette_name, is_dark);
    let theme = kyomi_theme(is_dark);
    chartml_mut.register_renderer("bar", CartesianRenderer::new());
    chartml_mut.register_renderer("line", CartesianRenderer::new());
    chartml_mut.register_renderer("area", CartesianRenderer::new());
    chartml_mut.register_renderer("pie", PieRenderer::new());
    chartml_mut.register_renderer("doughnut", PieRenderer::new());
    chartml_mut.register_renderer("scatter", ScatterRenderer::new());
    chartml_mut.register_renderer("metric", MetricRenderer::new());
    chartml_mut.register_renderer("table", TableRenderer::new());
    chartml_mut.register_transform(DataFusionTransform);
    chartml_mut.set_default_palette(colors);
    chartml_mut.set_theme(theme);
    chartml_mut
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
                class="absolute top-2 right-2 p-1.5 rounded-md bg-accent hover:bg-secondary/80 opacity-0 group-hover:opacity-100 transition-opacity z-10 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring focus-visible:opacity-100"
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

/// Apply type/orientation/mode overrides to a parsed ChartML spec.
///
/// Matches React's `ChartWithChrome` useMemo logic:
/// - Applies `type_override` to `visualize.type`
/// - Applies `orientation_override` (Some = set, None = remove)
/// - Applies `mode_override` (Some = set, None = remove)
/// - When type changes, strips per-row `mark` overrides so the new type applies uniformly
/// - When switching away from bar, removes incompatible `orientation`
/// - When switching away from bar/area, removes incompatible `mode`
pub(crate) fn apply_spec_overrides(
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
/// Uses `ChartHeaderBar` (Kyomi's native Rust/Tailwind header bar) instead of the
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
    let yaml_for_ask = yaml.clone();

    // Parse the spec to extract metadata
    let parsed_spec: Option<serde_json::Value> =
        serde_yaml::from_str(&yaml_owned).ok();

    let _chart_title = parsed_spec.as_ref().and_then(extract_title);
    let initial_chart_type = parsed_spec.as_ref().and_then(extract_chart_type);
    let initial_orientation = parsed_spec.as_ref().and_then(extract_chart_orientation);
    let initial_mode = parsed_spec.as_ref().and_then(extract_chart_mode);

    // Classify the data shape once — drives everything below (fetch path,
    // error surfacing, data-rewrite logic). For shapes that map to a user-
    // facing error we set `initial_error` so the error branch renders
    // immediately without a wasted fetch attempt.
    let data_shape = parsed_spec
        .as_ref()
        .map(classify_chart_data)
        .unwrap_or(ChartDataShape::Unknown);
    let named_sources = parsed_spec.as_ref().and_then(extract_named_sources);

    // For the flat-remote path we need the top-level datasource+query as
    // strings. For the named-single path the slug/query/name all live in
    // the first (and only) entry of `named_sources`.
    let flat_ds_slug = parsed_spec.as_ref().and_then(extract_datasource);
    let flat_sql_query = parsed_spec.as_ref().and_then(extract_query);

    // Which shapes trigger a remote fetch (loading spinner, async Effect).
    let is_remote = matches!(
        data_shape,
        ChartDataShape::FlatRemote | ChartDataShape::NamedSingleNoTransform
    );

    // For the remote-fetch paths, resolve the (slug, query, registered-name,
    // optional TTL) that the Effect will consume. The registered-name is what
    // the rewritten `data:` string will point at. For the flat shape we keep
    // the legacy `"_remote"` name — existing production dashboards depend on
    // it, and changing it would invalidate any YAML that happens to reference
    // `_remote` by hand.
    const FLAT_REMOTE_NAME: &str = "_remote";
    let (remote_slug, remote_query, registered_name, remote_ttl) = match data_shape {
        ChartDataShape::FlatRemote => (
            flat_ds_slug.clone(),
            flat_sql_query.clone(),
            Some(FLAT_REMOTE_NAME.to_string()),
            None,
        ),
        ChartDataShape::NamedSingleNoTransform => {
            let entry = named_sources.as_ref().and_then(|v| v.first()).cloned();
            match entry {
                Some(ns) => (
                    Some(ns.slug),
                    Some(ns.query),
                    Some(ns.name),
                    ns.ttl,
                ),
                None => (None, None, None, None),
            }
        }
        _ => (None, None, None, None),
    };

    // Classify-time error message for shapes we cannot render. The empty
    // string means "no classify-time error" — set lazily below when we know
    // the shape.
    let initial_error: Option<String> = match data_shape {
        ChartDataShape::NamedSingleWithTransform => Some(
            "Named data sources with a transform block require chartml ≥ 4.2 \
             (tracked in KYO-90). Until then, use the flat 'data: { datasource, query }' \
             shape or remove the transform."
                .to_string(),
        ),
        ChartDataShape::NamedMultiNoTransform => Some(
            "Named data sources require a transform block when multiple sources are defined"
                .to_string(),
        ),
        ChartDataShape::NamedMultiWithTransform => Some(
            "Cross-source SQL joins aren't supported by the Rust chartml renderer yet \
             (tracked in KYO-90). Each source is registered but the transform stage \
             can only reference a single table."
                .to_string(),
        ),
        _ => None,
    };

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

        apply_spec_overrides(
            &yaml_for_spec,
            t_ovr.as_deref(),
            o_ovr.as_ref().map(|o| o.as_deref()),
            m_ovr.as_ref().map(|m| m.as_deref()),
        )
    });

    // Create the ChartML instance
    let palette = chart_palette.unwrap_or_else(|| "kyomi".to_string());
    let is_dark = crate::components::theme::use_theme()
        .map(|s| s.effective.get_untracked() == "dark")
        .unwrap_or(false);
    let chartml = configured_chartml(&palette, is_dark);

    // -- Remote data path: fetch data, register on ChartML instance, render via ChartMLChart --
    // We store fetched remote data as a signal. When data arrives, we create a new ChartML
    // instance with the data registered under `registered_name` and rewrite the YAML `data:`
    // section to reference it. This lets ChartMLChart handle everything uniformly.
    let (remote_chartml, set_remote_chartml) = signal(None::<Arc<ChartML>>);
    let (remote_error, set_remote_error) = signal(initial_error.clone());
    let (remote_loading, set_remote_loading) = signal(is_remote && initial_error.is_none());

    // Pull the dashboard-scoped cache from context. Fallback: a fresh per-block
    // cache for call sites (e.g. chart-builder preview) without a dashboard
    // ancestor. Still honors TTL/dedup within that block's lifetime.
    let source_cache = use_context::<DashboardSourceCache>()
        .unwrap_or_else(DashboardSourceCache::new);

    if is_remote && initial_error.is_none() {
        let ds_slug = remote_slug.clone();
        let sql = remote_query.clone();
        let reg_name = registered_name.clone();
        let palette_for_remote = palette.clone();
        let is_dark_for_remote = is_dark;
        let ttl_for_remote = remote_ttl;
        let cache_for_remote = source_cache.clone();

        Effect::new(move || {
            let params = parameters.get();
            let _refresh = refresh_count.get();
            let ds = ds_slug.clone();
            let q = sql.clone();
            let name = reg_name.clone();
            let pal = palette_for_remote.clone();
            let is_dark = is_dark_for_remote;
            let ttl = ttl_for_remote;
            let cache = cache_for_remote.clone();

            set_remote_loading.set(true);
            set_remote_error.set(None);
            set_is_refreshing.set(true);

            leptos::task::spawn_local(async move {
                // These unwraps cannot panic — we only install this Effect
                // when `is_remote` is true, which guarantees all three are
                // Some. Kept as unwrap to make that invariant loud.
                let slug = ds.unwrap();
                let query = q.unwrap();
                let register_as = name.unwrap();
                let resolved_sql = substitute_params(&query, &params);

                // Cache key is per (slug, resolved_sql) — parameter substitutions
                // with different values produce independent cache entries.
                let fetch_slug = slug.clone();
                let fetch_query = resolved_sql.clone();
                let fetch_result = cache
                    .fetch(&slug, &resolved_sql, ttl, move || async move {
                        match query_datasource_arrow(fetch_slug, fetch_query, None).await {
                            Ok(query_result) => {
                                use base64::Engine;
                                let ipc_bytes = base64::engine::general_purpose::STANDARD
                                    .decode(&query_result.ipc_base64)
                                    .map_err(|e| format!("Base64 decode error: {e}"))?;
                                chartml_core::data::DataTable::from_ipc_bytes(&ipc_bytes)
                                    .map_err(|e| format!("Arrow decode error: {e}"))
                            }
                            Err(e) => Err(format!("Query error: {e}")),
                        }
                    })
                    .await;

                match fetch_result {
                    Ok(data_table) => {
                        let mut chartml_mut = build_chartml_mut(&pal, is_dark);
                        chartml_mut.register_source(&register_as, data_table);
                        set_remote_chartml.set(Some(Arc::new(chartml_mut)));
                        let now_ms = js_sys::Date::now();
                        set_last_refreshed.set(Some(now_ms));
                        set_remote_loading.set(false);
                        set_is_refreshing.set(false);
                    }
                    Err(e) => {
                        set_remote_error.set(Some(e));
                        set_remote_loading.set(false);
                        set_is_refreshing.set(false);
                    }
                }
            });
        });
    }

    // Set initial "Last refreshed" timestamp for non-fetching charts (inline data
    // and error-shape charts) — rendered immediately on mount, no async path.
    if !is_remote {
        set_last_refreshed.set(Some(js_sys::Date::now()));
    }

    // RwSignal for ChartMLChart spec — updated by Effect when overrides change.
    // For remote shapes, rewrite `data:` to the registered source name so
    // chartml resolves it via DataRef::Named. For inline/string-ref shapes,
    // pass through unchanged.
    let (chartml_spec, set_chartml_spec) = signal(yaml_owned.clone());
    let reg_name_for_rewrite = registered_name.clone();
    Effect::new(move || {
        let base_spec = effective_spec.get();

        let final_spec = match reg_name_for_rewrite.as_deref() {
            Some(name) => {
                // Rewrite the data section to reference the registered source name
                match serde_yaml::from_str::<serde_json::Value>(&base_spec) {
                    Ok(mut val) => {
                        if let Some(obj) = val.as_object_mut() {
                            obj.insert(
                                "data".to_string(),
                                serde_json::Value::String(name.to_string()),
                            );
                        }
                        serde_yaml::to_string(&val).unwrap_or(base_spec)
                    }
                    Err(_) => base_spec,
                }
            }
            None => base_spec,
        };
        set_chartml_spec.set(final_spec);
    });

    // Determine which callbacks are available
    let has_edit = on_edit_chart.is_some();
    let has_delete = on_delete_chart.is_some();
    let has_save = on_save_to_dashboard.is_some();
    let has_info = on_chart_info.is_some();
    let has_ask = on_ask_about_chart.is_some();

    // Store callbacks for use in closures
    let edit_cb = StoredValue::new(on_edit_chart);
    let delete_cb = StoredValue::new(on_delete_chart);
    let save_cb = StoredValue::new(on_save_to_dashboard);
    let info_cb = StoredValue::new(on_chart_info);
    let ask_cb = StoredValue::new(on_ask_about_chart);
    let yaml_for_save_stored = StoredValue::new(yaml_for_save);
    let yaml_for_info_stored = StoredValue::new(yaml_for_info);
    let yaml_for_ask_stored = StoredValue::new(yaml_for_ask);

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
    let on_ask_cb = Callback::new(move |()| {
        if let Some(cb) = ask_cb.get_value() {
            let yaml = yaml_for_ask_stored.get_value();
            let chart_md = format!("```chartml\n{}\n```", yaml);
            cb.run(chart_md);
        }
    });

    // Spec signal for ChartMLChart — reads from the RwSignal updated by the Effect above
    let spec_signal = Signal::derive(move || chartml_spec.get());

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
    let on_ask_stored = StoredValue::new(on_ask_cb);

    view! {
        <div>
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
                let ask_action = on_ask_stored.get_value();
                let last_sig = Signal::derive(move || last_refreshed.get());
                let refreshing_sig = Signal::derive(move || is_refreshing.get());
                view! {
                    <ChartHeaderBar
                        chart_type=ct.unwrap_or_default()
                        chart_orientation=co.unwrap_or_default()
                        chart_mode=cm.unwrap_or_default()
                        show_type_selector=true
                        show_refresh=true
                        show_edit=has_edit
                        show_delete=has_delete
                        show_save_to_dashboard=has_save
                        show_info=has_info
                        show_ask_about=has_ask
                        on_type_change=type_cb
                        on_orientation_change=orient_cb
                        on_mode_change=mode_cb
                        on_refresh=refresh_cb
                        on_edit=edit_action
                        on_delete=delete_action
                        on_save_to_dashboard=save_action
                        on_info=info_action
                        on_ask_about=ask_action
                        last_updated=last_sig
                        is_refreshing=refreshing_sig
                    />
                }
            }}
            // Chart content area — unified through ChartMLChart
            {
                // Shapes that produce an error without fetching (named-single with
                // transform, named-multi). They render the error-only branch below.
                let has_initial_error = initial_error.is_some();
                if is_remote || has_initial_error {
                    // Remote / error path: show loading/error states, or ChartMLChart once data arrives
                    let can_retry = is_remote; // error-only shapes have no fetch to retry
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
                                                    {if can_retry {
                                                        view! {
                                                            <button
                                                                on:click=move |_| set_refresh_count.update(|c| *c += 1)
                                                                class="mt-2 text-xs text-primary underline transition-colors hover:text-primary/80"
                                                            >
                                                                "Retry"
                                                            </button>
                                                        }.into_any()
                                                    } else {
                                                        view! { <span /> }.into_any()
                                                    }}
                                                </div>
                                            </div>
                                        </div>
                                    }.into_any()
                                } else if remote_loading.get() {
                                    view! {
                                        <div class="flex flex-col items-center justify-center py-12 gap-3">
                                            <img src="/kyomi_animated_logo.svg" alt="Loading" class="w-8 h-8" />
                                            <span class="text-sm text-muted-foreground">"Loading chart..."</span>
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
                    // Inline / string-ref data path — delegate to ChartMLChart directly
                    view! {
                        <ChartMLChart
                            spec=spec_signal
                            chartml=chartml_for_inline
                        />
                    }.into_any()
                }
            }
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
    /// User's chart palette preference (e.g. "kyomi", "balanced", "vibrant", "accessible")
    #[prop(optional, into)]
    chart_palette: Option<String>,
    /// Additional CSS class(es) to apply to the prose wrapper div
    #[prop(optional, into)]
    class: Option<String>,
) -> impl IntoView {
    let palette_name = StoredValue::new(chart_palette.unwrap_or_else(|| "kyomi".to_string()));
    let extra_class = class.unwrap_or_default();

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

    view! {
        <div class=format!("prose-kyomi{}", if extra_class.is_empty() { String::new() } else { format!(" {extra_class}") })>
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
                        ContentSegment::ChartML { yamls, block_index } => {
                            let edit = edit_cb.get_value();
                            let delete = delete_cb.get_value();
                            let save = save_cb.get_value();
                            let info = info_cb.get_value();
                            let ask = ask_cb.get_value();
                            let palette_val = palette_name.get_value();
                            let chart_items = yamls
                                .into_iter()
                                .enumerate()
                                .map(|(array_index, chart_yaml)| {
                                    let col_span = serde_yaml::from_str::<serde_json::Value>(&chart_yaml)
                                        .ok()
                                        .as_ref()
                                        .map(extract_col_span)
                                        .unwrap_or(12);
                                    let col_class = chart_col_span_class(col_span);
                                    let palette = palette_val.clone();
                                    view! {
                                        <div class=col_class>
                                            <ChartBlock
                                                yaml=chart_yaml
                                                block_index=block_index
                                                array_index=array_index
                                                parameters=parameters
                                                on_edit_chart=edit
                                                on_delete_chart=delete
                                                on_save_to_dashboard=save
                                                on_chart_info=info
                                                on_ask_about_chart=ask
                                                chart_palette=palette
                                            />
                                        </div>
                                    }
                                })
                                .collect_view();
                            view! {
                                <div class="grid grid-cols-12 gap-4 my-2">{chart_items}</div>
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

// Tests for `kyomi_palette` and `kyomi_theme` live alongside their
// definitions in `crates/kyomi-chart-theme/src/lib.rs`.
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
        match &segments[1] {
            ContentSegment::ChartML { yamls, .. } => {
                assert_eq!(yamls.len(), 1);
                assert!(yamls[0].contains("type: bar"));
            }
            _ => panic!("expected ChartML segment"),
        }
        assert!(matches!(&segments[2], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_multiple_chartml_blocks() {
        let input =
            "Text\n```chartml\nchart1\n```\nMiddle\n```chartml\nchart2\n```\nEnd";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 5);
        assert!(matches!(&segments[0], ContentSegment::Markdown(_)));
        match &segments[1] {
            ContentSegment::ChartML { yamls, .. } => {
                assert_eq!(yamls.len(), 1);
                assert_eq!(yamls[0], "chart1");
            }
            _ => panic!("expected ChartML segment"),
        }
        assert!(matches!(&segments[2], ContentSegment::Markdown(_)));
        match &segments[3] {
            ContentSegment::ChartML { yamls, .. } => {
                assert_eq!(yamls.len(), 1);
                assert_eq!(yamls[0], "chart2");
            }
            _ => panic!("expected ChartML segment"),
        }
        assert!(matches!(&segments[4], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_chartml_at_start() {
        let input = "```chartml\nchart_data\n```\nAfter.";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 2);
        match &segments[0] {
            ContentSegment::ChartML { yamls, .. } => {
                assert_eq!(yamls.len(), 1);
                assert_eq!(yamls[0], "chart_data");
            }
            _ => panic!("expected ChartML segment"),
        }
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
        match &segments[0] {
            ContentSegment::ChartML { block_index, yamls } => {
                assert_eq!(*block_index, 0);
                assert_eq!(yamls.len(), 1);
            }
            _ => panic!("expected ChartML segment at index 0"),
        }
        match &segments[2] {
            ContentSegment::ChartML { block_index, yamls } => {
                assert_eq!(*block_index, 1);
                assert_eq!(yamls.len(), 1);
            }
            _ => panic!("expected ChartML segment at index 2"),
        }
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
        match &segments[1] {
            ContentSegment::ChartML { yamls, .. } => {
                assert_eq!(yamls.len(), 1);
                assert!(yamls[0].contains("type: chart"));
            }
            _ => panic!("expected ChartML segment"),
        }
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
    // ChartML array splitting + colSpan extraction
    // -----------------------------------------------------------------------

    #[test]
    fn test_chartml_array_block_splits_per_chart() {
        let input = "```chartml\n- type: chart\n  version: 1\n  title: A\n- type: chart\n  version: 1\n  title: B\n```";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 1);
        match &segments[0] {
            ContentSegment::ChartML { yamls, block_index } => {
                assert_eq!(*block_index, 0);
                assert_eq!(yamls.len(), 2);
                assert!(yamls[0].contains("title: A"));
                assert!(yamls[1].contains("title: B"));
                // Each item should contain its own type/version, not be the raw array
                assert!(!yamls[0].trim_start().starts_with('-'));
            }
            _ => panic!("expected ChartML segment"),
        }
    }

    #[test]
    fn test_extract_col_span() {
        let s: serde_json::Value = serde_json::json!({"layout": {"colSpan": 6}});
        assert_eq!(extract_col_span(&s), 6);
        let s2: serde_json::Value = serde_json::json!({"layout": {"col_span": 3}});
        assert_eq!(extract_col_span(&s2), 3);
        let s3: serde_json::Value = serde_json::json!({"layout": {"colSpan": 99}});
        assert_eq!(extract_col_span(&s3), 12); // out of range → default 12
        let s4: serde_json::Value = serde_json::json!({});
        assert_eq!(extract_col_span(&s4), 12);
    }

    // -----------------------------------------------------------------------
    // Named-source dispatch helper tests
    // -----------------------------------------------------------------------

    /// Helper to parse a YAML snippet into a serde_json::Value for tests.
    fn yaml(src: &str) -> serde_json::Value {
        serde_yaml::from_str(src).expect("test fixture must parse as YAML")
    }

    #[test]
    fn test_extract_named_sources_flat_remote_is_none() {
        let spec = yaml(
            "type: chart\n\
             data:\n  datasource: main_db\n  query: SELECT 1\n",
        );
        assert!(extract_named_sources(&spec).is_none());
    }

    #[test]
    fn test_extract_named_sources_inline_provider_is_none() {
        let spec = yaml(
            "type: chart\n\
             data:\n  provider: inline\n  rows:\n    - x: 1\n",
        );
        assert!(extract_named_sources(&spec).is_none());
    }

    #[test]
    fn test_extract_named_sources_string_ref_is_none() {
        let spec = yaml("type: chart\ndata: my_source\n");
        assert!(extract_named_sources(&spec).is_none());
    }

    #[test]
    fn test_extract_named_sources_empty_map_is_none() {
        let spec = yaml("type: chart\ndata: {}\n");
        assert!(extract_named_sources(&spec).is_none());
    }

    #[test]
    fn test_extract_named_sources_single() {
        let spec = yaml(
            "type: chart\n\
             data:\n  visitors:\n    datasource: main_db\n    query: SELECT * FROM hits\n",
        );
        let v = extract_named_sources(&spec).expect("one named source");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "visitors");
        assert_eq!(v[0].slug, "main_db");
        assert_eq!(v[0].query, "SELECT * FROM hits");
        assert_eq!(v[0].ttl, None);
    }

    #[test]
    fn test_extract_named_sources_multi_with_cache_ttl() {
        let spec = yaml(
            "type: chart\n\
             data:\n  visitors:\n    datasource: main_db\n    query: q1\n    cache:\n      ttl: 6h\n  revenue:\n    datasource: main_db\n    query: q2\n",
        );
        let v = extract_named_sources(&spec).expect("two named sources");
        assert_eq!(v.len(), 2);
        let by_name: HashMap<_, _> = v.iter().map(|s| (s.name.as_str(), s)).collect();
        assert_eq!(by_name["visitors"].ttl, Some(Duration::from_secs(6 * 3600)));
        assert_eq!(by_name["revenue"].ttl, None);
    }

    #[test]
    fn test_extract_named_sources_endpoint_url_aliases() {
        let spec = yaml(
            "type: chart\n\
             data:\n  sales:\n    endpoint: main_db\n    url: SELECT 1\n",
        );
        let v = extract_named_sources(&spec).expect("accepts endpoint+url aliases");
        assert_eq!(v[0].slug, "main_db");
        assert_eq!(v[0].query, "SELECT 1");
    }

    #[test]
    fn test_extract_named_sources_missing_slug_returns_none() {
        let spec = yaml(
            "type: chart\n\
             data:\n  visitors:\n    query: SELECT 1\n",
        );
        assert!(extract_named_sources(&spec).is_none());
    }

    #[test]
    fn test_parse_ttl_units() {
        assert_eq!(parse_ttl("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_ttl("45s"), Some(Duration::from_secs(45)));
        assert_eq!(parse_ttl("30m"), Some(Duration::from_secs(30 * 60)));
        assert_eq!(parse_ttl("6h"), Some(Duration::from_secs(6 * 3600)));
        assert_eq!(parse_ttl("1d"), Some(Duration::from_secs(86_400)));
    }

    #[test]
    fn test_parse_ttl_case_insensitive() {
        assert_eq!(parse_ttl("6H"), Some(Duration::from_secs(6 * 3600)));
        assert_eq!(parse_ttl("500MS"), Some(Duration::from_millis(500)));
    }

    #[test]
    fn test_parse_ttl_rejects_invalid() {
        assert_eq!(parse_ttl(""), None);
        assert_eq!(parse_ttl("abc"), None);
        assert_eq!(parse_ttl("5"), None); // no unit
        assert_eq!(parse_ttl("1.5h"), None); // fractions rejected
        assert_eq!(parse_ttl("-3s"), None);
        assert_eq!(parse_ttl("5 minutes"), None);
    }

    #[test]
    fn test_has_transform_true_when_non_empty_map() {
        let spec = yaml("type: chart\ntransform:\n  sql: SELECT 1\n");
        assert!(has_transform(&spec));
    }

    #[test]
    fn test_has_transform_false_when_missing() {
        let spec = yaml("type: chart\n");
        assert!(!has_transform(&spec));
    }

    #[test]
    fn test_has_transform_false_when_empty_map() {
        let spec = yaml("type: chart\ntransform: {}\n");
        assert!(!has_transform(&spec));
    }

    #[test]
    fn test_has_transform_false_when_null() {
        let spec = yaml("type: chart\ntransform: null\n");
        assert!(!has_transform(&spec));
    }

    #[test]
    fn test_classify_chart_data_string_ref() {
        let spec = yaml("type: chart\ndata: sales\n");
        assert_eq!(classify_chart_data(&spec), ChartDataShape::StringRef);
    }

    #[test]
    fn test_classify_chart_data_inline_provider() {
        let spec = yaml("type: chart\ndata:\n  provider: inline\n  rows:\n    - x: 1\n");
        assert_eq!(classify_chart_data(&spec), ChartDataShape::Inline);
    }

    #[test]
    fn test_classify_chart_data_inline_rows_only() {
        let spec = yaml("type: chart\ndata:\n  rows:\n    - x: 1\n    - x: 2\n");
        assert_eq!(classify_chart_data(&spec), ChartDataShape::Inline);
    }

    #[test]
    fn test_classify_chart_data_flat_remote() {
        let spec = yaml(
            "type: chart\ndata:\n  datasource: main_db\n  query: SELECT 1\n",
        );
        assert_eq!(classify_chart_data(&spec), ChartDataShape::FlatRemote);
    }

    #[test]
    fn test_classify_chart_data_named_single_no_transform() {
        let spec = yaml(
            "type: chart\n\
             data:\n  visitors:\n    datasource: main_db\n    query: SELECT 1\n",
        );
        assert_eq!(
            classify_chart_data(&spec),
            ChartDataShape::NamedSingleNoTransform
        );
    }

    #[test]
    fn test_classify_chart_data_named_single_with_transform() {
        let spec = yaml(
            "type: chart\n\
             data:\n  visitors:\n    datasource: main_db\n    query: SELECT 1\n\
             transform:\n  sql: SELECT * FROM visitors\n",
        );
        assert_eq!(
            classify_chart_data(&spec),
            ChartDataShape::NamedSingleWithTransform
        );
    }

    #[test]
    fn test_classify_chart_data_named_multi_no_transform() {
        let spec = yaml(
            "type: chart\n\
             data:\n  a:\n    datasource: db\n    query: q1\n  b:\n    datasource: db\n    query: q2\n",
        );
        assert_eq!(
            classify_chart_data(&spec),
            ChartDataShape::NamedMultiNoTransform
        );
    }

    #[test]
    fn test_classify_chart_data_named_multi_with_transform() {
        let spec = yaml(
            "type: chart\n\
             data:\n  a:\n    datasource: db\n    query: q1\n  b:\n    datasource: db\n    query: q2\n\
             transform:\n  sql: SELECT * FROM a JOIN b USING(id)\n",
        );
        assert_eq!(
            classify_chart_data(&spec),
            ChartDataShape::NamedMultiWithTransform
        );
    }

    #[test]
    fn test_classify_chart_data_missing_data_is_unknown() {
        let spec = yaml("type: chart\ntitle: X\n");
        assert_eq!(classify_chart_data(&spec), ChartDataShape::Unknown);
    }

    #[test]
    fn test_classify_chart_data_malformed_named_map_is_unknown() {
        // Entry value is a scalar, not a map — not a valid named-sources shape.
        let spec = yaml("type: chart\ndata:\n  foo: bar\n");
        assert_eq!(classify_chart_data(&spec), ChartDataShape::Unknown);
    }
}
