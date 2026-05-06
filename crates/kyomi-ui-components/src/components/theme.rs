// SPDX-License-Identifier: AGPL-3.0-or-later

//! Theme management — applies light/dark/system theme to the document.
//!
//! Theme preference is persisted in localStorage (`kyomi-theme`) for instant
//! application before WASM loads (via inline script in `index.html`), and
//! synced to the server for cross-device consistency.
//!
//! - "light" → removes `dark` class from `<html>`
//! - "dark" → adds `dark` class to `<html>`
//! - "system" → follows `prefers-color-scheme` media query, listens for changes

use leptos::prelude::*;

#[cfg(target_arch = "wasm32")]
const STORAGE_KEY: &str = "kyomi-theme";

/// Global theme signal, provided via Leptos context.
#[derive(Clone, Copy)]
pub struct ThemeState {
    /// The user's preference: "light", "dark", or "system".
    pub preference: RwSignal<String>,
    /// The resolved effective theme: "light" or "dark".
    pub effective: RwSignal<String>,
}

/// Provide theme context and set up the DOM effect.
///
/// Reads initial preference from localStorage (fast path), falling back to
/// `initial_preference` (from server or "system" default). Writes localStorage
/// on every preference change.
#[component]
pub fn ThemeProvider(
    #[prop(into)] initial_preference: String,
    children: Children,
) -> impl IntoView {
    // Read localStorage first — this matches what the inline script already applied.
    let initial = read_local_storage()
        .unwrap_or(initial_preference);

    let preference = RwSignal::new(initial);
    let effective = RwSignal::new(String::from("dark")); // updated immediately by Effect

    let state = ThemeState {
        preference,
        effective,
    };
    provide_context(state);

    // Apply theme whenever preference changes: resolve → apply DOM → persist
    Effect::new(move || {
        let pref = preference.get();
        let resolved = resolve_theme(&pref);
        effective.set(resolved.clone());
        apply_to_document(&resolved);
        write_local_storage(&pref);
    });

    // Listen for OS theme changes when preference is "system".
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;

        let window = web_sys::window().expect("window");
        if let Ok(Some(mq)) = window.match_media("(prefers-color-scheme: dark)") {
            let cb = Closure::<dyn Fn()>::new(move || {
                if preference.get_untracked() == "system" {
                    let resolved = resolve_theme("system");
                    effective.set(resolved.clone());
                    apply_to_document(&resolved);
                }
            });
            let _ = mq.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());

            // Keep closure alive for the component's lifetime, clean up on unmount.
            // SendWrapper is needed because Closure<dyn Fn()> is !Sync but
            // on_cleanup requires Send + Sync. On WASM this is a no-op wrapper.
            let cb_fn: js_sys::Function = cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let cleanup_cb = SendWrapper::new(cb);
            let cleanup_mq = SendWrapper::new(mq);
            let cleanup_fn = SendWrapper::new(cb_fn);
            on_cleanup(move || {
                let _ = cleanup_mq.remove_event_listener_with_callback("change", &cleanup_fn);
                drop(cleanup_cb);
            });
        }
    }

    children()
}

/// Set the theme preference. Updates the signal which triggers the DOM effect.
pub fn set_theme(theme: &str) {
    if let Some(state) = use_context::<ThemeState>() {
        state.preference.set(theme.to_string());
    }
}

/// Persist theme preference to localStorage.
///
/// Called explicitly from layout (on auth sync) and settings (on manual change).
/// Also called by the ThemeProvider effect on every preference change.
pub fn save_theme_to_local_storage(theme: &str) {
    write_local_storage(theme);
}

/// Get the current theme state.
pub fn use_theme() -> Option<ThemeState> {
    use_context::<ThemeState>()
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Resolve "system" to the actual theme based on media query.
fn resolve_theme(preference: &str) -> String {
    match preference {
        "light" => "light".to_string(),
        "dark" => "dark".to_string(),
        _ => {
            #[cfg(target_arch = "wasm32")]
            {
                let prefers_dark = web_sys::window()
                    .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
                    .map(|mq| mq.matches())
                    .unwrap_or(true);
                if prefers_dark { "dark" } else { "light" }.to_string()
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                "dark".to_string()
            }
        }
    }
}

/// Apply the resolved theme to the `<html>` element.
fn apply_to_document(theme: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(html) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        {
            let class_list = html.class_list();
            match theme {
                "dark" => { let _ = class_list.add_1("dark"); }
                _ => { let _ = class_list.remove_1("dark"); }
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    { let _ = theme; }
}

/// Read theme preference from localStorage.
fn read_local_storage() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten())
            .filter(|v| v == "light" || v == "dark" || v == "system")
    }
    #[cfg(not(target_arch = "wasm32"))]
    { None }
}

/// Write theme preference to localStorage.
fn write_local_storage(theme: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(storage) = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
        {
            let _ = storage.set_item(STORAGE_KEY, theme);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    { let _ = theme; }
}
