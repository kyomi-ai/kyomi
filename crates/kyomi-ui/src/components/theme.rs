// SPDX-License-Identifier: AGPL-3.0-or-later

//! Theme management — applies light/dark/system theme to the document.
//!
//! Matches the React `ThemeContext` behavior:
//! - "light" → removes `dark` class from `<html>`
//! - "dark" → adds `dark` class to `<html>`
//! - "system" → follows `prefers-color-scheme` media query

use leptos::prelude::*;

/// Global theme signal, provided via Leptos context.
/// Components read this to know the current theme preference.
/// The `apply_theme` effect watches this and updates the DOM.
#[derive(Clone, Copy)]
pub struct ThemeState {
    /// The user's preference: "light", "dark", or "system".
    pub preference: RwSignal<String>,
    /// The resolved effective theme: "light" or "dark".
    pub effective: RwSignal<String>,
}

/// Provide theme context and set up the DOM effect.
///
/// Call once at app root. Reads the initial theme from the profile
/// preference and applies it immediately.
#[component]
pub fn ThemeProvider(
    #[prop(into)] initial_preference: String,
    children: Children,
) -> impl IntoView {
    let preference = RwSignal::new(initial_preference);
    let effective = RwSignal::new(String::from("dark")); // default until resolved

    let state = ThemeState {
        preference,
        effective,
    };
    provide_context(state);

    // Apply theme whenever preference changes
    Effect::new(move || {
        let pref = preference.get();
        let resolved = resolve_theme(&pref);
        effective.set(resolved.clone());
        apply_to_document(&resolved);
    });

    children()
}

/// Set the theme preference. Updates the signal which triggers the DOM effect.
pub fn set_theme(theme: &str) {
    if let Some(state) = use_context::<ThemeState>() {
        state.preference.set(theme.to_string());
    }
}

/// Get the current theme preference signal.
pub fn use_theme() -> Option<ThemeState> {
    use_context::<ThemeState>()
}

/// Resolve "system" to the actual theme based on media query.
fn resolve_theme(preference: &str) -> String {
    match preference {
        "light" => "light".to_string(),
        "dark" => "dark".to_string(),
        _ => {
            // System preference — check prefers-color-scheme
            #[cfg(target_arch = "wasm32")]
            {
                let window = web_sys::window().expect("window");
                let prefers_dark = window
                    .match_media("(prefers-color-scheme: dark)")
                    .ok()
                    .flatten()
                    .map(|mq| mq.matches())
                    .unwrap_or(true); // default to dark if can't detect
                if prefers_dark { "dark" } else { "light" }.to_string()
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                "dark".to_string() // SSR default
            }
        }
    }
}

/// Apply the resolved theme to the `<html>` element.
fn apply_to_document(theme: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let document = web_sys::window()
            .expect("window")
            .document()
            .expect("document");
        let html = document.document_element().expect("html element");
        let class_list = html.class_list();

        match theme {
            "dark" => {
                let _ = class_list.add_1("dark");
            }
            _ => {
                let _ = class_list.remove_1("dark");
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = theme;
    }
}
