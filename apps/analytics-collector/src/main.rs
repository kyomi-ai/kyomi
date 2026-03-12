mod batcher;
mod clickhouse;
mod collector;
mod models;
mod quota;
mod snippet;
mod transform;

use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .json()
        .init();

    // Create ClickHouse client (tables are created during site provisioning)
    let ch_client = Arc::new(clickhouse::create_client());

    // Connect to Redis (optional — fail-open if unavailable)
    let redis = match std::env::var("REDIS_URL") {
        Ok(url) => {
            let client = redis::Client::open(url.as_str())
                .expect("Invalid REDIS_URL");
            match redis::aio::ConnectionManager::new(client).await {
                Ok(cm) => {
                    info!("Connected to Redis for quota enforcement and session tracking");
                    Some(cm)
                }
                Err(e) => {
                    error!(error = %e, "Failed to connect to Redis — quota enforcement and session tracking degraded");
                    None
                }
            }
        }
        Err(_) => {
            info!("REDIS_URL not set — quota enforcement and session tracking degraded");
            None
        }
    };

    // Create event batcher (spawns background flush loop)
    let batcher = batcher::EventBatcher::new(ch_client.clone());

    // Load transform definitions and start the transform engine
    let transforms_dir = std::env::var("TRANSFORMS_DIR")
        .unwrap_or_else(|_| "./transforms".into());
    let transforms_path = std::path::Path::new(&transforms_dir);

    let (transform_engine, ch_http_config) = match transform::engine::TransformEngine::new(transforms_path) {
        Ok(engine) => {
            let engine = Arc::new(engine);
            let ch_config = transform::engine::ChHttpConfig::from_env();
            (Some(engine), Some(ch_config))
        }
        Err(e) => {
            error!(error = %e, "Failed to load transforms — running without transforms");
            (None, None)
        }
    };

    // Build shared application state (secrets loaded once at startup)
    let state = Arc::new(collector::AppState::new(
        ch_client.clone(),
        redis,
        batcher,
        transform_engine,
        ch_http_config,
    ));

    let port: u16 = std::env::var("COLLECTOR_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8003);

    // Build router
    let app = Router::new()
        .route("/health", get(health))
        .route("/api/collect", post(collector::collect_event))
        .route("/api/collect", axum::routing::options(collector::collect_preflight))
        .route("/api/k.js", get(snippet::serve_snippet))
        .route("/k.js", get(snippet::serve_snippet))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!(addr = %addr, "Starting analytics collector");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind listener");

    axum::serve(listener, app).await.expect("Server error");
}

/// GET /health — simple health check.
async fn health(
    axum::extract::State(state): axum::extract::State<Arc<collector::AppState>>,
) -> StatusCode {
    match crate::clickhouse::health_check(&state.clickhouse).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            tracing::warn!(error = ?e, "Health check failed");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}
