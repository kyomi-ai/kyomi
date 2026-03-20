// SPDX-License-Identifier: AGPL-3.0-or-later

//! Card components — matches shadcn/ui Card pattern.
//!
//! Provides Card, CardHeader, CardContent, CardTitle, and CardDescription
//! as reusable building blocks for settings pages, dashboards, etc.

use leptos::prelude::*;

/// Card container with border, shadow, and rounded corners.
#[component]
pub fn Card(children: Children) -> impl IntoView {
    view! {
        <div class="bg-card rounded-xl shadow-sm border border-border overflow-hidden">
            {children()}
        </div>
    }
}

/// Card header section — typically contains title and description.
#[component]
pub fn CardHeader(children: Children) -> impl IntoView {
    view! { <div class="p-6 pb-2">{children()}</div> }
}

/// Card content section — the main body below the header.
#[component]
pub fn CardContent(children: Children) -> impl IntoView {
    view! { <div class="p-6 pt-4">{children()}</div> }
}

/// Card title — semibold heading text.
#[component]
pub fn CardTitle(children: Children) -> impl IntoView {
    view! { <h3 class="text-lg font-semibold text-foreground">{children()}</h3> }
}

/// Card description — muted text below the title.
#[component]
pub fn CardDescription(children: Children) -> impl IntoView {
    view! { <p class="text-sm text-muted-foreground mt-1">{children()}</p> }
}
