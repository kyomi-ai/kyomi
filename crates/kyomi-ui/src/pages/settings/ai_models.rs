// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog of AI models shown in the workspace AI settings page.
//!
//! The canonical model ID strings are authoritatively validated against the
//! provider client code in `kyomi-agent` — if those drift, this file is the
//! place to update. At time of writing, `kyomi-agent` does not expose a single
//! list constant per provider (only `DEFAULT_MODEL` and per-model pricing
//! lookups), so we maintain the catalog here and cross-check IDs during review.
//!
//! Sources of truth:
//! - `crates/kyomi-agent/src/anthropic.rs`
//! - `crates/kyomi-agent/src/openai.rs`
//! - `crates/kyomi-agent/src/gemini.rs`

/// A single selectable model option.
pub struct ModelOption {
    pub id: &'static str,
    pub label: &'static str,
}

/// Anthropic models offered in BYOK mode.
///
/// IDs cross-checked against `kyomi-agent::anthropic::get_model_pricing`.
pub const ANTHROPIC_MODELS: &[ModelOption] = &[
    ModelOption { id: "claude-sonnet-4-5-20250929", label: "Claude Sonnet 4.5" },
    ModelOption { id: "claude-opus-4-20250514", label: "Claude Opus 4" },
    ModelOption { id: "claude-haiku-4-5-20251001", label: "Claude Haiku 4.5" },
];

/// OpenAI models offered in BYOK mode.
///
/// IDs cross-checked against `kyomi-agent::openai` (`DEFAULT_MODEL = "gpt-4o-mini"`).
pub const OPENAI_MODELS: &[ModelOption] = &[
    ModelOption { id: "gpt-4o", label: "GPT-4o" },
    ModelOption { id: "gpt-4o-mini", label: "GPT-4o mini" },
];

/// Gemini models offered in BYOK mode.
///
/// IDs cross-checked against `kyomi-agent::gemini::calculate_cost` (recognises
/// `gemini-2.5-pro`, `gemini-2.5-flash`, `gemini-2.0-flash`).
pub const GEMINI_MODELS: &[ModelOption] = &[
    ModelOption { id: "gemini-2.5-pro", label: "Gemini 2.5 Pro" },
    ModelOption { id: "gemini-2.5-flash", label: "Gemini 2.5 Flash" },
];

/// Models Kyomi supports in credits mode.
///
/// Kyomi credits are billed against the AI token bundle (Anthropic-only — the
/// existing bundle billing only meters Anthropic). Keep this list Anthropic-only
/// until the bundle accounting supports other providers.
pub const KYOMI_CREDITS_MODELS: &[ModelOption] = &[
    ModelOption { id: "claude-sonnet-4-5-20250929", label: "Claude Sonnet 4.5" },
    ModelOption { id: "claude-haiku-4-5-20251001", label: "Claude Haiku 4.5" },
];

/// Return the BYOK model list for a given provider id.
pub fn models_for_provider(provider: &str) -> &'static [ModelOption] {
    match provider {
        "anthropic" => ANTHROPIC_MODELS,
        "openai" => OPENAI_MODELS,
        "gemini" => GEMINI_MODELS,
        _ => &[],
    }
}

/// Human-readable label for a model ID, falling back to the ID itself if the
/// catalog doesn't know about it (custom model IDs in BYOK mode).
pub fn label_for_model(provider: &str, model_id: &str) -> String {
    models_for_provider(provider)
        .iter()
        .chain(KYOMI_CREDITS_MODELS.iter())
        .find(|m| m.id == model_id)
        .map(|m| m.label.to_string())
        .unwrap_or_else(|| model_id.to_string())
}

/// Human-readable label for a provider id.
pub fn provider_label(provider: &str) -> &'static str {
    match provider {
        "kyomi" => "Kyomi credits",
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "gemini" => "Gemini",
        _ => "Unknown",
    }
}
