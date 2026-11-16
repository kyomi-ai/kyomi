// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-974: an Anthropic response's token counts are priced, written to
//! `api_usage_log` by the production INSERT, and counted by the credit/cap
//! logic (`BillingService`), against the real migrated schema.

use kyomi_auth::billing_service::BillingService;
use kyomi_core::DbPool;
use serde_json::json;

use crate::anthropic::AnthropicClient;
use crate::usage_ledger::{UsageLedger, AI_COST_MULTIPLIER};
use kyomi_core::models::ApiUsageInsert;

/// Price `response` and write the row exactly as `execute_agent_chat` does:
/// the ledger builds it and `ApiUsageInsert` writes it.
async fn log_run(db: &DbPool, workspace_id: &str, model: &str, response: serde_json::Value) {
    let parsed = AnthropicClient::parse_response(model, &response, None).unwrap();
    let ledger = UsageLedger::default();
    ledger.record(model, &parsed);
    let u = ledger.billing_row(false, "anthropic", model, "chat_agent").unwrap();
    let DbPool::Sqlite(sq) = db else { panic!("sqlite fixture") };
    sqlx::query(&ApiUsageInsert::insert_sql(false))
        .bind("u1")
        .bind(workspace_id)
        .bind("session-1")
        .bind(chrono::Utc::now())
        .bind(&u.provider)
        .bind(&u.model)
        .bind(u.input_tokens)
        .bind(u.output_tokens)
        .bind(u.total_tokens)
        .bind(u.cost_estimate)
        .bind(&u.component)
        .bind(u.provider_cost_usd)
        .bind(u.cache_creation_input_tokens)
        .bind(u.cache_read_input_tokens)
        .bind(&u.extra_metadata)
        .bind(None::<&str>)
        .execute(sq)
        .await
        .unwrap();
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
