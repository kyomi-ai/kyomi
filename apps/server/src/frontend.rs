// SPDX-License-Identifier: AGPL-3.0-or-later

//! Embedded frontend assets served as an Axum fallback handler.
//!
//! Serves the Vite-built SPA from the binary itself, with proper
//! cache headers:
//! - `assets/` (Vite content-hashed) → immutable, cache forever
//! - `index.html`, manifests, `sw.js` → `no-cache` (always revalidate)
//! - Everything else (e.g. `duckdb/`) → ETag-based caching (revalidate on upgrade)
//!
//! NOTE: `apps/frontend/dist/` must exist before compiling this crate.
//! Run `cd apps/frontend && npm run build` if you see embed-related compile errors.

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../frontend/dist/"]
struct FrontendAssets;

/// Serve an embedded frontend file, or index.html as SPA fallback.
pub async fn serve(headers: HeaderMap, uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Try the exact file first
    if let Some(file) = FrontendAssets::get(path) {
        return file_response(path, &file, &headers);
    }

    // SPA fallback: serve the Leptos shell for any unmatched page request.
    // React's index.html is no longer used — all routing is handled by Leptos.
    crate::leptos_frontend::serve_leptos_shell().await
}

/// Files that should never be cached (always revalidated by the browser).
fn is_no_cache(path: &str) -> bool {
    matches!(
        path,
        "index.html" | "sw.js" | "manifest.webmanifest" | "registerSW.js"
    )
}

fn file_response(
    path: &str,
    file: &rust_embed::EmbeddedFile,
    request_headers: &HeaderMap,
) -> Response {
    let mime = mime_from_path(path);

    if path.starts_with("assets/") {
        // Vite content-hashed filenames — cache forever, no ETag needed.
        return (
            [
                (header::CONTENT_TYPE, mime),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                ),
            ],
            file.data.to_vec(),
        )
            .into_response();
    }

    if is_no_cache(path) {
        // SPA entry points — always revalidate.
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

    // Everything else (duckdb/, favicons, etc.) — ETag-based caching.
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
                header::CACHE_CONTROL,
                // `no-cache` means "cache it, but revalidate every time" (not "don't cache").
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
