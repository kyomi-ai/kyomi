// SPDX-License-Identifier: AGPL-3.0-or-later
#![recursion_limit = "512"]

//! kyomi-ui — Leptos frontend for Kyomi.
//!
//! This crate contains the Leptos components and server functions that
//! progressively replace the React frontend. Both SSR (server) and
//! hydrate (WASM) targets are supported via feature flags.

pub mod app;
pub mod components;
pub mod datasource;
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
    use server_fns::auth::*;
    register_explicit::<GetAuthConfig>();
    register_explicit::<LoginWithPassword>();

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

    use server_fns::security::*;
    register_explicit::<HasPassword>();
    register_explicit::<SetPassword>();
    register_explicit::<ChangePassword>();
    register_explicit::<GetTotpStatus>();
    register_explicit::<SetupTotp>();
    register_explicit::<EnableTotp>();
    register_explicit::<DisableTotp>();
    register_explicit::<GetSessions>();
    register_explicit::<RevokeSession>();
    register_explicit::<LogoutAllSessions>();
    register_explicit::<ListPasskeys>();
    register_explicit::<StartPasskeyRegistration>();
    register_explicit::<CompletePasskeyRegistration>();
    register_explicit::<DeletePasskey>();
    register_explicit::<RenamePasskey>();

    use server_fns::sidebar::*;
    register_explicit::<GetRecentSessions>();
    register_explicit::<GetSidebarUser>();

    use server_fns::usage::*;
    register_explicit::<GetAiUsageStatus>();

    use server_fns::analytics::*;
    register_explicit::<ListAnalyticsSites>();
    register_explicit::<GetAnalyticsUsage>();
    register_explicit::<CreateAnalyticsSite>();
    register_explicit::<UpdateAnalyticsSite>();
    register_explicit::<DeleteAnalyticsSite>();

    use server_fns::team::*;
    register_explicit::<ListWorkspaceMembers>();
    register_explicit::<UpdateMemberRole>();
    register_explicit::<RemoveMember>();
    register_explicit::<ListWorkspaceInvitations>();
    register_explicit::<InviteMember>();
    register_explicit::<CancelInvitation>();
    register_explicit::<ListOwnershipTransfers>();
    register_explicit::<CancelOwnershipTransfer>();

    use server_fns::datasources::*;
    register_explicit::<ListDatasources>();
    register_explicit::<GetDatasourceTypes>();
    register_explicit::<ToggleDatasource>();
    register_explicit::<DeleteDatasource>();
    register_explicit::<CreateDatasourceModal>();
    register_explicit::<UpdateDatasourceSettings>();
    register_explicit::<SaveDatasourceCredentials>();
    register_explicit::<GetDatasourceSettings>();
    register_explicit::<TestDatasourceStandalone>();
    register_explicit::<TestExistingDatasource>();
    register_explicit::<DiscoverDatasourceResources>();
    register_explicit::<QueryDatasourceArrow>();

    use server_fns::billing::*;
    register_explicit::<GetSubscriptionInfo>();
    register_explicit::<GetInvoices>();
    register_explicit::<CreateCheckout>();
    register_explicit::<CancelSubscription>();
    register_explicit::<ReactivateSubscription>();
    register_explicit::<UpdateTeamSize>();
    register_explicit::<CreatePortalSession>();

    use server_fns::workspace::*;
    register_explicit::<GetWorkspaceSettings>();
    register_explicit::<UpdateWorkspaceName>();
    register_explicit::<UpdateWorkspaceModel>();
    register_explicit::<UpdateWorkspaceChartmlConfig>();
    register_explicit::<PopulateKnowledgeGraph>();

    #[cfg(feature = "slack")]
    {
        use server_fns::slack::*;
        register_explicit::<GetSlackStatus>();
        register_explicit::<SlackConnect>();
        register_explicit::<SlackDisconnect>();
        register_explicit::<GetSlackChannels>();
        register_explicit::<GetDefaultWatchChannel>();
        register_explicit::<SetDefaultWatchChannel>();

        // Workspace-level Slack server functions
        use server_fns::workspace::{
            GetWorkspaceSlackStatus, GetSlackInstallUrl, UninstallWorkspaceSlack,
        };
        register_explicit::<GetWorkspaceSlackStatus>();
        register_explicit::<GetSlackInstallUrl>();
        register_explicit::<UninstallWorkspaceSlack>();
    }
}
