// SPDX-License-Identifier: AGPL-3.0-or-later

//! Icon components — inline SVG icons matching Lucide (used in React frontend).
//!
//! Each icon is a Leptos component rendering an SVG element.
//! Default size is 16x16 matching the React `<Icon size={16} />` pattern.

use leptos::prelude::*;

/// Common icon props.
#[derive(Clone, Debug)]
pub struct IconProps {
    pub size: u32,
    pub class: String,
}

impl Default for IconProps {
    fn default() -> Self {
        Self {
            size: 16,
            class: String::new(),
        }
    }
}

/// Helper to render an SVG icon with standard attributes.
fn icon_svg(
    size: u32,
    class: &str,
    paths: &str,
) -> impl IntoView {
    let size_str = size.to_string();
    // We use innerHTML for the path data since Leptos doesn't have great SVG path support
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width=size_str.clone()
            height=size_str
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            class=class.to_string()
            inner_html=paths.to_string()
        />
    }
}

/// Check icon (checkmark).
#[component]
pub fn CheckIcon(#[prop(default = 16)] size: u32, #[prop(default = "")] class: &'static str) -> impl IntoView {
    icon_svg(size, class, r#"<polyline points="20 6 9 17 4 12"/>"#)
}

/// X (close) icon.
#[component]
pub fn XIcon(#[prop(default = 16)] size: u32, #[prop(default = "")] class: &'static str) -> impl IntoView {
    icon_svg(size, class, r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#)
}

/// Copy icon.
#[component]
pub fn CopyIcon(#[prop(default = 16)] size: u32, #[prop(default = "")] class: &'static str) -> impl IntoView {
    icon_svg(size, class, r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#)
}

/// External link icon.
#[component]
pub fn ExternalLinkIcon(#[prop(default = 16)] size: u32, #[prop(default = "")] class: &'static str) -> impl IntoView {
    icon_svg(size, class, r#"<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>"#)
}

/// Trash icon.
#[component]
pub fn TrashIcon(#[prop(default = 16)] size: u32, #[prop(default = "")] class: &'static str) -> impl IntoView {
    icon_svg(size, class, r#"<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>"#)
}

/// User icon.
#[component]
pub fn UserIcon(#[prop(default = 16)] size: u32, #[prop(default = "")] class: &'static str) -> impl IntoView {
    icon_svg(size, class, r#"<path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>"#)
}
