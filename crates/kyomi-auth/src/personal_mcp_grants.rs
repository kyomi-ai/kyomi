// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bounded discovery and durable exact grants. Never registers or invokes tools.
//! HTTP happens outside DB transactions; connection/catalog CAS rejects stale work.
use crate::personal_mcp_service::{self as connections, McpSecret};
use kyomi_core::{Config, DbPool, Error, Result, db_fetch_optional, db_with_pool};
use kyomi_mcp_client::{Limits, McpClient, Revision, RpcOutcome, SafeHttpClient};
use kyomi_types::personal_mcp::{McpAuthKind, PersonalMcpConnection};
use kyomi_types::personal_mcp_tools::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

/// Trusted identity from the authenticated request, never from tool/model arguments.
#[derive(Clone, Copy)]
pub struct McpScope<'a> {
    pub actor: &'a str,
    pub connection_id: &'a str,
    pub workspace_id: &'a str,
    pub generation: i64,
}

/// Trusted credential-adapter output. Never logged, serialized or returned to UI.
/// OAuth's adapter validates its token's endpoint/resource/expiry before returning.
pub struct DiscoveryCredentials {
    pub authorization: Option<String>,
    pub account_revision: String,
}
pub type CredentialResolver =
    fn(&PersonalMcpConnection, Option<&McpSecret>) -> Result<DiscoveryCredentials>;

/// Supplied tokens/no-auth have no hidden account state; configuration changes
/// increment the persisted generation. OAuth uses its separate trusted adapter.
pub fn personal_credentials(
    connection: &PersonalMcpConnection,
    secret: Option<&McpSecret>,
) -> Result<DiscoveryCredentials> {
    let authorization = match connection.auth_kind {
        McpAuthKind::None => None,
        McpAuthKind::Bearer => Some(format!(
            "Bearer {}",
            secret
                .ok_or_else(|| Error::Conflict("MCP credentials unavailable".into()))?
                .expose()
        )),
        McpAuthKind::Oauth => {
            return Err(Error::Conflict(
                "OAuth credential adapter required; reconnect or refresh the account".into(),
            ));
        }
    };
    Ok(DiscoveryCredentials {
        authorization,
        account_revision: connection.generation.to_string(),
    })
}
fn invalid(message: &str) -> Error {
    Error::BadRequest(message.into())
}
fn stale() -> Error {
    Error::Conflict(
        "MCP catalog or grant changed; refresh tools and enable the exact revision again".into(),
    )
}
fn remote_error() -> Error {
    Error::BadRequest("MCP discovery failed; check connection authentication, supported schemas and catalog limits".into())
}
fn digest(value: &Value) -> String {
    hex::encode(Sha256::digest(value.to_string().as_bytes()))
}

/// Fixed opaque identity uses only provider-safe ASCII (60 bytes). Distinct owners
/// and connections never share aliases, even when endpoint and remote name match.
pub fn tool_alias(owner: &str, connection: &str, remote_name: &str) -> String {
    format!(
        "mcp_{}",
        &digest(&json!([owner, connection, remote_name]))[..56]
    )
}

fn build_catalog(
    scope: McpScope<'_>,
    revision: Revision,
    account: &str,
    server: &Value,
    values: Vec<Value>,
    now: i64,
) -> Result<PersonalMcpCatalog> {
    if account.is_empty() || account.len() > 1024 || values.len() > MCP_MAX_CATALOG_TOOLS {
        return Err(invalid(
            "MCP catalog exceeds limits; choose a server with a smaller tool catalog",
        ));
    }
    let mut names = BTreeSet::new();
    let mut aliases = BTreeSet::new();
    let mut bytes = 0usize;
    let mut tools = Vec::new();
    for value in values {
        bytes = bytes.saturating_add(value.to_string().len());
        if bytes > 1024 * 1024 {
            return Err(invalid(
                "MCP catalog exceeds 1 MiB; reduce the server catalog and refresh",
            ));
        }
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty() && n.len() <= 256 && !n.chars().any(char::is_control))
            .ok_or_else(|| invalid("invalid external tool name"))?;
        if !names.insert(name.to_owned()) {
            return Err(invalid(
                "duplicate external tool names; fix the server catalog",
            ));
        }
        let description = match value.get("description") {
            None => "",
            Some(Value::String(s)) if s.len() <= 2048 => s,
            _ => return Err(invalid("external tool description exceeds 2048 bytes")),
        };
        let schema = value
            .get("inputSchema")
            .ok_or_else(|| invalid("external tool input schema required"))?;
        if schema.to_string().len() > 16384 {
            return Err(invalid(
                "external tool schema exceeds 16 KiB; simplify it before granting",
            ));
        }
        validate_mcp_schema(schema).map_err(invalid)?;
        if let Some(output) = value.get("outputSchema") {
            if output.to_string().len() > 16384 {
                return Err(invalid("external output schema exceeds 16 KiB"));
            }
            validate_mcp_schema(output).map_err(invalid)?;
        }
        if value
            .get("execution")
            .and_then(|v| v.get("taskSupport"))
            .and_then(Value::as_str)
            .is_some_and(|v| v != "forbidden")
        {
            return Err(invalid("task-augmented external tools are unsupported"));
        }
        let alias = tool_alias(scope.actor, scope.connection_id, name);
        if !aliases.insert(alias.clone()) {
            return Err(invalid("external tool identity collision"));
        }
        // Conservative: any definition/server/account/protocol change renews consent.
        // Nothing in annotations classifies a tool as safe or grants it authority.
        let hash = digest(&json!([
            scope.actor,
            scope.connection_id,
            scope.generation,
            revision.as_str(),
            account,
            server,
            value
        ]));
        tools.push(PersonalMcpTool {
            alias,
            remote_name: name.into(),
            description: description.into(),
            input_schema: schema.clone(),
            annotations: value.get("annotations").cloned().unwrap_or(Value::Null),
            revision: hash,
        });
    }
    tools.sort_by(|a, b| a.alias.cmp(&b.alias));
    Ok(PersonalMcpCatalog {
        connection_id: scope.connection_id.into(),
        generation: scope.generation,
        protocol_revision: revision.as_str().into(),
        catalog_id: uuid::Uuid::new_v4().to_string(),
        refreshed_at: now,
        expires_at: now + MCP_CATALOG_TTL_SECONDS,
        tools,
        rejected_tools: Vec::new(),
    })
}

// Locking the connection plus live membership/enablement serializes CRUD with
// disconnect, membership removal and workspace disable. Never holds across HTTP.
macro_rules! lock_scope {
    ($tx:expr, $scope:expr) => {{
        let s = $scope;
        let changed = sqlx::query("UPDATE user_mcp_connections SET generation = generation WHERE connection_id = $1 AND owner_user_id = $2 AND generation = $3 AND status = 'ready' AND EXISTS (SELECT 1 FROM users WHERE user_id = $2 AND active = TRUE)")
            .bind(s.connection_id).bind(s.actor).bind(s.generation).execute(&mut *$tx).await?.rows_affected();
        if changed != 1 { return Err(stale()); }
        let member = sqlx::query("UPDATE workspace_users SET active = active WHERE workspace_id = $1 AND user_id = $2 AND active = TRUE")
            .bind(s.workspace_id).bind(s.actor).execute(&mut *$tx).await?.rows_affected();
        let enabled = sqlx::query("UPDATE user_mcp_workspace_enablements SET owner_user_id = owner_user_id WHERE connection_id = $1 AND workspace_id = $2 AND owner_user_id = $3")
            .bind(s.connection_id).bind(s.workspace_id).bind(s.actor).execute(&mut *$tx).await?.rows_affected();
        if member != 1 || enabled != 1 { return Err(Error::Forbidden("personal MCP connection is not enabled here".into())); }
    }};
}

/// Supported revision is explicit and part of the grant hash. All actual HTTP
/// uses the foundation's SafeHttpClient, including initialize and tools/list.
/// The credential resolver is trusted server wiring, never a request parameter.
pub async fn refresh_discovery(
    config: &Config,
    pool: &DbPool,
    scope: McpScope<'_>,
    transport: Arc<SafeHttpClient>,
    revision: Revision,
    key: &[u8; 32],
    resolver: CredentialResolver,
) -> Result<PersonalMcpCatalog> {
    refresh_discovery_inner(config, pool, scope, transport, revision, key, resolver).await
}

// Only the public SafeHttpClient entry point can reach this in production.
// The private transport seam lets deterministic tests prove authorization before I/O.
async fn refresh_discovery_inner(
    config: &Config,
    pool: &DbPool,
    scope: McpScope<'_>,
    transport: Arc<dyn kyomi_mcp_client::HttpTransport>,
    revision: Revision,
    key: &[u8; 32],
    resolver: CredentialResolver,
) -> Result<PersonalMcpCatalog> {
    connections::require_feature(config)?;
    let connection = connections::require_workspace_connection(
        pool,
        scope.actor,
        scope.connection_id,
        scope.generation,
        scope.workspace_id,
    )
    .await?;
    let secret = connections::load_workspace_secret(
        pool,
        scope.actor,
        scope.connection_id,
        scope.generation,
        scope.workspace_id,
        key,
    )
    .await?;
    let credentials = resolver(&connection, secret.as_ref())?;
    let refresh_id = begin_refresh(config, pool, scope).await?;
    let client = McpClient::new(
        transport,
        connection.endpoint,
        credentials.authorization,
        revision,
        Limits {
            tools: MCP_MAX_CATALOG_TOOLS,
            catalog_bytes: 1024 * 1024,
            schema_bytes: 16384,
            ..Limits::default()
        },
    )
    .map_err(|_| remote_error())?;
    // Always attempt bounded session teardown, including initialize/decode/list
    // failures. A cleanup failure cannot hide the original discovery error.
    let discovery = async {
        let server = match client.initialize().await.map_err(|_| remote_error())? {
            RpcOutcome::Result(v) => v,
            RpcOutcome::RemoteError { .. } => return Err(remote_error()),
        };
        let discovered = client
            .list_tools_catalog()
            .await
            .map_err(|_| remote_error())?;
        let mut catalog = build_catalog(
            scope,
            revision,
            &credentials.account_revision,
            &server,
            discovered.tools,
            chrono::Utc::now().timestamp(),
        )?;
        catalog.rejected_tools = discovered
            .rejected
            .into_iter()
            .map(|tool| RejectedPersonalMcpTool {
                remote_name: tool.name,
                reason: tool.reason.as_str().into(),
            })
            .collect();
        Ok(catalog)
    }
    .await;
    let cleanup = client.close().await;
    let mut catalog = match discovery {
        Err(primary) => return Err(primary),
        Ok(catalog) => {
            cleanup.map_err(|_| {
                Error::BadRequest("MCP discovery session cleanup failed; retry discovery".into())
            })?;
            catalog
        }
    };
    catalog.catalog_id = refresh_id;
    save_catalog(config, pool, scope, &catalog).await?;
    Ok(catalog)
}

/// Invalidate before HTTP. A failed refresh remains visibly unavailable; overlapping
/// refreshes use a persisted token so late old responses cannot replace newer work.
async fn begin_refresh(config: &Config, pool: &DbPool, scope: McpScope<'_>) -> Result<String> {
    connections::require_feature(config)?;
    let token = uuid::Uuid::new_v4().to_string();
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        lock_scope!(tx, scope);
        sqlx::query("INSERT INTO user_mcp_tool_catalogs (connection_id,workspace_id,generation,catalog_id,expires_at,catalog_json) VALUES ($1,$2,$3,$4,0,'') ON CONFLICT (connection_id,workspace_id) DO UPDATE SET generation = excluded.generation, catalog_id = excluded.catalog_id, expires_at = 0, catalog_json = ''")
            .bind(scope.connection_id).bind(scope.workspace_id).bind(scope.generation).bind(&token).execute(&mut *tx).await?;
        tx.commit().await?;
    });
    Ok(token)
}

async fn save_catalog(
    config: &Config,
    pool: &DbPool,
    scope: McpScope<'_>,
    catalog: &PersonalMcpCatalog,
) -> Result<()> {
    connections::require_feature(config)?;
    let serialized =
        serde_json::to_string(catalog).map_err(|_| invalid("invalid external catalog"))?;
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        lock_scope!(tx, scope);
        let current: Option<(String,)> = sqlx::query_as("SELECT catalog_id FROM user_mcp_tool_catalogs WHERE connection_id = $1 AND workspace_id = $2 AND generation = $3 AND catalog_id = $4")
            .bind(scope.connection_id).bind(scope.workspace_id).bind(scope.generation).bind(&catalog.catalog_id).fetch_optional(&mut *tx).await?;
        if current.is_none() {
            return Err(stale());
        }
        // Keep unchanged exact grants across refresh, but never new/changed tools.
        let grants: Vec<(String,String)> = sqlx::query_as("SELECT tool_name, schema_hash FROM user_mcp_tool_grants WHERE connection_id = $1 AND workspace_id = $2")
            .bind(scope.connection_id).bind(scope.workspace_id).fetch_all(&mut *tx).await?;
        for (name, hash) in grants {
            if catalog
                .tools
                .iter()
                .any(|t| t.remote_name == name && t.revision == hash)
            {
                sqlx::query("UPDATE user_mcp_tool_grants SET catalog_id = $3 WHERE connection_id = $1 AND workspace_id = $2 AND tool_name = $4")
                    .bind(scope.connection_id).bind(scope.workspace_id).bind(&catalog.catalog_id).bind(name).execute(&mut *tx).await?;
            } else {
                sqlx::query("DELETE FROM user_mcp_tool_grants WHERE connection_id = $1 AND workspace_id = $2 AND tool_name = $3")
                    .bind(scope.connection_id).bind(scope.workspace_id).bind(name).execute(&mut *tx).await?;
            }
        }
        let changed = sqlx::query("UPDATE user_mcp_tool_catalogs SET expires_at = $5, catalog_json = $6 WHERE connection_id = $1 AND workspace_id = $2 AND generation = $3 AND catalog_id = $4")
            .bind(scope.connection_id).bind(scope.workspace_id).bind(scope.generation).bind(&catalog.catalog_id).bind(catalog.expires_at).bind(&serialized).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(stale());
        }
        tx.commit().await?;
    });
    Ok(())
}

/// Missing, failed and expired catalogs are errors, never an empty grant list.
pub async fn get_catalog(
    config: &Config,
    pool: &DbPool,
    scope: McpScope<'_>,
) -> Result<PersonalMcpCatalog> {
    connections::require_feature(config)?;
    connections::require_workspace_connection(
        pool,
        scope.actor,
        scope.connection_id,
        scope.generation,
        scope.workspace_id,
    )
    .await?;
    let row: Option<(String,)> = db_fetch_optional!(
        pool,
        (String,),
        "SELECT t.catalog_json FROM user_mcp_tool_catalogs t JOIN user_mcp_connections c ON c.connection_id = t.connection_id JOIN users u ON u.user_id = c.owner_user_id JOIN user_mcp_workspace_enablements e ON e.connection_id = t.connection_id AND e.workspace_id = t.workspace_id JOIN workspace_users m ON m.user_id = e.owner_user_id AND m.workspace_id = e.workspace_id WHERE t.connection_id = $1 AND t.workspace_id = $2 AND c.owner_user_id = $3 AND c.generation = $4 AND t.generation = $4 AND c.status = 'ready' AND u.active = TRUE AND m.active = TRUE AND t.expires_at > $5",
        scope.connection_id,
        scope.workspace_id,
        scope.actor,
        scope.generation,
        chrono::Utc::now().timestamp()
    )?;
    serde_json::from_str(&row.ok_or_else(stale)?.0)
        .map_err(|_| Error::Internal("invalid stored MCP catalog".into()))
}

/// Durable automation requires explicit consent for every tool; annotations never
/// earn a weaker path. Destination consent binds the exact connection/revision.
pub async fn grant_tool(
    config: &Config,
    pool: &DbPool,
    scope: McpScope<'_>,
    request: GrantPersonalMcpTool,
) -> Result<()> {
    connections::require_feature(config)?;
    if !request.allow_automation || !request.acknowledge_data_destination {
        return Err(invalid(MCP_DATA_DISCLOSURE));
    }
    let catalog = get_catalog(config, pool, scope).await?;
    let tool = catalog
        .tools
        .iter()
        .find(|t| t.alias == request.alias && t.revision == request.revision)
        .ok_or_else(stale)?;
    if catalog.catalog_id != request.catalog_id {
        return Err(stale());
    }
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        lock_scope!(tx, scope);
        let changed = sqlx::query("INSERT INTO user_mcp_tool_grants (connection_id,workspace_id,tool_name,revision,schema_hash,generation,allow_automation,catalog_id,destination_consent) SELECT $1,$2,$3,$4,$5,$6,TRUE,$7,TRUE WHERE EXISTS (SELECT 1 FROM user_mcp_tool_catalogs WHERE connection_id = $1 AND workspace_id = $2 AND generation = $6 AND catalog_id = $7 AND expires_at > $8) ON CONFLICT (connection_id,workspace_id,tool_name) DO UPDATE SET revision = excluded.revision, schema_hash = excluded.schema_hash, generation = excluded.generation, allow_automation = TRUE, catalog_id = excluded.catalog_id, destination_consent = TRUE")
            .bind(scope.connection_id).bind(scope.workspace_id).bind(&tool.remote_name).bind(&catalog.protocol_revision).bind(&tool.revision).bind(scope.generation).bind(&catalog.catalog_id).bind(chrono::Utc::now().timestamp()).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(stale());
        }
        tx.commit().await?;
    });
    Ok(())
}

pub async fn revoke_tool(
    config: &Config,
    pool: &DbPool,
    scope: McpScope<'_>,
    alias: &str,
) -> Result<()> {
    connections::require_feature(config)?;
    // Permit revocation even after expiry; names are always derived server-side.
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        lock_scope!(tx, scope);
        let names: Vec<(String,)> = sqlx::query_as("SELECT tool_name FROM user_mcp_tool_grants WHERE connection_id = $1 AND workspace_id = $2")
            .bind(scope.connection_id).bind(scope.workspace_id).fetch_all(&mut *tx).await?;
        for (name,) in names {
            if tool_alias(scope.actor, scope.connection_id, &name) == alias {
                sqlx::query("DELETE FROM user_mcp_tool_grants WHERE connection_id = $1 AND workspace_id = $2 AND tool_name = $3")
                    .bind(scope.connection_id).bind(scope.workspace_id).bind(name).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
    });
    Ok(())
}

/// Immutable server-produced run selection. There is no public constructor or
/// deserializer: model/tool arguments cannot widen its aliases or scope. Retain
/// this object for the run and pass it to every invocation admission.
pub struct McpRunSelection {
    actor: String,
    connection_id: String,
    workspace_id: String,
    generation: i64,
    tools: Vec<AuthorizedPersonalMcpTool>,
}
impl McpRunSelection {
    pub fn tools(&self) -> &[AuthorizedPersonalMcpTool] {
        &self.tools
    }
    pub fn len(&self) -> usize {
        self.tools.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// Invocation must intersect the immutable run selection as well as current
/// durable grants. Re-selection after an intentional catalog refresh is explicit.
pub async fn require_tool_grant(
    config: &Config,
    pool: &DbPool,
    scope: McpScope<'_>,
    selection: &McpRunSelection,
    alias: &str,
    revision: &str,
) -> Result<AuthorizedPersonalMcpTool> {
    connections::require_feature(config)?;
    if selection.actor != scope.actor
        || selection.connection_id != scope.connection_id
        || selection.workspace_id != scope.workspace_id
        || selection.generation != scope.generation
    {
        return Err(Error::Forbidden(
            "external tool run selection belongs to another identity or workspace".into(),
        ));
    }
    let selected = selection
        .tools
        .iter()
        .find(|t| t.tool.alias == alias && t.tool.revision == revision)
        .ok_or_else(|| {
            Error::Forbidden("external tool is excluded from this run selection".into())
        })?;
    let current = check_current_grant(config, pool, scope, alias, revision).await?;
    if current.catalog_id != selected.catalog_id {
        return Err(stale());
    }
    Ok(current)
}

/// Recheck on EVERY invocation, including already-advertised tools. The returned
/// admission is point-in-time; runtime must also enforce disconnect/dispatch and
/// private-output serialization before network dispatch (KYO-931/933/934).
async fn check_current_grant(
    config: &Config,
    pool: &DbPool,
    scope: McpScope<'_>,
    alias: &str,
    revision: &str,
) -> Result<AuthorizedPersonalMcpTool> {
    let catalog = get_catalog(config, pool, scope).await?;
    let tool = catalog
        .tools
        .iter()
        .find(|t| t.alias == alias && t.revision == revision)
        .ok_or_else(stale)?
        .clone();
    db_with_pool!(pool, |p| {
        let mut tx = p.begin().await?;
        lock_scope!(tx, scope);
        let grant: Option<(String,)> = sqlx::query_as("SELECT g.tool_name FROM user_mcp_tool_grants g JOIN user_mcp_tool_catalogs t ON t.connection_id = g.connection_id AND t.workspace_id = g.workspace_id AND t.catalog_id = g.catalog_id WHERE g.connection_id = $1 AND g.workspace_id = $2 AND g.tool_name = $3 AND g.schema_hash = $4 AND g.generation = $5 AND g.revision = $6 AND g.catalog_id = $7 AND g.allow_automation = TRUE AND g.destination_consent = TRUE AND t.expires_at > $8")
            .bind(scope.connection_id).bind(scope.workspace_id).bind(&tool.remote_name).bind(revision).bind(scope.generation).bind(&catalog.protocol_revision).bind(&catalog.catalog_id).bind(chrono::Utc::now().timestamp()).fetch_optional(&mut *tx).await?;
        if grant.is_none() {
            return Err(Error::Forbidden("external tool is disabled or its exact grant was revoked; enable it in personal connection settings".into()));
        }
        tx.commit().await?;
    });
    Ok(AuthorizedPersonalMcpTool {
        connection_id: scope.connection_id.into(),
        generation: scope.generation,
        catalog_id: catalog.catalog_id,
        tool,
    })
}

/// Explicit run subsets always intersect grants. Over-budget selection fails with
/// actionable guidance, rather than truncating tools or widening a caller's list.
pub async fn select_granted_tools(
    config: &Config,
    pool: &DbPool,
    scope: McpScope<'_>,
    tools_subset: Option<&[String]>,
    max_advertisement_bytes: usize,
) -> Result<McpRunSelection> {
    let catalog = get_catalog(config, pool, scope).await?;
    let grants: Vec<(String, String)> = kyomi_core::db_fetch_all!(
        pool,
        (String, String),
        "SELECT tool_name, schema_hash FROM user_mcp_tool_grants WHERE connection_id = $1 AND workspace_id = $2 AND catalog_id = $3 AND generation = $4 AND allow_automation = TRUE AND destination_consent = TRUE",
        scope.connection_id,
        scope.workspace_id,
        &catalog.catalog_id,
        scope.generation
    )?;
    let mut selected = Vec::new();
    let mut bytes = 0usize;
    for tool in &catalog.tools {
        if tools_subset.is_some_and(|names| !names.contains(&tool.alias))
            || !grants
                .iter()
                .any(|(name, hash)| name == &tool.remote_name && hash == &tool.revision)
        {
            continue;
        }
        let authorized =
            check_current_grant(config, pool, scope, &tool.alias, &tool.revision).await?;
        // Byte count is a conservative token upper bound. Include provider envelope
        // and untrusted-metadata label overhead rather than counting raw schema alone.
        bytes = bytes.saturating_add(
            serde_json::to_vec(tool)
                .map_err(|_| invalid("invalid tool definition"))?
                .len()
                .saturating_add(512),
        );
        if selected.len() >= MCP_MAX_SELECTED_TOOLS
            || bytes > max_advertisement_bytes.min(MCP_MAX_ADVERTISEMENT_BYTES)
        {
            return Err(invalid(
                "selected MCP tools exceed the context budget; select fewer exact tool aliases in connection settings",
            ));
        }
        selected.push(authorized);
    }
    // Even an empty result must not hide a lost membership/unavailable catalog.
    let latest = get_catalog(config, pool, scope).await?;
    if latest.catalog_id != catalog.catalog_id {
        return Err(stale());
    }
    Ok(McpRunSelection {
        actor: scope.actor.into(),
        connection_id: scope.connection_id.into(),
        workspace_id: scope.workspace_id.into(),
        generation: scope.generation,
        tools: selected,
    })
}

#[cfg(test)]
mod tests;
