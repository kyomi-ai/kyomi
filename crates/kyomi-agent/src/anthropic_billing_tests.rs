// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-974: an Anthropic response's token counts are priced, written to
//! `api_usage_log` by the production INSERT, and counted by the credit/cap
//! logic (`BillingService`), against the real migrated schema.

use std::collections::HashMap;

use async_trait::async_trait;
use kyomi_auth::billing_service::BillingService;
use kyomi_core::DbPool;
use serde_json::json;

use crate::anthropic::AnthropicClient;
use crate::provider::{LLMProvider, RequestOptions};
use crate::types::{LLMResponse, Message, Tool};
use crate::usage_ledger::{BillingContext, MeteredClient, AI_COST_MULTIPLIER};

/// The real Anthropic response parser and price table behind the provider trait,
/// fed a canned API body: everything but the HTTP hop is production code.
struct CannedAnthropic {
    model: &'static str,
    body: serde_json::Value,
}

#[async_trait]
impl LLMProvider for CannedAnthropic {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[Tool],
        _options: &RequestOptions,
        _user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        AnthropicClient::parse_response(self.model, &self.body, Some("req_test".into()))
    }

    fn model(&self) -> &str {
        self.model
    }
}

fn metered(model: &'static str, body: serde_json::Value) -> MeteredClient {
    MeteredClient::new(Box::new(CannedAnthropic { model, body }))
}

async fn call(client: &MeteredClient) {
    client
        .complete(&[Message::user("hi")], &[], &RequestOptions::new(64), &HashMap::new())
        .await
        .unwrap();
}

/// Bill what `client` recorded, exactly as the production call sites do.
async fn bill(
    client: &MeteredClient,
    db: &DbPool,
    workspace_id: &str,
    component: &str,
    session_id: Option<&str>,
) {
    client
        .ledger()
        .bill(&BillingContext {
            db,
            user_id: "u1",
            workspace_id,
            session_id,
            component,
            provider: "anthropic",
            model: client.model(),
            is_byok: false,
            durable: None,
        })
        .await
        .unwrap();
}

/// One metered call billed as `component`.
async fn log_run(db: &DbPool, workspace_id: &str, model: &'static str, body: serde_json::Value) {
    let client = metered(model, body);
    call(&client).await;
    bill(&client, db, workspace_id, "chat_agent", Some("session-1")).await;
}

fn response(output_tokens: u64) -> serde_json::Value {
    json!({
        "content": [{"type": "text", "text": "ok"}],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 1000, "output_tokens": output_tokens,
            "cache_creation_input_tokens": 2000, "cache_read_input_tokens": 50000,
            "cache_creation": {"ephemeral_5m_input_tokens": 2000, "ephemeral_1h_input_tokens": 0}
        }
    })
}

#[tokio::test]
async fn anthropic_usage_is_billed_against_credits_and_trips_the_cap() {
    let db = kyomi_test_db::test_pool().await;
    let DbPool::Sqlite(sq) = &db else { panic!("sqlite fixture") };
    kyomi_test_db::seed_user(sq, "u1", "u1@example.com").await;
    kyomi_test_db::seed_workspace(sq, "w1", "u1").await;
    let billing = BillingService::new();

    // 1000*$2 + 100*$10 + 2000*$2.5 + 50000*$0.1 = 13,000 / 1e6 = $0.013 provider cost.
    log_run(&db, "w1", "claude-sonnet-5-5", response(100)).await;
    let after = billing.calculate_credits_info(&db, "w1").await.unwrap();
    let expected = 0.013 * AI_COST_MULTIPLIER;
    assert!((after.used_usd - expected).abs() < 1e-9, "used {} != {expected}", after.used_usd);
    assert_eq!(after.total_calls, 1);
    assert!(!after.exhausted);
    assert!(after.limit_usd > 0.0, "fixture workspace needs a credit budget");

    // A call costing more than the whole budget must block further usage.
    let out = (after.limit_usd * 2.0 / AI_COST_MULTIPLIER / 10.0 * 1_000_000.0) as u64;
    log_run(&db, "w1", "claude-sonnet-5-5", response(out)).await;
    let allowed = billing.check_ai_usage_allowed(&db, "w1").await.unwrap();
    assert!(allowed.blocked && !allowed.allowed, "cap must count Anthropic cost");

    // Cache totals and the per-call breakdown landed in the row.
    let (w, r, meta): (i32, i32, String) = sqlx::query_as(
        "SELECT cache_creation_input_tokens, cache_read_input_tokens, extra_metadata \
         FROM api_usage_log ORDER BY id LIMIT 1",
    )
    .fetch_one(sq)
    .await
    .unwrap();
    assert_eq!((w, r), (2000, 50000));
    let meta: serde_json::Value = serde_json::from_str(&meta).unwrap();
    assert_eq!(meta["calls"][0]["cache_read_input_tokens"], 50000);
    assert_eq!(meta["fresh_input_tokens"], 1000);
}

/// Acceptance: every LLM call type (chat run, title, dashboard summary, collection
/// tagging) writes exactly one usage row with a non-zero priced cost, through the
/// same metered path, and the credit/cap logic counts them all.
#[tokio::test]
async fn every_call_type_writes_exactly_one_metered_row_counted_by_credits() {
    let db = kyomi_test_db::test_pool().await;
    let DbPool::Sqlite(sq) = &db else { panic!("sqlite fixture") };
    kyomi_test_db::seed_user(sq, "u1", "u1@example.com").await;
    kyomi_test_db::seed_workspace(sq, "w1", "u1").await;

    // One client for the summary and the tagging pass, as `generate_dashboard_summary_inner`
    // uses it: each pass bills only the calls made since the previous bill.
    let summary_client = metered("claude-haiku-5-5", response(100));
    let chat = metered("claude-sonnet-5-5", response(100));
    let title = metered("claude-haiku-5-5", response(100));

    call(&chat).await;
    bill(&chat, &db, "w1", "custom_agent", Some("session-1")).await;
    call(&title).await;
    bill(&title, &db, "w1", "title", Some("session-1")).await;
    call(&summary_client).await;
    bill(&summary_client, &db, "w1", "dashboard_summary", None).await;
    call(&summary_client).await;
    bill(&summary_client, &db, "w1", "collection_tag", None).await;
    // Nothing new was called: billing again writes no row.
    bill(&summary_client, &db, "w1", "collection_tag", None).await;

    let rows: Vec<(String, f64, Option<String>)> = sqlx::query_as(
        "SELECT component, cost_estimate, session_id FROM api_usage_log ORDER BY id",
    )
    .fetch_all(sq)
    .await
    .unwrap();
    let components: Vec<_> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(components, ["custom_agent", "title", "dashboard_summary", "collection_tag"], "exactly one row each");
    assert!(rows.iter().all(|r| r.1 > 0.0), "every row is priced: {rows:?}");
    assert_eq!(rows[2].2, None, "a dashboard summary belongs to no chat session");

    // Sonnet 5.5: 1000*$2 + 100*$10 + 2000*$2.5 + 50000*$0.1 = $0.013; Haiku 5.5 (prompt 53,000 <= 100K):
    // 1000*$0.10 + 100*$0.50 + 2000*$0.125 + 50000*$0.01 = $0.0009.
    let expected = (0.013 + 3.0 * 0.0009) * AI_COST_MULTIPLIER;
    let credits = BillingService::new().calculate_credits_info(&db, "w1").await.unwrap();
    assert!((credits.used_usd - expected).abs() < 1e-9, "credits used {} != {expected}", credits.used_usd);
    assert_eq!(credits.total_calls, 4);
}

// -- Real call sites against a mock Anthropic API --------------------------------

/// An HTTP server answering every request with `body` as an Anthropic Messages API
/// response (so the real client, parser, price table, metering and billing run);
/// returns its base URL.
async fn mock_anthropic(body: serde_json::Value) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let payload = body.to_string();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let payload = payload.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Read the headers, then the declared body, so the client is not cut off mid-write.
                loop {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .to_ascii_lowercase()
                            .split("content-length:")
                            .nth(1)
                            .and_then(|r| r.split("\r\n").next())
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nrequest-id: req_mock\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

fn haiku_text_response(text: &str) -> serde_json::Value {
    json!({
        "id": "msg_mock",
        "content": [{"type": "text", "text": text}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1000, "output_tokens": 20}
    })
}

/// A workspace/user/session fixture and an app config pointing the real provider
/// factory at `base_url` (Haiku 5.5).
async fn fixture(base_url: String) -> (DbPool, kyomi_core::Config) {
    let db = kyomi_test_db::test_pool().await;
    let DbPool::Sqlite(sq) = &db else { panic!("sqlite fixture") };
    kyomi_test_db::seed_user(sq, "u1", "u1@example.com").await;
    kyomi_test_db::seed_workspace(sq, "w1", "u1").await;
    kyomi_core::db_execute!(&db, "INSERT INTO chat_sessions (session_id,user_id,workspace_id) VALUES ('s1','u1','w1')").unwrap();
    let mut app_config = kyomi_core::Config::test_config();
    app_config.llm_provider = Some("anthropic".into());
    app_config.llm_api_key = Some("test-key".into());
    app_config.llm_base_url = Some(base_url);
    app_config.llm_model = Some("claude-haiku-5-5".into());
    (db, app_config)
}

async fn usage_rows(db: &DbPool) -> Vec<(String, String, Option<String>, i32, f64)> {
    let DbPool::Sqlite(sq) = db else { panic!("sqlite fixture") };
    sqlx::query_as("SELECT component, model, session_id, input_tokens, cost_estimate FROM api_usage_log ORDER BY id")
        .fetch_all(sq)
        .await
        .unwrap()
}

/// The real `generate_title_inner`, against a mock Anthropic API, writes exactly
/// one priced `title` row that credits/caps count.
#[tokio::test]
async fn title_generation_writes_one_metered_usage_row() {
    let (db, app_config) = fixture(mock_anthropic(haiku_text_response("Revenue by Region")).await).await;
    let manager = kyomi_auth::websocket::WebSocketManager::new(None, db.clone());

    crate::execution::generate_title_inner(&db, &manager, "s1", "u1", "w1", "revenue by region?", &app_config)
        .await
        .unwrap();

    let rows = usage_rows(&db).await;
    assert_eq!(rows.len(), 1, "exactly one usage row: {rows:?}");
    let (component, model, session, input, cost) = &rows[0];
    assert_eq!((component.as_str(), model.as_str()), ("title", "claude-haiku-5-5"));
    assert_eq!(session.as_deref(), Some("s1"));
    assert_eq!(*input, 1000);
    // Haiku 5.5: 1000*$0.10 + 20*$0.50 = $0.00011, x1.1 markup.
    assert!((cost - 0.00011 * 1.1).abs() < 1e-9, "priced and marked up: {cost}");
}

/// The real `generate_dashboard_summary_inner`: the summary call and the collection
/// tagging call on the same client each write exactly one priced row (`dashboard_summary`,
/// `collection_tag`, no chat session), and the credit logic counts both.
#[tokio::test]
async fn dashboard_summary_and_tagging_each_write_one_metered_usage_row() {
    // The mock answers `[]` to both calls: a valid (empty) tag list and a non-empty summary.
    let (db, app_config) = fixture(mock_anthropic(haiku_text_response("[]")).await).await;
    kyomi_core::db_execute!(&db, "INSERT INTO dashboards (dashboard_id,user_id,workspace_id,title,content,doc_type) VALUES ('d1','u1','w1','Revenue','Monthly revenue by region.','knowledge')").unwrap();
    kyomi_core::db_execute!(&db, "INSERT INTO collections (id,workspace_id,name,description,created_by) VALUES ('c1','w1','Finance','Finance dashboards','u1')").unwrap();
    let params = crate::execution::DashboardSummaryParams {
        validation_context: crate::tools::QueryContext {
            db: db.clone(),
            user_id: "u1".into(),
            workspace_id: "w1".into(),
            encryption_key: std::sync::Arc::new([0u8; 32]),
            config: std::sync::Arc::new(app_config.clone()),
            connect_registry: None,
        },
        db: db.clone(),
        ws_manager: kyomi_auth::websocket::WebSocketManager::new(None, db.clone()),
        dashboard_id: "d1".into(),
        user_id: "u1".into(),
        workspace_id: "w1".into(),
        title: "Revenue".into(),
        content: "Monthly revenue by region.".into(),
        app_config: std::sync::Arc::new(app_config),
        doc_type: "knowledge".into(),
        copilot_receipt_id: None,
    };

    crate::execution::generate_dashboard_summary_inner(params).await.unwrap();

    let rows = usage_rows(&db).await;
    let components: Vec<_> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(components, ["dashboard_summary", "collection_tag"], "one row per call: {rows:?}");
    assert!(rows.iter().all(|r| r.2.is_none() && r.4 > 0.0), "no chat session, priced: {rows:?}");
    let credits = BillingService::new().calculate_credits_info(&db, "w1").await.unwrap();
    assert!((credits.used_usd - 2.0 * 0.00011 * 1.1).abs() < 1e-9, "credits count both: {}", credits.used_usd);
}

/// A durable run dropped mid-flight (DB-heartbeat cancel, lease loss) discards the
/// future holding the client and its ledger. The calls it already paid for must still
/// be billed: this drops a real running task after a paid call and waits for the row.
#[tokio::test]
async fn a_run_dropped_mid_flight_still_bills_its_paid_calls() {
    let db = kyomi_test_db::test_pool().await;
    let DbPool::Sqlite(sq) = &db else { panic!("sqlite fixture") };
    kyomi_test_db::seed_user(sq, "u1", "u1@example.com").await;
    kyomi_test_db::seed_workspace(sq, "w1", "u1").await;
    let client = metered("claude-sonnet-5-5", response(100));
    let billing = crate::usage_ledger::OwnedBilling {
        db: db.clone(),
        encryption_key: [0u8; 32],
        user_id: "u1".into(),
        workspace_id: "w1".into(),
        session_id: None,
        component: "custom_agent".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-5-5".into(),
        is_byok: false,
        durable: None,
    };
    let ledger = client.ledger();
    let run = tokio::spawn(async move {
        let _bill_if_dropped = crate::usage_ledger::BillOnDrop::new(client.ledger(), billing);
        call(&client).await; // a paid model call, then the run blocks (e.g. in a tool call)...
        std::future::pending::<()>().await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while ledger.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the run makes its paid call");

    run.abort(); // ...and is dropped without ever reaching its normal bill
    assert!(run.await.unwrap_err().is_cancelled());

    let mut rows = Vec::new();
    for _ in 0..200 {
        rows = usage_rows(&db).await;
        if !rows.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(rows.len(), 1, "the dropped run's paid call is billed exactly once: {rows:?}");
    assert!((rows[0].4 - 0.013 * AI_COST_MULTIPLIER).abs() < 1e-9, "priced: {}", rows[0].4);
    assert!(ledger.is_empty(), "nothing left to bill twice");
}

/// The non-durable usage insert runs against a real Postgres: `ApiUsageInsert::insert_sql(true)`
/// (the `$15::text::json` cast on the `json` column) through the real `UsageLedger::bill`,
/// then read back. Skipped without a reachable Postgres, except where CI requires it
/// (`KYOMI_REQUIRE_POSTGRES_TESTS=1`, the same switch kyomi-auth's Postgres tests use).
#[tokio::test]
async fn the_non_durable_usage_insert_round_trips_on_postgres() {
    let db = match kyomi_core::test_db::connect_test_pool().await {
        Ok(db) => db,
        Err(e) => {
            assert!(
                std::env::var("KYOMI_REQUIRE_POSTGRES_TESTS").as_deref() != Ok("1"),
                "KYOMI_REQUIRE_POSTGRES_TESTS=1 but Postgres is unreachable: {e}"
            );
            eprintln!("SKIP: no Postgres test server reachable: {e}");
            return;
        }
    };
    let DbPool::Postgres(pg) = &db else { panic!("connect_test_pool returns a Postgres pool") };
    let id = uuid::Uuid::new_v4().to_string();
    let (user, workspace) = (format!("usr-{id}"), format!("ws-{id}"));
    kyomi_core::db_execute!(&db, "INSERT INTO users(user_id,email,active) VALUES ($1,$2,true)", &user, format!("{user}@example.test")).unwrap();
    kyomi_core::db_execute!(&db, "INSERT INTO workspaces(workspace_id,name,owner_user_id) VALUES ($1,'test',$2)", &workspace, &user).unwrap();

    let client = metered("claude-sonnet-5-5", response(100));
    call(&client).await;
    client
        .ledger()
        .bill(&BillingContext {
            db: &db,
            user_id: &user,
            workspace_id: &workspace,
            session_id: None,
            component: "title",
            provider: "anthropic",
            model: "claude-sonnet-5-5",
            is_byok: false,
            durable: None,
        })
        .await
        .unwrap();

    let (component, input, write, read, cost, meta): (String, i32, i32, i32, f64, String) = sqlx::query_as(
        "SELECT component, input_tokens, cache_creation_input_tokens, cache_read_input_tokens, \
         cost_estimate, extra_metadata::text FROM api_usage_log WHERE workspace_id=$1",
    )
    .bind(&workspace)
    .fetch_one(pg)
    .await
    .expect("exactly one row, read back from Postgres");
    assert_eq!((component.as_str(), input, write, read), ("title", 53_000, 2_000, 50_000));
    assert!((cost - 0.013 * AI_COST_MULTIPLIER).abs() < 1e-9, "priced: {cost}");
    let meta: serde_json::Value = serde_json::from_str(&meta).unwrap();
    assert_eq!(meta["calls"][0]["cache_read_input_tokens"], 50_000, "the json column holds the breakdown");

    sqlx::query("DELETE FROM api_usage_log WHERE workspace_id=$1").bind(&workspace).execute(pg).await.unwrap();
    sqlx::query("DELETE FROM workspaces WHERE workspace_id=$1").bind(&workspace).execute(pg).await.unwrap();
    sqlx::query("DELETE FROM users WHERE user_id=$1").bind(&user).execute(pg).await.unwrap();
}
