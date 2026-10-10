//! Prove archived coverage context reaches the real refresh's persisted warnings.
use super::tests::{datasource_progress_envelope, datasource_status, seed_connect_fixture};
use super::*;

#[tokio::test]
async fn mostly_archived_history_is_visible_after_connect_refresh() {
    let db = DbPool::connect("sqlite::memory:").await.expect("sqlite");
    let DbPool::Sqlite(sq) = &db else {
        unreachable!("sqlite")
    };
    let ctx = seed_connect_fixture(sq, "archivedcontext").await;
    for i in 0..9 {
        sqlx::query("INSERT INTO datasource_table_cache (workspace_id, datasource_config_id, project_id, dataset_id, table_id, table_metadata, is_archived) VALUES (?, ?, '', ?, 'table', '{}', ?)")
            .bind(&ctx.workspace_id).bind(&ctx.datasource_config_id)
            .bind(format!("dataset_{i}")).bind(i != 0)
            .execute(sq).await.expect("seed historical containers");
    }
    let embedding = EmbeddingService::new().expect("load embedding model");
    // Successful enumeration with no current tables avoids a pgvector write
    // on SQLite and leaves the archive gate closed, preserving the fixture.
    let result = process_discovered_catalog(ProcessDiscoveredCatalogParams {
        db: &db,
        embedding: &embedding,
        ctx: &ctx,
        catalog_result: CatalogResult {
            containers: vec![kyomi_core::connect_protocol::CatalogContainer {
                name: "dataset_0".to_string(),
                tables: Vec::new(),
            }],
            errors: Vec::new(),
        },
        explicit_empty: false,
        start_time: Utc::now(),
    })
    .await;
    let errors = result
        .errors
        .expect("history warning reaches refresh result");
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0],
        "Catalog refresh has 8 archived container(s) not re-verified this run out of 9 historical container(s), with 1 live container(s) remaining — archived history may reflect deletion or incomplete prior coverage: dataset_1, dataset_2, dataset_3, dataset_4, dataset_5 (+3 more)"
    );
    let envelope = datasource_progress_envelope(sq, &ctx.datasource_config_id).await;
    assert_eq!(envelope["warnings"], serde_json::json!(errors));
    assert_eq!(envelope["error"], serde_json::Value::Null);
    assert_eq!(
        datasource_status(sq, &ctx.datasource_config_id).await,
        "idle"
    );
    assert_eq!(result.tables_archived, 0);
    let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM datasource_table_cache WHERE datasource_config_id = ? AND is_archived = 0")
        .bind(&ctx.datasource_config_id).fetch_one(sq).await.expect("live rows");
    assert_eq!(live, 1);
}
