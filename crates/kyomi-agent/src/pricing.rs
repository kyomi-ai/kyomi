// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared pricing types and cost calculation for LLM providers.
//!
//! Each provider (Anthropic, OpenAI, Gemini) has its own `get_model_pricing`
//! function that returns a [`ModelPricing`] for the given model name. The
//! shared [`calculate_cost`] function then computes the total USD cost from
//! the pricing and [`AgentTokenUsage`], including Anthropic-style cache tokens.
//!
//! Because OpenAI and Gemini always return 0 for the cache token fields,
//! the formula is universal — the cache terms simply vanish for those providers.

use crate::types::AgentTokenUsage;

// ---------------------------------------------------------------------------
// ModelPricing
// ---------------------------------------------------------------------------

/// Pricing per million tokens for an LLM model (in USD).
pub struct ModelPricing {
    /// Cost per million input tokens (USD).
    pub input: f64,
    /// Cost per million output tokens (USD).
    pub output: f64,
}

// ---------------------------------------------------------------------------
// calculate_cost
// ---------------------------------------------------------------------------

/// Calculate estimated cost in USD for an LLM API call.
///
/// Pricing breakdown:
/// - **Input tokens**: base price per million tokens
/// - **Cache write tokens**: 1.25x base input price (Anthropic prompt caching)
/// - **Cache read tokens**: 0.1x base input price (Anthropic prompt caching, 90% savings)
/// - **Output tokens**: output price per million tokens
///
/// For providers that do not support prompt caching (OpenAI, Gemini), the
/// `cache_creation_input_tokens` and `cache_read_input_tokens` fields in
/// [`AgentTokenUsage`] are always 0, so the cache terms evaluate to zero and the
/// formula reduces to `input_cost + output_cost`.
pub fn calculate_cost(pricing: &ModelPricing, usage: &AgentTokenUsage) -> f64 {
    let per_million = 1_000_000.0_f64;

    let input_cost = (f64::from(usage.input_tokens) / per_million) * pricing.input;
    let cache_write_cost =
        (f64::from(usage.cache_creation_input_tokens) / per_million) * pricing.input * 1.25;
    let cache_read_cost =
        (f64::from(usage.cache_read_input_tokens) / per_million) * pricing.input * 0.1;
    let output_cost = (f64::from(usage.output_tokens) / per_million) * pricing.output;

    input_cost + cache_write_cost + cache_read_cost + output_cost
}

// ---------------------------------------------------------------------------
// calculate_cost_with_fallback
// ---------------------------------------------------------------------------

/// Calculate estimated cost in USD, using a provider-specific pricing lookup
/// with a fallback for unknown models.
///
/// - `lookup` returns `Some(ModelPricing)` for known models, `None` otherwise.
/// - When `None`, logs a warning at `tracing::warn!` level identifying the
///   unknown model and provider, then uses `fallback` as the pricing.
/// - Delegates to [`calculate_cost`] for the actual arithmetic.
pub fn calculate_cost_with_fallback(
    model: &str,
    usage: &AgentTokenUsage,
    lookup: impl FnOnce(&str) -> Option<ModelPricing>,
    fallback: ModelPricing,
    provider_name: &str,
) -> f64 {
    let pricing = lookup(model).unwrap_or_else(|| {
        tracing::warn!(
            model,
            provider = provider_name,
            "unknown model for cost calculation, using fallback pricing"
        );
        fallback
    });
    calculate_cost(&pricing, usage)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_usage(input: u32, output: u32) -> AgentTokenUsage {
        AgentTokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_tokens: 0,
        }
    }

    #[test]
    fn basic_input_output() {
        let pricing = ModelPricing {
            input: 3.00,
            output: 15.00,
        };
        let usage = make_usage(1_000_000, 1_000_000);
        let cost = calculate_cost(&pricing, &usage);
        // 1M * $3/M + 1M * $15/M = $18.00
        assert!((cost - 18.0).abs() < 0.001);
    }

    #[test]
    fn with_cache_tokens() {
        let pricing = ModelPricing {
            input: 3.00,
            output: 15.00,
        };
        let usage = AgentTokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            cache_creation_input_tokens: 200_000,
            cache_read_input_tokens: 500_000,
            reasoning_tokens: 0,
        };
        let cost = calculate_cost(&pricing, &usage);
        // input: 100K/1M * $3 = $0.30
        // cache write: 200K/1M * $3 * 1.25 = $0.75
        // cache read: 500K/1M * $3 * 0.1 = $0.15
        // output: 50K/1M * $15 = $0.75
        // total = $1.95
        assert!((cost - 1.95).abs() < 0.001);
    }

    #[test]
    fn zero_tokens() {
        let pricing = ModelPricing {
            input: 3.00,
            output: 15.00,
        };
        let usage = AgentTokenUsage::default();
        let cost = calculate_cost(&pricing, &usage);
        assert!((cost - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cache_terms_zero_for_non_anthropic_usage() {
        // Simulates OpenAI/Gemini: cache fields are 0, formula = input + output only.
        let pricing = ModelPricing {
            input: 0.15,
            output: 0.60,
        };
        let usage = make_usage(1_000_000, 1_000_000);
        let cost = calculate_cost(&pricing, &usage);
        // 1M * $0.15/M + 1M * $0.60/M = $0.75
        assert!((cost - 0.75).abs() < 0.001);
    }
}
