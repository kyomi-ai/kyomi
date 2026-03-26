// SPDX-License-Identifier: AGPL-3.0-or-later

//! Factory for creating configured ChartML instances with all renderers
//! and the DataFusion transform middleware registered.

use chartml_core::ChartML;
use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_datafusion::DataFusionTransform;

/// Create a fully configured ChartML instance with all chart renderers
/// and the DataFusion transform pipeline registered.
///
/// Optionally accepts a color palette to inject into specs before rendering.
pub fn create_chartml() -> ChartML {
    let mut chartml = ChartML::new();

    // Register all chart type renderers
    chartml.register_renderer("bar", CartesianRenderer::new());
    chartml.register_renderer("line", CartesianRenderer::new());
    chartml.register_renderer("area", CartesianRenderer::new());
    chartml.register_renderer("pie", PieRenderer::new());
    chartml.register_renderer("doughnut", PieRenderer::new());
    chartml.register_renderer("scatter", ScatterRenderer::new());
    chartml.register_renderer("metric", MetricRenderer::new());

    // Register DataFusion-based transform middleware (sql, aggregate, forecast)
    chartml.register_transform(DataFusionTransform);

    chartml
}

/// Render a ChartML spec (with inline data) to PNG bytes.
///
/// This is the primary entry point for server-side chart rendering.
/// The caller is responsible for resolving datasource queries to inline data
/// before calling this function.
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
    let chartml = create_chartml();

    // If a palette is provided, inject it into the spec's style.colors
    let final_yaml = if let Some(colors) = palette {
        inject_palette(yaml, colors)
    } else {
        yaml.to_string()
    };

    chartml_render::render_to_png_async(&chartml, &final_yaml, width, height, density)
        .await
        .map_err(|e| format!("Chart rendering failed: {e}"))
}

/// Render a ChartML spec synchronously (for specs without async transforms).
pub fn render_chart_to_png_sync(
    yaml: &str,
    width: u32,
    height: u32,
    density: u32,
    palette: Option<&[String]>,
) -> Result<Vec<u8>, String> {
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
