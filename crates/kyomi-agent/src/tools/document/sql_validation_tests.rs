// SPDX-License-Identifier: AGPL-3.0-or-later

use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use kyomi_core::connect_protocol::{ConnectOp, ConnectResponse, ConnectResponseBody, DryRunParams};
use kyomi_datasource_server::{ConnectRegistry, DryRunResult};
use kyomi_datasource_server::connect::registry::CommandPayload;

use crate::test_support::{build_ctx, loaded_embedding, seed_user_and_workspace, test_pool};
use crate::tools::{AgentTool, ToolContext};
use crate::tools::dashboard::{CreateDashboardTool, ModifyDashboardTool};
use crate::tools::knowledge::{EditDocumentTool, WriteDocumentTool};
use super::DocType;

pub(crate) fn chartml(sql: &str) -> String {
    format!("```chartml\ntype: source\nversion: 1\nname: sales_data\ndatasource: sales\nquery: {sql}\n```")
}

/// Exercise the production Connect provider and registry against a real
/// SQLite EXPLAIN. The command receiver plays only the transport role;
/// validity and error detail come from the database, never canned results.
pub(crate) async fn attach_sql_datasource(ctx: &mut ToolContext) -> (
    Arc<AtomicUsize>, tokio::task::JoinHandle<()>, u64,
) {
    let kyomi_core::DbPool::Sqlite(pool) = &ctx.db else { unreachable!("SQLite fixture") };
    sqlx::query("INSERT INTO datasource_configs \
        (id, workspace_id, name, datasource_type, connection_config, active, slug, connection_type) \
        VALUES ('ds-sql', 'ws-1', 'Sales', 'postgres', '{}', 1, 'sales', 'connect')")
        .execute(pool).await.expect("seed datasource");
    let registry = ConnectRegistry::new_local();
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<CommandPayload>(16);
    let connection_id = registry.register("ds-sql", sender).await;
    let dry_runs = Arc::new(AtomicUsize::new(0));
    let observed = dry_runs.clone();
    let sql_pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
        .connect("sqlite::memory:").await.expect("datasource database");
    let handler = tokio::spawn(async move {
        while let Some((request, response)) = receiver.recv().await {
            assert_eq!(request.op, ConnectOp::DryRun);
            let params: DryRunParams = serde_json::from_value(request.params.expect("params"))
                .expect("dry-run params");
            observed.fetch_add(1, Ordering::SeqCst);
            let result = match sqlx::query(&format!("EXPLAIN {}", params.sql)).fetch_all(&sql_pool).await {
                Ok(_) => DryRunResult::success("SQL passed EXPLAIN"),
                Err(error) => DryRunResult::failure(error.to_string(), None, None),
            };
            response.send(ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Result {
                    result: serde_json::to_value(result).expect("serialize result"),
                },
            }).expect("deliver result");
        }
    });
    ctx.connect_registry = Some(registry);
    (dry_runs, handler, connection_id)
}

async fn ctx() -> ToolContext {
    let db = test_pool().await;
    seed_user_and_workspace(&db).await;
    let mut ctx = build_ctx(db);
    ctx.embedding = loaded_embedding();
    ctx
}

pub(crate) async fn finish(ctx: &ToolContext, handler: tokio::task::JoinHandle<()>, connection_id: u64) {
    ctx.connect_registry.as_ref().expect("registry").unregister("ds-sql", connection_id).await;
    handler.await.expect("datasource handler");
}

fn retry(result: &str) -> serde_json::Value {
    let value: serde_json::Value = serde_json::from_str(result).expect("JSON");
    assert_eq!(value["success"], false, "{result}");
    assert!(value["error"].as_str().expect("error").starts_with("Document contains invalid SQL: "), "{result}");
    assert!(value["validation_errors"][0].as_str().expect("SQL error").contains("syntax error"), "{result}");
    value
}

async fn document(ctx: &ToolContext, id: &str) -> kyomi_core::models::Dashboard {
    kyomi_auth::dashboard_service::get_dashboard(&ctx.db, id, "ws-1", "user-a")
        .await.expect("lookup").expect("document")
}

#[tokio::test]
async fn invalid_sql_create_returns_same_retry_for_both_tool_families() {
    let mut ctx = ctx().await;
    let (calls, handler, id) = attach_sql_datasource(&mut ctx).await;
    let content = chartml("SELECT FROM");
    let knowledge = WriteDocumentTool.execute(serde_json::json!({
        "path": "Rejected knowledge", "content": content,
    }), &ctx).await.expect("correctable failure");
    let dashboard = CreateDashboardTool.execute(serde_json::json!({
        "title": "Rejected dashboard", "content": content, "verified_no_duplicates": true,
    }), &ctx).await.expect("correctable failure");
    assert_eq!(retry(&knowledge), retry(&dashboard));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let kyomi_core::DbPool::Sqlite(pool) = &ctx.db else { unreachable!() };
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dashboards")
        .fetch_one(pool).await.expect("count documents");
    let chunks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_chunks")
        .fetch_one(pool).await.expect("count chunks");
    assert_eq!((rows, chunks), (0, 0), "invalid SQL must not persist any document or chunks");
    finish(&ctx, handler, id).await;
}

#[tokio::test]
async fn prose_creates_and_updates_perform_zero_dry_runs() {
    let mut ctx = ctx().await;
    let (calls, handler, id) = attach_sql_datasource(&mut ctx).await;
    // SQL-looking prose still has no ChartML fence.
    for content in ["Notes: SELECT FROM sales", "Revised notes: SELECT FROM sales"] {
        let result = WriteDocumentTool.execute(serde_json::json!({
            "path": "Plain notes", "content": content,
        }), &ctx).await.expect("prose write");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("JSON");
        assert_eq!(parsed["success"], true, "{result}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "plain prose must issue zero dry-runs");
    finish(&ctx, handler, id).await;
}

#[tokio::test]
async fn invalid_sql_full_updates_are_rejected_for_both_doc_types() {
    let mut ctx = ctx().await;
    let (calls, handler, connection_id) = attach_sql_datasource(&mut ctx).await;
    for doc_type in [DocType::Knowledge, DocType::Dashboard] {
        let title = format!("Full update {doc_type:?}");
        let id = kyomi_auth::dashboard_service::create_dashboard(
            &ctx.db, "user-a", "ws-1", &title, "Original notes", doc_type, None,
        ).await.expect("seed");
        let result = WriteDocumentTool.execute(serde_json::json!({
            "path": title, "content": chartml("SELECT FROM"),
        }), &ctx).await.expect("retry result");
        retry(&result);
        assert_eq!(document(&ctx, &id).await.content, "Original notes");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    finish(&ctx, handler, connection_id).await;
}

#[tokio::test]
async fn targeted_edit_validates_full_result_for_both_doc_types() {
    let mut ctx = ctx().await;
    let (calls, handler, connection_id) = attach_sql_datasource(&mut ctx).await;
    for doc_type in [DocType::Knowledge, DocType::Dashboard] {
        let title = format!("Targeted {doc_type:?}");
        let content = chartml("SELECT 1");
        let id = kyomi_auth::dashboard_service::create_dashboard(
            &ctx.db, "user-a", "ws-1", &title, &content, DocType::Knowledge, None,
        ).await.expect("seed");
        // Model a stored document without re-running the service write gate.
        kyomi_core::db_execute!(&ctx.db, "UPDATE dashboards SET doc_type = $1 WHERE dashboard_id = $2", doc_type.as_str(), &id).expect("seed document type");
        let result = EditDocumentTool.execute(serde_json::json!({
            "path": title, "old_text": "SELECT 1", "new_text": "SELECT FROM",
        }), &ctx).await.expect("retry result");
        retry(&result);
        assert_eq!(document(&ctx, &id).await.content, content);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2, "fragments without fences must still validate the full result");
    finish(&ctx, handler, connection_id).await;
}

#[tokio::test]
async fn dashboard_updates_and_title_only_writes_share_sql_retry() {
    let mut ctx = ctx().await;
    let (calls, handler, connection_id) = attach_sql_datasource(&mut ctx).await;
    let content = chartml("SELECT FROM");
    let id = kyomi_auth::dashboard_service::create_dashboard(
        &ctx.db, "user-a", "ws-1", "Existing", &content, DocType::Knowledge, None,
    ).await.expect("seed");
    // Legacy stored dashboard with invalid SQL: service writes now reject it.
    kyomi_core::db_execute!(&ctx.db, "UPDATE dashboards SET doc_type = $1 WHERE dashboard_id = $2", DocType::Dashboard.as_str(), &id).expect("seed legacy dashboard");
    for args in [
        serde_json::json!({"dashboard_id": id, "content": content}),
        serde_json::json!({"dashboard_id": id, "title": "Renamed"}),
    ] {
        retry(&ModifyDashboardTool.execute(args, &ctx).await.expect("retry result"));
        assert_eq!(document(&ctx, &id).await.title, "Existing");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    finish(&ctx, handler, connection_id).await;
}

#[tokio::test]
async fn valid_chartml_can_be_created_and_updated() {
    let mut ctx = ctx().await;
    let (calls, handler, connection_id) = attach_sql_datasource(&mut ctx).await;
    for content in [chartml("SELECT 1"), chartml("SELECT 2")] {
        let result = WriteDocumentTool.execute(serde_json::json!({
            "path": "Valid chart", "content": content,
        }), &ctx).await.expect("valid write");
        let value: serde_json::Value = serde_json::from_str(&result).expect("JSON");
        assert_eq!(value["success"], true, "{result}");
        assert_eq!(document(&ctx, value["id"].as_str().expect("id")).await.content, content);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2, "validate each complete content write once");
    finish(&ctx, handler, connection_id).await;
}
