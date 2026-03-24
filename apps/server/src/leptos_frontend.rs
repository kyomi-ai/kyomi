// SPDX-License-Identifier: AGPL-3.0-or-later

//! Leptos frontend serving — the sole frontend for the Kyomi application.
//!
//! The Leptos frontend is built by `trunk` from `crates/kyomi-ui/`
//! and outputs to `crates/kyomi-ui/dist/`. This module embeds those
//! files and serves them with proper cache headers:
//!
//! - Content-hashed files (WASM, JS, CSS with hashes) → immutable, cache forever
//! - `index.html`, `manifest.json`, `sw.js` → `no-cache` (always revalidate)
//! - Static assets (logos, icons, etc.) → ETag-based caching
//!
//! Routes:
//! - Page routes → serves the Leptos HTML shell (index.html)
//! - `/leptos/*` → serves WASM, JS, and CSS assets
//! - Fallback → serves index.html (SPA routing) or static assets

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../crates/kyomi-ui/dist/"]
struct LeptosAssets;

/// Serve the Leptos HTML shell for known page routes.
///
/// Returns the `index.html` that trunk generated, which loads
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

/// Fallback handler for all non-API, non-Leptos-asset routes.
///
/// Tries to serve the exact file from the embedded assets (e.g. logos,
/// manifest.json, favicon). If no file matches, returns the Leptos
/// HTML shell so client-side routing can handle the path.
pub async fn serve(headers: HeaderMap, uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Try the exact file first (static assets: logos, icons, manifest, etc.)
    if let Some(file) = LeptosAssets::get(path) {
        return file_response(path, &file, &headers);
    }

    // Trunk's `copy-dir` directive copies `public/` into `dist/public/`,
    // so assets referenced as `/kyomi_full_logo.svg` are embedded as
    // `public/kyomi_full_logo.svg`. Try that prefix before falling back.
    let public_path = format!("public/{path}");
    if let Some(file) = LeptosAssets::get(&public_path) {
        return file_response(&public_path, &file, &headers);
    }

    // SPA fallback: serve the Leptos shell for any unmatched page request.
    serve_leptos_shell().await
}

/// Files that should never be cached (always revalidated by the browser).
fn is_no_cache(path: &str) -> bool {
    matches!(
        path,
        "index.html" | "sw.js" | "manifest.json" | "manifest.webmanifest" | "registerSW.js"
    )
}

fn file_response(
    path: &str,
    file: &rust_embed::EmbeddedFile,
    request_headers: &HeaderMap,
) -> Response {
    let mime = mime_from_path(path);

    if is_no_cache(path) {
        // Entry points and manifests — always revalidate.
        return (
            [
                (header::CONTENT_TYPE, mime),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                ),
            ],
            file.data.to_vec(),
        )
            .into_response();
    }

    // Everything else (logos, favicons, etc.) — ETag-based caching.
    // Browser caches the file but revalidates with If-None-Match on each load.
    // After a binary upgrade the hash changes → full 200 with new content.
    let hash = file.metadata.sha256_hash();
    // First 16 bytes → 32 hex chars, wrapped in quotes for a strong ETag.
    let hex: String = hash[..16].iter().map(|b| format!("{b:02x}")).collect();
    let etag = format!("\"{hex}\"");

    // Check If-None-Match — return 304 if the client already has this version.
    if let Some(if_none_match) = request_headers.get(header::IF_NONE_MATCH) {
        if let Ok(client_etag) = if_none_match.to_str() {
            if client_etag == etag {
                return (
                    StatusCode::NOT_MODIFIED,
                    [
                        (header::ETAG, HeaderValue::from_str(&etag).unwrap()),
                        (
                            header::CACHE_CONTROL,
                            HeaderValue::from_static("public, no-cache"),
                        ),
                    ],
                )
                    .into_response();
            }
        }
    }

    let etag_header = HeaderValue::from_str(&etag).unwrap();
    (
        [
            (header::CONTENT_TYPE, mime),
            (
                // `no-cache` means "cache it, but revalidate every time" (not "don't cache").
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, no-cache"),
            ),
            (header::ETAG, etag_header),
        ],
        file.data.to_vec(),
    )
        .into_response()
}

fn mime_from_path(path: &str) -> HeaderValue {
    let mime = match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("webmanifest") => "application/manifest+json",
        Some("wasm") => "application/wasm",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    };
    HeaderValue::from_static(mime)
}
