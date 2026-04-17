// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kyomi Desktop — Tauri v2 native desktop app.
//!
//! Three startup paths:
//! - **First launch**: shows a mode selector (Just Me / Connect to Server)
//! - **Personal mode**: spawns the full embedded backend on localhost
//! - **Remote mode**: thin webview pointed at a remote Kyomi server URL

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod mode;

use mode::{AppMode, load_mode, reset_mode, save_mode};
use tauri::Manager;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // --switch-mode flag resets mode and falls through to show the selector
    if std::env::args().any(|a| a == "--switch-mode") {
        reset_mode();
        tracing::info!("Mode reset. Showing selector.");
    }

    match load_mode() {
        AppMode::FirstLaunch => start_mode_selector(),
        AppMode::Personal => start_personal_mode(),
        AppMode::Remote { url } => start_remote_mode(&url),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Mode Selector — first launch
// ═══════════════════════════════════════════════════════════════════════════

/// Show a mode selector page on a tiny local HTTP server, then restart into
/// the chosen mode.
fn start_mode_selector() {
    tracing::info!("First launch — showing mode selector");

    // Pick a random port for the selector's HTTP server
    let port = portpicker::pick_unused_port().expect("no free TCP port");

    // Spawn a tiny HTTP server that serves the selector HTML and receives the choice
    let (tx, rx) = std::sync::mpsc::channel::<AppMode>();

    let tx_clone = tx.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            use axum::{routing::{get, post}, Router, Json, response::Html};

            let html = mode::selector_html(port);
            let tx = tx_clone;

            let app = Router::new()
                .route("/", get(move || async move { Html(html.clone()) }))
                .route("/select", post(move |Json(body): Json<serde_json::Value>| async move {
                    let mode_str = body.get("mode").and_then(|v| v.as_str()).unwrap_or("");
                    let chosen = match mode_str {
                        "personal" => AppMode::Personal,
                        "remote" => {
                            let url = body.get("url").and_then(|v| v.as_str())
                                .unwrap_or("https://app.kyomi.ai").to_string();
                            AppMode::Remote { url }
                        }
                        _ => return "error",
                    };
                    save_mode(&chosen);
                    let _ = tx.send(chosen);
                    "ok"
                }));

            let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
                .await.unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });

    // Build Tauri and navigate to the selector page
    tauri::Builder::default()
        .setup(move |app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("Kyomi — Get Started");
                set_window_icon(&window);

                let url = format!("http://localhost:{port}/");
                let parsed = url::Url::parse(&url).expect("valid URL");
                let _ = tauri::WebviewWindow::navigate(&window, parsed);
            }

            // Wait for mode selection, then re-exec the binary without --switch-mode
            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                if let Ok(_mode) = rx.recv() {
                    // Re-exec ourselves without --switch-mode to start in the chosen mode.
                    let exe = std::env::current_exe().expect("failed to get current exe path");
                    let _ = std::process::Command::new(exe).spawn();
                    app_handle.exit(0);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running mode selector");
}

// ═══════════════════════════════════════════════════════════════════════════
// Remote Mode — thin webview client
// ═══════════════════════════════════════════════════════════════════════════

/// Open a webview pointed at a remote Kyomi server. No embedded backend.
fn start_remote_mode(server_url: &str) {
    tracing::info!(url = server_url, "Starting Kyomi Desktop (remote mode)");

    let url = server_url.to_string();

    tauri::Builder::default()
        .setup(move |app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("Kyomi");
                set_window_icon(&window);

                let parsed = url::Url::parse(&url).expect("valid server URL");
                let _ = tauri::WebviewWindow::navigate(&window, parsed);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Kyomi Desktop (remote)");
}

// ═══════════════════════════════════════════════════════════════════════════
// Personal Mode — full embedded backend
// ═══════════════════════════════════════════════════════════════════════════

fn start_personal_mode() {
    use std::sync::Arc;
    // Pick port: prefer 3000, fall back to random
    let port = if std::net::TcpListener::bind("127.0.0.1:3000").is_ok() {
        3000u16
    } else {
        portpicker::pick_unused_port().expect("no free TCP port available")
    };
    let server_url = format!("http://localhost:{port}");

    // Set env vars BEFORE Config::from_env() reads them.
    unsafe {
        std::env::set_var("KYOMI_MODE", "personal");
        std::env::set_var("PORT", port.to_string());
        std::env::set_var("FRONTEND_URL", &server_url);
        std::env::set_var("BASE_URL", &server_url);
        std::env::set_var("ENABLE_SCHEDULERS", "true");
    }

    // Use OS-standard app data directory if DATA_DIR not already set
    if std::env::var("DATA_DIR").is_err()
        && let Some(data_dir) = dirs::data_dir()
    {
        let kyomi_data = data_dir.join("ai.kyomi.desktop");
        unsafe {
            std::env::set_var("DATA_DIR", kyomi_data.to_string_lossy().as_ref());
        }
    }

    kyomi_core::constants::load_with_fallback().expect("failed to load shared constants");

    let config = kyomi_core::Config::from_env();
    let config_arc = Arc::new(config);

    let encryption_key = kyomi_auth::encryption::derive_key(&config_arc.encryption_key)
        .expect("ENCRYPTION_KEY must be a valid 32-byte base64url-encoded key");
    let encryption_key_arc = Arc::new(encryption_key);

    tracing::info!(port, "Starting Kyomi Desktop (personal mode)");

    let config_for_server = config_arc.clone();
    let encryption_key_for_server = encryption_key_arc.clone();

    // Clear WebKit cache on startup so the webview always loads fresh embedded assets.
    if let Some(data_dir) = dirs::data_dir() {
        let cache_dir = data_dir.join("ai.kyomi.desktop");
        for dir_name in &["WebKitCache", "CacheStorage", "serviceworkers"] {
            let dir = cache_dir.join(dir_name);
            if dir.exists() {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }

    tauri::Builder::default()
        .setup(move |app| {
            if let Some(window) = app.get_webview_window("main") {
                let title = if port == 3000 {
                    "Kyomi".to_string()
                } else {
                    format!("Kyomi (port {port})")
                };
                let _ = window.set_title(&title);
                set_window_icon(&window);

                #[cfg(debug_assertions)]
                window.open_devtools();
            }

            let app_handle = app.handle().clone();
            let config_arc = config_for_server;
            let encryption_key_arc = encryption_key_for_server;

            tauri::async_runtime::spawn(async move {
                if let Err(e) = start_server(config_arc, encryption_key_arc, port).await {
                    tracing::error!(error = %e, "Server failed");
                }
            });

            let server_url = format!("http://localhost:{port}");
            tauri::async_runtime::spawn(async move {
                wait_for_server_and_navigate(&server_url, &app_handle).await;
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Kyomi Desktop");
}

// ═══════════════════════════════════════════════════════════════════════════
// Shared helpers
// ═══════════════════════════════════════════════════════════════════════════


fn set_window_icon(window: &tauri::WebviewWindow) {
    let icon_bytes = include_bytes!("../icons/128x128.png");
    if let Ok(icon) = tauri::image::Image::from_bytes(icon_bytes) {
        let _ = window.set_icon(icon);
    }
}

async fn wait_for_server_and_navigate(url: &str, app_handle: &tauri::AppHandle<tauri::Wry>) {
    let health_url = format!("{url}/api/health");
    let client = reqwest::Client::new();

    for attempt in 1..=60 {
        match client.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!(url, "Backend server ready");
                if let Some(window) = app_handle.get_webview_window("main") {
                    let parsed = url::Url::parse(url).expect("valid URL");
                    let _ = tauri::WebviewWindow::navigate(&window, parsed);
                }
                return;
            }
            _ => {
                if attempt % 10 == 0 {
                    tracing::debug!(attempt, "Waiting for backend server...");
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
    }
    tracing::error!("Backend server did not become ready within 15 seconds");
}

/// Start the full Kyomi axum server (personal mode only).
async fn start_server(
    config_arc: std::sync::Arc<kyomi_core::Config>,
    encryption_key_arc: std::sync::Arc<[u8; 32]>,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    let embedding = kyomi_embed::LazyEmbedding::new();
    let embedding_for_loader = embedding.clone();
    tokio::task::spawn_blocking(move || match kyomi_embed::EmbeddingService::new() {
        Ok(svc) => embedding_for_loader.set(svc),
        Err(e) => {
            tracing::error!("Failed to load embedding model: {e}");
            tracing::error!("Full error chain: {e:?}");
        }
    });

    let db = kyomi_core::db::DbPool::connect(&config_arc.database_url).await?;

    if config_arc.is_personal() {
        kyomi_server::auto_provision_personal_mode(&db).await?;
    }

    let kv = kyomi_core::create_kv_store(None).await?;

    let rp_origin = url::Url::parse(&config_arc.frontend_url)?;
    let webauthn = match kyomi_auth::webauthn::build_webauthn(
        &config_arc.webauthn_rp_id,
        &config_arc.webauthn_rp_name,
        &rp_origin,
    ) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!("WebAuthn unavailable ({e}) — passkeys disabled");
            let localhost_origin = url::Url::parse("http://localhost")?;
            kyomi_auth::webauthn::build_webauthn(
                "localhost",
                &config_arc.webauthn_rp_name,
                &localhost_origin,
            )?
        }
    };

    let enable_schedulers = config_arc.enable_schedulers;
    let ws_manager = kyomi_auth::websocket::WebSocketManager::new(None, db.clone());
    let mcp_sessions = kyomi_auth::mcp_session_manager::MCPSessionManager::new(kv.clone());
    let connect_registry = kyomi_datasource_server::ConnectRegistry::new_local();
    let registry = kyomi_core::platform::PlatformRegistry::new();
    let platforms = Arc::new(registry);

    let state = kyomi_server::state::AppState {
        db: db.clone(),
        kv: kv.clone(),
        redis: None,
        config: config_arc.clone(),
        encryption_key: encryption_key_arc.clone(),
        webauthn: Arc::new(webauthn),
        embedding: embedding.clone(),
        ws_manager: ws_manager.clone(),
        stripe: None,
        mcp_sessions,
        cancel_registry: kyomi_server::cancel_registry::CancelRegistry::default(),
        connect_token: None,
        connect_registry: connect_registry.clone(),
        platforms: platforms.clone(),
    };

    let shutdown_token = CancellationToken::new();

    let watch_scheduler = if enable_schedulers {
        let catalog_scheduler = Arc::new(kyomi_agent::CatalogRefreshScheduler::new(
            db.clone(),
            kv.clone(),
            encryption_key_arc.clone(),
            embedding.clone(),
            shutdown_token.child_token(),
        ));
        let _ = catalog_scheduler.start();
        tracing::info!("Catalog refresh scheduler started");

        let scheduler = Arc::new(kyomi_agent::WatchScheduler::new(
            kyomi_agent::WatchSchedulerDeps {
                db,
                kv,
                encryption_key: encryption_key_arc,
                embedding,
                ws_manager,
                config: config_arc.clone(),
                connect_registry: Some(connect_registry),
                platforms,
                cancel: shutdown_token.child_token(),
            },
        ));
        let _scheduler_handle = scheduler.clone().start();
        tracing::info!("Watch scheduler started");
        Some(scheduler)
    } else {
        None
    };

    let router = kyomi_server::build_router(state, kyomi_server::ServerExtras::default());
    let app = kyomi_server::wrap_service(router);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;

    tracing::info!(port, "Kyomi backend listening");
    axum::serve(listener, app).await?;

    shutdown_token.cancel();
    if let Some(scheduler) = watch_scheduler {
        scheduler.shutdown().await;
    }

    tracing::info!("Kyomi backend shutdown complete");
    Ok(())
}
