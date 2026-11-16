// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-run record of every model call's token usage (KYO-974).
//!
//! One `api_usage_log` row is written per agent run (not per model call), so
//! the run's cache totals and the per-call breakdown are accumulated here as
//! each response arrives and written with that row.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::types::{AgentTokenUsage, LLMResponse, ProviderCallDetail};

/// Shared handle: the agent loop records into it, `execute_agent_chat` reads it.
#[derive(Clone, Default)]
pub struct UsageLedger(Arc<Mutex<Vec<CallRecord>>>);

#[derive(Debug, Clone)]
struct CallRecord {
    model: String,
    usage: AgentTokenUsage,
    detail: Option<ProviderCallDetail>,
}

/// Sums over every call in a run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerTotals {
    /// Uncached input tokens.
    pub fresh_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub reasoning_tokens: u64,
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
            detail: response.call_detail.clone(),
        });
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
    fn calls_without_detail_still_record_tokens() {
        let l = UsageLedger::default();
        let mut r = response(1, 2, 3, 4, 0);
        r.call_detail = None;
        l.record("gpt", &r);
        assert_eq!(l.totals().cache_read_input_tokens, 4);
        assert!(l.extra_metadata().unwrap()["calls"][0]["message_id"].is_null());
    }
}
