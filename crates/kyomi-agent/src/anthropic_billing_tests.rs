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
    use crate::provider::LLMProvider;
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
