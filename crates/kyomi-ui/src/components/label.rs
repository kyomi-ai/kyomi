// SPDX-License-Identifier: AGPL-3.0-or-later

//! Label component — matches `apps/frontend/src/components/ui/label.jsx` exactly.

use leptos::prelude::*;

/// Label class string matching the React Label component exactly.
/// React: `text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70`
pub const LABEL_CLASS: &str = "text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70";

/// Label component.
#[component]
pub fn Label(
    #[prop(optional, into)] html_for: String,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let classes = if class.is_empty() {
        LABEL_CLASS.to_string()
    } else {
        format!("{} {}", LABEL_CLASS, class)
    };
    view! { <label class=classes for=html_for>{children()}</label> }
}
