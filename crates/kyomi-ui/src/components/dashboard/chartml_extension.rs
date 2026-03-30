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

use std::sync::Arc;

use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_core::ChartML;
use chartml_leptos::ChartMLChart;
use kode_leptos::extension::Extension;
use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;

/// Create a configured ChartML instance for the extension.
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

/// Kode extension that renders `chartml` code blocks as live charts.
pub struct ChartMLExtension {
    chartml: Arc<ChartML>,
}

impl Default for ChartMLExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl ChartMLExtension {
    pub fn new() -> Self {
        Self {
            chartml: create_chartml(),
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

        let yaml = content.to_string();
        let chartml = self.chartml.clone();

        // Create a static signal for the spec — the WYSIWYG editor owns the
        // content lifecycle, so we just render what we're given.
        let spec = RwSignal::new(yaml);

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
