// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn registry_chart_renderers_escape_untrusted_labels_and_values() {
    let payload = "<img src=x onerror=\"void(0)\"><script>void(0)</script>";
    for visualize in [
        serde_json::json!({"type": "table", "columns": [{"field": "label", "label": payload}]}),
        serde_json::json!({"type": "metric", "label": payload, "value": "value"}),
        serde_json::json!({"type": "bar", "columns": "label", "rows": "value"}),
        serde_json::json!({"type": "pie", "columns": "label", "rows": "value"}),
        serde_json::json!({"type": "scatter", "columns": "value", "rows": "value", "marks": {"color": "label"}}),
    ] {
        let spec = serde_json::json!({"version": 1, "type": "chart", "title": payload, "data": {"rows": [{"label": payload, "value": 42}]}, "visualize": visualize});
        let yaml = serde_yaml::to_string(&spec).unwrap();
        let element = create_chartml().render_from_yaml(&yaml).unwrap();
        let svg = chartml_core::svg::element_to_svg(&element, 600.0, 400.0);
        assert!(!svg.contains("<img"), "{yaml}: {svg}");
        assert!(!svg.contains("<script"), "{yaml}: {svg}");
        assert!(svg.contains("<svg"));
        assert!(
            svg.contains("&lt;img"),
            "fixture must reach renderer output: {yaml}: {svg}"
        );
    }
}

#[test]
fn registry_svg_serializer_escapes_attribute_breakouts() {
    use chartml_core::element::ChartElement;
    let payload = "\" onload=\"void(0)\"><script>void(0)</script>";
    let element = ChartElement::Span {
        class: payload.to_string(),
        style: std::collections::HashMap::from([("color".to_string(), payload.to_string())]),
        content: payload.to_string(),
    };
    let svg = chartml_core::svg::element_to_svg(&element, 600.0, 400.0);
    assert!(!svg.contains("\" onload=\""));
    assert!(!svg.contains("<script"));
    assert!(svg.contains("&quot; onload=&quot;"));
}
