// SPDX-License-Identifier: AGPL-3.0-or-later
//! Browser/provider integration only. All OAuth decisions live in kyomi-auth,
//! shared with server-function callers; neither inbound OAuth nor datasource flows.
use axum::{
    Form, Json, Router,
    extract::{FromRef, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use kyomi_auth::{
    middleware::{AuthState, AuthUser},
    personal_mcp_oauth::{OAuthService, OAuthSettings},
    personal_mcp_service::require_feature,
};
use kyomi_core::{Config, Error, Result};
use kyomi_types::personal_mcp_oauth::{
    McpOAuthCallback, McpOAuthDiscovery, McpOAuthStarted, StartMcpOAuth,
};
use leptos::prelude::*;
use std::sync::Arc;

/// Small integration state also used by actual route contract fixtures.
#[derive(Clone)]
pub struct OAuthRouteState {
    pub auth: AuthState,
    pub config: Arc<Config>,
    pub key: Arc<[u8; 32]>,
    pub service: Option<Arc<OAuthService>>,
}
impl FromRef<OAuthRouteState> for AuthState {
    fn from_ref(state: &OAuthRouteState) -> Self {
        state.auth.clone()
    }
}
impl OAuthRouteState {
    fn service(&self) -> Result<&OAuthService> {
        require_feature(&self.config)?;
        self.service.as_deref().ok_or_else(|| Error::BadRequest("Personal MCP OAuth requires an HTTPS public origin and valid operator client settings".into()))
    }
}
pub fn production(state: &crate::state::AppState) -> Router<crate::state::AppState> {
    let service = if state.config.personal_mcp_enabled {
        OAuthSettings::from_deployment(&state.config.frontend_url)
            .and_then(OAuthService::production)
            .ok()
            .map(Arc::new)
    } else {
        None
    };
    routes(OAuthRouteState {
        auth: AuthState::from_ref(state),
        config: state.config.clone(),
        key: state.encryption_key.clone(),
        service,
    })
}
/// No AppState or network construction is hidden in handlers. Tests inject the
/// same orchestration service with a deterministic safe transport boundary.
pub fn routes<S: Clone + Send + Sync + 'static>(state: OAuthRouteState) -> Router<S> {
    Router::new()
        .route("/{connection}/discover", get(discover))
        .route("/{connection}/start", post(start))
        .route("/callback", get(callback_landing))
        .route("/callback/complete", post(callback))
        .route("/client-metadata.json", get(client_metadata))
        .layer(middleware::from_fn(private_response))
        .with_state(state)
}
async fn private_response(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    response
}
async fn discover(
    State(state): State<OAuthRouteState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<McpOAuthDiscovery>> {
    Ok(Json(
        state
            .service()?
            .discover(&state.auth.db, &user.user_id, &id)
            .await?,
    ))
}
async fn start(
    State(state): State<OAuthRouteState>,
    user: AuthUser,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<StartMcpOAuth>,
) -> Result<Json<McpOAuthStarted>> {
    let service = state.service()?;
    // The browser's explicit-consent POST must originate at this deployment.
    // Native authenticated callers can invoke the shared service directly.
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        != Some(service.settings().public_origin.as_str())
    {
        return Err(Error::Forbidden(
            "OAuth consent requires the same-origin browser".into(),
        ));
    }
    Ok(Json(
        service
            .start(&state.auth.db, &user.user_id, &id, input, &state.key)
            .await?,
    ))
}
/// An authorization-server redirect is cross-site: Strict login cookies are
/// withheld. A scriptless same-origin form restores the normal cookie context,
/// then the completion handler authenticates identity before any exchange.
async fn callback_landing(
    State(state): State<OAuthRouteState>,
    Query(query): Query<McpOAuthCallback>,
) -> Result<Html<String>> {
    state.service()?;
    if query.state.len() < 32
        || query.state.len() > 1024
        || query.code.as_ref().is_some_and(|value| value.len() > 8192)
        || query.iss.as_ref().is_some_and(|value| value.len() > 4096)
    {
        return Err(Error::BadRequest(
            "OAuth callback parameters are invalid".into(),
        ));
    }
    let owner = Owner::new();
    let body=owner.with(|| view! {
        <html><head><title>"Complete MCP connection"</title><meta name="referrer" content="no-referrer"/></head>
        <body><h1>"Complete your MCP connection"</h1>
        <p>"Continue with your current Kyomi account to finish connecting."</p>
        <form method="post" action="/api/v1/personal-mcp/oauth/callback/complete" autocomplete="off">
        <input type="hidden" name="state" value=query.state/>
        {query.code.map(|value| view! {<input type="hidden" name="code" value=value/>})}
        {query.iss.map(|value| view! {<input type="hidden" name="iss" value=value/>})}
        {query.error.map(|_| view! {<input type="hidden" name="error" value="access_denied"/>})}
        <button type="submit">"Complete connection"</button></form></body></html>
    }.to_html());
    Ok(Html(format!("<!doctype html>{body}")))
}
async fn callback(
    State(state): State<OAuthRouteState>,
    user: AuthUser,
    headers: HeaderMap,
    Form(query): Form<McpOAuthCallback>,
) -> Result<Response> {
    let service = state.service()?;
    if headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        != Some(service.settings().public_origin.as_str())
    {
        return Err(Error::Forbidden(
            "OAuth completion requires the same-origin browser".into(),
        ));
    }
    let return_to = service
        .callback(&state.auth.db, &user.user_id, query, &state.key)
        .await?;
    Ok(Redirect::to(&return_to).into_response())
}
async fn client_metadata(
    State(state): State<OAuthRouteState>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let settings = state.service()?.settings();
    if settings.client_metadata_url.is_none() {
        return Err(Error::NotFound(
            "OAuth client metadata document is not configured".into(),
        ));
    }
    Ok((StatusCode::OK, Json(settings.metadata_document())))
}
