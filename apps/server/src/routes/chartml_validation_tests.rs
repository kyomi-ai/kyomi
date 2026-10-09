// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
use std::sync::Arc;

async fn state() -> AppState {
    let config = kyomi_core::Config::test_config();
    let db = kyomi_core::DbPool::Sqlite(
        sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
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
    AppState {
        db: db.clone(),
        kv: kv.clone(),
        redis: None,
        config: Arc::new(config),
        encryption_key: Arc::new([0; 32]),
        webauthn: Arc::new(webauthn),
        embedding: kyomi_embed::LazyEmbedding::new(),
        ws_manager: kyomi_auth::websocket::WebSocketManager::new(None, db),
        stripe: None,
        mcp_sessions: kyomi_auth::mcp_session_manager::MCPSessionManager::new(kv),
        cancel_registry: crate::cancel_registry::CancelRegistry::default(),
        connect_token: None,
        connect_registry: kyomi_datasource_server::ConnectRegistry::new_local(),
        platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
        schema_drift: crate::schema_drift::SchemaDriftStatus::default(),
        process_instance: "validation-test".into(),
    }
}
fn user() -> AuthUser {
    AuthUser {
        user_id: "user-a".into(),
        email: "a@test.local".into(),
        name: None,
        roles: vec![],
        active: true,
        verified: true,
        workspace: kyomi_auth::middleware::WorkspaceContext {
            workspace_id: Some("ws-1".into()),
            ..Default::default()
        },
        token_exp: None,
        token_jti: None,
    }
}
#[tokio::test]
async fn rest_handlers_apply_schema_sql_and_component_locations() {
    let state = state().await;
    let chart =
        "type: chart\nversion: 1\ndata: {provider: inline, rows: []}\nvisualize: {type: bar}";
    let Json(response) = validate_chartml(
        State(state.clone()),
        user(),
        Json(ValidateRequest {
            chartml: chart.into(),
        }),
    )
    .await
    .unwrap();
    assert!(response.valid);
    let content = format!(
        "```chartml\n{chart}\n```\n```chartml\n- type: config\n  version: 1\n- type: chart\n  version: 1\n  data: {{provider: inline, rows: []}}\n  visualize: {{type: wrong}}\n```"
    );
    let Json(response) = validate_markdown(
        State(state.clone()),
        user(),
        Json(ValidateMarkdownRequest { content }),
    )
    .await
    .unwrap();
    assert!(!response.valid);
    assert!(response.blocks[0].valid);
    assert!(
        response.blocks[1].errors.iter().any(|e| e.block == 2
            && e.component == Some(2)
            && e.instance_path == "/1/visualize/type")
    );
    let Json(response) = validate_chartml(State(state), user(), Json(ValidateRequest { chartml: "type: chart\nversion: 1\ndata: {datasource: absent, query: SELECT 1}\nvisualize: {type: bar}".into() })).await.unwrap();
    assert!(!response.valid);
    assert_eq!(response.errors[0].stage, "sql_datasource");
    let Json(schema) = get_schema(user()).await.unwrap();
    assert_eq!(
        schema,
        serde_json::from_str::<Value>(kyomi_core::chartml_validation::SCHEMA).unwrap()
    );
}

#[tokio::test]
async fn chartml_rest_unclosed_tail_is_not_a_zero_block_success() {
    let Json(response) = validate_markdown(State(state().await), user(), Json(ValidateMarkdownRequest {
        content: "```chartml\ntype: chart\nversion: 1\ndata: {query: {datasource: absent, query: SELECT 1}}\nvisualize: {type: bar}".into()
    })).await.unwrap();
    assert!(!response.valid);
    assert_eq!(response.blocks.len(), 1);
    assert_eq!(response.blocks[0].errors[0].stage, "sql_datasource");
    assert_eq!(
        response.blocks[0].errors[0].instance_path,
        "/data/query/query"
    );
}

#[tokio::test]
async fn chartml_bare_fence_prefix_rest_rejects_trailing_invalid_chart() {
    let Json(response) = validate_markdown(
        State(state().await),
        user(),
        Json(ValidateMarkdownRequest {
            content: "```\n```chartml\ntype: chart\nversion: 99".into(),
        }),
    )
    .await
    .unwrap();
    assert!(!response.valid);
    assert_eq!(response.blocks.len(), 1);
    assert!(
        response.blocks[0]
            .errors
            .iter()
            .any(|e| e.stage == "schema")
    );
}
