// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable confidentiality and publication ordering for personal external data.
//! No network is performed under these transactions. Destination/tool consent is
//! independently checked by the grants adapter; model/remote content cannot grant it.
use kyomi_core::{Config, DbPool, Error, Result, db_fetch_optional, db_with_pool};

fn denied() -> Error {
    Error::Forbidden("Personal external data requires a private owner-only destination".into())
}

/// Server-selected execution surface. Missing/ambiguous contexts are denied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonalMcpSurface {
    InteractiveChat,
    PrivateDocumentCopilot,
    Denied,
}

/// Authenticated identity and current connection revision, never model arguments.
pub struct ExternalDispatch<'a> {
    pub actor: &'a str,
    pub workspace: &'a str,
    pub session: &'a str,
    pub connection: &'a str,
    pub generation: i64,
    pub surface: PersonalMcpSurface,
    pub document: Option<&'a str>,
}

/// Commit the permanent restriction BEFORE the adapter emits tool intent or sends
/// external bytes. The session UPDATE serializes with sharing, native publication
/// admission, and durable read authorization. Connection lifecycle ordering and
/// exact destination consent remain mandatory independent gates in the adapter.
pub async fn admit_external_dispatch(
    db: &DbPool,
    config: &Config,
    scope: ExternalDispatch<'_>,
) -> Result<()> {
    crate::personal_mcp_service::require_feature(config)?;
    if scope.surface == PersonalMcpSurface::Denied {
        return Err(denied());
    }
    db_with_pool!(db, |pool| {
        let mut tx = pool.begin().await?;
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "UPDATE chat_sessions SET event_sequence = event_sequence WHERE session_id=$1 AND workspace_id=$2 AND user_id=$3 AND NOT COALESCE(shared,FALSE) AND native_publications=0 AND platform_type IS NULL AND (($4 AND session_type='chat') OR (NOT $4 AND session_type IN ('dashboard_copilot','knowledge_copilot'))) AND EXISTS (SELECT 1 FROM workspace_users wu JOIN users u ON u.user_id=wu.user_id WHERE wu.workspace_id=$2 AND wu.user_id=$3 AND wu.active=TRUE AND u.active=TRUE) RETURNING user_id,personal_data_owner_id"
        ).bind(scope.session).bind(scope.workspace).bind(scope.actor)
         .bind(scope.surface == PersonalMcpSurface::InteractiveChat)
         .fetch_optional(&mut *tx).await?;
        if row.is_none() {
            return Err(denied());
        }
        let enabled: Option<(String,)> = sqlx::query_as("SELECT c.connection_id FROM user_mcp_connections c JOIN user_mcp_workspace_enablements e ON e.connection_id=c.connection_id AND e.owner_user_id=c.owner_user_id WHERE c.connection_id=$1 AND c.owner_user_id=$2 AND c.generation=$3 AND c.status='ready' AND e.workspace_id=$4")
            .bind(scope.connection).bind(scope.actor).bind(scope.generation).bind(scope.workspace).fetch_optional(&mut *tx).await?;
        if enabled.is_none() {
            return Err(denied());
        }
        if scope.surface == PersonalMcpSurface::PrivateDocumentCopilot {
            let document = scope.document.ok_or_else(denied)?;
            let owned: Option<(String,)> = sqlx::query_as("UPDATE dashboards SET title=title WHERE dashboard_id=$1 AND workspace_id=$2 AND user_id=$3 RETURNING dashboard_id")
                .bind(document).bind(scope.workspace).bind(scope.actor).fetch_optional(&mut *tx).await?;
            if owned.is_none() {
                return Err(denied());
            }
            // Collection mutations lock this same document row. Lock existing
            // collections too, so a private->public permission flip cannot win
            // after this check and before the marker commits.
            let collections = if db.is_postgres() {
                "SELECT c.is_public FROM collections c JOIN collection_dashboards cd ON cd.collection_id=c.id WHERE cd.dashboard_id=$1 FOR SHARE OF c"
            } else {
                "SELECT c.is_public FROM collections c JOIN collection_dashboards cd ON cd.collection_id=c.id WHERE cd.dashboard_id=$1"
            };
            let visibility: Vec<(bool,)> = sqlx::query_as(collections)
                .bind(document)
                .fetch_all(&mut *tx)
                .await?;
            if visibility.iter().any(|v| v.0) {
                return Err(denied());
            }
            sqlx::query("INSERT INTO personal_mcp_private_documents (dashboard_id,owner_user_id) VALUES ($1,$2) ON CONFLICT(dashboard_id) DO NOTHING")
                .bind(document).bind(scope.actor).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO personal_mcp_private_contexts (owner_user_id,workspace_id) VALUES ($1,$2) ON CONFLICT(owner_user_id,workspace_id) DO NOTHING")
                .bind(scope.actor).bind(scope.workspace).execute(&mut *tx).await?;
        } else if scope.document.is_some() {
            return Err(denied());
        }
        let changed = sqlx::query("UPDATE chat_sessions SET personal_data_owner_id=$1, personal_data_document_id=$3 WHERE session_id=$2 AND (personal_data_owner_id IS NULL OR personal_data_document_id IS NOT DISTINCT FROM $3)")
            .bind(scope.actor).bind(scope.session).bind(scope.document).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(denied());
        }
        tx.commit().await?;
    });
    Ok(())
}

/// Admission for a potentially publishing native action. The counter is durable:
/// a process lost mid-effect fails closed instead of admitting MCP concurrently.
/// Call finish_native_publication after the awaited tool, including on errors.
/// Never reset this counter on reconnect; a lost worker requires reconciliation.
pub async fn begin_native_publication(db: &DbPool, actor: &str, session: &str) -> Result<String> {
    let token = uuid::Uuid::new_v4().to_string();
    db_with_pool!(db, |pool| {
        let mut tx = pool.begin().await?;
        let admitted: Option<(String,)> = sqlx::query_as("UPDATE chat_sessions SET native_publications=native_publications+1 WHERE session_id=$1 AND personal_data_owner_id IS NULL AND (user_id=$2 OR COALESCE(shared,FALSE)) AND EXISTS (SELECT 1 FROM workspace_users wu JOIN users u ON u.user_id=wu.user_id WHERE wu.workspace_id=chat_sessions.workspace_id AND wu.user_id=$2 AND wu.active=TRUE AND u.active=TRUE) RETURNING session_id")
            .bind(session).bind(actor).fetch_optional(&mut *tx).await?;
        if admitted.is_none() {
            return Err(denied());
        }
        sqlx::query("INSERT INTO personal_mcp_native_publications (publication_id,session_id,actor_id) VALUES ($1,$2,$3)")
            .bind(&token).bind(session).bind(actor).execute(&mut *tx).await?;
        tx.commit().await?;
    });
    Ok(token)
}
/// Idempotent release of this exact completed operation, never another worker's
/// lease. Cancellation drops the execution future; its counter is retained until
/// the effect's worker has actually completed (the agent wrapper owns that task).
pub async fn finish_native_publication(db: &DbPool, actor: &str, token: &str) -> Result<()> {
    db_with_pool!(db, |pool| {
        let mut tx = pool.begin().await?;
        let row: Option<(String,)> = sqlx::query_as("DELETE FROM personal_mcp_native_publications WHERE publication_id=$1 AND actor_id=$2 RETURNING session_id")
            .bind(token).bind(actor).fetch_optional(&mut *tx).await?;
        if let Some((session,)) = row {
            sqlx::query("UPDATE chat_sessions SET native_publications=native_publications-1 WHERE session_id=$1 AND native_publications>0")
                .bind(session).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    });
    Ok(())
}

/// Owner private export/history remains usable. Shared exports/copies have no
/// exception: this predicate must precede any future destination-copy adapter.
pub async fn authorize_private_read(db: &DbPool, actor: &str, session: &str) -> Result<()> {
    let owner = db_fetch_optional!(
        db,
        (String,),
        "SELECT user_id FROM chat_sessions WHERE session_id=$1 AND (personal_data_owner_id IS NULL OR personal_data_owner_id=$2)",
        session,
        actor
    )?;
    owner.map(|_| ()).ok_or_else(denied)
}

/// Trusted server paths may auto-save ONLY into the restricted document owned by
/// the same actor. Collection triggers prevent subsequent public permission flips.
pub async fn is_private_copilot_destination(
    db: &DbPool,
    actor: &str,
    session: &str,
    document: &str,
) -> Result<bool> {
    Ok(db_fetch_optional!(db, (String,), "SELECT d.dashboard_id FROM dashboards d JOIN personal_mcp_private_documents p ON p.dashboard_id=d.dashboard_id JOIN chat_sessions s ON s.personal_data_owner_id=p.owner_user_id WHERE s.session_id=$1 AND s.personal_data_document_id=$3 AND s.user_id=$2 AND p.owner_user_id=$2 AND d.user_id=$2 AND d.dashboard_id=$3 AND s.workspace_id=d.workspace_id AND NOT EXISTS (SELECT 1 FROM collection_dashboards cd JOIN collections c ON c.id=cd.collection_id WHERE cd.dashboard_id=d.dashboard_id AND c.is_public=TRUE)", session, actor, document)?.is_some())
}

#[cfg(test)]
mod tests;

/// Persisted owner restriction, reused by tools and server authorization paths.
pub async fn restricted_owner(db: &DbPool, session: &str) -> Result<Option<String>> {
    Ok(db_fetch_optional!(
        db,
        (Option<String>,),
        "SELECT personal_data_owner_id FROM chat_sessions WHERE session_id=$1",
        session
    )?
    .and_then(|r| r.0))
}

/// Document-bearing agent reads conservatively inherit the workspace owner's
/// existing private-document restriction before any content enters model context.
/// Lists/searches may return several documents; conservative inheritance prevents
/// title, snippets and joined metadata from becoming an untracked disclosure path.
/// Scheduled/shared/ambiguous contexts cannot inherit and are rejected.
pub async fn inherit_private_document_context(
    db: &DbPool,
    actor: &str,
    workspace: &str,
    session: Option<&str>,
    document: Option<&str>,
) -> Result<()> {
    let exists = db_fetch_optional!(db, (String,), "SELECT owner_user_id FROM personal_mcp_private_contexts WHERE owner_user_id=$1 AND workspace_id=$2", actor, workspace)?.is_some();
    if !exists {
        return Ok(());
    }
    let session = session.ok_or_else(denied)?;
    let admitted = db_fetch_optional!(
        db,
        (String,),
        "UPDATE chat_sessions SET personal_data_owner_id=$1, personal_data_document_id=$4 WHERE session_id=$2 AND workspace_id=$3 AND user_id=$1 AND NOT COALESCE(shared,FALSE) AND platform_type IS NULL AND native_publications=0 AND (session_type='chat' OR (session_type IN ('dashboard_copilot','knowledge_copilot') AND $4 IS NOT NULL AND EXISTS (SELECT 1 FROM personal_mcp_private_documents p WHERE p.dashboard_id=$4 AND p.owner_user_id=$1))) AND (personal_data_owner_id IS NULL OR personal_data_document_id IS NOT DISTINCT FROM $4) RETURNING session_id",
        actor,
        session,
        workspace,
        document
    )?;
    admitted.map(|_| ()).ok_or_else(denied)
}

/// Bounded, owner-scoped diagnostics for a run blocked by outstanding native work.
/// No TTL cleanup is allowed: process/transport completion must be established by
/// the host before passing a specific operation to finish_native_publication.
/// A fresh conversation remains usable while an uncertain old run stays closed.
pub async fn outstanding_native_publications(
    db: &DbPool,
    actor: &str,
    session: &str,
) -> Result<Vec<String>> {
    let rows = kyomi_core::db_fetch_all!(
        db,
        (String,),
        "SELECT publication_id FROM personal_mcp_native_publications WHERE actor_id=$1 AND session_id=$2 ORDER BY publication_id LIMIT 100",
        actor,
        session
    )?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Explicit host-only recovery, deliberately not an AgentTool or server function.
/// `confirm_worker_stopped` must establish that the identified operation's worker
/// has terminated and cannot resume sending/writing. A terminal run flag, elapsed
/// time, disconnected browser or expired lease is NOT this evidence. Do not call
/// it from model/remote content. It cannot undo an already committed remote effect.
/// Verification runs before DB locking; exact actor/session/token scope prevents
/// releasing a different operation. Operators can enumerate at most 100 owned
/// claims with outstanding_native_publications and reconcile confirmed lost work.
pub async fn reconcile_stopped_native_publication<F>(
    db: &DbPool,
    actor: &str,
    session: &str,
    token: &str,
    confirm_worker_stopped: F,
) -> Result<bool>
where
    F: std::future::Future<Output = Result<()>>,
{
    confirm_worker_stopped.await?;
    let owned = db_fetch_optional!(db,(String,), "SELECT publication_id FROM personal_mcp_native_publications WHERE publication_id=$1 AND actor_id=$2 AND session_id=$3", token,actor,session)?.is_some();
    if !owned {
        return Ok(false);
    }
    finish_native_publication(db, actor, token).await?;
    Ok(true)
}

/// Defense at legacy message submission/Redis receipt. The socket transport also
/// retains current session/membership authorization through its actual flush.
/// Sessionless/account messages are handled by the existing workspace gate.
/// Missing rows carry no durable privacy marker; final conversation delivery
/// denies unknown sessions. Never return protected payloads in an error.
pub async fn can_receive_session_message(
    db: &DbPool,
    recipient: &str,
    session: &str,
) -> Result<bool> {
    Ok(restricted_owner(db, session)
        .await?
        .is_none_or(|owner| owner == recipient))
}

/// Legacy document updates carry no session ID. Their document identity is
/// protected metadata too; use this gate for both local and Redis submission.
/// Admission commits the marker before any personal content can be persisted.
pub async fn can_receive_notification(
    db: &DbPool,
    recipient: &str,
    message: &kyomi_core::WebSocketMessage,
) -> Result<bool> {
    if let Some(session) = message.session_id.as_deref()
        && !can_receive_session_message(db, recipient, session).await?
    {
        return Ok(false);
    }
    let document = message.data.as_ref().and_then(|data| {
        data.get("dashboard_id")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                matches!(
                    data.get("entity_type").and_then(serde_json::Value::as_str),
                    Some("dashboard" | "knowledge")
                )
                .then(|| data.get("entity_id").and_then(serde_json::Value::as_str))
                .flatten()
            })
    });
    let Some(document) = document else {
        return Ok(true);
    };
    Ok(db_fetch_optional!(
        db,
        (String,),
        "SELECT owner_user_id FROM personal_mcp_private_documents WHERE dashboard_id=$1",
        document
    )?
    .is_none_or(|row| row.0 == recipient))
}
