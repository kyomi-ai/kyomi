// SPDX-License-Identifier: AGPL-3.0-or-later

//! Card components — matches `apps/frontend/src/components/ui/card.jsx` exactly.

use leptos::prelude::*;

/// Card container.
/// React: `rounded-xl border border-border bg-card text-card-foreground shadow`
#[component]
pub fn Card(children: Children) -> impl IntoView {
    view! {
        <div class="rounded-xl border border-border bg-card text-card-foreground shadow">
            {children()}
        </div>
    }
}

/// Card header section.
/// React: `flex flex-col space-y-1.5 p-6`
#[component]
pub fn CardHeader(children: Children) -> impl IntoView {
    view! { <div class="flex flex-col space-y-1.5 p-6">{children()}</div> }
}

/// Card content section.
/// React: `p-6 pt-0`
#[component]
pub fn CardContent(children: Children) -> impl IntoView {
    view! { <div class="p-6 pt-0">{children()}</div> }
}

/// Card title.
/// React: `font-semibold leading-none tracking-tight`
#[component]
pub fn CardTitle(children: Children) -> impl IntoView {
    view! { <div class="font-semibold leading-none tracking-tight">{children()}</div> }
}

/// Card description.
/// React: `text-sm text-muted-foreground`
#[component]
pub fn CardDescription(children: Children) -> impl IntoView {
    view! { <div class="text-sm text-muted-foreground">{children()}</div> }
}

/// Card footer.
/// React: `flex items-center p-6 pt-0`
#[component]
pub fn CardFooter(children: Children) -> impl IntoView {
    view! { <div class="flex items-center p-6 pt-0">{children()}</div> }
}
