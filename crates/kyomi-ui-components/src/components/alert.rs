// SPDX-License-Identifier: AGPL-3.0-or-later

//! Alert components — matches `apps/frontend/src/components/ui/alert.jsx` exactly.
//!
//! Variants replicate the React `alertVariants` CVA config.
//! Sub-components: Alert, AlertTitle, AlertDescription.

use leptos::prelude::*;

/// Alert variant determines color/style.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum AlertVariant {
    #[default]
    Default,
    Warning,
    Error,
    Success,
    Info,
}

/// Base classes shared by all alert variants.
/// From React: `alertVariants` base string.
const BASE: &str = "relative w-full rounded-lg border px-4 py-3 text-sm [&>svg+div]:translate-y-[-3px] [&>svg]:absolute [&>svg]:left-4 [&>svg]:top-4 [&>svg]:text-foreground [&>svg~*]:pl-7";

fn variant_classes(variant: AlertVariant) -> &'static str {
    match variant {
        AlertVariant::Default => "bg-background text-foreground",
        AlertVariant::Warning => {
            "bg-warning text-warning-foreground border-warning-border [&>svg]:text-warning-foreground"
        }
        AlertVariant::Error => {
            "bg-error text-error-foreground border-error-border [&>svg]:text-error-foreground"
        }
        AlertVariant::Success => {
            "bg-success text-success-foreground border-success-border [&>svg]:text-success-foreground"
        }
        AlertVariant::Info => {
            "bg-info text-info-foreground border-info-border [&>svg]:text-info-foreground"
        }
    }
}

/// Alert container with `role="alert"`.
/// React: `alertVariants({ variant })` applied to a `<div role="alert">`.
#[component]
pub fn Alert(
    #[prop(default = AlertVariant::Default)]
    variant: AlertVariant,
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let classes = format!("{} {} {}", BASE, variant_classes(variant), class);

    view! {
        <div role="alert" class=classes>
            {children()}
        </div>
    }
}

/// Alert title.
/// React: `mb-1 font-medium leading-none tracking-tight`
#[component]
pub fn AlertTitle(
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let classes = format!("mb-1 font-medium leading-none tracking-tight {class}");
    view! {
        <h5 class=classes>
            {children()}
        </h5>
    }
}

/// Alert description.
/// React: `text-sm [&_p]:leading-relaxed`
#[component]
pub fn AlertDescription(
    #[prop(optional, into)]
    class: String,
    children: Children,
) -> impl IntoView {
    let classes = format!("text-sm [&_p]:leading-relaxed {class}");
    view! {
        <div class=classes>
            {children()}
        </div>
    }
}
