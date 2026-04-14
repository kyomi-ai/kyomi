// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace AI configuration server functions (Workspace BYOK).
//!
//! Backs the Settings → AI page. Reads and writes the per-workspace provider
//! choice (`kyomi` | `anthropic` | `openai` | `gemini`), optional API key
//! (AES-GCM encrypted at rest via [`kyomi_auth::workspace_secrets`]), optional
//! base URL, and default model.
//!
//! Semantics:
//!
//! * [`get_workspace_ai_config`] — any workspace member may read. Never
//!   returns the plaintext API key; callers get `has_api_key: bool` instead.
//! * [`update_workspace_ai_config`] — admin/owner only. Passing `api_key =
//!   None` for a BYOK provider preserves the existing encrypted key (used for
//!   model-only updates).
//! * [`test_workspace_ai_config`] — admin/owner only. Performs a free
//!   auth-check GET against the upstream provider using the candidate
//!   credentials. Never writes to the DB and never logs the key. The 10s
//!   timeout prevents a slow upstream from tying up a server thread.
//!
//! **SaaS-only**: all three server fns return a clear error when running in
//! self-hosted mode. BYOK in self-hosted already has a workspace-secrets
//! encryption-unavailable guard; this top-level gate keeps the feature off
//! entirely until we ship a self-hosted story.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Sanitised view of a workspace's AI configuration.
///
/// Never contains the plaintext API key — callers get `has_api_key` to drive
/// "Key saved ✓" UI instead.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceAiConfigView {
    /// `"kyomi" | "anthropic" | "openai" | "gemini"`.
    pub provider: String,
    /// Workspace default model name (from `settings.custom_settings.default_model`).
    pub model: Option<String>,
    /// `true` iff the workspace is in BYOK mode AND has a stored encrypted
    /// key. Always `false` in Kyomi mode.
    pub has_api_key: bool,
    /// Optional base URL override. Only set in BYOK mode.
    pub base_url: Option<String>,
    /// Remaining balance in the workspace's AI token bundle, in USD
    /// (`ai_bundle_balance_usd - ai_credits_used_usd`, clamped to zero).
    ///
    /// `None` when the balance could not be read (e.g. self-hosted mode, where
    /// this server fn is rejected before reaching this view). Any authenticated
    /// workspace member may see this value — same visibility as the billing
    /// page's balance display.
    pub ai_bundle_balance_usd: Option<f64>,
}

/// Result of a candidate-config connectivity test.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestAiConfigResult {
    /// `true` on any 2xx response from the upstream auth-check endpoint.
    pub ok: bool,
    /// Short human-readable message. `"Connection OK"` on success; a
    /// sanitised error summary on failure. **Never contains the API key.**
    pub message: String,
}

// ---------------------------------------------------------------------------
// Helpers (server-only)
// ---------------------------------------------------------------------------

/// Reject non-admin users. Mirrors `require_workspace_admin` in
/// `server_fns/workspace.rs` — admins include workspace owners.
#[cfg(feature = "ssr")]
fn require_workspace_admin(
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<(), ServerFnError> {
    if !auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
    {
        return Err(ServerFnError::new("Workspace admin access required"));
    }
    Ok(())
}

/// Reject self-hosted deployments. Workspace BYOK is a SaaS-only feature.
#[cfg(feature = "ssr")]
fn require_saas(ctx: &super::ServerContext) -> Result<(), ServerFnError> {
    if ctx.config.self_hosted {
        return Err(ServerFnError::new(
            "Workspace BYOK is only available in SaaS mode.",
        ));
    }
    Ok(())
}

/// Build a [`WorkspaceAiConfigView`] from the authoritative config record.
#[cfg(feature = "ssr")]
fn view_from_config(
    cfg: &kyomi_auth::workspace_ai_config::WorkspaceAiConfig,
    ai_bundle_balance_usd: Option<f64>,
) -> WorkspaceAiConfigView {
    WorkspaceAiConfigView {
        provider: cfg.provider.as_str().to_string(),
        model: cfg.model.clone(),
        has_api_key: cfg.is_byok() && cfg.api_key.is_some(),
        base_url: cfg.base_url.clone(),
        ai_bundle_balance_usd,
    }
}

/// Row shape for reading the AI token bundle balance.
///
/// `ai_bundle_balance_usd` is the total purchased balance and
/// `ai_credits_used_usd` is the amount consumed; the remaining balance shown
/// in the UI is `(balance - used).max(0.0)`. Matches the computation in
/// `server_fns::billing::get_subscription_info`.
#[cfg(feature = "ssr")]
#[derive(sqlx::FromRow)]
struct AiBundleRow {
    ai_bundle_balance_usd: f64,
    ai_credits_used_usd: f64,
}

/// Read the remaining AI token bundle balance for a workspace, in USD.
///
/// Returns `Ok(None)` if the workspace row is missing — callers fall back to
/// omitting the balance clause in the UI rather than pretending it's $0.00.
#[cfg(feature = "ssr")]
async fn load_ai_bundle_remaining_usd(
    db: &kyomi_core::DbPool,
    ws_id: &str,
) -> Result<Option<f64>, ServerFnError> {
    let row = kyomi_core::db_fetch_optional!(
        db,
        AiBundleRow,
        "SELECT \
         COALESCE(ai_bundle_balance_usd, 0) AS ai_bundle_balance_usd, \
         COALESCE(ai_credits_used_usd, 0) AS ai_credits_used_usd \
         FROM workspaces WHERE workspace_id = $1",
        ws_id
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(row.map(|r| (r.ai_bundle_balance_usd - r.ai_credits_used_usd).max(0.0)))
}

// ---------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------

/// Read the current workspace AI configuration. Any authenticated workspace
/// member may call this.
#[server(prefix = "/leptos-api")]
pub async fn get_workspace_ai_config() -> Result<WorkspaceAiConfigView, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_saas(&ctx)?;

    let ws_id = workspace_id(&auth)?;
    let cfg = kyomi_auth::workspace_ai_config::load(&ctx.db, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let balance = load_ai_bundle_remaining_usd(&ctx.db, ws_id).await?;

    Ok(view_from_config(&cfg, balance))
}

// ---------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------

/// Update the workspace AI configuration. Admin/owner only.
///
/// * `provider` — one of `"kyomi" | "anthropic" | "openai" | "gemini"`.
/// * `api_key` — `None` means "don't change the stored key"; `Some(k)` means
///   encrypt and store `k`. Ignored when `provider == "kyomi"` (the stored
///   key is always cleared in that case).
/// * `base_url` — optional base URL override for BYOK providers.
/// * `model` — optional default model name.
#[server(prefix = "/leptos-api")]
pub async fn update_workspace_ai_config(
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
) -> Result<WorkspaceAiConfigView, ServerFnError> {
    use std::str::FromStr;

    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_saas(&ctx)?;
    require_workspace_admin(&auth)?;

    let parsed_provider =
        kyomi_auth::workspace_ai_config::WorkspaceAiProvider::from_str(&provider)
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    let input = kyomi_auth::workspace_ai_config::UpdateWorkspaceAiConfigInput {
        provider: parsed_provider,
        api_key,
        base_url,
        model,
    };

    let ws_id = workspace_id(&auth)?;
    kyomi_auth::workspace_ai_config::update(&ctx.db, ws_id, input)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Re-load so the returned view reflects the committed state.
    let cfg = kyomi_auth::workspace_ai_config::load(&ctx.db, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let balance = load_ai_bundle_remaining_usd(&ctx.db, ws_id).await?;

    Ok(view_from_config(&cfg, balance))
}

// ---------------------------------------------------------------------------
// test
// ---------------------------------------------------------------------------

/// Test a candidate AI configuration without persisting anything. Admin/owner
/// only. Uses free, auth-check endpoints for each provider:
///
/// * **Anthropic** — `GET /v1/models` with `x-api-key` + `anthropic-version`
///   headers.
/// * **OpenAI** — `GET /v1/models` with `Authorization: Bearer <key>`.
/// * **Gemini** — `GET /v1beta/models?key=<key>` (no auth header; key is
///   passed as a query parameter per Google's API convention).
///
/// Never writes to the DB, never logs the key, and never echoes the key into
/// the returned error message.
#[server(prefix = "/leptos-api")]
pub async fn test_workspace_ai_config(
    provider: String,
    api_key: String,
    base_url: Option<String>,
    model: Option<String>,
) -> Result<TestAiConfigResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_saas(&ctx)?;
    require_workspace_admin(&auth)?;

    // `model` is accepted for forward-compat (and to match the update shape)
    // but the free auth-check endpoints don't need it.
    let _ = model;

    let key = api_key.trim();
    if key.is_empty() {
        return Ok(TestAiConfigResult {
            ok: false,
            message: "API key is required.".to_string(),
        });
    }

    let base_url = base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    test_provider(&provider, key, base_url.as_deref()).await
}

// ---------------------------------------------------------------------------
// Free auth-check HTTP helpers (server-only)
// ---------------------------------------------------------------------------

/// Default upstream base URL for each supported provider.
///
/// Kept as a separate function so unit tests can assert URL construction
/// without touching the network.
#[cfg(feature = "ssr")]
fn default_base_url(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("https://api.anthropic.com"),
        "openai" => Some("https://api.openai.com/v1"),
        "gemini" => Some("https://generativelanguage.googleapis.com/v1beta"),
        _ => None,
    }
}

/// Build the free auth-check URL for a provider. Gemini's key goes in the
/// query string; callers are responsible for URL-encoding the key when
/// sending it over the wire — this helper does not embed the key so tests
/// can run offline.
#[cfg(feature = "ssr")]
fn auth_check_url(provider: &str, base_url: Option<&str>) -> Option<String> {
    let base = base_url
        .map(|s| s.trim_end_matches('/').to_string())
        .or_else(|| default_base_url(provider).map(str::to_string))?;

    match provider {
        "anthropic" => Some(format!("{base}/v1/models")),
        "openai" => Some(format!("{base}/models")),
        "gemini" => Some(format!("{base}/models")),
        _ => None,
    }
}

/// Dispatch to the correct upstream provider's auth-check endpoint.
#[cfg(feature = "ssr")]
async fn test_provider(
    provider: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> Result<TestAiConfigResult, ServerFnError> {
    use std::time::Duration;

    let url = match auth_check_url(provider, base_url) {
        Some(u) => u,
        None => {
            return Ok(TestAiConfigResult {
                ok: false,
                message: format!("Unknown provider: {provider}"),
            });
        }
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| ServerFnError::new(format!("Failed to create HTTP client: {e}")))?;

    let req = match provider {
        "anthropic" => client
            .get(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        "openai" => client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}")),
        "gemini" => client.get(&url).query(&[("key", api_key)]),
        _ => {
            return Ok(TestAiConfigResult {
                ok: false,
                message: format!("Unknown provider: {provider}"),
            });
        }
    };

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            // Transport error — stringify the reqwest error directly. reqwest
            // never embeds request bodies or headers into its Display output,
            // so the API key cannot leak here.
            return Ok(TestAiConfigResult {
                ok: false,
                message: sanitise_error(&e.to_string(), api_key),
            });
        }
    };

    let status = resp.status();
    if status.is_success() {
        return Ok(TestAiConfigResult {
            ok: true,
            message: "Connection OK".to_string(),
        });
    }

    let body = match resp.text().await {
        Ok(text) => text,
        Err(e) => {
            return Ok(TestAiConfigResult {
                ok: false,
                message: sanitise_error(
                    &format!("HTTP {status}; failed to read response body: {e}"),
                    api_key,
                ),
            });
        }
    };
    let detail = extract_error_message(&body).unwrap_or_else(|| format!("HTTP {status}"));
    Ok(TestAiConfigResult {
        ok: false,
        message: sanitise_error(&detail, api_key),
    })
}

/// Defence-in-depth: strip the API key from any error string before returning
/// it to the client. Providers should never echo the key back, but we don't
/// trust upstream to be polite with their error messages.
#[cfg(feature = "ssr")]
fn sanitise_error(msg: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        return msg.to_string();
    }
    msg.replace(api_key, "<redacted>")
}

/// Try to extract a human-readable error message from a JSON error response.
///
/// Most AI providers use one of these shapes:
/// * `{"error": {"message": "..."}}` (OpenAI, Anthropic, Gemini)
/// * `{"error": "string"}`
#[cfg(feature = "ssr")]
fn extract_error_message(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;

    if let Some(msg) = parsed
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return Some(msg.to_string());
    }

    if let Some(msg) = parsed.get("error").and_then(|e| e.as_str()) {
        return Some(msg.to_string());
    }

    None
}

// ---------------------------------------------------------------------------
// Extractor re-exports
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;

    // ---- auth_check_url --------------------------------------------------

    #[test]
    fn anthropic_default_url_uses_v1_models() {
        assert_eq!(
            auth_check_url("anthropic", None).as_deref(),
            Some("https://api.anthropic.com/v1/models"),
        );
    }

    #[test]
    fn openai_default_url_uses_models() {
        assert_eq!(
            auth_check_url("openai", None).as_deref(),
            Some("https://api.openai.com/v1/models"),
        );
    }

    #[test]
    fn gemini_default_url_uses_v1beta_models() {
        assert_eq!(
            auth_check_url("gemini", None).as_deref(),
            Some("https://generativelanguage.googleapis.com/v1beta/models"),
        );
    }

    #[test]
    fn base_url_override_strips_trailing_slash() {
        assert_eq!(
            auth_check_url("openai", Some("https://proxy.example.com/v1/")).as_deref(),
            Some("https://proxy.example.com/v1/models"),
        );
    }

    #[test]
    fn base_url_override_works_for_anthropic() {
        assert_eq!(
            auth_check_url("anthropic", Some("https://anthropic.proxy.example.com")).as_deref(),
            Some("https://anthropic.proxy.example.com/v1/models"),
        );
    }

    #[test]
    fn unknown_provider_has_no_url() {
        assert_eq!(auth_check_url("bedrock", None), None);
    }

    // ---- sanitise_error --------------------------------------------------

    #[test]
    fn sanitise_error_redacts_key_when_present() {
        let key = "sk-ant-api03-supersecret";
        let msg = format!("Invalid key supplied: {key}");
        let cleaned = sanitise_error(&msg, key);
        assert!(!cleaned.contains(key), "key leaked: {cleaned}");
        assert!(cleaned.contains("<redacted>"));
    }

    #[test]
    fn sanitise_error_noop_when_key_absent() {
        let cleaned = sanitise_error("HTTP 401", "sk-xyz");
        assert_eq!(cleaned, "HTTP 401");
    }

    #[test]
    fn sanitise_error_ignores_empty_key() {
        // Must not replace every empty substring (which would blow up length).
        let cleaned = sanitise_error("HTTP 401", "");
        assert_eq!(cleaned, "HTTP 401");
    }

    // ---- extract_error_message ------------------------------------------

    #[test]
    fn extract_error_message_reads_openai_shape() {
        let body = r#"{"error": {"message": "Incorrect API key provided", "type": "invalid_request_error"}}"#;
        assert_eq!(
            extract_error_message(body).as_deref(),
            Some("Incorrect API key provided")
        );
    }

    #[test]
    fn extract_error_message_reads_string_shape() {
        let body = r#"{"error": "Unauthorized"}"#;
        assert_eq!(extract_error_message(body).as_deref(), Some("Unauthorized"));
    }

    #[test]
    fn extract_error_message_none_for_non_json() {
        assert_eq!(extract_error_message("<html>500</html>"), None);
    }
}
