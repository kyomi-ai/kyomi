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
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_core::element::ChartElement;
use chartml_core::ChartML;
use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

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
// ChartML instance factory
// ---------------------------------------------------------------------------

/// Create a configured ChartML instance with all chart renderers registered.
fn create_chartml() -> Arc<ChartML> {
    let mut chartml = ChartML::new();
    chartml.register_renderer("bar", CartesianRenderer::new());
    chartml.register_renderer("line", CartesianRenderer::new());
    chartml.register_renderer("area", CartesianRenderer::new());
    chartml.register_renderer("pie", PieRenderer::new());
    chartml.register_renderer("donut", PieRenderer::new());
    chartml.register_renderer("scatter", ScatterRenderer::new());
    chartml.register_renderer("metric", MetricRenderer::new());
    Arc::new(chartml)
}

// ---------------------------------------------------------------------------
// ChartElement → Leptos SVG view
// ---------------------------------------------------------------------------

/// Recursively render a `ChartElement` tree into Leptos view nodes.
///
/// Handles all variants: Svg, Group, Rect, Path, Circle, Line, Text, Div, Span.
fn render_chart_element(element: &ChartElement) -> AnyView {
    match element {
        ChartElement::Svg {
            viewbox,
            width: _,
            height,
            class,
            children,
        } => {
            let viewbox_str = viewbox.to_string();
            let class = class.clone();
            let height_str = height.map(|h| h.to_string()).unwrap_or_default();
            let children_views: Vec<AnyView> =
                children.iter().map(render_chart_element).collect();

            view! {
                <svg
                    viewBox=viewbox_str
                    width="100%"
                    height=height_str
                    class=class
                    style="overflow: visible; display: block;"
                >
                    {children_views}
                </svg>
            }
            .into_any()
        }

        ChartElement::Group {
            class,
            transform,
            children,
        } => {
            let class = class.clone();
            let transform_str = transform
                .as_ref()
                .map(|t| t.to_svg_string())
                .unwrap_or_default();
            let children_views: Vec<AnyView> =
                children.iter().map(render_chart_element).collect();

            view! {
                <g class=class transform=transform_str>
                    {children_views}
                </g>
            }
            .into_any()
        }

        ChartElement::Rect {
            x,
            y,
            width,
            height,
            fill,
            stroke,
            class,
            ..
        } => {
            let x_str = x.to_string();
            let y_str = y.to_string();
            let w_str = width.to_string();
            let h_str = height.to_string();
            let fill = fill.clone();
            let stroke_str = stroke.clone().unwrap_or_default();
            let class = class.clone();

            view! {
                <rect
                    x=x_str y=y_str width=w_str height=h_str
                    fill=fill stroke=stroke_str class=class
                />
            }
            .into_any()
        }

        ChartElement::Path {
            d,
            fill,
            stroke,
            stroke_width,
            stroke_dasharray,
            opacity,
            class,
            ..
        } => {
            let d = d.clone();
            let fill_str = fill.clone().unwrap_or_else(|| "none".to_string());
            let stroke_str = stroke.clone().unwrap_or_else(|| "none".to_string());
            let sw = stroke_width.map(|w| w.to_string()).unwrap_or_default();
            let sda = stroke_dasharray.clone().unwrap_or_default();
            let op = opacity.map(|o| o.to_string()).unwrap_or_default();
            let class = class.clone();

            view! {
                <path
                    d=d fill=fill_str stroke=stroke_str
                    stroke-width=sw stroke-dasharray=sda opacity=op class=class
                />
            }
            .into_any()
        }

        ChartElement::Circle {
            cx,
            cy,
            r,
            fill,
            stroke,
            class,
            ..
        } => {
            let cx_str = cx.to_string();
            let cy_str = cy.to_string();
            let r_str = r.to_string();
            let fill = fill.clone();
            let stroke_str = stroke.clone().unwrap_or_default();
            let class = class.clone();

            view! {
                <circle
                    cx=cx_str cy=cy_str r=r_str
                    fill=fill stroke=stroke_str class=class
                />
            }
            .into_any()
        }

        ChartElement::Line {
            x1,
            y1,
            x2,
            y2,
            stroke,
            stroke_width,
            stroke_dasharray,
            class,
        } => {
            let x1 = x1.to_string();
            let y1 = y1.to_string();
            let x2 = x2.to_string();
            let y2 = y2.to_string();
            let stroke = stroke.clone();
            let sw = stroke_width.map(|w| w.to_string()).unwrap_or_default();
            let sda = stroke_dasharray.clone().unwrap_or_default();
            let class = class.clone();

            view! {
                <line
                    x1=x1 y1=y1 x2=x2 y2=y2
                    stroke=stroke stroke-width=sw stroke-dasharray=sda class=class
                />
            }
            .into_any()
        }

        ChartElement::Text {
            x,
            y,
            content,
            anchor,
            dominant_baseline,
            transform,
            font_size,
            font_weight,
            fill,
            class,
            ..
        } => {
            let x = x.to_string();
            let y = y.to_string();
            let content = content.clone();
            let anchor = anchor.to_string();
            let db = dominant_baseline.clone().unwrap_or_default();
            let transform_str = transform
                .as_ref()
                .map(|t| t.to_svg_string())
                .unwrap_or_default();
            let fs = font_size.clone().unwrap_or_default();
            let fw = font_weight.clone().unwrap_or_default();
            let fill = fill.clone().unwrap_or_default();
            let class = class.clone();

            view! {
                <text
                    x=x y=y
                    text-anchor=anchor
                    dominant-baseline=db
                    transform=transform_str
                    font-size=fs
                    font-weight=fw
                    fill=fill
                    class=class
                >
                    {content}
                </text>
            }
            .into_any()
        }

        ChartElement::Div {
            class,
            style,
            children,
        } => {
            let class = class.clone();
            let mut pairs: Vec<_> = style.iter().collect();
            pairs.sort_by_key(|(k, _)| (*k).clone());
            let style_str = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", k, v))
                .collect::<Vec<_>>()
                .join("; ");
            let children_views: Vec<AnyView> =
                children.iter().map(render_chart_element).collect();

            view! {
                <div class=class style=style_str>
                    {children_views}
                </div>
            }
            .into_any()
        }

        ChartElement::Span {
            class,
            style,
            content,
        } => {
            let class = class.clone();
            let mut pairs: Vec<_> = style.iter().collect();
            pairs.sort_by_key(|(k, _)| (*k).clone());
            let style_str = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", k, v))
                .collect::<Vec<_>>()
                .join("; ");
            let content = content.clone();

            view! {
                <span class=class style=style_str>{content}</span>
            }
            .into_any()
        }
    }
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

/// Internal chart rendering state.
#[derive(Clone)]
enum ChartState {
    Loading,
    Success(ChartElement),
    Error(String),
}

/// Renders a single ChartML chart with chrome (title bar, actions, footer).
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
) -> impl IntoView {
    let yaml_owned = yaml.clone();
    let yaml_for_info = yaml.clone();
    let yaml_for_save = yaml.clone();

    // Parse the spec to extract metadata
    let parsed_spec: Option<serde_json::Value> =
        serde_yaml::from_str(&yaml_owned).ok();

    let _chart_title = parsed_spec
        .as_ref()
        .and_then(extract_title)
        .unwrap_or_else(|| "Chart".to_string());

    let datasource_slug = parsed_spec.as_ref().and_then(extract_datasource);
    let sql_query = parsed_spec.as_ref().and_then(extract_query);
    let chart_type = parsed_spec.as_ref().and_then(extract_chart_type);
    let chart_orientation = parsed_spec.as_ref().and_then(extract_chart_orientation);
    let chart_mode = parsed_spec.as_ref().and_then(extract_chart_mode);

    // Reactive state for the chart
    let (chart_state, set_chart_state) = signal(ChartState::Loading);
    let (refresh_count, set_refresh_count) = signal(0_u32);
    let (last_refreshed, set_last_refreshed) = signal(None::<f64>);
    let (is_refreshing, set_is_refreshing) = signal(false);

    // Create the ChartML instance (created once, shared via Arc)
    let chartml = create_chartml();

    // Effect that fetches data and renders the chart reactively.
    // Runs whenever parameters or refresh_count change.
    let ds_slug = datasource_slug.clone();
    let sql = sql_query.clone();
    let yaml_for_render = yaml_owned.clone();

    Effect::new(move || {
        let params = parameters.get();
        let _refresh = refresh_count.get();
        let ds = ds_slug.clone();
        let q = sql.clone();
        let yaml_str = yaml_for_render.clone();
        let chartml = chartml.clone();

        set_chart_state.set(ChartState::Loading);
        set_is_refreshing.set(true);

        leptos::task::spawn_local(async move {
            let result = if let (Some(slug), Some(query)) = (ds, q) {
                // Remote data — fetch via server function
                let resolved_sql = substitute_params(&query, &params);

                match query_datasource_arrow(slug, resolved_sql, None).await {
                    Ok(query_result) => {
                        use base64::Engine;
                        let ipc_bytes = match base64::engine::general_purpose::STANDARD
                            .decode(&query_result.ipc_base64)
                        {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                set_chart_state
                                    .set(ChartState::Error(format!("Base64 decode error: {e}")));
                                return;
                            }
                        };

                        let data_table =
                            match chartml_core::data::DataTable::from_ipc_bytes(&ipc_bytes) {
                                Ok(dt) => dt,
                                Err(e) => {
                                    set_chart_state
                                        .set(ChartState::Error(format!("Arrow decode error: {e}")));
                                    return;
                                }
                            };

                        chartml
                            .render_from_yaml_with_data_async(&yaml_str, data_table)
                            .await
                            .map_err(|e| format!("Chart render error: {e}"))
                    }
                    Err(e) => Err(format!("Query error: {e}")),
                }
            } else {
                // Inline data — render directly
                chartml
                    .render_from_yaml(&yaml_str)
                    .map_err(|e| format!("Chart render error: {e}"))
            };

            match result {
                Ok(element) => {
                    set_chart_state.set(ChartState::Success(element));
                    // Store timestamp as milliseconds for the chart-header-bar web component
                    let now_ms = js_sys::Date::now();
                    set_last_refreshed.set(Some(now_ms));
                    set_is_refreshing.set(false);
                }
                Err(err) => {
                    set_chart_state.set(ChartState::Error(err));
                    set_is_refreshing.set(false);
                }
            }
        });
    });

    // Retry handler for error state
    let handle_refresh_for_retry = move |_: leptos::ev::MouseEvent| {
        set_refresh_count.update(|c| *c += 1);
    };

    // Store callbacks as StoredValue so they can be used inside move closures.
    let edit_cb = StoredValue::new(on_edit_chart);
    let delete_cb = StoredValue::new(on_delete_chart);
    let save_cb = StoredValue::new(on_save_to_dashboard);
    let info_cb = StoredValue::new(on_chart_info);
    let yaml_for_save_stored = StoredValue::new(yaml_for_save);
    let yaml_for_info_stored = StoredValue::new(yaml_for_info);

    // NodeRef for the wrapper div — we find the chart-header-bar child inside it
    let header_wrapper_ref = NodeRef::<leptos::html::Div>::new();

    // Attach event listeners to the web component after it mounts
    Effect::new(move || {
        let Some(wrapper) = header_wrapper_ref.get() else {
            return;
        };
        let wrapper_el: &web_sys::Element = wrapper.as_ref();
        let Some(header_el) = wrapper_el.query_selector("chart-header-bar").ok().flatten() else {
            return;
        };
        {
            let el: &web_sys::EventTarget = header_el.as_ref();

            // Refresh event
            let refresh_closure = Closure::<dyn Fn()>::new(move || {
                set_refresh_count.update(|c| *c += 1);
            });
            let _ = el.add_event_listener_with_callback(
                "header-refresh",
                refresh_closure.as_ref().unchecked_ref(),
            );
            refresh_closure.forget();

            // Edit event
            if let Some(cb) = edit_cb.get_value() {
                let bi = block_index;
                let ai = array_index;
                let edit_closure = Closure::<dyn Fn()>::new(move || {
                    cb.run((bi, ai));
                });
                let _ = el.add_event_listener_with_callback(
                    "header-edit",
                    edit_closure.as_ref().unchecked_ref(),
                );
                edit_closure.forget();
            }

            // Delete event
            if let Some(cb) = delete_cb.get_value() {
                let bi = block_index;
                let ai = array_index;
                let delete_closure = Closure::<dyn Fn()>::new(move || {
                    cb.run((bi, ai));
                });
                let _ = el.add_event_listener_with_callback(
                    "header-delete",
                    delete_closure.as_ref().unchecked_ref(),
                );
                delete_closure.forget();
            }

            // Save-to-dashboard event
            if let Some(cb) = save_cb.get_value() {
                let yaml = yaml_for_save_stored.get_value();
                let save_closure = Closure::<dyn Fn()>::new(move || {
                    let chart_md = format!("```chartml\n{}\n```", yaml);
                    cb.run(chart_md);
                });
                let _ = el.add_event_listener_with_callback(
                    "header-save-to-dashboard",
                    save_closure.as_ref().unchecked_ref(),
                );
                save_closure.forget();
            }

            // Info event
            if let Some(cb) = info_cb.get_value() {
                let yaml = yaml_for_info_stored.get_value();
                let info_closure = Closure::<dyn Fn()>::new(move || {
                    cb.run(yaml.clone());
                });
                let _ = el.add_event_listener_with_callback(
                    "header-info",
                    info_closure.as_ref().unchecked_ref(),
                );
                info_closure.forget();
            }
        } // end inner block borrowing el
    });

    // Compute attribute values for the web component
    let last_updated_attr = move || {
        last_refreshed.get().map(|ms| ms.to_string())
    };
    let refreshing_attr = move || {
        if is_refreshing.get() { Some("".to_string()) } else { None }
    };

    // Build static boolean attributes based on which callbacks are provided
    let has_refresh = true; // Always show refresh
    let has_edit = on_edit_chart.is_some();
    let has_delete = on_delete_chart.is_some();
    let has_save = on_save_to_dashboard.is_some();
    let has_info = on_chart_info.is_some();

    view! {
        <div class="my-2">
            // Chart header bar web component (wrapped in div for NodeRef access)
            <div node_ref=header_wrapper_ref>
                <chart-header-bar
                    attr:last-updated=last_updated_attr
                    attr:refreshing=refreshing_attr
                    attr:show-refresh=has_refresh.then_some("")
                    attr:show-edit=has_edit.then_some("")
                    attr:show-delete=has_delete.then_some("")
                    attr:show-save-to-dashboard=has_save.then_some("")
                    attr:show-info=has_info.then_some("")
                    attr:show-type-selector=""
                    attr:chart-type=chart_type
                    attr:chart-orientation=chart_orientation
                    attr:chart-mode=chart_mode
                />
            </div>
            // Chart content area
            <div class="p-4">
                {move || {
                    match chart_state.get() {
                        ChartState::Loading => {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <svg class="animate-spin h-6 w-6 text-muted-foreground" fill="none" viewBox="0 0 24 24">
                                        <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
                                        <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
                                    </svg>
                                    <span class="ml-2 text-sm text-muted-foreground">"Loading chart..."</span>
                                </div>
                            }.into_any()
                        }
                        ChartState::Success(element) => {
                            view! {
                                <div class="w-full">
                                    {render_chart_element(&element)}
                                </div>
                            }.into_any()
                        }
                        ChartState::Error(err) => {
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
                                                on:click=handle_refresh_for_retry
                                                class="mt-2 text-xs text-primary hover:text-primary/80 underline"
                                            >
                                                "Retry"
                                            </button>
                                        </div>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }
                }}
            </div>
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
    /// Callback when a chart's "edit" action is clicked (block_index, array_index)
    #[prop(optional)]
    on_edit_chart: Option<Callback<(usize, usize)>>,
    /// Callback when a chart's "delete" action is clicked
    #[prop(optional)]
    on_delete_chart: Option<Callback<(usize, usize)>>,
    /// Callback to save a chart to another dashboard (receives chart YAML)
    #[prop(optional)]
    on_save_to_dashboard: Option<Callback<String>>,
    /// Callback to show chart info/spec (receives chart YAML)
    #[prop(optional)]
    on_chart_info: Option<Callback<String>>,
) -> impl IntoView {
    let segments = Memo::new(move |_| parse_segments(&content.get()));

    // Store callbacks in StoredValue so they can be cloned into the For loop closure.
    let edit_cb = StoredValue::new(on_edit_chart);
    let delete_cb = StoredValue::new(on_delete_chart);
    let save_cb = StoredValue::new(on_save_to_dashboard);
    let info_cb = StoredValue::new(on_chart_info);

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
}
