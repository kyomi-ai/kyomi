// SPDX-License-Identifier: AGPL-3.0-or-later

//! WASM entry point for the Leptos frontend.
//!
//! This is compiled to WASM and loaded in the browser.
//! Server functions are called via HTTP to the Axum backend.

use kyomi_ui::app::App;
use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);

    // Remove the loading screen now that the app has mounted.
    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(loading) = document.get_element_by_id("kyomi-loading")
    {
        loading.remove();
    }
}
