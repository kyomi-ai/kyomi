// SPDX-License-Identifier: AGPL-3.0-or-later

//! HTTP regression for KYO-847: the generated UpdateCollection server fn
//! must reject a member who knows another creator's private collection ID.

use std::collections::HashMap;

use kyomi_auth::{collection_service, user_service, workspace_service};
use kyomi_test_harness::{cleanup_test_user, setup_auth_context};
use serde_json::json;

#[tokio::test]
async fn update_collection_route_rejects_member_admin_and_outsider() {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let Some(ctx) = setup_auth_context("Collection owner", "privatecoll", &suffix).await else {
        return;
    };
    let collection = collection_service::create_collection(collection_service::NewCollectionParams {
        db: &ctx.db,
        workspace_id: &ctx.workspace_id,
        name: "Private collection",
        description: None,
        color: None,
        is_public: false,
        doc_type: "dashboard",
        created_by: &ctx.user_id,
    }).await.expect("create private collection");
    let path = <kyomi_ui::server_fns::collections::UpdateCollection as leptos::server_fn::ServerFn>::PATH;
    let client = reqwest::Client::new();
    let owner = client.post(format!("{}{path}", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .form(&[("collection_id", collection.id.as_str()), ("is_public", "true")])
        .send().await.expect("owner request");
    assert!(owner.status().is_success(), "owner route control: {}", owner.status());
    assert!(collection_service::get_collection(&ctx.db, &collection.id, &ctx.workspace_id, &ctx.user_id)
        .await.unwrap().unwrap().is_public);
    let owner_private = client.post(format!("{}{path}", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .form(&[("collection_id", collection.id.as_str()), ("is_public", "false")])
        .send().await.expect("owner private request");
    assert!(owner_private.status().is_success(), "owner can make collection private again");

    for (role, member) in [("workspace_user", true), ("workspace_admin", true), ("workspace_user", false)] {
        let email = format!("privatecoll-{role}-{member}-{suffix}@example.invalid");
        let user = user_service::create_user(&ctx.db, &email, Some("Synthetic actor"), true)
            .await.expect("create actor");
        let token_workspace = if member {
            workspace_service::create_workspace_user(&ctx.db, &ctx.workspace_id, &user.user_id, role)
                .await.expect("create member");
            ctx.workspace_id.clone()
        } else {
            user_service::create_workspace_for_user(&ctx.db, &user.user_id,
                Some("Other workspace"), &email, None).await.expect("create outsider workspace")
        };
        let claims = HashMap::from([
            ("workspace_id".to_string(), json!(token_workspace)),
            ("email".to_string(), json!(email)),
            ("workspace_roles".to_string(), json!([role])),
        ]);
        let token = kyomi_auth::jwt::create_access_token_str(&user.user_id, &ctx.jwt_secret, 60, claims)
            .expect("actor token");
        let response = client.post(format!("{}{path}", ctx.base_url))
            .header("origin", "http://localhost:5173")
            .header("cookie", format!("access_token={token}"))
            .form(&[("collection_id", collection.id.as_str()), ("is_public", "true")])
            .send().await.expect("actor request");
        assert!(!response.status().is_success(),
            "{role} member={member} must not publish private collection: {}", response.status());
        let after = collection_service::get_collection(&ctx.db, &collection.id, &ctx.workspace_id, &ctx.user_id)
            .await.expect("read collection").expect("collection remains");
        assert!(!after.is_public, "denied route must leave visibility unchanged");
        cleanup_test_user(&ctx.db, &email).await;
    }
    cleanup_test_user(&ctx.db, &format!("privatecoll-test-{suffix}@contract-test.local")).await;
}
