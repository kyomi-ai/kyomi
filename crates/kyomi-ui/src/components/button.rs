// SPDX-License-Identifier: AGPL-3.0-or-later

//! Button component — matches `apps/frontend/src/components/ui/button.jsx` exactly.
//!
//! Variants and sizes replicate the React `buttonVariants` CVA config.
//! All buttons in the app MUST use this component (per DESIGN_SYSTEM.md).

use leptos::prelude::*;

/// Button variant determines color/style.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum ButtonVariant {
    #[default]
    Default,
    Destructive,
    Outline,
    Secondary,
    Ghost,
    Link,
}

/// Button size.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum ButtonSize {
    #[default]
    Default,
    Sm,
    Lg,
    Icon,
}

/// Base classes shared by all button variants.
/// From React: `buttonVariants` base string.
const BASE: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";

fn variant_classes(variant: ButtonVariant) -> &'static str {
    match variant {
        ButtonVariant::Default => "bg-primary text-primary-foreground shadow hover:bg-primary/90",
        ButtonVariant::Destructive => "bg-destructive text-destructive-foreground shadow-sm hover:bg-destructive/90",
        ButtonVariant::Outline => "border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground",
        ButtonVariant::Secondary => "bg-secondary text-secondary-foreground shadow-sm hover:bg-secondary/80",
        ButtonVariant::Ghost => "text-foreground hover:bg-accent hover:text-accent-foreground",
        ButtonVariant::Link => "text-primary underline-offset-4 hover:underline",
    }
}

fn size_classes(size: ButtonSize) -> &'static str {
    match size {
        ButtonSize::Default => "h-9 px-4 py-2",
        ButtonSize::Sm => "h-8 rounded-md px-3 text-xs",
        ButtonSize::Lg => "h-10 rounded-md px-8",
        ButtonSize::Icon => "h-9 w-9",
    }
}

/// Button component matching the React shadcn/ui Button.
#[component]
pub fn Button(
    #[prop(default = ButtonVariant::Default)]
    variant: ButtonVariant,
    #[prop(default = ButtonSize::Default)]
    size: ButtonSize,
    #[prop(optional, into)]
    class: String,
    #[prop(optional)]
    disabled: bool,
    children: Children,
) -> impl IntoView {
    let classes = format!(
        "{} {} {} {}",
        BASE,
        variant_classes(variant),
        size_classes(size),
        class,
    );

    view! {
        <button class=classes disabled=disabled>
            {children()}
        </button>
    }
}
