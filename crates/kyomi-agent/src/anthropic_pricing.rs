// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-model Anthropic prices and context windows (KYO-974).
//!
//! The Messages API reports token counts but no cost, and `cost_estimate`
//! feeds credits and spend caps (`BillingService`), so every Anthropic call is
//! priced here from its token counts.
//!
//! Source: <https://platform.claude.com/docs/en/about-claude/pricing> and
//! `.../models/overview`, read 2026-11-16. Prices are USD per million tokens.
//! Re-verify against those pages before changing a row. Rules taken from the
//! pricing page:
//!
//! * 5-minute cache writes cost 1.25x input and 1-hour writes 2x input, except
//!   where a row below differs (the table lists the published numbers).
//! * Cache reads cost 0.1x input, except Opus 5.5 / Sonnet 5.5 (0.05x) and
//!   Fable 5.2 / Mythos 5.2 (0.025x).
//! * Claude Haiku 5.5 is priced by prompt length: a request whose prompt
//!   (input + cache writes + cache reads) is over 100,000 tokens pays the
//!   higher row for the whole request. Every other 4.6+ model includes the
//!   full 1M window at standard pricing.
//! * Thinking tokens are part of `output_tokens` and are billed as output.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use tracing::error;

/// Prompt length above which Haiku 5.5 uses its higher price row.
const HAIKU_5_5_TIER_THRESHOLD: u64 = 100_000;

/// USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

const fn rates(input: f64, output: f64, w5m: f64, w1h: f64, read: f64) -> Rates {
    Rates { input, output, cache_write_5m: w5m, cache_write_1h: w1h, cache_read: read }
}

/// Applied to a model id with no row so an unrecognised model over-estimates
/// rather than bills nothing. Each component is the maximum published for it
/// (all from Claude Opus 4 / 4.1); a test checks it dominates every row.
const FALLBACK_RATES: Rates = rates(15.0, 75.0, 18.75, 30.0, 1.5);

/// How one model family is priced and sized.
struct Family {
    /// Model id with any trailing `-YYYYMMDD` snapshot suffix removed.
    id: &'static str,
    rates: Rates,
    /// `Some` for Haiku 5.5: rates used when the prompt exceeds the threshold.
    long_prompt_rates: Option<Rates>,
    context_window: u32,
    /// Accepts `thinking: {type: "adaptive"}` (Opus/Sonnet 4.6+ and the 5.x models).
    /// Older models answer 400 to it.
    adaptive_thinking: bool,
}

const WINDOW_1M: u32 = 1_000_000;
const WINDOW_200K: u32 = 200_000;
const OPUS_STANDARD: Rates = rates(5.0, 25.0, 6.25, 10.0, 0.5);
const SONNET_STANDARD: Rates = rates(3.0, 15.0, 3.75, 6.0, 0.3);

const FAMILIES: &[Family] = &[
    Family { id: "claude-fable-5-2", rates: rates(8.0, 40.0, 10.0, 16.0, 0.2), long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-mythos-5-2", rates: rates(8.0, 40.0, 10.0, 16.0, 0.2), long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-fable-5-1", rates: rates(10.0, 50.0, 12.5, 20.0, 0.25), long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-mythos-5-1", rates: rates(10.0, 50.0, 12.5, 20.0, 0.25), long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-fable-5", rates: rates(10.0, 50.0, 12.5, 20.0, 1.0), long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-mythos-5", rates: rates(10.0, 50.0, 12.5, 20.0, 1.0), long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-opus-5-5", rates: rates(4.0, 20.0, 5.0, 8.0, 0.2), long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-opus-5", rates: OPUS_STANDARD, long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-opus-4-8", rates: OPUS_STANDARD, long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-opus-4-7", rates: OPUS_STANDARD, long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-opus-4-6", rates: OPUS_STANDARD, long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-opus-4-5", rates: OPUS_STANDARD, long_prompt_rates: None, context_window: WINDOW_200K, adaptive_thinking: false },
    Family { id: "claude-opus-4-1", rates: rates(15.0, 75.0, 18.75, 30.0, 1.5), long_prompt_rates: None, context_window: WINDOW_200K, adaptive_thinking: false },
    Family { id: "claude-opus-4-0", rates: rates(15.0, 75.0, 18.75, 30.0, 1.5), long_prompt_rates: None, context_window: WINDOW_200K, adaptive_thinking: false },
    Family { id: "claude-opus-4", rates: rates(15.0, 75.0, 18.75, 30.0, 1.5), long_prompt_rates: None, context_window: WINDOW_200K, adaptive_thinking: false },
    Family { id: "claude-sonnet-5-5", rates: rates(2.0, 10.0, 2.5, 4.0, 0.1), long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-sonnet-5", rates: rates(2.0, 10.0, 2.5, 4.0, 0.2), long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-sonnet-4-6", rates: SONNET_STANDARD, long_prompt_rates: None, context_window: WINDOW_1M, adaptive_thinking: true },
    Family { id: "claude-sonnet-4-5", rates: SONNET_STANDARD, long_prompt_rates: None, context_window: WINDOW_200K, adaptive_thinking: false },
    Family { id: "claude-sonnet-4-0", rates: SONNET_STANDARD, long_prompt_rates: None, context_window: WINDOW_200K, adaptive_thinking: false },
    Family { id: "claude-sonnet-4", rates: SONNET_STANDARD, long_prompt_rates: None, context_window: WINDOW_200K, adaptive_thinking: false },
    Family {
        id: "claude-haiku-5-5",
        rates: rates(0.10, 0.50, 0.125, 0.20, 0.01),
        long_prompt_rates: Some(rates(0.50, 2.50, 0.625, 1.0, 0.05)),
        context_window: WINDOW_1M,
        adaptive_thinking: true,
    },
    Family { id: "claude-haiku-4-5", rates: rates(1.0, 5.0, 1.25, 2.0, 0.1), long_prompt_rates: None, context_window: WINDOW_200K, adaptive_thinking: false },
];

/// `claude-haiku-4-5-20251001` -> `claude-haiku-4-5`; `-latest` is dropped too.
/// Only an exact 8-digit snapshot suffix is removed, so `claude-opus-4-5` is never mistaken for
/// `claude-opus-4`.
fn family_id(model: &str) -> &str {
    let model = model.strip_suffix("-latest").unwrap_or(model);
    match model.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => model,
    }
}

fn family(model: &str) -> Option<&'static Family> {
    let id = family_id(model);
    FAMILIES.iter().find(|f| f.id == id)
}

/// Token counts for one Messages API call, as the API reports them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallTokens {
    /// Fresh (uncached) input tokens.
    pub input: u64,
    /// Output tokens, including thinking tokens.
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
}

/// A computed price and whether the model had a price row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricedCall {
    pub cost_usd: f64,
    /// `false` when the model id is unknown and [`FALLBACK_RATES`] were used.
    pub known_model: bool,
}

/// Price one call. An unknown model is logged at error level and priced at the
/// highest published rates; it never costs 0.
pub fn price_call(model: &str, tokens: CallTokens) -> PricedCall {
    let (r, known_model) = match family(model) {
        Some(f) => {
            let prompt = tokens.input + tokens.cache_write_5m + tokens.cache_write_1h + tokens.cache_read;
            let r = match f.long_prompt_rates {
                Some(long) if prompt > HAIKU_5_5_TIER_THRESHOLD => long,
                _ => f.rates,
            };
            (r, true)
        }
        None => {
            // Once per model per process: a run makes many calls and the row's
            // `extra_metadata` already records `price_known: false` for each.
            static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
            let first = SEEN
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(model.to_string());
            if first {
                error!(model, "no Anthropic price for model; billing at the highest published rates (add it to anthropic_pricing.rs)");
            }
            (FALLBACK_RATES, false)
        }
    };
    let per_million = |tokens: u64, rate: f64| tokens as f64 * rate / 1_000_000.0;
    let cost_usd = per_million(tokens.input, r.input)
        + per_million(tokens.output, r.output)
        + per_million(tokens.cache_write_5m, r.cache_write_5m)
        + per_million(tokens.cache_write_1h, r.cache_write_1h)
        + per_million(tokens.cache_read, r.cache_read);
    PricedCall { cost_usd, known_model }
}

/// Whether the model accepts `thinking: {type: "adaptive"}`. An unknown model
/// is not sent it, so a new id never turns into a 400.
pub fn supports_adaptive_thinking(model: &str) -> bool {
    family(model).is_some_and(|f| f.adaptive_thinking)
}

/// Context window in tokens. Unknown models report 200K, the smallest window
/// any current model has, so the UI never understates usage.
pub fn context_window(model: &str) -> u32 {
    family(model).map_or(WINDOW_200K, |f| f.context_window)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn sonnet_5_5_hand_calculation() {
        // 1,000 fresh in, 2,000 out, 10,000 5m writes, 4,000 1h writes, 50,000 reads.
        // 1000*2 + 2000*10 + 10000*2.5 + 4000*4 + 50000*0.1 = 2000+20000+25000+16000+5000 = 68,000 / 1e6
        let p = price_call(
            "claude-sonnet-5-5",
            CallTokens { input: 1_000, output: 2_000, cache_write_5m: 10_000, cache_write_1h: 4_000, cache_read: 50_000 },
        );
        assert!(p.known_model);
        assert!(close(p.cost_usd, 0.068), "{}", p.cost_usd);
    }

    #[test]
    fn haiku_5_5_lower_tier_at_exactly_the_threshold() {
        // prompt = 100,000 -> not over the threshold. 100000*0.10 + 1000*0.50 = 10,500 / 1e6
        let p = price_call("claude-haiku-5-5", CallTokens { input: 100_000, output: 1_000, ..Default::default() });
        assert!(close(p.cost_usd, 0.0105), "{}", p.cost_usd);
    }

    #[test]
    fn haiku_5_5_higher_tier_counts_cache_tokens_toward_the_prompt() {
        // prompt = 1,000 fresh + 99,001 read = 100,001 -> whole request at the higher row.
        // 1000*0.50 + 99001*0.05 + 1000*2.50 = 500 + 4950.05 + 2500 = 7950.05 / 1e6
        let p = price_call(
            "claude-haiku-5-5",
            CallTokens { input: 1_000, output: 1_000, cache_read: 99_001, ..Default::default() },
        );
        assert!(close(p.cost_usd, 0.00795005), "{}", p.cost_usd);
    }

    #[test]
    fn dated_snapshot_resolves_to_its_family() {
        let a = price_call("claude-haiku-4-5-20251001", CallTokens { input: 1_000_000, ..Default::default() });
        assert!(a.known_model && close(a.cost_usd, 1.0));
        // `claude-opus-4-5` must not be read as the retired `claude-opus-4` ($15).
        let b = price_call("claude-opus-4-5", CallTokens { input: 1_000_000, ..Default::default() });
        assert!(b.known_model && close(b.cost_usd, 5.0));
    }

    #[test]
    fn fallback_dominates_every_published_row() {
        for f in FAMILIES {
            for r in [Some(f.rates), f.long_prompt_rates].into_iter().flatten() {
                assert!(FALLBACK_RATES.input >= r.input && FALLBACK_RATES.output >= r.output, "{}", f.id);
                assert!(FALLBACK_RATES.cache_write_5m >= r.cache_write_5m, "{}", f.id);
                assert!(FALLBACK_RATES.cache_write_1h >= r.cache_write_1h, "{}", f.id);
                assert!(FALLBACK_RATES.cache_read >= r.cache_read, "{}", f.id);
            }
        }
    }

    #[test]
    fn published_aliases_resolve_to_their_family() {
        let opus4 = price_call("claude-opus-4-0", CallTokens { input: 1_000_000, ..Default::default() });
        assert!(opus4.known_model && close(opus4.cost_usd, 15.0));
        let latest = price_call("claude-haiku-4-5-latest", CallTokens { input: 1_000_000, ..Default::default() });
        assert!(latest.known_model && close(latest.cost_usd, 1.0));
    }

    #[test]
    fn unknown_model_is_never_free() {
        let p = price_call("claude-future-9", CallTokens { input: 1_000_000, output: 1_000_000, ..Default::default() });
        assert!(!p.known_model);
        assert!(close(p.cost_usd, 90.0), "{}", p.cost_usd);
    }

    #[test]
    fn adaptive_thinking_support() {
        for m in ["claude-sonnet-5-5", "claude-haiku-5-5", "claude-opus-4-6", "claude-sonnet-4-6"] {
            assert!(supports_adaptive_thinking(m), "{m}");
        }
        for m in ["claude-haiku-4-5-20251001", "claude-sonnet-4-5-20250929", "claude-opus-4-5", "claude-future-9"] {
            assert!(!supports_adaptive_thinking(m), "{m}");
        }
    }

    #[test]
    fn context_windows() {
        assert_eq!(context_window("claude-sonnet-5-5"), 1_000_000);
        assert_eq!(context_window("claude-haiku-5-5"), 1_000_000);
        assert_eq!(context_window("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(context_window("claude-haiku-4-5-20251001"), 200_000);
        assert_eq!(context_window("claude-sonnet-4-5-20250929"), 200_000);
        assert_eq!(context_window("something-else"), 200_000);
    }
}
