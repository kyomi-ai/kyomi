// SPDX-License-Identifier: AGPL-3.0-or-later

//! Push subscription service — CRUD operations for Web Push subscriptions.
//!
//! Manages browser/device push subscriptions stored in the `push_subscriptions` table.

use chrono::{DateTime, Utc};
use kyomi_core::models::PushSubscription;
use kyomi_core::DbPool;
use kyomi_types::truncate_preview;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use url::Url;

/// Input for creating/updating a push subscription.
#[derive(Debug, Deserialize)]
pub struct SaveSubscriptionInput {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub user_agent: Option<String>,
    pub device_label: Option<String>,
}

// ===========================================================================
// Push endpoint validation (KYO-219 — SSRF via push subscription endpoint)
// ===========================================================================
//
// A push subscription `endpoint` is a URL the server will later POST to,
// carrying a VAPID `Authorization` JWT signed with the server's private key.
// Any authenticated user can submit one, so it must be validated at ingress
// (`apps/server/src/routes/push.rs::subscribe`) AND re-validated at egress
// (`crates/kyomi-agent/src/web_push.rs::send_to_subscription`), since rows
// written before this fix — or written directly to the DB — are unvalidated.
//
// This is the single shared predicate for both call sites. Do not duplicate
// this logic — a divergent second copy is exactly how this class of fix
// regresses.

/// Env var (self-hosted only) naming additional push-endpoint host suffixes to
/// trust, beyond the built-in browser push-service allowlist below. Intended
/// for self-hosted deployments running a custom push relay.
///
/// Comma-separated, e.g. `PUSH_ALLOWED_ENDPOINT_HOSTS=push.example.com,relay.internal.example.org`.
/// Matching uses the same exact-host-or-leading-dot-boundary rule as the
/// built-in list (see [`endpoint_host_is_allowed`]).
pub const PUSH_ALLOWED_ENDPOINT_HOSTS_ENV: &str = "PUSH_ALLOWED_ENDPOINT_HOSTS";

/// Built-in allowlist of Web Push service host suffixes used by the major
/// browsers. Real subscriptions only ever come from one of these.
///
/// An allowlist-by-hostname (rather than a denylist of private/loopback IP
/// ranges) sidesteps the entire DNS-rebinding and IPv4-mapped-IPv6 problem
/// class: the check never resolves the hostname to an address, so there is no
/// gap between "the address checked at registration" and "the address used
/// at send time" for an attacker to exploit.
const BUILTIN_ALLOWED_ENDPOINT_HOST_SUFFIXES: &[&str] = &[
    "googleapis.com",            // Chrome / Edge — FCM, e.g. fcm.googleapis.com
    "push.services.mozilla.com", // Firefox — autopush, e.g. updates.push.services.mozilla.com
    "notify.windows.com",        // Edge legacy / WNS
    "push.apple.com",            // Safari — APNs web push
];

/// Maximum accepted length for a push endpoint URL.
const MAX_ENDPOINT_LEN: usize = 2048;

/// Validate a Web Push subscription endpoint URL.
///
/// Returns `Ok(())` if the endpoint is safe to store and later POST to (with
/// the VAPID `Authorization` header attached). Returns `Err(reason)`
/// otherwise — `reason` is safe to surface to the caller as a 400 body.
///
/// Requirements:
/// - `https` scheme only (no `http`, `file`, `gopher`, etc.).
/// - No embedded credentials (`https://user:pass@host/...`).
/// - Host must match the built-in browser push-service allowlist, or — for
///   self-hosted deployments — a host named via
///   [`PUSH_ALLOWED_ENDPOINT_HOSTS_ENV`].
/// - Length capped at [`MAX_ENDPOINT_LEN`].
pub fn validate_push_endpoint(endpoint: &str) -> Result<(), String> {
    if endpoint.len() > MAX_ENDPOINT_LEN {
        return Err(format!(
            "push endpoint exceeds maximum length of {MAX_ENDPOINT_LEN} characters"
        ));
    }

    let url = Url::parse(endpoint).map_err(|e| format!("invalid push endpoint URL: {e}"))?;

    if url.scheme() != "https" {
        return Err(format!(
            "push endpoint must use https, got scheme {:?}",
            url.scheme()
        ));
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err("push endpoint must not contain embedded credentials".to_string());
    }

    let host = url
        .host_str()
        .ok_or_else(|| "push endpoint has no host".to_string())?;

    if !endpoint_host_is_allowed(host) {
        return Err(format!(
            "push endpoint host {host:?} is not a recognized push service"
        ));
    }

    Ok(())
}

/// Check whether `host` matches an allowed suffix — either the built-in
/// browser push-service list, or an operator-configured host from
/// [`PUSH_ALLOWED_ENDPOINT_HOSTS_ENV`].
///
/// Matching is exact-host-or-leading-dot-boundary: `host` matches `suffix`
/// iff `host == suffix` or `host.ends_with(".{suffix}")`. A naive
/// `host.ends_with(suffix)` check (no dot) would let `evilgoogleapis.com`
/// pass a `googleapis.com` allowlist entry — this does not.
fn endpoint_host_is_allowed(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();

    let operator_hosts = std::env::var(PUSH_ALLOWED_ENDPOINT_HOSTS_ENV).unwrap_or_default();
    let operator_suffixes = operator_hosts
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    BUILTIN_ALLOWED_ENDPOINT_HOST_SUFFIXES
        .iter()
        .map(|s| s.to_string())
        .chain(operator_suffixes)
        .any(|suffix| host == suffix || host.ends_with(&format!(".{suffix}")))
}

/// Row shape for the startup purge sweep — just enough to validate and,
/// if invalid, delete.
#[derive(sqlx::FromRow)]
struct PurgeCandidate {
    id: i32,
    endpoint: String,
}

/// Authoritative one-time purge of `push_subscriptions` rows whose endpoint
/// fails [`validate_push_endpoint`]. Run once at every server boot (see
/// `apps/server/src/main.rs`), immediately after the SQL migrations apply.
///
/// KYO-219: the SQL migrations that ship alongside this fix
/// (`apps/server/migrations/20260725010000_purge_invalid_push_endpoints.sql`
/// and its SQLite counterpart) are a coarse best-effort first pass — plain
/// SQL cannot be proven equivalent to `url::Url`'s WHATWG parsing, and three
/// separate divergences were found across three rounds of review (path,
/// query/fragment, and backslash smuggling). This sweep is what's actually
/// authoritative: it calls the exact predicate enforced at subscribe-time
/// and send-time, so a row survives it iff it would be accepted by a fresh
/// `/push/subscribe` call today.
///
/// Unlike the SQL migrations, this sweep sees
/// [`PUSH_ALLOWED_ENDPOINT_HOSTS_ENV`], so a self-hosted operator's
/// legitimate custom-relay subscriptions are correctly preserved on every
/// boot where the env var is set — it just can't restore rows an earlier
/// migration run already deleted before the env var existed.
///
/// Idempotent and safe to run unconditionally on every boot: after the
/// first pass there is nothing left to delete, and `push_subscriptions` is
/// small (one row per registered browser/device), so revalidating every row
/// is cheap. Returns the number of rows purged.
pub async fn purge_invalid_subscriptions(db: &DbPool) -> kyomi_core::Result<u64> {
    let candidates: Vec<PurgeCandidate> = kyomi_core::db_fetch_all!(
        db,
        PurgeCandidate,
        "SELECT id, endpoint FROM push_subscriptions"
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!(
            "failed to list push subscriptions for startup purge sweep: {e}"
        ))
    })?;

    let mut purged = 0u64;
    for candidate in candidates {
        if let Err(reason) = validate_push_endpoint(&candidate.endpoint) {
            match kyomi_core::db_execute!(
                db,
                "DELETE FROM push_subscriptions WHERE id = $1",
                candidate.id
            ) {
                Ok(_) => {
                    purged += 1;
                    info!(
                        subscription_id = candidate.id,
                        endpoint_prefix = %truncate_endpoint(&candidate.endpoint),
                        reason = %reason,
                        "Purged invalid push subscription endpoint at startup"
                    );
                }
                Err(e) => {
                    warn!(
                        subscription_id = candidate.id,
                        error = %e,
                        "Failed to purge invalid push subscription during startup sweep"
                    );
                }
            }
        }
    }

    Ok(purged)
}

/// Lightweight push device record for the settings UI.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PushSubscriptionDevice {
    pub id: i32,
    pub device_label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Save (upsert) a push subscription for a user.
///
/// Uses `ON CONFLICT (user_id, endpoint)` to update keys if the same device re-subscribes.
pub async fn save_subscription(
    db: &DbPool,
    user_id: &str,
    input: &SaveSubscriptionInput,
) -> kyomi_core::Result<PushSubscription> {
    let row = kyomi_core::db_fetch_one!(
        db,
        PushSubscription,
        r#"
        INSERT INTO push_subscriptions (user_id, endpoint, p256dh, auth, user_agent, device_label)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (user_id, endpoint)
        DO UPDATE SET
            p256dh = EXCLUDED.p256dh,
            auth = EXCLUDED.auth,
            user_agent = EXCLUDED.user_agent,
            device_label = EXCLUDED.device_label
        RETURNING id, user_id, endpoint, p256dh, auth, user_agent, device_label,
                  created_at, last_used_at, failure_count
        "#,
        user_id,
        &input.endpoint,
        &input.p256dh,
        &input.auth,
        &input.user_agent,
        &input.device_label
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to save push subscription: {e}")))?;

    info!(user_id = %user_id, endpoint_prefix = %truncate_endpoint(&input.endpoint), "Saved push subscription");
    Ok(row)
}

/// Remove a push subscription by endpoint.
pub async fn remove_subscription(
    db: &DbPool,
    user_id: &str,
    endpoint: &str,
) -> kyomi_core::Result<()> {
    kyomi_core::db_execute!(
        db,
        "DELETE FROM push_subscriptions WHERE user_id = $1 AND endpoint = $2",
        user_id,
        endpoint
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to remove push subscription: {e}")))?;

    info!(user_id = %user_id, "Removed push subscription");
    Ok(())
}

/// Remove a push subscription by ID (for settings UI delete button).
pub async fn remove_subscription_by_id(
    db: &DbPool,
    user_id: &str,
    id: i32,
) -> kyomi_core::Result<()> {
    let result = kyomi_core::db_execute!(
        db,
        "DELETE FROM push_subscriptions WHERE id = $1 AND user_id = $2",
        id,
        user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to remove push subscription: {e}")))?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::NotFound(
            "Push subscription not found".into(),
        ));
    }

    info!(user_id = %user_id, subscription_id = id, "Removed push subscription by ID");
    Ok(())
}

/// Get all push subscriptions for a user (full data, for push delivery).
pub async fn get_user_subscriptions(
    db: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<PushSubscription>> {
    let rows = kyomi_core::db_fetch_all!(
        db,
        PushSubscription,
        r#"
        SELECT id, user_id, endpoint, p256dh, auth, user_agent, device_label,
               created_at, last_used_at, failure_count
        FROM push_subscriptions
        WHERE user_id = $1
        ORDER BY created_at DESC
        "#,
        user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get push subscriptions: {e}")))?;

    Ok(rows)
}

/// List push subscriptions for the settings UI (lightweight).
pub async fn list_user_subscriptions(
    db: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<PushSubscriptionDevice>> {
    let rows = kyomi_core::db_fetch_all!(
        db,
        PushSubscriptionDevice,
        r#"
        SELECT id, device_label, created_at, last_used_at
        FROM push_subscriptions
        WHERE user_id = $1
        ORDER BY created_at DESC
        "#,
        user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to list push subscriptions: {e}")))?;

    Ok(rows)
}

/// Record a successful push delivery — reset failure count and update last_used_at.
pub async fn record_success(db: &DbPool, id: i32) {
    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE push_subscriptions SET last_used_at = {now_expr}, failure_count = 0 WHERE id = $1"
    );
    if let Err(e) = kyomi_core::db_execute!(db, &sql, id) {
        warn!(subscription_id = id, error = %e, "Failed to record push success");
    }
}

/// Record a push delivery failure.
///
/// If `is_gone` is true (HTTP 410/404), the subscription is deleted immediately
/// because the browser has unsubscribed. Otherwise, the failure count is incremented.
pub async fn record_failure(db: &DbPool, id: i32, is_gone: bool) {
    if is_gone {
        // Subscription expired/unsubscribed — delete immediately
        if let Err(e) = kyomi_core::db_execute!(
            db,
            "DELETE FROM push_subscriptions WHERE id = $1",
            id
        ) {
            warn!(subscription_id = id, error = %e, "Failed to delete gone push subscription");
        } else {
            info!(subscription_id = id, "Deleted expired push subscription (410 Gone)");
        }
    } else {
        // Transient failure — increment counter
        if let Err(e) = kyomi_core::db_execute!(
            db,
            "UPDATE push_subscriptions SET failure_count = failure_count + 1 WHERE id = $1",
            id
        ) {
            warn!(subscription_id = id, error = %e, "Failed to record push failure");
        }
    }
}

/// Clean up stale push subscriptions.
///
/// Deletes subscriptions that:
/// - Have more than 5 consecutive failures, OR
/// - Have not been used in over 90 days
pub async fn cleanup_stale(db: &DbPool) {
    let cutoff = Utc::now() - chrono::Duration::days(90);

    match kyomi_core::db_execute!(
        db,
        r#"
        DELETE FROM push_subscriptions
        WHERE failure_count > 5
           OR (last_used_at IS NOT NULL AND last_used_at < $1)
           OR (last_used_at IS NULL AND created_at < $1)
        "#,
        cutoff
    ) {
        Ok(result) => {
            let count = result.rows_affected();
            if count > 0 {
                info!(count = count, "Cleaned up stale push subscriptions");
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to cleanup stale push subscriptions");
        }
    }
}

/// Truncate an endpoint URL for logging (show first 60 chars).
///
/// Endpoint host is allowlist-validated (see `endpoint_host_is_allowed`), but
/// the path is not — a non-ASCII path byte-sliced at a raw offset could land
/// mid-character and panic. Values read back from the `push_subscriptions`
/// table (see the startup-purge caller above) may also predate that
/// validation entirely. `truncate_preview` cuts on character boundaries.
fn truncate_endpoint(endpoint: &str) -> String {
    truncate_preview(endpoint, 60)
}

#[cfg(test)]
mod tests {
    //! # Env var discipline
    //!
    //! `endpoint_host_is_allowed_env_extension` mutates the process-global
    //! env var `PUSH_ALLOWED_ENDPOINT_HOSTS`. It goes through the crate-wide
    //! [`crate::test_env::EnvVarGuard`] (see its module docs), which
    //! serializes it against every other env-mutating test in this crate —
    //! not just the ones in this file. Every other test in this module must
    //! not depend on the env var being unset — they don't, since
    //! `validate_push_endpoint`/`endpoint_host_is_allowed` only *add* to the
    //! built-in allowlist when the env var happens to be set, and none of
    //! the fixed reject/accept cases below could be flipped by an
    //! attacker-chosen operator suffix.

    use super::*;
    use crate::test_env::EnvVarGuard;

    fn with_hosts(value: &str) -> EnvVarGuard {
        EnvVarGuard::acquire().set(PUSH_ALLOWED_ENDPOINT_HOSTS_ENV, value)
    }

    // -----------------------------------------------------------------
    // Accept — real browser push-service URL shapes
    // -----------------------------------------------------------------

    #[test]
    fn accepts_real_fcm_endpoint() {
        assert_eq!(
            validate_push_endpoint(
                "https://fcm.googleapis.com/fcm/send/dGVzdC1zdWJzY3JpcHRpb24taWQ"
            ),
            Ok(())
        );
    }

    #[test]
    fn accepts_real_mozilla_autopush_endpoint() {
        assert_eq!(
            validate_push_endpoint(
                "https://updates.push.services.mozilla.com/wpush/v2/gAAAAABtest-token"
            ),
            Ok(())
        );
    }

    // -----------------------------------------------------------------
    // Reject — table from KYO-219
    // -----------------------------------------------------------------

    #[test]
    fn rejects_plain_http() {
        assert!(validate_push_endpoint("http://fcm.googleapis.com/fcm/send/abc").is_err());
    }

    #[test]
    fn rejects_loopback() {
        assert!(validate_push_endpoint("https://127.0.0.1/x").is_err());
    }

    #[test]
    fn rejects_localhost() {
        assert!(validate_push_endpoint("https://localhost/x").is_err());
    }

    #[test]
    fn rejects_cloud_metadata_address() {
        assert!(validate_push_endpoint("https://169.254.169.254/latest/meta-data/").is_err());
    }

    #[test]
    fn rejects_rfc1918_10_range() {
        assert!(validate_push_endpoint("https://10.0.0.1/x").is_err());
    }

    #[test]
    fn rejects_rfc1918_192_range() {
        assert!(validate_push_endpoint("https://192.168.1.1/x").is_err());
    }

    #[test]
    fn rejects_ipv6_loopback() {
        assert!(validate_push_endpoint("https://[::1]/x").is_err());
    }

    #[test]
    fn rejects_ipv6_link_local() {
        assert!(validate_push_endpoint("https://[fe80::1]/x").is_err());
    }

    #[test]
    fn rejects_embedded_credentials() {
        assert!(
            validate_push_endpoint("https://user:pass@fcm.googleapis.com/x").is_err()
        );
    }

    #[test]
    fn rejects_file_scheme() {
        assert!(validate_push_endpoint("file:///etc/passwd").is_err());
    }

    #[test]
    fn rejects_ipv4_mapped_ipv6_private_address() {
        // The case a naive private-IP denylist misses: 10.0.0.1 wrapped in an
        // IPv4-mapped IPv6 literal. Our allowlist rejects it for the same
        // reason it rejects every other IP literal — it never matches a
        // hostname suffix — so there's nothing rebinding-specific to bypass.
        assert!(validate_push_endpoint("https://[::ffff:10.0.0.1]/x").is_err());
    }

    // -----------------------------------------------------------------
    // Suffix-boundary bypass
    // -----------------------------------------------------------------

    #[test]
    fn rejects_suffix_boundary_bypass_host() {
        // Would pass a naive `host.ends_with("googleapis.com")` check because
        // "evil" + "googleapis.com" == "evilgoogleapis.com". Must be rejected.
        assert!(validate_push_endpoint("https://evilgoogleapis.com/fcm/send/abc").is_err());
    }

    #[test]
    fn accepts_exact_allowed_host_with_no_subdomain() {
        // Sanity check on the other side of the boundary rule: the bare
        // allowlisted domain (no subdomain) is accepted.
        assert_eq!(
            validate_push_endpoint("https://googleapis.com/fcm/send/abc"),
            Ok(())
        );
    }

    // -----------------------------------------------------------------
    // Misc validation
    // -----------------------------------------------------------------

    #[test]
    fn rejects_overlong_endpoint() {
        let overlong = format!(
            "https://fcm.googleapis.com/fcm/send/{}",
            "a".repeat(MAX_ENDPOINT_LEN)
        );
        assert!(validate_push_endpoint(&overlong).is_err());
    }

    #[test]
    fn rejects_unparseable_url() {
        assert!(validate_push_endpoint("not a url at all").is_err());
    }

    #[test]
    fn rejects_unrecognized_https_host() {
        assert!(validate_push_endpoint("https://attacker.example/collect").is_err());
    }

    // -----------------------------------------------------------------
    // Self-hosted escape hatch
    // -----------------------------------------------------------------

    #[test]
    fn endpoint_host_is_allowed_env_extension() {
        let _guard = with_hosts("relay.example.internal");

        assert_eq!(
            validate_push_endpoint("https://relay.example.internal/push/abc"),
            Ok(())
        );
        assert_eq!(
            validate_push_endpoint("https://device1.relay.example.internal/push/abc"),
            Ok(())
        );
        // The env var extension is also suffix-boundary-safe, not a raw
        // substring match.
        assert!(
            validate_push_endpoint("https://evilrelay.example.internal/push/abc").is_err()
        );
        // An unrelated host is still rejected even with the env var set.
        assert!(validate_push_endpoint("https://attacker.example/collect").is_err());
    }

    // -----------------------------------------------------------------
    // Migration purge logic (KYO-219) — SQLite dialect
    // -----------------------------------------------------------------
    //
    // `00028_purge_invalid_push_endpoints.sql` runs automatically when this
    // pool is created via `sqlx::migrate!`, but the table is empty at that
    // point, so it purges nothing. To actually exercise its WHERE clause we
    // re-run the exact statement — loaded from the migration file itself, so
    // this test can't drift from what ships — against hand-seeded good/bad
    // rows.

    const SQLITE_PURGE_MIGRATION_SQL: &str = include_str!(
        "../../../apps/server/migrations-sqlite/00028_purge_invalid_push_endpoints.sql"
    );

    #[tokio::test]
    async fn sqlite_purge_migration_deletes_bad_rows_and_keeps_good_rows() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        sqlx::query("INSERT INTO users (user_id, email) VALUES ('u1', 'u1@test.local')")
            .execute(&pool)
            .await
            .expect("seed user");

        let good = [
            "https://fcm.googleapis.com/fcm/send/abc",
            "https://updates.push.services.mozilla.com/wpush/v2/abc",
            // Bare allowlisted domain, no subdomain — the other side of the
            // boundary rule.
            "https://googleapis.com/fcm/send/abc",
        ];
        let bad = [
            "http://fcm.googleapis.com/fcm/send/abc",
            "https://127.0.0.1/x",
            "https://10.0.0.1/x",
            "https://user:pass@fcm.googleapis.com/x",
            "https://attacker.example/collect",
            // Suffix-boundary bypass (subdomain form) — must not survive the
            // migration either.
            "https://evilgoogleapis.com/fcm/send/abc",
            // Path-smuggling bypass: the real host is attacker.example, but
            // an unanchored `LIKE '%.googleapis.com/%'` substring search
            // would match the suffix appearing in the PATH instead of the
            // host. This is the exact bug a code reviewer caught in an
            // earlier version of this migration — must never regress.
            "https://attacker.example/.googleapis.com/x",
            // Query/fragment-smuggling bypass (second recurrence of the same
            // bug class, caught in a follow-up review): a host extraction
            // that stops only at "/" lets a URL with no "/" but a crafted
            // query string or fragment survive, because the "host" it
            // extracts still ends in the allowed suffix. The authority must
            // be bounded by the first of "/", "?", OR "#".
            "https://evil.com?x=y.googleapis.com",
            "https://evil.com#y.googleapis.com",
            // Userinfo bypass, both positions: once the authority is
            // extracted, "evil.com@fcm.googleapis.com" ends in
            // ".googleapis.com" and a naive implementation might keep it.
            // These are actually caught by the separate embedded-credentials
            // check (`endpoint_lower LIKE 'https://%@%'`), which fires on
            // the raw endpoint regardless of where the "@" lands — this
            // locks that in for both orderings.
            "https://evil.com@fcm.googleapis.com/x",
            "https://fcm.googleapis.com@evil.com/x",
            // Backslash-smuggling bypass (third recurrence): `url::Url`
            // implements the WHATWG URL Standard, which treats `\` as
            // equivalent to `/` for special schemes including `https` (a
            // legacy IE quirk with no RFC 3986 analog). Real host per
            // `url::Url` is evil.com; a host extraction bounded only by
            // "/", "?", "#" (not also "\") would keep this.
            "https://evil.com\\.googleapis.com/x",
        ];

        for (i, endpoint) in good.iter().chain(bad.iter()).enumerate() {
            sqlx::query(
                "INSERT INTO push_subscriptions (user_id, endpoint, p256dh, auth) \
                 VALUES ($1, $2, 'p', 'a')",
            )
            .bind("u1")
            .bind(*endpoint)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("seed subscription {i} ({endpoint}): {e}"));
        }

        // Re-run the purge statement shipped in the migration file itself.
        sqlx::query(SQLITE_PURGE_MIGRATION_SQL)
            .execute(&pool)
            .await
            .expect("run purge statement");

        let remaining: Vec<String> = sqlx::query_scalar("SELECT endpoint FROM push_subscriptions")
            .fetch_all(&pool)
            .await
            .expect("fetch remaining endpoints");

        for endpoint in &good {
            assert!(
                remaining.contains(&endpoint.to_string()),
                "legitimate endpoint {endpoint} was incorrectly purged"
            );
        }
        for endpoint in &bad {
            assert!(
                !remaining.contains(&endpoint.to_string()),
                "malicious endpoint {endpoint} survived the purge migration"
            );
        }
        assert_eq!(
            remaining.len(),
            good.len(),
            "unexpected row count after purge: {remaining:?}"
        );
    }

    // -----------------------------------------------------------------
    // Rust startup sweep is authoritative — the SQL migration alone is not
    // -----------------------------------------------------------------

    /// This is the point of the whole `purge_invalid_subscriptions` change:
    /// the SQL migrations are a coarse best-effort pass that cannot be
    /// proven equivalent to `validate_push_endpoint` (three real
    /// divergences were found in review; see the migration file comments).
    /// This test constructs a row that satisfies every check the SQL
    /// migration makes — https scheme, no embedded credentials, host
    /// matches the googleapis.com suffix — but still fails
    /// `validate_push_endpoint` for a reason the SQL never checks at all:
    /// the length cap. It proves the SQL pass keeps the row, and only the
    /// Rust sweep removes it. If the sweep were removed (or its call site
    /// deleted from `main.rs`), this test would fail at the final
    /// assertion.
    #[tokio::test]
    async fn purge_invalid_subscriptions_removes_row_the_sql_migration_would_keep() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        sqlx::query("INSERT INTO users (user_id, email) VALUES ('u1', 'u1@test.local')")
            .execute(&pool)
            .await
            .expect("seed user");

        // Syntactically valid per every check the SQL migration makes, but
        // exceeds MAX_ENDPOINT_LEN — a rule the SQL migration doesn't (and
        // can't reasonably) encode.
        let overlong_endpoint = format!(
            "https://fcm.googleapis.com/fcm/send/{}",
            "a".repeat(MAX_ENDPOINT_LEN)
        );

        sqlx::query(
            "INSERT INTO push_subscriptions (user_id, endpoint, p256dh, auth) \
             VALUES ($1, $2, 'p', 'a')",
        )
        .bind("u1")
        .bind(&overlong_endpoint)
        .execute(&pool)
        .await
        .expect("seed overlong subscription");

        // Sanity check: confirm the SQL migration alone keeps this row —
        // otherwise this test wouldn't be exercising a real gap.
        sqlx::query(SQLITE_PURGE_MIGRATION_SQL)
            .execute(&pool)
            .await
            .expect("run SQL purge migration");

        let survives_sql_pass: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM push_subscriptions WHERE endpoint = ?",
        )
        .bind(&overlong_endpoint)
        .fetch_one(&pool)
        .await
        .expect("count after SQL pass");
        assert_eq!(
            survives_sql_pass, 1,
            "the overlong endpoint should survive the SQL migration alone — \
             otherwise this test isn't proving what it claims"
        );

        // The Rust sweep is what actually removes it.
        let db = DbPool::Sqlite(pool);
        let purged = purge_invalid_subscriptions(&db)
            .await
            .expect("purge sweep should succeed");
        assert_eq!(purged, 1, "sweep should purge exactly the overlong row");

        let survives_sweep: i64 = kyomi_core::db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM push_subscriptions WHERE endpoint = $1",
            &overlong_endpoint
        )
        .expect("count after sweep");
        assert_eq!(
            survives_sweep, 0,
            "the Rust sweep must remove a row the SQL migration alone would have kept"
        );
    }

    // -----------------------------------------------------------------
    // truncate_endpoint (KYO-241) — byte-slicing at a raw offset panics
    // once the path (unvalidated by `endpoint_host_is_allowed`, see the
    // doc comment on `truncate_endpoint`) contains multi-byte UTF-8.
    // -----------------------------------------------------------------

    #[test]
    fn truncate_endpoint_short_is_unchanged() {
        let endpoint = "https://fcm.googleapis.com/fcm/send/abc";
        assert_eq!(truncate_endpoint(endpoint), endpoint);
    }

    #[test]
    fn truncate_endpoint_mid_multibyte_path_char_does_not_panic() {
        // Host passes `endpoint_host_is_allowed` (it's a real allowlisted
        // host); the path is attacker- or device-controlled and contains a
        // 3-byte CJK character. 36 ASCII prefix bytes + 23 ASCII filler
        // bytes = 59 bytes, so the 3-byte CJK char occupies bytes 59-61 —
        // straddling byte offset 60, the old `&endpoint[..60]` cut point.
        // Pre-fix this panicked with "byte index 60 is not a char boundary".
        let prefix = "https://fcm.googleapis.com/fcm/send/"; // 36 bytes
        assert_eq!(prefix.len(), 36);
        let endpoint = format!("{prefix}{}固{}", "a".repeat(23), "b".repeat(10));
        assert!(
            !endpoint.is_char_boundary(60),
            "test input must cut mid-character at byte 60 to exercise the old panic"
        );

        let truncated = truncate_endpoint(&endpoint);

        assert!(truncated.ends_with("..."));
        let content = truncated.trim_end_matches("...");
        assert_eq!(content.chars().count(), 60, "must keep exactly 60 characters");
    }

    #[test]
    fn truncate_endpoint_emoji_path_does_not_panic() {
        // 36 ASCII prefix bytes + 23 ASCII filler bytes = 59 bytes, so the
        // first 4-byte emoji occupies bytes 59-63 — straddling byte offset
        // 60, the old `&endpoint[..60]` cut point.
        let endpoint = format!(
            "https://fcm.googleapis.com/fcm/send/{}🎉🎉🎉",
            "x".repeat(23)
        );
        assert!(
            !endpoint.is_char_boundary(60),
            "test input must cut mid-character at byte 60 to exercise the old panic"
        );

        let truncated = truncate_endpoint(&endpoint);

        assert!(truncated.ends_with("..."));
        let content = truncated.trim_end_matches("...");
        assert_eq!(content.chars().count(), 60, "must keep exactly 60 characters");
    }
}
