// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for BigQuery catalog/search/info endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) for the BigQuery endpoints added in Phase 7F:
//!
//! - `GET /api/v1/bigquery/catalog` — Full catalog tree
//! - `GET /api/v1/bigquery/catalog/projects` — Project list with dataset counts
//! - `GET /api/v1/bigquery/catalog/{project}/datasets` — Datasets in a project
//! - `GET /api/v1/bigquery/catalog/{project}/{dataset}/tables` — Tables in dataset
//! - `GET /api/v1/bigquery/catalog/{project}/{dataset}/{table}/columns` — Columns
//! - `GET /api/v1/bigquery/catalog/status` — Refresh status
//! - `POST /api/v1/bigquery/catalog/refresh` — Trigger refresh
//! - `POST /api/v1/bigquery/catalog/projects/add` — Add projects
//! - `POST /api/v1/bigquery/catalog/projects/remove` — Remove project
//! - `POST /api/v1/bigquery/catalog/settings` — Deprecated settings
//! - `GET /api/v1/bigquery/projects/listAccessible` — List GCP projects
//! - `POST /api/v1/bigquery/search` — Semantic search
//! - `POST /api/v1/bigquery/info` — Table info
//!
//! Test organization:
//! - Section 1: Unauthenticated 401 tests
//! - Section 2: Authenticated response shape tests (catalog, search, info)
//! - Section 3: Admin-only tests
//! - Section 4: Deprecated endpoint test

use serde_json::{json, Value};

use kyomi_test_harness::{base_url, cleanup_test_user, AuthContext};

// ===========================================================================
// Test infrastructure
// ===========================================================================

async fn setup_auth_context(suffix: &str) -> Option<AuthContext> {
    kyomi_test_harness::setup_auth_context("BQ Test User", "bq", suffix).await
}

/// Clean up a specific datasource and its table cache entries.
async fn cleanup_datasource(db: &kyomi_core::DbPool, workspace_id: &str, slug: &str) {
    let ds_id: Option<String> = match db {
        kyomi_core::db::DbPool::Postgres(pg) =>
            sqlx::query_scalar::<_, String>("SELECT id FROM datasource_configs WHERE workspace_id = $1 AND slug = $2")
                .bind(workspace_id).bind(slug).fetch_optional(pg).await.unwrap_or(None),
        kyomi_core::db::DbPool::Sqlite(sq) =>
            sqlx::query_scalar::<_, String>("SELECT id FROM datasource_configs WHERE workspace_id = $1 AND slug = $2")
                .bind(workspace_id).bind(slug).fetch_optional(sq).await.unwrap_or(None),
    };

    if let Some(id) = ds_id {
        let _ = kyomi_core::db_execute!(db, "DELETE FROM datasource_search_embeddings WHERE datasource_config_id = $1", &id);
        let _ = kyomi_core::db_execute!(db, "DELETE FROM datasource_table_cache WHERE datasource_config_id = $1", &id);
        let _ = kyomi_core::db_execute!(db, "DELETE FROM user_datasource_credentials WHERE datasource_config_id = $1", &id);
        let _ = kyomi_core::db_execute!(db, "DELETE FROM user_datasource_preferences WHERE datasource_config_id = $1", &id);
        let _ = kyomi_core::db_execute!(db, "DELETE FROM datasource_configs WHERE id = $1", &id);
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn auth_get(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .get(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={token}"))
}

fn auth_post(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .post(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={token}"))
}

/// Create a BigQuery test datasource directly via the service layer.
async fn create_bq_test_datasource(ctx: &AuthContext, slug: &str) -> (String, String) {
    let ds = kyomi_auth::datasource_service::create_datasource(
        &ctx.db,
        &ctx.workspace_id,
        &format!("BQ Test {slug}"),
        Some(slug),
        "bigquery",
        json!({
            "auth_mode": "kyomi_oauth",
            "catalog_projects": ["test-project-1", "test-project-2"],
            "include_public_datasets": false
        }),
        None, // direct connection
    )
    .await
    .expect("should create test BigQuery datasource");

    (ds.id, ds.slug)
}

/// Insert test rows into datasource_table_cache for BigQuery-style data.
async fn insert_bq_test_tables(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    datasource_id: &str,
    tables: &[(&str, &str, &str, Value)],
) {
    let is_archived_val = if db.is_postgres() { "false" } else { "0" };
    let sql = format!(
        "INSERT INTO datasource_table_cache \
         (workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
          table_metadata, is_archived) \
         VALUES ($1, $2, $3, $4, $5, $6, {is_archived_val})"
    );
    for (project_id, dataset_id, table_id, metadata) in tables {
        let metadata_str = serde_json::to_string(metadata).unwrap();
        kyomi_core::db_execute!(
            db,
            &sql,
            workspace_id,
            datasource_id,
            *project_id,
            *dataset_id,
            *table_id,
            &metadata_str
        )
        .expect("should insert test table cache row");
    }
}

// ===========================================================================
// 1. Unauthenticated 401 tests
// ===========================================================================

#[tokio::test]
async fn bq_catalog_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/bigquery/catalog"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /catalog without auth should be 401");
}

#[tokio::test]
async fn bq_catalog_projects_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/bigquery/catalog/projects"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /catalog/projects without auth should be 401"
    );
}

#[tokio::test]
async fn bq_catalog_status_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/bigquery/catalog/status"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /catalog/status without auth should be 401"
    );
}

#[tokio::test]
async fn bq_search_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/bigquery/search"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(json!({"query": "test"}).to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /search without auth should be 401"
    );
}

#[tokio::test]
async fn bq_info_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/bigquery/info"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(json!({"table_id": "project.dataset.table"}).to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /info without auth should be 401");
}

#[tokio::test]
async fn bq_list_accessible_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!(
            "{base}/api/v1/bigquery/projects/listAccessible"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /projects/listAccessible without auth should be 401"
    );
}

#[tokio::test]
async fn bq_catalog_refresh_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/bigquery/catalog/refresh"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /catalog/refresh without auth should be 401"
    );
}

// ===========================================================================
// 2. Catalog tree response shape (authenticated)
// ===========================================================================

#[tokio::test]
async fn bq_catalog_returns_correct_shape() {
    let ctx = setup_auth_context("bq-cat").await;
    if ctx.is_none() {
        eprintln!("SKIP: bq_catalog_returns_correct_shape — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let (ds_id, _ds_slug) = create_bq_test_datasource(&ctx, "bq-cat-tree").await;

    // Insert BigQuery-style tables
    insert_bq_test_tables(
        &ctx.db,
        &ctx.workspace_id,
        &ds_id,
        &[
            (
                "test-project-1",
                "analytics",
                "events",
                json!({
                    "columns": [
                        {"name": "event_id", "type": "STRING"},
                        {"name": "timestamp", "type": "TIMESTAMP"}
                    ],
                    "table_description": "Analytics events",
                    "table_type": "TABLE",
                    "row_count": 1000000
                }),
            ),
            (
                "test-project-1",
                "analytics",
                "sessions",
                json!({
                    "columns": [
                        {"name": "session_id", "type": "STRING"}
                    ],
                    "table_description": "User sessions",
                    "table_type": "TABLE",
                    "row_count": 50000
                }),
            ),
            (
                "test-project-1",
                "sales",
                "orders",
                json!({
                    "columns": [
                        {"name": "order_id", "type": "STRING"},
                        {"name": "total", "type": "FLOAT64"}
                    ],
                    "description": "Customer orders",
                    "table_type": "TABLE",
                    "row_count": 10000
                }),
            ),
        ],
    )
    .await;

    // GET /catalog
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/bigquery/catalog",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("catalog request should succeed");

    assert_eq!(resp.status(), 200, "catalog should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    assert_eq!(body["status"], "success", "status should be 'success'");
    assert_eq!(body["total_tables"], 3, "total_tables should be 3");
    assert!(body.get("catalog").is_some(), "missing 'catalog'");

    // Verify catalog structure: {project: {dataset: [tables]}}
    let catalog = &body["catalog"];
    assert!(
        catalog.get("test-project-1").is_some(),
        "catalog should contain test-project-1"
    );

    let project = &catalog["test-project-1"];
    assert!(
        project.get("analytics").is_some(),
        "project should contain 'analytics' dataset"
    );
    assert!(
        project.get("sales").is_some(),
        "project should contain 'sales' dataset"
    );

    // Verify tables in analytics dataset
    let analytics_tables = project["analytics"].as_array().unwrap();
    assert_eq!(
        analytics_tables.len(),
        2,
        "analytics dataset should have 2 tables"
    );

    // Verify table shape
    let first_table = &analytics_tables[0];
    assert!(
        first_table.get("table_id").is_some(),
        "table should have 'table_id'"
    );
    assert!(
        first_table.get("full_table_id").is_some(),
        "table should have 'full_table_id'"
    );
    assert!(
        first_table.get("description").is_some(),
        "table should have 'description'"
    );
    assert!(
        first_table.get("updated_at").is_some(),
        "table should have 'updated_at'"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "bq-cat-tree").await;
    cleanup_test_user(&ctx.db, "bq-test-bq-cat@contract-test.local").await;
}

// ===========================================================================
// 3. Catalog projects response
// ===========================================================================

#[tokio::test]
async fn bq_catalog_projects_returns_correct_shape() {
    let ctx = setup_auth_context("bq-proj").await;
    if ctx.is_none() {
        eprintln!("SKIP: bq_catalog_projects_returns_correct_shape — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let (ds_id, _ds_slug) = create_bq_test_datasource(&ctx, "bq-cat-proj").await;

    insert_bq_test_tables(
        &ctx.db,
        &ctx.workspace_id,
        &ds_id,
        &[
            (
                "my-project",
                "dataset_a",
                "table1",
                json!({"columns": []}),
            ),
            (
                "my-project",
                "dataset_b",
                "table2",
                json!({"columns": []}),
            ),
        ],
    )
    .await;

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/bigquery/catalog/projects",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("catalog projects request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(body["status"], "success");

    let projects = body["projects"].as_array().unwrap();
    assert!(!projects.is_empty(), "projects should not be empty");

    // Find our project
    let our_project = projects
        .iter()
        .find(|p| p["project_id"] == "my-project")
        .expect("should find 'my-project'");
    assert_eq!(
        our_project["dataset_count"], 2,
        "my-project should have 2 datasets"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "bq-cat-proj").await;
    cleanup_test_user(&ctx.db, "bq-test-bq-proj@contract-test.local").await;
}

// ===========================================================================
// 4. Search response shape (empty query)
// ===========================================================================

#[tokio::test]
async fn bq_search_empty_query_returns_empty_results() {
    let ctx = setup_auth_context("bq-search-empty").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: bq_search_empty_query_returns_empty_results — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/bigquery/search",
        &ctx.access_token,
    )
    .body(json!({"query": ""}).to_string())
    .send()
    .await
    .expect("search request should succeed");

    assert_eq!(resp.status(), 200, "search should return 200");

    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(body["status"], "success");
    assert_eq!(body["results_count"], 0);
    assert!(body["results"].is_array());
    assert!(
        body["results"].as_array().unwrap().is_empty(),
        "empty query should return empty results"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "bq-test-bq-search-empty@contract-test.local").await;
}

// ===========================================================================
// 5. Search response shape (with query, no indexed data)
// ===========================================================================

#[tokio::test]
async fn bq_search_returns_correct_response_shape() {
    let ctx = setup_auth_context("bq-search-shape").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: bq_search_returns_correct_response_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/bigquery/search",
        &ctx.access_token,
    )
    .body(
        json!({
            "query": "customer orders",
            "limit": 5
        })
        .to_string(),
    )
    .send()
    .await
    .expect("search request should succeed");

    assert_eq!(resp.status(), 200, "search should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify response shape
    assert_eq!(body["status"], "success");
    assert!(body.get("query").is_some(), "missing 'query'");
    assert_eq!(body["query"], "customer orders");
    assert!(
        body.get("results_count").is_some(),
        "missing 'results_count'"
    );
    assert!(body.get("results").is_some(), "missing 'results'");
    assert!(body["results"].is_array(), "'results' should be an array");

    // Clean up
    cleanup_test_user(&ctx.db, "bq-test-bq-search-shape@contract-test.local").await;
}

// ===========================================================================
// 6. Info endpoint response shape
// ===========================================================================

#[tokio::test]
async fn bq_info_returns_table_data() {
    let ctx = setup_auth_context("bq-info").await;
    if ctx.is_none() {
        eprintln!("SKIP: bq_info_returns_table_data — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let (ds_id, _ds_slug) = create_bq_test_datasource(&ctx, "bq-info-test").await;

    // Insert a test table
    insert_bq_test_tables(
        &ctx.db,
        &ctx.workspace_id,
        &ds_id,
        &[(
            "my-project",
            "analytics",
            "events",
            json!({
                "columns": [
                    {"name": "event_id", "type": "STRING", "description": "Unique event ID"},
                    {"name": "event_type", "type": "STRING", "description": "Type of event"},
                    {"name": "timestamp", "type": "TIMESTAMP", "description": "Event time"}
                ],
                "table_description": "Analytics events table",
                "table_type": "TABLE",
                "row_count": 1500000
            }),
        )],
    )
    .await;

    // POST /info
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/bigquery/info",
        &ctx.access_token,
    )
    .body(
        json!({
            "table_id": "my-project.analytics.events"
        })
        .to_string(),
    )
    .send()
    .await
    .expect("info request should succeed");

    assert_eq!(resp.status(), 200, "info should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    assert_eq!(body["status"], "success");
    assert!(body.get("metadata").is_some(), "missing 'metadata'");

    let metadata = &body["metadata"];
    assert_eq!(
        metadata["table_id"], "my-project.analytics.events",
        "table_id should match"
    );
    assert_eq!(
        metadata["project_id"], "my-project",
        "project_id should match"
    );
    assert_eq!(
        metadata["dataset_id"], "analytics",
        "dataset_id should match"
    );
    assert_eq!(
        metadata["table_name"], "events",
        "table_name should match"
    );
    assert_eq!(
        metadata["description"], "Analytics events table",
        "description should match"
    );

    let columns = metadata["columns"].as_array().unwrap();
    assert_eq!(columns.len(), 3, "should have 3 columns");

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "bq-info-test").await;
    cleanup_test_user(&ctx.db, "bq-test-bq-info@contract-test.local").await;
}

// ===========================================================================
// 7. Info endpoint — table not found
// ===========================================================================

#[tokio::test]
async fn bq_info_not_found_returns_error() {
    let ctx = setup_auth_context("bq-info-404").await;
    if ctx.is_none() {
        eprintln!("SKIP: bq_info_not_found_returns_error — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/bigquery/info",
        &ctx.access_token,
    )
    .body(
        json!({
            "table_id": "nonexistent-project.dataset.table"
        })
        .to_string(),
    )
    .send()
    .await
    .expect("info request should succeed");

    assert_eq!(resp.status(), 200, "info should return 200 even for not found");

    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(body["status"], "error");
    assert!(body.get("error").is_some(), "missing 'error'");
    assert_eq!(body["error_type"], "not_found");

    // Clean up
    cleanup_test_user(&ctx.db, "bq-test-bq-info-404@contract-test.local").await;
}

// ===========================================================================
// 8. Catalog status response shape
// ===========================================================================

#[tokio::test]
async fn bq_catalog_status_returns_correct_shape() {
    let ctx = setup_auth_context("bq-status").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: bq_catalog_status_returns_correct_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (_ds_id, _ds_slug) = create_bq_test_datasource(&ctx, "bq-status-test").await;

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/bigquery/catalog/status",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("status request should succeed");

    assert_eq!(resp.status(), 200, "catalog status should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify all required fields
    assert!(
        body.get("indexed_projects").is_some(),
        "missing 'indexed_projects'"
    );
    assert!(
        body["indexed_projects"].is_array(),
        "'indexed_projects' should be an array"
    );
    assert!(
        body.get("current_status").is_some(),
        "missing 'current_status'"
    );
    assert_eq!(
        body["current_status"], "idle",
        "current_status should be 'idle'"
    );

    // Should have the 2 catalog projects from the datasource config
    let indexed_projects = body["indexed_projects"].as_array().unwrap();
    assert_eq!(
        indexed_projects.len(),
        2,
        "should have 2 indexed projects from config"
    );

    // Verify project shape
    let first = &indexed_projects[0];
    assert!(
        first.get("project_id").is_some(),
        "project missing 'project_id'"
    );
    assert!(
        first.get("dataset_count").is_some(),
        "project missing 'dataset_count'"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "bq-status-test").await;
    cleanup_test_user(&ctx.db, "bq-test-bq-status@contract-test.local").await;
}

// ===========================================================================
// 9. Catalog settings (deprecated) returns 400
// ===========================================================================

#[tokio::test]
async fn bq_catalog_settings_returns_400() {
    let ctx = setup_auth_context("bq-settings").await;
    if ctx.is_none() {
        eprintln!("SKIP: bq_catalog_settings_returns_400 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/bigquery/catalog/settings",
        &ctx.access_token,
    )
    .body(json!({"include_public_datasets": true}).to_string())
    .send()
    .await
    .expect("settings request should succeed");

    assert_eq!(
        resp.status(),
        400,
        "catalog settings (deprecated) should return 400"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "bq-test-bq-settings@contract-test.local").await;
}

// ===========================================================================
// 10. Catalog datasets and tables response shape
// ===========================================================================

#[tokio::test]
async fn bq_catalog_datasets_and_tables_response_shape() {
    let ctx = setup_auth_context("bq-ds-tabs").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: bq_catalog_datasets_and_tables_response_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (ds_id, _ds_slug) = create_bq_test_datasource(&ctx, "bq-ds-tabs-test").await;

    insert_bq_test_tables(
        &ctx.db,
        &ctx.workspace_id,
        &ds_id,
        &[
            (
                "my-project",
                "sales",
                "orders",
                json!({
                    "columns": [{"name": "id", "type": "INT64"}],
                    "description": "Orders",
                    "row_count": 100
                }),
            ),
            (
                "my-project",
                "sales",
                "products",
                json!({
                    "columns": [
                        {"name": "id", "type": "INT64"},
                        {"name": "name", "type": "STRING"}
                    ],
                    "description": "Products",
                    "row_count": 50
                }),
            ),
        ],
    )
    .await;

    // GET /catalog/{project}/datasets
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/bigquery/catalog/my-project/datasets",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("datasets request should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "success");

    let datasets = body["datasets"].as_array().unwrap();
    let sales_ds = datasets
        .iter()
        .find(|d| d["dataset_id"] == "sales")
        .expect("should find 'sales' dataset");
    assert_eq!(sales_ds["table_count"], 2, "sales should have 2 tables");

    // GET /catalog/{project}/{dataset}/tables
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/bigquery/catalog/my-project/sales/tables",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("tables request should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "success");

    let tables = body["tables"].as_array().unwrap();
    assert_eq!(tables.len(), 2, "should have 2 tables");

    // Verify table shape
    let first_table = &tables[0];
    assert!(first_table.get("table_id").is_some());
    assert!(first_table.get("full_table_id").is_some());
    assert!(first_table.get("description").is_some());
    assert!(first_table.get("column_count").is_some());
    assert!(first_table.get("updated_at").is_some());

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "bq-ds-tabs-test").await;
    cleanup_test_user(&ctx.db, "bq-test-bq-ds-tabs@contract-test.local").await;
}

// ===========================================================================
// 11. Catalog columns response shape
// ===========================================================================

#[tokio::test]
async fn bq_catalog_columns_returns_correct_shape() {
    let ctx = setup_auth_context("bq-cols").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: bq_catalog_columns_returns_correct_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (ds_id, _ds_slug) = create_bq_test_datasource(&ctx, "bq-cols-test").await;

    insert_bq_test_tables(
        &ctx.db,
        &ctx.workspace_id,
        &ds_id,
        &[(
            "my-project",
            "analytics",
            "events",
            json!({
                "columns": [
                    {"name": "event_id", "type": "STRING", "mode": "REQUIRED", "description": "Unique ID"},
                    {"name": "event_type", "type": "STRING", "mode": "NULLABLE", "description": "Type"},
                    {"name": "created_at", "type": "TIMESTAMP", "mode": "REQUIRED", "description": "When"}
                ]
            }),
        )],
    )
    .await;

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/bigquery/catalog/my-project/analytics/events/columns",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("columns request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "success");

    let columns = body["columns"].as_array().unwrap();
    assert_eq!(columns.len(), 3, "should have 3 columns");

    // Verify column shape
    let col = &columns[0];
    assert!(col.get("name").is_some(), "column missing 'name'");
    assert!(col.get("type").is_some(), "column missing 'type'");
    assert!(col.get("mode").is_some(), "column missing 'mode'");
    assert!(
        col.get("description").is_some(),
        "column missing 'description'"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "bq-cols-test").await;
    cleanup_test_user(&ctx.db, "bq-test-bq-cols@contract-test.local").await;
}

// ===========================================================================
// 12. Catalog refresh requires BigQuery datasource
// ===========================================================================

#[tokio::test]
async fn bq_catalog_refresh_returns_result() {
    let ctx = setup_auth_context("bq-refresh").await;
    if ctx.is_none() {
        eprintln!("SKIP: bq_catalog_refresh_returns_result — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let (_ds_id, _ds_slug) = create_bq_test_datasource(&ctx, "bq-refresh-test").await;

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/bigquery/catalog/refresh",
        &ctx.access_token,
    )
    .body(json!({"force": false}).to_string())
    .send()
    .await
    .expect("refresh request should succeed");

    // Catalog refresh is now implemented — delegates to the shared execute_catalog_refresh.
    assert_eq!(resp.status(), 200, "refresh should return 200");

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(body.get("status").is_some(), "response should have 'status'");
    assert!(body.get("message").is_some(), "response should have 'message'");

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "bq-refresh-test").await;
    cleanup_test_user(&ctx.db, "bq-test-bq-refresh@contract-test.local").await;
}

