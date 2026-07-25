// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reactive helpers for role-based UI gating.
//!
//! Wraps the shared `UserContext` resource so pages don't each hand-roll a
//! `Signal::derive` over `workspace_roles`. See `server_fns::context::UserContext`
//! for the source of truth on what "admin" means.

use leptos::prelude::*;

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
