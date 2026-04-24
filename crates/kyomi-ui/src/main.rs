// SPDX-License-Identifier: AGPL-3.0-or-later

//! WASM entry point for the Leptos frontend.
//!
//! This is compiled to WASM and loaded in the browser.
//! Server functions are called via HTTP to the Axum backend.

use kyomi_ui::app::App;
use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();

    #[cfg(feature = "hydrate")]
    {
        let document = web_sys::window()
            .and_then(|w| w.document())
            .expect("document");
        let body = document.body().expect("body");

        if body.get_attribute("data-ssr").is_some() {
            leptos::mount::hydrate_body(App);
            let _ = body.remove_attribute("data-ssr");
        } else {
            mount_to_body(App);
            if let Some(loading) = document.get_element_by_id("kyomi-loading") {
                loading.remove();
            }
        }
    }

    #[cfg(not(feature = "hydrate"))]
    {
        mount_to_body(App);
        if let Some(window) = web_sys::window()
            && let Some(document) = window.document()
            && let Some(loading) = document.get_element_by_id("kyomi-loading")
        {
            loading.remove();
        }
    }
}
