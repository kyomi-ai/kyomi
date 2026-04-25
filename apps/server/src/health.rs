// SPDX-License-Identifier: AGPL-3.0-or-later

//! Health check endpoint.
//!
//! - `GET /api/health` — service health with DB and KV store checks
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
/// Checks database and KV store (Redis or in-memory).
/// Returns "healthy" when all required services are connected.
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

    let all_healthy = db_ok && kv_ok;

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
