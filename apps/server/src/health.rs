// SPDX-License-Identifier: AGPL-3.0-or-later

//! Health check endpoint.
//!
//! - `GET /api/health` — service health with DB, KV store, chart renderer checks
//! - `GET /health` — alias for nginx-less deployments
//! - `GET /api/v1/health` — alias for uptime probes targeting the versioned API prefix

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Response shape matching the Python backend exactly.
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub services: std::collections::HashMap<String, String>,
}

/// Detect the database backend type from the connection URL.
fn database_type(database_url: &str) -> &'static str {
    if database_url.starts_with("sqlite://") || database_url.starts_with("sqlite:") {
        "sqlite"
    } else {
        "postgresql"
    }
}

/// Health check endpoint for load balancers and monitoring.
///
/// Checks database, KV store (Redis or in-memory), and optionally chart renderer.
/// Returns "healthy" when all required services are connected.
/// Chart renderer is only checked when explicitly configured.
pub async fn health_check(State(state): State<AppState>) -> Json<HealthResponse> {
    let mut services = std::collections::HashMap::new();
    let db_type = database_type(&state.config.database_url);

    // Check database (PostgreSQL or SQLite)
    let db_ok = match kyomi_core::db::ping(&state.db).await {
        Ok(()) => {
            services.insert("database".into(), format!("connected ({db_type})"));
            true
        }
        Err(e) => {
            tracing::error!("Database health check failed: {e}");
            services.insert("database".into(), format!("unavailable ({db_type})"));
            false
        }
    };

    // Check KV store (Redis-backed or in-memory depending on REDIS_URL)
    let kv_ok = match state.kv.ping().await {
        Ok(()) => {
            services.insert("kv_store".into(), "connected".into());
            true
        }
        Err(e) => {
            tracing::error!("KV store health check failed: {e}");
            services.insert("kv_store".into(), "unavailable".into());
            false
        }
    };

    // Check Chart Renderer — only when explicitly configured
    let chart_ok = if state.config.chart_renderer_configured() {
        let chart_renderer_url = &state.config.chart_renderer_url;
        let chart_check = async {
            kyomi_datasource_server::http_client()?
                .get(format!("{chart_renderer_url}/health"))
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
                .map_err(|e| kyomi_core::Error::Internal(e.to_string()))
        };
        match chart_check.await {
            Ok(resp) if resp.status().is_success() => {
                services.insert("chart_renderer".into(), "connected".into());
                true
            }
            Ok(resp) => {
                tracing::error!(
                    "Chart renderer health check returned status {}",
                    resp.status()
                );
                services.insert("chart_renderer".into(), "unavailable".into());
                false
            }
            Err(e) => {
                tracing::error!("Chart renderer health check failed: {e}");
                services.insert("chart_renderer".into(), "unavailable".into());
                false
            }
        }
    } else {
        // Not configured — not a health concern (standalone mode has no chart renderer)
        true
    };

    let all_healthy = db_ok && kv_ok && chart_ok;

    let version = &kyomi_core::constants::get().api.version;

    Json(HealthResponse {
        status: if all_healthy {
            "healthy".into()
        } else {
            "degraded".into()
        },
        version: version.clone(),
        services,
    })
}
