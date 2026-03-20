// SPDX-License-Identifier: AGPL-3.0-or-later

//! kyomi-ui — Leptos frontend for Kyomi.
//!
//! This crate contains the Leptos components and server functions that
//! progressively replace the React frontend. Both SSR (server) and
//! hydrate (WASM) targets are supported via feature flags.

pub mod app;
pub mod components;
pub mod pages;
pub mod server_fns;
pub mod types;

pub use app::App;

/// Register all server functions with the Leptos runtime.
///
/// Must be called once at server startup before building the Axum router.
/// This makes server function endpoints available for the WASM client to call.
#[cfg(feature = "ssr")]
pub fn register_server_functions() {
    use leptos::server_fn::axum::register_explicit;
    use server_fns::context::*;
    register_explicit::<GetUserContext>();

    use server_fns::profile::*;
    register_explicit::<GetProfile>();
    register_explicit::<GetDashboards>();
    register_explicit::<GetPendingInvitations>();
    register_explicit::<UpdateProfileName>();
    register_explicit::<UpdateTheme>();
    register_explicit::<UpdateLandingPage>();
    register_explicit::<UpdateDefaultDashboard>();
    register_explicit::<UpdateQueryRetention>();
    register_explicit::<UpdateChartPalette>();
    register_explicit::<AcceptInvitation>();
    register_explicit::<DeclineInvitation>();

    use server_fns::sidebar::*;
    register_explicit::<GetRecentSessions>();
    register_explicit::<GetSidebarUser>();

    #[cfg(feature = "slack")]
    {
        use server_fns::slack::*;
        register_explicit::<GetSlackStatus>();
        register_explicit::<SlackConnect>();
        register_explicit::<SlackDisconnect>();
        register_explicit::<GetSlackChannels>();
        register_explicit::<GetDefaultWatchChannel>();
        register_explicit::<SetDefaultWatchChannel>();
    }
}
