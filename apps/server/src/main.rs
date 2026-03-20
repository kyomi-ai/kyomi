// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kyomi Rust backend — entry point.

use kyomi_core::Config;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Structured logging — respects RUST_LOG env var
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .json()
        .init();

    // Load shared constants — tries disk first, falls back to embedded copy for standalone mode
    kyomi_core::constants::load_with_fallback()
        .expect("failed to load shared constants");

    let config = Config::from_env();
    let port = config.port;

    // Derive encryption key at startup (fail fast if misconfigured)
    let encryption_key = kyomi_auth::encryption::derive_key(&config.encryption_key)
        .expect("ENCRYPTION_KEY must be a valid 32-byte base64url-encoded key");

    // Lazy-load embedding model on a background thread (~440ms).
    // The server starts listening immediately; endpoints that need embeddings
    // get 503 during the brief warmup window.
    let embedding = kyomi_embed::LazyEmbedding::new();
    let embedding_for_loader = embedding.clone();
    tokio::task::spawn_blocking(move || {
        match kyomi_embed::EmbeddingService::new() {
            Ok(svc) => embedding_for_loader.set(svc),
            Err(e) => {
                tracing::error!("Failed to load embedding model: {e}");
                tracing::error!("Full error chain: {e:?}");
                std::process::exit(1);
            }
        }
    });

    // Connect to database and run migrations (Postgres or SQLite based on URL prefix)
    let db = kyomi_core::db::DbPool::connect(&config.database_url)
        .await
        .expect("failed to connect to database");

    // KVStore: abstracted key-value store for auth, rate limiting, and session ops.
    // Uses Redis when REDIS_URL is set; falls back to in-memory for single-instance mode.
    let redis_url = config.redis_url.clone(); // now Option<String>
    let kv = kyomi_core::create_kv_store(redis_url.as_deref())
        .await
        .expect("failed to initialise KV store");

    // Raw Redis pool for components that require direct Redis access:
    // agent execution (Lua scripts, pub/sub), ConnectRegistry, analytics counters.
    let redis: Option<kyomi_core::RedisPool> = if let Some(ref url) = redis_url {
        match kyomi_core::redis::create_pool(url).await {
            Ok(pool) => Some(pool),
            Err(e) => {
                tracing::error!(error = %e, "Failed to connect to Redis");
                std::process::exit(1);
            }
        }
    } else {
        tracing::info!("REDIS_URL not set — running in single-instance mode (in-memory KV store)");
        None
    };

    // Build WebAuthn instance.
    // In self-hosted mode, passkeys may not be available (e.g., HTTP-only or IP
    // address origin). Fall back to localhost so the server starts — passkey routes
    // will be unavailable but password auth works.
    let rp_origin = url::Url::parse(&config.frontend_url)
        .expect("FRONTEND_URL must be a valid URL for WebAuthn RP origin");
    let webauthn = match kyomi_auth::webauthn::build_webauthn(
        &config.webauthn_rp_id,
        &config.webauthn_rp_name,
        &rp_origin,
    ) {
        Ok(w) => w,
        Err(e) if config.self_hosted => {
            tracing::warn!(
                "WebAuthn unavailable ({e}) — passkeys disabled. \
                 Passkeys require HTTPS and a domain name (not an IP address)."
            );
            let localhost_origin = url::Url::parse("http://localhost")
                .expect("hardcoded URL");
            kyomi_auth::webauthn::build_webauthn("localhost", &config.webauthn_rp_name, &localhost_origin)
                .expect("fallback WebAuthn with localhost should always succeed")
        }
        Err(e) => panic!("failed to build WebAuthn instance: {e}"),
    };

    let enable_schedulers = config.enable_schedulers;
    let encryption_key_arc = Arc::new(encryption_key);

    // Clone state needed for analytics background jobs (Postgres-only)
    let analytics_redis = redis.clone(); // Option<RedisPool>
    let analytics_pg = if db.is_postgres() {
        Some(db.pg_pool().clone())
    } else {
        None
    };

    // WebSocket manager — uses Redis pub/sub when available for multi-replica delivery.
    let ws_redis = redis.as_ref().map(|pool| (pool.clone(), redis_url.clone().expect("redis pool implies redis_url")));
    let ws_manager = kyomi_auth::websocket::WebSocketManager::new(
        ws_redis,
        db.clone(),
    );

    let config_arc = Arc::new(config);
    let analytics_config = config_arc.clone();

    // Validate Slack configuration: warn if client ID is set but signing secret is missing.
    // In production this means webhook signature verification is disabled — a security risk.
    #[cfg(feature = "slack")]
    if config_arc.slack_client_id.as_ref().is_some_and(|s| !s.is_empty())
        && config_arc
            .slack_signing_secret
            .as_ref()
            .map_or(true, |s| s.is_empty())
    {
        tracing::warn!(
            "SLACK_CLIENT_ID is set but SLACK_SIGNING_SECRET is missing — \
             Slack webhook signature verification is DISABLED. \
             This is expected in dev but dangerous in production."
        );
    }

    // Build Stripe service if configured (optional — dev/test may not have keys)
    let stripe = match (
        config_arc.stripe_secret_key.as_deref(),
        config_arc.stripe_webhook_secret.as_deref(),
    ) {
        (Some(secret_key), Some(webhook_secret)) if !secret_key.is_empty() => {
            let svc =
                kyomi_auth::stripe_service::StripeService::new(secret_key, webhook_secret);
            let is_test = kyomi_auth::stripe_config::is_test_mode(secret_key);
            tracing::info!(
                test_mode = is_test,
                "Stripe service initialised"
            );
            Some(Arc::new(svc))
        }
        _ => {
            tracing::info!("Stripe not configured — billing features disabled");
            None
        }
    };

    // SlackClient creation moved to enterprise/kyomi-slack crate (Phase 12).
    // Create SlackClient early; SlackPlatform + SlackState built after connect_registry.
    #[cfg(feature = "slack")]
    let slack_client = match kyomi_slack::client::SlackClient::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create Slack client");
            std::process::exit(1);
        }
    };

    // MCP session manager — KVPool-backed for multi-replica consistency.
    let mcp_sessions = kyomi_server::mcp_session_manager::MCPSessionManager::new(kv.clone());

    // Build ConnectTokenService if private key is configured (optional — Connect features)
    let connect_token = match config_arc.connect_jwt_private_key.as_deref() {
        Some(pem) if !pem.is_empty() => {
            match kyomi_auth::connect_token::ConnectTokenService::new(pem, &config_arc.connect_url) {
                Ok(svc) => {
                    tracing::info!("Connect token service initialised (JWKS available at /.well-known/jwks.json)");
                    Some(Arc::new(svc))
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to initialise Connect token service — Connect features disabled");
                    None
                }
            }
        }
        _ => {
            tracing::info!("CONNECT_JWT_PRIVATE_KEY not configured — Connect features disabled");
            None
        }
    };

    // ConnectRegistry: backed by Redis for cross-replica routing when available,
    // local-only (single-pod) when Redis is absent.
    let connect_registry = if let (Some(redis_pool), Some(url)) = (&redis, &redis_url) {
        kyomi_datasource_server::ConnectRegistry::new(redis_pool.clone(), url.clone())
    } else {
        tracing::info!("ConnectRegistry running in local-only mode — REDIS_URL not configured");
        kyomi_datasource_server::ConnectRegistry::new_local()
    };

    // Platform registry — register messaging platform implementations.
    #[cfg_attr(not(feature = "slack"), allow(unused_mut))]
    let mut registry = kyomi_core::platform::PlatformRegistry::new();

    #[cfg(feature = "slack")]
    {
        let slack_platform = kyomi_slack::SlackPlatform::new(
            slack_client.clone(),
            db.clone(),
            config_arc.clone(),
            encryption_key_arc.clone(),
            Some(connect_registry.clone()),
        );
        registry.register(Arc::new(slack_platform));
    }

    let platforms = Arc::new(registry);

    #[cfg(feature = "slack")]
    let slack_state = kyomi_slack::SlackState {
        db: db.clone(),
        kv: kv.clone(),
        redis: redis.clone(),
        config: config_arc.clone(),
        encryption_key: encryption_key_arc.clone(),
        slack_client,
        ws_manager: ws_manager.clone(),
        embedding: embedding.clone(),
        connect_registry: connect_registry.clone(),
        platforms: platforms.clone(),
    };

    let state = kyomi_server::state::AppState {
        db: db.clone(),
        kv: kv.clone(),
        redis: redis.clone(),
        config: config_arc.clone(),
        encryption_key: encryption_key_arc.clone(),
        webauthn: Arc::new(webauthn),
        embedding: embedding.clone(),
        ws_manager: ws_manager.clone(),
        stripe,
        mcp_sessions,
        cancel_registry: kyomi_server::cancel_registry::CancelRegistry::default(),
        connect_token,
        connect_registry,
        platforms,
    };

    // Shared cancellation token for graceful shutdown of all background tasks
    let shutdown_token = CancellationToken::new();

    // Start background schedulers (if enabled)
    let watch_scheduler: Option<Arc<kyomi_agent::WatchScheduler>> = if enable_schedulers {
        // Catalog refresh scheduler — hourly catalog re-indexing + daily token cleanup
        let catalog_scheduler = Arc::new(kyomi_agent::CatalogRefreshScheduler::new(
            db.clone(),
            kv.clone(),
            encryption_key_arc.clone(),
            embedding.clone(),
            shutdown_token.child_token(),
        ));
        let (_refresh_handle, _cleanup_handle, _maintenance_handle, _query_history_cleanup_handle, _public_dataset_handle) = catalog_scheduler.start();
        tracing::info!("Catalog refresh scheduler started");

        // Watch scheduler — polls for due watches every 30s
        let scheduler = Arc::new(kyomi_agent::WatchScheduler::new(
            db,
            kv.clone(),
            encryption_key_arc,
            embedding,
            ws_manager,
            config_arc.clone(),
            Some(state.connect_registry.clone()),
            state.platforms.clone(),
            shutdown_token.child_token(),
        ));
        let _handle = scheduler.clone().start();
        tracing::info!("Watch scheduler started");

        Some(scheduler)
    } else {
        tracing::info!("Background schedulers disabled (ENABLE_SCHEDULERS=false)");
        None
    };

    // Analytics background jobs are Postgres-only (ClickHouse migration, notifications, etc.)
    if let Some(analytics_db) = analytics_pg {
        // One-shot: migrate analytics properties columns (Map -> String/JSON)
        {
            let db = analytics_db.clone();
            let ch_host = analytics_config.analytics_clickhouse_host.clone();
            let ch_port = analytics_config.analytics_clickhouse_port;
            let ch_pass = analytics_config.analytics_clickhouse_password.clone();
            let ch_secure = analytics_config.analytics_clickhouse_secure;
            tokio::spawn(async move {
                if let Err(e) = kyomi_auth::analytics_clickhouse::migrate_all_properties_columns(
                    &db, &ch_host, ch_port, &ch_pass, ch_secure,
                ).await {
                    tracing::warn!(error = %e, "Analytics properties migration failed — will retry on next restart");
                }
            });
        }

        // Analytics background jobs (notification dispatch, reconciliation, retention cleanup)
        // These require Redis for counter tracking — skipped in single-instance (no-Redis) mode.
        if enable_schedulers {
            if let Some(analytics_redis_pool) = analytics_redis {
            let email_service = kyomi_auth::email_service::EmailService::from_env();
            let frontend_url = analytics_config.frontend_url.clone();

            // Notification dispatch — every 30 seconds
            {
                let mut redis = analytics_redis_pool.clone();
                let db = analytics_db.clone();
                let shutdown = shutdown_token.child_token();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                    loop {
                        tokio::select! {
                            _ = shutdown.cancelled() => break,
                            _ = interval.tick() => {
                                kyomi_auth::analytics_notifications::dispatch_notifications(
                                    &mut redis, &db, &email_service, &frontend_url,
                                ).await;
                            }
                        }
                    }
                    tracing::info!("Analytics notification dispatcher stopped");
                });
                tracing::info!("Analytics notification dispatcher started (30s interval)");
            }

            // Counter reconciliation — every 6 hours
            {
                let mut redis = analytics_redis_pool.clone();
                let db = analytics_db.clone();
                let config = analytics_config.clone();
                let shutdown = shutdown_token.child_token();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
                    loop {
                        tokio::select! {
                            _ = shutdown.cancelled() => break,
                            _ = interval.tick() => {
                                if let Err(e) = kyomi_auth::analytics_quota::reconcile_counters(
                                    &mut redis, &db,
                                    &config.analytics_clickhouse_host,
                                    config.analytics_clickhouse_port,
                                    &config.analytics_clickhouse_password,
                                    config.analytics_clickhouse_secure,
                                ).await {
                                    tracing::warn!(error = %e, "Analytics counter reconciliation failed");
                                }
                            }
                        }
                    }
                    tracing::info!("Analytics counter reconciliation stopped");
                });
                tracing::info!("Analytics counter reconciliation started (6h interval)");
            }

            // Retention cleanup — every 24 hours
            {
                let db = analytics_db.clone();
                let config = analytics_config.clone();
                let shutdown = shutdown_token.child_token();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
                    loop {
                        tokio::select! {
                            _ = shutdown.cancelled() => break,
                            _ = interval.tick() => {
                                if let Err(e) = kyomi_auth::analytics_quota::cleanup_retention(
                                    &db,
                                    &config.analytics_clickhouse_host,
                                    config.analytics_clickhouse_port,
                                    &config.analytics_clickhouse_password,
                                    config.analytics_clickhouse_secure,
                                ).await {
                                    tracing::warn!(error = %e, "Analytics retention cleanup failed");
                                }
                            }
                        }
                    }
                    tracing::info!("Analytics retention cleanup stopped");
                });
                tracing::info!("Analytics retention cleanup started (24h interval)");
            }
            } else {
                tracing::info!("Analytics background jobs skipped — no Redis configured (single-instance mode)");
            }
        }
    } else {
        tracing::info!("SQLite backend — analytics background jobs disabled");
    }

    // Build the core router, then conditionally mount platform-specific routes.
    let router = kyomi_server::build_router(state);

    #[cfg(feature = "slack")]
    let router = router.nest(
        "/api/v1/slack",
        kyomi_slack::routes::routes().with_state(slack_state),
    );

    let app = kyomi_server::wrap_service(router);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("failed to bind");

    // Startup banner
    let db_type = if config_arc.database_url.starts_with("sqlite://") || config_arc.database_url.starts_with("sqlite:") {
        "SQLite"
    } else {
        "PostgreSQL"
    };
    let edition = match config_arc.edition {
        kyomi_core::config::SelfHostedEdition::Enterprise => "Enterprise",
        kyomi_core::config::SelfHostedEdition::Community => "Community",
    };
    let llm_status = if config_arc.llm_configured() {
        "configured"
    } else {
        "not configured (set ANTHROPIC_API_KEY, OPENAI_API_KEY, or LLM_API_KEY)"
    };

    eprintln!();
    eprintln!("  Kyomi Data Intelligence Platform");
    eprintln!("  \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}");
    eprintln!("  URL:      http://localhost:{port}");
    eprintln!("  Edition:  {edition}");
    eprintln!("  Database: {db_type}");
    eprintln!("  LLM:      {llm_status}");
    eprintln!();

    tracing::info!("Kyomi Rust backend listening on port {port}");

    // Run server with graceful shutdown on SIGTERM/SIGINT
    let shutdown_signal = async {
        let ctrl_c = tokio::signal::ctrl_c();
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => {
                tracing::info!("Received SIGINT, initiating graceful shutdown");
            }
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM, initiating graceful shutdown");
            }
        }
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await
        .expect("server error");

    // Server has stopped accepting new connections — shut down background tasks
    tracing::info!("HTTP server stopped, shutting down background tasks");

    // Cancel all background tasks via the shared token
    shutdown_token.cancel();

    // Wait for the watch scheduler to drain active executions
    if let Some(scheduler) = watch_scheduler {
        scheduler.shutdown().await;
    }

    tracing::info!("Kyomi Rust backend shutdown complete");
}
