// SPDX-License-Identifier: AGPL-3.0-or-later

//! Factory for creating configured ChartML instances with all renderers
//! and the DataFusion transform middleware registered.

use chartml_core::theme::Theme;
use kyomi_chart_theme::kyomi_theme;
use chartml_core::ChartML;
use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_chart_table::TableRenderer;
use chartml_datafusion::DataFusionTransform;
use std::sync::Once;

use crate::pdf_typst::bundled_fonts_owned;

/// Ensures chartml-render's shared font database is seeded with the
/// compile-time embedded Kyomi fonts (Instrument Serif, DM Sans, Geist
/// Mono) exactly once per process. Called at the top of every public
/// render entry point below — the first invocation wins, subsequent
/// invocations short-circuit via `Once::call_once`.
///
/// Without this, resvg's rasterizer can't resolve the `'DM Sans'` /
/// `'Instrument Serif'` / `'Geist Mono'` family names that the Kyomi
/// chart theme sets in SVG `font-family` attributes, and silently drops
/// every text element in the chart (tick labels, axis labels, titles,
/// legends). The PNG comes out with bars and gridlines but no text.
fn ensure_fonts_registered() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // `init_font_database` returns `false` if the fontdb has already
        // been lazily initialized by a prior render in this process. That
        // case is only possible if some other code path triggered rendering
        // before kyomi-agent's first call reaches here, which shouldn't
        // happen in the normal process lifecycle but we don't error on it —
        // the render proceeds with system-only fonts, which is strictly
        // better than panicking.
        let _ = chartml_render::init_font_database(bundled_fonts_owned());
    });
}

/// Create a fully configured ChartML instance with all chart renderers,
/// the DataFusion transform pipeline, and the Kyomi editorial theme
/// applied. Every server-side chart render path (PDF export, email
/// snapshots, MCP chart app, watches alerts) should use this factory so
/// chart chrome matches the dashboard viewer's browser rendering.
///
/// PDF and email snapshots are always rendered in light mode — print
/// media and email clients don't have a meaningful dark-mode preference
/// we can read at render time, and light-on-light is the safe default.
pub fn create_chartml() -> ChartML {
    create_chartml_with_theme(kyomi_theme(false))
}

/// Create a configured ChartML instance with an explicit theme. Exposed
/// so callers that need non-default chrome (e.g. a future dark-mode
/// scheduled report) can supply their own `Theme` without duplicating
/// the renderer registration boilerplate.
pub fn create_chartml_with_theme(theme: Theme) -> ChartML {
    let mut chartml = ChartML::new();

    // Register all chart type renderers
    chartml.register_renderer("bar", CartesianRenderer::new());
    chartml.register_renderer("line", CartesianRenderer::new());
    chartml.register_renderer("area", CartesianRenderer::new());
    chartml.register_renderer("pie", PieRenderer::new());
    chartml.register_renderer("doughnut", PieRenderer::new());
    chartml.register_renderer("scatter", ScatterRenderer::new());
    chartml.register_renderer("metric", MetricRenderer::new());
    chartml.register_renderer("table", TableRenderer::new());

    // Register DataFusion-based transform middleware (sql, aggregate, forecast)
    chartml.register_transform(DataFusionTransform);

    // Editorial chart chrome — Variant A typography, shape, and colors.
    chartml.set_theme(theme);

    chartml
}

/// Render a ChartML spec (with inline data) to PNG bytes.
///
/// This is the primary entry point for server-side chart rendering.
/// The caller is responsible for resolving datasource queries to inline data
/// before calling this function.
///
/// The SVG rasterization step (resvg + tiny-skia) is CPU-intensive and
/// synchronous — it runs on the blocking thread pool via `spawn_blocking` so
/// it does not starve the Tokio async runtime.
///
/// # Arguments
/// * `yaml` — ChartML YAML spec string (with inline data already resolved)
/// * `width` — chart width in CSS pixels
/// * `height` — chart height in CSS pixels
/// * `density` — DPI (72 = 1x, 144 = 2x for PDF)
/// * `palette` — optional color palette to inject into the spec
pub async fn render_chart_to_png(
    yaml: &str,
    width: u32,
    height: u32,
    density: u32,
    palette: Option<&[String]>,
) -> Result<Vec<u8>, String> {
    ensure_fonts_registered();

    // If a palette is provided, inject it into the spec's style.colors.
    // Do this before entering spawn_blocking so we work with owned data.
    let final_yaml = if let Some(colors) = palette {
        inject_palette(yaml, colors)
    } else {
        yaml.to_string()
    };

    // Rasterization (SVG parse → tiny-skia → PNG encode) is CPU-bound and
    // synchronous — offload it to the blocking thread pool.
    tokio::task::spawn_blocking(move || {
        let chartml = create_chartml();
        chartml_render::render_to_png(&chartml, &final_yaml, width, height, density)
            .map_err(|e| format!("Chart rendering failed: {e}"))
    })
    .await
    .map_err(|e| format!("Chart rendering panicked: {e}"))?
}

/// Render a ChartML spec synchronously (for specs without async transforms).
pub fn render_chart_to_png_sync(
    yaml: &str,
    width: u32,
    height: u32,
    density: u32,
    palette: Option<&[String]>,
) -> Result<Vec<u8>, String> {
    ensure_fonts_registered();
    let chartml = create_chartml();

    let final_yaml = if let Some(colors) = palette {
        inject_palette(yaml, colors)
    } else {
        yaml.to_string()
    };

    chartml_render::render_to_png(&chartml, &final_yaml, width, height, density)
        .map_err(|e| format!("Chart rendering failed: {e}"))
}

/// Inject a color palette into a ChartML YAML spec.
///
/// Adds `style.colors` to the visualize section of the spec.
/// If the spec already has colors, the palette is not injected.
fn inject_palette(yaml: &str, palette: &[String]) -> String {
    // Parse as JSON Value so we can manipulate the structure
    let mut value: serde_json::Value = match serde_yaml::from_str(yaml) {
        Ok(v) => v,
        Err(_) => return yaml.to_string(),
    };

    // Handle both single chart and array-of-documents formats
    let inject_into = |chart: &mut serde_json::Value, colors: &[String]| {
        if let Some(vis) = chart.get_mut("visualize") {
            let style = vis.as_object_mut()
                .and_then(|v| {
                    v.entry("style")
                        .or_insert_with(|| serde_json::json!({}));
                    v.get_mut("style")
                });
            if let Some(style) = style
                && style.get("colors").is_none()
                && let Some(s) = style.as_object_mut()
            {
                s.insert("colors".to_string(), serde_json::json!(colors));
            }
        }
    };

    match &mut value {
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                if item.get("type").and_then(|t| t.as_str()) == Some("chart") {
                    inject_into(item, palette);
                }
            }
        }
        obj if obj.get("type").and_then(|t| t.as_str()) == Some("chart") => {
            inject_into(obj, palette);
        }
        _ => {}
    }

    serde_yaml::to_string(&value).unwrap_or_else(|_| yaml.to_string())
}
