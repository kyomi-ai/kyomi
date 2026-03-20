// SPDX-License-Identifier: AGPL-3.0-or-later

//! Newsletter subscription endpoints.
//!
//! Wire-compatible with Python's `routers/subscribe.py`.
//! All three endpoints are public (no auth required).
//! `POST /subscribe` is rate-limited by IP via KVPool (Redis or in-memory).

use axum::{
    extract::State,
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::state::AppState;

/// Build the subscribe router.
///
/// Mounted under `/api/v1` so the full paths are:
/// - `POST /api/v1/subscribe`
/// - `GET  /api/v1/subscribers/count`
/// - `POST /api/v1/unsubscribe`
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/subscribe", post(subscribe_email))
        .route("/subscribers/count", get(get_subscribers_count))
        .route("/unsubscribe", post(unsubscribe_email))
}

// ---------------------------------------------------------------------------
// Rate limiting (Redis INCR + EXPIRE, 5 per hour per IP)
// ---------------------------------------------------------------------------

/// Maximum subscription attempts per IP within the rate window.
const RATE_LIMIT_MAX: i64 = 5;

/// Rate limit window in seconds (1 hour).
const RATE_LIMIT_WINDOW_SECS: i64 = 3600;

/// Check subscribe rate limit for an IP address.
///
/// Uses KVPool `incr` + `expire` pattern for distributed safety.
/// Returns `Ok(true)` if allowed, `Ok(false)` if rate-limited.
async fn check_subscribe_rate_limit(
    kv: &kyomi_core::KVPool,
    ip: &str,
) -> kyomi_core::Result<bool> {
    let key = format!("subscribe:rate:{ip}");

    // INCR atomically increments (creates with value 1 if key doesn't exist)
    let count = kv.incr(&key).await?;

    // Set TTL only on the first increment to establish the rate window.
    // Subsequent increments within the window intentionally do NOT reset the TTL —
    // the window slides forward only when the key expires and a new one is created.
    if count == 1 {
        kv.expire(&key, RATE_LIMIT_WINDOW_SECS as u64).await?;
    }

    Ok(count <= RATE_LIMIT_MAX)
}

// ---------------------------------------------------------------------------
// IP extraction
// ---------------------------------------------------------------------------

fn extract_client_ip(headers: &HeaderMap) -> String {
    crate::helpers::extract_client_ip(headers, None)
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SubscribeRequest {
    email: String,
    company_name: Option<String>,
    company_size: Option<String>,
    use_case: Option<String>,
    #[serde(default)]
    marketing_consent: bool,
    #[serde(default = "default_source")]
    source: Option<String>,
}

fn default_source() -> Option<String> {
    Some("web".to_string())
}

#[derive(Debug, Deserialize)]
struct UnsubscribeRequest {
    email: String,
}

// ---------------------------------------------------------------------------
// POST /subscribe
// ---------------------------------------------------------------------------

async fn subscribe_email(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<SubscribeRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Rate limit by IP
    let ip = extract_client_ip(&headers);
    let allowed = check_subscribe_rate_limit(&state.kv, &ip).await?;
    if !allowed {
        return Err(kyomi_core::Error::TooManyRequests(
            "Too many signup attempts. Please try again later.".into(),
            RATE_LIMIT_WINDOW_SECS as u64,
        ));
    }

    // Validate email: must have exactly one @, non-empty local and domain, domain has a dot
    let email = data.email.trim().to_lowercase();
    let is_valid = {
        let parts: Vec<&str> = email.splitn(2, '@').collect();
        parts.len() == 2
            && !parts[0].is_empty()
            && !parts[1].is_empty()
            && parts[1].contains('.')
            && !parts[1].starts_with('.')
            && !parts[1].ends_with('.')
    };
    if !is_valid {
        return Err(kyomi_core::Error::BadRequest(
            "A valid email address is required.".into(),
        ));
    }

    let source = data.source.as_deref().unwrap_or("web");

    // Check if subscriber already exists
    #[derive(sqlx::FromRow)]
    struct ExistsRow { _n: i32 }
    let existing = kyomi_core::db_fetch_optional!(
        &state.db, ExistsRow,
        "SELECT 1 as _n FROM email_subscribers WHERE email = $1",
        &email
    )?;

    let is_pg = state.db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);

    if existing.is_some() {
        // Update existing subscriber
        let sql = format!(
            "UPDATE email_subscribers \
             SET company_name = $1, company_size = $2, use_case = $3, \
                 marketing_consent = $4, source = $5, updated_at = {now_expr} \
             WHERE email = $6"
        );
        kyomi_core::db_execute!(
            &state.db, &sql,
            data.company_name.as_deref(),
            data.company_size.as_deref(),
            data.use_case.as_deref(),
            data.marketing_consent,
            source,
            &email
        )?;

        tracing::info!(email = %email, "Existing subscriber re-registered");
    } else {
        // Insert new subscriber
        let sql = format!(
            "INSERT INTO email_subscribers \
                (email, company_name, company_size, use_case, marketing_consent, source, \
                 created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, {now_expr}, {now_expr})"
        );
        kyomi_core::db_execute!(
            &state.db, &sql,
            &email,
            data.company_name.as_deref(),
            data.company_size.as_deref(),
            data.use_case.as_deref(),
            data.marketing_consent,
            source
        )?;

        tracing::info!(email = %email, source = %source, "New email subscriber");
    }

    // Send welcome email to consenting subscribers
    if data.marketing_consent {
        let email_clone = email.clone();
        tokio::spawn(async move {
            let email_svc = kyomi_auth::email_service::EmailService::from_env();
            let sent = email_svc.send_subscription_welcome(&email_clone).await;
            if sent {
                tracing::info!("📧 Welcome email sent to {email_clone}");
            } else {
                tracing::warn!("⚠️ Failed to send welcome email to {email_clone}");
            }
        });
    }

    Ok(Json(serde_json::json!({
        "message": "Thanks for signing up! We'll email you when we launch.",
        "email": email,
    })))
}

// ---------------------------------------------------------------------------
// GET /subscribers/count
// ---------------------------------------------------------------------------

async fn get_subscribers_count(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let is_pg = state.db.is_postgres();
    let bt = kyomi_core::sql_compat::bool_true(is_pg);
    // Postgres: COUNT(*) FILTER (WHERE ...), SQLite: SUM(CASE WHEN ... THEN 1 ELSE 0 END)
    let consent_expr = if is_pg {
        format!("COUNT(*) FILTER (WHERE marketing_consent = {bt})")
    } else {
        format!("SUM(CASE WHEN marketing_consent = {bt} THEN 1 ELSE 0 END)")
    };
    let sql = format!(
        "SELECT COUNT(*) AS total, {consent_expr} AS with_consent FROM email_subscribers"
    );

    #[derive(sqlx::FromRow)]
    struct CountRow { total: i64, with_consent: Option<i64> }

    let row = kyomi_core::db_fetch_one!(&state.db, CountRow, &sql)?;

    Ok(Json(serde_json::json!({
        "total": row.total,
        "with_marketing_consent": row.with_consent.unwrap_or(0),
    })))
}

// ---------------------------------------------------------------------------
// POST /unsubscribe
// ---------------------------------------------------------------------------

async fn unsubscribe_email(
    State(state): State<AppState>,
    Json(data): Json<UnsubscribeRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let email = data.email.trim().to_lowercase();

    // Update marketing consent — always returns success for privacy
    let is_pg = state.db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);
    let bf = kyomi_core::sql_compat::bool_false(is_pg);
    let sql = format!(
        "UPDATE email_subscribers \
         SET marketing_consent = {bf}, updated_at = {now_expr} \
         WHERE email = $1"
    );
    kyomi_core::db_execute!(&state.db, &sql, &email)?;

    tracing::info!(email = %email, "Unsubscribe processed");

    Ok(Json(serde_json::json!({
        "message": "You've been unsubscribed from Kyomi updates.",
    })))
}
