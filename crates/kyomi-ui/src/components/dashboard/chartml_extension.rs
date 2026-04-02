// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML extension for the kode WYSIWYG editor.
//!
//! Renders `chartml` fenced code blocks as live inline charts instead of
//! syntax-highlighted code. Uses `chartml-leptos::ChartMLChart` for
//! responsive rendering with tooltips and animations.
//!
//! This is Kyomi's extension of kode — kode provides the editor, Kyomi
//! provides the chart rendering. Same pattern as React extending Tiptap
//! with `ChartMLNode.jsx`.

use std::sync::{Arc, Mutex};

use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_core::ChartML;
use chartml_leptos::ChartMLChart;
use kode_leptos::extension::Extension;
use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;

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

/// Per-chart cached state: the YAML content and the reactive signal that
/// `ChartMLChart` reads. Reusing the same signal across render passes means
/// the chart component is only re-rendered when the YAML actually changes.
struct CachedChart {
    content: String,
    spec: RwSignal<String>,
}

/// Kode extension that renders `chartml` code blocks as live charts.
///
/// Maintains a per-render-pass index so that chart blocks are matched by
/// their ordinal position in the document (first chart = 0, second = 1, …).
/// Signals are reused across passes; charts only re-render when their YAML
/// content actually changes.
pub struct ChartMLExtension {
    chartml: Arc<ChartML>,
    /// Index of the next chart block to render in this pass.
    render_index: Mutex<usize>,
    /// Cached chart signals from the previous render pass, indexed by ordinal.
    cache: Mutex<Vec<CachedChart>>,
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
        }
    }

    pub fn with_colors(colors: Vec<String>) -> Self {
        Self {
            chartml: create_chartml(Some(colors)),
            render_index: Mutex::new(0),
            cache: Mutex::new(Vec::new()),
        }
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
        let mut idx = self.render_index.lock().unwrap();
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

        // Determine which chart slot this is (0-indexed ordinal within the doc).
        let chart_idx = {
            let mut idx = self.render_index.lock().unwrap();
            let i = *idx;
            *idx += 1;
            i
        };

        // Get or create the signal for this chart slot.
        let spec = {
            let mut cache = self.cache.lock().unwrap();
            if chart_idx < cache.len() {
                // Existing slot — update signal only if content changed.
                let entry = &mut cache[chart_idx];
                if entry.content != content {
                    entry.content = content.to_string();
                    entry.spec.set(content.to_string());
                }
                entry.spec
            } else {
                // New slot — create signal and push to cache.
                let signal = RwSignal::new(content.to_string());
                cache.push(CachedChart {
                    content: content.to_string(),
                    spec: signal,
                });
                signal
            }
        };

        let chartml = self.chartml.clone();

        Some(
            view! {
                <div class="my-2 not-prose">
                    <ChartMLChart
                        spec=Signal::derive(move || spec.get())
                        chartml=chartml
                    />
                </div>
            }
            .into_any(),
        )
    }
}
