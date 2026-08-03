// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reactive helpers for permission-based UI gating.
//!
//! Wraps the shared `UserContext` resource so pages don't each hand-roll a
//! `Signal::derive` over `permissions`. See `server_fns::context::UserContext`
//! for the source of truth — the `permissions` set (KYO-189 P2) is computed
//! server-side by `kyomi_auth::permissions::permissions_for` and shipped to
//! the client as-is; nothing on this side re-derives it. `UserContext` no
//! longer carries a raw `workspace_roles` list at all (KYO-189 P3), so
//! `can(Permission::X)` — via [`use_permissions`] — is the only gate
//! available.

use leptos::prelude::*;

use kyomi_types::Permission;

use crate::server_fns::context::UserContext;

/// Reactive permission checker over the shared `UserContext` resource.
///
/// Returned by [`use_permissions`]. Call `.can(permission)` inside a
/// reactive closure (a `Signal::derive`, a `<Show when=move || ...>`, etc.)
/// to gate a UI surface on the exact [`Permission`] its corresponding server
/// fn requires — never on `is_owner` directly.
#[derive(Clone, Copy)]
pub struct Permissions(Signal<Vec<Permission>>);

impl Permissions {
    /// Whether the current user holds `permission` in their active
    /// workspace. Fails closed: `false` while the resource is loading,
    /// errored, or absent.
    pub fn can(&self, permission: Permission) -> bool {
        self.0.get().contains(&permission)
    }
}

/// Derive the current permission set from a possibly-not-yet-loaded,
/// possibly-failed `UserContext` fetch.
///
/// Fails closed to an empty set — while loading (`None`) and on fetch
/// failure (`Some(Err(_))`) alike — but unlike the pre-KYO-240 version, a
/// failed fetch is no longer silent: it's logged via `tracing::warn!` so
/// "every permission-gated surface vanished" has a diagnostic trail instead
/// of none. The loading case deliberately does not log — it isn't a
/// failure, just a resource that hasn't resolved yet.
fn permissions_from(user_ctx: Option<Result<UserContext, ServerFnError>>) -> Vec<Permission> {
    user_ctx
        .and_then(|result| {
            result
                .map_err(|e| {
                    tracing::warn!(
                        error = %e,
                        "use_permissions: user context fetch failed, failing closed to zero permissions"
                    );
                })
                .ok()
        })
        .map(|ctx| ctx.permissions)
        .unwrap_or_default()
}

/// Reactive permission set derived from the shared `UserContext` resource.
///
/// The single lookup helper every UI gate should use (KYO-189 P2) — wraps
/// the `expect_context::<LocalResource<...>>()` boilerplate so no page
/// re-implements the resource read. Must be called from a component that
/// renders under the `Layout` (or any ancestor that provides the
/// `LocalResource<Result<UserContext, ServerFnError>>` context) — panics via
/// `expect_context` otherwise.
pub fn use_permissions() -> Permissions {
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();
    Permissions(Signal::derive(move || permissions_from(user_ctx.get())))
}

/// Why (or whether) the current user may use analytics settings.
///
/// Returned by [`analytics_access`] / [`use_analytics_access`]. See
/// [`analytics_access`] for the precedence rules.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnalyticsAccess {
    /// The analytics tab, the "Analytics Settings" datasource-row link, and
    /// the analytics page's content should all render normally.
    Allowed,
    /// Analytics requires Postgres + ClickHouse, unavailable on self-hosted
    /// SQLite deployments — regardless of permissions or billing state.
    SelfHosted,
    /// The workspace doesn't have billing enabled, so there's no analytics
    /// entitlement to manage.
    BillingDisabled,
    /// The user lacks `Permission::ManageAnalytics` in their active
    /// workspace.
    Denied,
}

/// The single "may this user use analytics?" decision (KYO-260).
///
/// Before this existed, the question was answered independently in three
/// places with three different subsets of these conditions: the Settings
/// tab bar (`settings_shell.rs`) checked all three correctly, but the
/// "Analytics Settings" link on the datasources page (`datasources.rs`)
/// gated only on `ds.is_analytics` — a datasource *type*, not a permission
/// — and the analytics page itself (`analytics.rs`) guarded only on
/// `is_self_hosted`. A non-admin workspace member could see and click the
/// link, land on a page with no permission guard, and get an empty shell
/// because every server fn behind it silently rejected them. Consolidating
/// the decision here means every consumer sees the same answer and a
/// future new condition only needs to be added once.
///
/// Precedence — self-hosted is checked first so a self-hosted *admin*'s
/// message is unchanged from before this fix ("not available in
/// self-hosted mode"), even though they would also fail the (on
/// self-hosted, irrelevant) billing check.
pub fn analytics_access(ctx: &UserContext) -> AnalyticsAccess {
    if ctx.is_self_hosted {
        AnalyticsAccess::SelfHosted
    } else if !ctx.can(Permission::ManageAnalytics) {
        AnalyticsAccess::Denied
    } else if !ctx.billing_enabled {
        AnalyticsAccess::BillingDisabled
    } else {
        AnalyticsAccess::Allowed
    }
}

/// Derive [`AnalyticsAccess`] from a possibly-not-yet-loaded, possibly-failed
/// `UserContext` fetch. Same shape as [`permissions_from`] — fails closed to
/// `Denied`, logs only the failed-fetch case (KYO-240).
fn analytics_access_from(user_ctx: Option<Result<UserContext, ServerFnError>>) -> AnalyticsAccess {
    user_ctx
        .and_then(|result| {
            result
                .map_err(|e| {
                    tracing::warn!(
                        error = %e,
                        "use_analytics_access: user context fetch failed, failing closed to Denied"
                    );
                })
                .ok()
        })
        .map(|ctx| analytics_access(&ctx))
        .unwrap_or(AnalyticsAccess::Denied)
}

/// Reactive wrapper around [`analytics_access`] over the shared
/// `UserContext` resource (KYO-260), mirroring [`use_permissions`].
///
/// Fails closed to `Denied` while the resource is loading, errored, or
/// absent — a page gating on this must never treat "we don't know yet" as
/// permission to render analytics content.
pub fn use_analytics_access() -> Signal<AnalyticsAccess> {
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();
    Signal::derive(move || analytics_access_from(user_ctx.get()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tracing::Level;

    use kyomi_test_tracing::capture_tracing;

    use super::*;

    /// Minimal `UserContext` fixture, built the way `settings_shell.rs`'s
    /// `ctx()` test helper is — only the fields `analytics_access` reads
    /// vary between cases; everything else is a neutral default.
    fn ctx(is_self_hosted: bool, billing_enabled: bool, permissions: Vec<Permission>) -> UserContext {
        UserContext {
            user_id: "user-1".to_string(),
            email: "user@example.com".to_string(),
            name: None,
            workspace_id: Some("ws-1".to_string()),
            workspace_name: Some("Test Workspace".to_string()),
            is_owner: false,
            subscription_tier: "free".to_string(),
            subscription_status: "active".to_string(),
            is_personal_mode: false,
            is_self_hosted,
            billing_enabled,
            capabilities: HashMap::new(),
            chart_palette: "balanced".to_string(),
            permissions,
        }
    }

    #[test]
    fn allowed_for_non_self_hosted_admin_with_billing() {
        let access = analytics_access(&ctx(false, true, vec![Permission::ManageAnalytics]));
        assert_eq!(access, AnalyticsAccess::Allowed);
    }

    #[test]
    fn self_hosted_for_self_hosted_admin() {
        // Precedence case: self-hosted wins even though this admin also
        // holds ManageAnalytics and billing_enabled is true — the
        // self-hosted message must be unchanged from before KYO-260.
        let access = analytics_access(&ctx(true, true, vec![Permission::ManageAnalytics]));
        assert_eq!(access, AnalyticsAccess::SelfHosted);
    }

    #[test]
    fn self_hosted_wins_over_denied_and_billing_disabled() {
        // A self-hosted non-admin with billing disabled must still report
        // SelfHosted, not Denied or BillingDisabled — self-hosted is
        // checked first regardless of the other two conditions.
        let access = analytics_access(&ctx(true, false, vec![]));
        assert_eq!(access, AnalyticsAccess::SelfHosted);
    }

    #[test]
    fn denied_for_non_admin() {
        let access = analytics_access(&ctx(false, true, vec![]));
        assert_eq!(access, AnalyticsAccess::Denied);
    }

    #[test]
    fn denied_takes_precedence_over_billing_disabled_for_non_admin() {
        // Precedence case: a non-admin with billing disabled must report
        // Denied (permission is checked first), not BillingDisabled.
        let access = analytics_access(&ctx(false, false, vec![]));
        assert_eq!(access, AnalyticsAccess::Denied);
    }

    #[test]
    fn billing_disabled_for_admin_without_billing() {
        let access = analytics_access(&ctx(false, false, vec![Permission::ManageAnalytics]));
        assert_eq!(access, AnalyticsAccess::BillingDisabled);
    }

    // ── KYO-240: failed fetches must fail closed AND log ──────────────────

    #[test]
    fn permissions_from_failed_fetch_yields_no_permissions_and_logs() {
        let logs = capture_tracing();

        let permissions = permissions_from(Some(Err(ServerFnError::new("simulated network failure"))));

        // Fail-closed: a failed fetch must still yield zero permissions,
        // exactly like before KYO-240.
        assert!(
            permissions.is_empty(),
            "a failed UserContext fetch must fail closed to zero permissions, got {permissions:?}"
        );

        // KYO-240: unlike before, the failure must also be diagnosable.
        let warnings = logs.events_at(Level::WARN);
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one WARN-level diagnostic for the failed fetch; captured: {:?}",
            logs.events()
        );
        assert!(
            warnings[0].1.contains("simulated network failure"),
            "the diagnostic should carry the underlying error so the failure can be root-caused; \
             captured: {:?}",
            warnings[0].1
        );
    }

    #[test]
    fn permissions_from_loading_yields_no_permissions_without_logging() {
        // The load-bearing counterpart to the test above: `None` (resource
        // still loading) is not a failure and must stay silent, or every
        // page render would spam a warning while `UserContext` resolves.
        let logs = capture_tracing();

        let permissions = permissions_from(None);

        assert!(permissions.is_empty());
        assert!(
            logs.events().is_empty(),
            "a merely-loading resource must not log a warning; captured: {:?}",
            logs.events()
        );
    }

    #[test]
    fn permissions_from_success_returns_the_fetched_permissions_without_logging() {
        let logs = capture_tracing();

        let permissions = permissions_from(Some(Ok(ctx(false, true, vec![Permission::ManageTeam]))));

        assert_eq!(permissions, vec![Permission::ManageTeam]);
        assert!(logs.events().is_empty());
    }

    #[test]
    fn analytics_access_from_failed_fetch_yields_denied_and_logs() {
        let logs = capture_tracing();

        let access = analytics_access_from(Some(Err(ServerFnError::new("simulated timeout"))));

        assert_eq!(access, AnalyticsAccess::Denied);
        let warnings = logs.events_at(Level::WARN);
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one WARN-level diagnostic for the failed fetch; captured: {:?}",
            logs.events()
        );
    }

    #[test]
    fn analytics_access_from_loading_yields_denied_without_logging() {
        let logs = capture_tracing();

        let access = analytics_access_from(None);

        assert_eq!(access, AnalyticsAccess::Denied);
        assert!(logs.events().is_empty());
    }
}
