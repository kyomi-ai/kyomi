// SPDX-License-Identifier: AGPL-3.0-or-later

//! Badge component — matches `apps/frontend/src/components/ui/badge.jsx` exactly.
//!
//! Variants replicate the React `badgeVariants` CVA config.

use leptos::prelude::*;

/// Badge variant determines color/style.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum BadgeVariant {
    #[default]
    Default,
    Secondary,
    Destructive,
    Warning,
    Outline,
}

/// Base classes shared by all badge variants.
/// From React: `badgeVariants` base string.
const BASE: &str = "inline-flex items-center rounded-md border px-2.5 py-0.5 text-xs font-semibold transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

fn variant_classes(variant: BadgeVariant) -> &'static str {
    match variant {
        BadgeVariant::Default => {
            "border-transparent bg-primary text-primary-foreground shadow hover:bg-primary/80"
        }
        BadgeVariant::Secondary => {
            "border-transparent bg-secondary text-secondary-foreground hover:bg-secondary/80"
        }
        BadgeVariant::Destructive => {
            "border-transparent bg-destructive text-destructive-foreground shadow hover:bg-destructive/80"
        }
        BadgeVariant::Warning => {
            "border-transparent bg-warning text-warning-foreground shadow hover:bg-warning/80"
        }
        BadgeVariant::Outline => "text-foreground",
    }
}

/// Badge component matching the React shadcn/ui Badge.
#[component]
pub fn Badge(
    #[prop(default = BadgeVariant::Default)]
    variant: BadgeVariant,
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let classes = format!("{} {} {}", BASE, variant_classes(variant), class);

    view! {
        <div class=classes>
            {children()}
        </div>
    }
}
