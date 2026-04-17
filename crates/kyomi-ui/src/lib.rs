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
pub mod parser;
pub mod query_cache;
pub mod server_fns;
pub mod types;
pub mod utils;

#[cfg(target_arch = "wasm32")]
mod wasm_math_shims;

pub use app::App;

/// Register all server functions with the Leptos runtime.
///
/// Must be called once at server startup before building the Axum router.
/// This makes server function endpoints available for the WASM client to call.
#[cfg(feature = "ssr")]
pub fn register_server_functions() {
    use leptos::server_fn::axum::register_explicit;

    use server_fns::ai::*;
    register_explicit::<GetWorkspaceAiConfig>();
    register_explicit::<UpdateWorkspaceAiConfig>();
    register_explicit::<TestWorkspaceAiConfig>();

    use server_fns::auth::*;
    register_explicit::<GetAuthConfig>();
    register_explicit::<LoginWithPassword>();
    register_explicit::<SignupStart>();
    register_explicit::<SignupComplete>();
    register_explicit::<GoogleOauthCallback>();
    register_explicit::<ResendVerification>();
    register_explicit::<RecoveryStart>();
    register_explicit::<RecoveryVerify>();
    register_explicit::<RecoverySetPassword>();
    register_explicit::<PasskeyLoginStart>();
    register_explicit::<PasskeyLoginComplete>();
    register_explicit::<PasskeyRegisterStart>();
    register_explicit::<PasskeyRegisterComplete>();
    register_explicit::<PasskeySignupComplete>();
    register_explicit::<PasskeyRecoveryVerify>();

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

    use server_fns::chat::*;
    register_explicit::<GetChartContext>();
    register_explicit::<GetWebsocketConfig>();
    register_explicit::<ListChatSessions>();
    register_explicit::<GetSessionMessages>();
    register_explicit::<UpdateSessionTitle>();
    register_explicit::<DeleteChatSession>();
    register_explicit::<BulkDeleteSessions>();
    register_explicit::<SearchChatMessages>();
    register_explicit::<SendChatMessage>();
    register_explicit::<ShareSession>();
    register_explicit::<UnshareSession>();
    register_explicit::<MarkSessionRead>();
    register_explicit::<ToggleMessagePin>();
    register_explicit::<UpdateMessageContent>();

    use server_fns::copilot::*;
    register_explicit::<CreateCopilotSession>();
    register_explicit::<SendCopilotMessage>();
    register_explicit::<DeleteCopilotSession>();

    use server_fns::collections::*;
    register_explicit::<ListCollections>();
    register_explicit::<CreateCollection>();
    register_explicit::<UpdateCollection>();
    register_explicit::<DeleteCollection>();
    register_explicit::<AddDashboardToCollection>();
    register_explicit::<RemoveDashboardFromCollection>();

    use server_fns::dashboards::*;
    register_explicit::<ListDashboards>();
    register_explicit::<GetDashboard>();
    register_explicit::<CreateDashboard>();
    register_explicit::<UpdateDashboard>();
    register_explicit::<DeleteDashboard>();
    register_explicit::<ListVersions>();
    register_explicit::<GetVersion>();
    register_explicit::<DiffVersions>();
    register_explicit::<RestoreVersion>();
    register_explicit::<GetUserDefaultDashboard>();
    register_explicit::<SetUserDefaultDashboard>();
    register_explicit::<GetWorkspaceDefaultDashboard>();
    register_explicit::<SetWorkspaceDefaultDashboard>();

    use server_fns::sql_editor::*;
    register_explicit::<ExecuteSqlQuery>();
    register_explicit::<FetchQueryPage>();
    register_explicit::<DryRunSql>();
    register_explicit::<StartQueryStream>();
    register_explicit::<ListQueryHistory>();
    register_explicit::<SaveQueryHistory>();
    register_explicit::<UpdateQueryHistory>();
    register_explicit::<DeleteQueryHistory>();
    register_explicit::<GetCatalogTree>();
    register_explicit::<SearchCatalog>();
    register_explicit::<RefreshCatalog>();
    register_explicit::<GetTableInfo>();
    register_explicit::<GenerateChartFromResults>();
    register_explicit::<GetWsConnectionInfo>();

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
    register_explicit::<CreatePortalSession>();
    register_explicit::<PurchaseAiBundle>();
    register_explicit::<PurchaseAnalyticsBundle>();
    register_explicit::<GetStripePublishableKey>();
    register_explicit::<GetCheckoutSessionStatus>();
    register_explicit::<UpdateUserLimit>();

    use server_fns::workspace::*;
    register_explicit::<GetWorkspaceSettings>();
    register_explicit::<UpdateWorkspaceName>();
    register_explicit::<UpdateWorkspaceModel>();
    register_explicit::<UpdateWorkspaceChartmlConfig>();
    register_explicit::<PopulateKnowledgeGraph>();

    use server_fns::knowledge::*;
    register_explicit::<ListKnowledgeDocs>();
    register_explicit::<CreateKnowledgeDoc>();
    register_explicit::<DeleteKnowledgeDoc>();

    use server_fns::watches::*;
    register_explicit::<ListWatches>();
    register_explicit::<CreateWatch>();
    register_explicit::<GetWatch>();
    register_explicit::<UpdateWatch>();
    register_explicit::<DeleteWatch>();
    register_explicit::<ToggleWatch>();
    register_explicit::<RunWatchNow>();
    register_explicit::<GetWatchExecutions>();
    register_explicit::<GetWatchExecution>();
    register_explicit::<GetAlerts>();
    register_explicit::<GetUnreadAlertsCount>();
    register_explicit::<MarkAlertRead>();
    register_explicit::<MarkAlertUnread>();
    register_explicit::<DeleteAlert>();
    register_explicit::<RestoreAlert>();
    register_explicit::<BulkDeleteAlerts>();
    register_explicit::<BulkMarkAlertsRead>();
    register_explicit::<BulkMarkAlertsUnread>();
    register_explicit::<ContinueAlertInChat>();
    register_explicit::<GetLastExecution>();
    register_explicit::<GetThinkingEvents>();

    use server_fns::onboarding::*;
    register_explicit::<AcceptTerms>();
    register_explicit::<GetOnboardingState>();
    register_explicit::<CreateSampleDatasource>();
    register_explicit::<CheckSampleDatasourceAvailable>();
    register_explicit::<GetOauthConnectUrl>();

    use server_fns::home::*;
    register_explicit::<GetLandingConfig>();

    use server_fns::unsubscribe::*;
    register_explicit::<UnsubscribeEmail>();

    use server_fns::ownership::*;
    register_explicit::<GetOwnershipTransfer>();
    register_explicit::<AcceptOwnershipTransfer>();
    register_explicit::<DeclineOwnershipTransfer>();

    use server_fns::setup::*;
    register_explicit::<CheckHasDatasources>();

    use server_fns::connect::*;
    register_explicit::<ListConnectDatasources>();
    register_explicit::<CreateConnectDatasource>();
    register_explicit::<RotateConnectToken>();

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
