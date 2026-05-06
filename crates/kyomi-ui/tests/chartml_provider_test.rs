// SPDX-License-Identifier: AGPL-3.0-or-later

//! Integration tests for `KyomiDatasourceProvider` — the chartml 5.0
//! `DataSourceProvider` impl that bridges chartml's resolver onto Kyomi's
//! Arrow streaming endpoint (KYO-243).
//!
//! These tests exercise the public `DataSourceProvider::fetch` entry point
//! end-to-end. The querier is mocked via `DatasourceQuerier` so we can inject
//! a `DataTable` directly without browser APIs or a Leptos server context.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chartml_core::data::{DataTable, Row};
use chartml_core::spec::InlineData;
use chartml_core::{DataSourceProvider, FetchError, FetchRequest};

use kyomi_ui::chartml_provider::{
    DatasourceQuerier, DatasourceQuerierRef, KyomiDatasourceProvider,
};

// ────────────────────────────────────────────────────────────────────────────
// Test helpers
// ────────────────────────────────────────────────────────────────────────────

/// Build a `FetchRequest` for the given inline-data spec. The other fields
/// are set to defaults appropriate for tests — only the fields the provider
/// actually reads (`spec.datasource`, `spec.query`) need to vary across tests.
fn request(spec: InlineData) -> FetchRequest {
    FetchRequest {
        source_name: None,
        spec,
        cache: None,
        headers: HashMap::new(),
        namespace: None,
        cancel_token: None,
    }
}

fn empty_inline() -> InlineData {
    InlineData {
        provider: None,
        rows: None,
        url: None,
        endpoint: None,
        cache: None,
        datasource: None,
        query: None,
    }
}

/// Recorded call captured by [`MockQuerier`] — what arguments
/// [`DataSourceProvider::fetch`] forwarded to the querier.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedCall {
    slug: String,
    sql: String,
}

/// Mock [`DatasourceQuerier`] that records every call and returns a
/// pre-built `DataTable`. Used to assert the provider forwards the exact
/// `(slug, query)` pair the resolver dispatched against.
struct MockQuerier {
    calls: Mutex<Vec<RecordedCall>>,
    response: DataTable,
}

impl MockQuerier {
    fn new(response: DataTable) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            response,
        }
    }

    fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().expect("mutex not poisoned").clone()
    }
}

#[async_trait]
impl DatasourceQuerier for MockQuerier {
    async fn query(
        &self,
        datasource_slug: String,
        sql: String,
    ) -> Result<DataTable, String> {
        self.calls.lock().expect("mutex not poisoned").push(RecordedCall {
            slug: datasource_slug,
            sql,
        });
        Ok(self.response.clone())
    }
}

/// Build a `DataTable` of two rows {x: "A", y: 1} / {x: "B", y: 2}.
fn two_row_table() -> DataTable {
    let rows: Vec<Row> = vec![
        [
            ("x".to_string(), serde_json::json!("A")),
            ("y".to_string(), serde_json::json!(1)),
        ]
        .into_iter()
        .collect(),
        [
            ("x".to_string(), serde_json::json!("B")),
            ("y".to_string(), serde_json::json!(2)),
        ]
        .into_iter()
        .collect(),
    ];
    DataTable::from_rows(&rows).expect("from_rows must succeed for valid rows")
}

// ────────────────────────────────────────────────────────────────────────────
// Construction
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn provider_carries_workspace_id() {
    let p = KyomiDatasourceProvider::new("acme-corp");
    assert_eq!(p.workspace_id(), "acme-corp");
}

// ────────────────────────────────────────────────────────────────────────────
// Querier forwarding — the provider must pass (slug, query) verbatim to the
// querier and return the DataTable it receives, with rows_returned metadata.
// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_provider_forwards_slug_and_query() {
    let mock = Arc::new(MockQuerier::new(two_row_table()));
    let querier: DatasourceQuerierRef = mock.clone();
    let provider = KyomiDatasourceProvider::with_querier("ws-1", querier);

    let spec = InlineData {
        datasource: Some("my-slug".to_string()),
        query: Some("SELECT 1".to_string()),
        ..empty_inline()
    };

    let result = provider
        .fetch(request(spec))
        .await
        .expect("fetch must succeed with a valid mock response");

    // The DataTable reaches the caller unchanged.
    assert_eq!(result.data.num_rows(), 2);

    // rows_returned metadata is populated from the actual row count.
    assert_eq!(
        result.metadata.get("rows_returned"),
        Some(&serde_json::Value::from(2usize)),
        "rows_returned must reflect actual row count",
    );

    // The streaming path does not produce execution_time_ms.
    assert!(
        !result.metadata.contains_key("execution_time_ms"),
        "streaming path must not produce execution_time_ms metadata",
    );

    // The exact (slug, query) pair the resolver dispatched must reach the
    // querier verbatim.
    let calls = mock.calls();
    assert_eq!(calls.len(), 1, "exactly one querier call per fetch");
    assert_eq!(
        calls[0],
        RecordedCall {
            slug: "my-slug".to_string(),
            sql: "SELECT 1".to_string(),
        },
        "provider must forward the resolver's (slug, query) pair verbatim",
    );
}

#[tokio::test]
async fn test_provider_propagates_query_failure_as_query_failed() {
    // A failing querier call must become `FetchError::QueryFailed(_)` so the
    // resolver's `on_error` hook can classify network/auth/SQL errors
    // separately from other errors.
    struct FailingQuerier;
    #[async_trait]
    impl DatasourceQuerier for FailingQuerier {
        async fn query(
            &self,
            _slug: String,
            _sql: String,
        ) -> Result<DataTable, String> {
            Err("boom".to_string())
        }
    }

    let provider =
        KyomiDatasourceProvider::with_querier("ws-1", Arc::new(FailingQuerier));
    let spec = InlineData {
        datasource: Some("ds".to_string()),
        query: Some("SELECT 1".to_string()),
        ..empty_inline()
    };

    let err = provider
        .fetch(request(spec))
        .await
        .expect_err("querier failure must surface as a FetchError");
    match err {
        FetchError::QueryFailed(msg) => {
            assert!(
                msg.contains("boom"),
                "QueryFailed message must include the underlying error: {msg}",
            );
        }
        other => panic!("expected QueryFailed, got: {other:?}"),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Input validation — exercised through the trait's fetch() entry point so
// the routing-friendly `FetchError` variants are observed end-to-end.
// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn fetch_missing_slug_returns_slug_not_found() {
    // A spec that reaches this provider with NO datasource is a host wiring
    // bug (the resolver normally only routes datasource-shape specs here).
    // The provider returns `SlugNotFound` rather than panicking so the
    // mistake surfaces with a recognizable error variant.
    let provider = KyomiDatasourceProvider::new("ws-1");
    let spec = InlineData {
        // No datasource set.
        query: Some("SELECT 1".to_string()),
        ..empty_inline()
    };
    let err = provider
        .fetch(request(spec))
        .await
        .expect_err("missing datasource must error");
    assert!(
        matches!(err, FetchError::SlugNotFound { .. }),
        "expected SlugNotFound, got: {err:?}",
    );
}

#[tokio::test]
async fn fetch_missing_query_returns_other() {
    let provider = KyomiDatasourceProvider::new("ws-1");
    let spec = InlineData {
        datasource: Some("warehouse".to_string()),
        // No query set.
        ..empty_inline()
    };
    let err = provider
        .fetch(request(spec))
        .await
        .expect_err("missing query must error");
    match err {
        FetchError::Other(msg) => {
            assert!(
                msg.contains("missing query"),
                "Other(...) message must mention missing query: {msg}",
            );
        }
        other => panic!("expected Other(...), got: {other:?}"),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Trait-object compatibility — the provider must be storable behind the
// `Arc<dyn DataSourceProvider>` alias the resolver uses, both natively and
// (by implication via the cfg_attr) on WASM. Plain compile-time check.
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn provider_is_arc_dyn_compatible() {
    let provider: Arc<dyn DataSourceProvider> =
        Arc::new(KyomiDatasourceProvider::new("ws"));
    // Sanity — the trait object is non-null.
    assert!(Arc::strong_count(&provider) >= 1);
}
