// SPDX-License-Identifier: AGPL-3.0-or-later

//! Persistence and browser regressions for the MCP inactivity timeout.

use chrono::{DateTime, Duration, Utc};
use kyomi_core::DbPool;

use crate::{
    jwt,
    token_service::{self, DeviceInfo, RefreshTokenVerifyResult},
};

#[derive(sqlx::FromRow)]
struct TokenState {
    expires_at: DateTime<Utc>,
    replaced_at: Option<DateTime<Utc>>,
    is_active: bool,
}

async fn state(db: &DbPool, id: &str) -> TokenState {
    kyomi_core::db_fetch_one!(
        db,
        TokenState,
        "SELECT expires_at, replaced_at, is_active FROM refresh_tokens WHERE token_id = $1",
        id
    )
    .unwrap()
}

async fn stored(
    db: &DbPool,
    user: &str,
    client: Option<&str>,
    expiry: DateTime<Utc>,
) -> (String, String) {
    let raw = jwt::create_refresh_token();
    let id = token_service::store_refresh_token(
        db,
        user,
        &token_service::hash_refresh_token(&raw),
        expiry,
        &DeviceInfo {
            user_agent: None,
            ip_address: None,
            country_code: None,
            oauth_client_id: client.map(str::to_string),
        },
        &token_service::generate_family_id(),
    )
    .await
    .unwrap();
    (id, raw)
}

async fn persistence_guards(db: DbPool, tag: &str) {
    let user = crate::user_service::create_user(&db, &format!("{tag}@test.local"), None, true)
        .await
        .unwrap();
    let original = now() + Duration::hours(1);
    let later = now() + Duration::days(7);
    let (id, raw) = stored(&db, &user.user_id, Some("sliding-client"), original).await;
    // Verification itself must not globally introduce sliding browser expiry.
    assert!(matches!(
        token_service::verify_refresh_token(&db, &raw)
            .await
            .unwrap(),
        RefreshTokenVerifyResult::Valid(_)
    ));
    assert_eq!(state(&db, &id).await.expires_at, original);
    assert!(
        token_service::renew_oauth_refresh_token(&db, &id, "sliding-client", later)
            .await
            .unwrap()
    );
    assert_eq!(state(&db, &id).await.expires_at, later);
    // An older request completing last cannot shorten the newer deadline.
    assert!(
        token_service::renew_oauth_refresh_token(&db, &id, "sliding-client", original)
            .await
            .unwrap()
    );
    assert_eq!(state(&db, &id).await.expires_at, later);
    assert!(
        !token_service::renew_oauth_refresh_token(
            &db,
            &id,
            "wrong-client",
            later + Duration::days(1)
        )
        .await
        .unwrap()
    );
    assert_eq!(state(&db, &id).await.expires_at, later);
    assert!(
        !token_service::renew_oauth_refresh_token(&db, "unknown", "sliding-client", later)
            .await
            .unwrap()
    );

    // Verify before revocation, then resume renewal only after revocation commits.
    let verified = token_service::verify_refresh_token(&db, &raw)
        .await
        .unwrap();
    let RefreshTokenVerifyResult::Valid(data) = verified else {
        panic!("valid fixture")
    };
    token_service::revoke_refresh_token(&db, &id).await.unwrap();
    assert!(
        !token_service::renew_oauth_refresh_token(
            &db,
            &data.token_id,
            "sliding-client",
            later + Duration::days(1)
        )
        .await
        .unwrap()
    );
    assert!(!state(&db, &id).await.is_active);
    assert_eq!(state(&db, &id).await.expires_at, later);

    for client in [Some("sliding-client"), None] {
        let expiry = if client.is_some() {
            now() - Duration::hours(1)
        } else {
            original
        };
        let (id, _) = stored(&db, &user.user_id, client, expiry).await;
        assert!(
            !token_service::renew_oauth_refresh_token(&db, &id, "sliding-client", later)
                .await
                .unwrap()
        );
        assert_eq!(state(&db, &id).await.expires_at, expiry);
    }
    // A revoked timestamp must also fence renewal even if is_active is
    // inconsistent; a stale verifier must never clear revocation state.
    let (revoked_id, _) = stored(&db, &user.user_id, Some("sliding-client"), original).await;
    kyomi_core::db_execute!(
        &db,
        "UPDATE refresh_tokens SET revoked_at = $1 WHERE token_id = $2",
        &now(),
        &revoked_id
    )
    .unwrap();
    assert!(
        !token_service::renew_oauth_refresh_token(&db, &revoked_id, "sliding-client", later)
            .await
            .unwrap()
    );
    assert_eq!(state(&db, &revoked_id).await.expires_at, original);
    let (inactive_id, _) = stored(&db, &user.user_id, Some("sliding-client"), original).await;
    kyomi_core::db_execute!(
        &db,
        "UPDATE users SET active = $1 WHERE user_id = $2",
        &false,
        &user.user_id
    )
    .unwrap();
    assert!(
        !token_service::renew_oauth_refresh_token(&db, &inactive_id, "sliding-client", later)
            .await
            .unwrap()
    );
    assert_eq!(state(&db, &inactive_id).await.expires_at, original);
    kyomi_core::db_execute!(
        &db,
        "DELETE FROM refresh_tokens WHERE user_id = $1",
        &user.user_id
    )
    .unwrap();
    kyomi_core::db_execute!(&db, "DELETE FROM users WHERE user_id = $1", &user.user_id).unwrap();
}

async fn concurrency_and_browser(db: DbPool, tag: &str) {
    // Provision through the canonical harness first, then use independent
    // connections to exercise Postgres row-lock contention on that private DB.
    let db = match db {
        DbPool::Postgres(pg) => DbPool::Postgres(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(4)
                .connect_with((*pg.connect_options()).clone())
                .await
                .unwrap(),
        ),
        sqlite => sqlite,
    };
    let user = crate::user_service::create_user(&db, &format!("{tag}@test.local"), None, true)
        .await
        .unwrap();
    let expiry = now() + Duration::hours(1);
    let (id, _) = stored(&db, &user.user_id, Some("sliding-client"), expiry).await;
    let early = now() + Duration::days(7);
    let late = early + Duration::hours(1);
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for deadline in [early, late] {
        let (db, id, barrier) = (db.clone(), id.clone(), barrier.clone());
        workers.push(tokio::spawn(async move {
            barrier.wait().await;
            token_service::renew_oauth_refresh_token(&db, &id, "sliding-client", deadline)
                .await
                .unwrap()
        }));
    }
    barrier.wait().await;
    for worker in workers {
        assert!(worker.await.unwrap());
    }
    assert_eq!(state(&db, &id).await.expires_at, late);
    // Either lock order is safe: a successful renewal cannot undo revocation.
    let (renewed, revoked) = tokio::join!(
        token_service::renew_oauth_refresh_token(
            &db,
            &id,
            "sliding-client",
            late + Duration::hours(1)
        ),
        token_service::revoke_refresh_token(&db, &id)
    );
    renewed.unwrap();
    assert!(revoked.unwrap());
    assert!(!state(&db, &id).await.is_active);
    assert!(
        !token_service::renew_oauth_refresh_token(
            &db,
            &id,
            "sliding-client",
            late + Duration::days(1)
        )
        .await
        .unwrap()
    );

    let (old_id, raw) = stored(&db, &user.user_id, None, expiry).await;
    let device = DeviceInfo {
        user_agent: None,
        ip_address: None,
        country_code: None,
        oauth_client_id: None,
    };
    let rotated =
        crate::token_refresh::refresh_tokens(&db, "sliding-browser-secret", &raw, &device)
            .await
            .unwrap();
    assert_ne!(rotated.raw_refresh_token, raw);
    assert!(jwt::validate_token(&rotated.access_token, "sliding-browser-secret").is_ok());
    let old = state(&db, &old_id).await;
    assert_eq!(old.expires_at, expiry);
    let replaced = old.replaced_at.unwrap();
    assert!(matches!(
        token_service::verify_refresh_token(&db, &raw)
            .await
            .unwrap(),
        RefreshTokenVerifyResult::GracePeriod(_)
    ));
    // Model an OAuth-marked replaced token: it may be accepted during grace,
    // but neither expiry nor replaced_at may advance.
    kyomi_core::db_execute!(
        &db,
        "UPDATE refresh_tokens SET oauth_client_id = $1 WHERE token_id = $2",
        "sliding-client",
        &old_id
    )
    .unwrap();
    assert!(
        token_service::renew_oauth_refresh_token(&db, &old_id, "sliding-client", late)
            .await
            .unwrap()
    );
    assert_eq!(state(&db, &old_id).await.expires_at, expiry);
    assert_eq!(state(&db, &old_id).await.replaced_at, Some(replaced));
    let past_grace = now()
        - Duration::seconds(
            kyomi_core::constants::get()
                .jwt
                .refresh_token_grace_period_seconds
                + 5,
        );
    kyomi_core::db_execute!(
        &db,
        "UPDATE refresh_tokens SET replaced_at = $1 WHERE token_id = $2",
        &past_grace,
        &old_id
    )
    .unwrap();
    assert!(
        !token_service::renew_oauth_refresh_token(&db, &old_id, "sliding-client", late)
            .await
            .unwrap()
    );
    assert!(matches!(
        token_service::verify_refresh_token(&db, &raw)
            .await
            .unwrap(),
        RefreshTokenVerifyResult::TheftDetected { .. }
    ));
    assert!(matches!(
        token_service::verify_refresh_token(&db, &rotated.raw_refresh_token)
            .await
            .unwrap(),
        RefreshTokenVerifyResult::Invalid
    ));
    kyomi_core::db_execute!(
        &db,
        "DELETE FROM refresh_tokens WHERE user_id = $1",
        &user.user_id
    )
    .unwrap();
    kyomi_core::db_execute!(&db, "DELETE FROM users WHERE user_id = $1", &user.user_id).unwrap();
}

#[tokio::test]
async fn sliding_expiry_persistence_guards_sqlite() {
    persistence_guards(
        crate::test_support::test_pool().await,
        "sliding-guards-sqlite",
    )
    .await;
}

#[tokio::test]
async fn sliding_expiry_persistence_guards_postgres() {
    let Some(db) =
        crate::test_pg::postgres_test_pool_or_skip("sliding_expiry_persistence_guards_postgres")
            .await
    else {
        return;
    };
    persistence_guards(db, &crate::test_pg::unique_test_id("sliding-guards")).await;
}

#[tokio::test]
async fn sliding_expiry_concurrency_and_browser_sqlite() {
    concurrency_and_browser(
        crate::test_support::test_pool().await,
        "sliding-concurrent-sqlite",
    )
    .await;
}

#[tokio::test]
async fn sliding_expiry_concurrency_and_browser_postgres() {
    let Some(db) = crate::test_pg::postgres_test_pool_or_skip(
        "sliding_expiry_concurrency_and_browser_postgres",
    )
    .await
    else {
        return;
    };
    concurrency_and_browser(db, &crate::test_pg::unique_test_id("sliding-concurrent")).await;
}

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap()
}
