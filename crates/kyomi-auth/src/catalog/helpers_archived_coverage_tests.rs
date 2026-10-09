//! Archived container history is context, never proof of a live shortfall.
use super::tests::seed_container_scoped_fixture;
use super::*;

async fn fixture(
    suffix: &str,
    live_count: usize,
    archived_count: usize,
) -> (DbPool, String, String) {
    let db = DbPool::connect("sqlite::memory:").await.expect("sqlite");
    let DbPool::Sqlite(sq) = &db else {
        unreachable!("sqlite")
    };
    let rows: Vec<_> = (0..live_count + archived_count)
        .map(|i| {
            (
                "project".to_string(),
                format!("dataset_{i}"),
                "table".to_string(),
            )
        })
        .collect();
    let refs: Vec<_> = rows
        .iter()
        .map(|(p, d, t)| (p.as_str(), d.as_str(), t.as_str()))
        .collect();
    let (ws, ds) = seed_container_scoped_fixture(sq, suffix, &refs).await;
    for i in live_count..live_count + archived_count {
        sqlx::query("UPDATE datasource_table_cache SET is_archived = 1 WHERE datasource_config_id = ? AND dataset_id = ?")
            .bind(&ds).bind(format!("dataset_{i}"))
            .execute(sq).await.expect("archive history");
    }
    (db, ws, ds)
}

fn enumerated(count: usize) -> HashSet<ContainerKey> {
    (0..count)
        .map(|i| ("project".to_string(), format!("dataset_{i}")))
        .collect()
}

#[tokio::test]
async fn mostly_archived_history_persists_informational_refresh_warning() {
    let (db, ws, ds) = fixture("archivedsignal", 1, 8).await;
    let coverage = check_container_coverage(&db, &ws, &ds, &enumerated(1))
        .await
        .expect("coverage");
    assert!(
        !coverage.material,
        "archived history alone cannot prove refresh failure"
    );
    let warning = coverage
        .warning
        .as_ref()
        .expect("archived history must be visible");
    assert!(warning.contains("8 archived container(s)"), "{warning}");
    assert!(warning.contains("9 historical container(s)"), "{warning}");
    assert!(warning.contains("project.dataset_1"), "{warning}");
    assert!(warning.contains("(+3 more)"), "{warning}");
    let mut status = "idle";
    let mut reason = None;
    let mut errors = Vec::new();
    apply_container_coverage(coverage, &mut status, &mut reason, &mut errors);
    update_datasource_status(&db, &ws, &ds, status, None, reason.as_deref(), &errors)
        .await
        .expect("persist status");
    let DbPool::Sqlite(sq) = &db else {
        unreachable!("sqlite")
    };
    let (saved_status, progress): (String, String) = sqlx::query_as("SELECT catalog_refresh_status, catalog_refresh_progress FROM datasource_configs WHERE id = ?")
        .bind(&ds).fetch_one(sq).await.expect("read persisted refresh");
    let envelope: serde_json::Value = serde_json::from_str(&progress).expect("progress JSON");
    assert_eq!(saved_status, "idle");
    assert_eq!(envelope["error"], serde_json::Value::Null);
    assert_eq!(envelope["warnings"], serde_json::json!(errors));
    assert_eq!(errors.len(), 1);
}

#[tokio::test]
async fn healthy_single_container_has_no_history_warning() {
    let (db, ws, ds) = fixture("genuinesingle", 1, 0).await;
    let coverage = check_container_coverage(&db, &ws, &ds, &enumerated(1))
        .await
        .expect("coverage");
    assert!(coverage.warning.is_none());
    assert!(!coverage.material);
}

#[tokio::test]
async fn zero_live_with_archived_history_reports_no_shortfall() {
    let (db, ws, ds) = fixture("archivedonly", 0, 8).await;
    let coverage = check_container_coverage(&db, &ws, &ds, &HashSet::new())
        .await
        .expect("coverage");
    assert!(coverage.warning.is_none());
    assert!(!coverage.material);
}

#[tokio::test]
async fn reverified_archived_history_has_no_warning() {
    let (db, ws, ds) = fixture("reverifiedhistory", 1, 8).await;
    let coverage = check_container_coverage(&db, &ws, &ds, &enumerated(9))
        .await
        .expect("coverage");
    assert!(coverage.warning.is_none());
    assert!(!coverage.material);
}

#[tokio::test]
async fn minority_archived_history_has_no_warning() {
    let (db, ws, ds) = fixture("minorityhistory", 8, 1).await;
    let coverage = check_container_coverage(&db, &ws, &ds, &enumerated(8))
        .await
        .expect("coverage");
    assert!(coverage.warning.is_none());
    assert!(!coverage.material);
}

#[tokio::test]
async fn live_and_archived_tables_in_same_container_do_not_count_as_history() {
    let (db, ws, ds) = fixture("mixedhistory", 1, 0).await;
    let DbPool::Sqlite(sq) = &db else {
        unreachable!("sqlite")
    };
    sqlx::query("INSERT INTO datasource_table_cache (workspace_id, datasource_config_id, project_id, dataset_id, table_id, table_metadata, is_archived) VALUES (?, ?, 'project', 'dataset_0', 'old_table', '{}', 1)")
        .bind(&ws).bind(&ds).execute(sq).await.expect("archived table in live container");
    let coverage = check_container_coverage(&db, &ws, &ds, &enumerated(1))
        .await
        .expect("coverage");
    assert!(coverage.warning.is_none());
    assert!(!coverage.material);
}

#[tokio::test]
async fn archived_history_keeps_project_qualified_keys_and_datasource_scope() {
    let (db, ws, ds) = fixture("qualifiedhistory", 1, 0).await;
    let DbPool::Sqlite(sq) = &db else {
        unreachable!("sqlite")
    };
    sqlx::query("INSERT INTO datasource_table_cache (workspace_id, datasource_config_id, project_id, dataset_id, table_id, table_metadata, is_archived) VALUES (?, ?, 'other_project', 'dataset_0', 'table', '{}', 1)")
        .bind(&ws).bind(&ds).execute(sq).await.expect("same name in other project");
    let coverage = check_container_coverage(&db, &ws, &ds, &enumerated(1))
        .await
        .expect("coverage");
    assert_eq!(
        coverage.warning.as_deref(),
        Some(
            "Catalog refresh has 1 archived container(s) not re-verified this run out of 2 historical container(s), with 1 live container(s) remaining — archived history may reflect deletion or incomplete prior coverage: other_project.dataset_0"
        )
    );
    let (other_ws, other_ds) =
        seed_container_scoped_fixture(sq, "isolatedhistory", &[("project", "dataset_0", "table")])
            .await;
    let isolated = check_container_coverage(&db, &other_ws, &other_ds, &enumerated(1))
        .await
        .expect("isolated coverage");
    assert!(isolated.warning.is_none());
    let wrong_workspace = check_container_coverage(&db, &other_ws, &ds, &enumerated(1))
        .await
        .expect("workspace scoped coverage");
    assert!(wrong_workspace.warning.is_none());
    let wrong_datasource = check_container_coverage(&db, &ws, &other_ds, &enumerated(1))
        .await
        .expect("datasource scoped coverage");
    assert!(wrong_datasource.warning.is_none());
}

#[tokio::test]
async fn archived_context_preserves_material_live_shortfall() {
    let (db, ws, ds) = fixture("combinedhistory", 4, 8).await;
    let coverage = check_container_coverage(&db, &ws, &ds, &enumerated(1))
        .await
        .expect("coverage");
    assert!(coverage.material);
    let warning = coverage.warning.expect("both concerns visible");
    assert!(warning.contains("1 of 4 known container(s)"), "{warning}");
    assert!(warning.contains("8 archived container(s)"), "{warning}");
    assert!(warning.contains("12 historical container(s)"), "{warning}");
}
