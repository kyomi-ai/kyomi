// SPDX-License-Identifier: AGPL-3.0-or-later

//! The chart types Kyomi draws and the renderer behind each.
//!
//! Every app that draws Kyomi charts — the main app (`kyomi-ui`'s
//! `configured_chartml`) and the MCP chart app (`setup_chartml`) — registers its
//! renderers through [`register_kyomi_renderers`], so a chart type added or
//! changed here reaches both. Before this lived in one place, the two kept
//! their own lists and the MCP app had silently fallen behind (no `donut`).

use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_chart_table::TableRenderer;
use chartml_core::ChartML;

/// The chart types [`register_kyomi_renderers`] registers, sorted. Exposed so an
/// app can check that its own registry is the shared one.
pub const KYOMI_CHART_TYPES: &[&str] = &[
    "area", "bar", "donut", "doughnut", "line", "metric", "pie", "scatter", "table",
];

/// Register a renderer for every type in [`KYOMI_CHART_TYPES`] on `chartml`.
pub fn register_kyomi_renderers(chartml: &mut ChartML) {
    chartml.register_renderer("bar", CartesianRenderer::new());
    chartml.register_renderer("line", CartesianRenderer::new());
    chartml.register_renderer("area", CartesianRenderer::new());
    chartml.register_renderer("pie", PieRenderer::new());
    chartml.register_renderer("donut", PieRenderer::new());
    chartml.register_renderer("doughnut", PieRenderer::new());
    chartml.register_renderer("scatter", ScatterRenderer::new());
    chartml.register_renderer("metric", MetricRenderer::new());
    chartml.register_renderer("table", TableRenderer::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_the_kyomi_chart_types_are_registered() {
        let mut chartml = ChartML::new();
        register_kyomi_renderers(&mut chartml);

        let mut registered = chartml.registry().renderer_types();
        registered.sort();
        assert_eq!(
            registered, KYOMI_CHART_TYPES,
            "KYOMI_CHART_TYPES must name exactly the types register_kyomi_renderers registers"
        );
    }
}
