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

use std::sync::OnceLock;

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../crates/kyomi-ui/dist/"]
struct LeptosAssets;

/// Serve the Leptos shell for a protected route — redirects to /login if
/// no `access_token` cookie is present. Matches React's `<ProtectedRoute>`.
pub async fn serve_protected_page(headers: HeaderMap, uri: axum::http::Uri) -> Response {
    if let Some(redirect) = check_auth_cookie_or_redirect(&headers, Some(&uri)) {
        return redirect;
    }
    serve_leptos_shell().await
}

/// Check for auth cookies and redirect to `/login?redirect=<path>` if none present.
///
/// Serves the page if either `access_token` OR `refresh_token` is present.
/// When only `refresh_token` exists (access token expired), the pre-boot
/// refresh script in index.html will exchange it for a new access token
/// before WASM boots.
///
/// The redirect path is validated to contain only safe characters (alphanumeric,
/// `/`, `-`, `_`) to prevent query-param injection or open-redirect attacks.
fn check_auth_cookie_or_redirect(headers: &HeaderMap, uri: Option<&axum::http::Uri>) -> Option<Response> {
    let cookies_str = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let has_cookie = |name: &str| {
        let prefix = format!("{name}=");
        cookies_str.split(';').any(|c| {
            let c = c.trim();
            c.starts_with(&prefix) && c.len() > prefix.len()
        })
    };

    let access_name = &kyomi_core::constants::get().cookies.access_token_name;
    let refresh_name = &kyomi_core::constants::get().cookies.refresh_token_name;

    if has_cookie(access_name) || has_cookie(refresh_name) {
        return None;
    }

    // Build redirect URL, validating the path contains only safe characters
    let redirect_path = uri.map(|u| u.path().to_string()).unwrap_or_default();
    if redirect_path.is_empty()
        || redirect_path == "/"
        || redirect_path == "/login"
        || !redirect_path.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'/' || b == b'-' || b == b'_')
    {
        return Some(Redirect::to("/login").into_response());
    }
    Some(Redirect::to(&format!("/login?redirect={redirect_path}")).into_response())
}

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
///
/// Delegates to [`file_response`] so pre-compressed `.br`/`.gz` variants
/// and proper cache headers are used for content-hashed assets, matching
/// the fallback [`serve`] handler.
pub async fn serve_leptos_asset(
    axum::extract::Path(path): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    match LeptosAssets::get(&path) {
        Some(file) => file_response(&path, &file, &headers),
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
    // Protected routes require an auth cookie — redirect to /login without one.
    // Public routes (auth pages, trial, welcome, unsubscribe, onboarding) pass through.
    let is_public = matches!(
        path,
        "login" | "welcome" | "unsubscribe" | "onboarding" | "setup"
        | "billing/return"
    ) || path.starts_with("signup/")
        || path.starts_with("auth/")
        || path.starts_with("account/")
        || path.starts_with("verify");

    if !is_public
        && let Some(redirect) = check_auth_cookie_or_redirect(&headers, Some(&uri))
    {
        return redirect;
    }

    serve_leptos_shell().await
}

/// Files that should never be cached (always revalidated by the browser).
fn is_no_cache(path: &str) -> bool {
    matches!(
        path,
        "index.html" | "sw.js" | "manifest.json" | "manifest.webmanifest" | "registerSW.js"
    )
}

/// Files with content hashes in their filename that can be cached forever.
/// Trunk generates filenames like `kyomi-ui-<hash>_bg.wasm` and `kyomi-ui-<hash>.js`.
fn is_content_hashed(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    // Trunk output: name contains a hex hash before the extension
    // e.g. kyomi-ui-b2c0d10d0de1bbf3_bg.wasm, kyomi-ui-b2c0d10d0de1bbf3.js
    (name.ends_with(".wasm") || name.ends_with(".js") || name.ends_with(".css"))
        && name.contains('-')
        && name.len() > 20
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

    if is_content_hashed(path) {
        // Content-hashed filenames (WASM, JS, CSS from trunk) — cache forever.
        // The hash changes when content changes, so this is safe.

        // Serve pre-compressed versions produced by the trunk post-build hook.
        // Prefer brotli (~45% smaller than gzip for WASM), fall back to gzip,
        // then the raw file. Cloudflare and nginx pass origin Content-Encoding
        // through without re-compressing.
        let accept_encoding = request_headers
            .get(header::ACCEPT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if accept_encoding.contains("br")
            && let Some(br_file) = LeptosAssets::get(&format!("{path}.br"))
        {
            return (
                [
                    (header::CONTENT_TYPE, mime),
                    (
                        header::CACHE_CONTROL,
                        HeaderValue::from_static("public, max-age=31536000, immutable"),
                    ),
                    (header::CONTENT_ENCODING, HeaderValue::from_static("br")),
                    (header::VARY, HeaderValue::from_static("accept-encoding")),
                ],
                br_file.data.to_vec(),
            )
                .into_response();
        }

        if accept_encoding.contains("gzip")
            && let Some(gz_file) = LeptosAssets::get(&format!("{path}.gz"))
        {
            return (
                [
                    (header::CONTENT_TYPE, mime),
                    (
                        header::CACHE_CONTROL,
                        HeaderValue::from_static("public, max-age=31536000, immutable"),
                    ),
                    (header::CONTENT_ENCODING, HeaderValue::from_static("gzip")),
                    (header::VARY, HeaderValue::from_static("accept-encoding")),
                ],
                gz_file.data.to_vec(),
            )
                .into_response();
        }

        return (
            [
                (header::CONTENT_TYPE, mime),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                ),
                (header::VARY, HeaderValue::from_static("accept-encoding")),
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
    if let Some(if_none_match) = request_headers.get(header::IF_NONE_MATCH)
        && let Ok(client_etag) = if_none_match.to_str()
        && client_etag == etag {
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

// ─── SSR for login page ─────────────────────────────────────────────────

struct TemplateParts {
    prefix: String,
    suffix: String,
}

static TEMPLATE_PARTS: OnceLock<Option<TemplateParts>> = OnceLock::new();

fn get_template_parts() -> Option<&'static TemplateParts> {
    TEMPLATE_PARTS
        .get_or_init(|| {
            let file = LeptosAssets::get("index.html")?;
            let html = String::from_utf8_lossy(&file.data);

            // Search for <body after </head> to avoid false matches inside
            // CSS comments or style blocks that contain the string "<body".
            let head_end = html.find("</head>")?;
            let body_start = head_end + html[head_end..].find("<body")?;
            let body_tag_end = html[body_start..].find('>')? + body_start + 1;

            let body_tag = &html[body_start..body_tag_end];
            let modified_body_tag = body_tag.replacen("<body", "<body data-ssr", 1);

            let prefix = format!(
                "{}{}",
                &html[..body_start],
                modified_body_tag,
            );
            let suffix = "\n</body>\n</html>".to_string();

            Some(TemplateParts { prefix, suffix })
        })
        .as_ref()
}

/// Build an axum handler that SSR-renders the login page.
///
/// Uses `render_app_to_stream_with_context` to render `<App/>` server-side,
/// then wraps the output in the Trunk-built `index.html` template (which has
/// the CSS, fonts, and WASM script tags). The `<body>` gets a `data-ssr`
/// attribute so the WASM entry point hydrates instead of mounting fresh.
///
/// Falls back to the static CSR shell if the template can't be parsed.
pub fn login_ssr_handler(
    server_ctx: kyomi_ui::server_fns::ServerContext,
) -> impl FnMut(
    axum::http::Request<axum::body::Body>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Response> + Send>,
> + Clone
       + Send
       + 'static {
    use kyomi_ui::app::App;
    use leptos::prelude::*;

    // Leptos reactive Effects call spawn_local(), which in tokio requires a
    // LocalSet (!Send). Since axum handlers must produce Send futures, we
    // register a custom executor: spawn → tokio::spawn, spawn_local → no-op.
    // Effects are client-side behaviour and don't need to run during SSR.
    struct SsrExecutor;
    impl any_spawner::CustomExecutor for SsrExecutor {
        fn spawn(&self, fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>) {
            tokio::spawn(fut);
        }
        fn spawn_local(&self, _fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>) {}
        fn poll_local(&self) {}
    }
    _ = any_spawner::Executor::init_custom_executor(SsrExecutor);

    let renderer = leptos_axum::render_app_to_stream_with_context(
        move || {
            provide_context(server_ctx.clone());
        },
        App,
    );

    move |req| {
        let renderer = renderer.clone();
        Box::pin(async move {
            let Some(tpl) = get_template_parts() else {
                return serve_leptos_shell().await;
            };

            let ssr_response = renderer(req).await;

            let Ok(body_bytes) =
                axum::body::to_bytes(ssr_response.into_body(), 2 * 1024 * 1024).await
            else {
                return serve_leptos_shell().await;
            };

            let ssr_html = String::from_utf8_lossy(&body_bytes);
            let full = format!("{}{ssr_html}{}", tpl.prefix, tpl.suffix);

            ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], full).into_response()
        })
    }
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
