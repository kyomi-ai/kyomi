// SPDX-License-Identifier: AGPL-3.0-or-later

//! Auth-specific test fixtures, composed from [`kyomi_test_db`]'s shared
//! migrated SQLite pool and generic row helpers. Keep domain-specific
//! fixtures here; see the shared crate's "Extending this module" guidance.

use kyomi_core::DbPool;
pub(crate) use kyomi_test_db::{
    seed_membership, seed_user, seed_user_with_active, seed_workspace, sqlite_pool, test_pool,
};

/// Insert a `workspace_users` membership row with the schema's default role
/// (`workspace_user`) and an explicit `created_at`, for tests that assert on
/// membership ordering.
pub(crate) async fn seed_membership_at(
    sq: &sqlx::SqlitePool,
    workspace_id: &str,
    user_id: &str,
    active: bool,
    created_at: &str,
) {
    sqlx::query(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at) \
         VALUES ($1, $2, 'workspace_user', $3, $4)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(active)
    .bind(created_at)
    .execute(sq)
    .await
    .expect("insert membership");
}

/// Seed two users (`"user-a"`, `"user-b"`) into one workspace (`"ws-1"`),
/// with user-a as `workspace_admin`/owner and user-b as a plain member —
/// **including their `workspace_users` membership rows**, not just the
/// `users`/`workspaces` rows.
///
/// Do not drop those membership inserts to "simplify" this fixture:
/// `sync_log_service::get_entries_since`'s visibility filter — and every
/// endpoint that calls it — depends on the caller already being a
/// `workspace_users` member of the workspace it's querying. That had to be
/// rediscovered once already when writing KYO-258's tests (see KYO-271); a
/// caller that only needs the `users`/`workspaces` rows without membership
/// should call [`seed_user`]/[`seed_workspace`] directly rather than strip
/// this helper down.
///
/// `user_a_email`/`user_b_email` are parameters, not literals, because the
/// two pre-extraction copies (`chat_service`, `collection_service`) used
/// different email strings for no reason tied to what they were testing.
pub(crate) async fn seed_two_users_one_workspace(
    sq: &sqlx::SqlitePool,
    user_a_email: &str,
    user_b_email: &str,
) {
    seed_user(sq, "user-a", user_a_email).await;
    seed_user(sq, "user-b", user_b_email).await;
    seed_workspace(sq, "ws-1", "user-a").await;
    seed_membership(sq, "ws-1", "user-a", "workspace_admin", true).await;
    seed_membership(sq, "ws-1", "user-b", "user", true).await;
}

/// A fixed AES-256 test key — deliberately not random so encryption tests
/// are reproducible byte-for-byte.
pub(crate) fn test_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(b"test-key-1234567");
    key[16..].copy_from_slice(b"8901234567890123");
    key
}

/// Exercise both real signed-token extractors against the supplied backend.
pub(crate) async fn authenticate_session(
    db: &DbPool, secret: &str, token: &str, path: &str, allow_lapsed: bool,
) -> kyomi_core::Result<()> {
    use axum::extract::FromRequestParts;
    use crate::middleware::{AuthState, AuthUser, AuthUserAllowLapsed};
    let mut parts = axum::http::Request::builder().uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(()).expect("request").into_parts().0;
    let state = AuthState { db: db.clone(), jwt_secret: secret.into(), is_personal: false, self_hosted: false };
    if allow_lapsed {
        AuthUserAllowLapsed::from_request_parts(&mut parts, &state).await.map(|_| ())
    } else {
        AuthUser::from_request_parts(&mut parts, &state).await.map(|_| ())
    }
}
