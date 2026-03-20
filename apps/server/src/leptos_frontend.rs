// SPDX-License-Identifier: AGPL-3.0-or-later

//! Leptos frontend serving — serves the WASM-based Leptos UI.
//!
//! The Leptos frontend is built by `trunk` from `crates/kyomi-ui/`
//! and outputs to `crates/kyomi-ui/dist/`. This module embeds those
//! files and serves them alongside the React SPA.
//!
//! Routes:
//! - `/settings/profile` → serves the Leptos HTML shell (index.html)
//! - `/leptos/*` → serves WASM, JS, and CSS assets

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../crates/kyomi-ui/dist/"]
struct LeptosAssets;

/// Serve the Leptos HTML shell for `/settings/profile`.
///
/// This returns the `index.html` that trunk generated, which loads
/// the WASM bundle and bootstraps the Leptos app.
pub async fn serve_leptos_shell() -> Response {
    match LeptosAssets::get("index.html") {
        Some(file) => {
            let html = String::from_utf8_lossy(&file.data);
            Html(html.into_owned()).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            "Leptos frontend not built. Run: cd crates/kyomi-ui && trunk build --public-url /leptos/",
        )
            .into_response(),
    }
}

/// Serve Leptos static assets (WASM, JS, CSS) from `/leptos/`.
pub async fn serve_leptos_asset(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
    match LeptosAssets::get(&path) {
        Some(file) => {
            let mime = mime_from_path(&path);
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (
                        header::CACHE_CONTROL,
                        // Content-hashed filenames — cache forever
                        HeaderValue::from_static("public, max-age=31536000, immutable"),
                    ),
                ],
                file.data.to_vec(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn mime_from_path(path: &str) -> HeaderValue {
    let mime = match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    HeaderValue::from_static(mime)
}
