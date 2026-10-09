// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared refresh-token flow used by both the explicit `POST /auth/refresh`
//! REST endpoint and the transparent auto-refresh middleware.
//!
//! This module encapsulates the pure token-minting + rotation logic so that
//! callers can wrap it with whatever HTTP concerns they need (rate limiting,
//! cookie setting, JSON body assembly) without duplicating the core flow.

use std::collections::HashMap;

use crate::{
    jwt, token_service,
    token_service::{DeviceInfo, RefreshTokenVerifyResult},
    user_service,
};

/// Result of a successful refresh: new tokens + user context for downstream
/// response assembly (cookies, JSON body, etc.).
pub struct RefreshedTokens {
    pub access_token: String,
    pub raw_refresh_token: String,
    pub access_expires_in_secs: i64,
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub roles: Vec<String>,
}

/// Attempt to mint a new access token and rotate the refresh token.
///
/// Handles grace-period and theft detection via
/// [`token_service::verify_refresh_token`]. Returns `Err` on verification
/// failure, theft detection, or database errors.
///
/// Does NOT perform rate limiting — callers layer that on top if they want it.
/// Does NOT set cookies — callers assemble the HTTP response themselves.
pub async fn refresh_tokens(
    db: &kyomi_core::DbPool,
    jwt_secret: &str,
    refresh_token_value: &str,
    device: &DeviceInfo,
) -> kyomi_core::Result<RefreshedTokens> {
    // Verify refresh token (handles grace period + theft detection)
    let verify_result = token_service::verify_refresh_token(db, refresh_token_value).await?;

    let user_data = match verify_result {
        RefreshTokenVerifyResult::Valid(data) | RefreshTokenVerifyResult::GracePeriod(data) => data,
        RefreshTokenVerifyResult::TheftDetected { .. } => {
            return Err(kyomi_core::Error::Unauthorized(
                "Refresh token has been revoked (possible token theft detected)".into(),
            ));
        }
        RefreshTokenVerifyResult::Invalid => {
            return Err(kyomi_core::Error::Unauthorized(
                "Invalid or expired refresh token".into(),
            ));
        }
    };

    if user_data.oauth_client_id.is_some() {
        return Err(kyomi_core::Error::Unauthorized(
            "OAuth client refresh token cannot refresh a browser session".into(),
        ));
    }

    refresh_verified_tokens(db, jwt_secret, user_data, device).await
}

// Verification is deliberately separate from the transactional rotation: the
// latter rechecks revocation under lock, regardless of when verification ran.
async fn refresh_verified_tokens(
    db: &kyomi_core::DbPool,
    jwt_secret: &str,
    user_data: token_service::RefreshTokenUserData,
    device: &DeviceInfo,
) -> kyomi_core::Result<RefreshedTokens> {
    // Build JWT claims — mirrors POST /auth/refresh
    let mut extra = HashMap::new();
    extra.insert("user_id".into(), serde_json::json!(user_data.user_id));
    extra.insert("email".into(), serde_json::json!(user_data.email));
    extra.insert("name".into(), serde_json::json!(user_data.name));
    extra.insert("roles".into(), serde_json::json!(user_data.roles));

    // Load workspace context (same behaviour as the REST handler: best-effort)
    if let Ok(Some((ws, wu))) =
        user_service::get_user_workspace_context(db, &user_data.user_id).await
    {
        extra.insert("workspace_id".into(), serde_json::json!(ws.workspace_id));
        extra.insert("workspace_roles".into(), serde_json::json!(vec![wu.role]));
    }

    let jwt_config = &kyomi_core::constants::get().jwt;
    // Read fresh state before minting; rotation rechecks the token under the
    // same user lock used by revocation, after this access token is signed.
    let user = user_service::get_user_by_id(db, &user_data.user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Unauthorized("User not found".into()))?;
    let new_access_token = jwt::create_session_access_token_str(
        &user_data.user_id,
        jwt_secret,
        jwt_config.access_token_expire_minutes,
        extra,
        user.sessions_valid_from,
    )?;

    // Always rotate: every tab gets a fresh token (prevents multi-tab sign-out bug)
    let new_raw_refresh = jwt::create_refresh_token();
    let new_token_hash = token_service::hash_refresh_token(&new_raw_refresh);
    let expires_at =
        chrono::Utc::now() + chrono::Duration::days(jwt_config.refresh_token_expire_days);

    token_service::rotate_refresh_token(
        db,
        &user_data.token_id,
        &user_data.user_id,
        &user_data.family_id,
        &new_token_hash,
        expires_at,
        device,
    )
    .await?;

    Ok(RefreshedTokens {
        access_token: new_access_token,
        raw_refresh_token: new_raw_refresh,
        access_expires_in_secs: jwt_config.access_token_expire_minutes * 60,
        user_id: user_data.user_id,
        email: user_data.email,
        name: user_data.name,
        roles: user_data.roles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::authenticate_session;
    const SECRET: &str = "cutoff-race-test-secret";

    async fn controlled_refresh_race(db: kyomi_core::DbPool, tag: &str) {
        let user = user_service::create_user(&db, &format!("{tag}@test.local"), None, true)
            .await
            .unwrap();
        let kv = kyomi_core::kv_store_memory::InMemoryKVStore::new_pool();
        let device = DeviceInfo {
            user_agent: None,
            ip_address: None,
            country_code: None,
            oauth_client_id: None,
        };
        let session =
            crate::session::create_authenticated_session(&db, &kv, SECRET, &user, &device)
                .await
                .unwrap();
        let (verified_tx, verified_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let race_db = db.clone();
        let race_device = device.clone();
        let refresh = session.refresh_token.clone();
        // The real flow's verification/rotation boundary is held explicitly.
        let worker = tokio::spawn(async move {
            let result = token_service::verify_refresh_token(&race_db, &refresh)
                .await
                .unwrap();
            let RefreshTokenVerifyResult::Valid(data) = result else {
                panic!("valid pre-event refresh");
            };
            verified_tx.send(()).unwrap();
            resume_rx.await.unwrap();
            refresh_verified_tokens(&race_db, SECRET, data, &race_device).await
        });
        verified_rx.await.unwrap();
        token_service::revoke_all_user_sessions(&db, &user.user_id)
            .await
            .unwrap();
        resume_tx.send(()).unwrap();
        let result = worker.await.unwrap();
        assert!(
            matches!(result, Err(kyomi_core::Error::Unauthorized(_))),
            "a verified old refresh must not rotate after revocation"
        );
        let sessions = token_service::get_user_sessions(&db, &user.user_id)
            .await
            .unwrap();
        assert!(
            sessions.is_empty(),
            "race must not leave an active replacement refresh"
        );
        assert!(
            authenticate_session(&db, SECRET, &session.access_token, "/", false)
                .await
                .is_err()
        );
        // Reverse order: an entire real refresh commits, then the event cuts it off.
        let fresh = crate::session::create_authenticated_session(&db, &kv, SECRET, &user, &device)
            .await
            .unwrap();
        let rotated = refresh_tokens(&db, SECRET, &fresh.refresh_token, &device)
            .await
            .unwrap();
        token_service::revoke_all_user_sessions(&db, &user.user_id)
            .await
            .unwrap();
        assert!(
            authenticate_session(&db, SECRET, &rotated.access_token, "/", false)
                .await
                .is_err()
        );
        assert!(
            refresh_tokens(&db, SECRET, &rotated.raw_refresh_token, &device)
                .await
                .is_err()
        );
        // Login mint before event / persistence after event is also fenced.
        let stale_issued = jwt::validate_token(&rotated.access_token, SECRET)
            .unwrap()
            .claims
            .session_iat_us
            .unwrap();
        assert!(matches!(
            token_service::store_session_refresh_token(
                &db,
                &user.user_id,
                "stale-login-hash",
                chrono::Utc::now() + chrono::Duration::days(7),
                &device,
                "stale-login-family",
                stale_issued
            )
            .await,
            Err(kyomi_core::Error::Unauthorized(_))
        ));
        kyomi_core::db_execute!(
            &db,
            "DELETE FROM refresh_tokens WHERE user_id = $1",
            &user.user_id
        )
        .unwrap();
        kyomi_core::db_execute!(&db, "DELETE FROM users WHERE user_id = $1", &user.user_id)
            .unwrap();
    }

    #[tokio::test]
    async fn session_cutoff_controlled_refresh_race_sqlite() {
        controlled_refresh_race(
            crate::test_support::test_pool().await,
            "sqlite-refresh-race",
        )
        .await;
    }

    #[tokio::test]
    async fn session_cutoff_controlled_refresh_race_postgres() {
        let Some(db) = crate::test_pg::postgres_test_pool_or_skip(
            "session_cutoff_controlled_refresh_race_postgres",
        )
        .await
        else {
            return;
        };
        controlled_refresh_race(db, &crate::test_pg::unique_test_id("refresh-race")).await;
    }

    async fn queued_revocation_issuance(db: kyomi_core::DbPool, tag: &str) {
        use std::future::Future;
        let user = user_service::create_user(&db, &format!("{tag}@test.local"), None, true)
            .await
            .unwrap();
        let kv = kyomi_core::kv_store_memory::InMemoryKVStore::new_pool();
        let device = DeviceInfo {
            user_agent: None,
            ip_address: None,
            country_code: None,
            oauth_client_id: None,
        };
        let original =
            crate::session::create_authenticated_session(&db, &kv, SECRET, &user, &device)
                .await
                .unwrap();
        // Both harnesses have one connection. Holding the user transaction
        // deterministically queues the real revocation at pool acquisition.
        macro_rules! queued_event {
            ($pool:expr) => {{
                let mut tx = $pool.begin().await.unwrap();
                sqlx::query(
                    "UPDATE users SET sessions_valid_from = sessions_valid_from WHERE user_id = $1",
                )
                .bind(&user.user_id)
                .execute(&mut *tx)
                .await
                .unwrap();
                let event = token_service::revoke_all_user_sessions(&db, &user.user_id);
                let mut event = std::pin::pin!(event);
                std::future::poll_fn(|cx| {
                    assert!(
                        event.as_mut().poll(cx).is_pending(),
                        "event must wait for the held transaction"
                    );
                    std::task::Poll::Ready(())
                })
                .await;
                // Real signed mint after the event started, before it got its lock.
                let token = jwt::create_session_access_token_str(
                    &user.user_id,
                    SECRET,
                    15,
                    Default::default(),
                    user.sessions_valid_from,
                )
                .unwrap();
                tx.commit().await.unwrap();
                assert!(event.await.unwrap() > 0);
                token
            }};
        }
        let during_wait = match &db {
            kyomi_core::DbPool::Postgres(pg) => queued_event!(pg),
            kyomi_core::DbPool::Sqlite(sq) => queued_event!(sq),
        };
        for allow_lapsed in [false, true] {
            assert!(
                matches!(
                    authenticate_session(&db, SECRET, &during_wait, "/", allow_lapsed).await,
                    Err(kyomi_core::Error::Unauthorized(_))
                ),
                "issuance while revocation waited must be cut off after commit"
            );
        }
        assert!(
            refresh_tokens(&db, SECRET, &original.refresh_token, &device)
                .await
                .is_err()
        );
        let fresh = crate::session::create_authenticated_session(&db, &kv, SECRET, &user, &device)
            .await
            .unwrap();
        authenticate_session(&db, SECRET, &fresh.access_token, "/", false)
            .await
            .unwrap();
        refresh_tokens(&db, SECRET, &fresh.refresh_token, &device)
            .await
            .unwrap();
        kyomi_core::db_execute!(
            &db,
            "DELETE FROM refresh_tokens WHERE user_id = $1",
            &user.user_id
        )
        .unwrap();
        kyomi_core::db_execute!(&db, "DELETE FROM users WHERE user_id = $1", &user.user_id)
            .unwrap();
    }

    #[tokio::test]
    async fn session_cutoff_queued_revocation_issuance_sqlite() {
        queued_revocation_issuance(
            crate::test_support::test_pool().await,
            "sqlite-queued-event",
        )
        .await;
    }

    #[tokio::test]
    async fn session_cutoff_queued_revocation_issuance_postgres() {
        let Some(db) = crate::test_pg::postgres_test_pool_or_skip(
            "session_cutoff_queued_revocation_issuance_postgres",
        )
        .await
        else {
            return;
        };
        queued_revocation_issuance(db, &crate::test_pg::unique_test_id("queued-event")).await;
    }

    #[tokio::test]
    async fn session_cutoff_revoke_failure_rolls_back_both_writes() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user(
            crate::test_support::sqlite_pool(&db),
            "user-1",
            "rollback@test.local",
        )
        .await;
        let token = jwt::create_access_token_str("user-1", SECRET, 15, Default::default()).unwrap();
        let device = DeviceInfo {
            user_agent: None,
            ip_address: None,
            country_code: None,
            oauth_client_id: None,
        };
        token_service::store_refresh_token(
            &db,
            "user-1",
            "rollback-hash",
            chrono::Utc::now() + chrono::Duration::days(7),
            &device,
            "rollback-family",
        )
        .await
        .unwrap();
        sqlx::query("CREATE TRIGGER reject_revoke BEFORE UPDATE ON refresh_tokens BEGIN SELECT RAISE(ABORT, 'controlled revocation failure'); END")
            .execute(crate::test_support::sqlite_pool(&db)).await.unwrap();
        assert!(
            token_service::revoke_all_user_sessions(&db, "user-1")
                .await
                .is_err()
        );
        assert_eq!(
            user_service::get_user_by_id(&db, "user-1")
                .await
                .unwrap()
                .unwrap()
                .sessions_valid_from,
            None
        );
        authenticate_session(&db, SECRET, &token, "/", false)
            .await
            .unwrap();
        assert_eq!(
            token_service::get_user_sessions(&db, "user-1")
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
