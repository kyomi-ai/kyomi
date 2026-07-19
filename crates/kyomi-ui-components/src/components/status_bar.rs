// SPDX-License-Identifier: AGPL-3.0-or-later

//! Global status bar — a full-width banner at the bottom of the layout shell.
//!
//! Used for transient alerts such as pending workspace invitations.

use leptos::prelude::*;

/// Visual variant for the status bar.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum StatusBarVariant {
    #[default]
    Info,
    Warning,
    Error,
    Success,
}

/// Full-width status bar anchored to the bottom of the layout.
///
/// DESIGN.md Status Bars: `px-6 py-3.5`, `gap-4`.
/// The bar is `flex-shrink-0` so it always stays visible at the bottom.
#[component]
pub fn StatusBar(
    #[prop(default = StatusBarVariant::Info)] variant: StatusBarVariant,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let variant_classes = match variant {
        StatusBarVariant::Info => "bg-info/10 border-info/30 text-info-foreground",
        StatusBarVariant::Warning => "bg-warning/10 border-warning/30 text-warning-foreground",
        StatusBarVariant::Error => "bg-error/10 border-error/30 text-error-foreground",
        StatusBarVariant::Success => "bg-success/10 border-success/30 text-success-foreground",
    };

    view! {
        <div class=format!("w-full flex-shrink-0 border-t {variant_classes} {class}")>
            <div class="max-w-7xl mx-auto flex items-center justify-between gap-4 w-full px-6 py-3.5">
                {children()}
            </div>
        </div>
    }
}
