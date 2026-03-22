// SPDX-License-Identifier: AGPL-3.0-or-later

//! Card components — matches `apps/frontend/src/components/ui/card.jsx` exactly.

use leptos::prelude::*;

/// Card container.
/// React: `rounded-xl border border-border bg-card text-card-foreground shadow`
#[component]
pub fn Card(
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let classes = format!(
        "rounded-xl border border-border bg-card text-card-foreground shadow {}",
        class
    );
    view! {
        <div class=classes>
            {children()}
        </div>
    }
}

/// Card header section.
/// React: `flex flex-col space-y-1.5 p-6`
#[component]
pub fn CardHeader(
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let classes = format!("flex flex-col space-y-1.5 p-6 {}", class);
    view! { <div class=classes>{children()}</div> }
}

/// Card content section.
/// React: `p-6 pt-0`
#[component]
pub fn CardContent(
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let classes = format!("p-6 pt-0 {}", class);
    view! { <div class=classes>{children()}</div> }
}

/// Card title.
/// React: `font-semibold leading-none tracking-tight`
#[component]
pub fn CardTitle(
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let classes = format!("font-semibold leading-none tracking-tight {}", class);
    view! { <div class=classes>{children()}</div> }
}

/// Card description.
/// React: `text-sm text-muted-foreground`
#[component]
pub fn CardDescription(
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let classes = format!("text-sm text-muted-foreground {}", class);
    view! { <div class=classes>{children()}</div> }
}

/// Card footer.
/// React: `flex items-center p-6 pt-0`
#[component]
pub fn CardFooter(children: Children) -> impl IntoView {
    view! { <div class="flex items-center p-6 pt-0">{children()}</div> }
}
