// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
use kyomi_core::{db_execute, db_fetch_scalar};

const KEY: [u8; 32] = [23; 32];
fn input(name: &str, kind: McpAuthKind, secret: Option<&str>) -> SavePersonalMcpConnection {
    SavePersonalMcpConnection {
        name: name.into(),
        endpoint: "https://example.com/mcp".into(),
        auth_kind: kind,
        bearer_secret: secret.map(str::to_owned),
    }
}

async fn matrix(pool: DbPool) {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let a = format!("mcp-a-{suffix}");
    let b = format!("mcp-b-{suffix}");
    let ws = format!("mcp-w-{suffix}");
    let ws2 = format!("mcp-x-{suffix}");
    for user in [&a, &b] {
        db_execute!(
            &pool,
            "INSERT INTO users (user_id, email) VALUES ($1, $2)",
            user,
            format!("{user}@example.com")
        )
        .unwrap();
    }
    for workspace in [&ws, &ws2] {
        db_execute!(
            &pool,
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $1, $2)",
            workspace,
            &a
        )
        .unwrap();
        for (user, role) in [(&a, "workspace_user"), (&b, "workspace_admin")] {
            db_execute!(&pool, "INSERT INTO workspace_users (workspace_id, user_id, role, active) VALUES ($1,$2,$3,TRUE)", workspace, user, role).unwrap();
        }
    }
    let bearer = create_connection(
        &pool,
        &a,
        input(
            "Personal account",
            McpAuthKind::Bearer,
            Some("test-personal-token"),
        ),
        &KEY,
    )
    .await
    .unwrap();
    assert_eq!(bearer.generation, 1);
    assert_eq!(bearer.endpoint, "https://example.com/mcp");
    let public = serde_json::to_string(&bearer).unwrap();
    assert!(!public.contains("test-personal-token"));
    assert!(!public.contains("secret"));
    assert!(
        !format!(
            "{:?}",
            input("x", McpAuthKind::Bearer, Some("test-personal-token"))
        )
        .contains("test-personal-token")
    );
    let stored: String = db_fetch_scalar!(
        &pool,
        String,
        "SELECT encrypted_secret FROM user_mcp_secrets WHERE connection_id = $1",
        &bearer.connection_id
    )
    .unwrap();
    assert_ne!(stored, "test-personal-token");
    assert_eq!(
        crate::encryption::decrypt(&stored, &KEY).unwrap(),
        "test-personal-token"
    );
    assert!(
        create_connection(
            &pool,
            &a,
            input("Personal account", McpAuthKind::None, None),
            &KEY
        )
        .await
        .is_err()
    );
    let second = create_connection(
        &pool,
        &a,
        input("Second account", McpAuthKind::None, None),
        &KEY,
    )
    .await
    .unwrap();
    assert_ne!(bearer.connection_id, second.connection_id);
    assert_eq!(list_connections(&pool, &a, "", 100).await.unwrap().len(), 2);
    assert_eq!(list_connections(&pool, &a, "", 1).await.unwrap().len(), 1);
    assert!(
        list_connections(&pool, &b, "", 100)
            .await
            .unwrap()
            .is_empty()
    );
    let id = &bearer.connection_id;
    assert!(matches!(
        get_connection(&pool, &b, id).await,
        Err(Error::NotFound(_))
    ));
    assert!(
        update_connection(
            &pool,
            &b,
            id,
            1,
            input("stolen", McpAuthKind::None, None),
            &KEY
        )
        .await
        .is_err()
    );
    assert!(disable_connection(&pool, &b, id, 1).await.is_err());
    assert!(delete_connection(&pool, &b, id, 1).await.is_err());
    assert!(
        set_workspace_enabled(&pool, &b, id, 1, &ws, true)
            .await
            .is_err()
    );
    assert!(
        require_workspace_connection(&pool, &b, id, 1, &ws)
            .await
            .is_err()
    );
    assert!(
        load_workspace_secret(&pool, &b, id, 1, &ws, &KEY)
            .await
            .is_err()
    );
    assert!(
        require_workspace_connection(&pool, &a, id, 1, &ws)
            .await
            .is_err()
    );
    set_workspace_enabled(&pool, &a, id, 1, &ws, true)
        .await
        .unwrap();
    set_workspace_enabled(&pool, &a, id, 1, &ws, true)
        .await
        .unwrap();
    assert!(
        require_workspace_connection(&pool, &a, id, 1, &ws2)
            .await
            .is_err()
    );
    let secret = load_workspace_secret(&pool, &a, id, 1, &ws, &KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(secret.expose(), "test-personal-token");
    assert!(!format!("{secret:?}").contains("test-personal-token"));
    assert!(
        create_connection(
            &pool,
            &a,
            input("mask", McpAuthKind::Bearer, Some("********")),
            &KEY
        )
        .await
        .is_err()
    );
    assert!(
        create_connection(
            &pool,
            &a,
            input("none-secret", McpAuthKind::None, Some("bad")),
            &KEY
        )
        .await
        .is_err()
    );
    assert!(
        create_connection(&pool, &a, input("missing", McpAuthKind::Bearer, None), &KEY)
            .await
            .is_err()
    );
    // A failed SQL uniqueness update rolls back generation and preserves ciphertext.
    assert!(
        update_connection(
            &pool,
            &a,
            id,
            1,
            input("Second account", McpAuthKind::Bearer, Some("replacement")),
            &KEY
        )
        .await
        .is_err()
    );
    assert_eq!(get_connection(&pool, &a, id).await.unwrap().generation, 1);
    assert_eq!(
        load_workspace_secret(&pool, &a, id, 1, &ws, &KEY)
            .await
            .unwrap()
            .unwrap()
            .expose(),
        "test-personal-token"
    );
    let updated = update_connection(
        &pool,
        &a,
        id,
        1,
        input("Renamed", McpAuthKind::Bearer, None),
        &KEY,
    )
    .await
    .unwrap();
    assert_eq!(updated.generation, 2);
    assert_eq!(
        load_workspace_secret(&pool, &a, id, 2, &ws, &KEY)
            .await
            .unwrap()
            .unwrap()
            .expose(),
        "test-personal-token"
    );
    assert!(
        update_connection(
            &pool,
            &a,
            id,
            1,
            input("stale", McpAuthKind::Bearer, Some("wrong")),
            &KEY
        )
        .await
        .is_err()
    );
    let rotated = update_connection(
        &pool,
        &a,
        id,
        2,
        input("Renamed", McpAuthKind::Bearer, Some("rotated-token")),
        &KEY,
    )
    .await
    .unwrap();
    assert_eq!(rotated.generation, 3);
    assert_eq!(
        load_workspace_secret(&pool, &a, id, 3, &ws, &KEY)
            .await
            .unwrap()
            .unwrap()
            .expose(),
        "rotated-token"
    );
    db_execute!(&pool, "INSERT INTO user_mcp_tool_grants (connection_id, workspace_id, tool_name, revision, schema_hash, generation) VALUES ($1,$2,'read','test-revision','test-hash',3)", id, &ws).unwrap();
    db_execute!(
        &pool,
        "UPDATE workspace_users SET active = FALSE WHERE workspace_id = $1 AND user_id = $2",
        &ws,
        &a
    )
    .unwrap();
    assert!(
        require_workspace_connection(&pool, &a, id, 3, &ws)
            .await
            .is_err()
    );
    assert!(
        set_workspace_enabled(&pool, &a, id, 3, &ws, true)
            .await
            .is_err()
    );
    // The failed enablement transaction preserves the existing grant, not half a delete.
    assert_eq!(
        db_fetch_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_tool_grants WHERE connection_id = $1",
            id
        )
        .unwrap(),
        1
    );
    db_execute!(
        &pool,
        "DELETE FROM workspace_users WHERE workspace_id = $1 AND user_id = $2",
        &ws,
        &a
    )
    .unwrap();
    assert_eq!(
        db_fetch_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_tool_grants WHERE connection_id = $1",
            id
        )
        .unwrap(),
        0
    );
    assert_eq!(get_connection(&pool, &a, id).await.unwrap().generation, 3);
    db_execute!(&pool, "INSERT INTO workspace_users (workspace_id,user_id,role,active) VALUES ($1,$2,'workspace_user',TRUE)", &ws, &a).unwrap();
    assert!(
        require_workspace_connection(&pool, &a, id, 3, &ws)
            .await
            .is_err()
    );
    set_workspace_enabled(&pool, &a, id, 3, &ws, true)
        .await
        .unwrap();
    db_execute!(&pool, "INSERT INTO user_mcp_tool_grants (connection_id, workspace_id, tool_name, revision, schema_hash, generation) VALUES ($1,$2,'read','test-revision','test-hash',3)", id, &ws).unwrap();
    let no_auth = update_connection(
        &pool,
        &a,
        id,
        3,
        input("Renamed", McpAuthKind::None, None),
        &KEY,
    )
    .await
    .unwrap();
    assert_eq!(no_auth.generation, 4);
    assert_eq!(
        db_fetch_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_tool_grants WHERE connection_id = $1",
            id
        )
        .unwrap(),
        0
    );
    db_execute!(&pool, "INSERT INTO user_mcp_tool_grants (connection_id, workspace_id, tool_name, revision, schema_hash, generation) VALUES ($1,$2,'read','test-revision','test-hash',4)", id, &ws).unwrap();
    assert!(
        load_workspace_secret(&pool, &a, id, 4, &ws, &KEY)
            .await
            .unwrap()
            .is_none()
    );
    let disabled = disable_connection(&pool, &a, id, 4).await.unwrap();
    assert_eq!(disabled.status, "disabled");
    assert_eq!(
        db_fetch_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_tool_grants WHERE connection_id = $1",
            id
        )
        .unwrap(),
        0
    );
    assert!(
        require_workspace_connection(&pool, &a, id, 5, &ws)
            .await
            .is_err()
    );
    assert!(disable_connection(&pool, &a, id, 4).await.is_err());
    delete_connection(&pool, &a, id, 5).await.unwrap();
    assert!(get_connection(&pool, &a, id).await.is_err());

    let oauth = create_connection(&pool, &a, input("OAuth", McpAuthKind::Oauth, None), &KEY)
        .await
        .unwrap();
    let oid = &oauth.connection_id;
    let state = "test-state-with-at-least-32-random-bytes";
    let auth = || McpAuthTransaction {
        state,
        issuer: "https://issuer.example.com",
        resource: "https://example.com/mcp",
        redirect_uri: "https://app.example.com/callback",
        payload: "test-pkce-verifier",
        expires_at: chrono::Utc::now().timestamp() + 300,
    };
    store_auth_transaction(&pool, &a, oid, 1, auth(), &KEY)
        .await
        .unwrap();
    let row: (String, String) = kyomi_core::db_fetch_one!(&pool, (String, String), "SELECT state_hash, encrypted_payload FROM user_mcp_auth_transactions WHERE connection_id = $1", oid).unwrap();
    assert_eq!(row.0, state_hash(state));
    assert_ne!(row.0, state);
    assert_ne!(row.1, auth().payload);
    assert_eq!(
        crate::encryption::decrypt(&row.1, &KEY).unwrap(),
        auth().payload
    );
    db_execute!(
        &pool,
        "UPDATE user_mcp_auth_transactions SET expires_at = $1 WHERE connection_id = $2",
        chrono::Utc::now().timestamp() - 1,
        oid
    )
    .unwrap();
    assert!(
        consume_auth_transaction(
            &pool,
            &a,
            oid,
            1,
            McpAuthContext {
                state,
                issuer: auth().issuer,
                resource: auth().resource,
                redirect_uri: auth().redirect_uri
            },
            &KEY
        )
        .await
        .is_err()
    );
    store_auth_transaction(&pool, &a, oid, 1, auth(), &KEY)
        .await
        .unwrap();
    assert!(
        consume_auth_transaction(
            &pool,
            &b,
            oid,
            1,
            McpAuthContext {
                state,
                issuer: auth().issuer,
                resource: auth().resource,
                redirect_uri: auth().redirect_uri
            },
            &KEY
        )
        .await
        .is_err()
    );
    assert!(
        consume_auth_transaction(
            &pool,
            &a,
            oid,
            1,
            McpAuthContext {
                state,
                issuer: "wrong-issuer",
                resource: auth().resource,
                redirect_uri: auth().redirect_uri
            },
            &KEY
        )
        .await
        .is_err()
    );
    let consumed = consume_auth_transaction(
        &pool,
        &a,
        oid,
        1,
        McpAuthContext {
            state,
            issuer: auth().issuer,
            resource: auth().resource,
            redirect_uri: auth().redirect_uri,
        },
        &KEY,
    )
    .await
    .unwrap();
    assert_eq!(consumed.expose(), "test-pkce-verifier");
    assert!(
        consume_auth_transaction(
            &pool,
            &a,
            oid,
            1,
            McpAuthContext {
                state,
                issuer: auth().issuer,
                resource: auth().resource,
                redirect_uri: auth().redirect_uri
            },
            &KEY
        )
        .await
        .is_err()
    );
    let ready = commit_oauth_secret(&pool, &a, oid, 1, "test-oauth-secret", &KEY)
        .await
        .unwrap();
    assert_eq!(ready.generation, 2);
    assert!(
        commit_oauth_secret(&pool, &a, oid, 1, "stale-refresh", &KEY)
            .await
            .is_err()
    );
    store_auth_transaction(&pool, &a, oid, 2, auth(), &KEY)
        .await
        .unwrap();
    disable_connection(&pool, &a, oid, 2).await.unwrap();
    assert!(
        commit_oauth_secret(&pool, &a, oid, 2, "resurrect", &KEY)
            .await
            .is_err()
    );
    assert!(
        consume_auth_transaction(
            &pool,
            &a,
            oid,
            2,
            McpAuthContext {
                state,
                issuer: auth().issuer,
                resource: auth().resource,
                redirect_uri: auth().redirect_uri
            },
            &KEY
        )
        .await
        .is_err()
    );
    assert_eq!(
        db_fetch_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_auth_transactions WHERE connection_id = $1",
            oid
        )
        .unwrap(),
        0
    );

    verify_deleted_owner_cascades(&pool, &a, &b, &ws, &suffix).await;

    // Cleanup existing fixture rows; independent cascade coverage above retains the workspace.
    let owned_by_b = create_connection(
        &pool,
        &b,
        input("Personal account", McpAuthKind::Bearer, Some("b-token")),
        &KEY,
    )
    .await
    .unwrap();
    set_workspace_enabled(&pool, &b, &owned_by_b.connection_id, 1, &ws, true)
        .await
        .unwrap();
    db_execute!(
        &pool,
        "DELETE FROM workspace_users WHERE workspace_id = $1 OR workspace_id = $2",
        &ws,
        &ws2
    )
    .unwrap();
    db_execute!(
        &pool,
        "DELETE FROM workspaces WHERE workspace_id = $1 OR workspace_id = $2",
        &ws,
        &ws2
    )
    .unwrap();
    db_execute!(&pool, "DELETE FROM users WHERE user_id = $1", &a).unwrap();
    assert!(
        list_connections(&pool, &a, "", 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        create_connection(&pool, &a, input("gone", McpAuthKind::None, None), &KEY)
            .await
            .is_err()
    );
    assert_eq!(
        db_fetch_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_connections WHERE owner_user_id = $1",
            &a
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db_fetch_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_secrets WHERE connection_id = $1",
            oid
        )
        .unwrap(),
        0
    );
    assert_eq!(
        get_connection(&pool, &b, &owned_by_b.connection_id)
            .await
            .unwrap()
            .generation,
        1
    );
    db_execute!(&pool, "DELETE FROM users WHERE user_id = $1", &b).unwrap();
    assert_eq!(
        db_fetch_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_secrets WHERE connection_id = $1",
            &owned_by_b.connection_id
        )
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn personal_mcp_sqlite_service_and_migrations() {
    matrix(crate::test_support::test_pool().await).await;
}

#[tokio::test]
async fn personal_mcp_postgres_service_and_migrations() {
    let Some(pool) =
        crate::test_pg::postgres_test_pool_or_skip("personal_mcp_postgres_service_and_migrations")
            .await
    else {
        return;
    };
    matrix(pool).await;
}

#[test]
fn personal_mcp_validation_and_default_off() {
    let cfg = kyomi_core::Config::test_config();
    assert!(!cfg.personal_mcp_enabled);
    assert!(require_feature(&cfg).is_err());
    let mut cfg = cfg;
    cfg.personal_mcp_enabled = true;
    require_feature(&cfg).unwrap();
    for url in [
        "http://example.com/mcp",
        "https://user:password@example.com/mcp",
        "https://example.com/mcp?token=secret",
        "https://example.com/mcp#secret",
    ] {
        assert!(endpoint(url).is_err());
    }
    for token in [
        "token\r\nheader",
        "token\0bad",
        "token\u{7f}bad",
        "********",
        "[REDACTED]",
    ] {
        assert!(validate(&input("token", McpAuthKind::Bearer, Some(token))).is_err());
    }
    let request = r#"{"name":"x","endpoint":"https://example.com","auth_kind":"none","headers":{"Authorization":"secret"}}"#;
    assert!(serde_json::from_str::<SavePersonalMcpConnection>(request).is_err());
}

/// Exercise populated connection, membership, and user deletion as separate
/// lifecycle stages while the workspace and other owners survive.
async fn verify_deleted_owner_cascades(
    pool: &DbPool,
    a: &str,
    b: &str,
    workspace: &str,
    suffix: &str,
) {
    let c = format!("mcp-c-{suffix}");
    db_execute!(
        pool,
        "INSERT INTO users (user_id,email) VALUES ($1,$2)",
        &c,
        format!("{c}@example.com")
    )
    .unwrap();
    db_execute!(pool, "INSERT INTO workspace_users (workspace_id,user_id,role,active) VALUES ($1,$2,'workspace_user',TRUE)", workspace, &c).unwrap();
    let owned = create_connection(
        pool,
        &c,
        input("Delete owner", McpAuthKind::Oauth, None),
        &KEY,
    )
    .await
    .unwrap();
    let id = &owned.connection_id;
    commit_oauth_secret(pool, &c, id, 1, "test-delete-owner-credential", &KEY)
        .await
        .unwrap();
    set_workspace_enabled(pool, &c, id, 2, workspace, true)
        .await
        .unwrap();
    let auth = McpAuthTransaction {
        state: "test-delete-owner-state-longer-than-32",
        issuer: "https://issuer.example.com",
        resource: "https://example.com/mcp",
        redirect_uri: "https://app.example.com/callback",
        payload: "test-delete-owner-verifier",
        expires_at: chrono::Utc::now().timestamp() + 300,
    };
    store_auth_transaction(pool, &c, id, 2, auth, &KEY)
        .await
        .unwrap();
    let rollback = create_connection(
        pool,
        a,
        input("State rollback", McpAuthKind::Oauth, None),
        &KEY,
    )
    .await
    .unwrap();
    let prior_state = "test-prior-state-longer-than-32-bytes";
    let pending = |state| McpAuthTransaction {
        state,
        issuer: "https://issuer.example.com",
        resource: "https://example.com/mcp",
        redirect_uri: "https://app.example.com/callback",
        payload: "test-prior-pkce",
        expires_at: chrono::Utc::now().timestamp() + 300,
    };
    store_auth_transaction(
        pool,
        a,
        &rollback.connection_id,
        1,
        pending(prior_state),
        &KEY,
    )
    .await
    .unwrap();
    // This deletes the old transaction, then hits the unique state hash owned by C.
    // SQL rollback must restore the previous transaction in its entirety.
    assert!(
        store_auth_transaction(
            pool,
            a,
            &rollback.connection_id,
            1,
            pending("test-delete-owner-state-longer-than-32"),
            &KEY
        )
        .await
        .is_err()
    );
    let restored: String = db_fetch_scalar!(
        pool,
        String,
        "SELECT state_hash FROM user_mcp_auth_transactions WHERE connection_id = $1",
        &rollback.connection_id
    )
    .unwrap();
    assert_eq!(restored, state_hash(prior_state));
    delete_connection(pool, a, &rollback.connection_id, 1)
        .await
        .unwrap();

    db_execute!(pool, "INSERT INTO user_mcp_tool_grants (connection_id,workspace_id,tool_name,revision,schema_hash,generation) VALUES ($1,$2,'read','test-revision','test-hash',2)", id, workspace).unwrap();
    let control = create_connection(
        pool,
        b,
        input(
            "Surviving account",
            McpAuthKind::Bearer,
            Some("test-surviving-token"),
        ),
        &KEY,
    )
    .await
    .unwrap();
    let control_id = &control.connection_id;
    set_workspace_enabled(pool, b, control_id, 1, workspace, true)
        .await
        .unwrap();
    db_execute!(pool, "INSERT INTO user_mcp_tool_grants (connection_id,workspace_id,tool_name,revision,schema_hash,generation) VALUES ($1,$2,'read','test-revision','test-hash',1)", control_id, workspace).unwrap();
    for table in [
        "user_mcp_connections",
        "user_mcp_secrets",
        "user_mcp_auth_transactions",
        "user_mcp_workspace_enablements",
        "user_mcp_tool_grants",
    ] {
        let query = format!("SELECT COUNT(*) FROM {table} WHERE connection_id = $1");
        assert_eq!(
            db_fetch_scalar!(pool, i64, &query, id).unwrap(),
            1,
            "seed {table}"
        );
    }
    // Connection deletion must cascade the complete populated group while
    // membership survives; the owner-deletion path below has its own prerequisites.
    let direct = create_connection(
        pool,
        &c,
        input("Direct delete", McpAuthKind::Oauth, None),
        &KEY,
    )
    .await
    .unwrap();
    let direct_id = &direct.connection_id;
    commit_oauth_secret(pool, &c, direct_id, 1, "test-direct-delete-token", &KEY)
        .await
        .unwrap();
    set_workspace_enabled(pool, &c, direct_id, 2, workspace, true)
        .await
        .unwrap();
    store_auth_transaction(
        pool,
        &c,
        direct_id,
        2,
        McpAuthTransaction {
            state: "test-direct-delete-state-at-least-32",
            issuer: "https://issuer.example.com",
            resource: "https://example.com/mcp",
            redirect_uri: "https://app.example.com/callback",
            payload: "test-direct-delete-verifier",
            expires_at: chrono::Utc::now().timestamp() + 300,
        },
        &KEY,
    )
    .await
    .unwrap();
    db_execute!(pool, "INSERT INTO user_mcp_tool_grants (connection_id,workspace_id,tool_name,revision,schema_hash,generation) VALUES ($1,$2,'read','test-revision','test-hash',2)", direct_id, workspace).unwrap();
    for table in [
        "user_mcp_connections",
        "user_mcp_secrets",
        "user_mcp_auth_transactions",
        "user_mcp_workspace_enablements",
        "user_mcp_tool_grants",
    ] {
        let query = format!("SELECT COUNT(*) FROM {table} WHERE connection_id = $1");
        assert_eq!(
            db_fetch_scalar!(pool, i64, &query, direct_id).unwrap(),
            1,
            "direct seed {table}"
        );
    }
    delete_connection(pool, &c, direct_id, 2).await.unwrap();
    for table in [
        "user_mcp_connections",
        "user_mcp_secrets",
        "user_mcp_auth_transactions",
        "user_mcp_workspace_enablements",
        "user_mcp_tool_grants",
    ] {
        let query = format!("SELECT COUNT(*) FROM {table} WHERE connection_id = $1");
        assert_eq!(
            db_fetch_scalar!(pool, i64, &query, direct_id).unwrap(),
            0,
            "direct cascade {table}"
        );
    }
    assert_eq!(
        db_fetch_scalar!(
            pool,
            i64,
            "SELECT COUNT(*) FROM workspace_users WHERE workspace_id = $1 AND user_id = $2",
            workspace,
            &c
        )
        .unwrap(),
        1
    );
    // Existing user deletion requires removing memberships first. Exercise the
    // actual service and assert each stage, retaining the workspace/B controls.
    crate::workspace_service::remove_workspace_member(pool, workspace, a, a, &c, None)
        .await
        .unwrap();
    for table in ["user_mcp_workspace_enablements", "user_mcp_tool_grants"] {
        let query = format!("SELECT COUNT(*) FROM {table} WHERE connection_id = $1");
        assert_eq!(
            db_fetch_scalar!(pool, i64, &query, id).unwrap(),
            0,
            "membership cascade {table}"
        );
    }
    for table in [
        "user_mcp_connections",
        "user_mcp_secrets",
        "user_mcp_auth_transactions",
    ] {
        let query = format!("SELECT COUNT(*) FROM {table} WHERE connection_id = $1");
        assert_eq!(
            db_fetch_scalar!(pool, i64, &query, id).unwrap(),
            1,
            "personal ownership survives leaving workspace {table}"
        );
    }
    assert!(crate::user_service::delete_user(pool, &c).await.unwrap());
    for table in [
        "user_mcp_connections",
        "user_mcp_secrets",
        "user_mcp_auth_transactions",
        "user_mcp_workspace_enablements",
        "user_mcp_tool_grants",
    ] {
        let query = format!("SELECT COUNT(*) FROM {table} WHERE connection_id = $1");
        assert_eq!(
            db_fetch_scalar!(pool, i64, &query, id).unwrap(),
            0,
            "cascade {table}"
        );
    }
    assert_eq!(
        db_fetch_scalar!(
            pool,
            i64,
            "SELECT COUNT(*) FROM workspaces WHERE workspace_id = $1 AND owner_user_id = $2",
            workspace,
            a
        )
        .unwrap(),
        1
    );
    assert!(get_connection(pool, &c, id).await.is_err());
    assert_eq!(
        db_fetch_scalar!(
            pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_tool_grants WHERE connection_id = $1",
            control_id
        )
        .unwrap(),
        1
    );
    assert_eq!(
        load_workspace_secret(pool, b, control_id, 1, workspace, &KEY)
            .await
            .unwrap()
            .unwrap()
            .expose(),
        "test-surviving-token"
    );
    // Two competing refresh commits from one generation have exactly one winner.
    let race = create_connection(pool, a, input("CAS race", McpAuthKind::Oauth, None), &KEY)
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        commit_oauth_secret(pool, a, &race.connection_id, 1, "first-result", &KEY),
        commit_oauth_secret(pool, a, &race.connection_id, 1, "second-result", &KEY)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(
        get_connection(pool, a, &race.connection_id)
            .await
            .unwrap()
            .generation,
        2
    );
    disable_connection(pool, a, &race.connection_id, 2)
        .await
        .unwrap();
    assert!(
        commit_oauth_secret(pool, a, &race.connection_id, 2, "late-result", &KEY)
            .await
            .is_err()
    );
    assert_eq!(
        db_fetch_scalar!(
            pool,
            i64,
            "SELECT COUNT(*) FROM user_mcp_secrets WHERE connection_id = $1",
            &race.connection_id
        )
        .unwrap(),
        0
    );
}
