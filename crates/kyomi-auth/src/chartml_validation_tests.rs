// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
use kyomi_datasource_server::{DatasourceProvider, DryRunResult};
use std::sync::{Arc, Mutex};
struct FakeProvider {
    calls: Arc<Mutex<Vec<String>>>,
    valid: bool,
    transport_error: bool,
    hang: bool,
    message: String,
}
#[async_trait::async_trait]
impl DatasourceProvider for FakeProvider {
    async fn test_connection(&self) -> kyomi_connect_protocol::Result<bool> {
        panic!("not a connection probe")
    }
    async fn execute_query(
        &self,
        _: &str,
        _: Option<u32>,
        _: Option<u32>,
        _: bool,
        _: Option<&str>,
    ) -> kyomi_connect_protocol::Result<kyomi_datasource_server::QueryResult> {
        panic!("must never execute an ordinary query")
    }
    async fn dry_run(&self, sql: &str) -> kyomi_connect_protocol::Result<DryRunResult> {
        self.calls.lock().unwrap().push(sql.into());
        if self.hang {
            std::future::pending::<()>().await;
        }
        if self.transport_error {
            return Err(kyomi_connect_protocol::Error::Connection(
                self.message.clone(),
            ));
        }
        Ok(if self.valid {
            DryRunResult::success(&self.message)
        } else {
            DryRunResult::failure(&self.message, None, None)
        })
    }
    async fn close(&self) {
        self.calls.lock().unwrap().push("close".into());
    }
}
#[tokio::test]
async fn actual_provider_dry_run_is_mandatory_and_unavailable_never_valid() {
    for (valid, message, expected) in [
        (true, "Query valid", true),
        (false, "Syntax error", false),
        (true, "Dry run not available for this provider", false),
    ] {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let provider = FakeProvider {
            calls: calls.clone(),
            valid,
            transport_error: false,
            hang: false,
            message: message.into(),
        };
        let errors = kyomi_core::chartml_validation::validate_blocks(&["type: chart\nversion: 1\ndata: {datasource: warehouse, query: SELECT 1}\nvisualize: {type: table}"], |source| {
            assert_eq!(source.datasource.as_deref(), Some("warehouse"));
            let provider = &provider;
            async move { validate_provider_query(provider, &source.sql).await }
        }).await;
        assert_eq!(errors.is_empty(), expected);
        assert_eq!(*calls.lock().unwrap(), vec!["SELECT 1", "close"]);
    }
}
#[tokio::test]
async fn missing_context_fails_sql_but_accepts_non_sql_document() {
    assert!(validate_content("Plain markdown", None).await.is_ok());
    let error = validate_content("```chartml\ntype: chart\nversion: 1\ndata: {datasource: warehouse, query: SELECT 1}\nvisualize: {type: table}\n```", None).await.unwrap_err();
    assert!(error.to_string().contains("authorized datasource context"));
}

#[tokio::test]
async fn dashboard_save_rejects_sql_without_context_and_preserves_inline_docs() {
    let db = crate::test_support::test_pool().await;
    let sql = "```chartml\ntype: chart\nversion: 1\ndata: {datasource: warehouse, query: SELECT 1}\nvisualize: {type: table}\n```";
    let result = crate::dashboard_service::create_dashboard(
        &db,
        "user-a",
        "ws-1",
        "Chart",
        sql,
        kyomi_core::models::DocType::Dashboard,
        None,
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("sql_unavailable"));
    let result = crate::dashboard_service::create_dashboard(&db, "user-a", "ws-1", "Chart", "```chartml\ntype: chart\nversion: 1\ndata: {provider: inline, rows: []}\nvisualize: {type: invalid}\n```", kyomi_core::models::DocType::Dashboard, None).await;
    assert!(result.unwrap_err().to_string().contains("/visualize/type"));
}

#[tokio::test]
async fn provider_transport_failures_are_sanitized_and_closed() {
    for message in [
        "Access denied",
        "Connection failure",
        "HTTP request failed for http://example.test/?password=secret123",
    ] {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let provider = FakeProvider {
            calls: calls.clone(),
            valid: true,
            transport_error: true,
            hang: false,
            message: message.into(),
        };
        let error = validate_provider_query(&provider, "SELECT 1")
            .await
            .unwrap_err();
        assert_eq!(error.stage, "sql_dry_run");
        assert!(!error.message.contains("secret123"));
        assert_eq!(*calls.lock().unwrap(), vec!["SELECT 1", "close"]);
    }
}
#[tokio::test]
async fn provider_timeout_is_failure_and_releases_resources() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = FakeProvider {
        calls: calls.clone(),
        valid: true,
        transport_error: false,
        hang: true,
        message: "".into(),
    };
    let error = validate_provider_query(&provider, "SELECT 1")
        .await
        .unwrap_err();
    assert_eq!(error.stage, "sql_timeout");
    assert_eq!(*calls.lock().unwrap(), vec!["SELECT 1", "close"]);
}

#[tokio::test]
async fn chartml_unclosed_sql_save_cannot_succeed() {
    let db = crate::test_support::test_pool().await;
    let error = crate::dashboard_service::create_dashboard(&db, "user-a", "ws-1", "Chart",
        "```chartml\ntype: chart\nversion: 1\ndata: {datasource: warehouse, query: SELECT 1}\nvisualize: {type: table}",
        kyomi_core::models::DocType::Dashboard, None).await.unwrap_err();
    assert!(error.to_string().contains("sql_unavailable"));
}
#[tokio::test]
async fn chartml_actual_provider_receives_substituted_defaults() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = FakeProvider {
        calls: calls.clone(),
        valid: true,
        transport_error: false,
        hang: false,
        message: "Query valid".into(),
    };
    let errors = kyomi_core::chartml_validation::validate_blocks(&[
        "type: chart\nversion: 1\ndata: {datasource: '{{db}}', query: 'SELECT {{count}}'}\nvisualize: {type: table}",
        "type: params\nversion: 1\nname: defaults\nparams: [{id: db, type: text, label: Database, default: warehouse}, {id: count, type: number, label: Count, default: 4}]"
    ], |source| { let provider = &provider; async move {
        assert_eq!(source.datasource.as_deref(), Some("warehouse"));
        validate_provider_query(provider, &source.sql).await
    }}).await;
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(*calls.lock().unwrap(), vec!["SELECT 4", "close"]);
}
#[test]
fn chartml_context_errors_never_expose_database_or_json_details() {
    let json_error =
        serde_json::from_str::<serde_json::Value>("secret-token-not-json").unwrap_err();
    let error = safe_context_error(
        "resolve credentials",
        &kyomi_core::Error::SerdeJson(json_error),
    );
    assert_eq!(
        error,
        "Failed to resolve credentials: internal server error"
    );
    let error = safe_context_error(
        "resolve datasource",
        &kyomi_core::Error::Sqlx(sqlx::Error::Protocol("database secret-token".into())),
    );
    assert!(!error.contains("secret-token"));
    assert_eq!(error, "Failed to resolve datasource: internal server error");
}

#[tokio::test]
async fn chartml_bare_fence_prefix_save_rejects_trailing_invalid_chart() {
    let db = crate::test_support::test_pool().await;
    let error = crate::dashboard_service::create_dashboard(
        &db,
        "user-a",
        "ws-1",
        "Chart",
        "```\n```chartml\ntype: chart\nversion: 99",
        kyomi_core::models::DocType::Dashboard,
        None,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("schema"));
}

#[tokio::test]
async fn chartml_provider_receives_quoted_dollar_sql_default() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = FakeProvider {
        calls: calls.clone(),
        valid: true,
        transport_error: false,
        hang: false,
        message: "Query valid".into(),
    };
    let errors = kyomi_core::chartml_validation::validate_blocks(
        &[r#"type: chart
version: 1
params:
- {id: sql, type: text, label: SQL, default: SELECT 8}
- {id: db, type: text, label: Database, default: warehouse}
data: {datasource: "$db", query: "$sql"}
visualize: {type: table}
"#],
        |source| {
            let provider = &provider;
            async move {
                assert_eq!(source.datasource.as_deref(), Some("warehouse"));
                validate_provider_query(provider, &source.sql).await
            }
        },
    )
    .await;
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(*calls.lock().unwrap(), vec!["SELECT 8", "close"]);
}
