// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reactive helpers for permission-based UI gating.
//!
//! Wraps the shared `UserContext` resource so pages don't each hand-roll a
//! `Signal::derive` over `workspace_roles` / `permissions`. See
//! `server_fns::context::UserContext` for the source of truth — the
//! `permissions` set (KYO-189 P2) is computed server-side by
//! `kyomi_auth::permissions::permissions_for` and shipped to the client
//! as-is; nothing on this side re-derives it.

use leptos::prelude::*;

use kyomi_types::Permission;

use crate::server_fns::context::UserContext;

/// Reactive `is workspace admin` derived from the shared `UserContext`
/// resource provided by the settings shell (see `settings_shell.rs`).
///
/// Fails closed: `false` while the resource is loading, errored, or absent.
/// Must be called from a component that renders under the settings shell (or
/// any ancestor that provides the `LocalResource<Result<UserContext,
/// ServerFnError>>` context) — panics via `expect_context` otherwise.
pub fn use_is_workspace_admin() -> Signal<bool> {
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();
    Signal::derive(move || {
        user_ctx
            .get()
            .and_then(|r| r.ok())
            .map(|ctx| ctx.is_workspace_admin())
            .unwrap_or(false)
    })
}

/// Reactive permission checker over the shared `UserContext` resource.
///
/// Returned by [`use_permissions`]. Call `.can(permission)` inside a
/// reactive closure (a `Signal::derive`, a `<Show when=move || ...>`, etc.)
/// to gate a UI surface on the exact [`Permission`] its corresponding server
/// fn requires — never on `workspace_roles` or `is_owner` directly.
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

/// Reactive permission set derived from the shared `UserContext` resource.
///
/// The single lookup helper every UI gate should use (KYO-189 P2) — wraps
/// the same `expect_context::<LocalResource<...>>()` boilerplate as
/// [`use_is_workspace_admin`] so no page re-implements the resource read.
/// Must be called from a component that renders under the `Layout` (or any
/// ancestor that provides the `LocalResource<Result<UserContext,
/// ServerFnError>>` context) — panics via `expect_context` otherwise.
pub fn use_permissions() -> Permissions {
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();
    Permissions(Signal::derive(move || {
        user_ctx
            .get()
            .and_then(|r| r.ok())
            .map(|ctx| ctx.permissions)
            .unwrap_or_default()
    }))
}
