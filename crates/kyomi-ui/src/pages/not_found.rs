// SPDX-License-Identifier: AGPL-3.0-or-later

//! 404 Not Found page — shown when no route matches.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{ButtonLink, ButtonVariant};

/// Full-page 404 component following the Empty State Pattern from DESIGN.md.
///
/// Wrapped in `<Layout>` by the router fallback, so the sidebar and auth guard
/// are already present. If an unauthenticated user hits a 404 route, Layout
/// redirects them to `/login` before this component ever renders.
#[component]
pub fn NotFoundPage() -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center min-h-[60vh] gap-6 text-center px-4">
            <div class="text-muted-foreground mx-auto mb-6 flex items-center justify-center">
                <Icon
                    icon=phosphor_leptos::COMPASS
                    weight=IconWeight::Duotone
                    size="64px"
                />
            </div>
            <div class="space-y-2">
                <h1 class="text-xl font-semibold text-foreground mb-2">
                    "We couldn\u{2019}t find that page"
                </h1>
                <p class="text-muted-foreground max-w-md">
                    "The link may be broken, or the page may have moved."
                </p>
            </div>
            <div class="flex items-center gap-3">
                <ButtonLink href="/">
                    "Go to dashboards"
                </ButtonLink>
                <ButtonLink href="/chat" variant=ButtonVariant::Ghost>
                    "Open chat"
                </ButtonLink>
            </div>
        </div>
    }
}
