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

/// One model entry returned by [`list_workspace_ai_models`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiModelInfo {
    /// Provider-specific model identifier (e.g. `gpt-4o`, `claude-sonnet-4-5-20250929`,
    /// `gemini-2.5-pro`). Sent verbatim back to the provider when issuing requests.
    pub id: String,
    /// Human-readable display label. Falls back to `id` when the provider does
    /// not supply a friendly name.
    pub label: String,
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
// list_workspace_ai_models
// ---------------------------------------------------------------------------

/// List models available from a BYOK provider, filtered to chat-completion
/// capable entries. Admin/owner only. SaaS only.
///
/// API key resolution:
/// * If `api_key` is `Some(non-empty)` after trimming, that candidate key is
///   used (lets the UI refresh the list right after a successful test, before
///   the user has hit Save).
/// * Otherwise the stored config is loaded; if it matches the requested
///   provider AND has a stored key, that key is used.
/// * Otherwise returns `Ok(vec![])` — the UI degrades to the custom-model
///   input. This is a normal pre-save state, not an error.
///
/// `kyomi` is rejected: the curated `KYOMI_CREDITS_MODELS` list is the source
/// of truth for that mode and is not fetched live.
#[server(prefix = "/leptos-api")]
pub async fn list_workspace_ai_models(
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_saas(&ctx)?;
    require_workspace_admin(&auth)?;

    if provider == "kyomi" {
        return Err(ServerFnError::new(
            "Kyomi mode uses a curated model list; live model listing is not supported.",
        ));
    }
    if !matches!(provider.as_str(), "anthropic" | "openai" | "gemini") {
        return Err(ServerFnError::new(format!("Unknown provider: {provider}")));
    }

    // Resolve the candidate key.
    let candidate_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Resolve the candidate key AND stored base_url together: when we fall
    // back to the stored key, we must also honour the workspace's stored
    // base_url override so users with custom proxies (corporate Anthropic
    // proxy, Azure OpenAI, etc.) list models from their configured endpoint
    // rather than silently hitting the default one.
    let (resolved_key, stored_base_url): (String, Option<String>) = match candidate_key {
        Some(k) => (k, None),
        None => {
            let ws_id = workspace_id(&auth)?;
            let cfg = kyomi_auth::workspace_ai_config::load(&ctx.db, ws_id)
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?;
            // Cross-provider guard: never use a stored key intended for
            // provider X to list models from provider Y.
            if cfg.provider.as_str() != provider {
                return Ok(Vec::new());
            }
            let stored_base = cfg.base_url.clone();
            match cfg.api_key {
                Some(k) if !k.trim().is_empty() => (k, stored_base),
                _ => return Ok(Vec::new()),
            }
        }
    };

    // Resolve base URL: caller-supplied override → stored workspace config → default.
    let base = base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
        .or_else(|| {
            stored_base_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_end_matches('/').to_string())
        })
        .or_else(|| default_base_url(&provider).map(str::to_string))
        .ok_or_else(|| ServerFnError::new(format!("No base URL for provider: {provider}")))?;

    fetch_provider_models(&provider, &resolved_key, &base).await
}

/// HTTP fetch + parse dispatch for the three live-listing providers.
#[cfg(feature = "ssr")]
async fn fetch_provider_models(
    provider: &str,
    api_key: &str,
    base: &str,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    use std::time::Duration;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| ServerFnError::new(format!("Failed to create HTTP client: {e}")))?;

    let url = match provider {
        "anthropic" => format!("{base}/v1/models"),
        "openai" => format!("{base}/models"),
        "gemini" => format!("{base}/models"),
        _ => return Err(ServerFnError::new(format!("Unknown provider: {provider}"))),
    };

    let req = match provider {
        "anthropic" => client
            .get(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        "openai" => client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}")),
        "gemini" => client
            .get(&url)
            .query(&[("key", api_key), ("pageSize", "1000")]),
        _ => unreachable!(),
    };

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return Err(ServerFnError::new(sanitise_error(&e.to_string(), api_key)));
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let detail = extract_error_message(&body).unwrap_or_else(|| format!("HTTP {status}"));
        return Err(ServerFnError::new(sanitise_error(&detail, api_key)));
    }

    let body = resp
        .text()
        .await
        .map_err(|e| ServerFnError::new(sanitise_error(&e.to_string(), api_key)))?;

    let parsed = match provider {
        "anthropic" => parse_anthropic_models(&body),
        "openai" => parse_openai_models(&body),
        "gemini" => parse_gemini_models(&body),
        _ => unreachable!(),
    };

    parsed.map_err(|e| {
        ServerFnError::new(sanitise_error(
            &format!("Failed to parse provider response: {e}"),
            api_key,
        ))
    })
}

// ---------------------------------------------------------------------------
// Provider parsers (pure CPU — unit-testable without network)
// ---------------------------------------------------------------------------

/// Parse Anthropic `GET /v1/models` response.
///
/// Shape: `{"data":[{"id":"claude-...","display_name":"Claude ...","type":"model","created_at":"2024-..."}]}`.
/// All entries with `type == "model"` are kept; label prefers `display_name`,
/// sorted by `created_at` descending (newest first).
#[cfg(feature = "ssr")]
fn parse_anthropic_models(body: &str) -> Result<Vec<AiModelInfo>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default, rename = "type")]
        kind: Option<String>,
        #[serde(default)]
        created_at: Option<String>,
    }

    let parsed: Resp = serde_json::from_str(body)?;
    let mut entries: Vec<(Option<String>, AiModelInfo)> = parsed
        .data
        .into_iter()
        .filter(|e| e.kind.as_deref().unwrap_or("model") == "model")
        .map(|e| {
            let label = e.display_name.clone().unwrap_or_else(|| e.id.clone());
            (
                e.created_at,
                AiModelInfo {
                    id: e.id,
                    label,
                },
            )
        })
        .collect();

    // Sort by created_at desc; entries missing created_at fall to the end and
    // sort reverse-alphabetically among themselves.
    entries.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => y.cmp(x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.1.id.cmp(&a.1.id),
    });

    Ok(entries.into_iter().map(|(_, m)| m).collect())
}

/// Parse OpenAI `GET /v1/models` response.
///
/// Shape: `{"data":[{"id":"gpt-4o","created":1234567890,"object":"model"}]}`.
/// Filters to chat-completion-capable models (allowlist of id prefixes minus
/// a denylist of substrings for non-chat variants like embeddings, audio,
/// vision-only, image, moderation, etc.). Sorted by `created` desc.
#[cfg(feature = "ssr")]
fn parse_openai_models(body: &str) -> Result<Vec<AiModelInfo>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        #[serde(default)]
        created: Option<i64>,
    }

    let parsed: Resp = serde_json::from_str(body)?;

    const ALLOWED_PREFIXES: &[&str] = &["gpt-", "chatgpt-", "o1", "o3", "o4"];
    const DENY_SUBSTRINGS: &[&str] = &[
        "-instruct",
        "-audio",
        "-realtime",
        "-tts",
        "-transcribe",
        "-search",
        "-image",
        "embedding",
        "whisper",
        "dall-e",
        "tts-",
        "moderation",
        "babbage",
        "davinci",
        "-vision-preview",
    ];

    let mut entries: Vec<(Option<i64>, AiModelInfo)> = parsed
        .data
        .into_iter()
        .filter(|e| {
            let id = &e.id;
            ALLOWED_PREFIXES.iter().any(|p| id.starts_with(p))
                && !DENY_SUBSTRINGS.iter().any(|s| id.contains(s))
        })
        .map(|e| {
            (
                e.created,
                AiModelInfo {
                    label: e.id.clone(),
                    id: e.id,
                },
            )
        })
        .collect();

    entries.sort_by(|a, b| match (a.0, b.0) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.1.id.cmp(&a.1.id),
    });

    Ok(entries.into_iter().map(|(_, m)| m).collect())
}

/// Parse Gemini `GET /v1beta/models` response.
///
/// Shape: `{"models":[{"name":"models/gemini-1.5-pro","displayName":"Gemini 1.5 Pro","supportedGenerationMethods":["generateContent"]}]}`.
/// Requires `supportedGenerationMethods` to contain `generateContent`. Strips
/// the `models/` prefix from the id. Sorted reverse-alphabetically by id so
/// newer model families surface first (`gemini-2.5-pro` > `gemini-1.5-pro`).
#[cfg(feature = "ssr")]
fn parse_gemini_models(body: &str) -> Result<Vec<AiModelInfo>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        models: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        name: String,
        #[serde(default, rename = "displayName")]
        display_name: Option<String>,
        #[serde(default, rename = "supportedGenerationMethods")]
        supported_generation_methods: Vec<String>,
    }

    let parsed: Resp = serde_json::from_str(body)?;

    const DENY_SUBSTRINGS: &[&str] = &[
        "embedding",
        "aqa",
        "imagen",
        "text-bison",
        "chat-bison",
        "-tuning",
    ];

    let mut entries: Vec<AiModelInfo> = parsed
        .models
        .into_iter()
        .filter(|e| {
            e.supported_generation_methods
                .iter()
                .any(|m| m == "generateContent")
        })
        .filter_map(|e| {
            let id = e.name.strip_prefix("models/").unwrap_or(&e.name).to_string();
            if DENY_SUBSTRINGS.iter().any(|s| id.contains(s)) {
                return None;
            }
            let label = e.display_name.unwrap_or_else(|| id.clone());
            Some(AiModelInfo { id, label })
        })
        .collect();

    entries.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(entries)
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

    // ---- parse_anthropic_models -----------------------------------------

    #[test]
    fn parse_anthropic_models_keeps_all_and_uses_display_name() {
        let body = r#"{
            "data": [
                {"id":"claude-sonnet-4-5-20250929","display_name":"Claude Sonnet 4.5","type":"model","created_at":"2025-09-29T00:00:00Z"},
                {"id":"claude-haiku-4-5-20251001","display_name":"Claude Haiku 4.5","type":"model","created_at":"2025-10-01T00:00:00Z"}
            ]
        }"#;
        let parsed = parse_anthropic_models(body).unwrap();
        assert_eq!(parsed.len(), 2);
        // Newest first (haiku 2025-10-01 > sonnet 2025-09-29)
        assert_eq!(parsed[0].id, "claude-haiku-4-5-20251001");
        assert_eq!(parsed[0].label, "Claude Haiku 4.5");
        assert_eq!(parsed[1].id, "claude-sonnet-4-5-20250929");
        assert_eq!(parsed[1].label, "Claude Sonnet 4.5");
    }

    #[test]
    fn parse_anthropic_models_sorts_by_created_at_desc() {
        let body = r#"{
            "data": [
                {"id":"claude-old","display_name":"Old","type":"model","created_at":"2023-01-01T00:00:00Z"},
                {"id":"claude-new","display_name":"New","type":"model","created_at":"2025-01-01T00:00:00Z"},
                {"id":"claude-mid","display_name":"Mid","type":"model","created_at":"2024-06-01T00:00:00Z"}
            ]
        }"#;
        let parsed = parse_anthropic_models(body).unwrap();
        assert_eq!(
            parsed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["claude-new", "claude-mid", "claude-old"],
        );
    }

    #[test]
    fn parse_anthropic_models_falls_back_to_id_when_display_name_missing() {
        let body = r#"{
            "data": [
                {"id":"claude-bare","type":"model"}
            ]
        }"#;
        let parsed = parse_anthropic_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].label, "claude-bare");
    }

    // ---- parse_openai_models --------------------------------------------

    #[test]
    fn parse_openai_models_excludes_embeddings_and_whisper() {
        let body = r#"{
            "data": [
                {"id":"gpt-4o","created":1715000000,"object":"model"},
                {"id":"text-embedding-3-large","created":1700000000,"object":"model"},
                {"id":"whisper-1","created":1690000000,"object":"model"},
                {"id":"dall-e-3","created":1695000000,"object":"model"},
                {"id":"tts-1","created":1690000000,"object":"model"},
                {"id":"omni-moderation-latest","created":1690000000,"object":"model"},
                {"id":"babbage-002","created":1690000000,"object":"model"},
                {"id":"davinci-002","created":1690000000,"object":"model"},
                {"id":"gpt-3.5-turbo-instruct","created":1690000000,"object":"model"}
            ]
        }"#;
        let parsed = parse_openai_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "gpt-4o");
    }

    #[test]
    fn parse_openai_models_keeps_gpt4o_and_o1() {
        let body = r#"{
            "data": [
                {"id":"gpt-4o","created":1715000000,"object":"model"},
                {"id":"gpt-4o-mini","created":1716000000,"object":"model"},
                {"id":"o1-preview","created":1720000000,"object":"model"},
                {"id":"o3-mini","created":1735000000,"object":"model"},
                {"id":"chatgpt-4o-latest","created":1730000000,"object":"model"}
            ]
        }"#;
        let parsed = parse_openai_models(body).unwrap();
        let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
        // Sorted by created desc.
        assert_eq!(
            ids,
            vec![
                "o3-mini",
                "chatgpt-4o-latest",
                "o1-preview",
                "gpt-4o-mini",
                "gpt-4o",
            ]
        );
    }

    #[test]
    fn parse_openai_models_excludes_audio_and_realtime_variants() {
        let body = r#"{
            "data": [
                {"id":"gpt-4o-audio-preview","created":1720000000,"object":"model"},
                {"id":"gpt-4o-realtime-preview","created":1720000000,"object":"model"},
                {"id":"gpt-4o-search-preview","created":1720000000,"object":"model"},
                {"id":"gpt-4o","created":1715000000,"object":"model"}
            ]
        }"#;
        let parsed = parse_openai_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "gpt-4o");
    }

    // ---- parse_gemini_models --------------------------------------------

    #[test]
    fn parse_gemini_models_requires_generate_content() {
        let body = r#"{
            "models": [
                {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro","supportedGenerationMethods":["generateContent","countTokens"]},
                {"name":"models/text-embedding-004","displayName":"Text Embedding 004","supportedGenerationMethods":["embedContent"]}
            ]
        }"#;
        let parsed = parse_gemini_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "gemini-2.5-pro");
        assert_eq!(parsed[0].label, "Gemini 2.5 Pro");
    }

    #[test]
    fn parse_gemini_models_excludes_embeddings() {
        let body = r#"{
            "models": [
                {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro","supportedGenerationMethods":["generateContent"]},
                {"name":"models/gemini-embedding-001","displayName":"Gemini Embedding","supportedGenerationMethods":["generateContent"]},
                {"name":"models/aqa","displayName":"Attributed QA","supportedGenerationMethods":["generateContent"]},
                {"name":"models/imagen-3.0","displayName":"Imagen","supportedGenerationMethods":["generateContent"]}
            ]
        }"#;
        let parsed = parse_gemini_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "gemini-2.5-pro");
    }

    #[test]
    fn parse_gemini_models_sorts_reverse_alpha() {
        let body = r#"{
            "models": [
                {"name":"models/gemini-1.5-pro","displayName":"Gemini 1.5 Pro","supportedGenerationMethods":["generateContent"]},
                {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro","supportedGenerationMethods":["generateContent"]},
                {"name":"models/gemini-2.0-flash","displayName":"Gemini 2.0 Flash","supportedGenerationMethods":["generateContent"]}
            ]
        }"#;
        let parsed = parse_gemini_models(body).unwrap();
        assert_eq!(
            parsed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["gemini-2.5-pro", "gemini-2.0-flash", "gemini-1.5-pro"],
        );
    }
}
