// SPDX-License-Identifier: AGPL-3.0-or-later

//! Skeleton loading placeholder — matches `apps/frontend/src/components/ui/skeleton.jsx` exactly.
//!
//! Renders an animated placeholder div used while content is loading.

use leptos::prelude::*;

/// Base classes from the React Skeleton component.
/// From React: `cn("bg-accent animate-pulse rounded-md", className)`
const BASE: &str = "bg-accent animate-pulse rounded-md";

/// Skeleton component matching the React shadcn/ui Skeleton.
///
/// A simple animated placeholder that indicates content is loading.
/// Used by Data Sources and other views that need loading states.
#[component]
pub fn Skeleton(
    #[prop(optional, into)]
    class: String,
) -> impl IntoView {
    let classes = format!("{} {}", BASE, class);

    view! {
        <div data-slot="skeleton" class=classes />
    }
}
