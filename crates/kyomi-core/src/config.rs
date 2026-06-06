// SPDX-License-Identifier: AGPL-3.0-or-later

//! Application configuration loaded from environment variables.

use std::env;

use crate::standalone;

/// Self-hosted edition — controls which commercial features are available.
///
/// Set via `KYOMI_EDITION` env var (only read when `SELF_HOSTED=true`).
/// - `community`: AGPLv3 free edition — open-core feature set
/// - `enterprise`: Commercial edition — all features available (with required services)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfHostedEdition {
    Community,
    Enterprise,
}

/// Deployment mode for the Kyomi backend.
///
/// Set via `KYOMI_MODE` env var. Falls back to `SELF_HOSTED` for backward compat.
/// - `saas`: Multi-tenant hosted service (app.kyomi.ai)
/// - `self_hosted`: Team server with full auth (password, optional OAuth)
/// - `personal`: Single-user desktop app — zero auth, SQLite, localhost only
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KyomiMode {
    Saas,
    SelfHosted,
    Personal,
}

impl KyomiMode {
    /// Derive the legacy `self_hosted` bool from the mode.
    fn self_hosted(&self) -> bool {
        matches!(self, KyomiMode::SelfHosted | KyomiMode::Personal)
    }

    /// Derive the edition from the mode.
    /// Personal always gets Community. SelfHosted reads from `KYOMI_EDITION` env.
    /// Saas returns Community (edition is irrelevant for SaaS).
    fn edition(&self) -> SelfHostedEdition {
        match self {
            KyomiMode::SelfHosted => {
                match env::var("KYOMI_EDITION").unwrap_or_default().to_lowercase().as_str() {
                    "enterprise" => SelfHostedEdition::Enterprise,
                    _ => SelfHostedEdition::Community,
                }
            }
            _ => SelfHostedEdition::Community,
        }
    }
}

/// Central application configuration.
///
/// Mirrors the Python `KyomiAPIConfig` — loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// PostgreSQL connection string
    pub database_url: String,

    /// Redis connection string.
    ///
    /// When `None`, the server falls back to the in-memory KV store
    /// (suitable for single-instance / self-hosted deployments).
    pub redis_url: Option<String>,

    /// JWT signing secret (HS256, shared with Python backend)
    pub jwt_secret: String,

    /// AES-256-GCM encryption key for credentials at rest
    pub encryption_key: String,

    /// Server listen port
    pub port: u16,

    /// Enable background schedulers (watches, catalog refresh)
    pub enable_schedulers: bool,

    /// Demo mode — disables HSTS headers
    pub demo_mode: bool,

    /// Self-hosted mode — disables billing, analytics, and grants enterprise capabilities
    pub self_hosted: bool,

    /// Self-hosted edition. Only meaningful when `self_hosted = true`.
    /// Controls which commercial-gated features are available.
    pub edition: SelfHostedEdition,

    /// Deployment mode. Determines auth strategy, database backend, and UI surface.
    /// The `self_hosted` and `edition` fields are computed from this for backward compat.
    pub mode: KyomiMode,

    /// Whether SMTP is configured at startup time.
    /// True when both `SMTP_HOST` and `SMTP_USER` env vars are set.
    /// Matches `EmailService::is_configured()` logic.
    pub smtp_configured: bool,

    // ── Auth Methods ─────────────────────────────────────────────────────
    /// Enable passkey (WebAuthn) authentication. Defaults to true.
    pub passkeys_enabled: bool,

    /// Enable password-based authentication. Defaults to true.
    pub password_auth_enabled: bool,

    // ── Google OAuth ────────────────────────────────────────────────────
    /// Google OAuth client ID
    pub google_oauth_client_id: Option<String>,

    /// Google OAuth client secret
    pub google_oauth_client_secret: Option<String>,

    // ── WebAuthn (Passkeys) ─────────────────────────────────────────────
    /// Relying Party ID for WebAuthn (e.g., "localhost" or "kyomi.ai")
    pub webauthn_rp_id: String,

    /// Relying Party display name
    pub webauthn_rp_name: String,

    // ── Frontend ────────────────────────────────────────────────────────
    /// Frontend URL for constructing callback/redirect URLs
    pub frontend_url: String,

    /// Backend base URL for constructing OAuth redirect URIs
    pub base_url: String,

    // ── AI / Agent ──────────────────────────────────────────────────────
    /// Anthropic API key for LLM calls.
    pub anthropic_api_key: Option<String>,

    /// Generic LLM provider name (e.g., "anthropic", "openai", "gemini").
    /// When set together with `llm_api_key`, overrides the legacy `anthropic_api_key`.
    pub llm_provider: Option<String>,

    /// API key for the LLM provider specified by `llm_provider`.
    pub llm_api_key: Option<String>,

    /// Model override for the LLM provider (uses provider default when absent).
    pub llm_model: Option<String>,

    /// Custom base URL for the LLM API (e.g., for proxies or OpenAI-compatible endpoints).
    pub llm_base_url: Option<String>,

    /// Model for lightweight tasks (title generation, dashboard summaries).
    /// Uses the cheapest model for the provider when absent.
    pub llm_title_model: Option<String>,

    // ── Stripe ──────────────────────────────────────────────────────────
    /// Stripe secret key (sk_test_ or sk_live_).
    /// Optional — server starts without it (no billing features).
    pub stripe_secret_key: Option<String>,

    /// Stripe publishable key (pk_test_ or pk_live_).
    /// Optional — needed for embedded checkout on the frontend.
    pub stripe_publishable_key: Option<String>,

    /// Stripe webhook signing secret (whsec_...).
    /// Optional — server starts without it (webhooks rejected).
    pub stripe_webhook_secret: Option<String>,

    // ── Slack ───────────────────────────────────────────────────────────
    /// Slack app client ID (for OAuth flows).
    /// Optional — server starts without it (Slack features unavailable).
    pub slack_client_id: Option<String>,

    /// Slack app client secret (for OAuth token exchange).
    pub slack_client_secret: Option<String>,

    /// Slack app signing secret (for verifying request signatures).
    pub slack_signing_secret: Option<String>,

    // ── Slack Feedback ─────────────────────────────────────────────────
    /// Slack incoming webhook URL for posting feedback notifications.
    /// Optional — feedback still stored without it, just no Slack alerts.
    pub slack_feedback_webhook_url: Option<String>,

    /// Slack bot token for feedback screenshot uploads (files.getUploadURLExternal API).
    /// This is the Kyomi app's own bot token, separate from per-workspace bot tokens.
    /// Optional — screenshots are skipped if not configured.
    pub slack_bot_token: Option<String>,

    /// Slack channel ID for uploading feedback screenshots.
    /// Optional — screenshots are skipped if not configured.
    pub slack_feedback_channel_id: Option<String>,

    // ── Trakkt Feedback ──────────────────────────────────────────────
    /// Trakkt API bearer token for creating feedback issues.
    /// Optional — feedback still stored locally without it, just no Trakkt issues.
    pub trakkt_api_token: Option<String>,

    /// Trakkt API base URL. Defaults to "https://trakkt.app".
    pub trakkt_api_url: String,

    /// Trakkt team key for feedback issue creation (e.g. "KYO").
    pub trakkt_feedback_team_key: Option<String>,

    // ── Admin Notifications ──────────────────────────────────────────
    /// Email address for admin notifications (feedback, signups).
    /// Defaults to "support@kyomi.ai".
    pub support_email: String,

    // ── Web Push (VAPID) ──────────────────────────────────────────────
    /// Base64url-encoded ECDSA P-256 private key for VAPID signing.
    /// Optional — push notifications disabled when not configured.
    pub vapid_private_key: Option<String>,

    /// Contact URI for push service identification (e.g., "mailto:support@kyomi.ai").
    /// Optional — push notifications disabled when not configured.
    pub vapid_contact: Option<String>,

    // ── Analytics ──────────────────────────────────────────────────────
    /// HMAC-SHA256 secret for signing analytics site keys.
    /// The collector uses this to verify keys statelessly (no DB lookup).
    /// Empty string disables analytics site creation.
    pub analytics_signing_secret: String,

    /// Analytics ClickHouse host for admin operations (user/policy DDL).
    pub analytics_clickhouse_host: String,

    /// Analytics ClickHouse HTTP port.
    pub analytics_clickhouse_port: u16,

    /// Analytics ClickHouse admin password (for the `default` user).
    pub analytics_clickhouse_password: String,

    /// Whether to use HTTPS for ClickHouse connections.
    pub analytics_clickhouse_secure: bool,

    // ── Kyomi Connect ────────────────────────────────────────────────
    /// PEM-encoded ECDSA P-256 private key for signing Connect JWTs (ES256).
    /// Optional — Connect features disabled when not configured.
    pub connect_jwt_private_key: Option<String>,

    /// WebSocket URL embedded in Connect tokens (where the Connect binary connects).
    /// Defaults to `wss://connect.kyomi.ai/v1`.
    pub connect_url: String,

    // ── Workspace Secrets (BYOK) ────────────────────────────────────────
    /// Base64-encoded 32-byte master key for encrypting workspace-level
    /// secrets (BYOK API keys), loaded from `WORKSPACE_SECRETS_KEY`.
    ///
    /// **Required in SaaS mode** — the server refuses to start without it.
    /// Optional in self-hosted mode; absence disables BYOK features.
    pub workspace_secrets_key: Option<String>,
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// When `DATABASE_URL` is absent, enters standalone mode: creates a data
    /// directory, generates secrets, and configures SQLite + community edition.
    ///
    /// Panics on missing required variables — fail fast at startup.
    pub fn from_env() -> Self {
        // Standalone mode: if DATABASE_URL is not set, auto-configure for single-binary operation.
        if env::var("DATABASE_URL").is_err() {
            let data_dir = standalone::data_dir();
            std::fs::create_dir_all(&data_dir).expect("Failed to create data directory");

            let standalone_config = standalone::load_or_create_config(&data_dir)
                .expect("Failed to load or create standalone config");

            // Inject env vars for the rest of initialization
            let db_path = data_dir.join("kyomi.db");
            // SAFETY: env::set_var is called before the tokio runtime spawns worker threads
            // (this runs in main() before any async work begins), so there are no data races.
            unsafe {
                env::set_var(
                    "DATABASE_URL",
                    format!("sqlite://{}?mode=rwc", db_path.display()),
                );
                env::set_var("JWT_SECRET_KEY", &standalone_config.jwt_secret);
                env::set_var("ENCRYPTION_KEY", &standalone_config.encryption_key);
                // Only set SELF_HOSTED if KYOMI_MODE is not already set to personal.
                // Personal mode is distinct from self-hosted; it inherits self_hosted=true
                // via KyomiMode::Personal.self_hosted(), but we don't pollute the env.
                if env::var("KYOMI_MODE").unwrap_or_default().to_lowercase() != "personal" {
                    env::set_var("SELF_HOSTED", "true");
                    env::set_var("KYOMI_EDITION", "community");
                }
            }

            // Set defaults only if not already set by user
            if env::var("PORT").is_err() {
                // SAFETY: Single-threaded startup — no worker threads yet.
                unsafe { env::set_var("PORT", "3000") };
            }
            if env::var("FRONTEND_URL").is_err() {
                let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
                // SAFETY: Single-threaded startup — no worker threads yet.
                unsafe { env::set_var("FRONTEND_URL", format!("http://localhost:{port}")) };
            }
            if env::var("BASE_URL").is_err() {
                let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
                // SAFETY: Single-threaded startup — no worker threads yet.
                unsafe { env::set_var("BASE_URL", format!("http://localhost:{port}")) };
            }
        }

        // Determine deployment mode.
        // KYOMI_MODE takes precedence; fall back to SELF_HOSTED for backward compat.
        let mode = match env::var("KYOMI_MODE").unwrap_or_default().to_lowercase().as_str() {
            "personal" => KyomiMode::Personal,
            "self_hosted" | "selfhosted" => KyomiMode::SelfHosted,
            "saas" => KyomiMode::Saas,
            _ => {
                // Backward compat: check legacy SELF_HOSTED bool
                if env::var("SELF_HOSTED")
                    .unwrap_or_else(|_| "false".into())
                    .parse()
                    .unwrap_or(false)
                {
                    KyomiMode::SelfHosted
                } else {
                    KyomiMode::Saas
                }
            }
        };

        Self {
            database_url: required_env("DATABASE_URL"),
            redis_url: env::var("REDIS_URL").ok(),
            jwt_secret: required_env("JWT_SECRET_KEY"),
            encryption_key: required_env("ENCRYPTION_KEY"),
            port: env::var("PORT")
                .unwrap_or_else(|_| "8003".into())
                .parse()
                .expect("PORT must be a valid u16"),
            enable_schedulers: env::var("ENABLE_SCHEDULERS")
                .unwrap_or_else(|_| "true".into())
                .parse()
                .unwrap_or(true),
            demo_mode: env::var("DEMO_MODE")
                .unwrap_or_else(|_| "false".into())
                .parse()
                .unwrap_or(false),
            self_hosted: mode.self_hosted(),
            edition: mode.edition(),
            mode,
            smtp_configured: env::var("SMTP_HOST").is_ok() && env::var("SMTP_USER").is_ok(),
            passkeys_enabled: env::var("PASSKEYS_ENABLED")
                .unwrap_or_else(|_| "true".into())
                .parse()
                .unwrap_or(true),
            password_auth_enabled: env::var("PASSWORD_AUTH_ENABLED")
                .unwrap_or_else(|_| "true".into())
                .parse()
                .unwrap_or(true),
            google_oauth_client_id: env::var("GOOGLE_OAUTH_CLIENT_ID").ok(),
            google_oauth_client_secret: env::var("GOOGLE_OAUTH_CLIENT_SECRET").ok(),
            webauthn_rp_id: env::var("WEBAUTHN_RP_ID").unwrap_or_else(|_| {
                // Infer from FRONTEND_URL, matching Python backend behavior
                let frontend = env::var("FRONTEND_URL")
                    .unwrap_or_else(|_| "http://localhost:5173".into());
                url::Url::parse(&frontend)
                    .ok()
                    .and_then(|u| u.host_str().map(String::from))
                    .unwrap_or_else(|| "localhost".into())
            }),
            webauthn_rp_name: env::var("WEBAUTHN_RP_NAME")
                .unwrap_or_else(|_| "Kyomi".into()),
            frontend_url: env::var("FRONTEND_URL")
                .unwrap_or_else(|_| "http://localhost:5173".into()),
            base_url: env::var("BASE_URL")
                .unwrap_or_else(|_| "http://localhost:8003".into()),
            anthropic_api_key: env::var("ANTHROPIC_API_KEY").ok(),
            llm_provider: env::var("LLM_PROVIDER").ok(),
            llm_api_key: env::var("LLM_API_KEY").ok(),
            llm_model: env::var("LLM_MODEL").ok(),
            llm_base_url: env::var("LLM_BASE_URL").ok(),
            llm_title_model: env::var("LLM_TITLE_MODEL").ok(),
            stripe_secret_key: env::var("STRIPE_SECRET_KEY").ok(),
            stripe_publishable_key: env::var("STRIPE_PUBLISHABLE_KEY").ok(),
            stripe_webhook_secret: env::var("STRIPE_WEBHOOK_SECRET").ok(),
            slack_client_id: env::var("SLACK_CLIENT_ID").ok(),
            slack_client_secret: env::var("SLACK_CLIENT_SECRET").ok(),
            slack_signing_secret: env::var("SLACK_SIGNING_SECRET").ok(),
            slack_feedback_webhook_url: env::var("SLACK_FEEDBACK_WEBHOOK_URL").ok(),
            slack_bot_token: env::var("SLACK_BOT_TOKEN").ok(),
            slack_feedback_channel_id: env::var("SLACK_FEEDBACK_CHANNEL_ID").ok(),
            trakkt_api_token: env::var("TRAKKT_API_TOKEN").ok(),
            trakkt_api_url: env::var("TRAKKT_API_URL")
                .unwrap_or_else(|_| "https://trakkt.app".into()),
            trakkt_feedback_team_key: env::var("TRAKKT_FEEDBACK_TEAM_KEY").ok(),
            support_email: env::var("SUPPORT_EMAIL")
                .unwrap_or_else(|_| "support@kyomi.ai".into()),
            vapid_private_key: env::var("VAPID_PRIVATE_KEY").ok(),
            vapid_contact: env::var("VAPID_CONTACT").ok(),
            analytics_signing_secret: env::var("ANALYTICS_SIGNING_SECRET")
                .unwrap_or_default(),
            analytics_clickhouse_host: env::var("ANALYTICS_CLICKHOUSE_HOST")
                .unwrap_or_else(|_| "localhost".into()),
            analytics_clickhouse_port: env::var("ANALYTICS_CLICKHOUSE_PORT")
                .unwrap_or_else(|_| "8123".into())
                .parse()
                .expect("ANALYTICS_CLICKHOUSE_PORT must be a valid u16"),
            analytics_clickhouse_password: env::var("ANALYTICS_CLICKHOUSE_PASSWORD")
                .unwrap_or_default(),
            analytics_clickhouse_secure: env::var("ANALYTICS_CLICKHOUSE_SECURE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            connect_jwt_private_key: env::var("CONNECT_JWT_PRIVATE_KEY").ok(),
            connect_url: env::var("CONNECT_URL")
                .unwrap_or_else(|_| "wss://connect.kyomi.ai/v1".into()),
            workspace_secrets_key: env::var("WORKSPACE_SECRETS_KEY")
                .ok()
                .filter(|v| !v.is_empty()),
        }
    }

    /// Load configuration for tests with sensible defaults.
    ///
    /// Available under `#[cfg(test)]` and the `test-helpers` feature (for
    /// integration tests in dependent crates).
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn test_config() -> Self {
        // base64url-encoded 32-byte key for test encryption
        // Decoded: "test-aes-key-for-unit-tests!!!!!" (32 bytes)
        const TEST_ENCRYPTION_KEY_B64: &str = "dGVzdC1hZXMta2V5LWZvci11bml0LXRlc3RzISEhISE=";

        Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://kyomi_test:test@localhost:5434/kyomi_test".into()),
            redis_url: env::var("REDIS_URL").ok(),
            jwt_secret: env::var("JWT_SECRET_KEY")
                .unwrap_or_else(|_| "test-jwt-secret-not-for-production".into()),
            encryption_key: env::var("ENCRYPTION_KEY")
                .unwrap_or_else(|_| TEST_ENCRYPTION_KEY_B64.into()),
            port: 0, // random port for tests
            enable_schedulers: false,
            demo_mode: false,
            self_hosted: false,
            edition: SelfHostedEdition::Community,
            mode: KyomiMode::Saas,
            smtp_configured: false,
            passkeys_enabled: true,
            password_auth_enabled: true,
            google_oauth_client_id: Some("test-google-client-id".into()),
            google_oauth_client_secret: Some("test-google-client-secret".into()),
            webauthn_rp_id: "localhost".into(),
            webauthn_rp_name: "Kyomi Test".into(),
            frontend_url: "http://localhost:5173".into(),
            base_url: "http://localhost:8003".into(),
            anthropic_api_key: Some("test-anthropic-key".into()),
            llm_provider: None,
            llm_api_key: None,
            llm_model: None,
            llm_base_url: None,
            llm_title_model: None,
            stripe_secret_key: None,
            stripe_publishable_key: None,
            stripe_webhook_secret: None,
            slack_client_id: None,
            slack_client_secret: None,
            slack_signing_secret: None,
            slack_feedback_webhook_url: None,
            slack_bot_token: None,
            slack_feedback_channel_id: None,
            trakkt_api_token: None,
            trakkt_api_url: "https://trakkt.app".into(),
            trakkt_feedback_team_key: None,
            support_email: "support@kyomi.ai".into(),
            vapid_private_key: None,
            vapid_contact: None,
            analytics_signing_secret: "test-analytics-signing-secret".into(),
            analytics_clickhouse_host: "localhost".into(),
            analytics_clickhouse_port: 8123,
            analytics_clickhouse_password: "test-clickhouse-password".into(),
            analytics_clickhouse_secure: false,
            connect_jwt_private_key: None,
            connect_url: "wss://localhost:8003/connect/v1".into(),
            workspace_secrets_key: None,
        }
    }

}

impl Config {
    /// Returns true if this is an Enterprise self-hosted deployment.
    /// Always false for SaaS (self_hosted=false) — SaaS features are controlled by the capability service.
    pub fn is_enterprise(&self) -> bool {
        self.self_hosted && self.edition == SelfHostedEdition::Enterprise
    }

    /// Returns true if the server is running in personal (desktop) mode.
    pub fn is_personal(&self) -> bool {
        self.mode == KyomiMode::Personal
    }

    /// Returns true if SMTP is configured (account emails and alerts enabled).
    pub fn smtp_configured(&self) -> bool {
        self.smtp_configured
    }

    /// Returns true if Slack integration is configured (client ID + secret present).
    pub fn slack_configured(&self) -> bool {
        self.slack_client_id.is_some() && self.slack_client_secret.is_some()
    }

    /// Returns true if an LLM provider is configured (AI features available).
    pub fn llm_configured(&self) -> bool {
        self.anthropic_api_key.is_some() || self.llm_api_key.is_some()
    }
}

fn required_env(key: &str) -> String {
    env::var(key).unwrap_or_else(|_| panic!("{key} environment variable is required"))
}
