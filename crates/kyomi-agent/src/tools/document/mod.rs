// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared document-operations core for the agent tool layer (KYO-538).
//!
//! Dashboards and knowledge documents are one table (`dashboards`) with a
//! `DocType` discriminator, and `kyomi_auth::dashboard_service` is already
//! parameterised by `doc_type_filter` at the storage layer (see
//! `~/repos/kyomi/CLAUDE.md`, *Where things live*). The *agent tool layer*
//! was not: `tools::knowledge` and `tools::dashboard` grew two independent
//! copies of "resolve a locator", "read a document", and "apply a content
//! update", each calling `dashboard_service` the same way but drifting in
//! the details around them. This module is the shared core those two
//! families now call into.
//!
//! This module (and its submodules) knows nothing about `AgentTool`, JSON
//! schemas, or the LLM beyond what the [`AgentTool`] trait itself requires —
//! the actual business logic only calls `kyomi_auth::dashboard_service`.
//! [`read`], [`edit`], and [`delete`] each host one `AgentTool` struct whose
//! *entire* tool-facing shape (not just the underlying DB call) is
//! genuinely identical across both families, selected by the [`DocType`]
//! they are constructed with:
//! [`DocumentReadTool`] (`read_knowledge_file` / `get_dashboard_info`),
//! [`DocumentEditTool`] (`edit_knowledge_file`), and [`DocumentDeleteTool`]
//! (`delete_dashboard`).
//!
//! Tools whose parameter schema or response shape diverges beyond what a
//! `DocType` switch can express cleanly — `search_knowledge`,
//! `list_knowledge_files`, `search_dashboards`, `write_knowledge_file`,
//! `create_dashboard`, `modify_dashboard` — stay in `tools::knowledge` /
//! `tools::dashboard` as their own structs, but call into this module's
//! shared functions for the parts that genuinely are the same operation
//! ([`resolve_document`], [`find_document_by_title`], [`apply_update`]).
//!
//! KYO-538 is stage 2 of 7 in the document-tool consolidation and unifies
//! *structure*, not *behaviour*. Every place the two families genuinely
//! behave differently today (embedding-refresh timing, validation,
//! targeted-edit reach across doc types) is preserved exactly and called
//! out with a `NOTE:` naming the ticket that will decide whether to
//! collapse it (KYO-541/542). CAS enforcement was one such difference
//! until KYO-539 unified it — both families now thread a real
//! `expected_content_hash` through [`apply_update`].

mod delete;
mod edit;
mod read;

pub use delete::DocumentDeleteTool;
pub use edit::DocumentEditTool;
pub use read::DocumentReadTool;

use kyomi_core::models::Dashboard;
pub(crate) use kyomi_core::models::DocType;

// ---------------------------------------------------------------------------
// resolve / find-by-title
// ---------------------------------------------------------------------------

/// Search for a document by exact title match within the workspace.
/// Returns the dashboard if found.
///
/// Moved out of `tools::knowledge` unchanged by KYO-538 —
/// `write_knowledge_file` (still in `tools::knowledge`) calls this directly
/// to check for an existing document, and [`resolve_document`] below calls
/// it as a fallback.
///
/// NOTE (KYO-538 binding decision 4, pinned by KYO-537's
/// `edit_knowledge_file_reaches_across_doc_type_into_dashboards`):
/// `doc_type_filter: None` here is deliberate, current, load-bearing
/// behavior, not a bug to fix in this ticket — any caller resolving a
/// locator through this function (currently `edit_knowledge_file` and
/// `write_knowledge_file`'s existing-document lookup) reaches `Dashboard`-
/// doc_type rows too.
pub(crate) async fn find_document_by_title(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    user_id: &str,
    title: &str,
) -> kyomi_core::Result<Option<Dashboard>> {
    let results = kyomi_auth::dashboard_service::search_dashboards(
        db,
        workspace_id,
        user_id,
        Some(title),
        None,
        kyomi_auth::dashboard_service::SearchSort::Recent,
        100,
    )
    .await?;

    // Find exact title match (case-sensitive)
    let matched = results.iter().find(|d| d.title == title);

    if let Some(m) = matched {
        kyomi_auth::dashboard_service::get_dashboard(db, &m.dashboard_id, workspace_id, user_id).await
    } else {
        Ok(None)
    }
}

/// Resolve a document by path or ID. Supports:
/// - UUID lookup (if input looks like a UUID)
/// - Exact title match
/// - Backward compat: if path contains `/`, strip directory and search by filename
///
/// Moved out of `tools::knowledge` unchanged by KYO-538. Per KYO-538 binding
/// decision 3, the slash-stripping "file" metaphor stays as-is — a
/// React-era artifact, but shipped API.
pub(crate) async fn resolve_document(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    user_id: &str,
    path: &str,
) -> kyomi_core::Result<Option<Dashboard>> {
    // If it looks like a UUID, try direct lookup first
    if uuid::Uuid::parse_str(path).is_ok() {
        let doc = kyomi_auth::dashboard_service::get_dashboard(db, path, workspace_id, user_id).await?;
        if doc.is_some() {
            return Ok(doc);
        }
    }

    // Try exact title match
    let doc = find_document_by_title(db, workspace_id, user_id, path).await?;
    if doc.is_some() {
        return Ok(doc);
    }

    // Backward compat: if path contains `/`, strip directory and search by filename
    if let Some(slash_pos) = path.rfind('/') {
        let filename = &path[slash_pos + 1..];
        if !filename.is_empty() {
            return find_document_by_title(db, workspace_id, user_id, filename).await;
        }
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------

/// Read a single document, dispatched by `doc_type`.
///
/// NOTE: this is a genuine, preserved behavioural fork between the two
/// families' "read a document" tools, not an accidental one — knowledge
/// reads accept a flexible locator (UUID, exact title, or a legacy
/// slash-path — see [`resolve_document`]) and never record a view.
/// Dashboard reads require an exact `dashboard_id` and record a view for
/// popularity tracking (dashboards are the only entity type with a
/// popularity feature — a product distinction, not a bug). This is not one
/// of KYO-539/541/542's three adjudicated differences (CAS, embedding
/// refresh, validation); it is preserved here as pre-existing, deliberate
/// product behavior with no ticket currently scheduled to change it.
pub(crate) async fn read_document(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    user_id: &str,
    doc_type: DocType,
    locator: &str,
) -> kyomi_core::Result<Option<Dashboard>> {
    match doc_type {
        DocType::Knowledge => resolve_document(db, workspace_id, user_id, locator).await,
        DocType::Dashboard => {
            let doc =
                kyomi_auth::dashboard_service::get_dashboard(db, locator, workspace_id, user_id).await?;

            if doc.is_some() {
                // Record the view for popularity tracking. Fire-and-forget,
                // matching pre-KYO-538 `get_dashboard_info` exactly: a
                // view-tracking failure must never fail the read itself.
                let _ =
                    kyomi_auth::dashboard_service::record_view(db, locator, user_id, workspace_id).await;
            }

            Ok(doc)
        }
    }
}

// ---------------------------------------------------------------------------
// apply_update — the shared "call update_dashboard, classify the result" tail
// ---------------------------------------------------------------------------

/// Parameters for [`apply_update`].
pub(crate) struct ApplyUpdateParams<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub dashboard_id: &'a str,
    pub workspace_id: &'a str,
    pub user_id: &'a str,
    pub title: Option<&'a str>,
    pub content: Option<&'a str>,
    pub change_summary: Option<&'a str>,
    pub expected_content_hash: Option<&'a str>,
}

/// Outcome of [`apply_update`] — the three cases every pre-KYO-538 caller
/// (`write_knowledge_file`'s update branch, `edit_knowledge_file`, and
/// `modify_dashboard`) already classified identically.
pub(crate) enum ApplyUpdateOutcome {
    Updated,
    NotFound,
    Conflict(String),
}

/// Shared tail of every "replace a document's content" tool: call
/// `dashboard_service::update_dashboard` and classify the result.
///
/// NOTE: `embed` is always `None` here, matching all three pre-KYO-538 call
/// sites exactly — every caller performs its own embedding refresh
/// afterward, by its own doc_type-specific policy. `write_knowledge_file`
/// and `edit_knowledge_file` rechunk synchronously immediately after this
/// call succeeds; `modify_dashboard` instead spawns a background embedding
/// job (and only conditionally, when substantial content was supplied).
/// KYO-541 is expected to unify embedding-refresh timing; preserved as-is
/// here, and each caller keeps its own post-update step rather than this
/// function attempting to own both policies.
///
/// NOTE: CAS (`expected_content_hash`) is caller-supplied and not enforced
/// by this function itself — it is simply threaded through to
/// `update_dashboard`. As of KYO-539, every caller in both families
/// (knowledge and dashboard) passes a real hash from a prior read of the
/// document (or `None` for legacy rows predating hashing, which disables
/// CAS for that write exactly as it always has). A stale hash gets
/// `Conflict` back rather than silently clobbering a concurrent write.
pub(crate) async fn apply_update(
    params: ApplyUpdateParams<'_>,
) -> kyomi_core::Result<ApplyUpdateOutcome> {
    match kyomi_auth::dashboard_service::update_dashboard(
        kyomi_auth::dashboard_service::UpdateDashboardParams {
            db: params.db,
            embed: None,
            dashboard_id: params.dashboard_id,
            workspace_id: params.workspace_id,
            user_id: params.user_id,
            title: params.title,
            content: params.content,
            change_summary: params.change_summary,
            expected_content_hash: params.expected_content_hash,
        },
    )
    .await
    {
        Ok(true) => Ok(ApplyUpdateOutcome::Updated),
        Ok(false) => Ok(ApplyUpdateOutcome::NotFound),
        Err(kyomi_core::Error::Conflict(msg)) => Ok(ApplyUpdateOutcome::Conflict(msg)),
        Err(e) => Err(e),
    }
}
