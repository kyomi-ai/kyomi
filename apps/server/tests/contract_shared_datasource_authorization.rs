// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exercise the actual generated datasource mutation routes. Shared settings
//! belong to the workspace and must remain admin-only even when members know
//! the datasource ID and submit a complete, otherwise valid configuration.

use std::collections::HashMap;

use kyomi_auth::{datasource_service, user_service, workspace_service};
use kyomi_test_harness::{cleanup_test_user, setup_auth_context};
use leptos::server_fn::ServerFn;
use serde_json::json;

#[tokio::test]
async fn member_cannot_create_or_change_workspace_shared_credentials() {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let ctx = setup_auth_context("Datasource owner", "sharedds", &suffix)
        .await
        .expect("shared datasource contract requires an in-process test server");
    let member_email = format!("sharedds-member-{suffix}@example.invalid");
    let member = user_service::create_user(&ctx.db, &member_email, Some("Member"), true)
        .await
        .expect("create member");
    workspace_service::create_workspace_user(
        &ctx.db,
        &ctx.workspace_id,
        &member.user_id,
        "workspace_user",
    )
    .await
    .expect("add member");
    let claims = HashMap::from([
        ("workspace_id".to_string(), json!(ctx.workspace_id)),
        ("email".to_string(), json!(member_email)),
    ]);
    let token =
        kyomi_auth::jwt::create_access_token_str(&member.user_id, &ctx.jwt_secret, 60, claims)
            .expect("member token");
    let original_config = json!({
        "host": "warehouse.example.invalid",
        "database": "analytics",
        "shared_credentials": true,
        "shared_username": "workspace-reader",
        "shared_password": "synthetic-original-password"
    });
    let ds = datasource_service::create_datasource(
        &ctx.db,
        datasource_service::CreateDatasourceParams {
            workspace_id: &ctx.workspace_id,
            name: "Shared warehouse",
            slug: Some("shared-warehouse"),
            ds_type: "postgres",
            connection_config: original_config.clone(),
            connection_type: Some("direct"),
            encryption_key: &ctx.encryption_key,
        },
    )
    .await
    .expect("seed shared datasource");
    let client = reqwest::Client::new();
    let create_path = kyomi_ui::server_fns::datasources::CreateDatasourceModal::PATH;
    let create = client
        .post(format!("{}{create_path}", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={token}"))
        .json(&json!({
            "name": "Unauthorized shared warehouse",
            "slug": "unauthorized-shared-warehouse",
            "datasource_type": "postgres",
            "connection_config": original_config,
            "credentials": {}
        }))
        .send()
        .await
        .expect("member create request");
    assert!(!create.status().is_success());
    assert!(
        create
            .text()
            .await
            .unwrap()
            .contains("Workspace admin access required")
    );
    assert!(
        datasource_service::get_datasource_by_slug(
            &ctx.db,
            "unauthorized-shared-warehouse",
            &ctx.workspace_id,
        )
        .await
        .unwrap()
        .is_none(),
        "rejected create must not persist a datasource"
    );

    let update_path = kyomi_ui::server_fns::datasources::UpdateDatasourceSettings::PATH;
    let changed_config = json!({
        "host": "warehouse.example.invalid",
        "database": "analytics",
        "shared_credentials": false,
        "shared_username": "changed-reader",
        "shared_password": "synthetic-rotated-password"
    });
    let payload = json!({
        "datasource_id": ds.id,
        "name": "Changed warehouse",
        "slug": "shared-warehouse",
        "connection_config": changed_config
    });
    let update = client
        .post(format!("{}{update_path}", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={token}"))
        .json(&payload)
        .send()
        .await
        .expect("member update request");
    assert!(!update.status().is_success());
    assert!(
        update
            .text()
            .await
            .unwrap()
            .contains("Workspace admin access required")
    );
    let after = datasource_service::get_datasource(&ctx.db, &ds.id, &ctx.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.connection_config, ds.connection_config);
    assert_eq!(after.name, ds.name);

    // Positive control: the same payload traverses the route and persists for
    // an administrator, ruling out a malformed request or unavailable route.
    let admin_update = client
        .post(format!("{}{update_path}", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .json(&payload)
        .send()
        .await
        .expect("admin update request");
    assert!(admin_update.status().is_success());
    let admin_response = admin_update.text().await.unwrap();
    assert!(!admin_response.contains("synthetic-rotated-password"));
    assert!(!admin_response.contains("shared_password"));
    let after_admin = datasource_service::get_datasource(&ctx.db, &ds.id, &ctx.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let config = &after_admin.connection_config;
    assert_eq!(config["shared_credentials"], false);
    assert_eq!(config["shared_username"], "changed-reader");
    assert_eq!(after_admin.name, "Changed warehouse");
    cleanup_test_user(&ctx.db, &member_email).await;
    cleanup_test_user(
        &ctx.db,
        &format!("sharedds-test-{suffix}@contract-test.local"),
    )
    .await;
}
