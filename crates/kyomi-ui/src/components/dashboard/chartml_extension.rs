// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML extension for the kode WYSIWYG editor.
//!
//! Renders `chartml` fenced code blocks as static SVG charts using persistent
//! DOM elements. The WYSIWYG editor rebuilds its entire Leptos view tree on
//! every keystroke, which would normally unmount and remount chart components.
//!
//! This extension avoids that by:
//! 1. Rendering charts to SVG strings (cached by ordinal — unchanged charts
//!    skip the render pipeline entirely)
//! 2. Creating persistent `web_sys::Element` nodes that survive view tree
//!    rebuilds — on each render pass, the same DOM element is moved into the
//!    new wrapper div via `appendChild`, so the browser never re-parses or
//!    re-renders the SVG

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_core::ChartML;
use chartml_render::element_to_svg;
use kode_leptos::extension::Extension;
use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;

/// Default render width for static SVG charts. The viewBox preserves the
/// layout geometry; we strip the fixed width/height after rendering so the
/// SVG scales to fill its container.
const DEFAULT_RENDER_WIDTH: f64 = 800.0;
const DEFAULT_RENDER_HEIGHT: f64 = 400.0;

/// Make an SVG string responsive by replacing fixed `width`/`height`
/// attributes on the root `<svg>` with `width="100%"` (height removed so
/// the viewBox aspect ratio controls it).
fn make_svg_responsive(svg: &str) -> String {
    // The root <svg> tag is always at the start.
    let Some(close) = svg.find('>') else {
        return svg.to_string();
    };
    let tag = &svg[..close];
    let rest = &svg[close..];

    // Strip existing width and height attributes using simple string ops.
    // The SVG is our own output so the format is predictable.
    let tag = strip_attr(tag, "width");
    let tag = strip_attr(&tag, "height");

    format!("{} width=\"100%\"{}", tag, rest)
}

/// Remove a single `name="..."` attribute from an SVG tag string.
fn strip_attr(tag: &str, attr: &str) -> String {
    let pattern = format!("{}=\"", attr);
    let Some(start) = tag.find(&pattern) else {
        return tag.to_string();
    };
    let after_eq = start + pattern.len();
    let Some(end_quote) = tag[after_eq..].find('"') else {
        return tag.to_string();
    };
    // Also strip leading whitespace before the attribute
    let trim_start = tag[..start].trim_end().len();
    format!("{}{}", &tag[..trim_start], &tag[after_eq + end_quote + 1..])
}

/// Render a chart error as inline HTML.
fn render_error_html(e: &dyn std::fmt::Display) -> String {
    format!(
        "<div class=\"chartml-error\" style=\"color:#dc3545;font-family:monospace;\
         padding:12px;background:#fff5f5;border:1px solid #dc3545;\
         border-radius:4px;\">{e}</div>"
    )
}

/// Create a configured ChartML instance, optionally with a color palette.
fn create_chartml(colors: Option<Vec<String>>) -> Arc<ChartML> {
    let mut chartml = ChartML::new();
    chartml.register_renderer("bar", CartesianRenderer::new());
    chartml.register_renderer("line", CartesianRenderer::new());
    chartml.register_renderer("area", CartesianRenderer::new());
    chartml.register_renderer("pie", PieRenderer::new());
    chartml.register_renderer("donut", PieRenderer::new());
    chartml.register_renderer("scatter", ScatterRenderer::new());
    chartml.register_renderer("metric", MetricRenderer::new());
    if let Some(colors) = colors {
        chartml.set_default_palette(colors);
    }
    Arc::new(chartml)
}

/// Per-chart cached state: YAML content, rendered SVG, and persistent DOM node.
struct CachedChart {
    content: String,
    svg_html: String,
    /// Persistent DOM element that survives Leptos view tree rebuilds.
    /// Only populated on WASM — `None` in native tests.
    #[cfg(target_arch = "wasm32")]
    element: send_wrapper::SendWrapper<web_sys::Element>,
}

/// Kode extension that renders `chartml` code blocks as static SVG charts
/// with persistent DOM elements.
pub struct ChartMLExtension {
    chartml: Arc<ChartML>,
    /// Index of the next chart block to render in this pass.
    render_index: Mutex<usize>,
    /// Cached chart data, indexed by ordinal position in the document.
    cache: Mutex<Vec<CachedChart>>,
    /// Number of actual chart renders (cache misses). Used for testing.
    render_count: AtomicUsize,
}

impl Default for ChartMLExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl ChartMLExtension {
    pub fn new() -> Self {
        Self {
            chartml: create_chartml(None),
            render_index: Mutex::new(0),
            cache: Mutex::new(Vec::new()),
            render_count: AtomicUsize::new(0),
        }
    }

    pub fn with_colors(colors: Vec<String>) -> Self {
        Self {
            chartml: create_chartml(Some(colors)),
            render_index: Mutex::new(0),
            cache: Mutex::new(Vec::new()),
            render_count: AtomicUsize::new(0),
        }
    }

    /// Render chart YAML to SVG, using the cache if content is unchanged.
    ///
    /// Returns the SVG HTML string. This is the pure, testable core of the
    /// caching mechanism — no Leptos or DOM dependency.
    /// On WASM, the render_code_block method handles caching with persistent
    /// DOM elements directly, so this is only used by tests and SSR.
    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(crate) fn render_svg_cached(&self, content: &str, chart_idx: usize) -> String {
        let mut cache = self.cache.lock().expect("chart cache lock");

        // Cache hit: same ordinal, same content → return cached SVG.
        if chart_idx < cache.len() && cache[chart_idx].content == content {
            return cache[chart_idx].svg_html.clone();
        }

        // Cache miss: render to SVG.
        self.render_count.fetch_add(1, Ordering::Relaxed);
        let svg_html = match self.chartml.render_from_yaml_with_size(
            content,
            Some(DEFAULT_RENDER_WIDTH),
            None,
        ) {
            Ok(element) => make_svg_responsive(&element_to_svg(&element, DEFAULT_RENDER_WIDTH, DEFAULT_RENDER_HEIGHT)),
            Err(e) => render_error_html(&e),
        };

        // Update or push cache entry (native only — WASM path handled separately)
        #[cfg(not(target_arch = "wasm32"))]
        {
            if chart_idx < cache.len() {
                cache[chart_idx] = CachedChart {
                    content: content.to_string(),
                    svg_html: svg_html.clone(),
                };
            } else {
                cache.push(CachedChart {
                    content: content.to_string(),
                    svg_html: svg_html.clone(),
                });
            }
        }

        svg_html
    }

    /// Number of actual chart renders (cache misses). Exposed for testing.
    #[cfg(test)]
    pub(crate) fn render_count(&self) -> usize {
        self.render_count.load(Ordering::Relaxed)
    }
}

impl Extension for ChartMLExtension {
    fn name(&self) -> &str {
        "chartml"
    }

    fn code_block_languages(&self) -> &[&str] {
        &["chartml"]
    }

    fn begin_render_pass(&self) {
        let mut idx = self.render_index.lock().expect("render_index lock");
        *idx = 0;
    }

    fn render_code_block(
        &self,
        language: &str,
        content: &str,
        _block_start: usize,
        _block_end: usize,
    ) -> Option<AnyView> {
        if language != "chartml" {
            return None;
        }

        let chart_idx = {
            let mut idx = self.render_index.lock().expect("render_index lock");
            let i = *idx;
            *idx += 1;
            i
        };

        // ── WASM path: persistent DOM elements ──────────────────────────
        #[cfg(target_arch = "wasm32")]
        {
            use send_wrapper::SendWrapper;

            let mut cache = self.cache.lock().expect("chart cache lock");

            // Get or create persistent DOM element for this chart slot.
            let element = if chart_idx < cache.len() {
                let entry = &mut cache[chart_idx];
                if entry.content != content {
                    // Content changed — re-render SVG into the persistent element.
                    self.render_count.fetch_add(1, Ordering::Relaxed);
                    let svg_html = match self.chartml.render_from_yaml_with_size(
                        content,
                        Some(DEFAULT_RENDER_WIDTH),
                        None,
                    ) {
                        Ok(el) => {
                            make_svg_responsive(&element_to_svg(&el, DEFAULT_RENDER_WIDTH, DEFAULT_RENDER_HEIGHT))
                        }
                        Err(e) => render_error_html(&e),
                    };
                    entry.element.set_inner_html(&svg_html);
                    entry.content = content.to_string();
                    entry.svg_html = svg_html;
                }
                entry.element.clone()
            } else {
                // New chart slot — create persistent DOM element.
                self.render_count.fetch_add(1, Ordering::Relaxed);
                let svg_html = match self.chartml.render_from_yaml_with_size(
                    content,
                    Some(DEFAULT_RENDER_WIDTH),
                    None,
                ) {
                    Ok(el) => make_svg_responsive(&element_to_svg(&el, DEFAULT_RENDER_WIDTH, DEFAULT_RENDER_HEIGHT)),
                    Err(e) => render_error_html(&e),
                };
                let document = web_sys::window().unwrap().document().unwrap();
                let el = document.create_element("div").unwrap();
                el.set_attribute("class", "chartml-persistent").unwrap();
                el.set_inner_html(&svg_html);
                let wrapped = SendWrapper::new(el);
                cache.push(CachedChart {
                    content: content.to_string(),
                    svg_html,
                    element: wrapped.clone(),
                });
                wrapped
            };

            // Drop the lock before building the view.
            drop(cache);

            // Return a wrapper div. On mount, adopt the persistent element
            // via appendChild — this MOVES it (no clone, no re-parse).
            let container_ref = NodeRef::<leptos::html::Div>::new();
            let el = element;
            Effect::new(move || {
                if let Some(container) = container_ref.get() {
                    let _ = container.append_child(&*el);
                }
            });

            return Some(
                view! { <div class="my-2 not-prose" node_ref=container_ref /> }.into_any(),
            );
        }

        // ── SSR/native fallback: static inner_html ──────────────────────
        #[cfg(not(target_arch = "wasm32"))]
        {
            let svg_html = self.render_svg_cached(content, chart_idx);
            Some(view! { <div class="my-2 not-prose" inner_html=svg_html /> }.into_any())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR_CHART_YAML: &str = "\
type: chart
version: 1
data:
  provider: inline
  rows:
    - { category: A, value: 10 }
    - { category: B, value: 20 }
visualize:
  type: bar
  columns: category
  rows: value";

    const LINE_CHART_YAML: &str = "\
type: chart
version: 1
data:
  provider: inline
  rows:
    - { x: 1, y: 10 }
    - { x: 2, y: 20 }
visualize:
  type: line
  columns: x
  rows: y";

    #[test]
    fn caches_svg_for_unchanged_content() {
        let ext = ChartMLExtension::new();

        // First render → cache miss
        ext.begin_render_pass();
        let svg1 = ext.render_svg_cached(BAR_CHART_YAML, 0);
        assert_eq!(ext.render_count(), 1);
        assert!(!svg1.is_empty());

        // Second render with same content → cache hit, no re-render
        ext.begin_render_pass();
        let svg2 = ext.render_svg_cached(BAR_CHART_YAML, 0);
        assert_eq!(ext.render_count(), 1, "should not re-render when content unchanged");
        assert_eq!(svg1, svg2);
    }

    #[test]
    fn re_renders_when_content_changes() {
        let ext = ChartMLExtension::new();

        ext.begin_render_pass();
        let svg1 = ext.render_svg_cached(BAR_CHART_YAML, 0);
        assert_eq!(ext.render_count(), 1);

        // Change content → cache miss, re-render
        ext.begin_render_pass();
        let svg2 = ext.render_svg_cached(LINE_CHART_YAML, 0);
        assert_eq!(ext.render_count(), 2, "should re-render when content changes");
        assert_ne!(svg1, svg2);
    }

    #[test]
    fn multiple_charts_tracked_independently() {
        let ext = ChartMLExtension::new();

        // First pass: render two different charts
        ext.begin_render_pass();
        let bar_svg = ext.render_svg_cached(BAR_CHART_YAML, 0);
        let line_svg = ext.render_svg_cached(LINE_CHART_YAML, 1);
        assert_eq!(ext.render_count(), 2);

        // Second pass: same content for both → both cached
        ext.begin_render_pass();
        let bar_svg2 = ext.render_svg_cached(BAR_CHART_YAML, 0);
        let line_svg2 = ext.render_svg_cached(LINE_CHART_YAML, 1);
        assert_eq!(ext.render_count(), 2, "neither chart should re-render");
        assert_eq!(bar_svg, bar_svg2);
        assert_eq!(line_svg, line_svg2);
    }
}
