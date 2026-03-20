// SPDX-License-Identifier: AGPL-3.0-or-later

//! Root application component and router.

use leptos::prelude::*;
use leptos_meta::provide_meta_context;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};

use crate::components::{Layout, ThemeProvider};
use crate::pages::settings::profile::ProfilePage;

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
                        <Route path=path!("/settings/profile") view=ProfilePage/>
                    </Routes>
                </Layout>
            </Router>
        </ThemeProvider>
    }
}
