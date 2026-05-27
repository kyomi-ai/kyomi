// SPDX-License-Identifier: AGPL-3.0-or-later

//! Display helpers for AI model and provider IDs.
//!
//! * [`label_for_model`] — human-readable display name for a model ID.
//!   Kyomi-credits mode now fetches available models dynamically via
//!   [`crate::server_fns::ai::list_openrouter_models`], so no static model
//!   catalog is maintained here.
//! * [`provider_label`] — human-readable display name for a provider ID.

/// Human-readable label for a model ID.
///
/// Model IDs are used directly as display names — they are already the
/// canonical names shown to users (e.g. `"claude-sonnet-4-6"`).
pub fn label_for_model(_provider: &str, model_id: &str) -> String {
    model_id.to_string()
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
