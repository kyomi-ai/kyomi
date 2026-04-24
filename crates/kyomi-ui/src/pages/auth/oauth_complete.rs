// SPDX-License-Identifier: AGPL-3.0-or-later

//! OAuth completion page — shown after a successful OAuth flow in a popup/tab.
//!
//! Route: `/oauth-complete`
//!
//! The OAuth flow opens this page in a popup or new tab. Once the OAuth provider
//! redirects here, the user is authenticated. This page simply confirms success
//! and lets the user close the tab or navigate to the main app.
//!
//! No server calls — static content only. The auth session is already established
//! by the backend redirect that landed the user here.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{ButtonLink, ButtonVariant};
use crate::pages::auth::auth_layout::AuthLayout;

#[component]
pub fn OAuthCompletePage() -> impl IntoView {
    view! {
        <AuthLayout
            title=Signal::derive(|| "Authentication Complete".to_string())
            subtitle=Signal::derive(|| "Your authentication was successful.".to_string())
        >
            <div class="flex flex-col items-center gap-6 py-4">
                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-success/10">
                    <Icon
                        icon=phosphor_leptos::CHECK_CIRCLE
                        weight=IconWeight::Fill
                        attr:class="w-8 h-8 text-success-foreground"
                    />
                </div>
                <p class="text-sm text-muted-foreground text-center">
                    "You may close this tab and return to the application."
                </p>
                <ButtonLink href="/" variant=ButtonVariant::Outline class="w-full">
                    "Go to Dashboards"
                </ButtonLink>
            </div>
        </AuthLayout>
    }
}
