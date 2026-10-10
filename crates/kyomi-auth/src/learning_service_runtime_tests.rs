//! Request-path runtime starvation regression (KYO-657).
use super::{SaveLearningParams, save_learning};
use crate::test_support::{seed_user, seed_workspace, sqlite_pool, test_pool};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn save_learning_does_not_block_the_runtime() {
    let db = test_pool().await;
    let sq = sqlite_pool(&db);
    seed_user(sq, "runtime-user", "runtime-user@test.local").await;
    seed_workspace(sq, "runtime-workspace", "runtime-user").await;
    // Model loading is startup work, outside the measured request window.
    let embed = kyomi_embed::EmbeddingService::new().expect("load real embedding model");
    // Exercise the model's full 512-token window, as a long learning insight can.
    let insight = "revenue ".repeat(512);
    let stop = Arc::new(AtomicBool::new(false));
    let worst_gap = Arc::new(AtomicU64::new(0));
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let heartbeat = tokio::spawn({
        let stop = stop.clone();
        let worst_gap = worst_gap.clone();
        async move {
            let mut last = Instant::now();
            ready_tx.send(()).expect("heartbeat ready receiver");
            while !stop.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let now = Instant::now();
                worst_gap.fetch_max(
                    now.duration_since(last).as_millis() as u64,
                    Ordering::Relaxed,
                );
                last = now;
            }
        }
    });
    ready_rx.await.expect("heartbeat started before request");
    // Spawn the real production request onto the sole worker shared with the heartbeat.
    let request = tokio::spawn(async move {
        let id = save_learning(SaveLearningParams {
            db: &db,
            embedding_svc: &embed,
            workspace_id: "runtime-workspace",
            user_id: "runtime-user",
            session_id: "runtime-session",
            insight: &insight,
            context: None,
            scope: "workspace",
            datasource_config_id: None,
            learning_type: "general",
            reference_queries: None,
            structured_metadata: None,
        })
        .await
        .expect("save_learning succeeds");
        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT embedding FROM agent_learnings WHERE learning_id = $1")
                .bind(id)
                .fetch_one(sqlite_pool(&db))
                .await
                .expect("read stored embedding");
        assert_eq!(stored.len(), kyomi_embed::EmbeddingService::DIMENSIONS * 4);
    });
    request.await.expect("request task must not panic");
    // Let an overdue tick record any request-induced stall before stopping it.
    tokio::time::sleep(Duration::from_millis(30)).await;
    stop.store(true, Ordering::Relaxed);
    heartbeat.await.expect("heartbeat task must not panic");
    let worst_gap_ms = worst_gap.load(Ordering::Relaxed);
    eprintln!("save_learning heartbeat worst gap: {worst_gap_ms}ms");
    assert!(
        worst_gap_ms < 500,
        "save_learning blocked the single-worker runtime: worst 10ms heartbeat gap {worst_gap_ms}ms (limit 500ms)"
    );
}
