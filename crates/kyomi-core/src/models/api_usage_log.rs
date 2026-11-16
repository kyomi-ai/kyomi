// SPDX-License-Identifier: AGPL-3.0-or-later

//! API usage log model — maps to the `api_usage_log` table.
//!
//! Records individual LLM API calls for billing and usage tracking.
//! Each row represents one call to an AI provider (Anthropic, etc.).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An individual LLM API call record for billing/usage tracking.
///
/// The `cost_estimate` field (in USD) is the primary billing metric.
/// Token counts are retained for detailed analytics.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ApiUsageLog {
    /// Auto-increment primary key.
    pub id: i32,

    /// FK to users table.
    pub user_id: String,

    /// FK to workspaces table.
    pub workspace_id: String,

    /// FK to chat_sessions table (nullable — not all calls are chat-scoped).
    pub session_id: Option<String>,

    /// When the API call was made.
    pub timestamp: DateTime<Utc>,

    /// LLM provider name (e.g. "anthropic", "openai").
    pub provider: String,

    /// Model identifier (e.g. "claude-3-haiku").
    pub model: String,

    /// Number of input tokens.
    pub input_tokens: i32,

    /// Number of output tokens.
    pub output_tokens: i32,

    /// Total tokens (input + output).
    pub total_tokens: i32,

    /// Prompt cache creation tokens (Anthropic-specific).
    #[sqlx(default)]
    pub cache_creation_input_tokens: i32,

    /// Prompt cache read tokens (Anthropic-specific).
    #[sqlx(default)]
    pub cache_read_input_tokens: i32,

    /// Estimated cost in USD for this call.
    ///
    /// This is the amount billed against Kyomi bundle credits. For BYOK rows
    /// (where the workspace pays the provider directly) this is always `0.0`
    /// so `BillingService::calculate_credits_info`'s `SUM(cost_estimate)` does
    /// not debit Kyomi credits for BYOK traffic.
    pub cost_estimate: Option<f64>,

    /// Real upstream provider cost in USD (observability only).
    ///
    /// * `NULL` for Kyomi-mode rows — `cost_estimate` already reflects the
    ///   real cost (plus markup), so this column would be redundant.
    /// * Populated for BYOK rows with the raw provider cost computed from
    ///   token counts × provider pricing. Not used for billing; retained so
    ///   we can surface upstream provider spend in diagnostics.
    #[sqlx(default)]
    pub provider_cost_usd: Option<f64>,

    /// Component that made the call (e.g. "chat_agent", "sql_copilot", "kyomi_watch").
    pub component: Option<String>,

    /// Unique request identifier for tracing.
    pub request_id: Option<String>,

    /// Additional metadata JSON blob.
    pub extra_metadata: Option<serde_json::Value>,
}

/// The values written for one `api_usage_log` row, and the statement that
/// writes them. Every writer (the legacy chat path and the durable-run path)
/// uses this one definition so the column list cannot drift (KYO-974).
#[derive(Debug, Clone, PartialEq)]
pub struct ApiUsageInsert {
    pub provider: String,
    pub model: String,
    /// Whole prompt: fresh + cache writes + cache reads.
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
    /// Billed against credits: provider cost x markup, or 0 for BYOK.
    pub cost_estimate: f64,
    pub component: String,
    /// Real provider cost, BYOK rows only.
    pub provider_cost_usd: Option<f64>,
    pub cache_creation_input_tokens: i32,
    pub cache_read_input_tokens: i32,
    /// Per-call breakdown, serialized JSON.
    pub extra_metadata: Option<String>,
    /// Durable-run rows carry the run id so a re-executed run cannot bill twice.
    pub request_id: Option<String>,
}

impl ApiUsageInsert {
    /// The INSERT statement. Binds, in order: `$1` user_id, `$2` workspace_id,
    /// `$3` session_id, `$4` timestamp, `$5` provider, `$6` model,
    /// `$7` input_tokens, `$8` output_tokens, `$9` total_tokens,
    /// `$10` cost_estimate, `$11` component, `$12` provider_cost_usd,
    /// `$13` cache_creation_input_tokens, `$14` cache_read_input_tokens,
    /// `$15` extra_metadata, `$16` request_id. `extra_metadata` is a `json`
    /// column on Postgres (the text parameter needs the cast) and `TEXT` on SQLite.
    pub fn insert_sql(postgres: bool) -> String {
        let metadata = if postgres { "$15::text::json" } else { "$15" };
        format!(
            "INSERT INTO api_usage_log (user_id, workspace_id, session_id, timestamp, \
             provider, model, input_tokens, output_tokens, total_tokens, \
             cache_creation_input_tokens, cache_read_input_tokens, cost_estimate, \
             component, provider_cost_usd, extra_metadata, request_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $13, $14, $10, $11, $12, {metadata}, $16)"
        )
    }
}

#[cfg(test)]
mod insert_tests {
    use super::ApiUsageInsert;

    #[test]
    fn insert_binds_every_placeholder_once_and_no_literals() {
        for postgres in [true, false] {
            let sql = ApiUsageInsert::insert_sql(postgres);
            for n in 1..=16 {
                assert_eq!(sql.matches(&format!("${n},")).count() + sql.matches(&format!("${n})")).count()
                    + sql.matches(&format!("${n}::")).count(), 1, "${n} in {sql}");
            }
            assert!(!sql.contains("$17"));
            assert!(!sql.contains(", 0, 0,"), "cache columns must be real values: {sql}");
            assert_eq!(sql.contains("::text::json"), postgres);
        }
    }
}
