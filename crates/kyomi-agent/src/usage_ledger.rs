// SPDX-License-Identifier: AGPL-3.0-or-later

//! The single metering path for every LLM call (KYO-974).
//!
//! Every provider the server constructs is a [`MeteredClient`]: each response is
//! recorded into a [`UsageLedger`] at the moment the provider returns it, and
//! [`UsageLedger::bill`] writes the `api_usage_log` row (pricing, markup, BYOK
//! rule, cache totals, per-call breakdown, run-id idempotency) in one place.
//! Chat runs, watches, copilot, titles, dashboard summaries and collection
//! tagging all use it; there is no per-call-type billing logic.
//!
//! One row is written per `bill` call (a chat run, one title, one summary, one
//! tagging pass), summing the model calls recorded since the previous `bill`.

use std::sync::{Arc, Mutex};

use serde_json::json;

use async_trait::async_trait;
use kyomi_core::models::ApiUsageInsert;
use kyomi_core::DbPool;
use tracing::{error, warn};

use crate::provider::{LLMProvider, RequestOptions};
use crate::types::{AgentTokenUsage, LLMResponse, Message, ProviderCallDetail, Tool};

/// Shared handle: a [`MeteredClient`] records into it, [`UsageLedger::bill`] reads it.
#[derive(Clone, Default)]
pub struct UsageLedger(Arc<Mutex<Vec<CallRecord>>>);

#[derive(Debug, Clone)]
struct CallRecord {
    model: String,
    usage: AgentTokenUsage,
    /// Provider cost of this call in USD (`LLMResponse::cost`).
    cost: Option<f64>,
    detail: Option<ProviderCallDetail>,
}

/// Markup on provider cost to cover payment processing fees; Kyomi-billed rows only.
pub const AI_COST_MULTIPLIER: f64 = 1.1;

fn saturating_i32(n: u64) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Sums over every call in a run.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LedgerTotals {
    /// Uncached input tokens.
    pub fresh_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub reasoning_tokens: u64,
    /// Provider cost in USD, summed over calls that reported one.
    pub cost_usd: f64,
}

impl LedgerTotals {
    /// The `api_usage_log.input_tokens` value: the whole prompt, including
    /// cache writes and reads. `fresh_input_tokens` is in `extra_metadata`.
    pub fn prompt_tokens(&self) -> u64 {
        self.fresh_input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }
}

impl UsageLedger {
    pub fn record(&self, model: &str, response: &LLMResponse) {
        self.lock().push(CallRecord {
            model: model.to_string(),
            usage: response.usage.clone(),
            cost: response.cost,
            detail: response.call_detail.clone(),
        });
    }

    /// Whether no call is waiting to be billed.
    pub(crate) fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<CallRecord>> {
        // A poisoned ledger still holds valid records; keep reading it.
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn totals(&self) -> LedgerTotals {
        self.lock().iter().fold(LedgerTotals::default(), |t, c| LedgerTotals {
            fresh_input_tokens: t.fresh_input_tokens + u64::from(c.usage.input_tokens),
            output_tokens: t.output_tokens + u64::from(c.usage.output_tokens),
            cache_creation_input_tokens: t.cache_creation_input_tokens
                + u64::from(c.usage.cache_creation_input_tokens),
            cache_read_input_tokens: t.cache_read_input_tokens
                + u64::from(c.usage.cache_read_input_tokens),
            reasoning_tokens: t.reasoning_tokens + u64::from(c.usage.reasoning_tokens),
            cost_usd: t.cost_usd + c.cost.unwrap_or(0.0),
        })
    }

    /// The run's `api_usage_log` row, or `None` when no tokens were used. The
    /// ledger is the single source for tokens, cost and cache totals: it sees
    /// every provider call, including the max-iterations wrap-up.
    pub fn billing_row(
        &self,
        is_byok: bool,
        provider: &str,
        model: &str,
        component: &str,
    ) -> Option<ApiUsageInsert> {
        let t = self.totals();
        let (input, output) = (t.prompt_tokens(), t.output_tokens);
        if input == 0 && output == 0 {
            return None;
        }
        if !is_byok && self.lock().iter().any(|c| c.cost.is_none()) {
            // A static configuration fact: say it once per provider/model, not on every call.
            static SEEN: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
            let first = SEEN
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(format!("{provider}/{model}"));
            if first {
                warn!(
                    provider,
                    model,
                    "Kyomi-billed LLM calls report no cost: billed at $0 (only Anthropic, which is priced here, and OpenRouter report cost)"
                );
            }
        }
        // BYOK: the workspace pays the provider, so Kyomi credits are untouched
        // (`cost_estimate` 0) and the real cost is kept for observability.
        let (cost_estimate, provider_cost_usd) = if is_byok {
            (0.0, Some(t.cost_usd))
        } else {
            (t.cost_usd * AI_COST_MULTIPLIER, None)
        };
        Some(ApiUsageInsert {
            provider: provider.to_string(),
            model: model.to_string(),
            component: component.to_string(),
            request_id: None,
            input_tokens: saturating_i32(input),
            output_tokens: saturating_i32(output),
            total_tokens: saturating_i32(input + output),
            cost_estimate,
            provider_cost_usd,
            cache_creation_input_tokens: saturating_i32(t.cache_creation_input_tokens),
            cache_read_input_tokens: saturating_i32(t.cache_read_input_tokens),
            extra_metadata: self.extra_metadata().map(|m| m.to_string()),
        })
    }

    /// `api_usage_log.extra_metadata`, or `None` when no call was recorded.
    pub fn extra_metadata(&self) -> Option<serde_json::Value> {
        let calls = self.lock();
        if calls.is_empty() {
            return None;
        }
        let per_call: Vec<_> = calls
            .iter()
            .map(|c| {
                let d = c.detail.as_ref();
                json!({
                    "model": c.model,
                    "message_id": d.and_then(|d| d.message_id.as_deref()),
                    "request_id": d.and_then(|d| d.request_id.as_deref()),
                    "service_tier": d.and_then(|d| d.service_tier.as_deref()),
                    "stop_reason": d.map(|d| d.stop_reason.as_str()),
                    "stop_details": d.and_then(|d| d.stop_details.as_ref()),
                    "fresh_input_tokens": c.usage.input_tokens,
                    "output_tokens": c.usage.output_tokens,
                    "thinking_tokens": c.usage.reasoning_tokens,
                    "cache_creation_input_tokens": c.usage.cache_creation_input_tokens,
                    "cache_creation_5m_input_tokens": d.map(|d| d.cache_creation_5m),
                    "cache_creation_1h_input_tokens": d.map(|d| d.cache_creation_1h),
                    "cache_read_input_tokens": c.usage.cache_read_input_tokens,
                    "cost_usd": d.map(|d| d.cost_usd),
                    "price_known": d.map(|d| d.price_known),
                    // `false`: the provider reported no cost for this call (OpenAI-direct and
                    // Gemini report none), so it was billed at $0.
                    "cost_reported": c.cost.is_some(),
                })
            })
            .collect();
        drop(calls);
        let t = self.totals();
        Some(json!({
            "schema": 1,
            "row_scope": "one row per agent run; sums every model call in the run",
            "input_tokens_column": "fresh + cache_creation + cache_read",
            "fresh_input_tokens": t.fresh_input_tokens,
            "thinking_tokens": t.reasoning_tokens,
            "calls": per_call,
        }))
    }
}

/// Where and for whom a [`UsageLedger::bill`] row is written.
pub struct BillingContext<'a> {
    pub db: &'a DbPool,
    pub user_id: &'a str,
    pub workspace_id: &'a str,
    /// Chat session, when the call belongs to one (dashboard summaries do not).
    pub session_id: Option<&'a str>,
    /// What made the call (`custom_agent`, `title`, `dashboard_summary`, ...).
    pub component: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    /// BYOK rows cost Kyomi credits nothing (the real cost is kept for diagnostics).
    pub is_byok: bool,
    /// A durable chat run (and the key its store needs): the write is fenced by
    /// the run's lease and keyed by run id.
    pub durable: Option<(&'a crate::adapter::DurableRun, &'a [u8; 32])>,
}

/// A [`BillingContext`] that owns its data, so it can outlive the run that made it.
#[derive(Clone)]
pub(crate) struct OwnedBilling {
    pub db: DbPool,
    pub encryption_key: [u8; 32],
    pub user_id: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub component: String,
    pub provider: String,
    pub model: String,
    pub is_byok: bool,
    pub durable: Option<crate::adapter::DurableRun>,
}

impl OwnedBilling {
    pub(crate) fn context(&self) -> BillingContext<'_> {
        BillingContext {
            db: &self.db,
            user_id: &self.user_id,
            workspace_id: &self.workspace_id,
            session_id: self.session_id.as_deref(),
            component: &self.component,
            provider: &self.provider,
            model: &self.model,
            is_byok: self.is_byok,
            durable: self.durable.as_ref().map(|run| (run, &self.encryption_key)),
        }
    }
}

/// Bills whatever the ledger still holds when it is dropped. A durable run can be
/// dropped mid-flight (user cancel observed through the DB heartbeat, lease loss):
/// the future holding the client and its ledger is discarded and the normal bill
/// at the end of the run never executes, so the calls already paid for would be
/// lost. After a normal `bill` the ledger is empty and this does nothing.
pub(crate) struct BillOnDrop {
    ledger: UsageLedger,
    billing: OwnedBilling,
}

impl BillOnDrop {
    pub(crate) fn new(ledger: UsageLedger, billing: OwnedBilling) -> Self {
        Self { ledger, billing }
    }
}

impl Drop for BillOnDrop {
    fn drop(&mut self) {
        if self.ledger.is_empty() {
            return;
        }
        let ledger = self.ledger.clone();
        let billing = self.billing.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            error!(
                user_id = %billing.user_id,
                workspace_id = %billing.workspace_id,
                component = %billing.component,
                totals = ?ledger.totals(),
                "BILLING ROW LOST: usage ledger dropped with unbilled calls and no runtime to bill them"
            );
            return;
        };
        runtime.spawn(async move {
            if let Err(e) = ledger.bill(&billing.context()).await {
                error!(
                    error = %e,
                    user_id = %billing.user_id,
                    workspace_id = %billing.workspace_id,
                    component = %billing.component,
                    "BILLING ROW LOST: failed to bill a dropped run's usage"
                );
            }
        });
    }
}

/// Hands a durable run's ledger to the worker that owns the run. The run's execution
/// future can be dropped mid-flight (user cancel seen through the DB heartbeat), and
/// the lease-fenced usage write is only valid while the run is still `Running`, so the
/// worker bills what is left *before* it finishes the run: deterministic, unlike a
/// spawned task racing the terminal write. After a normal run the ledger is empty and
/// this does nothing. (If the lease was lost the fence rightly rejects the write and
/// the loss is logged: another owner now owns the run.)
#[derive(Clone, Default)]
pub struct BillingSlot(Arc<SlotInner>);

#[derive(Default)]
struct SlotInner(Mutex<Option<(UsageLedger, OwnedBilling)>>);

impl Drop for SlotInner {
    /// The last holder of a durable run is gone. Anything still unbilled (a panic, a
    /// shutdown abort, a refused write) must not vanish quietly.
    fn drop(&mut self) {
        let Some((ledger, billing)) = self.0.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner).take() else {
            return;
        };
        if !ledger.is_empty() {
            error!(
                user_id = %billing.user_id,
                workspace_id = %billing.workspace_id,
                component = %billing.component,
                model = %billing.model,
                totals = ?ledger.totals(),
                "BILLING ROW LOST: a durable run ended with unbilled model calls"
            );
        }
    }
}

impl std::fmt::Debug for BillingSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BillingSlot")
    }
}

impl BillingSlot {
    /// Called by `execute_agent_chat` once the run's ledger and billing identity exist.
    pub(crate) fn register(&self, ledger: UsageLedger, billing: OwnedBilling) {
        // The raw key is not needed here: `bill_remaining` takes it as an argument.
        let billing = OwnedBilling { encryption_key: [0u8; 32], ..billing };
        let mut slot = self.0 .0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        debug_assert!(
            slot.as_ref().is_none_or(|(held, _)| held.is_empty()),
            "a registration must not replace a ledger that still has unbilled calls"
        );
        *slot = Some((ledger, billing));
    }

    /// Bill any calls the run made but never billed, under the run's *current* lease.
    pub async fn bill_remaining(
        &self,
        run: &crate::adapter::DurableRun,
        encryption_key: &[u8; 32],
    ) -> kyomi_core::Result<()> {
        let registered = self.0 .0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let Some((ledger, mut billing)) = registered else { return Ok(()) };
        if ledger.is_empty() {
            return Ok(());
        }
        billing.durable = Some(run.clone());
        billing.encryption_key = *encryption_key;
        ledger.bill(&billing.context()).await
    }
}

impl UsageLedger {
    /// Take the calls recorded since the last `bill`, so each call is billed once.
    fn drain(&self) -> Vec<CallRecord> {
        std::mem::take(&mut *self.lock())
    }

    /// Write the `api_usage_log` row for the calls recorded since the last
    /// `bill`; no row when none used any tokens. The one writer for every call
    /// type. A durable run's failure propagates (the lease-fenced write is part
    /// of the run); any other failure means that call is not billed, so it is
    /// logged with every value needed to rebuild the row.
    pub async fn bill(&self, ctx: &BillingContext<'_>) -> kyomi_core::Result<()> {
        let taken = self.drain();
        let calls = UsageLedger(Arc::new(Mutex::new(taken.clone())));
        let Some(mut usage) = calls.billing_row(ctx.is_byok, ctx.provider, ctx.model, ctx.component) else {
            return Ok(());
        };
        if let Some((run, encryption_key)) = ctx.durable {
            usage.request_id = Some(run.run_id.as_str().to_string());
            let written = kyomi_auth::conversation_events::ConversationStore::new(
                ctx.db, encryption_key, ctx.user_id, ctx.workspace_id,
            )
            .fenced_api_usage(
                &run.conversation_id, &run.run_id, &run.lease,
                chrono::Utc::now().timestamp_millis(), &usage,
            )
            .await
            .map_err(|error| kyomi_core::Error::ServiceUnavailable(error.to_string()));
            if written.is_err() {
                // Keep the calls so a later attempt (the worker bills what is left before it
                // finishes the run) can still write them.
                self.lock().extend(taken);
            }
            return written;
        }
        if let Err(e) = kyomi_core::db_execute!(
            ctx.db,
            &ApiUsageInsert::insert_sql(matches!(ctx.db, DbPool::Postgres(_))),
            ctx.user_id,
            ctx.workspace_id,
            ctx.session_id,
            chrono::Utc::now(),
            &usage.provider,
            &usage.model,
            usage.input_tokens,
            usage.output_tokens,
            usage.total_tokens,
            usage.cost_estimate,
            &usage.component,
            usage.provider_cost_usd,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
            &usage.extra_metadata,
            None::<&str>
        ) {
            error!(
                error = %e,
                user_id = %ctx.user_id,
                workspace_id = %ctx.workspace_id,
                session_id = ?ctx.session_id,
                component = %usage.component,
                model = %usage.model,
                input_tokens = usage.input_tokens,
                output_tokens = usage.output_tokens,
                cache_creation_input_tokens = usage.cache_creation_input_tokens,
                cache_read_input_tokens = usage.cache_read_input_tokens,
                cost_estimate = usage.cost_estimate,
                "BILLING ROW LOST: failed to log API usage to database"
            );
        }
        Ok(())
    }
}

/// A provider whose every response is recorded in a [`UsageLedger`]. The only
/// way the server obtains an LLM provider (`create_provider_from_workspace`
/// returns this type), so no call can bypass metering.
pub struct MeteredClient {
    inner: Box<dyn LLMProvider>,
    ledger: UsageLedger,
}

impl MeteredClient {
    pub fn new(inner: Box<dyn LLMProvider>) -> Self {
        Self { inner, ledger: UsageLedger::default() }
    }

    /// A handle to this client's ledger (shared with the client).
    pub fn ledger(&self) -> UsageLedger {
        self.ledger.clone()
    }
}

#[async_trait]
impl LLMProvider for MeteredClient {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        options: &RequestOptions,
        user_names: &std::collections::HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        let response = self.inner.complete(messages, tools, options, user_names).await?;
        self.ledger.record(self.inner.model(), &response);
        Ok(response)
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn context_window(&self) -> u32 {
        self.inner.context_window()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(input: u32, out: u32, w: u32, r: u32, think: u32) -> LLMResponse {
        LLMResponse {
            raw_response: serde_json::Value::Null,
            content: String::new(),
            finish_reason: "end_turn".into(),
            usage: AgentTokenUsage {
                input_tokens: input,
                output_tokens: out,
                cache_creation_input_tokens: w,
                cache_read_input_tokens: r,
                reasoning_tokens: think,
            },
            tool_calls: None,
            cost: Some(0.01),
            thinking_content: None,
            call_detail: Some(ProviderCallDetail {
                message_id: Some("msg_1".into()),
                request_id: Some("req_1".into()),
                cache_creation_5m: w,
                stop_reason: "end_turn".into(),
                cost_usd: 0.01,
                price_known: true,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn empty_ledger_has_no_metadata_and_zero_totals() {
        let l = UsageLedger::default();
        assert!(l.extra_metadata().is_none());
        assert_eq!(l.totals(), LedgerTotals::default());
    }

    #[test]
    fn totals_sum_every_call_and_prompt_tokens_include_cache() {
        let l = UsageLedger::default();
        l.record("claude-sonnet-5-5", &response(10, 100, 5000, 0, 40));
        l.record("claude-sonnet-5-5", &response(20, 200, 0, 5000, 60));
        let t = l.totals();
        assert_eq!(t.fresh_input_tokens, 30);
        assert_eq!(t.output_tokens, 300);
        assert_eq!(t.cache_creation_input_tokens, 5000);
        assert_eq!(t.cache_read_input_tokens, 5000);
        assert_eq!(t.reasoning_tokens, 100);
        assert_eq!(t.prompt_tokens(), 10_030);
    }

    #[test]
    fn metadata_lists_each_call_with_its_breakdown() {
        let l = UsageLedger::default();
        l.record("claude-sonnet-5-5", &response(10, 100, 5000, 0, 40));
        l.record("claude-sonnet-5-5", &response(20, 200, 0, 5000, 60));
        let m = l.extra_metadata().unwrap();
        let calls = m["calls"].as_array().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["cache_creation_input_tokens"], 5000);
        assert_eq!(calls[0]["cache_read_input_tokens"], 0);
        assert_eq!(calls[1]["cache_read_input_tokens"], 5000);
        assert_eq!(calls[0]["message_id"], "msg_1");
        assert_eq!(calls[0]["request_id"], "req_1");
        assert_eq!(m["fresh_input_tokens"], 30);
    }

    #[test]
    fn billing_row_sums_cost_tokens_and_cache_for_the_whole_run() {
        let l = UsageLedger::default();
        l.record("claude-sonnet-5-5", &response(10, 100, 5000, 0, 40));
        l.record("claude-sonnet-5-5", &response(20, 200, 0, 5000, 60));
        let row = l.billing_row(false, "anthropic", "m", "chat_agent").unwrap();
        assert_eq!(row.input_tokens, 10_030);
        assert_eq!(row.output_tokens, 300);
        assert_eq!(row.total_tokens, 10_330);
        assert_eq!((row.cache_creation_input_tokens, row.cache_read_input_tokens), (5000, 5000));
        assert!((row.cost_estimate - 0.02 * AI_COST_MULTIPLIER).abs() < 1e-12);
        assert_eq!(row.provider_cost_usd, None);
        assert!(row.extra_metadata.is_some());
    }

    #[test]
    fn byok_row_is_free_to_kyomi_and_keeps_the_real_cost() {
        let l = UsageLedger::default();
        l.record("claude-sonnet-5-5", &response(10, 100, 0, 0, 0));
        let row = l.billing_row(true, "anthropic", "m", "chat_agent").unwrap();
        assert_eq!(row.cost_estimate, 0.0);
        assert_eq!(row.provider_cost_usd, Some(0.01));
    }

    #[test]
    fn no_tokens_means_no_row() {
        assert!(UsageLedger::default().billing_row(false, "anthropic", "m", "chat_agent").is_none());
    }

    #[test]
    fn huge_totals_saturate_instead_of_wrapping() {
        let l = UsageLedger::default();
        l.record("m", &response(u32::MAX, u32::MAX, 0, 0, 0));
        l.record("m", &response(u32::MAX, u32::MAX, 0, 0, 0));
        let row = l.billing_row(false, "anthropic", "m", "chat_agent").unwrap();
        assert_eq!((row.input_tokens, row.total_tokens), (i32::MAX, i32::MAX));
    }

    /// A Kyomi-billed call whose provider reports no cost (OpenAI direct, Gemini) is
    /// flagged in the row's per-call metadata instead of looking priced.
    #[test]
    fn a_call_with_no_reported_cost_is_flagged_in_the_row_metadata() {
        let l = UsageLedger::default();
        let mut r = response(10, 5, 0, 0, 0);
        r.cost = None;
        l.record("gpt-direct", &r);
        let row = l.billing_row(false, "openai", "gpt-direct", "custom_agent").unwrap();
        let meta: serde_json::Value = serde_json::from_str(row.extra_metadata.as_deref().unwrap()).unwrap();
        assert_eq!(meta["calls"][0]["cost_reported"], false);
        assert_eq!(row.cost_estimate, 0.0);
    }

    #[test]
    fn calls_without_detail_still_record_tokens() {
        let l = UsageLedger::default();
        let mut r = response(1, 2, 3, 4, 0);
        r.call_detail = None;
        l.record("gpt", &r);
        assert_eq!(l.totals().cache_read_input_tokens, 4);
        assert!(l.extra_metadata().unwrap()["calls"][0]["message_id"].is_null());
    }
}
