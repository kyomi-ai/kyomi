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
    // instance with the data registered as a named source ("_remote") and rewrite the YAML
    // `data:` section to reference it. This lets ChartMLChart handle everything uniformly.
    let (remote_chartml, set_remote_chartml) = signal(None::<Arc<ChartML>>);
    let (remote_error, set_remote_error) = signal(None::<String>);
    let (remote_loading, set_remote_loading) = signal(is_remote);

    if is_remote {
        let ds_slug = datasource_slug.clone();
        let sql = sql_query.clone();
        let palette_for_remote = palette.clone();
        let is_dark_for_remote = is_dark;

        Effect::new(move || {
            let params = parameters.get();
            let _refresh = refresh_count.get();
            let ds = ds_slug.clone();
            let q = sql.clone();
            let pal = palette_for_remote.clone();
            let is_dark = is_dark_for_remote;

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
                        let instance = configured_chartml(&pal, is_dark);
                        // Safety: Arc::get_mut works here because we just created it and
                        // hold the only reference. We need to register the source before
                        // wrapping it.
                        let mut chartml_mut = ChartML::new();
                        // Re-register all renderers + transform + palette + theme on the
                        // mutable instance — must stay in sync with `configured_chartml`.
                        let colors = kyomi_palette(&pal, is_dark);
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

    // Set initial "Last refreshed" timestamp for inline charts (rendered immediately on mount)
    if !is_remote {
        set_last_refreshed.set(Some(js_sys::Date::now()));
    }

    // RwSignal for ChartMLChart spec — updated by Effect when overrides change.
    let (chartml_spec, set_chartml_spec) = signal(yaml_owned.clone());
    Effect::new(move || {
        let base_spec = effective_spec.get();

        let final_spec = if !is_remote {
            base_spec
        } else {
            // Rewrite the data section to reference the named "_remote" source
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
                                                    class="mt-2 text-xs text-primary underline transition-colors hover:text-primary/80"
                                                >
                                                    "Retry"
                                                </button>
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
        // Single outer grid lets adjacent chartml blocks share rows; each
        // segment picks its own col-span so two `colSpan: 6` charts in
        // separate ```chartml``` fences render side-by-side instead of in
        // isolated 12-column grids. Prose typography rules still apply
        // inside each Markdown segment's inner_html div.
        <div class=format!("prose-kyomi grid grid-cols-12 gap-4{}", if extra_class.is_empty() { String::new() } else { format!(" {extra_class}") })>
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
                                <div class="col-span-12" inner_html=html></div>
                            }.into_any()
                        }
                        ContentSegment::CodeBlock { language, code } => {
                            view! {
                                <div class="col-span-12">
                                    <CodeBlockView language=language code=code />
                                </div>
                            }.into_any()
                        }
                        ContentSegment::ChartML { yamls, block_index } => {
                            let edit = edit_cb.get_value();
                            let delete = delete_cb.get_value();
                            let save = save_cb.get_value();
                            let info = info_cb.get_value();
                            let ask = ask_cb.get_value();
                            let palette_val = palette_name.get_value();
                            // Emit each chart as its own grid item directly into the
                            // outer grid (no nested `grid grid-cols-12` wrapper) so
                            // siblings from different ChartML segments can share rows.
                            yamls
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
                                .collect_view()
                                .into_any()
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

    /// Regression guard for KYO-95: two adjacent ```chartml``` fences with
    /// `layout.colSpan: 6` must parse into two separate ChartML segments,
    /// each carrying a single yaml whose colSpan is 6. The renderer flattens
    /// these into the outer 12-column grid, so both end up as
    /// `col-span-12 md:col-span-6` siblings sharing one row instead of
    /// occupying isolated grids.
    #[test]
    fn test_adjacent_chartml_blocks_with_col_span() {
        let input = "\
```chartml
- type: bar
  layout:
    colSpan: 6
  data: a
```

```chartml
- type: line
  layout:
    colSpan: 6
  data: b
```";
        let segments = parse_segments(input);
        let chartml_segments: Vec<_> = segments
            .iter()
            .filter_map(|s| match s {
                ContentSegment::ChartML { yamls, .. } => Some(yamls),
                _ => None,
            })
            .collect();
        assert_eq!(
            chartml_segments.len(),
            2,
            "two adjacent chartml blocks should produce two segments"
        );
        for yamls in &chartml_segments {
            assert_eq!(yamls.len(), 1, "each block contains a single chart");
            let val: serde_json::Value = serde_yaml::from_str(&yamls[0]).unwrap();
            assert_eq!(
                extract_col_span(&val),
                6,
                "each chart's colSpan should parse as 6"
            );
        }
    }
}
