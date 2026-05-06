// SPDX-License-Identifier: AGPL-3.0-or-later

//! StatusBadge component — matches `apps/frontend/src/components/ui/status-badge.jsx` exactly.
//!
//! Variants replicate the React `statusBadgeVariants` CVA config.
//! Used by: TwoFactorAuth, SessionManagement, Billing.

use leptos::prelude::*;

/// StatusBadge variant determines color/style.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum StatusBadgeVariant {
    #[default]
    Default,
    Warning,
    Error,
    Success,
    Info,
}

/// Base classes shared by all status badge variants.
/// From React: `statusBadgeVariants` base string.
const BASE: &str = "inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-semibold transition-colors";

fn variant_classes(variant: StatusBadgeVariant) -> &'static str {
    match variant {
        StatusBadgeVariant::Default => "bg-muted text-muted-foreground",
        StatusBadgeVariant::Warning => {
            "bg-warning text-warning-foreground border border-warning-border"
        }
        StatusBadgeVariant::Error => "bg-error text-error-foreground border border-error-border",
        StatusBadgeVariant::Success => {
            "bg-success text-success-foreground border border-success-border"
        }
        StatusBadgeVariant::Info => "bg-info text-info-foreground border border-info-border",
    }
}

/// StatusBadge component matching the React shadcn/ui StatusBadge.
///
/// Small, inline status indicator (pill/tag) for token status,
/// workflow state, billing plan indicators, etc.
#[component]
pub fn StatusBadge(
    #[prop(default = StatusBadgeVariant::Default)]
    variant: StatusBadgeVariant,
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let classes = format!("{} {} {}", BASE, variant_classes(variant), class);

    view! {
        <span class=classes>
            {children()}
        </span>
    }
}
