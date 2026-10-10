// SPDX-License-Identifier: AGPL-3.0-or-later

//! Render the real ChartML tool contract through the public dispatcher.

use super::super::*;
use serde_json::json;

fn render(mut schema: Value) -> String {
    schema["tool"] = json!("validate_chartml");
    render_tool_schema(schema).to_html()
}

#[test]
fn single_success_renders_chartml_and_checked_count() {
    let html = render(json!({
        "input": {"blocks": ["data:\n  values: []\nvisualize:\n  type: table"]},
        "output": {"valid": true, "blocks_checked": 1}
    }));
    assert!(html.contains("ChartML Block 1:"), "{html}");
    assert!(
        html.contains("data:\n  values: []\nvisualize:\n  type: table"),
        "{html}"
    );
    assert!(
        html.contains("ChartML is valid: 1 block checked."),
        "{html}"
    );
    assert!(!html.contains("Validation Failed"), "{html}");
    assert!(!html.contains("Unknown validation error"), "{html}");
    assert!(!html.contains("No ChartML provided"), "{html}");
    assert!(!html.contains("Query Cost"), "{html}");
    assert!(!html.contains("Bytes Scanned"), "{html}");
}

#[test]
fn multiple_blocks_and_all_diagnostics_render_in_order_with_context() {
    let html = render(json!({
        "input": {"blocks": ["visualize:\n  type: table", "data:\n  query: SELECT broken"]},
        "output": {"valid": false, "errors": [
            {"block": 1, "type": "missing_key", "message": "Missing data key", "instance_path": "/visualize/0", "schema_path": "/properties/visualize/items/required", "stage": "schema", "component": 3},
            {"block": 2, "type": "sql_error", "message": "SQL syntax error", "stage": "sql", "component": "datasource"},
            {"block": 2, "type": "dry_run_unavailable", "message": "Dry-run unavailable for this datasource", "path": "/data/query"}
        ]}
    }));
    for text in [
        "ChartML Block 1:",
        "ChartML Block 2:",
        "visualize:\n  type: table",
        "data:\n  query: SELECT broken",
        "Block 1: Validation Failed",
        "Block 2: Validation Failed",
        "Missing data key",
        "SQL syntax error",
        "Dry-run unavailable for this datasource",
        "missing_key",
        "sql_error",
        "dry_run_unavailable",
        "/visualize/0",
        "/properties/visualize/items/required",
        "Instance path",
        "Schema path",
        "/data/query",
        "Stage",
        "schema",
        "sql",
        "Component",
        "table",
        "datasource",
    ] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
    assert_eq!(html.matches("Validation Failed").count(), 3, "{html}");
    assert!(html.contains("class=\"font-medium\">3</span>"), "{html}");
    assert!(html.find("Missing data key") < html.find("SQL syntax error"));
    assert!(html.find("SQL syntax error") < html.find("Dry-run unavailable"));
    assert!(!html.contains("Unknown validation error"), "{html}");
}

#[test]
fn multiple_success_uses_returned_blocks_checked_count() {
    let html = render(json!({
        "input": {"blocks": ["first block", "second block"]},
        "output": {"valid": true, "blocks_checked": 2}
    }));
    for text in [
        "ChartML Block 1:",
        "ChartML Block 2:",
        "first block",
        "second block",
        "ChartML is valid: 2 blocks checked.",
    ] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
}

#[test]
fn empty_blocks_report_zero_checked_without_claiming_chartml_is_valid() {
    let html = render(json!({
        "input": {"blocks": []}, "output": {"valid": true, "blocks_checked": 0}
    }));
    assert!(
        html.contains("No ChartML blocks supplied (0 blocks)."),
        "{html}"
    );
    assert!(
        html.contains("Validation completed: 0 blocks checked."),
        "{html}"
    );
    assert!(!html.contains("ChartML is valid"), "{html}");
    assert!(!html.contains("Validation Failed"), "{html}");
}

#[test]
fn absent_or_null_output_remains_pending_with_supplied_block() {
    for schema in [
        json!({"input": {"blocks": ["pending chart"]}}),
        json!({"input": {"blocks": ["pending chart"]}, "output": null}),
    ] {
        let html = render(schema);
        assert!(html.contains("pending chart"), "{html}");
        assert!(
            html.contains("Validation pending — no result received."),
            "{html}"
        );
        assert!(!html.contains("Validation Failed"), "{html}");
        assert!(!html.contains("Unrecognized validation result"), "{html}");
    }
}

#[test]
fn malformed_outputs_are_distinct_from_real_validation_diagnostics() {
    for output in [
        json!({}),
        json!("unexpected"),
        json!({"valid": "true"}),
        json!({"valid": true}),
        json!({"valid": true, "blocks_checked": -1}),
        json!({"valid": true, "blocks_checked": 1, "errors": [{"block": 1, "message": "contradiction"}]}),
        json!({"valid": false}),
        json!({"valid": false, "errors": []}),
        json!({"valid": false, "errors": "invalid"}),
        json!({"valid": false, "errors": [{"block": 1}]}),
        json!({"valid": false, "errors": [{"block": 0, "message": "invalid block"}]}),
    ] {
        let html = render(json!({"input": {"blocks": ["chart"]}, "output": output}));
        assert!(html.contains("Unrecognized validation result"), "{html}");
        assert!(!html.contains("Validation Failed"), "{html}");
        assert!(!html.contains("Unknown validation error"), "{html}");
        assert!(!html.contains("Validation pending"), "{html}");
    }
}

#[test]
fn malformed_diagnostic_does_not_hide_other_returned_errors() {
    let html = render(json!({
        "input": {"blocks": ["chart"]},
        "output": {"valid": false, "errors": [null, {"block": 1, "type": "missing_key", "message": "Real diagnostic"}]}
    }));
    assert!(html.contains("Unrecognized validation result"), "{html}");
    assert!(html.contains("Block 1: Validation Failed"), "{html}");
    assert!(html.contains("Real diagnostic"), "{html}");
}

#[test]
fn malformed_input_is_visible_and_does_not_hide_supplied_blocks() {
    let html = render(json!({"input": {"blocks": [42, "visible block"]}}));
    for text in [
        "ChartML Block 1:",
        "Unrecognized ChartML input",
        "ChartML Block 2:",
        "visible block",
        "Validation pending",
    ] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
    let html = render(json!({"input": {"chartml": "legacy"}}));
    assert!(html.contains("ChartML input unavailable"), "{html}");
    assert!(!html.contains("No ChartML provided"), "{html}");
}

#[test]
fn chartml_and_diagnostics_are_escaped_as_text() {
    let html = render(json!({
        "input": {"blocks": ["<script>alert(1)</script>"]},
        "output": {"valid": false, "errors": [{"block": 1, "message": "<img src=x onerror=alert(1)>", "path": "<svg>"}]}
    }));
    assert!(!html.contains("<script>"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert!(!html.contains("<svg>"), "{html}");
    assert!(html.contains("&lt;script&gt;"), "{html}");
    assert!(html.contains("&lt;img"), "{html}");
    assert!(html.contains("&lt;svg&gt;"), "{html}");
}
