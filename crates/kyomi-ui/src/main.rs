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
}
