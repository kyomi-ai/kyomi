// SPDX-License-Identifier: AGPL-3.0-or-later

//! Button component — matches DESIGN.md "Button Variants" specification.
//!
//! All buttons in the app MUST use this component. Never use raw `<button>`
//! with inline Tailwind classes. See DESIGN.md "Component Patterns" section.

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
    /// Ghost button with muted text — for icon actions that should not draw attention.
    /// Starts `text-muted-foreground`, hovers to `text-foreground` with `bg-accent`.
    GhostMuted,
    /// Ghost button for destructive actions — muted text, red hover.
    /// Use with `opacity-0 group-hover:opacity-100` for hover-to-reveal on cards.
    GhostDestructive,
    Link,
    /// Active/toggled-on state — amber tint.
    Active,
}

/// Button size.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum ButtonSize {
    #[default]
    Default,
    Sm,
    Lg,
    Icon,
    /// Compact icon-only: 28px square — for dense UI like panel action rows.
    IconSm,
}

/// Base classes shared by all button variants.
///
/// DESIGN.md: DM Sans 14px weight 600, rounded-md (8px), gap-1.5,
/// transition-colors 200ms, focus-visible ring-1, disabled states.
const BASE: &str = "inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md text-sm font-semibold transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 disabled:cursor-not-allowed [&_svg]:pointer-events-none [&_svg]:shrink-0";

fn variant_classes(variant: ButtonVariant) -> &'static str {
    match variant {
        // Primary: amber bg, white text
        ButtonVariant::Default => "bg-primary text-primary-foreground shadow hover:bg-primary/90",
        // Destructive: red bg, white text
        ButtonVariant::Destructive => "bg-destructive text-destructive-foreground shadow-sm hover:bg-destructive/90",
        // Outline: transparent bg, border, text color
        ButtonVariant::Outline => "border border-border bg-transparent text-foreground shadow-sm hover:bg-secondary",
        // Secondary: warm surface bg, border, text color (from design preview)
        ButtonVariant::Secondary => "bg-secondary text-foreground border border-border hover:border-[--color-border-strong]",
        // Ghost: transparent, accent text, hover accent-light
        ButtonVariant::Ghost => "bg-transparent text-primary hover:bg-accent",
        // Ghost muted: transparent, muted text, hover foreground + accent bg
        ButtonVariant::GhostMuted => "bg-transparent text-muted-foreground hover:text-foreground hover:bg-accent",
        // Ghost destructive: transparent, muted text, red hover — for delete actions on cards
        ButtonVariant::GhostDestructive => "bg-transparent text-muted-foreground hover:text-error-foreground hover:bg-error/10",
        // Link: underline style
        ButtonVariant::Link => "text-primary underline-offset-4 hover:underline",
        // Active/toggled: amber tint
        ButtonVariant::Active => "bg-primary/10 text-primary border border-primary/20",
    }
}

fn size_classes(size: ButtonSize) -> &'static str {
    match size {
        // Default: 10px 20px (py-2.5 px-5)
        ButtonSize::Default => "px-5 py-2.5",
        // Small: 8px 14px (py-2 px-3.5), 13px font — py-2 gives ~36px height for touch accessibility
        ButtonSize::Sm => "px-3.5 py-2 text-[13px]",
        // Large
        ButtonSize::Lg => "px-8 py-3",
        // Icon-only: square
        ButtonSize::Icon => "h-9 w-9 p-0",
        // Compact icon-only: 28px square for dense action rows
        ButtonSize::IconSm => "h-7 w-7 p-0",
    }
}

/// Button component matching DESIGN.md button specification.
///
/// # Usage
/// ```rust
/// // Primary button (default)
/// <Button on:click=handler>"Save"</Button>
///
/// // Secondary small button
/// <Button variant=ButtonVariant::Secondary size=ButtonSize::Sm>"Cancel"</Button>
///
/// // Icon button with aria label
/// <Button variant=ButtonVariant::Ghost size=ButtonSize::Icon aria_label="Back">..</Button>
/// ```
#[component]
pub fn Button(
    #[prop(default = ButtonVariant::Default)]
    variant: ButtonVariant,
    #[prop(default = ButtonSize::Default)]
    size: ButtonSize,
    /// Additional classes for layout only (e.g., "hidden md:flex", "flex-shrink-0").
    /// Never use this for colors, padding, radius, or fonts — those are owned by the component.
    #[prop(optional, into)]
    class: String,
    #[prop(optional, into)]
    disabled: MaybeProp<bool>,
    /// Accessibility label — required for icon-only buttons.
    #[prop(optional, into)]
    aria_label: Option<String>,
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
        <button class=classes disabled=move || disabled.get().unwrap_or(false) aria-label=aria_label.clone() title=aria_label>
            {children()}
        </button>
    }
}

/// Reactive button — variant changes based on a signal.
///
/// Use when the button toggles between states (e.g., active/inactive).
#[component]
pub fn ToggleButton(
    /// Signal that returns the current variant.
    #[prop(into)]
    variant: Signal<ButtonVariant>,
    #[prop(default = ButtonSize::Default)]
    size: ButtonSize,
    #[prop(optional, into)]
    class: String,
    #[prop(optional, into)]
    disabled: MaybeProp<bool>,
    #[prop(optional, into)]
    aria_label: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    let size_cls = size_classes(size);
    let extra = class;

    view! {
        <button
            class=move || format!(
                "{} {} {} {}",
                BASE,
                variant_classes(variant.get()),
                size_cls,
                extra,
            )
            disabled=move || disabled.get().unwrap_or(false)
            aria-label=move || aria_label.get()
            title=move || aria_label.get()
        >
            {children()}
        </button>
    }
}

/// Link styled as a button — for navigation actions (e.g., "Edit Dashboard").
#[component]
pub fn ButtonLink(
    #[prop(into)]
    href: String,
    #[prop(default = ButtonVariant::Default)]
    variant: ButtonVariant,
    #[prop(default = ButtonSize::Default)]
    size: ButtonSize,
    #[prop(optional, into)]
    class: String,
    #[prop(optional, into)]
    aria_label: Option<String>,
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
        <a href=href class=classes aria-label=aria_label.clone() title=aria_label>
            {children()}
        </a>
    }
}
