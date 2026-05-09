// SPDX-License-Identifier: AGPL-3.0-or-later

//! Markdown + ChartML renderer component.
//!
//! Splits content into alternating Markdown, code-block, and ChartML segments.
//! Markdown is rendered via `pulldown-cmark`, code blocks get syntax-styled
//! `<pre><code>` with a copy button, and ChartML blocks are delegated to
//! [`super::kyomi_chart::KyomiChart`] — the single source of truth for
//! per-chart state, override signals, header chrome, and the
//! `chartml_leptos::ChartMLChart` call that dispatches every `data:` shape
//! (inline, flat-remote, named-single, named-multi + transform) through the
//! chartml 5.0 resolver. Kyomi installs its `KyomiDatasourceProvider` via
//! Leptos context at the dashboard root so any chart spec carrying a
//! `datasource` slug + `query` flows through Kyomi's auth + datasource
//! plumbing transparently.

use std::collections::HashMap;

use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_chart_table::TableRenderer;
use chartml_core::spec::VisualizeSpec;
use chartml_core::ChartRenderer;
// `kyomi_palette` and `kyomi_theme` live in the tiny `kyomi-chart-theme`
// crate so they're accessible from both the browser rendering path
// (kyomi-ui compiled for wasm32) and the SSR rendering path (kyomi-agent
// via chartml_factory → PDF export, email snapshots, MCP chart app). The
// crate has zero server-only dependencies, so it compiles cleanly under
// the `hydrate` feature. Re-exported here so existing callers in this
// file don't have to touch their import paths.
#[cfg(target_arch = "wasm32")]
pub(crate) use kyomi_chart_theme::{kyomi_palette, kyomi_theme};
use crate::chartml_provider::configured_chartml;
use super::kyomi_chart::KyomiChart;
use leptos::prelude::*;

// ---------------------------------------------------------------------------
// Content segmentation
// ---------------------------------------------------------------------------

/// A segment of dashboard content.
#[derive(Clone, Debug, PartialEq, Hash)]
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
pub(crate) fn split_chartml_block(block_content: &str) -> Vec<String> {
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

/// Splice a single chart item back into a chartml block.
///
/// - If `block_content` is a single YAML mapping, `array_index` MUST be `0`
///   and the block is replaced wholesale with `new_item_yaml`.
/// - If `block_content` is a YAML sequence, the `array_index`-th entry is
///   replaced with `new_item_yaml`'s parsed value and the sequence is
///   re-serialized as YAML.
/// - If `block_content` fails to parse, or the index is out of range for a
///   sequence (or non-zero for a mapping), returns `None`.
///
/// The returned string is a YAML document ready to be substituted back into
/// the text between the opening ```` ```chartml ```` fence and the closing
/// ```` ``` ```` fence. It does NOT include fence lines.
pub(crate) fn splice_chartml_item(
    block_content: &str,
    array_index: usize,
    new_item_yaml: &str,
) -> Option<String> {
    let parsed: serde_json::Value = serde_yaml::from_str(block_content).ok()?;
    match parsed {
        serde_json::Value::Array(mut items) => {
            if array_index >= items.len() {
                return None;
            }
            let new_item: serde_json::Value =
                serde_yaml::from_str(new_item_yaml).ok()?;
            items[array_index] = new_item;
            let replacement = serde_json::Value::Array(items);
            serde_yaml::to_string(&replacement).ok()
        }
        serde_json::Value::Object(_) => {
            if array_index != 0 {
                return None;
            }
            Some(new_item_yaml.to_string())
        }
        // Null / scalar / etc. — not a valid chartml block shape.
        _ => None,
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

/// Replace `{{param_id}}` placeholders with parameter values.
///
/// Operates on raw text — when applied to a full ChartML YAML spec, every
/// occurrence of `{{key}}` (in `query:` fields, `title:`, anywhere else) is
/// substituted. This matches the legacy behavior of the React frontend and
/// keeps existing dashboard templates working unchanged through the
/// chartml 5.0 cutover.
pub(crate) fn substitute_params(yaml: &str, params: &HashMap<String, String>) -> String {
    let mut result = yaml.to_string();
    for (key, value) in params {
        result = result.replace(&format!("{{{{{key}}}}}"), value);
    }
    result
}

// ---------------------------------------------------------------------------
// ChartML spec extraction helpers
// ---------------------------------------------------------------------------
// Used to populate the `ChartHeaderBar` selectors. The chart title itself is
// extracted by chartml-leptos directly from the YAML it receives, so the
// renderer doesn't surface it here.

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

/// Extract the explicit chart height from `visualize.style.height` in the YAML spec.
///
/// Returns `None` if the spec omits it — callers should fall back to the
/// per-type renderer default (via [`default_chart_height_for_type`]) so the
/// loading placeholder reserves the same vertical space the rendered chart
/// will occupy, preventing layout shift on data-arrival.
pub(crate) fn extract_chart_height(spec: &serde_json::Value) -> Option<f64> {
    spec.get("visualize")
        .and_then(|v| v.get("style"))
        .and_then(|s| s.get("height"))
        .and_then(|h| h.as_f64())
}

/// Hard fallback when we can't look up a per-type default — matches the
/// historical blanket default used before KYO-118 and the `height` returned
/// by every non-metric built-in renderer today.
const GENERIC_CHART_HEIGHT_PX: f64 = 400.0;

/// Return the built-in default height (in pixels) for the given chart type
/// by asking the matching concrete renderer. Used when the YAML spec omits
/// an explicit `visualize.style.height` so the outer wrapper reserves the
/// same height the rendered chart will occupy — metric cards are 150px,
/// everything else is 400px.
///
/// TODO(KYO-109): The render-pipeline unification ticket should move this
/// reserved-height calc into `chartml-core` (or a shared Kyomi helper) so
/// the Leptos consumer doesn't have to know which concrete renderer backs
/// each chart type. Today we mirror `configured_chartml`'s type → renderer
/// mapping by hand.
pub(crate) fn default_chart_height_for_type(chart_type: Option<&str>) -> f64 {
    // Ask the concrete renderer for its own default. The renderers'
    // `default_dimensions` currently ignores the spec arg, but we pass a
    // minimal `VisualizeSpec` (typed to match the renderer) so future
    // spec-aware defaults work without another round of wiring.
    let chart_type = match chart_type {
        Some(t) => t,
        None => return GENERIC_CHART_HEIGHT_PX,
    };
    let viz: VisualizeSpec = match serde_json::from_value(serde_json::json!({
        "type": chart_type,
    })) {
        Ok(v) => v,
        Err(_) => return GENERIC_CHART_HEIGHT_PX,
    };
    let dims = match chart_type {
        "bar" | "line" | "area" => CartesianRenderer::new().default_dimensions(&viz),
        "pie" | "donut" | "doughnut" => PieRenderer::new().default_dimensions(&viz),
        "scatter" => ScatterRenderer::new().default_dimensions(&viz),
        "metric" => MetricRenderer::new().default_dimensions(&viz),
        "table" => TableRenderer::new().default_dimensions(&viz),
        _ => None,
    };
    dims.map(|d| d.height).unwrap_or(GENERIC_CHART_HEIGHT_PX)
}

/// Extract `layout.colSpan` (or snake_case `col_span`) from a parsed spec,
/// clamped to 1..=12. Defaults to 12 when missing / invalid.
pub(crate) fn extract_col_span(spec: &serde_json::Value) -> u8 {
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
pub(crate) fn chart_col_span_class(col_span: u8) -> &'static str {
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

/// Set `layout.colSpan` in a per-item ChartML YAML string.
///
/// Parses the YAML, sets or creates the `layout.colSpan` field, and
/// re-serializes. Returns `None` if the YAML cannot be parsed or serialized.
///
/// Used by the `chart-resize-request` handler in the dashboard editor to
/// persist width changes made via the WYSIWYG drag-to-resize handles.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn set_col_span(yaml: &str, new_col_span: u8) -> Option<String> {
    let mut spec: serde_json::Value = serde_yaml::from_str(yaml).ok()?;
    let layout = spec
        .as_object_mut()?
        .entry("layout")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let layout_obj = layout.as_object_mut()?;
    layout_obj.insert(
        "colSpan".to_string(),
        serde_json::Value::Number(serde_json::Number::from(new_col_span)),
    );
    serde_yaml::to_string(&spec).ok()
}

/// Set `visualize.style.height` in a per-item ChartML YAML string.
///
/// Parses the YAML, sets or creates the `visualize.style.height` field, and
/// re-serializes. Returns `None` if the YAML cannot be parsed or serialized.
///
/// Used by the `chart-resize-request` handler in the dashboard editor to
/// persist height changes made via the WYSIWYG drag-to-resize handles.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn set_chart_height(yaml: &str, new_height: f64) -> Option<String> {
    let mut spec: serde_json::Value = serde_yaml::from_str(yaml).ok()?;
    let spec_obj = spec.as_object_mut()?;
    let visualize = spec_obj
        .entry("visualize")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let viz_obj = visualize.as_object_mut()?;
    let style = viz_obj
        .entry("style")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let style_obj = style.as_object_mut()?;
    // Store as integer when the value is whole-number to keep YAML clean
    let height_val = if new_height.fract() == 0.0 {
        serde_json::Value::Number(serde_json::Number::from(new_height as i64))
    } else {
        serde_json::Number::from_f64(new_height)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Number(serde_json::Number::from(
                new_height as i64,
            )))
    };
    style_obj.insert("height".to_string(), height_val);
    serde_yaml::to_string(&spec).ok()
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
                set_copied.try_set(true);
                gloo_timers::future::TimeoutFuture::new(2000).await;
                set_copied.try_set(false);
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
    /// Optional workspace UUID. When non-empty and no
    /// `chartml_leptos::ProviderRef` is already in Leptos context,
    /// `MarkdownRenderer` installs a `KyomiDatasourceProvider` (and the
    /// matching IndexedDB cache backend signal) scoped to that workspace via
    /// [`crate::chartml_provider::provide_chart_context`]. Lets callers that
    /// mount outside `DashboardChartProviders` (e.g. the Watches alerts +
    /// execution-log viewers — KYO-119) render chartml blocks with
    /// `data: { datasource, query }` without patching each callsite
    /// individually. When the caller doesn't pass it, passes an empty
    /// string, or a provider is already in context (the Dashboard
    /// viewer/editor case), this prop is a no-op — preserving today's
    /// behavior for every existing caller.
    ///
    /// Empty string is the "no id" sentinel because Leptos' `#[prop(optional,
    /// into)]` on `String` already wraps the caller's value in `Some` for
    /// the internal field, so we can't distinguish "caller passed nothing"
    /// from "caller passed `None`" at runtime — treating `""` as "skip"
    /// gives callers a single knob that matches `UserContext.workspace_id
    /// .unwrap_or_default()` naturally.
    #[prop(optional, into)]
    workspace_id: String,
) -> impl IntoView {
    // Register the chart provider + cache backend for this subtree when the
    // caller asks for it and nothing higher up has already done so. Dashboards
    // wire `DashboardChartProviders` above `MarkdownRenderer`, so they keep
    // that single shared provider + cache. Watches/other hosts that mount the
    // renderer in isolation pass `workspace_id` and we install the same
    // context entries locally — same plumbing, just narrower scope.
    //
    // Called at component-body scope (before the view tree is built) so the
    // `provide_context` entries are visible to every descendant
    // `ChartMLChart` via `use_context`.
    if !workspace_id.is_empty()
        && use_context::<chartml_leptos::ProviderRef>().is_none()
    {
        crate::chartml_provider::provide_chart_context(&workspace_id);
    }

    let palette_name = chart_palette.unwrap_or_else(|| "kyomi".to_string());
    let theme_state = crate::components::theme::use_theme();
    // Read once without tracking — charts render with the correct palette on
    // mount but don't live-update on theme toggle (page refresh required).
    // Using get_untracked avoids the double-render that occurred when a
    // tracked Memo fired twice on page load (default then localStorage).
    let is_dark = theme_state
        .map(|s| s.effective.get_untracked() == "dark")
        .unwrap_or(false);
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
                key=|(i, seg)| {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    i.hash(&mut hasher);
                    seg.hash(&mut hasher);
                    hasher.finish()
                }
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
                            // Emit each chart as its own grid item directly into the
                            // outer grid (no nested `grid grid-cols-12` wrapper) so
                            // siblings from different ChartML segments can share rows.
                            let palette_for_charts = palette_name.clone();
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
                                    let palette_clone = palette_for_charts.clone();
                                    let chartml = configured_chartml(&palette_clone, is_dark);
                                    view! {
                                        <div class=col_class>
                                            <KyomiChart
                                                yaml=chart_yaml.clone()
                                                block_index=block_index
                                                array_index=array_index
                                                parameters=parameters
                                                chartml=chartml
                                                on_edit_chart=edit
                                                on_delete_chart=delete
                                                on_save_to_dashboard=save
                                                on_chart_info=info
                                                on_ask_about_chart=ask
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
    // Substitution end-to-end (whole-spec text replacement)
    // -----------------------------------------------------------------------

    #[test]
    fn test_substitute_params_inside_yaml_query() {
        // Verifies that placeholder substitution still works when applied to
        // the full YAML text — `{{region}}` inside a `query:` value gets
        // replaced exactly the way it did in the bespoke fetch path.
        let yaml = "data:\n  datasource: db\n  query: SELECT * FROM t WHERE r = '{{region}}'\n";
        let mut params = HashMap::new();
        params.insert("region".to_string(), "US".to_string());
        let out = substitute_params(yaml, &params);
        assert!(out.contains("WHERE r = 'US'"));
        assert!(!out.contains("{{region}}"));
    }

    #[test]
    fn test_substitute_params_in_title() {
        let yaml = "title: Sales for {{year}}\n";
        let mut params = HashMap::new();
        params.insert("year".to_string(), "2026".to_string());
        let out = substitute_params(yaml, &params);
        assert_eq!(out, "title: Sales for 2026\n");
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

// ---------------------------------------------------------------------------
// default_chart_height_for_type — guards metric-card layout shift (KYO-118)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod default_height_tests {
    use super::*;

    #[test]
    fn metric_returns_150_not_generic_400() {
        // Regression guard for KYO-118: metric cards were reserving 400px,
        // then the MetricRenderer rendered into 150px, causing layout shift.
        assert_eq!(default_chart_height_for_type(Some("metric")), 150.0);
    }

    #[test]
    fn non_metric_types_return_400() {
        for t in ["bar", "line", "area", "pie", "donut", "scatter", "table"] {
            assert_eq!(default_chart_height_for_type(Some(t)), 400.0, "type: {t}");
        }
    }

    #[test]
    fn unknown_and_missing_types_fall_back_to_generic() {
        assert_eq!(default_chart_height_for_type(None), 400.0);
        assert_eq!(default_chart_height_for_type(Some("not-a-chart")), 400.0);
    }
}

// ---------------------------------------------------------------------------
// split_chartml_block — focused coverage
// ---------------------------------------------------------------------------

#[cfg(test)]
mod split_tests {
    use super::*;

    #[test]
    fn mapping_returns_single_item() {
        let block = "type: bar\ntitle: A\n";
        let out = split_chartml_block(block);
        assert_eq!(out.len(), 1);
        // Mapping is preserved verbatim (split doesn't re-serialize it).
        assert_eq!(out[0], block);
    }

    #[test]
    fn sequence_returns_one_string_per_item() {
        let block = "- type: bar\n  title: A\n- type: line\n  title: B\n- type: pie\n  title: C\n";
        let out = split_chartml_block(block);
        assert_eq!(out.len(), 3);
        // Each item re-serialized as its own mapping (no leading `-`).
        for item in &out {
            assert!(!item.trim_start().starts_with('-'));
        }
        assert!(out[0].contains("title: A"));
        assert!(out[1].contains("title: B"));
        assert!(out[2].contains("title: C"));
    }

    #[test]
    fn malformed_yaml_returns_original() {
        let block = "type: [unclosed";
        let out = split_chartml_block(block);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], block);
    }

    #[test]
    fn empty_string_returns_original() {
        // Empty string parses as null; fallback returns the original content.
        let out = split_chartml_block("");
        assert_eq!(out, vec!["".to_string()]);
    }
}

// ---------------------------------------------------------------------------
// splice_chartml_item — round-trip and edge cases
// ---------------------------------------------------------------------------

#[cfg(test)]
mod splice_tests {
    use super::*;

    fn parse(yaml: &str) -> serde_json::Value {
        serde_yaml::from_str(yaml).expect("yaml parses")
    }

    #[test]
    fn mapping_index_zero_replaces_whole_block() {
        let block = "type: bar\ntitle: Old\n";
        let new_item = "type: line\ntitle: New\n";
        let out = splice_chartml_item(block, 0, new_item).expect("mapping @ 0");
        // The mapping case returns the new item verbatim.
        assert_eq!(out, new_item);
    }

    #[test]
    fn mapping_nonzero_index_returns_none() {
        let block = "type: bar\ntitle: Old\n";
        let new_item = "type: line\ntitle: New\n";
        assert!(splice_chartml_item(block, 1, new_item).is_none());
    }

    #[test]
    fn sequence_replaces_only_target_index() {
        let block = "- type: bar\n  title: A\n- type: line\n  title: B\n- type: pie\n  title: C\n";
        let new_item = "type: scatter\ntitle: B2\n";
        let out = splice_chartml_item(block, 1, new_item).expect("seq @ 1");

        // Re-parse and assert structurally — don't compare bytes.
        let parsed = parse(&out);
        let arr = parsed.as_array().expect("array");
        assert_eq!(arr.len(), 3);

        assert_eq!(arr[0]["title"].as_str(), Some("A"));
        assert_eq!(arr[0]["type"].as_str(), Some("bar"));

        assert_eq!(arr[1]["title"].as_str(), Some("B2"));
        assert_eq!(arr[1]["type"].as_str(), Some("scatter"));

        assert_eq!(arr[2]["title"].as_str(), Some("C"));
        assert_eq!(arr[2]["type"].as_str(), Some("pie"));
    }

    #[test]
    fn sequence_out_of_range_returns_none() {
        let block = "- type: bar\n  title: A\n- type: line\n  title: B\n- type: pie\n  title: C\n";
        let new_item = "type: scatter\ntitle: X\n";
        assert!(splice_chartml_item(block, 5, new_item).is_none());
    }

    #[test]
    fn malformed_block_returns_none() {
        let block = "type: [unclosed";
        let new_item = "type: bar\n";
        assert!(splice_chartml_item(block, 0, new_item).is_none());
    }

    #[test]
    fn malformed_new_item_in_sequence_returns_none() {
        // Sequence + unparseable new item must fail, not corrupt the block.
        let block = "- type: bar\n- type: line\n";
        let malformed_new = "type: [unclosed";
        assert!(splice_chartml_item(block, 0, malformed_new).is_none());
    }

    #[test]
    fn round_trip_split_then_splice_preserves_structure() {
        // Take a sequence, split it, splice each item back at its own index,
        // and verify every intermediate parses to the same structure as the
        // original. We re-parse rather than compare strings because YAML
        // re-serialization may reorder keys or normalize whitespace.
        let block = "- type: bar\n  title: A\n  version: 1\n- type: line\n  title: B\n  version: 1\n- type: pie\n  title: C\n  version: 1\n";
        let items = split_chartml_block(block);
        assert_eq!(items.len(), 3);

        let original_parsed = parse(block);

        for (i, item) in items.iter().enumerate() {
            let spliced = splice_chartml_item(block, i, item)
                .unwrap_or_else(|| panic!("splice @ {i}"));
            let spliced_parsed = parse(&spliced);
            assert_eq!(
                spliced_parsed, original_parsed,
                "round-trip at index {i} should parse identically",
            );
        }
    }

    // ── set_col_span tests ───────────────────────────────────────────────────

    #[test]
    fn set_col_span_creates_layout_section_when_absent() {
        let yaml = "type: bar\ntitle: My Chart\n";
        let result = set_col_span(yaml, 6).expect("should succeed");
        let v: serde_json::Value = serde_yaml::from_str(&result).unwrap();
        assert_eq!(v["layout"]["colSpan"], 6);
    }

    #[test]
    fn set_col_span_updates_existing_col_span() {
        let yaml = "type: bar\nlayout:\n  colSpan: 12\n";
        let result = set_col_span(yaml, 4).expect("should succeed");
        let v: serde_json::Value = serde_yaml::from_str(&result).unwrap();
        assert_eq!(v["layout"]["colSpan"], 4);
    }

    #[test]
    fn set_col_span_preserves_other_layout_fields() {
        let yaml = "type: bar\nlayout:\n  colSpan: 12\n  align: center\n";
        let result = set_col_span(yaml, 6).expect("should succeed");
        let v: serde_json::Value = serde_yaml::from_str(&result).unwrap();
        assert_eq!(v["layout"]["colSpan"], 6);
        assert_eq!(v["layout"]["align"], "center");
    }

    #[test]
    fn set_col_span_returns_none_on_invalid_yaml() {
        assert!(set_col_span("type: [unclosed", 6).is_none());
    }

    // ── set_chart_height tests ───────────────────────────────────────────────

    #[test]
    fn set_chart_height_creates_visualize_style_when_absent() {
        let yaml = "type: bar\ntitle: My Chart\n";
        let result = set_chart_height(yaml, 300.0).expect("should succeed");
        let v: serde_json::Value = serde_yaml::from_str(&result).unwrap();
        assert_eq!(v["visualize"]["style"]["height"], 300);
    }

    #[test]
    fn set_chart_height_updates_existing_height() {
        let yaml = "type: bar\nvisualize:\n  style:\n    height: 200\n";
        let result = set_chart_height(yaml, 350.0).expect("should succeed");
        let v: serde_json::Value = serde_yaml::from_str(&result).unwrap();
        assert_eq!(v["visualize"]["style"]["height"], 350);
    }

    #[test]
    fn set_chart_height_stores_whole_number_as_integer() {
        // Whole-number heights should serialize as integers, not floats.
        let yaml = "type: bar\n";
        let result = set_chart_height(yaml, 400.0).expect("should succeed");
        // The YAML should contain "400" not "400.0"
        assert!(result.contains("400"), "expected integer in output: {result}");
        assert!(!result.contains("400.0"), "unexpected float in output: {result}");
    }

    #[test]
    fn set_chart_height_preserves_other_style_fields() {
        let yaml = "type: bar\nvisualize:\n  style:\n    height: 200\n    color: red\n";
        let result = set_chart_height(yaml, 300.0).expect("should succeed");
        let v: serde_json::Value = serde_yaml::from_str(&result).unwrap();
        assert_eq!(v["visualize"]["style"]["height"], 300);
        assert_eq!(v["visualize"]["style"]["color"], "red");
    }

    #[test]
    fn set_chart_height_returns_none_on_invalid_yaml() {
        assert!(set_chart_height("type: [unclosed", 300.0).is_none());
    }
}
