// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tooltip component — CSS-only alternative to the Radix-based
//! `apps/frontend/src/components/ui/tooltip.jsx`.
//!
//! Uses the Tailwind `group` + `group-hover:visible` pattern to show
//! tooltip content on hover without JavaScript positioning logic.
//! Visual classes are copied from the React `TooltipContent`.

use leptos::prelude::*;

/// Tooltip content classes matching the React TooltipContent visual appearance.
///
/// From React source:
///   bg-popover text-popover-foreground border-2 border-border shadow-xl
///   ring-1 ring-black/5 rounded-md px-3 py-2 text-xs
///
/// Plus positioning and visibility classes for the CSS-only approach.
const CONTENT_CLASS: &str = "invisible group-hover:visible absolute bottom-full left-1/2 -translate-x-1/2 mb-2 z-[1100] w-max max-w-xs bg-popover text-popover-foreground border-2 border-border rounded-md shadow-xl ring-1 ring-black/5 px-3 py-2 text-xs text-balance whitespace-pre-wrap pointer-events-none";

/// CSS-only tooltip that displays `content` above the trigger element on hover.
///
/// # Props
/// - `content` — the text shown in the tooltip popup.
/// - `children` — the trigger element the user hovers over.
/// - `class` — optional extra classes on the outer wrapper.
#[component]
pub fn Tooltip(
    /// Text displayed in the tooltip popup.
    #[prop(into)]
    content: String,
    /// Optional extra classes on the outer `group relative` wrapper.
    #[prop(optional, into)]
    class: String,
    /// The trigger element.
    children: Children,
) -> impl IntoView {
    let wrapper_class = format!("group relative inline-flex {class}");

    view! {
        <div class=wrapper_class>
            {children()}
            <div class=CONTENT_CLASS role="tooltip">
                {content}
            </div>
        </div>
    }
}
