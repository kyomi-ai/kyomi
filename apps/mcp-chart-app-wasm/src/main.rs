// SPDX-License-Identifier: AGPL-3.0-or-later

//! MCP Chart App — Pure Rust/WASM frontend for rendering interactive ChartML
//! charts inside Claude.ai and other MCP hosts.
//!
//! Architecture:
//! - Rust MCP transport handles postMessage protocol (no JS SDK needed)
//! - chartml-leptos renders charts via WASM
//! - All UI is Leptos components (ChartHeaderBar, info panel, dashboard panel)

mod app;
mod dashboard_panel;
mod info_panel;
mod mcp_interop;
pub mod mcp_transport;
mod type_convert;

fn main() {
    console_error_panic_hook::set_once();
    // Remove the loading spinner before mounting the app
    if let Some(window) = web_sys::window() {
        if let Some(doc) = window.document() {
            if let Some(spinner) = doc.get_element_by_id("chart") {
                spinner.remove();
            }
        }
    }
    leptos::mount::mount_to_body(app::App);
}
