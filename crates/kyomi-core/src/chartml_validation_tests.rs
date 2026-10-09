// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
use std::sync::{Arc, Mutex};

const CHART: &str =
    "type: chart\nversion: 1\ndata: {provider: inline, rows: []}\nvisualize: {type: bar}";
#[test]
fn accepts_all_component_forms() {
    for yaml in [
        CHART,
        "type: config\nversion: 1",
        "type: style\nversion: 1\nname: palette",
        "type: params\nversion: 1\nname: filters\nparams: []",
        "type: source\nversion: 1\nname: values\nprovider: inline\nrows: []",
        "- type: config\n  version: 1\n- type: style\n  version: 1\n  name: palette",
    ] {
        assert!(validate_block(yaml, 1).is_ok(), "{yaml}");
    }
}
#[test]
fn rejects_constraints_previously_ignored() {
    for yaml in [
        CHART.replace("type: bar", "type: nonsense"),
        CHART.replace("version: 1", "version: 2"),
        CHART.replace("rows: []", "rows: wrong"),
        CHART.replace("{type: bar}", "{}"),
        format!("{CHART}\nextra: true"),
        format!("{CHART}\ntransform: {{forecast: {{horizon: 0}}}}"),
        "type: params\nversion: 1\nname: p\nparams: [{id: p, type: nope, label: P, default: 1}]"
            .into(),
    ] {
        let errors = validate_block(&yaml, 2).expect_err("must reject invalid schema instance");
        assert!(errors.iter().all(|e| e.stage == "schema" && e.block == 2));
        assert!(errors.iter().any(|e| !e.instance_path.is_empty()));
    }
}
#[test]
fn rejects_non_json_yaml_without_coercion() {
    for yaml in [
        "type: !custom chart",
        "1: value",
        "type: chart\nversion: .nan",
        "type: chart\nversion: .inf",
    ] {
        let errors = validate_block(yaml, 1).expect_err("conversion must fail");
        assert_eq!(errors[0].stage, "conversion");
    }
    assert_eq!(validate_block("[", 1).unwrap_err()[0].stage, "yaml");
}
#[tokio::test]
async fn validates_every_inline_named_and_reused_source() {
    let blocks = [
        "- type: source\n  version: 1\n  name: shared\n  datasource: warehouse\n  query: SELECT 1\n- type: chart\n  version: 1\n  data: shared\n  visualize: {type: bar}",
        "type: chart\nversion: 1\ndata:\n  first: {datasource: primary, query: SELECT 2}\n  second: {datasource: secondary, query: SELECT 3}\nvisualize: {type: table}",
    ];
    let calls = Arc::new(Mutex::new(Vec::new()));
    let errors = validate_blocks(&blocks, |source| {
        calls.lock().unwrap().push((
            source.block,
            source.component,
            source.datasource,
            source.sql,
        ));
        async { Ok(()) }
    })
    .await;
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            (1, 1, Some("warehouse".into()), "SELECT 1".into()),
            (2, 1, Some("primary".into()), "SELECT 2".into()),
            (2, 1, Some("secondary".into()), "SELECT 3".into())
        ]
    );
}
#[tokio::test]
async fn schema_failure_prevents_all_datasource_calls_and_inline_needs_none() {
    for blocks in [
        vec![CHART],
        vec![
            CHART,
            "type: chart\nversion: 99\ndata: {datasource: db, query: SELECT 1}\nvisualize: {type: bar}",
        ],
    ] {
        let calls = Arc::new(Mutex::new(0));
        let errors = validate_blocks(&blocks, |_| {
            *calls.lock().unwrap() += 1;
            async { Ok(()) }
        })
        .await;
        assert_eq!(*calls.lock().unwrap(), 0);
        assert_eq!(errors.is_empty(), blocks.len() == 1);
    }
}
#[tokio::test]
async fn datasource_failures_cannot_be_success() {
    let chart =
        "type: chart\nversion: 1\ndata: {datasource: db, query: BAD SQL}\nvisualize: {type: bar}";
    for reason in [
        "SQL syntax error",
        "datasource not found",
        "access denied",
        "connection failure",
        "timeout",
        "unsupported dry-run",
    ] {
        let errors = validate_blocks(&[chart], |_| async { Err(reason.into()) }).await;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].stage, "sql_dry_run");
        assert_eq!(errors[0].instance_path, "/data/query");
        assert_eq!(errors[0].message, reason);
    }
}

#[test]
fn schema_asset_identity_and_initialization_fail_closed() {
    let master: Value = serde_json::from_str(SCHEMA).unwrap();
    let minified: Value = serde_json::from_str(include_str!(
        "../../../data/chartml-spec/chartml_schema.min.json"
    ))
    .unwrap();
    assert_eq!(master, minified);
    assert_eq!(master["$schema"], "http://json-schema.org/draft-07/schema#");
    assert!(compile_schema(&serde_json::json!({"type": "invalid-type"})).is_err());
    assert!(compile_schema(&serde_json::json!({"$ref": "#/definitions/not-present"})).is_err());
}

#[tokio::test]
async fn unresolved_named_source_is_not_validation_success() {
    let errors = validate_blocks(
        &["type: chart\nversion: 1\ndata: absent_source\nvisualize: {type: bar}"],
        |_| async { panic!("no datasource call without source") },
    )
    .await;
    assert_eq!(errors[0].stage, "source_resolution");
    assert_eq!(errors[0].instance_path, "/data");
}

#[test]
fn renderable_same_line_closing_fence_is_validated() {
    let content = "```chartml\ntype: chart\nversion: 99```";
    assert_eq!(markdown_blocks(content).len(), 1);
    assert!(!validate_markdown_schema(content).is_empty());
    assert_eq!(strip_markdown_blocks(content, &[0], ""), "");
}
#[tokio::test]
async fn failed_named_sql_source_invalidates_dependent_blocks() {
    let definition = "type: source\nversion: 1\nname: shared\ndatasource: db\nquery: BAD SQL";
    let chart = "type: chart\nversion: 1\ndata: shared\nvisualize: {type: bar}";
    let errors = validate_blocks(&[definition, chart, CHART], |_| async {
        Err("syntax error".into())
    })
    .await;
    assert!(
        errors
            .iter()
            .any(|e| e.block == 1 && e.stage == "sql_dry_run")
    );
    assert!(
        errors
            .iter()
            .any(|e| e.block == 2 && e.stage == "sql_dry_run" && e.instance_path == "/data")
    );
    assert!(!errors.iter().any(|e| e.block == 3));
    let duplicate = validate_blocks(&[definition, definition, chart], |_| async { Ok(()) }).await;
    assert!(
        duplicate.iter().any(
            |e| e.block == 3 && (e.stage == "source_resolution" || e.stage == "sql_parameters")
        )
    );
}

#[tokio::test]
async fn chartml_named_query_key_and_refs_are_checked() {
    let named = "type: chart\nversion: 1\ndata: {query: {datasource: warehouse, query: BAD SQL}, sibling: {datasource: warehouse, query: SELECT 2}}\nvisualize: {type: bar}";
    let calls = Arc::new(Mutex::new(Vec::new()));
    let errors = validate_blocks(&[named, CHART], |source| {
        calls.lock().unwrap().push(source.sql.clone());
        async move {
            if source.sql == "BAD SQL" {
                Err("syntax".into())
            } else {
                Ok(())
            }
        }
    })
    .await;
    assert_eq!(*calls.lock().unwrap(), vec!["BAD SQL", "SELECT 2"]);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].instance_path, "/data/query/query");
    let references =
        "type: chart\nversion: 1\ndata: {query: missing, sibling: other}\nvisualize: {type: bar}";
    let errors = validate_blocks(&[references], |_| async { panic!("no SQL") }).await;
    assert!(errors.iter().any(|e| e.instance_path == "/data/query"));
    assert!(errors.iter().any(|e| e.instance_path == "/data/sibling"));
}
#[tokio::test]
async fn chartml_parameters_resolve_cross_block_defaults_before_provider() {
    let chart = "- type: config\n  version: 1\n- type: chart\n  version: 1\n  params: [{id: amount, type: number, label: Amount, default: 7}]\n  data: {datasource: '{{warehouse}}', query: 'SELECT {{amount}}, {{flag}}, \"{{label}}\"'}\n  visualize: {type: table}";
    let params = "type: params\nversion: 1\nname: defaults\nparams: [{id: warehouse, type: text, label: Warehouse, default: db}, {id: amount, type: number, label: Amount, default: 2}, {id: flag, type: text, label: Flag, default: true}, {id: label, type: text, label: Label, default: hello}]";
    let errors = validate_blocks(&[chart, params], |source| async move {
        assert_eq!(source.block, 1);
        assert_eq!(source.component, 2);
        assert_eq!(source.instance_path, "/1/data/query");
        assert_eq!(source.datasource.as_deref(), Some("db"));
        assert_eq!(source.sql, "SELECT 2, true, \"hello\"");
        Ok(())
    })
    .await;
    assert!(errors.is_empty(), "{errors:?}");
    let unresolved =
        "type: source\nversion: 1\nname: shared\ndatasource: db\nquery: SELECT {{absent}}";
    let ref_chart = "type: chart\nversion: 1\ndata: {query: shared}\nvisualize: {type: bar}";
    let errors = validate_blocks(&[unresolved, ref_chart], |_| async {
        panic!("unresolved SQL must never reach provider")
    })
    .await;
    assert!(
        errors
            .iter()
            .any(|e| e.block == 1 && e.stage == "sql_parameters")
    );
    assert!(
        errors.iter().any(|e| e.block == 2
            && e.stage == "sql_parameters"
            && e.instance_path == "/data/query")
    );
}
#[test]
fn chartml_scanner_covers_unclosed_tail_and_ignores_nested_code() {
    let content = "```sql\n```chartml\nnot executable\n```\n```chartml\ntype: chart\nversion: 99";
    assert_eq!(markdown_blocks(content).len(), 1);
    let errors = validate_markdown_schema(content);
    assert!(!errors.is_empty());
    assert!(errors.iter().all(|e| e.block == 1));
    assert_eq!(
        strip_markdown_blocks(content, &[0], "$1"),
        "```sql\n```chartml\nnot executable\n```\n$1"
    );
}

#[tokio::test]
async fn chartml_query_key_positive_references_never_become_sql() {
    let chart = "type: chart\nversion: 1\ndata: {query: shared}\nvisualize: {type: bar}";
    let inline = "type: source\nversion: 1\nname: shared\nprovider: inline\nrows: []";
    assert!(
        validate_blocks(&[inline, chart], |_| async {
            panic!("inline reference is not SQL")
        })
        .await
        .is_empty()
    );
    let sql = "type: source\nversion: 1\nname: shared\ndatasource: warehouse\nquery: SELECT 1";
    let calls = Arc::new(Mutex::new(Vec::new()));
    let errors = validate_blocks(&[sql, chart], |source| {
        calls.lock().unwrap().push(source.sql);
        async { Ok(()) }
    })
    .await;
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(*calls.lock().unwrap(), vec!["SELECT 1"]);
}
#[test]
fn chartml_bare_fence_prefix_cannot_hide_unclosed_chart() {
    let content = "```\n```chartml\ntype: chart\nversion: 99";
    assert_eq!(markdown_blocks(content).len(), 1);
    assert!(!validate_markdown_schema(content).is_empty());
    assert_eq!(
        strip_markdown_blocks(content, &[0], "removed"),
        "```\nremoved"
    );
}
#[tokio::test]
async fn chartml_parameter_defaults_from_failed_blocks_invalidate_consumers() {
    let owner = "- type: params\n  version: 1\n  name: defaults\n  params: [{id: amount, type: number, label: Amount, default: 2}]\n- type: source\n  version: 1\n  name: bad\n  datasource: db\n  query: BAD SQL";
    let consumer =
        "type: source\nversion: 1\nname: consumer\ndatasource: db\nquery: SELECT {{amount}}";
    let reference = "type: chart\nversion: 1\ndata: {query: consumer}\nvisualize: {type: table}";
    let errors = validate_blocks(&[owner, consumer, reference, CHART], |source| async move {
        if source.sql == "BAD SQL" {
            Err("syntax error".into())
        } else {
            assert_eq!(source.sql, "SELECT 2");
            Ok(())
        }
    })
    .await;
    assert!(
        errors
            .iter()
            .any(|e| e.block == 2 && e.stage == "sql_parameters")
    );
    assert!(
        errors.iter().any(
            |e| e.block == 3 && (e.stage == "source_resolution" || e.stage == "sql_parameters")
        )
    );
    assert!(!errors.iter().any(|e| e.block == 4));
}

#[tokio::test]
async fn chartml_local_only_curly_default_is_unresolved() {
    let chart = "type: chart\nversion: 1\nparams: [{id: amount, type: number, label: Amount, default: 7}]\ndata: {datasource: db, query: 'SELECT {{amount}}'}\nvisualize: {type: table}";
    let errors = validate_blocks(&[chart], |_| async {
        panic!("UI does not substitute local-only curly defaults")
    })
    .await;
    assert_eq!(errors[0].stage, "sql_parameters");
}

#[tokio::test]
async fn chartml_retained_fallback_defaults_are_resolved_and_dry_run_again() {
    let owner = "- type: params\n  version: 1\n  name: original\n  params: [{id: amount, type: number, label: Amount, default: 2}, {id: db, type: text, label: Database, default: first}]\n- type: source\n  version: 1\n  name: bad\n  datasource: first\n  query: BAD SQL";
    let fallback = "type: params\nversion: 1\nname: fallback\nparams: [{id: amount, type: number, label: Amount, default: 9}, {id: db, type: text, label: Database, default: second}]";
    let consumer = "type: chart\nversion: 1\ndata: {datasource: '{{db}}', query: 'SELECT {{amount}}'}\nvisualize: {type: table}";
    let calls = Arc::new(Mutex::new(Vec::new()));
    let errors = validate_blocks(&[owner, fallback, consumer, CHART], |source| {
        calls
            .lock()
            .unwrap()
            .push((source.block, source.datasource.unwrap(), source.sql.clone()));
        async move {
            if source.sql == "BAD SQL" {
                Err("syntax".into())
            } else {
                Ok(())
            }
        }
    })
    .await;
    assert!(errors.iter().all(|e| e.block == 1), "{errors:?}");
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            (1, "first".into(), "BAD SQL".into()),
            (3, "first".into(), "SELECT 2".into()),
            (3, "second".into(), "SELECT 9".into())
        ]
    );
}
#[tokio::test]
async fn chartml_valid_source_lost_with_bad_sibling_invalidates_reference() {
    let owner = "- type: source\n  version: 1\n  name: good\n  provider: inline\n  rows: []\n- type: source\n  version: 1\n  name: bad\n  datasource: db\n  query: BAD SQL";
    let dependent = "type: chart\nversion: 1\ndata: {query: good}\nvisualize: {type: bar}";
    let errors = validate_blocks(&[owner, dependent, CHART], |_| async {
        Err("syntax".into())
    })
    .await;
    assert!(
        errors
            .iter()
            .any(|e| e.block == 2 && e.stage == "source_resolution")
    );
    assert!(!errors.iter().any(|e| e.block == 3));
}

#[tokio::test]
async fn chartml_quoted_dollar_defaults_resolve_actual_sql_and_schema_types() {
    let chart = r#"type: chart
version: 1
params:
- {id: sql, type: text, label: SQL, default: SELECT 7}
- {id: warehouse, type: text, label: Warehouse, default: db}
- {id: kind, type: text, label: Kind, default: table}
data: {datasource: "$warehouse", query: "$sql"}
visualize: {type: "$kind"}
"#;
    let errors = validate_blocks(&[chart], |source| async move {
        assert_eq!(source.datasource.as_deref(), Some("db"));
        assert_eq!(source.sql, "SELECT 7");
        assert_eq!(source.instance_path, "/data/query");
        Ok(())
    })
    .await;
    assert!(errors.is_empty(), "{errors:?}");
    for replacement in ["7", "true", "[one, two]", "{nested: value}"] {
        let invalid = chart.replace("default: SELECT 7", &format!("default: {replacement}"));
        let errors = validate_blocks(&[&invalid], |_| async {
            panic!("resolved non-string SQL fails schema before provider")
        })
        .await;
        assert!(
            errors
                .iter()
                .any(|e| e.stage == "schema" && e.instance_path == "/data/query"),
            "{errors:?}"
        );
    }
    let unresolved = chart.replace("query: \"$sql\"", "query: \"$missing\"");
    let errors = validate_blocks(&[&unresolved], |_| async {
        panic!("unresolved quoted reference")
    })
    .await;
    assert_eq!(errors[0].stage, "sql_parameters");
}
