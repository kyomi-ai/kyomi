// SPDX-License-Identifier: AGPL-3.0-or-later
//! Drive the installed production build_router TraceLayer, not its span helper.
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use tracing::{
    field::{Field, Visit},
    instrument::WithSubscriber,
};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<String>>>);
impl Visit for Capture {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{}={value:?}", field.name()));
    }
}
impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_new_span(
        &self,
        attributes: &tracing::span::Attributes<'_>,
        _id: &tracing::Id,
        _context: Context<'_, S>,
    ) {
        self.0
            .lock()
            .unwrap()
            .push(format!("span={}", attributes.metadata().name()));
        attributes.record(&mut self.clone());
    }
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        event.record(&mut self.clone());
    }
}
/// No network, migrations or embedding model is needed: the real callback
/// handler's default-off gate succeeds in producing a response before DB work.
async fn app_state() -> crate::state::AppState {
    let _ = kyomi_core::constants::load_with_fallback();
    let config = kyomi_core::Config::test_config();
    let db = kyomi_core::DbPool::Sqlite(
        sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap(),
    );
    let kv = kyomi_core::kv_store::create_kv_store(None).await.unwrap();
    let origin = url::Url::parse(&config.frontend_url).unwrap();
    let webauthn = kyomi_auth::webauthn::build_webauthn(
        &config.webauthn_rp_id,
        &config.webauthn_rp_name,
        &origin,
    )
    .unwrap();
    crate::state::AppState {
        db: db.clone(),
        kv: kv.clone(),
        redis: None,
        config: Arc::new(config),
        encryption_key: Arc::new([43; 32]),
        webauthn: Arc::new(webauthn),
        embedding: kyomi_embed::LazyEmbedding::new(),
        ws_manager: kyomi_auth::websocket::WebSocketManager::new(None, db),
        stripe: None,
        mcp_sessions: kyomi_auth::mcp_session_manager::MCPSessionManager::new(kv),
        cancel_registry: crate::cancel_registry::CancelRegistry::default(),
        connect_token: None,
        connect_registry: crate::connect::registry::ConnectRegistry::new_local(),
        platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
        schema_drift: crate::schema_drift::SchemaDriftStatus::default(),
        process_instance: "oauth-trace-test:0".into(),
    }
}
#[tokio::test]
async fn personal_mcp_oauth_global_http_span_omits_query_headers_and_bodies() {
    let app = super::build_router(app_state().await, super::ServerExtras::default());
    let fields = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(Capture(fields.clone()));
    let request = axum::http::Request::builder()
        .uri("/api/v1/personal-mcp/oauth/callback?code=test-private-code&state=test-private-state")
        .header("authorization", "Bearer test-private-login-token")
        .body(axum::body::Body::from("test-private-token-body"))
        .unwrap();
    let response = async { app.oneshot(request).await.unwrap() }
        .with_subscriber(subscriber)
        .await;
    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    let recorded = fields.lock().unwrap().join(" ");
    // Positive controls make a missing/uninstalled trace layer fail too.
    assert!(recorded.contains("span=http_request"));
    assert!(recorded.contains("method=GET"));
    assert!(recorded.contains("path=\"/api/v1/personal-mcp/oauth/callback\""));
    for secret in [
        "test-private-code",
        "test-private-state",
        "test-private-login-token",
        "test-private-token-body",
        "?code=",
    ] {
        assert!(!recorded.contains(secret));
    }
}
