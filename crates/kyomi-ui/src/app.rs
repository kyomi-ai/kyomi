// SPDX-License-Identifier: AGPL-3.0-or-later

//! Root application component and router.

use leptos::prelude::*;
use leptos_meta::provide_meta_context;
use leptos_router::{
    components::{Outlet, ParentRoute, Redirect, Route, Router, Routes},
    path,
};

use crate::components::{Layout, NavigationProgress, ThemeProvider};
use crate::pages::accept_ownership::AcceptOwnershipPage;
use crate::pages::auth::account_recovery::AccountRecoveryPage;
use crate::pages::auth::account_recovery_complete::AccountRecoveryCompletePage;
use crate::pages::auth::google_callback::GoogleCallbackPage;
use crate::pages::auth::login::LoginPage;
use crate::pages::auth::passkey_recovery::PasskeyRecoveryPage;
use crate::pages::auth::passkey_recovery_complete::PasskeyRecoveryCompletePage;
use crate::pages::auth::passkey_signup_complete::PasskeySignupCompletePage;
use crate::pages::auth::signup_complete::SignupCompletePage;
use crate::pages::chat::{ChatPage, ChatsListPage};
use crate::pages::connect_setup::ConnectSetupPage;
use crate::pages::dashboards::{DashboardEditorPage, DashboardsListPage, DashboardViewerPage};
use crate::pages::home::HomePage;
use crate::pages::knowledge::KnowledgePage;
use crate::pages::not_implemented::NotImplementedPage;
use crate::pages::onboarding::DatasourceOnboardingPage;
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
use crate::pages::trial::TrialChatPage;
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
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
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

    view! {
        <ThemeProvider initial_preference="system">
            <NavigationProgress is_routing />
            <Router set_is_routing>
                <Routes fallback=|| view! { <p>"Page not found"</p> }>
                    // Auth pages — NO sidebar/layout wrapper
                    <Route path=path!("/login") view=LoginPage/>
                    <Route path=path!("/signup/complete") view=SignupCompletePage/>
                    <Route path=path!("/auth/google/callback") view=GoogleCallbackPage/>
                    <Route path=path!("/account/recover") view=AccountRecoveryPage/>
                    <Route path=path!("/account/recover/complete") view=AccountRecoveryCompletePage/>
                    <Route path=path!("/auth/passkey-signup") view=PasskeySignupCompletePage/>
                    <Route path=path!("/auth/recover-passkey") view=PasskeyRecoveryPage/>
                    <Route path=path!("/auth/recover-passkey/complete") view=PasskeyRecoveryCompletePage/>
                    // Auth pages — not yet implemented
                    <Route path=path!("/verify-email") view=|| view! { <NotImplementedPage name="Verify Email"/> }/>
                    <Route path=path!("/verify") view=|| view! { <NotImplementedPage name="Verify Email"/> }/>
                    <Route path=path!("/oauth-complete") view=|| view! { <NotImplementedPage name="OAuth Complete"/> }/>
                    <Route path=path!("/auth/slack-connect") view=|| view! { <NotImplementedPage name="Slack Connect"/> }/>
                    <Route path=path!("/auth/google/link-callback") view=|| view! { <NotImplementedPage name="Google Account Link"/> }/>
                    <Route path=path!("/auth/oauth/:provider/callback") view=|| view! { <NotImplementedPage name="OAuth Callback"/> }/>
                    // Public pages — NO layout, NO auth
                    <Route path=path!("/try") view=TrialChatPage/>
                    <Route path=path!("/welcome") view=WelcomePage/>
                    <Route path=path!("/unsubscribe") view=UnsubscribePage/>
                    // Flow pages — NO sidebar, requires auth (standalone layout)
                    <Route path=path!("/onboarding") view=DatasourceOnboardingPage/>
                    <Route path=path!("/onboarding/catalog") view=|| view! { <Redirect path="/onboarding"/> }/>
                    <Route path=path!("/setup") view=PersonalSetupPage/>
                    <Route path=path!("/connect/setup") view=ConnectSetupPage/>
                    <Route path=path!("/accept-ownership/:transfer_id") view=AcceptOwnershipPage/>
                    // Dashboard pages — wrapped in Layout (sidebar)
                    <Route path=path!("/dashboards") view=|| view! { <Layout><DashboardsListPage/></Layout> }/>
                    <Route path=path!("/dashboard/:id") view=|| view! { <Layout><DashboardViewerPage/></Layout> }/>
                    <Route path=path!("/dashboard/:id/edit") view=|| view! { <Layout><DashboardEditorPage/></Layout> }/>
                    // Main app pages — wrapped in Layout
                    <Route path=path!("/") view=|| view! { <Layout><HomePage/></Layout> }/>
                    // Chat pages — ParentRoute keeps ChatPage mounted when navigating
                    // from /chat (new chat) to /chat/:session_id (after session creation),
                    // so streaming state is not lost on the URL transition.
                    <ParentRoute path=path!("/chat") view=|| view! { <Layout><ChatPage/></Layout> }>
                        <Route path=path!("") view=|| view! { <Outlet/> }/>
                        <Route path=path!("/:session_id") view=|| view! { <Outlet/> }/>
                    </ParentRoute>
                    <Route path=path!("/chats") view=|| view! { <Layout><ChatsListPage/></Layout> }/>
                    <Route path=path!("/sql-editor") view=|| view! { <Layout><crate::pages::sql_editor::SqlEditorPage/></Layout> }/>
                    <Route path=path!("/knowledge") view=|| view! { <Layout><KnowledgePage/></Layout> }/>
                    <Route path=path!("/watches") view=|| view! { <Layout><WatchesPage/></Layout> }/>
                    <Route path=path!("/watches/:view") view=|| view! { <Layout><WatchesPage/></Layout> }/>
                    // Settings pages — wrapped in Layout (sidebar) + SettingsShell
                    <ParentRoute path=path!("/settings") view=|| view! {
                        <Layout>
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
                        </Layout>
                    }>
                        <Route path=path!("") view=|| view! { <Redirect path="/settings/profile"/> }/>
                        <Route path=path!("/profile") view=ProfilePage/>
                        <Route path=path!("/security") view=SecurityTab/>
                        <Route path=path!("/workspace") view=WorkspacePage/>
                        <Route path=path!("/datasources") view=DatasourcesPage/>
                        <Route path=path!("/analytics") view=AnalyticsPage/>
                        <Route path=path!("/usage") view=UsagePage/>
                        <Route path=path!("/billing") view=BillingPage/>
                        <Route path=path!("/team") view=TeamPage/>
                    </ParentRoute>
                </Routes>
            </Router>
        </ThemeProvider>
    }
}
