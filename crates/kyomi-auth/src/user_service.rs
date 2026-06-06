// SPDX-License-Identifier: AGPL-3.0-or-later

//! User service — CRUD operations against the `users` table.
//!
//! All queries use runtime dispatch macros for Postgres/SQLite compatibility.
//! Wire-compatible with Python's `PostgreSQLUserStore`.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use kyomi_core::models::{ApiToken, User, UserAuthMethod, Workspace, WorkspaceUser};
use kyomi_core::sql_compat;
use kyomi_core::DbPool;
use rand::Rng;
use sha2::{Digest, Sha256};

/// Returns true if any users exist in the database.
pub async fn has_any_users(pool: &DbPool) -> kyomi_core::Result<bool> {
    // Use EXISTS for efficiency — stops scanning after first row
    let row: (i32,) = kyomi_core::db_fetch_one!(
        pool, (i32,),
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM users) THEN 1 ELSE 0 END"
    )?;
    Ok(row.0 == 1)
}

/// Get a user by their user_id.
pub async fn get_user_by_id(pool: &DbPool, user_id: &str) -> kyomi_core::Result<Option<User>> {
    let user = kyomi_core::db_fetch_optional!(
        pool, User,
        "SELECT * FROM users WHERE user_id = $1",
        user_id
    )?;
    Ok(user)
}

/// Get a user by their email address.
pub async fn get_user_by_email(pool: &DbPool, email: &str) -> kyomi_core::Result<Option<User>> {
    let user = kyomi_core::db_fetch_optional!(
        pool, User,
        "SELECT * FROM users WHERE email = $1",
        email
    )?;
    Ok(user)
}

/// Update the user's last_login timestamp.
pub async fn update_last_login(pool: &DbPool, user_id: &str) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE users SET last_login = {now}, updated_at = {now} WHERE user_id = $1"
    );
    let result = kyomi_core::db_execute!(pool, &sql, user_id)?;
    Ok(result.rows_affected() > 0)
}

/// Update the user's display name.
pub async fn update_user_name(
    pool: &DbPool,
    user_id: &str,
    name: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE users SET name = $1, updated_at = {now} WHERE user_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, name, user_id)?;
    Ok(result.rows_affected() > 0)
}

/// Update BigQuery preferences for a user.
pub async fn update_bigquery_preferences(
    pool: &DbPool,
    user_id: &str,
    billing_project: Option<&str>,
    default_project: Option<&str>,
    query_size_limit_gb: Option<i32>,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE users SET \
         billing_project = COALESCE($1, billing_project), \
         default_project = COALESCE($2, default_project), \
         query_size_limit_gb = COALESCE($3, query_size_limit_gb), \
         updated_at = {now} \
         WHERE user_id = $4"
    );
    let result = kyomi_core::db_execute!(
        pool, &sql,
        billing_project, default_project, query_size_limit_gb, user_id
    )?;
    Ok(result.rows_affected() > 0)
}

/// Update the user's last_workspace_id.
pub async fn update_last_workspace(
    pool: &DbPool,
    user_id: &str,
    workspace_id: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE users SET last_workspace_id = $1, updated_at = {now} WHERE user_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, workspace_id, user_id)?;
    Ok(result.rows_affected() > 0)
}

/// Mark a user as email-verified.
pub async fn mark_user_verified(pool: &DbPool, email: &str) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "UPDATE users SET verified = {bt}, updated_at = {now} WHERE email = $1"
    );
    let result = kyomi_core::db_execute!(pool, &sql, email)?;
    Ok(result.rows_affected() > 0)
}

/// Check whether a user has a password auth method.
pub async fn has_password(pool: &DbPool, user_id: &str) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT COUNT(*) as count FROM user_auth_methods \
         WHERE user_id = $1 AND auth_type = 'password' AND active = {bt}"
    );
    let count: i64 = kyomi_core::db_fetch_scalar!(pool, i64, &sql, user_id)?;
    Ok(count > 0)
}

/// Get workspace details by workspace_id.
pub async fn get_workspace(
    pool: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<Option<Workspace>> {
    let ws = kyomi_core::db_fetch_optional!(
        pool, Workspace,
        "SELECT * FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?;
    Ok(ws)
}

/// Get workspace membership for a user in a specific workspace.
pub async fn get_workspace_user(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> kyomi_core::Result<Option<WorkspaceUser>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT * FROM workspace_users \
         WHERE workspace_id = $1 AND user_id = $2 AND active = {bt}"
    );
    let wu = kyomi_core::db_fetch_optional!(pool, WorkspaceUser, &sql, workspace_id, user_id)?;
    Ok(wu)
}

/// Get the user's workspace context (first active workspace, or last used).
/// Returns (workspace, workspace_user) if found.
pub async fn get_user_workspace_context(
    pool: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Option<(Workspace, WorkspaceUser)>> {
    // First try last_workspace_id from the user record
    let user = get_user_by_id(pool, user_id).await?;
    if let Some(user) = &user
        && let Some(ref last_ws_id) = user.last_workspace_id
        && let Some(ws) = get_workspace(pool, last_ws_id).await?
        && let Some(wu) = get_workspace_user(pool, last_ws_id, user_id).await?
    {
        return Ok(Some((ws, wu)));
    }

    // Fallback: get first active workspace membership
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT * FROM workspace_users \
         WHERE user_id = $1 AND active = {bt} \
         ORDER BY created_at ASC LIMIT 1"
    );
    let wu = kyomi_core::db_fetch_optional!(pool, WorkspaceUser, &sql, user_id)?;

    if let Some(wu) = wu
        && let Some(ws) = get_workspace(pool, &wu.workspace_id).await?
    {
        return Ok(Some((ws, wu)));
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// User creation (Phase 3 — OAuth + Passkey signup)
// ---------------------------------------------------------------------------

/// Generate a user_id matching Python's format: `"user-{token_urlsafe(16)}"`.
fn generate_user_id() -> String {
    let random_bytes: [u8; 16] = rand::rng().random();
    format!("user-{}", URL_SAFE_NO_PAD.encode(random_bytes))
}

/// Generate a workspace_id matching Python's format: `"ws-{uuid4()}"`.
fn generate_workspace_id() -> String {
    format!("ws-{}", uuid::Uuid::new_v4())
}

/// Create a new user in the database.
///
/// Returns the created User. `verified` defaults to false, `active` to true.
/// Matches Python's `user_store.create_user()`.
pub async fn create_user(
    pool: &DbPool,
    email: &str,
    name: Option<&str>,
    verified: bool,
) -> kyomi_core::Result<User> {
    let user_id = generate_user_id();
    let display_name = name.unwrap_or("");
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);

    let sql = format!(
        "INSERT INTO users (user_id, email, name, verified, active) \
         VALUES ($1, $2, $3, $4, {bt})"
    );
    kyomi_core::db_execute!(pool, &sql, &user_id, email, display_name, verified)?;

    // Return the created user
    get_user_by_id(pool, &user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("User created but not found".into()))
}

/// Create a personal workspace for a new user.
///
/// Returns the workspace_id. Matches Python's workspace creation flow.
///
/// When `config` is provided and contains Stripe keys (SaaS mode):
/// - Creates workspace with `cloud` tier, `trialing` status, 30-day trial
/// - Seeds $1 AI credit balance to get started
/// - Creates a Stripe customer and stores the `stripe_customer_id`
///
/// When `config` is `None` or Stripe is not configured:
/// - Self-hosted gets `team` tier with `active` status (no trial, no billing)
/// - SaaS without Stripe gets `cloud` tier with `trialing` status
pub async fn create_workspace_for_user(
    pool: &DbPool,
    user_id: &str,
    _user_name: Option<&str>,
    user_email: &str,
    config: Option<&kyomi_core::Config>,
) -> kyomi_core::Result<String> {
    let self_hosted = config
        .map(|c| c.self_hosted)
        .unwrap_or_else(|| std::env::var("SELF_HOSTED").unwrap_or_default() == "true");

    let workspace_id = generate_workspace_id();
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);

    // Determine whether Stripe is available (SaaS mode with billing).
    let has_stripe = config
        .and_then(|c| c.stripe_secret_key.as_deref())
        .is_some_and(|k| !k.is_empty());

    // Self-hosted: team tier with active status (no trial, no billing).
    // SaaS with Stripe: cloud tier with 30-day trial and $1 AI credits.
    //   Stripe is the source of truth for subscription status — it will be
    //   updated after the Stripe subscription is created below.
    // SaaS without Stripe: cloud tier with 30-day trial (app-managed fallback), no credits.
    let (tier, status, subscription_status, trial_ends_at, ai_bundle_balance, user_limit) =
        if self_hosted {
            ("team", "active", "active", None, 0.0_f64, Some(999_999_i32))
        } else {
            let trial = chrono::Utc::now() + chrono::Duration::days(30);
            let credits = if has_stripe { 1.0 } else { 0.0 };
            ("cloud", "trial", "trialing", Some(trial), credits, None)
        };

    kyomi_core::db_execute!(
        pool,
        "INSERT INTO workspaces \
         (workspace_id, name, admin_email, owner_user_id, status, subscription_tier, \
          subscription_status, trial_ends_at, user_limit, ai_bundle_balance_usd) \
         VALUES ($1, NULL, $2, $3, $4, $5, $6, $7, $8, $9)",
        &workspace_id, user_email, user_id, status, tier,
        subscription_status, &trial_ends_at, &user_limit, ai_bundle_balance
    )?;

    // Add user as admin of the workspace
    let ws_user_sql = format!(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
         VALUES ($1, $2, 'workspace_admin', {bt})"
    );
    kyomi_core::db_execute!(pool, &ws_user_sql, &workspace_id, user_id)?;

    // Set as last workspace
    update_last_workspace(pool, user_id, &workspace_id).await?;

    // Create Stripe customer and subscription (SaaS mode only).
    // Non-blocking — if this fails, the user still gets an app-managed trial
    // (using trial_ends_at as fallback). The customer/subscription can be
    // created later when they visit the billing page.
    if let Some(config) = config
        && let Some(ref secret_key) = config.stripe_secret_key
        && !secret_key.is_empty()
    {
        let webhook_secret = config.stripe_webhook_secret.as_deref().unwrap_or_default();
        let stripe = crate::stripe_service::StripeService::new(secret_key, webhook_secret);
        match stripe.create_customer(user_email, &workspace_id, "New Workspace").await {
            Ok(customer_id) => {
                if let Err(e) = kyomi_core::db_execute!(
                    pool,
                    "UPDATE workspaces SET stripe_customer_id = $1 WHERE workspace_id = $2",
                    &customer_id, &workspace_id
                ) {
                    tracing::error!(
                        workspace_id = %workspace_id, error = %e,
                        "Failed to store Stripe customer ID"
                    );
                }

                // Create Stripe subscription with 30-day trial.
                // Stripe becomes the source of truth for subscription state.
                if let Some(price_id) = crate::stripe_config::get_cloud_price_id() {
                    match stripe
                        .create_subscription(&customer_id, price_id, 1, 30, &workspace_id)
                        .await
                    {
                        Ok(sub_data) => {
                            if let Err(e) = kyomi_core::db_execute!(
                                pool,
                                "UPDATE workspaces SET \
                                 stripe_subscription_id = $1, \
                                 subscription_status = $2, \
                                 subscription_period_start = $3, \
                                 subscription_period_end = $4 \
                                 WHERE workspace_id = $5",
                                &sub_data.stripe_subscription_id,
                                &sub_data.status,
                                sub_data.period_start,
                                sub_data.period_end,
                                &workspace_id
                            ) {
                                tracing::error!(
                                    workspace_id = %workspace_id, error = %e,
                                    "Failed to store Stripe subscription data"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                workspace_id = %workspace_id, error = %e,
                                "Failed to create Stripe subscription at signup"
                            );
                            // Don't fail signup — app-managed trial via trial_ends_at
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!(
                    workspace_id = %workspace_id, error = %e,
                    "Failed to create Stripe customer at signup"
                );
                // Don't fail signup — customer can be created later
            }
        }
    }

    Ok(workspace_id)
}

// ---------------------------------------------------------------------------
// OAuth data (encrypted JSON in users.oauth_data)
// ---------------------------------------------------------------------------

/// Update the user's encrypted oauth_data field.
///
/// The `encrypted_data` should be the output of `encryption::encrypt(json_string, key)`.
pub async fn update_user_oauth_data(
    pool: &DbPool,
    user_id: &str,
    encrypted_data: Option<&str>,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE users SET oauth_data = $1, updated_at = {now} WHERE user_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, encrypted_data, user_id)?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Terms of Service
// ---------------------------------------------------------------------------

/// Update terms acceptance for a user.
pub async fn update_terms_acceptance(
    pool: &DbPool,
    user_id: &str,
    terms_version: &str,
    marketing_consent: bool,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE users SET \
         terms_accepted_at = {now}, \
         terms_accepted_version = $1, \
         marketing_consent = $2, \
         updated_at = {now} \
         WHERE user_id = $3"
    );
    let result = kyomi_core::db_execute!(pool, &sql, terms_version, marketing_consent, user_id)?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Auth methods (user_auth_methods table)
// ---------------------------------------------------------------------------

/// Get an auth method for a user by type.
pub async fn get_auth_method(
    pool: &DbPool,
    user_id: &str,
    auth_type: &str,
) -> kyomi_core::Result<Option<UserAuthMethod>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT * FROM user_auth_methods \
         WHERE user_id = $1 AND auth_type = $2 AND active = {bt}"
    );
    let method = kyomi_core::db_fetch_optional!(pool, UserAuthMethod, &sql, user_id, auth_type)?;
    Ok(method)
}

/// Upsert an auth method (insert or update auth_data).
///
/// Uses ON CONFLICT with the unique constraint on (user_id, auth_type).
///
/// Postgres receives `&serde_json::Value` directly so sqlx binds it as `jsonb`,
/// preserving type safety.  SQLite receives a serialized JSON string.
pub async fn upsert_auth_method(
    pool: &DbPool,
    user_id: &str,
    auth_type: &str,
    auth_data: &serde_json::Value,
) -> kyomi_core::Result<()> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "INSERT INTO user_auth_methods (user_id, auth_type, auth_data, active) \
         VALUES ($1, $2, $3, {bt}) \
         ON CONFLICT (user_id, auth_type) \
         DO UPDATE SET auth_data = $3, last_used = {now}, active = {bt}"
    );

    match pool {
        DbPool::Postgres(pg) => {
            sqlx::query(&sql)
                .bind(user_id)
                .bind(auth_type)
                .bind(auth_data)
                .execute(pg)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("upsert_auth_method: {e}")))?;
        }
        DbPool::Sqlite(sq) => {
            let auth_data_str = serde_json::to_string(auth_data)
                .map_err(|e| kyomi_core::Error::Internal(format!("JSON serialization failed: {e}")))?;
            sqlx::query(&sql)
                .bind(user_id)
                .bind(auth_type)
                .bind(&auth_data_str)
                .execute(sq)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("upsert_auth_method: {e}")))?;
        }
    }
    Ok(())
}

/// Update the last_used timestamp on an auth method.
pub async fn touch_auth_method(
    pool: &DbPool,
    user_id: &str,
    auth_type: &str,
) -> kyomi_core::Result<()> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "UPDATE user_auth_methods SET last_used = {now} \
         WHERE user_id = $1 AND auth_type = $2 AND active = {bt}"
    );
    kyomi_core::db_execute!(pool, &sql, user_id, auth_type)?;
    Ok(())
}

/// Check if a user has TOTP (2FA) enabled.
pub async fn has_totp_enabled(pool: &DbPool, user_id: &str) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT COUNT(*) as count FROM user_auth_methods \
         WHERE user_id = $1 AND auth_type = 'totp' AND active = {bt}"
    );
    let count: i64 = kyomi_core::db_fetch_scalar!(pool, i64, &sql, user_id)?;
    Ok(count > 0)
}

/// Remove an auth method (set active = false).
pub async fn remove_auth_method(
    pool: &DbPool,
    user_id: &str,
    auth_type: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let bf = sql_compat::bool_false(is_pg);
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "UPDATE user_auth_methods SET active = {bf} \
         WHERE user_id = $1 AND auth_type = $2 AND active = {bt}"
    );
    let result = kyomi_core::db_execute!(pool, &sql, user_id, auth_type)?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Passkey-specific user lookups
// ---------------------------------------------------------------------------

/// Find a user by a WebAuthn credential_id (searches all webauthn auth methods).
///
/// This scans the `auth_data->'credentials'` JSON for a matching key.
/// Postgres uses the `?` jsonb operator; SQLite uses json_each to check keys.
pub async fn find_user_by_credential_id(
    pool: &DbPool,
    credential_id: &str,
) -> kyomi_core::Result<Option<User>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = if is_pg {
        format!(
            "SELECT u.* FROM users u \
             JOIN user_auth_methods am ON u.user_id = am.user_id \
             WHERE am.auth_type = 'webauthn' \
               AND am.active = {bt} \
               AND (am.auth_data->'credentials')::jsonb ? $1"
        )
    } else {
        format!(
            "SELECT u.* FROM users u \
             JOIN user_auth_methods am ON u.user_id = am.user_id \
             WHERE am.auth_type = 'webauthn' \
               AND am.active = {bt} \
               AND $1 IN (SELECT key FROM json_each(json_extract(am.auth_data, '$.credentials')))"
        )
    };
    let user = kyomi_core::db_fetch_optional!(pool, User, &sql, credential_id)?;
    Ok(user)
}

// ---------------------------------------------------------------------------
// Passkey credential management (stored in auth_data JSON)
// ---------------------------------------------------------------------------

/// Add a passkey credential to a user's webauthn auth method.
///
/// Creates the auth method if it doesn't exist. Stores credential in the
/// `auth_data.credentials` JSON map keyed by credential_id.
///
/// `passkey_json` is the serialized webauthn-rs `Passkey` for direct Rust use.
/// `public_key_b64` is the base64-encoded public key for Python compatibility.
pub async fn add_passkey_to_user(
    pool: &DbPool,
    user_id: &str,
    credential_id: &str,
    public_key_b64: &str,
    sign_count: u32,
    device_name: &str,
    passkey_json: &serde_json::Value,
) -> kyomi_core::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    let credential_entry = serde_json::json!({
        "public_key": public_key_b64,
        "sign_count": sign_count,
        "device_name": device_name,
        "created_at": now,
        "last_used": null,
        "passkey": passkey_json,
    });

    // Try to get existing webauthn auth method
    let existing = get_auth_method(pool, user_id, "webauthn").await?;

    match existing {
        Some(method) => {
            let mut auth_data = method.auth_data.clone();
            let credentials = auth_data
                .as_object_mut()
                .and_then(|o| o.get_mut("credentials"))
                .and_then(|c| c.as_object_mut());

            match credentials {
                Some(creds) => {
                    creds.insert(credential_id.to_string(), credential_entry);
                }
                None => {
                    let mut creds = serde_json::Map::new();
                    creds.insert(credential_id.to_string(), credential_entry);
                    auth_data["credentials"] = serde_json::Value::Object(creds);
                }
            }

            upsert_auth_method(pool, user_id, "webauthn", &auth_data).await?;
        }
        None => {
            let auth_data = serde_json::json!({
                "credentials": {
                    credential_id: credential_entry,
                },
                "created_at": now,
            });
            upsert_auth_method(pool, user_id, "webauthn", &auth_data).await?;
        }
    }

    Ok(())
}

/// Get all passkey credentials for a user as a map of credential_id -> data.
pub async fn get_passkey_credentials(
    pool: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<serde_json::Map<String, serde_json::Value>> {
    let method = get_auth_method(pool, user_id, "webauthn").await?;

    match method {
        Some(m) => {
            let creds = m.auth_data
                .get("credentials")
                .and_then(|c| c.as_object())
                .cloned()
                .unwrap_or_default();
            Ok(creds)
        }
        None => Ok(serde_json::Map::new()),
    }
}

/// Update a credential's sign_count and last_used after successful authentication.
pub async fn update_credential_usage(
    pool: &DbPool,
    user_id: &str,
    credential_id: &str,
    new_sign_count: u32,
    updated_passkey_json: &serde_json::Value,
) -> kyomi_core::Result<bool> {
    let method = get_auth_method(pool, user_id, "webauthn").await?;

    let Some(method) = method else {
        return Ok(false);
    };

    let mut auth_data = method.auth_data.clone();
    let credentials = auth_data
        .as_object_mut()
        .and_then(|o| o.get_mut("credentials"))
        .and_then(|c| c.as_object_mut());

    let Some(creds) = credentials else {
        return Ok(false);
    };

    let Some(cred) = creds.get_mut(credential_id).and_then(|c| c.as_object_mut()) else {
        return Ok(false);
    };

    cred.insert("sign_count".to_string(), serde_json::json!(new_sign_count));
    cred.insert("last_used".to_string(), serde_json::json!(chrono::Utc::now().to_rfc3339()));
    cred.insert("passkey".to_string(), updated_passkey_json.clone());

    upsert_auth_method(pool, user_id, "webauthn", &auth_data).await?;

    // Also touch the auth method last_used
    touch_auth_method(pool, user_id, "webauthn").await?;

    Ok(true)
}

/// Delete a passkey credential from a user's webauthn auth method.
///
/// Returns false if credential not found or would be the last passkey.
pub async fn delete_passkey_from_user(
    pool: &DbPool,
    user_id: &str,
    credential_id: &str,
) -> kyomi_core::Result<Option<&'static str>> {
    let method = get_auth_method(pool, user_id, "webauthn").await?;

    let Some(method) = method else {
        return Ok(Some("Passkey not found"));
    };

    let mut auth_data = method.auth_data.clone();
    let credentials = auth_data
        .as_object_mut()
        .and_then(|o| o.get_mut("credentials"))
        .and_then(|c| c.as_object_mut());

    let Some(creds) = credentials else {
        return Ok(Some("Passkey not found"));
    };

    if !creds.contains_key(credential_id) {
        return Ok(Some("Passkey not found"));
    }

    if creds.len() <= 1 {
        return Ok(Some("Cannot delete your only passkey. Add another passkey first."));
    }

    creds.remove(credential_id);
    upsert_auth_method(pool, user_id, "webauthn", &auth_data).await?;

    Ok(None)
}

/// Update the device name for a passkey credential.
///
/// Returns an error message if credential not found or validation fails.
pub async fn update_passkey_device_name(
    pool: &DbPool,
    user_id: &str,
    credential_id: &str,
    new_device_name: &str,
) -> kyomi_core::Result<bool> {
    let method = get_auth_method(pool, user_id, "webauthn").await?;

    let Some(method) = method else {
        return Ok(false);
    };

    let mut auth_data = method.auth_data.clone();
    let credentials = auth_data
        .as_object_mut()
        .and_then(|o| o.get_mut("credentials"))
        .and_then(|c| c.as_object_mut());

    let Some(creds) = credentials else {
        return Ok(false);
    };

    let Some(cred) = creds.get_mut(credential_id).and_then(|c| c.as_object_mut()) else {
        return Ok(false);
    };

    cred.insert("device_name".to_string(), serde_json::json!(new_device_name));
    upsert_auth_method(pool, user_id, "webauthn", &auth_data).await?;

    Ok(true)
}

/// Update extra_metadata JSON field for a user (merge semantics).
///
/// Postgres: uses `jsonb || jsonb` merge operator.
/// SQLite: uses `json_patch()` for merge semantics.
pub async fn update_extra_metadata(
    pool: &DbPool,
    user_id: &str,
    metadata: &serde_json::Value,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let metadata_str = serde_json::to_string(metadata)
        .map_err(|e| kyomi_core::Error::Internal(format!("JSON serialization failed: {e}")))?;

    let sql = if is_pg {
        format!(
            "UPDATE users SET \
             extra_metadata = (COALESCE(extra_metadata::jsonb, '{{}}'::jsonb) || $1::jsonb)::json, \
             updated_at = {now} \
             WHERE user_id = $2"
        )
    } else {
        format!(
            "UPDATE users SET \
             extra_metadata = json_patch(COALESCE(extra_metadata, '{{}}'), $1), \
             updated_at = {now} \
             WHERE user_id = $2"
        )
    };
    let result = kyomi_core::db_execute!(pool, &sql, &metadata_str, user_id)?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// ChartML config + knowledge (Phase 4B — user endpoints)
// ---------------------------------------------------------------------------

/// Update user's ChartML config JSON.
pub async fn update_chartml_config(
    pool: &DbPool,
    user_id: &str,
    config: &serde_json::Value,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE users SET chartml_config = $1, updated_at = {now} WHERE user_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, config, user_id)?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Chart palette lookup (KYO-129)
// ---------------------------------------------------------------------------

/// Extract the palette style name from a `chartml_config` JSON value.
///
/// Accepts both storage shapes that exist in production data:
///   - Flat (legacy REST writer):   `{"style": "vibrant", ...}`
///   - Nested (Leptos writer):      `{"config": {"style": "vibrant", ...}}`
///
/// Flat-first lookup matches the order used in the pre-KYO-129 browser
/// reader (`context.rs`) — both shapes are accepted so writers can change
/// shape without breaking existing readers. Returns None when the value
/// is missing a `style` string in either shape.
pub fn extract_palette_style(value: &serde_json::Value) -> Option<&str> {
    value
        .get("style")
        .or_else(|| value.get("config").and_then(|c| c.get("style")))
        .and_then(|s| s.as_str())
}

/// Look up the user's preferred chart palette name from
/// `users.chartml_config`.
///
/// Returns `"kyomi"` (the default) when the user has no stored preference,
/// when `chartml_config` is null, when the `style` field is missing in both
/// shapes, or when the DB query fails. The fallback behavior matches what
/// every pre-KYO-129 reader did independently.
pub async fn get_user_palette_name(pool: &DbPool, user_id: &str) -> String {
    const DEFAULT_PALETTE: &str = "kyomi";

    #[derive(sqlx::FromRow)]
    struct Row {
        chartml_config: Option<serde_json::Value>,
    }

    match kyomi_core::db_fetch_optional!(
        pool, Row,
        "SELECT chartml_config FROM users WHERE user_id = $1",
        user_id
    ) {
        Ok(Some(row)) => row
            .chartml_config
            .as_ref()
            .and_then(extract_palette_style)
            .map(String::from)
            .unwrap_or_else(|| DEFAULT_PALETTE.to_string()),
        Ok(None) => {
            tracing::warn!(user_id = %user_id, "User not found when loading palette preference");
            DEFAULT_PALETTE.to_string()
        }
        Err(e) => {
            tracing::warn!(error = %e, user_id = %user_id, "Failed to query user palette preference");
            DEFAULT_PALETTE.to_string()
        }
    }
}

/// Update user's knowledge text.
pub async fn update_knowledge(
    pool: &DbPool,
    user_id: &str,
    knowledge: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE users SET knowledge = $1, updated_at = {now} WHERE user_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, knowledge, user_id)?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// API tokens (Phase 4B — user endpoints)
// ---------------------------------------------------------------------------

/// Generate a token_id matching Python's format: `"tok-{uuid4()}"`.
fn generate_token_id() -> String {
    format!("tok-{}", uuid::Uuid::new_v4())
}

/// Create an API token. Returns `(token_id, token_plaintext)`.
///
/// The raw token is format `kyomi-{random_hex_32}`.
/// Only the SHA-256 hash is stored — the plaintext is returned once.
pub async fn create_api_token(
    pool: &DbPool,
    user_id: &str,
    name: &str,
    expires_days: Option<i32>,
    created_by: &str,
) -> kyomi_core::Result<(String, String)> {
    let token_id = generate_token_id();
    let random_bytes: [u8; 32] = rand::rng().random();
    let token_plaintext = format!("kyomi-{}", format_hex(&random_bytes));

    let mut hasher = Sha256::new();
    hasher.update(token_plaintext.as_bytes());
    let token_hash = format!("{:x}", hasher.finalize());

    let expires_at = expires_days.map(|days| {
        chrono::Utc::now() + chrono::Duration::days(days as i64)
    });

    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "INSERT INTO api_tokens (token_id, user_id, name, token_hash, active, expires_at, created_by) \
         VALUES ($1, $2, $3, $4, {bt}, $5, $6)"
    );
    kyomi_core::db_execute!(
        pool, &sql,
        &token_id, user_id, name, &token_hash, &expires_at, created_by
    )?;

    Ok((token_id, token_plaintext))
}

/// Format bytes as a lowercase hex string.
fn format_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Get API tokens for a user (active and revoked).
pub async fn get_user_api_tokens(
    pool: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<ApiToken>> {
    let tokens = kyomi_core::db_fetch_all!(
        pool, ApiToken,
        "SELECT * FROM api_tokens WHERE user_id = $1 ORDER BY created_at DESC",
        user_id
    )?;
    Ok(tokens)
}

/// Revoke an API token (set active = false, record revoked_by).
pub async fn revoke_api_token(
    pool: &DbPool,
    token_id: &str,
    revoked_by: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bf = sql_compat::bool_false(is_pg);
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "UPDATE api_tokens SET active = {bf}, revoked_at = {now}, revoked_by = $1 \
         WHERE token_id = $2 AND active = {bt}"
    );
    let result = kyomi_core::db_execute!(pool, &sql, revoked_by, token_id)?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Admin user management (Phase 4B — user endpoints)
// ---------------------------------------------------------------------------

/// List all users (admin).
pub async fn list_all_users(pool: &DbPool) -> kyomi_core::Result<Vec<User>> {
    let users = kyomi_core::db_fetch_all!(
        pool, User,
        "SELECT * FROM users ORDER BY created_at DESC"
    )?;
    Ok(users)
}

/// Admin: create a user with specified roles (no password).
///
/// The admin creates the account; the user sets their password later.
/// This wraps `create_user` and sets roles via `extra_metadata`.
pub async fn admin_create_user(
    pool: &DbPool,
    email: &str,
    name: &str,
    roles: &[String],
) -> kyomi_core::Result<User> {
    // Create the user (verified = false by default for admin-created users
    // actually the Python sets require_verification=False which means verified=true)
    let user = create_user(pool, email, Some(name), true).await?;

    // Set roles in extra_metadata
    let metadata = serde_json::json!({ "roles": roles });
    update_extra_metadata(pool, &user.user_id, &metadata).await?;

    // Re-fetch to get updated extra_metadata
    get_user_by_id(pool, &user.user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("User created but not found".into()))
}

/// Admin: update user fields (name, email, active status).
///
/// Only updates provided fields (None = no change).
pub async fn admin_update_user(
    pool: &DbPool,
    user_id: &str,
    name: Option<&str>,
    active: Option<bool>,
    roles: Option<&[String]>,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);

    // Update name if provided
    if let Some(name) = name {
        let sql = format!(
            "UPDATE users SET name = $1, updated_at = {now} WHERE user_id = $2"
        );
        kyomi_core::db_execute!(pool, &sql, name, user_id)?;
    }

    // Update active status if provided
    if let Some(active) = active {
        let sql = format!(
            "UPDATE users SET active = $1, updated_at = {now} WHERE user_id = $2"
        );
        kyomi_core::db_execute!(pool, &sql, active, user_id)?;
    }

    // Update roles if provided
    if let Some(roles) = roles {
        let metadata = serde_json::json!({ "roles": roles });
        update_extra_metadata(pool, user_id, &metadata).await?;
    }

    Ok(true)
}

/// Delete a user from the database.
pub async fn delete_user(pool: &DbPool, user_id: &str) -> kyomi_core::Result<bool> {
    let result = kyomi_core::db_execute!(
        pool, "DELETE FROM users WHERE user_id = $1", user_id
    )?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_palette_style_flat() {
        let v = json!({"type": "config", "version": 1, "style": "vibrant"});
        assert_eq!(extract_palette_style(&v), Some("vibrant"));
    }

    #[test]
    fn extract_palette_style_nested() {
        let v = json!({"config": {"type": "config", "version": 1, "style": "balanced"}});
        assert_eq!(extract_palette_style(&v), Some("balanced"));
    }

    #[test]
    fn extract_palette_style_prefers_flat_over_nested_when_both_present() {
        // Defensive: if a future writer accidentally emits both, flat wins,
        // mirroring the pre-KYO-129 reader order.
        let v = json!({"style": "vibrant", "config": {"style": "balanced"}});
        assert_eq!(extract_palette_style(&v), Some("vibrant"));
    }

    #[test]
    fn extract_palette_style_missing_returns_none() {
        let v = json!({"type": "config", "version": 1});
        assert_eq!(extract_palette_style(&v), None);
    }

    #[test]
    fn extract_palette_style_empty_object_returns_none() {
        assert_eq!(extract_palette_style(&json!({})), None);
    }

    #[test]
    fn extract_palette_style_null_returns_none() {
        assert_eq!(extract_palette_style(&json!(null)), None);
    }

    #[test]
    fn extract_palette_style_non_object_returns_none() {
        // Malformed data — e.g. a bare string accidentally stored.
        assert_eq!(extract_palette_style(&json!("vibrant")), None);
        assert_eq!(extract_palette_style(&json!(42)), None);
        assert_eq!(extract_palette_style(&json!([1, 2, 3])), None);
    }

    #[test]
    fn extract_palette_style_style_not_a_string_returns_none() {
        // If `style` is present but isn't a string, we should ignore it.
        let v = json!({"style": 42});
        assert_eq!(extract_palette_style(&v), None);
    }
}
