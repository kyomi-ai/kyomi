// SPDX-License-Identifier: AGPL-3.0-or-later

//! EmptyState component — reusable empty data display.
//!
//! Matches `apps/frontend/src/components/ui/empty-state.jsx` (CVA-based).
//!
//! Usage:
//! ```ignore
//! view! {
//!     <EmptyState
//!         variant=EmptyStateVariant::Default
//!         title="No results found"
//!         description="Try adjusting your search criteria"
//!     />
//!
//!     <EmptyState
//!         variant=EmptyStateVariant::Info
//!         icon=view! { <DatabaseIcon class="w-12 h-12" /> }
//!         title="Get started"
//!         description="Connect a data source to begin"
//!         action=|| view! {
//!             <Button on:click=|_| {}>"Connect Datasource"</Button>
//!         }
//!     />
//! }
//! ```

use leptos::prelude::*;

/// Variant for the EmptyState component, controlling background and text colors.
///
/// React reference: `emptyStateVariants` CVA config in `empty-state.jsx`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EmptyStateVariant {
    #[default]
    Default,
    Warning,
    Error,
    Success,
    Info,
}

impl EmptyStateVariant {
    /// Container classes: background + border color.
    /// React: `emptyStateVariants` in CVA config.
    fn container_class(self) -> &'static str {
        match self {
            Self::Default => "bg-background border-border",
            Self::Warning => "bg-warning border-warning-border",
            Self::Error => "bg-error border-error-border",
            Self::Success => "bg-success border-success-border",
            Self::Info => "bg-info border-info-border",
        }
    }

    /// Text color for description and icon.
    /// React: `textColorMap` in `empty-state.jsx`.
    fn text_class(self) -> &'static str {
        match self {
            Self::Default => "text-muted-foreground",
            Self::Warning => "text-warning-foreground",
            Self::Error => "text-error-foreground",
            Self::Success => "text-success-foreground",
            Self::Info => "text-info-foreground",
        }
    }

    /// Title color — default variant uses foreground, others use variant color.
    /// React: `variant === "default" ? "text-foreground" : textColorMap[variant]`
    fn title_class(self) -> &'static str {
        match self {
            Self::Default => "text-foreground",
            other => other.text_class(),
        }
    }
}

/// Reusable empty state display for when no data is available.
///
/// React reference: `apps/frontend/src/components/ui/empty-state.jsx`
#[component]
pub fn EmptyState(
    /// Visual variant controlling colors. Default: `Default`.
    #[prop(default = EmptyStateVariant::Default)]
    variant: EmptyStateVariant,
    /// Optional icon slot displayed above the title.
    #[prop(optional)]
    icon: Option<ChildrenFn>,
    /// Title text.
    #[prop(into)]
    title: String,
    /// Description text below the title.
    #[prop(into)]
    description: String,
    /// Optional action slot (typically a Button).
    #[prop(optional)]
    action: Option<ChildrenFn>,
    /// Additional CSS classes on the container.
    #[prop(into, optional)]
    class: String,
) -> impl IntoView {
    let container = format!(
        "rounded-lg border p-8 text-center {} {}",
        variant.container_class(),
        class,
    );

    let icon_class = format!(
        "mx-auto mb-4 w-12 h-12 flex items-center justify-center {}",
        variant.text_class(),
    );

    let title_class = format!("text-base font-semibold mb-2 {}", variant.title_class());

    let desc_class = format!("text-sm mb-4 {}", variant.text_class());

    view! {
        <div class=container>
            {icon.map(|i| view! {
                <div class=icon_class.clone()>{i()}</div>
            })}
            <h3 class=title_class>{title}</h3>
            <p class=desc_class>{description}</p>
            {action.map(|a| view! {
                <div class="mt-4">{a()}</div>
            })}
        </div>
    }
}
