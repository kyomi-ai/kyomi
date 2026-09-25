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
//! ([`resolve_document`], [`find_document_by_title`], [`apply_update`],
//! [`apply_create`]).
//!
//! KYO-538 is stage 2 of 7 in the document-tool consolidation and unifies
//! *structure*, not *behaviour*. Every place the two families genuinely
//! behave differently today (embedding-refresh timing, validation,
//! targeted-edit reach across doc types) is preserved exactly and called
//! out with a `NOTE:` naming the ticket that will decide whether to
//! collapse it (KYO-541/542). CAS enforcement was one such difference
//! until KYO-539 unified it — both families now thread a real
//! `expected_content_hash` through [`apply_update`]. Initial
//! `knowledge_chunks` population on create was another — until KYO-776
//! unified it, `CreateDashboardTool` never populated it at all, while
//! `write_knowledge_file`'s create branch always did — both now go through
//! [`apply_create`].

mod delete;
mod edit;
mod read;

pub use delete::DocumentDeleteTool;
pub use edit::DocumentEditTool;
pub use read::DocumentReadTool;

use kyomi_core::models::Dashboard;
pub(crate) use kyomi_core::models::DocType;
use kyomi_embed::EmbeddingService;

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
    pub copilot_receipt: Option<kyomi_auth::dashboard_service::CopilotReceiptInput<'a>>,
    /// KYO-541: the shared chunk-refresh step below needs a resolved
    /// embedding service to call `rechunk_document`, regardless of which
    /// doc-type family called in. Every caller resolves this via
    /// `ctx.embedding.wait_ready().await?` before constructing this struct
    /// — the same call every pre-KYO-541 caller already made for its own
    /// (now-removed) rechunk step, except `modify_dashboard`, which did not
    /// rechunk at all and now must.
    pub embed: &'a EmbeddingService,
    /// [`ToolContext::document_id`](crate::tools::ToolContext::document_id)
    /// of the caller, if any — the single document a dashboard/knowledge
    /// copilot is scoped to. Every caller must pass this through explicitly
    /// (there is no default), so a future call site cannot simply forget
    /// the check by omission (KYO-536): see [`apply_update`]'s enforcement
    /// at the top of its body.
    pub document_scope: Option<&'a str>,
}

/// Outcome of [`apply_update`] — the three cases every pre-KYO-538 caller
/// (`write_knowledge_file`'s update branch, `edit_knowledge_file`, and
/// `modify_dashboard`) already classified identically.
pub(crate) enum ApplyUpdateOutcome {
    Updated,
    NotFound,
    Conflict(String),
}

/// A dashboard/knowledge copilot scoped to one open document
/// (`scope`, i.e. `ToolContext::document_id`) must not act on a *different*
/// document — deletion is a write like any other, and the read/edit tools
/// it shares this scope with. `target_dashboard_id` is the already-resolved
/// row id the caller is about to act on (post
/// `resolve_document`/`find_document_by_title` for locator-based callers),
/// not a raw user-supplied locator. `scope` is `None` for every non-copilot
/// caller (chat/MCP/Slack/watch), which always passes.
///
/// [`apply_update`] calls this itself, so every "replace a document's
/// content" tool inherits it automatically; [`DocumentDeleteTool`]'s delete
/// path calls it directly since deletion doesn't go through `apply_update`.
/// No copilot is actually granted a delete tool today (KYO-536), but the
/// guard is here anyway so a future one that is would inherit it rather
/// than needing its own copy.
pub(crate) fn enforce_document_scope(
    scope: Option<&str>,
    target_dashboard_id: &str,
) -> kyomi_core::Result<()> {
    if let Some(scope) = scope
        && scope != target_dashboard_id
    {
        return Err(kyomi_core::Error::Forbidden(format!(
            "This copilot is scoped to a single open document and cannot act on a \
             different one (requested {target_dashboard_id}, scoped to {scope})"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// apply_create — the shared "call create_dashboard, then populate
// knowledge_chunks" tail (KYO-776)
// ---------------------------------------------------------------------------

/// Parameters for [`apply_create`].
pub(crate) struct ApplyCreateParams<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub user_id: &'a str,
    pub workspace_id: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub doc_type: DocType,
    /// KYO-776: the shared population step below needs a resolved embedding
    /// service to call `rechunk_document`, the same requirement
    /// [`ApplyUpdateParams::embed`] documents for updates. Every caller
    /// resolves this via `ctx.embedding.wait_ready().await?` before
    /// constructing this struct.
    pub embed: &'a EmbeddingService,
}

/// Shared tail of every "create a new document" tool: call
/// `dashboard_service::create_dashboard`, then populate `knowledge_chunks`
/// for the row it just inserted.
///
/// NOTE (KYO-776 resolved policy, mirroring [`apply_update`]'s own NOTE
/// above): `create_dashboard`'s own `embed` parameter is always passed as
/// `None` here, which means its internal "rechunk newly created document in
/// background" branch (`spawn_rechunk_document`, see
/// `kyomi_auth::dashboard_service::create_dashboard`) never fires from this
/// call site. That is deliberate, not an oversight — this function does the
/// population itself, synchronously, immediately below. Passing `Some`
/// instead would *additionally* spawn a second, redundant rechunk in the
/// background on every create.
///
/// The population is synchronous rather than backgrounded for the same
/// reason `apply_update` rechunks synchronously rather than via
/// `spawn_rechunk_document` (see its doc comment above): it makes a
/// chunking failure visible in the tool's own result instead of vanishing
/// into a detached task, and it means `search_knowledge` can find the
/// document immediately rather than only after a background task happens
/// to finish first.
///
/// Before KYO-776, `CreateDashboardTool` never rechunked at all — a
/// dashboard created through the agent had zero `knowledge_chunks` rows
/// until its first edit, so `search_knowledge` could not find it in the
/// interval (`search_knowledge_chunks`, `tools/knowledge.rs`, reads
/// `knowledge_chunks` for both doc types whenever a caller omits
/// `doc_type`, the normal case). `write_knowledge_file`'s create-on-no-match
/// branch already rechunked explicitly on its own; it now goes through this
/// shared function instead of carrying its own copy of the same two calls.
pub(crate) async fn apply_create(params: ApplyCreateParams<'_>) -> kyomi_core::Result<String> {
    let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
        params.db,
        params.user_id,
        params.workspace_id,
        params.title,
        params.content,
        params.doc_type,
        None, // rechunk happens synchronously below — see the NOTE above.
    )
    .await?;

    kyomi_auth::dashboard_service::rechunk_document(
        params.db,
        params.embed,
        &dashboard_id,
        params.content,
        params.workspace_id,
    )
    .await?;

    Ok(dashboard_id)
}

/// Shared tail of every "replace a document's content" tool: call
/// `dashboard_service::update_dashboard` and classify the result.
///
/// NOTE (KYO-541 resolved policy — this function now owns chunk refresh):
/// `UpdateDashboardParams.embed` is always passed as `None` to
/// `update_dashboard` here, which means that function's own
/// `spawn_rechunk_document` fire-and-forget path never fires from this call
/// site. That is deliberate, not an oversight — this function does the
/// chunk refresh itself, synchronously, immediately below (see the
/// `rechunk_document` call after a successful update). Passing `Some`
/// instead would *additionally* spawn a second, redundant rechunk in the
/// background on every write.
///
/// The refresh is synchronous rather than backgrounded for the same reason
/// KYO-539 made `generate_dashboard_summary` read-then-write instead of
/// fire-and-forget: `rechunk_document` writes chunks derived from a content
/// snapshot, so two racing writers' background jobs could land the older
/// snapshot's chunks last, leaving `knowledge_chunks` reflecting a stale
/// version of the document even though the row itself has the newer
/// content. Awaiting here makes chunk order follow write order, and it
/// makes a chunking failure visible in the tool's own result instead of
/// vanishing into a detached task.
///
/// Before KYO-541, every caller performed its own embedding refresh
/// afterward, by its own doc_type-specific policy: `write_knowledge_file`
/// and `edit_knowledge_file` rechunked synchronously immediately after this
/// call succeeded; `modify_dashboard` never rechunked knowledge_chunks at
/// all — the real bug this ticket fixes, since `search_knowledge_chunks`
/// reads `knowledge_chunks` for both doc types whenever a caller omits
/// `doc_type` (the normal case).
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
    // KYO-536: a prompt-injected or model-confused write to some other
    // document id must fail here, before `update_dashboard` is ever called
    // — not merely be discouraged in the tool's prompt text. This is the
    // single choke point every "replace a document's content" tool goes
    // through, so a future document-mutating tool inherits the guard
    // automatically rather than needing its own copy. See
    // `enforce_document_scope`.
    enforce_document_scope(params.document_scope, params.dashboard_id)?;
    let is_copilot_write = params.copilot_receipt.is_some();

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
            copilot_receipt: params.copilot_receipt,
        },
    )
    .await
    {
        Ok(true) => {
            // Chunks are derived from content, so there is nothing to
            // refresh when only the title changed (`params.content` is
            // `None`). When content did change, refresh unconditionally —
            // for both `DocType::Knowledge` and `DocType::Dashboard`; see
            // the module-level `NOTE` above for why this happens here,
            // synchronously, rather than in each caller or via
            // `update_dashboard`'s own background path.
            if let Some(content) = params.content {
                let rechunk_result = kyomi_auth::dashboard_service::rechunk_document(
                    params.db,
                    params.embed,
                    params.dashboard_id,
                    content,
                    params.workspace_id,
                )
                .await;
                if let Err(e) = rechunk_result {
                    if is_copilot_write {
                        // The document and receipt committed together. A
                        // derived-index failure cannot turn that saved write
                        // into a reported failure with no Undo control.
                        tracing::warn!(error = %e, dashboard_id = %params.dashboard_id,
                            "copilot write persisted but chunk refresh failed");
                    } else {
                        return Err(e);
                    }
                }
            }
            Ok(ApplyUpdateOutcome::Updated)
        }
        Ok(false) => Ok(ApplyUpdateOutcome::NotFound),
        Err(kyomi_core::Error::Conflict(msg)) => Ok(ApplyUpdateOutcome::Conflict(msg)),
        Err(e) => Err(e),
    }
}
