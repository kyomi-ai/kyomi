// SPDX-License-Identifier: AGPL-3.0-or-later

//! Token service — refresh token and verification token operations.
//!
//! Refresh tokens: stored as SHA-256 hash of opaque `rt_<base64url>` value.
//! Verification tokens: stored as bcrypt hash.
//!
//! ## Token Rotation
//!
//! Each login creates a "token family" (shared `family_id`). On refresh,
//! the old token is marked as replaced (`replaced_at = NOW()`) and a new
//! token is created in the same family. A 30-second grace period allows
//! multi-tab race conditions. If an old token is used after the grace
//! period, the entire family is revoked (theft detection).

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use kyomi_core::models::VerificationToken;
use kyomi_core::sql_compat;
use kyomi_core::DbPool;
use rand::Rng;
use sha2::{Digest, Sha256};

/// Device info extracted from the HTTP request.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub country_code: Option<String>,
    pub oauth_client_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Refresh token verification result
// ---------------------------------------------------------------------------

/// Result of verifying a refresh token, accounting for rotation state.
#[derive(Debug)]
pub enum RefreshTokenVerifyResult {
    /// Token is valid and current (not replaced). Caller should rotate it.
    Valid(RefreshTokenUserData),
    /// Token was replaced but is within the grace period. Caller should
    /// rotate again so each tab gets a fresh token in its cookie.
    GracePeriod(RefreshTokenUserData),
    /// Token was replaced AND used after the grace period expired.
    /// This indicates theft — the entire family has been revoked.
    TheftDetected {
        family_id: String,
        user_id: String,
    },
    /// Token not found, expired, or revoked.
    Invalid,
}

// ---------------------------------------------------------------------------
// Refresh token operations
// ---------------------------------------------------------------------------

/// Hash a raw refresh token using SHA-256 for storage/lookup.
pub fn hash_refresh_token(raw_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Generate a new family_id for a login session.
pub fn generate_family_id() -> String {
    let random_bytes: [u8; 16] = rand::rng().random();
    format!("fam_{}", URL_SAFE_NO_PAD.encode(random_bytes))
}

/// Store a new refresh token in the database.
///
/// Returns the token_id (format: `"rt_{token_urlsafe(16)}"`).
pub async fn store_refresh_token(
    pool: &DbPool,
    user_id: &str,
    token_hash: &str,
    expires_at: DateTime<Utc>,
    device_info: &DeviceInfo,
    family_id: &str,
) -> kyomi_core::Result<String> {
    let random_bytes: [u8; 16] = rand::rng().random();
    let token_id = format!("rt_{}", URL_SAFE_NO_PAD.encode(random_bytes));

    kyomi_core::db_execute!(
        pool,
        "INSERT INTO refresh_tokens \
         (token_id, user_id, token_hash, expires_at, user_agent, ip_address, country_code, family_id, oauth_client_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        &token_id, user_id, token_hash, &expires_at,
        &device_info.user_agent, &device_info.ip_address,
        &device_info.country_code, family_id, &device_info.oauth_client_id
    )?;

    Ok(token_id)
}

/// Verify a raw refresh token, handling rotation grace period and theft detection.
pub async fn verify_refresh_token(
    pool: &DbPool,
    raw_token: &str,
) -> kyomi_core::Result<RefreshTokenVerifyResult> {
    let token_hash = hash_refresh_token(raw_token);
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bt = sql_compat::bool_true(is_pg);

    // Fetch the token + user data in one query (include replaced_at and family_id)
    let sql = format!(
        "SELECT rt.token_id, rt.user_id, rt.family_id, rt.replaced_at, \
                u.email, u.name, u.extra_metadata, u.active as user_active \
         FROM refresh_tokens rt \
         JOIN users u ON u.user_id = rt.user_id \
         WHERE rt.token_hash = $1 \
           AND rt.is_active = {bt} \
           AND rt.expires_at > {now}"
    );
    let row = kyomi_core::db_fetch_optional!(
        pool, RefreshTokenWithUser, &sql, &token_hash
    )?;

    let Some(row) = row else {
        return Ok(RefreshTokenVerifyResult::Invalid);
    };

    if !row.user_active {
        return Ok(RefreshTokenVerifyResult::Invalid);
    }

    // Extract roles from extra_metadata
    let roles = row.extra_metadata
        .as_ref()
        .and_then(|m| m.get("roles"))
        .and_then(|r| serde_json::from_value::<Vec<String>>(r.clone()).ok())
        .unwrap_or_else(|| vec!["user".to_string()]);

    let user_data = RefreshTokenUserData {
        user_id: row.user_id.clone(),
        email: row.email,
        name: row.name,
        roles,
        token_id: row.token_id,
        family_id: row.family_id.clone(),
    };

    // Check if this token has been replaced (rotated)
    if let Some(replaced_at) = row.replaced_at {
        let grace_seconds = kyomi_core::constants::get().jwt.refresh_token_grace_period_seconds;
        let grace_deadline = replaced_at + Duration::seconds(grace_seconds);

        if Utc::now() <= grace_deadline {
            // Within grace period — allow but don't rotate again
            // Update last_used
            let update_sql = format!(
                "UPDATE refresh_tokens SET last_used = {now} WHERE token_id = $1"
            );
            kyomi_core::db_execute!(pool, &update_sql, &user_data.token_id)?;

            return Ok(RefreshTokenVerifyResult::GracePeriod(user_data));
        } else {
            // Past grace period — theft detected! Revoke entire family.
            tracing::warn!(
                family_id = %row.family_id,
                user_id = %row.user_id,
                token_id = %user_data.token_id,
                "refresh token reuse after grace period — revoking token family (theft detection)"
            );
            revoke_token_family(pool, &row.family_id).await?;
            return Ok(RefreshTokenVerifyResult::TheftDetected {
                family_id: row.family_id,
                user_id: row.user_id,
            });
        }
    }

    // Token is current (not replaced) — update last_used
    let update_sql = format!(
        "UPDATE refresh_tokens SET last_used = {now} WHERE token_id = $1"
    );
    kyomi_core::db_execute!(pool, &update_sql, &user_data.token_id)?;

    Ok(RefreshTokenVerifyResult::Valid(user_data))
}

/// Rotate a refresh token: mark the old one as replaced, create a new one in the same family.
///
/// Returns the new token_id.
pub async fn rotate_refresh_token(
    pool: &DbPool,
    old_token_id: &str,
    user_id: &str,
    family_id: &str,
    new_token_hash: &str,
    expires_at: DateTime<Utc>,
    device_info: &DeviceInfo,
) -> kyomi_core::Result<String> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);

    // Mark the old token as replaced
    let replace_sql = format!(
        "UPDATE refresh_tokens SET replaced_at = {now} WHERE token_id = $1"
    );
    kyomi_core::db_execute!(pool, &replace_sql, old_token_id)?;

    // Create the new token in the same family
    let random_bytes: [u8; 16] = rand::rng().random();
    let new_token_id = format!("rt_{}", URL_SAFE_NO_PAD.encode(random_bytes));

    kyomi_core::db_execute!(
        pool,
        "INSERT INTO refresh_tokens \
         (token_id, user_id, token_hash, expires_at, user_agent, ip_address, country_code, family_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        &new_token_id, user_id, new_token_hash, &expires_at,
        &device_info.user_agent, &device_info.ip_address,
        &device_info.country_code, family_id
    )?;

    Ok(new_token_id)
}

/// Revoke all tokens in a family (theft detection or logout).
pub async fn revoke_token_family(
    pool: &DbPool,
    family_id: &str,
) -> kyomi_core::Result<u64> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bf = sql_compat::bool_false(is_pg);
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "UPDATE refresh_tokens \
         SET is_active = {bf}, revoked_at = {now} \
         WHERE family_id = $1 AND is_active = {bt}"
    );
    let result = kyomi_core::db_execute!(pool, &sql, family_id)?;
    Ok(result.rows_affected())
}

/// Revoke a specific refresh token.
pub async fn revoke_refresh_token(
    pool: &DbPool,
    token_id: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bf = sql_compat::bool_false(is_pg);
    let sql = format!(
        "UPDATE refresh_tokens \
         SET is_active = {bf}, revoked_at = {now} \
         WHERE token_id = $1"
    );
    let result = kyomi_core::db_execute!(pool, &sql, token_id)?;
    Ok(result.rows_affected() > 0)
}

/// Revoke a specific refresh token owned by a user — revokes the entire family.
///
/// Looks up the token's family_id, then revokes all tokens in that family.
pub async fn revoke_user_refresh_token(
    pool: &DbPool,
    user_id: &str,
    token_id: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT family_id FROM refresh_tokens \
         WHERE token_id = $1 AND user_id = $2 AND is_active = {bt}"
    );

    // Use a helper struct for the scalar-like query
    #[derive(sqlx::FromRow)]
    struct FamilyIdRow {
        family_id: String,
    }

    let row = kyomi_core::db_fetch_optional!(pool, FamilyIdRow, &sql, token_id, user_id)?;

    let Some(row) = row else {
        return Ok(false);
    };

    // Revoke the entire family
    let count = revoke_token_family(pool, &row.family_id).await?;
    Ok(count > 0)
}

/// Revoke ALL refresh tokens for a user. Returns count of revoked tokens.
pub async fn revoke_all_user_refresh_tokens(
    pool: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<u64> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bf = sql_compat::bool_false(is_pg);
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "UPDATE refresh_tokens \
         SET is_active = {bf}, revoked_at = {now} \
         WHERE user_id = $1 AND is_active = {bt}"
    );
    let result = kyomi_core::db_execute!(pool, &sql, user_id)?;
    Ok(result.rows_affected())
}

/// Get all active sessions (refresh tokens) for a user.
///
/// Only returns the *current* token per family (where `replaced_at IS NULL`),
/// so rotated-away tokens don't clutter the session list.
///
/// Postgres uses `DISTINCT ON`; SQLite uses a subquery with GROUP BY.
pub async fn get_user_sessions(
    pool: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<SessionInfo>> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bt = sql_compat::bool_true(is_pg);

    let sql = if is_pg {
        format!(
            "SELECT s.token_id, s.token_hash, s.family_id, s.created_at, s.last_used, \
                    s.expires_at, s.user_agent, s.ip_address, s.country_code, \
                    s.oauth_client_name \
             FROM ( \
                 SELECT DISTINCT ON (rt.family_id) \
                        rt.token_id, rt.token_hash, rt.family_id, rt.created_at, rt.last_used, \
                        rt.expires_at, rt.user_agent, rt.ip_address, rt.country_code, \
                        oc.name AS oauth_client_name \
                 FROM refresh_tokens rt \
                 LEFT JOIN oauth_clients oc ON rt.oauth_client_id = oc.client_id \
                 WHERE rt.user_id = $1 AND rt.is_active = {bt} AND rt.expires_at > {now} \
                       AND rt.replaced_at IS NULL \
                 ORDER BY rt.family_id, rt.created_at DESC \
             ) s \
             ORDER BY s.last_used DESC NULLS LAST"
        )
    } else {
        // SQLite: replaced_at IS NULL already gives us one token per family
        // (the current non-replaced token). No DISTINCT ON needed.
        format!(
            "SELECT rt.token_id, rt.token_hash, rt.family_id, rt.created_at, rt.last_used, \
                    rt.expires_at, rt.user_agent, rt.ip_address, rt.country_code, \
                    oc.name AS oauth_client_name \
             FROM refresh_tokens rt \
             LEFT JOIN oauth_clients oc ON rt.oauth_client_id = oc.client_id \
             WHERE rt.user_id = $1 AND rt.is_active = {bt} AND rt.expires_at > {now} \
                   AND rt.replaced_at IS NULL \
             ORDER BY COALESCE(rt.last_used, '1970-01-01') DESC"
        )
    };

    let sessions = kyomi_core::db_fetch_all!(pool, SessionInfo, &sql, user_id)?;
    Ok(sessions)
}

// ---------------------------------------------------------------------------
// Verification token operations
// ---------------------------------------------------------------------------

/// Create a new email verification token.
///
/// Returns the raw (unhashed) token value for inclusion in the verification link.
///
/// `expire_hours` overrides the default from constants.toml — used for short-lived
/// recovery tokens (15 min = 0.25 hours). Pass `None` for the default.
pub async fn create_verification_token(
    pool: &DbPool,
    email: &str,
    token_type: &str,
) -> kyomi_core::Result<String> {
    create_verification_token_with_expiry(pool, email, token_type, None).await
}

/// Create a verification token with custom expiry (in hours).
pub async fn create_verification_token_with_expiry(
    pool: &DbPool,
    email: &str,
    token_type: &str,
    expire_hours: Option<f64>,
) -> kyomi_core::Result<String> {
    let random_bytes: [u8; 32] = rand::rng().random();
    let raw_token = URL_SAFE_NO_PAD.encode(random_bytes);

    // Hash with bcrypt (verification tokens are low-entropy, need slow hash)
    let token_hash = bcrypt::hash(&raw_token, bcrypt::DEFAULT_COST)
        .map_err(|e| kyomi_core::Error::Internal(format!("bcrypt hash failed: {e}")))?;

    let id_bytes: [u8; 16] = rand::rng().random();
    let token_id = format!("vt_{}", URL_SAFE_NO_PAD.encode(id_bytes));

    let default_hours = kyomi_core::constants::get().jwt.email_verification_expire_hours as f64;
    let hours = expire_hours.unwrap_or(default_hours);
    let expires_at = Utc::now() + Duration::seconds((hours * 3600.0) as i64);

    kyomi_core::db_execute!(
        pool,
        "INSERT INTO verification_tokens \
         (token_id, email, token_hash, token_type, expires_at) \
         VALUES ($1, $2, $3, $4, $5)",
        &token_id, email, &token_hash, token_type, &expires_at
    )?;

    Ok(raw_token)
}

/// Verify an email verification token.
///
/// Checks all unused, unexpired tokens of the given type and compares with bcrypt.
/// If valid, marks the token as used and returns the associated email.
pub async fn verify_verification_token(
    pool: &DbPool,
    raw_token: &str,
    token_type: &str,
) -> kyomi_core::Result<Option<String>> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bf = sql_compat::bool_false(is_pg);
    let bt = sql_compat::bool_true(is_pg);

    let fetch_sql = format!(
        "SELECT * FROM verification_tokens \
         WHERE token_type = $1 AND used = {bf} AND expires_at > {now}"
    );
    let tokens = kyomi_core::db_fetch_all!(pool, VerificationToken, &fetch_sql, token_type)?;

    for db_token in tokens {
        let is_match = bcrypt::verify(raw_token, &db_token.token_hash)
            .unwrap_or(false);

        if is_match {
            // Mark as used
            let update_sql = format!(
                "UPDATE verification_tokens \
                 SET used = {bt}, used_at = {now} \
                 WHERE token_id = $1"
            );
            kyomi_core::db_execute!(pool, &update_sql, &db_token.token_id)?;

            return Ok(Some(db_token.email));
        }
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// Helper types
// ---------------------------------------------------------------------------

/// User data returned by refresh token verification.
#[derive(Debug, Clone)]
pub struct RefreshTokenUserData {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub roles: Vec<String>,
    pub token_id: String,
    pub family_id: String,
}

/// Internal query result for refresh token + user join.
#[derive(Debug, sqlx::FromRow)]
struct RefreshTokenWithUser {
    token_id: String,
    user_id: String,
    family_id: String,
    replaced_at: Option<DateTime<Utc>>,
    email: String,
    name: Option<String>,
    extra_metadata: Option<serde_json::Value>,
    user_active: bool,
}

/// Session info returned to the frontend.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct SessionInfo {
    pub token_id: String,
    /// Used internally for `is_current` comparison — never serialized.
    #[serde(skip)]
    pub token_hash: String,
    /// Family ID for this session — used for current-session comparison.
    #[serde(skip)]
    pub family_id: String,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub country_code: Option<String>,
    pub oauth_client_name: Option<String>,
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::{seed_user, sqlite_pool, test_pool};

    // ─── family_id NOT NULL constraint (KYO-294) ─────────────────────────────
    //
    // Regression coverage for the SQLite/Postgres divergence
    // schema-parity-allowlist.toml tracked before
    // migrations-sqlite/00034_refresh_tokens_family_id_not_null.sql: SQLite's
    // `refresh_tokens.family_id` was nullable (no NOT NULL step after 00003's
    // backfill), so an INSERT that omitted family_id silently succeeded on
    // self-hosted while the same INSERT already failed loudly on Postgres
    // (20260216121703_add_refresh_token_rotation.sql's `SET NOT NULL`).
    // Confirmed by hand against the pre-00034 schema before writing these:
    // the sqlite insert below succeeds silently (storing family_id = NULL)
    // without the fix, and is rejected with it.
    //
    // These bypass `store_refresh_token` (which always supplies family_id)
    // and insert raw SQL directly, the same way a bad migration, import
    // script, or future callsite could.

    fn test_device_info() -> DeviceInfo {
        DeviceInfo {
            user_agent: Some("test-agent".to_string()),
            ip_address: Some("127.0.0.1".to_string()),
            country_code: None,
            oauth_client_id: None,
        }
    }

    #[tokio::test]
    async fn sqlite_insert_omitting_family_id_fails() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "user-1", "user1@example.com").await;

        let result = sqlx::query(
            "INSERT INTO refresh_tokens (token_id, user_id, token_hash, expires_at) \
             VALUES ('rt-no-family', 'user-1', 'hash-1', '2030-01-01T00:00:00Z')",
        )
        .execute(sq)
        .await;

        assert!(
            result.is_err(),
            "omitting family_id must be rejected by the NOT NULL constraint \
             (no default), got: {result:?}"
        );
    }

    #[tokio::test]
    async fn postgres_insert_omitting_family_id_fails() {
        let test_name = "postgres_insert_omitting_family_id_fails";
        let Some(db) = crate::test_pg::postgres_test_pool_or_skip(test_name).await else {
            return;
        };
        let pg = crate::test_pg::postgres_pool(&db);

        let user_id = crate::test_pg::unique_test_id("user");
        crate::test_pg::seed_user_pg(pg, &user_id, &format!("{user_id}@example.com")).await;

        let token_id = crate::test_pg::unique_test_id("rt");
        let result = sqlx::query(
            "INSERT INTO refresh_tokens (token_id, user_id, token_hash, expires_at) \
             VALUES ($1, $2, 'hash-1', now() + interval '30 days')",
        )
        .bind(&token_id)
        .bind(&user_id)
        .execute(pg)
        .await;

        assert!(
            result.is_err(),
            "omitting family_id must be rejected by Postgres's NOT NULL constraint, got: {result:?}"
        );

        sqlx::query("DELETE FROM users WHERE user_id = $1")
            .bind(&user_id)
            .execute(pg)
            .await
            .expect("cleanup user (postgres)");
    }

    // ─── Rotation + family revocation end-to-end (KYO-294) ───────────────────
    //
    // Confirms rotation and family-based revocation still work on SQLite
    // with the 00034 migration applied — nothing about the NOT NULL
    // constraint changes these code paths, since both insert sites already
    // bound family_id unconditionally, but this is the regression guard
    // that would catch it if that stopped being true.

    #[tokio::test]
    async fn rotation_and_family_revocation_end_to_end_on_sqlite() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "user-1", "user1@example.com").await;

        let device_info = test_device_info();
        let expires_at = Utc::now() + Duration::days(30);
        let family_id = generate_family_id();

        // 1. Store the initial token in a new family.
        let raw_token_1 = "rt_initial_raw_token_value";
        let token_hash_1 = hash_refresh_token(raw_token_1);
        let token_id_1 = store_refresh_token(
            &db, "user-1", &token_hash_1, expires_at, &device_info, &family_id,
        )
        .await
        .expect("store initial refresh token");

        // 2. Verifying the fresh token returns Valid, with the family_id we
        //    supplied at insert.
        match verify_refresh_token(&db, raw_token_1).await.expect("verify initial token") {
            RefreshTokenVerifyResult::Valid(user_data) => {
                assert_eq!(user_data.token_id, token_id_1);
                assert_eq!(user_data.family_id, family_id);
                assert_eq!(user_data.user_id, "user-1");
            }
            other => panic!("expected Valid, got a different variant: {other:?}"),
        }

        // 3. Rotate: old token marked replaced, new token created in the
        //    same family.
        let raw_token_2 = "rt_rotated_raw_token_value";
        let token_hash_2 = hash_refresh_token(raw_token_2);
        let token_id_2 = rotate_refresh_token(
            &db, &token_id_1, "user-1", &family_id, &token_hash_2, expires_at, &device_info,
        )
        .await
        .expect("rotate refresh token");
        assert_ne!(token_id_2, token_id_1);

        // 4. The old token is now within its grace period — GracePeriod, not
        //    Invalid.
        match verify_refresh_token(&db, raw_token_1).await.expect("verify replaced token") {
            RefreshTokenVerifyResult::GracePeriod(user_data) => {
                assert_eq!(user_data.family_id, family_id);
            }
            other => panic!("expected GracePeriod, got a different variant: {other:?}"),
        }

        // 5. The new token verifies as Valid.
        match verify_refresh_token(&db, raw_token_2).await.expect("verify rotated token") {
            RefreshTokenVerifyResult::Valid(user_data) => {
                assert_eq!(user_data.token_id, token_id_2);
                assert_eq!(user_data.family_id, family_id);
            }
            other => panic!("expected Valid, got a different variant: {other:?}"),
        }

        // 6. Revoke the whole family — both tokens (replaced + current) are
        //    active rows in this family, so both are revoked.
        let revoked_count = revoke_token_family(&db, &family_id).await.expect("revoke family");
        assert_eq!(revoked_count, 2, "both tokens in the family must be revoked");

        // 7. The previously-valid rotated token must now fail verification.
        match verify_refresh_token(&db, raw_token_2).await.expect("verify after revocation") {
            RefreshTokenVerifyResult::Invalid => {}
            other => panic!("expected Invalid after family revocation, got: {other:?}"),
        }
    }
}
