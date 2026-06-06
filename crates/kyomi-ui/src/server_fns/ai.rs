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
    /// Optional model used specifically for session title generation
    /// (from `settings.custom_settings.title_model`).
    ///
    /// When `None`, title generation falls back to the cheapest model for the
    /// configured provider. When `Some`, that model is used verbatim.
    #[serde(default)]
    pub title_model: Option<String>,
}

/// One model entry returned by model-listing functions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AiModelInfo {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cost_per_token: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_cost_per_token: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
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
    title_model: Option<String>,
) -> WorkspaceAiConfigView {
    WorkspaceAiConfigView {
        provider: cfg.provider.as_str().to_string(),
        model: cfg.model.clone(),
        has_api_key: cfg.is_byok() && cfg.api_key.is_some(),
        base_url: cfg.base_url.clone(),
        ai_bundle_balance_usd,
        title_model,
    }
}

/// Read the live remaining AI token bundle balance for a workspace, in USD.
///
/// Delegates to
/// [`kyomi_auth::billing_service::BillingService::get_bundle_remaining_usd`]
/// which computes the value from authoritative live usage records rather than
/// the stale `ai_credits_used_usd` cache column. Returns `Ok(None)` when the
/// billing service errors (e.g. workspace missing) so the UI omits the balance
/// clause rather than surfacing "$0.00".
#[cfg(feature = "ssr")]
async fn load_ai_bundle_remaining_usd(
    db: &kyomi_core::DbPool,
    ws_id: &str,
) -> Result<Option<f64>, ServerFnError> {
    let service = kyomi_auth::billing_service::BillingService::new();
    Ok(service.get_bundle_remaining_usd(db, ws_id).await.ok())
}

// ---------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------

/// Read the current workspace AI configuration. Any authenticated workspace
/// member may call this.
#[server(prefix = "/leptos-api")]
pub async fn get_workspace_ai_config() -> Result<WorkspaceAiConfigView, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_saas(&ac.ctx)?;

    let cfg = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    let balance = load_ai_bundle_remaining_usd(ac.db(), &ac.ws_id).await?;

    Ok(view_from_config(&cfg, balance, cfg.title_model.clone()))
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

    let ac = AuthenticatedContext::extract().await?;
    require_saas(&ac.ctx)?;
    require_workspace_admin(&ac.auth)?;

    let parsed_provider =
        kyomi_auth::workspace_ai_config::WorkspaceAiProvider::from_str(&provider)
            .into_sfn()?;

    let input = kyomi_auth::workspace_ai_config::UpdateWorkspaceAiConfigInput {
        provider: parsed_provider,
        api_key,
        base_url,
        model,
    };

    kyomi_auth::workspace_ai_config::update(ac.db(), &ac.ws_id, input)
        .await
        .into_sfn()?;

    // Re-load so the returned view reflects the committed state.
    let cfg = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;
    let balance = load_ai_bundle_remaining_usd(ac.db(), &ac.ws_id).await?;

    Ok(view_from_config(&cfg, balance, cfg.title_model.clone()))
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
/// `kyomi` is rejected: Kyomi-credits mode uses the OpenRouter model list
/// (via [`list_openrouter_models`]) rather than a per-provider live fetch.
#[server(prefix = "/leptos-api")]
pub async fn list_workspace_ai_models(
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_saas(&ac.ctx)?;
    require_workspace_admin(&ac.auth)?;

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
            let cfg = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
                .await
                .into_sfn()?;
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

/// Fetch the raw model-list response body from a provider's API.
///
/// Shared by both `fetch_provider_models` (BYOK) and
/// `fetch_openrouter_models_live` (Kyomi credits). Each caller parses the
/// body with its own parser to produce the appropriate return type.
#[cfg(feature = "ssr")]
async fn fetch_models_raw(
    provider: &str,
    api_key: &str,
    base: &str,
) -> Result<String, ServerFnError> {
    use std::time::Duration;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
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

    resp.text()
        .await
        .map_err(|e| ServerFnError::new(sanitise_error(&e.to_string(), api_key)))
}

/// Fetch and parse the model list for a BYOK provider.
#[cfg(feature = "ssr")]
async fn fetch_provider_models(
    provider: &str,
    api_key: &str,
    base: &str,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    let body = fetch_models_raw(provider, api_key, base).await?;

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
                    ..Default::default()
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

/// Parse an OpenAI-compatible `GET /v1/models` response.
///
/// Shape: `{"data":[{"id":"gpt-4o","created":1234567890,"object":"model"}]}`.
/// Returns all models from the endpoint — no filtering. Works for OpenAI,
/// OpenRouter, vLLM, Ollama, and any other OpenAI-compatible API.
/// Sorted by `created` desc, then alphabetically by id.
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
        #[serde(default)]
        context_length: Option<u64>,
    }

    let parsed: Resp = serde_json::from_str(body)?;

    let mut entries: Vec<(Option<i64>, AiModelInfo)> = parsed
        .data
        .into_iter()
        .map(|e| {
            (
                e.created,
                AiModelInfo {
                    label: e.id.clone(),
                    id: e.id,
                    context_length: e.context_length,
                    ..Default::default()
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
            Some(AiModelInfo { id, label, ..Default::default() })
        })
        .collect();

    entries.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(entries)
}

// ---------------------------------------------------------------------------
// list_openrouter_models
// ---------------------------------------------------------------------------

/// List models available from OpenRouter.
///
/// Resolves credentials in priority order:
/// 1. Workspace BYOK config — used when the workspace has its own OpenRouter
///    base URL (`base_url` containing `"openrouter.ai"`) and stored API key.
/// 2. Server-level env config — used for Kyomi-credits mode workspaces where
///    the LLM key is supplied via the `LLM_API_KEY` / `LLM_BASE_URL` env
///    variables.
///
/// Returns an empty list when neither source provides a valid OpenRouter
/// configuration so the caller can degrade gracefully (e.g. show a plain
/// text input instead of a dropdown).
///
/// Results are cached per-process for one hour to avoid hammering the
/// OpenRouter API on every settings page load. The cache is invalidated
/// whenever `force_refresh = true` is passed (e.g. after the user changes
/// their API key).
///
/// Unlike [`list_workspace_ai_models`] this function is not restricted to
/// SaaS mode — OpenRouter works in self-hosted deployments too.
#[server(prefix = "/leptos-api")]
pub async fn list_openrouter_models(
    force_refresh: bool,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let cfg = kyomi_auth::workspace_ai_config::load(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?;

    // Resolution order:
    // 1. Workspace BYOK config — used when the workspace has its own OpenRouter key.
    // 2. Server-level env config — used for Kyomi-mode workspaces where the LLM
    //    key lives in `LLM_API_KEY` / `LLM_BASE_URL` env vars.
    //
    // Neither path proceeds unless the resolved base URL points at openrouter.ai,
    // ensuring we don't call the OpenRouter model list against non-OpenRouter keys.

    // Check workspace BYOK config first.
    let workspace_base_url = cfg.base_url.as_deref().unwrap_or("");
    if workspace_base_url.contains("openrouter.ai") {
        let api_key = match cfg.api_key {
            Some(k) if !k.trim().is_empty() => k,
            _ => return Ok(Vec::new()),
        };
        return fetch_openrouter_models_cached(&api_key, force_refresh).await;
    }

    // Fall back to server-level env config for Kyomi-mode workspaces.
    let server_base_url = ac.ctx.config.llm_base_url.as_deref().unwrap_or("");
    if server_base_url.contains("openrouter.ai") {
        let api_key = match ac.ctx.config.llm_api_key.as_deref() {
            Some(k) if !k.trim().is_empty() => k.to_string(),
            _ => return Ok(Vec::new()),
        };
        return fetch_openrouter_models_cached(&api_key, force_refresh).await;
    }

    Ok(Vec::new())
}

/// Compute a short fingerprint for an API key used as the per-workspace cache
/// key.  Uses [`std::collections::hash_map::DefaultHasher`] so no external
/// crate is required.  The full key is never stored — only the 16-hex-digit
/// hash.
#[cfg(feature = "ssr")]
fn cache_key_for_api_key(api_key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    api_key.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Fetch OpenRouter models, serving from cache when the cached value is fresh.
///
/// Cache TTL: 1 hour.  The cache is a `HashMap` keyed by a fingerprint of the
/// workspace's API key so that distinct workspaces (each with their own
/// OpenRouter key) never receive another workspace's cached model list.
#[cfg(feature = "ssr")]
async fn fetch_openrouter_models_cached(
    api_key: &str,
    force_refresh: bool,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    struct CacheEntry {
        models: Vec<AiModelInfo>,
        fetched_at: Instant,
    }

    static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    const TTL: Duration = Duration::from_secs(3600);

    let key = cache_key_for_api_key(api_key);

    // Check whether we can serve from cache.
    if !force_refresh {
        let guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = guard.get(&key)
            && entry.fetched_at.elapsed() < TTL
        {
            return Ok(entry.models.clone());
        }
    }

    // Cache miss or forced refresh — fetch from upstream.
    let models = fetch_openrouter_models_live(api_key).await?;

    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    guard.insert(
        key,
        CacheEntry {
            models: models.clone(),
            fetched_at: Instant::now(),
        },
    );

    Ok(models)
}

/// Fetch and parse OpenRouter models. Uses the shared `fetch_models_raw`
/// (OpenRouter is OpenAI-compatible) and applies the OpenRouter-specific
/// parser that extracts pricing and context length.
#[cfg(feature = "ssr")]
async fn fetch_openrouter_models_live(
    api_key: &str,
) -> Result<Vec<AiModelInfo>, ServerFnError> {
    let body =
        fetch_models_raw("openai", api_key, "https://openrouter.ai/api/v1").await?;

    parse_openrouter_models(&body)
        .map_err(|e| ServerFnError::new(format!("Failed to parse OpenRouter response: {e}")))
}

/// Parse `GET https://openrouter.ai/api/v1/models` response.
///
/// Shape:
/// ```json
/// {
///   "data": [
///     {
///       "id": "openai/gpt-4o",
///       "name": "GPT-4o",
///       "pricing": { "prompt": "0.0000025", "completion": "0.00001" },
///       "context_length": 128000
///     }
///   ]
/// }
/// ```
///
/// Pricing strings are per-token USD amounts. We parse them as `f64` and
/// default to `0.0` on parse failure so a single malformed entry does not
/// discard the whole list.
#[cfg(feature = "ssr")]
fn parse_openrouter_models(body: &str) -> Result<Vec<AiModelInfo>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        pricing: Option<Pricing>,
        #[serde(default)]
        context_length: Option<u64>,
    }
    #[derive(Deserialize)]
    struct Pricing {
        #[serde(default)]
        prompt: Option<serde_json::Value>,
        #[serde(default)]
        completion: Option<serde_json::Value>,
    }

    let parsed: Resp = serde_json::from_str(body)?;

    let mut models: Vec<AiModelInfo> = parsed
        .data
        .into_iter()
        .map(|e| {
            let name = e.name.unwrap_or_else(|| e.id.clone());
            let (prompt_cost, completion_cost) = e
                .pricing
                .map(|p| {
                    (
                        parse_cost_value(p.prompt.as_ref()),
                        parse_cost_value(p.completion.as_ref()),
                    )
                })
                .unwrap_or((0.0, 0.0));
            AiModelInfo {
                id: e.id,
                label: name,
                prompt_cost_per_token: Some(prompt_cost),
                completion_cost_per_token: Some(completion_cost),
                context_length: e.context_length,
            }
        })
        .collect();

    // Sort alphabetically by id so the list is stable and easy to scan.
    models.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(models)
}

/// Parse an OpenRouter pricing value, which may be a JSON string (`"0.0000025"`)
/// or a JSON number (`0.0000025`).  Returns `0.0` on any parse failure.
#[cfg(feature = "ssr")]
fn parse_cost_value(v: Option<&serde_json::Value>) -> f64 {
    match v {
        Some(serde_json::Value::String(s)) => s.parse::<f64>().unwrap_or(0.0),
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Extractor re-exports
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, AuthenticatedContext, IntoServerFnError};

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

    // ---- parse_openrouter_models ----------------------------------------

    #[test]
    fn parse_openrouter_models_extracts_id_name_pricing() {
        let body = r#"{
            "data": [
                {
                    "id": "openai/gpt-4o",
                    "name": "GPT-4o",
                    "pricing": { "prompt": "0.0000025", "completion": "0.00001" },
                    "context_length": 128000
                },
                {
                    "id": "anthropic/claude-sonnet-4-5",
                    "name": "Claude Sonnet 4.5",
                    "pricing": { "prompt": "0.000003", "completion": "0.000015" },
                    "context_length": 200000
                }
            ]
        }"#;
        let parsed = parse_openrouter_models(body).unwrap();
        assert_eq!(parsed.len(), 2);
        // Sorted alphabetically by id.
        assert_eq!(parsed[0].id, "anthropic/claude-sonnet-4-5");
        assert_eq!(parsed[0].name, "Claude Sonnet 4.5");
        assert!((parsed[0].prompt_cost_per_token - 0.000003).abs() < 1e-10);
        assert!((parsed[0].completion_cost_per_token - 0.000015).abs() < 1e-10);
        assert_eq!(parsed[0].context_length, 200000);
        assert_eq!(parsed[1].id, "openai/gpt-4o");
    }

    #[test]
    fn parse_openrouter_models_handles_missing_name() {
        let body = r#"{
            "data": [
                {
                    "id": "some/model",
                    "context_length": 4096
                }
            ]
        }"#;
        let parsed = parse_openrouter_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        // Name falls back to id when absent.
        assert_eq!(parsed[0].name, "some/model");
        assert_eq!(parsed[0].prompt_cost_per_token, 0.0);
        assert_eq!(parsed[0].completion_cost_per_token, 0.0);
    }

    #[test]
    fn parse_openrouter_models_handles_numeric_pricing() {
        // Some OpenRouter entries return pricing as JSON numbers, not strings.
        let body = r#"{
            "data": [
                {
                    "id": "free/model",
                    "name": "Free Model",
                    "pricing": { "prompt": 0.0, "completion": 0.0 },
                    "context_length": 8192
                }
            ]
        }"#;
        let parsed = parse_openrouter_models(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].prompt_cost_per_token, 0.0);
    }

    #[test]
    fn parse_cost_value_handles_string_and_number() {
        assert!((parse_cost_value(Some(&serde_json::json!("0.0000025"))) - 0.0000025).abs() < 1e-15);
        assert!((parse_cost_value(Some(&serde_json::json!(0.0000025))) - 0.0000025).abs() < 1e-15);
        assert_eq!(parse_cost_value(None), 0.0);
        assert_eq!(parse_cost_value(Some(&serde_json::json!("not-a-number"))), 0.0);
    }
}
