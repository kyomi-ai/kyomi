// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket URL helper — single shared utility for constructing the
//! `/ws/{workspace_id}_{user_id}` URL with the correct protocol and token.
//!
//! The app has exactly one WebSocket connection per authenticated tab,
//! managed by `components::chat::websocket_client::WebSocketProvider`.
//! Features that need real-time events subscribe by message type against
//! the shared `WebSocketContext` — they do not open their own sockets.
//!
//! This module used to contain two other dead code paths
//! (`use_dashboard_updates` and `fetch_ws_token`) that predated the shared
//! context; both have been removed. The only consumer of `build_ws_url`
//! today is `websocket_client.rs`, which fetches its token via the
//! `get_websocket_config` server function.

// ---------------------------------------------------------------------------
// WASM implementation (browser)
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod inner {
    /// Build the WebSocket URL with the correct protocol, path, and token.
    ///
    /// Derives the WebSocket protocol (`ws:` / `wss:`) from the current page's
    /// `location.protocol` and constructs the standard path:
    /// `{ws_protocol}//{host}/ws/{workspace_id}_{user_id}?token={token}`
    pub fn build_ws_url(
        user_id: &str,
        workspace_id: &str,
        token: &str,
    ) -> Result<String, String> {
        let window = web_sys::window().ok_or("No window object")?;
        let location = window.location();
        let protocol = location.protocol().map_err(|_| "no protocol")?;
        let host = location.host().map_err(|_| "no host")?;

        let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };

        Ok(format!(
            "{ws_protocol}//{host}/ws/{workspace_id}_{user_id}?token={token}"
        ))
    }
}

#[cfg(target_arch = "wasm32")]
pub use inner::build_ws_url;
