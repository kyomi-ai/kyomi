// SPDX-License-Identifier: AGPL-3.0-or-later

//! Owner-scoped persistence only. No HTTP, datasource fallback, workspace admin
//! bypass, or credential events. Callers pass the authenticated user, never an
//! owner supplied in a request. Every mutation serializes on the connection row.
use kyomi_core::models::personal_mcp::UserMcpConnection;
use kyomi_core::{DbPool, Error, Result, db_fetch_all, db_fetch_optional, db_with_pool};
use kyomi_types::personal_mcp::{McpAuthKind, PersonalMcpConnection, SavePersonalMcpConnection};
use sha2::{Digest, Sha256};

const COLUMNS: &str = "connection_id, owner_user_id, name, endpoint, auth_kind, status, generation";

fn not_found() -> Error {
    Error::NotFound("personal MCP connection".into())
}
fn conflict() -> Error {
    Error::Conflict("personal MCP connection changed; reload and retry".into())
}

fn dto(row: UserMcpConnection) -> Result<PersonalMcpConnection> {
    let auth_kind = match row.auth_kind.as_str() {
        "none" => McpAuthKind::None,
        "bearer" => McpAuthKind::Bearer,
        "oauth" => McpAuthKind::Oauth,
        _ => {
            return Err(Error::Internal(
                "invalid stored MCP authentication kind".into(),
            ));
        }
    };
    Ok(PersonalMcpConnection {
        connection_id: row.connection_id,
        owner_user_id: row.owner_user_id,
        name: row.name,
        endpoint: row.endpoint,
        auth_kind,
        status: row.status,
        generation: row.generation,
    })
}

/// Canonicalize configuration, without resolving/connecting. The transport must
/// independently enforce deployment egress policy at connection time.
fn endpoint(value: &str) -> Result<String> {
    let mut url = url::Url::parse(value)
        .map_err(|_| Error::BadRequest("invalid MCP HTTPS endpoint".into()))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err(Error::BadRequest(
            "MCP endpoint must be HTTPS without credentials, query or fragment".into(),
        ));
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

fn validate(input: &SavePersonalMcpConnection) -> Result<String> {
    if input.name.trim().is_empty() || input.name.len() > 100 || input.name != input.name.trim() {
        return Err(Error::BadRequest(
            "MCP connection name must be 1–100 bytes without surrounding whitespace".into(),
        ));
    }
    if let Some(secret) = &input.bearer_secret
        && (input.auth_kind != McpAuthKind::Bearer
            || secret.is_empty()
            || secret.len() > 16384
            || secret.chars().any(|c| c.is_whitespace() || c.is_control())
            || secret.chars().all(|c| matches!(c, '*' | '•' | '●'))
            || secret == "[REDACTED]")
    {
        return Err(Error::BadRequest(
            "invalid personal MCP bearer secret".into(),
        ));
    }
    endpoint(&input.endpoint)
}

/// Bounded, stable owner-only pagination. A workspace administrator has no extra authority.
pub async fn list_connections(
    pool: &DbPool,
    actor: &str,
    after_id: &str,
    limit: u32,
) -> Result<Vec<PersonalMcpConnection>> {
    let rows = db_fetch_all!(
        pool,
        UserMcpConnection,
        &format!(
            "SELECT {COLUMNS} FROM user_mcp_connections WHERE owner_user_id = $1 AND connection_id > $2 AND EXISTS (SELECT 1 FROM users WHERE user_id = $1 AND active = TRUE) ORDER BY connection_id LIMIT $3"
        ),
        actor,
        after_id,
        i64::from(limit.clamp(1, 100))
    )?;
    rows.into_iter().map(dto).collect()
}

pub async fn get_connection(pool: &DbPool, actor: &str, id: &str) -> Result<PersonalMcpConnection> {
    let row = db_fetch_optional!(pool, UserMcpConnection,
        &format!("SELECT {COLUMNS} FROM user_mcp_connections WHERE owner_user_id = $1 AND connection_id = $2 AND EXISTS (SELECT 1 FROM users WHERE user_id = $1 AND active = TRUE)"), actor, id)?
        .ok_or_else(not_found)?;
    dto(row)
}

pub async fn create_connection(
    pool: &DbPool,
    actor: &str,
    input: SavePersonalMcpConnection,
    key: &[u8; 32],
) -> Result<PersonalMcpConnection> {
    let canonical = validate(&input)?;
    if input.auth_kind == McpAuthKind::Bearer && input.bearer_secret.is_none() {
        return Err(Error::BadRequest("bearer secret required".into()));
    }
    let encrypted = input
        .bearer_secret
        .as_deref()
        .map(|s| crate::encryption::encrypt(s, key))
        .transpose()?;
    let id = uuid::Uuid::new_v4().to_string();
    let status = if input.auth_kind == McpAuthKind::Oauth {
        "needs_auth"
    } else {
        "ready"
    };
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        let affected = sqlx::query("INSERT INTO user_mcp_connections (connection_id, owner_user_id, name, endpoint, auth_kind, status) SELECT $1, $2, $3, $4, $5, $6 WHERE EXISTS (SELECT 1 FROM users WHERE user_id = $2 AND active = TRUE)")
            .bind(&id).bind(actor).bind(&input.name).bind(&canonical).bind(input.auth_kind.as_str()).bind(status)
            .execute(&mut *tx).await?.rows_affected();
        if affected != 1 {
            return Err(not_found());
        }
        if let Some(secret) = &encrypted {
            sqlx::query(
                "INSERT INTO user_mcp_secrets (connection_id, encrypted_secret) VALUES ($1, $2)",
            )
            .bind(&id)
            .bind(secret)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
    });
    get_connection(pool, actor, &id).await
}

/// Full public configuration update, preserving an omitted bearer only when its
/// auth mode and endpoint remain unchanged. Any change invalidates old grants/state.
pub async fn update_connection(
    pool: &DbPool,
    actor: &str,
    id: &str,
    generation: i64,
    input: SavePersonalMcpConnection,
    key: &[u8; 32],
) -> Result<PersonalMcpConnection> {
    let canonical = validate(&input)?;
    let old = get_connection(pool, actor, id).await?;
    if input.auth_kind == McpAuthKind::Bearer
        && input.bearer_secret.is_none()
        && (old.auth_kind != McpAuthKind::Bearer
            || old.endpoint != canonical
            || old.status == "disabled")
    {
        return Err(Error::BadRequest(
            "bearer secret required for new authentication context".into(),
        ));
    }
    let encrypted = input
        .bearer_secret
        .as_deref()
        .map(|s| crate::encryption::encrypt(s, key))
        .transpose()?;
    let status = if input.auth_kind == McpAuthKind::Oauth {
        "needs_auth"
    } else {
        "ready"
    };
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        let changed = sqlx::query("UPDATE user_mcp_connections SET name = $4, endpoint = $5, auth_kind = $6, status = $7, generation = generation + 1 WHERE owner_user_id = $1 AND connection_id = $2 AND generation = $3 AND EXISTS (SELECT 1 FROM users WHERE user_id = $1 AND active = TRUE)")
            .bind(actor).bind(id).bind(generation).bind(&input.name).bind(&canonical).bind(input.auth_kind.as_str()).bind(status)
            .execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(conflict());
        }
        sqlx::query("DELETE FROM user_mcp_tool_grants WHERE connection_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM user_mcp_auth_transactions WHERE connection_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        // OAuth reconnect and no-auth configuration explicitly discard previous secrets.
        if input.auth_kind != McpAuthKind::Bearer || encrypted.is_some() {
            sqlx::query("DELETE FROM user_mcp_secrets WHERE connection_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(secret) = &encrypted {
            sqlx::query(
                "INSERT INTO user_mcp_secrets (connection_id, encrypted_secret) VALUES ($1, $2)",
            )
            .bind(id)
            .bind(secret)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
    });
    get_connection(pool, actor, id).await
}

/// Immediate disable atomically bumps generation and removes credentials,
/// enablements (cascading grants), and pending authorization transactions.
pub async fn disable_connection(
    pool: &DbPool,
    actor: &str,
    id: &str,
    generation: i64,
) -> Result<PersonalMcpConnection> {
    get_connection(pool, actor, id).await?;
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        let changed = sqlx::query("UPDATE user_mcp_connections SET status = 'disabled', generation = generation + 1 WHERE owner_user_id = $1 AND connection_id = $2 AND generation = $3 AND EXISTS (SELECT 1 FROM users WHERE user_id = $1 AND active = TRUE)")
            .bind(actor).bind(id).bind(generation).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(conflict());
        }
        sqlx::query("DELETE FROM user_mcp_secrets WHERE connection_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM user_mcp_workspace_enablements WHERE connection_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM user_mcp_auth_transactions WHERE connection_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    });
    get_connection(pool, actor, id).await
}

pub async fn delete_connection(
    pool: &DbPool,
    actor: &str,
    id: &str,
    generation: i64,
) -> Result<()> {
    get_connection(pool, actor, id).await?;
    let affected = kyomi_core::db_execute!(pool,
        "DELETE FROM user_mcp_connections WHERE owner_user_id = $1 AND connection_id = $2 AND generation = $3 AND EXISTS (SELECT 1 FROM users WHERE user_id = $1 AND active = TRUE)", actor, id, generation)?.rows_affected();
    if affected != 1 {
        return Err(conflict());
    }
    Ok(())
}

/// Current membership is checked in the same write transaction. Membership
/// deletion cascades enablements/grants; inactive memberships never authorize use.
pub async fn set_workspace_enabled(
    pool: &DbPool,
    actor: &str,
    id: &str,
    generation: i64,
    workspace: &str,
    enabled: bool,
) -> Result<()> {
    get_connection(pool, actor, id).await?;
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        let changed = sqlx::query("UPDATE user_mcp_connections SET generation = generation WHERE owner_user_id = $1 AND connection_id = $2 AND generation = $3 AND status = 'ready' AND EXISTS (SELECT 1 FROM users WHERE user_id = $1 AND active = TRUE)")
            .bind(actor).bind(id).bind(generation).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(conflict());
        }
        if !enabled {
            sqlx::query("DELETE FROM user_mcp_workspace_enablements WHERE connection_id = $1 AND workspace_id = $2")
                .bind(id).bind(workspace).execute(&mut *tx).await?;
        } else {
            let changed = sqlx::query("INSERT INTO user_mcp_workspace_enablements (connection_id, owner_user_id, workspace_id) SELECT $1, $2, $3 WHERE EXISTS (SELECT 1 FROM workspace_users WHERE workspace_id = $3 AND user_id = $2 AND active = TRUE) ON CONFLICT (connection_id, workspace_id) DO NOTHING")
                .bind(id).bind(actor).bind(workspace).execute(&mut *tx).await?.rows_affected();
            if changed == 0 {
                let member: Option<(String,)> = sqlx::query_as("SELECT user_id FROM workspace_users WHERE workspace_id = $1 AND user_id = $2 AND active = TRUE")
                    .bind(workspace).bind(actor).fetch_optional(&mut *tx).await?;
                if member.is_none() {
                    return Err(Error::Forbidden(
                        "active workspace membership required".into(),
                    ));
                }
            }
        }
        tx.commit().await?;
    });
    Ok(())
}

/// Owner-only readiness check for a future test/discovery/execution request.
/// The caller still must enforce the deployment kill switch before any network.
pub async fn require_workspace_connection(
    pool: &DbPool,
    actor: &str,
    id: &str,
    generation: i64,
    workspace: &str,
) -> Result<PersonalMcpConnection> {
    let connection = get_connection(pool, actor, id).await?;
    let enabled: Option<(String,)> = db_fetch_optional!(
        pool,
        (String,),
        "SELECT e.connection_id FROM user_mcp_workspace_enablements e JOIN workspace_users m ON m.workspace_id = e.workspace_id AND m.user_id = e.owner_user_id JOIN user_mcp_connections c ON c.connection_id = e.connection_id WHERE e.owner_user_id = $1 AND e.connection_id = $2 AND e.workspace_id = $3 AND m.active = TRUE AND c.status = 'ready' AND c.generation = $4",
        actor,
        id,
        workspace,
        generation
    )?;
    if enabled.is_none() || connection.generation != generation {
        return Err(Error::Forbidden(
            "personal MCP connection is not enabled here".into(),
        ));
    }
    Ok(connection)
}

/// Secret material for trusted network adapters only; never serialize or log it.
/// API DTOs deliberately cannot carry this type. No arbitrary headers are accepted.
pub struct McpSecret(String);
impl std::fmt::Debug for McpSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpSecret([REDACTED])")
    }
}
impl McpSecret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Read secrets only after current owner/workspace/generation authorization.
/// This is point-in-time admission, not a network dispatch lease. Lifecycle adapters
/// must serialize revocation with dispatch separately; no DB lock spans HTTP.
pub async fn load_workspace_secret(
    pool: &DbPool,
    actor: &str,
    id: &str,
    generation: i64,
    workspace: &str,
    key: &[u8; 32],
) -> Result<Option<McpSecret>> {
    require_workspace_connection(pool, actor, id, generation, workspace).await?;
    let row: Option<(String, Option<String>)> = db_fetch_optional!(
        pool,
        (String, Option<String>),
        "SELECT c.auth_kind, s.encrypted_secret FROM user_mcp_connections c LEFT JOIN user_mcp_secrets s ON s.connection_id = c.connection_id JOIN users u ON u.user_id = c.owner_user_id JOIN user_mcp_workspace_enablements e ON e.connection_id = c.connection_id AND e.owner_user_id = c.owner_user_id JOIN workspace_users m ON m.workspace_id = e.workspace_id AND m.user_id = e.owner_user_id WHERE c.owner_user_id = $1 AND c.connection_id = $2 AND c.generation = $3 AND c.status = 'ready' AND e.workspace_id = $4 AND u.active = TRUE AND m.active = TRUE",
        actor,
        id,
        generation,
        workspace
    )?;
    let (kind, encrypted) =
        row.ok_or_else(|| Error::Forbidden("personal MCP authorization changed".into()))?;
    if kind == "none" {
        return Ok(None);
    }
    let encrypted = encrypted
        .ok_or_else(|| Error::Conflict("personal MCP credentials are unavailable".into()))?;
    crate::encryption::decrypt(&encrypted, key)
        .map(McpSecret)
        .map(Some)
}

/// OAuth adapters commit results after HTTP completes. A disconnect or competing
/// refresh winning CAS makes this fail; old credentials can never resurrect it.
/// The opaque JSON is encrypted using existing primitives and has no Debug path.
pub async fn commit_oauth_secret(
    pool: &DbPool,
    actor: &str,
    id: &str,
    generation: i64,
    secret: &str,
    key: &[u8; 32],
) -> Result<PersonalMcpConnection> {
    get_connection(pool, actor, id).await?;
    if secret.is_empty() || secret.len() > 65536 {
        return Err(Error::BadRequest(
            "invalid MCP OAuth credential payload".into(),
        ));
    }
    let encrypted = crate::encryption::encrypt(secret, key)?;
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        let changed = sqlx::query("UPDATE user_mcp_connections SET status = 'ready', generation = generation + 1 WHERE owner_user_id = $1 AND connection_id = $2 AND generation = $3 AND auth_kind = 'oauth' AND status <> 'disabled' AND EXISTS (SELECT 1 FROM users WHERE user_id = $1 AND active = TRUE)")
            .bind(actor).bind(id).bind(generation).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(conflict());
        }
        sqlx::query("INSERT INTO user_mcp_secrets (connection_id, encrypted_secret) VALUES ($1, $2) ON CONFLICT (connection_id) DO UPDATE SET encrypted_secret = excluded.encrypted_secret")
            .bind(id).bind(&encrypted).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM user_mcp_auth_transactions WHERE connection_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM user_mcp_tool_grants WHERE connection_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    });
    get_connection(pool, actor, id).await
}

/// Deployment kill switch: every outbound discovery and invocation adapter calls
/// this before constructing or using its network client.
pub fn require_feature(config: &kyomi_core::Config) -> Result<()> {
    if !config.personal_mcp_enabled {
        return Err(Error::Forbidden("personal external MCP is disabled".into()));
    }
    Ok(())
}

/// Bound authorization context; PKCE verifier and dynamic client credentials live
/// only in the encrypted payload. State is stored hashed and consumed once.
pub struct McpAuthTransaction<'a> {
    pub state: &'a str,
    pub issuer: &'a str,
    pub resource: &'a str,
    pub redirect_uri: &'a str,
    pub payload: &'a str,
    pub expires_at: i64,
}

fn state_hash(state: &str) -> String {
    hex::encode(Sha256::digest(state.as_bytes()))
}

pub async fn store_auth_transaction(
    pool: &DbPool,
    actor: &str,
    id: &str,
    generation: i64,
    auth: McpAuthTransaction<'_>,
    key: &[u8; 32],
) -> Result<()> {
    let connection = get_connection(pool, actor, id).await?;
    if auth.state.len() < 32
        || auth.state.len() > 1024
        || auth.payload.is_empty()
        || auth.payload.len() > 65536
        || [auth.issuer, auth.resource, auth.redirect_uri]
            .iter()
            .any(|s| s.is_empty() || s.len() > 4096)
        || auth.expires_at <= chrono::Utc::now().timestamp()
        || auth.expires_at > chrono::Utc::now().timestamp() + 1800
    {
        return Err(Error::BadRequest(
            "invalid MCP authorization transaction lifetime or state".into(),
        ));
    }
    let encrypted = crate::encryption::encrypt(auth.payload, key)?;
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        let changed = sqlx::query("UPDATE user_mcp_connections SET generation = generation WHERE owner_user_id = $1 AND connection_id = $2 AND generation = $3 AND auth_kind = 'oauth' AND status <> 'disabled' AND EXISTS (SELECT 1 FROM users WHERE user_id = $1 AND active = TRUE)")
            .bind(actor).bind(id).bind(generation).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(conflict());
        }
        sqlx::query("DELETE FROM user_mcp_auth_transactions WHERE connection_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO user_mcp_auth_transactions (state_hash, connection_id, generation, endpoint, issuer, resource, redirect_uri, encrypted_payload, expires_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(state_hash(auth.state)).bind(id).bind(generation).bind(&connection.endpoint)
            .bind(auth.issuer).bind(auth.resource).bind(auth.redirect_uri).bind(&encrypted).bind(auth.expires_at)
            .execute(&mut *tx).await?;
        tx.commit().await?;
    });
    Ok(())
}

/// Atomically consume a state only when its complete authorization context still
/// matches. Wrong owner, expired state, changed endpoint/generation, or replay fails.
pub struct McpAuthContext<'a> {
    pub state: &'a str,
    pub issuer: &'a str,
    pub resource: &'a str,
    pub redirect_uri: &'a str,
}

pub async fn consume_auth_transaction(
    pool: &DbPool,
    actor: &str,
    id: &str,
    generation: i64,
    context: McpAuthContext<'_>,
    key: &[u8; 32],
) -> Result<McpSecret> {
    let connection = get_connection(pool, actor, id).await?;
    let row: Option<(String,)> = db_with_pool!(pool, |p| {
        sqlx::query_as("DELETE FROM user_mcp_auth_transactions WHERE state_hash = $1 AND connection_id = $2 AND generation = $3 AND endpoint = $4 AND issuer = $5 AND resource = $6 AND redirect_uri = $7 AND expires_at > $8 AND EXISTS (SELECT 1 FROM user_mcp_connections c JOIN users u ON u.user_id = c.owner_user_id WHERE c.connection_id = $2 AND c.owner_user_id = $9 AND c.generation = $3 AND c.endpoint = $4 AND c.status <> 'disabled' AND u.active = TRUE) RETURNING encrypted_payload")
            .bind(state_hash(context.state)).bind(id).bind(generation).bind(&connection.endpoint)
            .bind(context.issuer).bind(context.resource).bind(context.redirect_uri).bind(chrono::Utc::now().timestamp()).bind(actor)
            .fetch_optional(p).await?
    });
    crate::encryption::decrypt(
        &row.ok_or_else(|| Error::Forbidden("invalid or expired MCP authorization state".into()))?
            .0,
        key,
    )
    .map(McpSecret)
}

#[cfg(test)]
mod tests;
