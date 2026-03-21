// SPDX-License-Identifier: AGPL-3.0-or-later

//! Root application component and router.

use leptos::prelude::*;
use leptos_meta::provide_meta_context;
use leptos_router::{
    components::{Outlet, ParentRoute, Route, Router, Routes},
    path,
};

use crate::components::{Layout, ThemeProvider};
use crate::pages::auth::google_callback::GoogleCallbackPage;
use crate::pages::auth::login::LoginPage;
use crate::pages::auth::signup_complete::SignupCompletePage;
use crate::pages::settings::analytics::AnalyticsPage;
use crate::pages::settings::datasources::DatasourcesPage;
use crate::pages::settings::profile::ProfilePage;
use crate::pages::settings::security::SecurityTab;
use crate::pages::settings::settings_shell::SettingsShell;
use crate::pages::settings::billing::BillingPage;
use crate::pages::settings::team::TeamPage;
use crate::pages::settings::usage::UsagePage;
use crate::pages::settings::workspace::WorkspacePage;

/// Shell HTML page that loads the WASM bundle.
///
/// Used for SSR to render the outer HTML document. In CSR mode,
/// this is served as a static HTML file.
#[component]
pub fn Shell(#[prop(optional)] children: Option<Children>) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en" class="dark">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <title>"Kyomi — Settings"</title>
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

    view! {
        <ThemeProvider initial_preference="system">
            <Router>
                <Layout>
                    <Routes fallback=|| view! { <p>"Page not found"</p> }>
                        // Auth pages
                        <Route path=path!("/login") view=LoginPage/>
                        <Route path=path!("/signup/complete") view=SignupCompletePage/>
                        <Route path=path!("/auth/google/callback") view=GoogleCallbackPage/>
                        // SettingsShell mounts once; child routes swap via <Outlet/>.
                        // No re-mount on tab navigation = no flicker.
                        <ParentRoute path=path!("/settings") view=|| view! {
                            <div class="flex flex-col h-full bg-muted overflow-x-hidden" style:flex-direction="column">
                                <div class="flex-1 overflow-y-auto p-4 md:p-6 relative">
                                    // Close button — matches React SettingsPage.jsx positioning
                                    <a
                                        href="/"
                                        class="absolute top-4 right-4 md:top-6 md:right-6 p-2 text-muted-foreground hover:text-foreground hover:bg-accent rounded-lg transition-colors z-10"
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
                </Layout>
            </Router>
        </ThemeProvider>
    }
}
