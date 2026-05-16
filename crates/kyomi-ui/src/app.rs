// SPDX-License-Identifier: AGPL-3.0-or-later

//! Root application component and router.

use leptos::prelude::*;
use leptos_meta::provide_meta_context;
use leptos_router::{
    components::{Outlet, ParentRoute, Redirect, Route, Router, Routes},
    path,
};

use crate::components::toast::ToastProvider;
use crate::components::{Layout, NavigationProgress, ThemeProvider};
use crate::pages::accept_ownership::AcceptOwnershipPage;
use crate::pages::billing_return::BillingReturnPage;
use crate::pages::auth::account_recovery::AccountRecoveryPage;
use crate::pages::auth::account_recovery_complete::AccountRecoveryCompletePage;
use crate::pages::auth::datasource_oauth_callback::DatasourceOAuthCallbackPage;
use crate::pages::auth::google_callback::GoogleCallbackPage;
use crate::pages::auth::google_link_callback::GoogleLinkCallbackPage;
use crate::pages::auth::login::LoginPage;
use crate::pages::auth::passkey_recovery::PasskeyRecoveryPage;
use crate::pages::auth::oauth_complete::OAuthCompletePage;
use crate::pages::auth::passkey_recovery_complete::PasskeyRecoveryCompletePage;
use crate::pages::auth::passkey_signup_complete::PasskeySignupCompletePage;
use crate::pages::auth::signup_complete::SignupCompletePage;
use crate::pages::auth::verify_email::VerifyEmailPage;
use crate::pages::chat::{ChatPage, ChatsListPage};
use crate::pages::connect_setup::ConnectSetupPage;
use crate::pages::dashboards::{DashboardEditorPage, DashboardsListPage, DashboardViewerPage};
use crate::pages::home::HomePage;
use crate::pages::inbox::InboxPage;
use crate::pages::knowledge::KnowledgePage;
use crate::pages::not_found::NotFoundPage;
use crate::pages::onboarding::DatasourceOnboardingPage;
use crate::pages::settings::ai::AiPage;
use crate::pages::settings::analytics::AnalyticsPage;
use crate::pages::settings::billing::BillingPage;
use crate::pages::settings::datasources::DatasourcesPage;
use crate::pages::settings::profile::ProfilePage;
use crate::pages::settings::security::SecurityTab;
use crate::pages::settings::settings_shell::SettingsShell;
use crate::pages::settings::team::TeamPage;
use crate::pages::settings::usage::UsagePage;
use crate::pages::settings::workspace::WorkspacePage;
use crate::pages::setup::personal_setup::PersonalSetupPage;
use crate::pages::unsubscribe::UnsubscribePage;
use crate::pages::watches::WatchesPage;
use crate::pages::welcome::WelcomePage;

/// Shell HTML page that loads the WASM bundle.
///
/// Used for SSR to render the outer HTML document. In CSR mode,
/// this is served as a static HTML file.
#[component]
pub fn Shell(#[prop(optional)] children: Option<Children>) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"/>
                <title>"Kyomi"</title>
                <leptos_meta::MetaTags/>
            </head>
            <body class="min-h-screen bg-background text-foreground antialiased">
                {children.map(|c| c())}
            </body>
        </html>
    }
}

/// Root Leptos application component.
///
/// Wraps everything in ThemeProvider (defaults to "system" until profile loads).
/// The ProfilePage updates the theme when the user's preference is known.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    let (is_routing, set_is_routing) = signal(false);
    provide_context(is_routing);

    view! {
        <ThemeProvider initial_preference="system">
            <ToastProvider>
            <NavigationProgress is_routing />
            <Router set_is_routing>
                <Routes fallback=|| view! { <Layout><NotFoundPage/></Layout> }>
                    // Auth pages — NO sidebar/layout wrapper
                    <Route path=path!("/login") view=|| view! { <LoginPage/> }/>
                    <Route path=path!("/signup") view=|| view! { <LoginPage signup_mode=true/> }/>
                    <Route path=path!("/signup/complete") view=SignupCompletePage/>
                    <Route path=path!("/auth/google/callback") view=GoogleCallbackPage/>
                    <Route path=path!("/account/recover") view=AccountRecoveryPage/>
                    <Route path=path!("/account/recover/complete") view=AccountRecoveryCompletePage/>
                    <Route path=path!("/auth/passkey-signup") view=PasskeySignupCompletePage/>
                    <Route path=path!("/auth/recover-passkey") view=PasskeyRecoveryPage/>
                    <Route path=path!("/auth/recover-passkey/complete") view=PasskeyRecoveryCompletePage/>
                    <Route path=path!("/verify-email") view=VerifyEmailPage/>
                    <Route path=path!("/verify") view=|| view! { <Redirect path="/verify-email"/> }/>
                    <Route path=path!("/oauth-complete") view=OAuthCompletePage/>
                    <Route path=path!("/auth/google/link-callback") view=GoogleLinkCallbackPage/>
                    <Route path=path!("/auth/oauth/:provider/callback") view=DatasourceOAuthCallbackPage/>
                    // Public pages — NO layout, NO auth
                    // `/try` (trial chat) was removed; redirect external links to /login.
                    <Route path=path!("/try") view=|| view! { <Redirect path="/login"/> }/>
                    <Route path=path!("/welcome") view=WelcomePage/>
                    <Route path=path!("/unsubscribe") view=UnsubscribePage/>
                    <Route path=path!("/billing/return") view=BillingReturnPage/>
                    // Flow pages — NO sidebar, requires auth (standalone layout)
                    <Route path=path!("/onboarding") view=DatasourceOnboardingPage/>
                    <Route path=path!("/onboarding/catalog") view=|| view! { <Redirect path="/onboarding"/> }/>
                    <Route path=path!("/setup") view=PersonalSetupPage/>
                    <Route path=path!("/connect/setup") view=ConnectSetupPage/>
                    <Route path=path!("/accept-ownership/:transfer_id") view=AcceptOwnershipPage/>
                    // ── Authenticated app shell ──────────────────────────
                    // ALL routes below mount under a single Layout instance
                    // via this ParentRoute. The Layout — and the WebSocket
                    // connection it owns — persists across navigation instead
                    // of being torn down and rebuilt on every page change.
                    // Previously each route wrapped its own <Layout>, so each
                    // navigation created a fresh WebSocket; after 10 navs the
                    // server's MAX_CONNECTIONS_PER_USER (10) was hit and every
                    // subsequent connection was rejected with close code 4029.
                    <ParentRoute path=path!("") view=|| view! { <Layout><Outlet/></Layout> }>
                        <Route path=path!("/") view=HomePage/>
                        <Route path=path!("/dashboards") view=DashboardsListPage/>
                        <Route path=path!("/dashboard/:id") view=DashboardViewerPage/>
                        <Route path=path!("/dashboard/:id/edit") view=DashboardEditorPage/>
                        // Chat — nested ParentRoute so ChatPage stays mounted
                        // across /chat ↔ /chat/:session_id transitions.
                        <ParentRoute path=path!("/chat") view=ChatPage>
                            <Route path=path!("") view=|| view! { <Outlet/> }/>
                            <Route path=path!("/:session_id") view=|| view! { <Outlet/> }/>
                        </ParentRoute>
                        <Route path=path!("/chats") view=ChatsListPage/>
                        <Route path=path!("/sql-editor") view=crate::pages::sql_editor::SqlEditorPage/>
                        <Route path=path!("/knowledge") view=KnowledgePage/>
                        // Knowledge docs reuse the dashboard viewer/editor.
                        <Route path=path!("/knowledge/:id") view=DashboardViewerPage/>
                        <Route path=path!("/knowledge/:id/edit") view=DashboardEditorPage/>
                        <Route path=path!("/inbox") view=InboxPage/>
                        <Route path=path!("/watches") view=WatchesPage/>
                        // Settings — nested ParentRoute provides the SettingsShell
                        // chrome around the settings tab routes.
                        <ParentRoute path=path!("/settings") view=|| view! {
                            <div class="flex flex-col h-full bg-muted overflow-x-hidden" style:flex-direction="column">
                                <div class="flex-1 overflow-y-auto p-4 md:p-6 relative">
                                    // Close button — matches React SettingsPage.jsx positioning
                                    <a
                                        href="/"
                                        class="absolute top-4 right-4 md:top-6 md:right-6 p-2 text-muted-foreground hover:text-foreground hover:bg-secondary rounded-lg transition-colors z-10"
                                        aria-label="Close settings"
                                    >
                                        <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>
                                        </svg>
                                    </a>
                                    <div class="w-full">
                                        <SettingsShell/>
                                    </div>
                                </div>
                            </div>
                        }>
                            <Route path=path!("") view=|| view! { <Redirect path="/settings/profile"/> }/>
                            <Route path=path!("/profile") view=ProfilePage/>
                            <Route path=path!("/security") view=SecurityTab/>
                            <Route path=path!("/workspace") view=WorkspacePage/>
                            <Route path=path!("/datasources") view=DatasourcesPage/>
                            <Route path=path!("/ai") view=AiPage/>
                            <Route path=path!("/analytics") view=AnalyticsPage/>
                            <Route path=path!("/usage") view=UsagePage/>
                            <Route path=path!("/billing") view=BillingPage/>
                            <Route path=path!("/team") view=TeamPage/>
                        </ParentRoute>
                    </ParentRoute>
                </Routes>
            </Router>
            </ToastProvider>
        </ThemeProvider>
    }
}
