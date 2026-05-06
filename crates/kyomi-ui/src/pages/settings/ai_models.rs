// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog of AI models shown in the workspace AI settings page.
//!
//! Two distinct catalogs live here:
//!
//! * [`KYOMI_CREDITS_MODELS`] — the curated list rendered when the workspace
//!   is in Kyomi-credits mode. Pricing is metered against the AI token bundle
//!   in [`kyomi-agent`], so this list must stay in lockstep with the providers
//!   the bundle accounting actually supports (Anthropic only at the moment).
//! * BYOK models — fetched live from each provider via
//!   [`crate::server_fns::ai::list_workspace_ai_models`]. The frontend filters
//!   to chat-completion-capable entries server-side, so there is no static
//!   list maintained here for BYOK mode.
//!
//! IDs in [`KYOMI_CREDITS_MODELS`] are cross-checked against
//! `kyomi-agent::anthropic::get_model_pricing`.

/// A single selectable model option.
pub struct ModelOption {
    pub id: &'static str,
    pub label: &'static str,
}

/// Models Kyomi supports in credits mode.
///
/// Kyomi credits are billed against the AI token bundle (Anthropic-only — the
/// existing bundle billing only meters Anthropic). Keep this list Anthropic-only
/// until the bundle accounting supports other providers.
pub const KYOMI_CREDITS_MODELS: &[ModelOption] = &[
    ModelOption { id: "claude-sonnet-4-6", label: "Claude Sonnet 4.6" },
    ModelOption { id: "claude-haiku-4-5-20251001", label: "Claude Haiku 4.5" },
];

/// Human-readable label for a model ID.
///
/// Only the curated [`KYOMI_CREDITS_MODELS`] catalog is consulted; BYOK model
/// IDs (which are now fetched dynamically per workspace) fall back to the raw
/// ID, which is already the canonical name shown to the user.
pub fn label_for_model(_provider: &str, model_id: &str) -> String {
    KYOMI_CREDITS_MODELS
        .iter()
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
