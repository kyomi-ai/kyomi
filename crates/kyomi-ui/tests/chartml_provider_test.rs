// SPDX-License-Identifier: AGPL-3.0-or-later

//! Integration tests for `KyomiDatasourceProvider` — the chartml 5.0
//! `DataSourceProvider` impl that bridges chartml's resolver onto Kyomi's
//! `query_datasource_arrow` server function (KYO-79 phase 6).
//!
//! These tests exercise the public `DataSourceProvider::fetch` entry point
//! end-to-end. The decode-pipeline tests live in a `#[cfg(test)] mod tests`
//! block inside `chartml_provider.rs` so the underlying `build_fetch_result`
//! helper can stay `pub(crate)` rather than leaking into the crate's public
//! API.
//!
//! The actual server-fn call (`query_datasource_arrow`) is hidden behind the
//! `DatasourceQuerier` trait so we can mock it here without standing up a
//! Leptos server context.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine;
use chartml_core::data::{DataTable, Row};
use chartml_core::spec::InlineData;
use chartml_core::{DataSourceProvider, FetchError, FetchRequest};
use leptos::server_fn::ServerFnError;

use kyomi_ui::chartml_provider::{
    DatasourceQuerier, DatasourceQuerierRef, KyomiDatasourceProvider,
};
use kyomi_ui::server_fns::datasources::QueryArrowResult;

// ────────────────────────────────────────────────────────────────────────────
// Test helpers
// ────────────────────────────────────────────────────────────────────────────

/// Build a `FetchRequest` for the given inline-data spec. The other fields
/// (`source_name`, `cache`, `headers`, `namespace`, `cancel_token`) are set
/// to defaults appropriate for the test — only the fields the provider
/// actually reads (`spec.datasource`, `spec.query`) need to vary across
/// tests.
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
/// [`DataSourceProvider::fetch`] forwarded to the server-fn shim.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedCall {
    slug: String,
    sql: String,
    limit: Option<i32>,
}

/// Mock [`DatasourceQuerier`] that records every call and returns a
/// pre-canned `QueryArrowResult`. Used to assert the provider forwards the
/// exact `(slug, query, limit)` tuple the resolver dispatched against.
struct MockQuerier {
    calls: Mutex<Vec<RecordedCall>>,
    response: QueryArrowResult,
}

impl MockQuerier {
    fn new(response: QueryArrowResult) -> Self {
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
        limit: Option<i32>,
    ) -> Result<QueryArrowResult, ServerFnError> {
        self.calls.lock().expect("mutex not poisoned").push(RecordedCall {
            slug: datasource_slug,
            sql,
            limit,
        });
        Ok(self.response.clone())
    }
}

/// Build a `DataTable` of two rows {x: "A", y: 1} / {x: "B", y: 2} and
/// serialize it to base64-encoded Arrow IPC bytes — matching the wire
/// format `query_datasource_arrow` returns. Used by the mock as a canned
/// response so `build_fetch_result` decodes successfully.
fn known_table_ipc_b64() -> String {
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
    let table = DataTable::from_rows(&rows).expect("from_rows must succeed for valid rows");
    let bytes = table.to_ipc_bytes().expect("to_ipc_bytes must succeed");
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

fn canned_response() -> QueryArrowResult {
    QueryArrowResult {
        ipc_base64: known_table_ipc_b64(),
        num_rows: 2,
        execution_time_ms: Some(42),
    }
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
// Server-fn forwarding — Phase 6 plan §1204-1205 spec-required test.
//
// The resolver only routes datasource-shape specs to this provider. When it
// does, the provider must forward the slug + query verbatim to
// `query_datasource_arrow`, with `limit = None` so the user's SQL is the only
// thing constraining row count. Adding a default cap here would silently
// truncate dashboards that worked before Phase 6.
// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_provider_calls_query_datasource_arrow() {
    let mock = Arc::new(MockQuerier::new(canned_response()));
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

    // The decoded payload reaches the caller unchanged — proves the result
    // path runs through `build_fetch_result` after the mock returns.
    assert_eq!(result.data.num_rows(), 2);

    // The exact (slug, query, limit) tuple the resolver dispatched against
    // must reach `query_datasource_arrow` verbatim. `limit = None` is
    // load-bearing — see the comment above the test.
    let calls = mock.calls();
    assert_eq!(calls.len(), 1, "exactly one server-fn call per fetch");
    assert_eq!(
        calls[0],
        RecordedCall {
            slug: "my-slug".to_string(),
            sql: "SELECT 1".to_string(),
            limit: None,
        },
        "provider must forward the resolver's (slug, query, None) tuple verbatim",
    );
}

#[tokio::test]
async fn test_provider_propagates_query_failure_as_query_failed() {
    // A failing server-fn call must become `FetchError::QueryFailed(_)` so
    // the resolver's `on_error` hook can classify network/auth/SQL errors
    // separately from decode failures.
    struct FailingQuerier;
    #[async_trait]
    impl DatasourceQuerier for FailingQuerier {
        async fn query(
            &self,
            _slug: String,
            _sql: String,
            _limit: Option<i32>,
        ) -> Result<QueryArrowResult, ServerFnError> {
            Err(ServerFnError::new("boom"))
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
        .expect_err("server-fn failure must surface as a FetchError");
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
