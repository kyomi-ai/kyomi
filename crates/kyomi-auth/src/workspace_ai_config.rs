// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace AI configuration service.
//!
//! Loads and updates per-workspace LLM provider settings. Every workspace
//! selects one of:
//!
//! * [`WorkspaceAiProvider::Kyomi`] — use Kyomi's server-side keys. The
//!   workspace has no stored API key; usage is metered against
//!   `ai_credits_used_usd` / `ai_bundle_balance_usd`.
//! * [`WorkspaceAiProvider::Anthropic`] /
//!   [`WorkspaceAiProvider::OpenAI`] /
//!   [`WorkspaceAiProvider::Gemini`] — BYOK. The workspace provides its own
//!   API key (AES-GCM encrypted at rest via [`crate::workspace_secrets`]) and
//!   an optional base URL for proxies / compatible endpoints. BYOK usage is
//!   NOT debited from Kyomi credits.
//!
//! The default model (`model`) is stored under
//! `workspaces.settings.custom_settings.default_model`, matching the existing
//! model-settings endpoint in `apps/server/src/routes/workspaces.rs`.

use std::str::FromStr;

use kyomi_core::DbPool;

use crate::workspace_secrets::{self, WorkspaceSecretError};

// ---------------------------------------------------------------------------
// Provider enum
// ---------------------------------------------------------------------------

/// LLM provider selection for a workspace.
///
/// Serialised lowercase to match the `workspaces.ai_provider` CHECK constraint
/// and the Leptos `settings/ai_provider.rs` form values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceAiProvider {
    /// Use Kyomi's managed keys (default for new workspaces).
    Kyomi,
    /// Workspace-owned Anthropic API key.
    Anthropic,
    /// Workspace-owned OpenAI API key.
    OpenAI,
    /// Workspace-owned Google Gemini API key.
    Gemini,
}

impl WorkspaceAiProvider {
    /// Return the canonical string form stored in the DB (`ai_provider`
    /// column). Matches the CHECK constraint values exactly.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Kyomi => "kyomi",
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
            Self::Gemini => "gemini",
        }
    }

}

impl std::str::FromStr for WorkspaceAiProvider {
    type Err = WorkspaceAiConfigError;

    /// Parse a provider string. Matching is case-sensitive (values come from
    /// the DB or a typed form, never from user free-text).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "kyomi" => Ok(Self::Kyomi),
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAI),
            "gemini" => Ok(Self::Gemini),
            other => Err(WorkspaceAiConfigError::InvalidProvider(other.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Config value + update input
// ---------------------------------------------------------------------------

/// Decrypted, ready-to-use workspace AI configuration.
#[derive(Clone, Debug)]
pub struct WorkspaceAiConfig {
    /// Which provider to route LLM calls to.
    pub provider: WorkspaceAiProvider,
    /// Workspace default model name, e.g. `"claude-sonnet-4-20250514"`.
    pub model: Option<String>,
    /// Plaintext API key — `None` when `provider == Kyomi`, `Some` otherwise
    /// (assuming the workspace has been configured; BYOK without a stored key
    /// is rejected at write time).
    pub api_key: Option<String>,
    /// Optional base URL override (for proxies / compatible endpoints).
    pub base_url: Option<String>,
    /// Optional model used specifically for session title generation
    /// (from `settings.custom_settings.title_model`).
    ///
    /// When `None`, title generation falls back to the cheapest model for the
    /// configured provider.
    pub title_model: Option<String>,
    /// Context window size for the configured model in tokens (0 = unknown).
    /// Stored in `settings.custom_settings.context_window`.
    pub context_window: u32,
}

impl WorkspaceAiConfig {
    /// `true` when the workspace uses its own provider credentials.
    pub fn is_byok(&self) -> bool {
        self.provider != WorkspaceAiProvider::Kyomi
    }
}

/// Input for [`update`]. All fields are applied atomically within one DB
/// transaction.
#[derive(Clone, Debug)]
pub struct UpdateWorkspaceAiConfigInput {
    /// Target provider.
    pub provider: WorkspaceAiProvider,
    /// Plaintext API key to encrypt and store. For BYOK updates that only
    /// change the model / base URL, leave this `None` — the existing
    /// encrypted key is preserved.
    pub api_key: Option<String>,
    /// Base URL override. `None` means leave unchanged in BYOK mode; in Kyomi
    /// mode the base URL is always cleared regardless.
    pub base_url: Option<String>,
    /// Default model name. When `Some`, written to
    /// `settings.custom_settings.default_model`.
    pub model: Option<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by this module.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceAiConfigError {
    /// `ai_provider` column held a value not in the CHECK constraint set.
    /// Should never happen for rows written through this module; indicates
    /// direct DB tampering or an out-of-sync migration.
    #[error("invalid AI provider value: {0}")]
    InvalidProvider(String),

    /// Caller requested BYOK mode but supplied no new key and the workspace
    /// has no existing encrypted key to fall back to.
    #[error(
        "BYOK provider {provider} requires an API key — none supplied and no stored key exists"
    )]
    MissingApiKey { provider: &'static str },

    /// Workspace row does not exist.
    #[error("workspace {0} not found")]
    WorkspaceNotFound(String),

    /// `WORKSPACE_SECRETS_KEY` is not configured — BYOK is disabled in
    /// self-hosted mode without this env var. Kyomi-mode loads/updates still
    /// succeed.
    #[error("workspace secrets encryption is unavailable — BYOK is disabled")]
    EncryptionUnavailable,

    /// Wraps an encryption / decryption failure from
    /// [`crate::workspace_secrets`].
    #[error(transparent)]
    Secret(#[from] WorkspaceSecretError),

    /// Wraps a serde_json failure serialising the settings JSON blob.
    #[error("settings JSON serialisation failed: {0}")]
    Json(String),

    /// Wraps a sqlx error from the underlying DB call.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

// ---------------------------------------------------------------------------
// Row type — matches the subset of `workspaces` columns this module reads.
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct WorkspaceAiRow {
    ai_provider: String,
    ai_api_key_encrypted: Option<String>,
    ai_base_url: Option<String>,
    settings: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

/// Load the AI configuration for a workspace.
///
/// * For Kyomi mode, `api_key` is always `None` even if a stale encrypted blob
///   exists in the column (the update path clears it, but we defend against
///   direct DB mutation).
/// * For BYOK mode, the stored key is decrypted via
///   [`crate::workspace_secrets::decrypt_secret`]. If encryption is
///   unavailable ([`workspace_secrets::is_available`] is false), returns
///   [`WorkspaceAiConfigError::EncryptionUnavailable`] — BYOK cannot operate
///   without the master key.
pub async fn load(
    db: &DbPool,
    workspace_id: &str,
) -> Result<WorkspaceAiConfig, WorkspaceAiConfigError> {
    let row = kyomi_core::db_fetch_optional!(
        db,
        WorkspaceAiRow,
        "SELECT ai_provider, ai_api_key_encrypted, ai_base_url, settings \
         FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?
    .ok_or_else(|| WorkspaceAiConfigError::WorkspaceNotFound(workspace_id.to_string()))?;

    let provider = WorkspaceAiProvider::from_str(&row.ai_provider)?;
    let model = read_default_model(&row.settings);
    let title_model = read_title_model(&row.settings);

    let (api_key, base_url) = match provider {
        WorkspaceAiProvider::Kyomi => {
            // Kyomi mode never exposes a key, even if one was left behind.
            (None, None)
        }
        _ => {
            if !workspace_secrets::is_available() {
                return Err(WorkspaceAiConfigError::EncryptionUnavailable);
            }
            let api_key = match row.ai_api_key_encrypted.as_deref() {
                Some(ct) => Some(workspace_secrets::decrypt_secret(ct)?),
                None => None,
            };
            (api_key, row.ai_base_url)
        }
    };

    let context_window = read_context_window(&row.settings);

    Ok(WorkspaceAiConfig {
        provider,
        model,
        api_key,
        base_url,
        title_model,
        context_window,
    })
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

/// Apply a configuration update atomically.
///
/// Semantics:
///
/// * **Kyomi mode**: clears `ai_api_key_encrypted` and `ai_base_url`. If
///   `input.model` is `Some`, also writes
///   `settings.custom_settings.default_model`.
/// * **BYOK mode**: requires either a new `input.api_key` OR a pre-existing
///   non-null `ai_api_key_encrypted` on the row (model-only updates are
///   allowed without re-entering the key). Returns
///   [`WorkspaceAiConfigError::MissingApiKey`] otherwise.
///   * When a new key is provided, it is encrypted via
///     [`crate::workspace_secrets::encrypt_secret`] before storage. Plaintext
///     is never written to the DB.
///   * `input.base_url` is applied when `Some`; `None` leaves the existing
///     value unchanged.
///
/// All column writes (provider, key, base URL, settings JSON) run inside one
/// transaction so observers never see a half-applied update.
pub async fn update(
    db: &DbPool,
    workspace_id: &str,
    input: UpdateWorkspaceAiConfigInput,
) -> Result<(), WorkspaceAiConfigError> {
    // Fetch current row first so we can validate "has existing key" and merge
    // into the settings JSON without dropping other keys.
    let row = kyomi_core::db_fetch_optional!(
        db,
        WorkspaceAiRow,
        "SELECT ai_provider, ai_api_key_encrypted, ai_base_url, settings \
         FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?
    .ok_or_else(|| WorkspaceAiConfigError::WorkspaceNotFound(workspace_id.to_string()))?;

    // Compute the new column values.
    let (new_encrypted_key, new_base_url): (Option<String>, Option<String>) = match input.provider {
        WorkspaceAiProvider::Kyomi => {
            // Always clear the encrypted key + base URL when switching to Kyomi.
            (None, None)
        }
        _ => {
            // BYOK validation + key preservation.
            let has_existing = row.ai_api_key_encrypted.is_some();
            let encrypted = match input.api_key.as_deref() {
                Some(plaintext) => {
                    if !workspace_secrets::is_available() {
                        return Err(WorkspaceAiConfigError::EncryptionUnavailable);
                    }
                    Some(workspace_secrets::encrypt_secret(plaintext)?)
                }
                None => {
                    if !has_existing {
                        return Err(WorkspaceAiConfigError::MissingApiKey {
                            provider: input.provider.as_str(),
                        });
                    }
                    // Preserve the existing encrypted blob.
                    row.ai_api_key_encrypted.clone()
                }
            };
            // For base_url: Some(...) overrides, None preserves existing.
            let base_url = input.base_url.clone().or(row.ai_base_url.clone());
            (encrypted, base_url)
        }
    };

    // Merge model into settings JSON (non-destructive for other keys).
    let new_settings = match input.model.as_deref() {
        Some(model) => Some(merge_default_model(&row.settings, model)),
        None => row.settings.clone(),
    };
    let settings_str = match &new_settings {
        Some(v) => Some(
            serde_json::to_string(v)
                .map_err(|e| WorkspaceAiConfigError::Json(e.to_string()))?,
        ),
        None => None,
    };

    let provider_str = input.provider.as_str();

    // Single UPDATE covering all four columns, run inside a transaction so
    // the settings JSON and the dedicated columns commit atomically.
    // Postgres needs an explicit cast ($4::jsonb) because $4 binds as text
    // and the `settings` column is jsonb — COALESCE requires matching types.
    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let pg_sql = "UPDATE workspaces SET \
                 ai_provider = $1, \
                 ai_api_key_encrypted = $2, \
                 ai_base_url = $3, \
                 settings = COALESCE($4::json, settings) \
                 WHERE workspace_id = $5";
            let mut tx = pg.begin().await?;
            let result = sqlx::query(pg_sql)
                .bind(provider_str)
                .bind(new_encrypted_key.as_deref())
                .bind(new_base_url.as_deref())
                .bind(settings_str.as_deref())
                .bind(workspace_id)
                .execute(&mut *tx)
                .await?;
            if result.rows_affected() == 0 {
                return Err(WorkspaceAiConfigError::WorkspaceNotFound(
                    workspace_id.to_string(),
                ));
            }
            tx.commit().await?;
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let sq_sql = "UPDATE workspaces SET \
                 ai_provider = $1, \
                 ai_api_key_encrypted = $2, \
                 ai_base_url = $3, \
                 settings = COALESCE($4, settings) \
                 WHERE workspace_id = $5";
            let mut tx = sq.begin().await?;
            let result = sqlx::query(sq_sql)
                .bind(provider_str)
                .bind(new_encrypted_key.as_deref())
                .bind(new_base_url.as_deref())
                .bind(settings_str.as_deref())
                .bind(workspace_id)
                .execute(&mut *tx)
                .await?;
            if result.rows_affected() == 0 {
                return Err(WorkspaceAiConfigError::WorkspaceNotFound(
                    workspace_id.to_string(),
                ));
            }
            tx.commit().await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Settings JSON helpers
// ---------------------------------------------------------------------------

/// Read `settings.custom_settings.default_model` as a string, matching the
/// layout written by `apps/server/src/routes/workspaces.rs::update_model_settings`.
fn read_default_model(settings: &Option<serde_json::Value>) -> Option<String> {
    read_custom_settings_string(settings, "default_model")
}

/// Read `settings.custom_settings.title_model` as a string.
///
/// When set, the title generation logic uses this model instead of overriding
/// to the cheapest model per provider. `None` means "use the cheapest model"
/// (existing fallback behaviour).
pub fn read_title_model(settings: &Option<serde_json::Value>) -> Option<String> {
    read_custom_settings_string(settings, "title_model")
}

fn read_context_window(settings: &Option<serde_json::Value>) -> u32 {
    settings
        .as_ref()
        .and_then(|s| s.get("custom_settings"))
        .and_then(|cs| cs.get("context_window"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
}

/// Load the `title_model` setting for a workspace directly from the database.
///
/// Fetches only the `settings` JSON column — cheaper than a full [`load`]
/// call. Returns `Ok(None)` both when no title model is configured and when
/// the workspace row cannot be found; callers treat either case as "use the
/// cheapest-model fallback".
pub async fn load_title_model(
    db: &DbPool,
    workspace_id: &str,
) -> Result<Option<String>, WorkspaceAiConfigError> {
    #[derive(sqlx::FromRow)]
    struct SettingsRow {
        settings: Option<serde_json::Value>,
    }

    let row = kyomi_core::db_fetch_optional!(
        db,
        SettingsRow,
        "SELECT settings FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?;

    Ok(row.and_then(|r| read_title_model(&r.settings)))
}

/// Internal helper: read a single string value from `settings.custom_settings[key]`.
fn read_custom_settings_string(settings: &Option<serde_json::Value>, key: &str) -> Option<String> {
    settings
        .as_ref()
        .and_then(|s| s.get("custom_settings"))
        .and_then(|cs| cs.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Non-destructively merge a new `default_model` value into the settings JSON.
///
/// Matches the behaviour of `merge_custom_settings` in
/// `apps/server/src/routes/workspaces.rs`. Kept private to avoid a route →
/// auth crate dependency; the two implementations are trivial and share the
/// same contract.
fn merge_default_model(
    settings: &Option<serde_json::Value>,
    model: &str,
) -> serde_json::Value {
    merge_custom_settings_key(settings, "default_model", model)
}


/// Internal helper: non-destructively write `settings.custom_settings[key] = model`.
fn merge_custom_settings_key(
    settings: &Option<serde_json::Value>,
    key: &str,
    model: &str,
) -> serde_json::Value {
    let mut s = settings
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));

    if !s.is_object() {
        // Unexpected shape — replace with a fresh object.
        s = serde_json::json!({});
    }

    if s.get("custom_settings").is_none()
        && let Some(obj) = s.as_object_mut()
    {
        obj.insert("custom_settings".to_string(), serde_json::json!({}));
    }

    if let Some(cs) = s.get_mut("custom_settings").and_then(|v| v.as_object_mut()) {
        cs.insert(key.to_string(), serde_json::json!(model));
    }

    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Provider enum --------------------------------------------------

    #[test]
    fn provider_as_str_covers_all_variants() {
        assert_eq!(WorkspaceAiProvider::Kyomi.as_str(), "kyomi");
        assert_eq!(WorkspaceAiProvider::Anthropic.as_str(), "anthropic");
        assert_eq!(WorkspaceAiProvider::OpenAI.as_str(), "openai");
        assert_eq!(WorkspaceAiProvider::Gemini.as_str(), "gemini");
    }

    #[test]
    fn provider_from_str_roundtrip() {
        for p in [
            WorkspaceAiProvider::Kyomi,
            WorkspaceAiProvider::Anthropic,
            WorkspaceAiProvider::OpenAI,
            WorkspaceAiProvider::Gemini,
        ] {
            let parsed = WorkspaceAiProvider::from_str(p.as_str()).unwrap();
            assert_eq!(parsed, p, "roundtrip failed for {p:?}");
        }
    }

    #[test]
    fn provider_from_str_rejects_unknown() {
        let err = WorkspaceAiProvider::from_str("bedrock").unwrap_err();
        match err {
            WorkspaceAiConfigError::InvalidProvider(v) => assert_eq!(v, "bedrock"),
            other => panic!("expected InvalidProvider, got {other:?}"),
        }
    }

    #[test]
    fn provider_from_str_is_case_sensitive() {
        // DB CHECK constraint is lowercase; we refuse mixed case rather than
        // silently normalising to keep the round-trip tight.
        assert!(WorkspaceAiProvider::from_str("Kyomi").is_err());
        assert!(WorkspaceAiProvider::from_str("ANTHROPIC").is_err());
    }

    #[test]
    fn provider_serde_lowercase() {
        let json = serde_json::to_string(&WorkspaceAiProvider::Anthropic).unwrap();
        assert_eq!(json, "\"anthropic\"");
        let back: WorkspaceAiProvider = serde_json::from_str("\"openai\"").unwrap();
        assert_eq!(back, WorkspaceAiProvider::OpenAI);
    }

    // ---- is_byok --------------------------------------------------------

    #[test]
    fn is_byok_false_for_kyomi() {
        let cfg = WorkspaceAiConfig {
            provider: WorkspaceAiProvider::Kyomi,
            model: None,
            api_key: None,
            base_url: None,
            title_model: None,
        context_window: 0,
        };
        assert!(!cfg.is_byok());
    }

    #[test]
    fn is_byok_true_for_all_byok_providers() {
        for p in [
            WorkspaceAiProvider::Anthropic,
            WorkspaceAiProvider::OpenAI,
            WorkspaceAiProvider::Gemini,
        ] {
            let cfg = WorkspaceAiConfig {
                provider: p,
                model: None,
                api_key: Some("sk-...".into()),
                base_url: None,
                title_model: None,
            context_window: 0,
            };
            assert!(cfg.is_byok(), "expected BYOK for {p:?}");
        }
    }

    // ---- settings JSON helpers -----------------------------------------

    #[test]
    fn read_default_model_returns_none_for_missing_settings() {
        assert_eq!(read_default_model(&None), None);
        assert_eq!(read_default_model(&Some(serde_json::json!({}))), None);
        assert_eq!(
            read_default_model(&Some(serde_json::json!({ "custom_settings": {} }))),
            None
        );
    }

    #[test]
    fn read_default_model_extracts_string() {
        let s = Some(serde_json::json!({
            "custom_settings": { "default_model": "claude-sonnet-4-20250514" }
        }));
        assert_eq!(
            read_default_model(&s).as_deref(),
            Some("claude-sonnet-4-20250514")
        );
    }

    #[test]
    fn merge_default_model_creates_nested_structure() {
        let merged = merge_default_model(&None, "gpt-4o");
        assert_eq!(
            merged,
            serde_json::json!({ "custom_settings": { "default_model": "gpt-4o" } })
        );
    }

    #[test]
    fn merge_default_model_preserves_other_keys() {
        let existing = Some(serde_json::json!({
            "custom_settings": {
                "default_model": "old-model",
                "chartml_config": { "palette": "default" }
            },
            "other_top_level": 42
        }));
        let merged = merge_default_model(&existing, "new-model");
        assert_eq!(merged["other_top_level"], serde_json::json!(42));
        assert_eq!(
            merged["custom_settings"]["chartml_config"]["palette"],
            serde_json::json!("default")
        );
        assert_eq!(
            merged["custom_settings"]["default_model"],
            serde_json::json!("new-model")
        );
    }

    #[test]
    fn merge_default_model_replaces_non_object_root() {
        // Pathological case: settings column somehow holds a non-object.
        // We reset to a fresh object rather than panic.
        let junk = Some(serde_json::json!("not-an-object"));
        let merged = merge_default_model(&junk, "m");
        assert_eq!(
            merged,
            serde_json::json!({ "custom_settings": { "default_model": "m" } })
        );
    }

    // ---- title_model settings helpers -----------------------------------

    #[test]
    fn read_title_model_returns_none_for_missing_settings() {
        assert_eq!(read_title_model(&None), None);
        assert_eq!(read_title_model(&Some(serde_json::json!({}))), None);
        assert_eq!(
            read_title_model(&Some(serde_json::json!({ "custom_settings": {} }))),
            None
        );
    }

    #[test]
    fn read_title_model_extracts_string() {
        let s = Some(serde_json::json!({
            "custom_settings": { "title_model": "claude-haiku-4-20250514" }
        }));
        assert_eq!(
            read_title_model(&s).as_deref(),
            Some("claude-haiku-4-20250514")
        );
    }

    #[test]
    fn read_title_model_independent_of_default_model() {
        // Both keys can coexist under custom_settings.
        let s = Some(serde_json::json!({
            "custom_settings": {
                "default_model": "gpt-4o",
                "title_model": "gpt-4o-mini"
            }
        }));
        assert_eq!(read_default_model(&s).as_deref(), Some("gpt-4o"));
        assert_eq!(read_title_model(&s).as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn merge_custom_settings_key_creates_nested_structure() {
        let merged = merge_custom_settings_key(&None, "title_model", "gpt-4o-mini");
        assert_eq!(
            merged,
            serde_json::json!({ "custom_settings": { "title_model": "gpt-4o-mini" } })
        );
    }

    #[test]
    fn merge_custom_settings_key_preserves_other_keys() {
        let existing = Some(serde_json::json!({
            "custom_settings": {
                "default_model": "gpt-4o",
                "title_model": "old-title-model",
                "chartml_config": { "palette": "default" }
            },
            "other_top_level": 99
        }));
        let merged = merge_custom_settings_key(&existing, "title_model", "gpt-4o-mini");
        assert_eq!(merged["other_top_level"], serde_json::json!(99));
        assert_eq!(
            merged["custom_settings"]["default_model"],
            serde_json::json!("gpt-4o"),
            "default_model must not be disturbed"
        );
        assert_eq!(
            merged["custom_settings"]["chartml_config"]["palette"],
            serde_json::json!("default")
        );
        assert_eq!(
            merged["custom_settings"]["title_model"],
            serde_json::json!("gpt-4o-mini")
        );
    }

    // ---- DB-backed roundtrip tests -------------------------------------
    //
    // These run against an in-memory SQLite pool that applies the full
    // `apps/server/migrations-sqlite` chain, so they exercise the real
    // `workspaces` schema (including `ai_provider`, `ai_api_key_encrypted`,
    // and `ai_base_url` added by 00015). The fixture lives in the nested
    // `test_support` module below — kept in-file because it's only used by
    // this module and is `#[cfg(test)]`.

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    // --- Env var serialization ------------------------------------------
    //
    // Both `workspace_secrets::tests` and these tests mutate
    // `WORKSPACE_SECRETS_KEY`. Each test module has its own lock, but they
    // still race when run in the same binary. We intentionally use a module-
    // local lock here (matching the `workspace_secrets` pattern) and rely on
    // tests in this module never overlapping env reads with that module —
    // since they're in separate files and cargo-test parallelism schedules
    // them independently, each acquires its own lock before touching the var
    // and clears it before releasing. This matches the pattern already in
    // `workspace_secrets::tests`.

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn with_key(key_b64: &str) -> Self {
            let lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("WORKSPACE_SECRETS_KEY").ok();
            // SAFETY: lock is held for the duration of the test.
            unsafe { std::env::set_var("WORKSPACE_SECRETS_KEY", key_b64) };
            Self { _lock: lock, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("WORKSPACE_SECRETS_KEY", v),
                    None => std::env::remove_var("WORKSPACE_SECRETS_KEY"),
                }
            }
        }
    }

    fn test_key() -> String {
        BASE64_STANDARD.encode([0x7Fu8; 32])
    }

    // --- Fixture helpers ------------------------------------------------

    mod test_support {
        use kyomi_core::DbPool;
        use sqlx::sqlite::SqlitePoolOptions;

        /// Build an in-memory SQLite pool with the full server migration
        /// chain applied. A single shared connection is used because
        /// `:memory:` databases are per-connection in SQLite; `max_connections=1`
        /// guarantees all queries hit the same in-memory database.
        pub async fn test_sqlite_pool() -> DbPool {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .expect("connect in-memory sqlite");

            sqlx::query("PRAGMA foreign_keys=ON")
                .execute(&pool)
                .await
                .expect("enable foreign keys");

            sqlx::migrate!("../../apps/server/migrations-sqlite")
                .run(&pool)
                .await
                .expect("run sqlite migrations");

            DbPool::Sqlite(pool)
        }

        /// Insert a minimal users row + workspaces row. All non-defaulted
        /// NOT NULL columns are supplied; everything else uses the schema
        /// default (including `ai_provider` which defaults to `'kyomi'`).
        pub async fn insert_test_workspace(db: &DbPool, workspace_id: &str) {
            let sq = match db {
                DbPool::Sqlite(sq) => sq,
                _ => panic!("test fixture requires sqlite pool"),
            };

            let user_id = format!("user-{workspace_id}");
            let email = format!("{workspace_id}@test.local");

            sqlx::query("INSERT INTO users (user_id, email) VALUES (?1, ?2)")
                .bind(&user_id)
                .bind(&email)
                .execute(sq)
                .await
                .expect("insert user");

            sqlx::query(
                "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
                 VALUES (?1, ?2, ?3)",
            )
            .bind(workspace_id)
            .bind("Test Workspace")
            .bind(&user_id)
            .execute(sq)
            .await
            .expect("insert workspace");
        }
    }

    use test_support::{insert_test_workspace, test_sqlite_pool};

    // --- Tests ----------------------------------------------------------

    #[tokio::test]
    async fn db_roundtrip_kyomi_to_byok_to_kyomi() {
        let _env = EnvGuard::with_key(&test_key());
        let db = test_sqlite_pool().await;
        let ws = "ws-roundtrip";
        insert_test_workspace(&db, ws).await;

        // 1. Fresh workspace defaults to Kyomi mode.
        let cfg = load(&db, ws).await.unwrap();
        assert_eq!(cfg.provider, WorkspaceAiProvider::Kyomi);
        assert_eq!(cfg.api_key, None);
        assert_eq!(cfg.model, None);
        assert_eq!(cfg.base_url, None);

        // 2. Switch to BYOK (Anthropic) with key, model, base URL.
        update(
            &db,
            ws,
            UpdateWorkspaceAiConfigInput {
                provider: WorkspaceAiProvider::Anthropic,
                api_key: Some("sk-ant-test".to_string()),
                model: Some("claude-sonnet-4-5-20250929".to_string()),
                base_url: Some("https://proxy.example.com".to_string()),
            },
        )
        .await
        .unwrap();

        let cfg = load(&db, ws).await.unwrap();
        assert_eq!(cfg.provider, WorkspaceAiProvider::Anthropic);
        assert_eq!(cfg.api_key.as_deref(), Some("sk-ant-test"));
        assert_eq!(cfg.model.as_deref(), Some("claude-sonnet-4-5-20250929"));
        assert_eq!(cfg.base_url.as_deref(), Some("https://proxy.example.com"));

        // 3. Switch back to Kyomi — key + base URL must be cleared.
        update(
            &db,
            ws,
            UpdateWorkspaceAiConfigInput {
                provider: WorkspaceAiProvider::Kyomi,
                api_key: None,
                model: None,
                base_url: None,
            },
        )
        .await
        .unwrap();

        let cfg = load(&db, ws).await.unwrap();
        assert_eq!(cfg.provider, WorkspaceAiProvider::Kyomi);
        assert_eq!(cfg.api_key, None);
        assert_eq!(cfg.base_url, None);

        // 4. Verify the encrypted column itself was cleared at the DB level
        //    (load() masks it in Kyomi mode regardless).
        let sq = match &db {
            kyomi_core::db::DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        let stored: Option<String> = sqlx::query_scalar(
            "SELECT ai_api_key_encrypted FROM workspaces WHERE workspace_id = ?1",
        )
        .bind(ws)
        .fetch_one(sq)
        .await
        .unwrap();
        assert_eq!(
            stored, None,
            "encrypted key column must be NULL after switching to Kyomi"
        );
    }

    #[tokio::test]
    async fn db_model_only_update_preserves_key() {
        let _env = EnvGuard::with_key(&test_key());
        let db = test_sqlite_pool().await;
        let ws = "ws-model-only";
        insert_test_workspace(&db, ws).await;

        // Seed with BYOK (OpenAI) + initial key.
        update(
            &db,
            ws,
            UpdateWorkspaceAiConfigInput {
                provider: WorkspaceAiProvider::OpenAI,
                api_key: Some("sk-openai-original".to_string()),
                model: Some("gpt-4o".to_string()),
                base_url: None,
            },
        )
        .await
        .unwrap();

        // Model-only update (api_key = None, base_url = None).
        update(
            &db,
            ws,
            UpdateWorkspaceAiConfigInput {
                provider: WorkspaceAiProvider::OpenAI,
                api_key: None,
                model: Some("gpt-4o-mini".to_string()),
                base_url: None,
            },
        )
        .await
        .unwrap();

        let cfg = load(&db, ws).await.unwrap();
        assert_eq!(cfg.provider, WorkspaceAiProvider::OpenAI);
        assert_eq!(
            cfg.api_key.as_deref(),
            Some("sk-openai-original"),
            "existing encrypted key must be preserved across model-only updates"
        );
        assert_eq!(cfg.model.as_deref(), Some("gpt-4o-mini"));
    }

    #[tokio::test]
    async fn db_byok_without_key_rejected() {
        let _env = EnvGuard::with_key(&test_key());
        let db = test_sqlite_pool().await;
        let ws = "ws-byok-no-key";
        insert_test_workspace(&db, ws).await;

        // Workspace is in default Kyomi mode with no stored key.
        let err = update(
            &db,
            ws,
            UpdateWorkspaceAiConfigInput {
                provider: WorkspaceAiProvider::Anthropic,
                api_key: None,
                model: Some("claude-sonnet-4-5-20250929".to_string()),
                base_url: None,
            },
        )
        .await
        .unwrap_err();

        match err {
            WorkspaceAiConfigError::MissingApiKey { provider } => {
                assert_eq!(provider, "anthropic");
            }
            other => panic!("expected MissingApiKey, got {other:?}"),
        }
    }
}
