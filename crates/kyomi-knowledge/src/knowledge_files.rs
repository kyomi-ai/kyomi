// SPDX-License-Identifier: AGPL-3.0-or-later

//! Knowledge files CRUD service.
//!
//! Manages a markdown file tree per workspace. Files are chunked and embedded
//! for semantic search, while the full document content is the source of truth.
//!
//! Key design:
//! - Files and folders form a tree via `parent_id` self-FK
//! - Content changes trigger rechunking + re-embedding
//! - `content_hash` enables optimistic concurrency (CAS on update)
//! - Table references extracted from backtick patterns

use kyomi_core::db::DbPool;
use kyomi_core::sql_compat::cast_to_text;
use kyomi_embed::EmbeddingService;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use uuid::Uuid;

/// Build the full SELECT column list for `KnowledgeFile` rows, applying
/// `cast_to_text` on UUID columns so the query works on both Postgres and SQLite.
fn knowledge_file_columns(is_pg: bool) -> String {
    let ct = |col: &str| cast_to_text(is_pg, col);
    format!(
        "{id} AS id, {workspace_id} AS workspace_id, {parent_id} AS parent_id, \
         name, is_folder, content, content_hash, sort_order, \
         {created_by} AS created_by, {updated_by} AS updated_by, created_at, updated_at",
        id = ct("id"),
        workspace_id = ct("workspace_id"),
        parent_id = ct("parent_id"),
        created_by = ct("created_by"),
        updated_by = ct("updated_by"),
    )
}

/// Build the SELECT column list for `KnowledgeFileTreeEntry` rows.
fn tree_entry_columns(is_pg: bool) -> String {
    let ct = |col: &str| cast_to_text(is_pg, col);
    format!(
        "{id} AS id, {parent_id} AS parent_id, name, is_folder, sort_order, \
         updated_at, {updated_by} AS updated_by",
        id = ct("id"),
        parent_id = ct("parent_id"),
        updated_by = ct("updated_by"),
    )
}

/// Build the SELECT column list for `KnowledgeFileSearchResult` rows.
fn search_result_columns(is_pg: bool) -> String {
    let ct = |col: &str| cast_to_text(is_pg, col);
    format!(
        "{id} AS id, {parent_id} AS parent_id, name, is_folder",
        id = ct("id"),
        parent_id = ct("parent_id"),
    )
}

/// Regex for extracting backtick-wrapped table references (e.g., `schema.table`).
static TABLE_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`(\w+\.\w+(?:\.\w+)?)`").expect("valid regex"));

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

/// A knowledge file record (file or folder).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct KnowledgeFile {
    pub id: String,
    pub workspace_id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub is_folder: bool,
    pub content: Option<String>,
    pub content_hash: Option<String>,
    pub sort_order: i32,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Search result entry with content preview.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct KnowledgeFileSearchResult {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub is_folder: bool,
    pub content_preview: Option<String>,
}

/// Lightweight tree entry (no content).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct KnowledgeFileTreeEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub is_folder: bool,
    pub sort_order: i32,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: Option<String>,
}

// ---------------------------------------------------------------------------
// Content hashing
// ---------------------------------------------------------------------------

/// Compute a short SHA-256 hash of content (first 16 hex chars).
fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}

// ---------------------------------------------------------------------------
// CRUD operations
// ---------------------------------------------------------------------------

/// Create a new file or folder.
///
/// Returns the created file record.
pub async fn create_file(
    db: &DbPool,
    embed: &EmbeddingService,
    workspace_id: &str,
    parent_id: Option<&str>,
    name: &str,
    content: Option<&str>,
    is_folder: bool,
    user_id: &str,
) -> anyhow::Result<KnowledgeFile> {
    let id = Uuid::new_v4().to_string();
    let content_hash = content.map(hash_content);

    let is_pg = db.is_postgres();
    let cols = knowledge_file_columns(is_pg);
    let now = kyomi_core::sql_compat::now(is_pg);
    let sql = format!(
        "INSERT INTO knowledge_files \
            (id, workspace_id, parent_id, name, is_folder, content, content_hash, \
             created_by, updated_by, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, {now}, {now}) \
         RETURNING {cols}"
    );
    let file = kyomi_core::db_fetch_one!(
        db,
        KnowledgeFile,
        &sql,
        &id,
        &workspace_id,
        &parent_id as &Option<&str>,
        &name,
        &is_folder,
        &content as &Option<&str>,
        &content_hash as &Option<String>,
        &user_id
    )?;

    // Chunk + embed file content
    if let Some(text) = content
        && !text.trim().is_empty() {
            rechunk_file(db, embed, &file.id, text, workspace_id).await?;
        }

    Ok(file)
}

/// Get a single file with content.
pub async fn get_file(
    db: &DbPool,
    workspace_id: &str,
    file_id: &str,
) -> anyhow::Result<Option<KnowledgeFile>> {
    let cols = knowledge_file_columns(db.is_postgres());
    let sql = format!(
        "SELECT {cols} FROM knowledge_files \
         WHERE id = $1 AND workspace_id = $2"
    );
    let file = kyomi_core::db_fetch_optional!(
        db,
        KnowledgeFile,
        &sql,
        &file_id,
        &workspace_id
    )?;

    Ok(file)
}

/// Resolve a file by path (e.g., `Revenue/Metrics.md`).
///
/// Walks the parent_id chain from root to find the target file.
pub async fn get_file_by_path(
    db: &DbPool,
    workspace_id: &str,
    path: &str,
) -> anyhow::Result<Option<KnowledgeFile>> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if parts.is_empty() {
        return Ok(None);
    }

    let mut current_parent: Option<String> = None;
    let cols = knowledge_file_columns(db.is_postgres());
    let sql_with_parent = format!(
        "SELECT {cols} FROM knowledge_files \
         WHERE workspace_id = $1 AND parent_id = $2 AND name = $3"
    );
    let sql_null_parent = format!(
        "SELECT {cols} FROM knowledge_files \
         WHERE workspace_id = $1 AND parent_id IS NULL AND name = $2"
    );

    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;

        let file = if current_parent.is_some() {
            kyomi_core::db_fetch_optional!(
                db,
                KnowledgeFile,
                &sql_with_parent,
                &workspace_id,
                &current_parent as &Option<String>,
                part
            )?
        } else {
            kyomi_core::db_fetch_optional!(
                db,
                KnowledgeFile,
                &sql_null_parent,
                &workspace_id,
                part
            )?
        };

        match file {
            Some(f) => {
                if is_last {
                    return Ok(Some(f));
                }
                if !f.is_folder {
                    return Ok(None); // intermediate part is not a folder
                }
                current_parent = Some(f.id);
            }
            None => return Ok(None),
        }
    }

    Ok(None)
}

/// Update file content with optimistic concurrency via content_hash.
///
/// Returns the updated file, or `None` if the hash didn't match (conflict).
pub async fn update_file_content(
    db: &DbPool,
    embed: &EmbeddingService,
    workspace_id: &str,
    file_id: &str,
    content: &str,
    user_id: &str,
    expected_hash: Option<&str>,
) -> anyhow::Result<Option<KnowledgeFile>> {
    let new_hash = hash_content(content);

    // CAS: only update if content_hash matches.
    // When expected_hash is None, only allow the update if content is NULL (first write).
    // This prevents TOCTOU races — once content exists, callers must provide expected_hash.
    let is_pg = db.is_postgres();
    let cols = knowledge_file_columns(is_pg);
    let now = kyomi_core::sql_compat::now(is_pg);
    let result = if let Some(expected) = expected_hash {
        let sql = format!(
            "UPDATE knowledge_files \
             SET content = $1, content_hash = $2, updated_by = $3, updated_at = {now} \
             WHERE id = $4 AND workspace_id = $5 AND content_hash = $6 \
             RETURNING {cols}"
        );
        kyomi_core::db_fetch_optional!(
            db,
            KnowledgeFile,
            &sql,
            &content,
            &new_hash,
            &user_id,
            &file_id,
            &workspace_id,
            &expected
        )?
    } else {
        // No expected_hash: only allow if content is NULL (first write)
        let sql_first_write = format!(
            "UPDATE knowledge_files \
             SET content = $1, content_hash = $2, updated_by = $3, updated_at = {now} \
             WHERE id = $4 AND workspace_id = $5 AND content IS NULL \
             RETURNING {cols}"
        );
        let updated = kyomi_core::db_fetch_optional!(
            db,
            KnowledgeFile,
            &sql_first_write,
            &content,
            &new_hash,
            &user_id,
            &file_id,
            &workspace_id
        )?;
        if updated.is_none() {
            // Could be "file not found" or "file exists but already has content".
            // Disambiguate by checking if the file exists at all.
            let sql_exists = format!(
                "SELECT {cols} FROM knowledge_files \
                 WHERE id = $1 AND workspace_id = $2"
            );
            let exists = kyomi_core::db_fetch_optional!(
                db,
                KnowledgeFile,
                &sql_exists,
                &file_id,
                &workspace_id
            )?;
            if exists.is_none() {
                anyhow::bail!("File not found");
            }
            // File exists but has content — real conflict
            anyhow::bail!(
                "Conflict: file already has content, provide expected content_hash for update"
            );
        }
        updated
    };

    if let Some(ref file) = result {
        rechunk_file(db, embed, &file.id, content, workspace_id).await?;
    }

    Ok(result)
}

/// Edit file content via targeted string replacement.
///
/// Returns the updated file, or an error if old_text not found or found multiple times.
pub async fn edit_file_content(
    db: &DbPool,
    embed: &EmbeddingService,
    workspace_id: &str,
    file_id: &str,
    old_text: &str,
    new_text: &str,
    user_id: &str,
) -> anyhow::Result<KnowledgeFile> {
    let file = get_file(db, workspace_id, file_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("File not found"))?;

    let content = file
        .content
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Cannot edit a folder"))?;

    let count = content.matches(old_text).count();
    if count == 0 {
        anyhow::bail!("old_text not found in file content. The file may have been modified.");
    }
    if count > 1 {
        anyhow::bail!(
            "old_text found {count} times in file content. It must appear exactly once."
        );
    }

    let new_content = content.replacen(old_text, new_text, 1);

    let updated = update_file_content(
        db,
        embed,
        workspace_id,
        file_id,
        &new_content,
        user_id,
        file.content_hash.as_deref(), // use current hash for CAS
    )
    .await?;

    updated.ok_or_else(|| {
        anyhow::anyhow!("Conflict: file was modified concurrently. Re-read and retry.")
    })
}

/// Rename a file or folder.
pub async fn rename_file(
    db: &DbPool,
    workspace_id: &str,
    file_id: &str,
    new_name: &str,
    user_id: &str,
) -> anyhow::Result<()> {
    let now = kyomi_core::sql_compat::now(db.is_postgres());
    let sql = format!(
        "UPDATE knowledge_files SET name = $1, updated_by = $2, updated_at = {now} \
         WHERE id = $3 AND workspace_id = $4"
    );
    let result = kyomi_core::db_execute!(
        db,
        &sql,
        &new_name,
        &user_id,
        &file_id,
        &workspace_id
    )?;

    if result.rows_affected() == 0 {
        anyhow::bail!("File not found");
    }

    Ok(())
}

/// Move a file to a new parent folder.
pub async fn move_file(
    db: &DbPool,
    workspace_id: &str,
    file_id: &str,
    new_parent_id: Option<&str>,
    sort_order: Option<i32>,
    user_id: &str,
) -> anyhow::Result<()> {
    let now = kyomi_core::sql_compat::now(db.is_postgres());
    let result = if let Some(ref order) = sort_order {
        let sql = format!(
            "UPDATE knowledge_files SET parent_id = $1, sort_order = $2, \
             updated_by = $3, updated_at = {now} \
             WHERE id = $4 AND workspace_id = $5"
        );
        kyomi_core::db_execute!(
            db,
            &sql,
            &new_parent_id as &Option<&str>,
            order,
            &user_id,
            &file_id,
            &workspace_id
        )?
    } else {
        let sql = format!(
            "UPDATE knowledge_files SET parent_id = $1, updated_by = $2, updated_at = {now} \
             WHERE id = $3 AND workspace_id = $4"
        );
        kyomi_core::db_execute!(
            db,
            &sql,
            &new_parent_id as &Option<&str>,
            &user_id,
            &file_id,
            &workspace_id
        )?
    };

    if result.rows_affected() == 0 {
        anyhow::bail!("File not found");
    }

    Ok(())
}

/// Update only the sort_order of a file, preserving its current parent_id.
pub async fn update_sort_order(
    db: &DbPool,
    workspace_id: &str,
    file_id: &str,
    sort_order: i32,
    user_id: &str,
) -> anyhow::Result<()> {
    let now = kyomi_core::sql_compat::now(db.is_postgres());
    let sql = format!(
        "UPDATE knowledge_files SET sort_order = $1, updated_by = $2, updated_at = {now} \
         WHERE id = $3 AND workspace_id = $4"
    );
    let result = kyomi_core::db_execute!(
        db,
        &sql,
        &sort_order,
        &user_id,
        &file_id,
        &workspace_id
    )?;

    if result.rows_affected() == 0 {
        anyhow::bail!("File not found");
    }

    Ok(())
}

/// Delete a file or folder (CASCADE handles children and chunks).
pub async fn delete_file(
    db: &DbPool,
    workspace_id: &str,
    file_id: &str,
) -> anyhow::Result<()> {
    let result = kyomi_core::db_execute!(
        db,
        "DELETE FROM knowledge_files WHERE id = $1 AND workspace_id = $2",
        &file_id,
        &workspace_id
    )?;

    if result.rows_affected() == 0 {
        anyhow::bail!("File not found");
    }

    Ok(())
}

/// Fetch content for multiple files by ID in a single query.
///
/// Returns a map of file_id → content for all matching files that have content.
/// Uses `= ANY($2)` on Postgres and individual `IN` placeholders on SQLite.
pub async fn get_files_content_by_ids(
    db: &DbPool,
    workspace_id: &str,
    ids: &[&str],
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    #[derive(sqlx::FromRow)]
    struct IdContent {
        id: String,
        content: Option<String>,
    }

    let is_pg = db.is_postgres();
    let id_col = cast_to_text(is_pg, "id");
    let rows: Vec<IdContent> = match db {
        DbPool::Postgres(pg) => {
            let id_vec: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
            let sql = format!(
                "SELECT {id_col} AS id, content FROM knowledge_files \
                 WHERE workspace_id = $1 AND id = ANY($2) AND content IS NOT NULL"
            );
            sqlx::query_as::<_, IdContent>(&sql)
                .bind(workspace_id)
                .bind(&id_vec)
                .fetch_all(pg)
                .await?
        }
        DbPool::Sqlite(sq) => {
            // Build IN clause with individual placeholders: ($2, $3, ...)
            let placeholders: Vec<String> =
                (0..ids.len()).map(|i| format!("${}", i + 2)).collect();
            let in_clause = placeholders.join(", ");
            let sql = format!(
                "SELECT {id_col} AS id, content FROM knowledge_files \
                 WHERE workspace_id = $1 AND id IN ({in_clause}) AND content IS NOT NULL"
            );
            let mut query = sqlx::query_as::<_, IdContent>(&sql).bind(workspace_id);
            for id in ids {
                query = query.bind(*id);
            }
            query.fetch_all(sq).await?
        }
    };

    let map = rows
        .into_iter()
        .filter_map(|r| r.content.map(|c| (r.id, c)))
        .collect();

    Ok(map)
}

/// Search files by name or content (ILIKE).
///
/// Returns up to 50 matching files with a content preview (first 200 chars).
pub async fn search_files(
    db: &DbPool,
    workspace_id: &str,
    query: &str,
) -> anyhow::Result<Vec<KnowledgeFileSearchResult>> {
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let is_pg = db.is_postgres();
    let search_cols = search_result_columns(is_pg);
    let name_match = kyomi_core::sql_compat::ilike(is_pg, "name", "$2");
    let content_match = kyomi_core::sql_compat::ilike(is_pg, "content", "$2");
    let sql = format!(
        "SELECT {search_cols}, SUBSTR(content, 1, 200) as content_preview \
         FROM knowledge_files \
         WHERE workspace_id = $1 \
           AND ({name_match} ESCAPE '\\' OR {content_match} ESCAPE '\\') \
         ORDER BY name \
         LIMIT 50"
    );
    let results = kyomi_core::db_fetch_all!(
        db,
        KnowledgeFileSearchResult,
        &sql,
        &workspace_id,
        &pattern
    )?;

    Ok(results)
}

/// List all files/folders in a workspace as a flat list (client builds tree).
pub async fn list_tree(
    db: &DbPool,
    workspace_id: &str,
) -> anyhow::Result<Vec<KnowledgeFileTreeEntry>> {
    let tree_cols = tree_entry_columns(db.is_postgres());
    let sql = format!(
        "SELECT {tree_cols} FROM knowledge_files \
         WHERE workspace_id = $1 \
         ORDER BY sort_order, name"
    );
    let entries = kyomi_core::db_fetch_all!(
        db,
        KnowledgeFileTreeEntry,
        &sql,
        &workspace_id
    )?;

    Ok(entries)
}

/// Ensure parent folders exist for a given path, creating them if needed.
///
/// Returns the parent_id for the leaf file (None if root-level).
pub async fn ensure_parent_folders(
    db: &DbPool,
    workspace_id: &str,
    path: &str,
    user_id: &str,
) -> anyhow::Result<Option<String>> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 1 {
        return Ok(None); // root level, no parent needed
    }

    let mut current_parent: Option<String> = None;
    let is_pg = db.is_postgres();
    let cols = knowledge_file_columns(is_pg);
    let bt = kyomi_core::sql_compat::bool_true(is_pg);
    let sql_folder_with_parent = format!(
        "SELECT {cols} FROM knowledge_files \
         WHERE workspace_id = $1 AND parent_id = $2 AND name = $3 AND is_folder = {bt}"
    );
    let sql_folder_null_parent = format!(
        "SELECT {cols} FROM knowledge_files \
         WHERE workspace_id = $1 AND parent_id IS NULL AND name = $2 AND is_folder = {bt}"
    );

    // Walk all but the last part (those are folder names)
    for folder_name in &parts[..parts.len() - 1] {
        // Check if folder exists
        let existing = if current_parent.is_some() {
            kyomi_core::db_fetch_optional!(
                db,
                KnowledgeFile,
                &sql_folder_with_parent,
                &workspace_id,
                &current_parent as &Option<String>,
                folder_name
            )?
        } else {
            kyomi_core::db_fetch_optional!(
                db,
                KnowledgeFile,
                &sql_folder_null_parent,
                &workspace_id,
                folder_name
            )?
        };

        match existing {
            Some(folder) => {
                current_parent = Some(folder.id);
            }
            None => {
                // Create the folder, using ON CONFLICT DO NOTHING for concurrent safety.
                // Two partial unique indexes cover the NULL/non-NULL parent_id cases.
                let new_id = Uuid::new_v4().to_string();
                if current_parent.is_some() {
                    let sql = format!(
                        "INSERT INTO knowledge_files \
                            (id, workspace_id, parent_id, name, is_folder, created_by, updated_by) \
                         VALUES ($1, $2, $3, $4, {bt}, $5, $5) \
                         ON CONFLICT (workspace_id, parent_id, name) WHERE parent_id IS NOT NULL \
                         DO NOTHING"
                    );
                    kyomi_core::db_execute!(
                        db,
                        &sql,
                        &new_id,
                        &workspace_id,
                        &current_parent as &Option<String>,
                        folder_name,
                        &user_id
                    )?;
                } else {
                    let sql = format!(
                        "INSERT INTO knowledge_files \
                            (id, workspace_id, parent_id, name, is_folder, created_by, updated_by) \
                         VALUES ($1, $2, $3, $4, {bt}, $5, $5) \
                         ON CONFLICT (workspace_id, name) WHERE parent_id IS NULL \
                         DO NOTHING"
                    );
                    kyomi_core::db_execute!(
                        db,
                        &sql,
                        &new_id,
                        &workspace_id,
                        &current_parent as &Option<String>,
                        folder_name,
                        &user_id
                    )?;
                }

                // SELECT the folder id regardless of whether our INSERT or a
                // concurrent INSERT won the race.
                let folder = if current_parent.is_some() {
                    kyomi_core::db_fetch_one!(
                        db,
                        KnowledgeFile,
                        &sql_folder_with_parent,
                        &workspace_id,
                        &current_parent as &Option<String>,
                        folder_name
                    )?
                } else {
                    kyomi_core::db_fetch_one!(
                        db,
                        KnowledgeFile,
                        &sql_folder_null_parent,
                        &workspace_id,
                        folder_name
                    )?
                };
                current_parent = Some(folder.id);
            }
        }
    }

    Ok(current_parent)
}

// ---------------------------------------------------------------------------
// Chunking + embedding
// ---------------------------------------------------------------------------

/// Target chunk size in characters (~500 tokens).
const CHUNK_SIZE: usize = 2000;
/// Overlap between adjacent chunks in characters (~100 tokens).
const CHUNK_OVERLAP: usize = 400;

/// Delete old chunks, split content, embed, and insert new chunks.
///
/// Also extracts table references into `knowledge_file_tables`.
pub(crate) async fn rechunk_file(
    db: &DbPool,
    embed: &EmbeddingService,
    file_id: &str,
    content: &str,
    workspace_id: &str,
) -> anyhow::Result<()> {
    if content.trim().is_empty() {
        // No content — just delete old chunks and table refs
        kyomi_core::db_execute!(
            db,
            "DELETE FROM knowledge_chunks WHERE file_id = $1",
            &file_id
        )?;
        kyomi_core::db_execute!(
            db,
            "DELETE FROM knowledge_file_tables WHERE file_id = $1",
            &file_id
        )?;
        return Ok(());
    }

    // Split into chunks
    let chunks = split_into_chunks(content, CHUNK_SIZE, CHUNK_OVERLAP);

    if chunks.is_empty() {
        return Ok(());
    }

    // Embed BEFORE deleting old chunks — if embedding fails, old chunks remain intact
    let chunk_refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
    let embeddings = embed.embed_passages(&chunk_refs)?;

    anyhow::ensure!(
        embeddings.len() == chunks.len(),
        "BUG: embedding count {} != chunk count {}",
        embeddings.len(),
        chunks.len()
    );

    // Now safe to delete old chunks and table references
    kyomi_core::db_execute!(
        db,
        "DELETE FROM knowledge_chunks WHERE file_id = $1",
        &file_id
    )?;
    kyomi_core::db_execute!(
        db,
        "DELETE FROM knowledge_file_tables WHERE file_id = $1",
        &file_id
    )?;

    // Insert chunks with embeddings (different binding for Pg vs SQLite)
    for (i, (chunk_text, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
        let chunk_id = Uuid::new_v4().to_string();

        match db {
            DbPool::Postgres(pg) => {
                let vector = pgvector::Vector::from(embedding.clone());
                sqlx::query(
                    "INSERT INTO knowledge_chunks \
                        (id, file_id, workspace_id, content, chunk_index, embedding) \
                     VALUES ($1, $2, $3, $4, $5, $6::vector)",
                )
                .bind(&chunk_id)
                .bind(file_id)
                .bind(workspace_id)
                .bind(chunk_text)
                .bind(i as i32)
                .bind(&vector)
                .execute(pg)
                .await?;
            }
            DbPool::Sqlite(sq) => {
                let emb_bytes = kyomi_core::embedding_compat::embedding_to_bytes(embedding);
                sqlx::query(
                    "INSERT INTO knowledge_chunks \
                        (id, file_id, workspace_id, content, chunk_index, embedding) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(&chunk_id)
                .bind(file_id)
                .bind(workspace_id)
                .bind(chunk_text)
                .bind(i as i32)
                .bind(&emb_bytes)
                .execute(sq)
                .await?;
            }
        }
    }

    // Extract and store table references
    let table_refs = extract_table_references(content);
    for table_ref in &table_refs {
        // ON CONFLICT to handle duplicates gracefully
        kyomi_core::db_execute!(
            db,
            "INSERT INTO knowledge_file_tables (file_id, workspace_id, table_full_name) \
             VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING",
            &file_id,
            &workspace_id,
            &table_ref
        )?;
    }

    tracing::debug!(
        file_id,
        chunks = chunks.len(),
        table_refs = table_refs.len(),
        "Rechunked knowledge file"
    );

    Ok(())
}

/// Split text into fixed-size chunks with overlap.
pub fn split_into_chunks(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }
    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let mut end = (start + chunk_size).min(text.len());

        // Back up to a valid UTF-8 char boundary
        while !text.is_char_boundary(end) && end > start {
            end -= 1;
        }

        // Try to break at a paragraph or sentence boundary
        let chunk_end = if end < text.len() {
            find_break_point(text, start, end)
        } else {
            end
        };

        chunks.push(text[start..chunk_end].to_string());

        if chunk_end >= text.len() {
            break;
        }

        // Next chunk starts at (end - overlap), but never before current start + 1
        let next_start = if chunk_end > overlap {
            chunk_end - overlap
        } else {
            chunk_end
        };

        if next_start <= start {
            // Safety: always advance
            start = chunk_end;
        } else {
            start = next_start;
        }
    }

    chunks
}

/// Find a good break point near `target_end` within the text.
/// Prefers paragraph breaks (\n\n), then line breaks (\n), then sentence ends.
fn find_break_point(text: &str, start: usize, target_end: usize) -> usize {
    let search_start = target_end.saturating_sub(200).max(start);
    let segment = &text[search_start..target_end];

    // Prefer paragraph break
    if let Some(pos) = segment.rfind("\n\n") {
        return search_start + pos + 2;
    }

    // Then line break
    if let Some(pos) = segment.rfind('\n') {
        return search_start + pos + 1;
    }

    // Then sentence end
    if let Some(pos) = segment.rfind(". ") {
        return search_start + pos + 2;
    }

    // Fall back to target_end
    target_end
}

/// Extract table references from content.
///
/// Looks for backtick-wrapped identifiers matching `word.word` pattern
/// (at least one dot, no spaces). E.g., `` `billing.subscriptions` `` matches.
pub fn extract_table_references(content: &str) -> Vec<String> {
    let re = &*TABLE_REF_RE;
    let mut refs: Vec<String> = re
        .captures_iter(content)
        .map(|cap| cap[1].to_string())
        .collect();

    refs.sort();
    refs.dedup();
    refs
}

// ---------------------------------------------------------------------------
// Knowledge tree text for system prompt
// ---------------------------------------------------------------------------

/// Build the `<knowledge_tree>` XML block for the system prompt.
///
/// Returns an empty string if no files exist.
pub async fn build_knowledge_tree_text(
    db: &DbPool,
    workspace_id: &str,
) -> anyhow::Result<String> {
    let entries = list_tree(db, workspace_id).await?;

    if entries.is_empty() {
        return Ok(String::new());
    }

    // Build a tree structure from the flat list
    use std::collections::HashMap;

    let mut children_map: HashMap<Option<&str>, Vec<&KnowledgeFileTreeEntry>> = HashMap::new();
    for entry in &entries {
        children_map
            .entry(entry.parent_id.as_deref())
            .or_default()
            .push(entry);
    }

    let mut output = String::from("<knowledge_tree>\n");
    render_tree_level(&children_map, None, 0, MAX_TREE_DEPTH, &mut output);
    output.push_str("</knowledge_tree>");

    Ok(output)
}

/// Maximum recursion depth for tree rendering.
const MAX_TREE_DEPTH: usize = 20;

/// Recursively render a tree level as indented text.
fn render_tree_level(
    children_map: &std::collections::HashMap<Option<&str>, Vec<&KnowledgeFileTreeEntry>>,
    parent_id: Option<&str>,
    depth: usize,
    max_depth: usize,
    output: &mut String,
) {
    let Some(children) = children_map.get(&parent_id) else {
        return;
    };

    if depth >= max_depth {
        let indent = "   ".repeat(depth);
        output.push_str(&format!("{indent}...\n"));
        return;
    }

    let indent = "   ".repeat(depth);
    for entry in children {
        if entry.is_folder {
            output.push_str(&format!("{indent}\u{1F4C1} {}\n", entry.name));
            render_tree_level(children_map, Some(&entry.id), depth + 1, max_depth, output);
        } else {
            output.push_str(&format!("{indent}\u{1F4C4} {}\n", entry.name));
        }
    }
}

// ---------------------------------------------------------------------------
// File path reconstruction
// ---------------------------------------------------------------------------

/// Reconstruct the full path of a file (e.g., `Revenue/Metrics.md`).
pub async fn get_file_path(
    db: &DbPool,
    workspace_id: &str,
    file_id: &str,
) -> anyhow::Result<String> {
    // Use a recursive CTE to walk up the parent chain
    let rows = kyomi_core::db_fetch_all!(
        db,
        PathSegmentRow,
        "WITH RECURSIVE ancestors AS ( \
            SELECT id, parent_id, name, 0 AS depth \
            FROM knowledge_files \
            WHERE id = $1 AND workspace_id = $2 \
          UNION ALL \
            SELECT kf.id, kf.parent_id, kf.name, a.depth + 1 \
            FROM knowledge_files kf \
            JOIN ancestors a ON kf.id = a.parent_id \
            WHERE kf.workspace_id = $2 \
         ) \
         SELECT name FROM ancestors ORDER BY depth DESC",
        &file_id,
        &workspace_id
    )?;

    let path: Vec<String> = rows.into_iter().map(|r| r.name).collect();
    Ok(path.join("/"))
}

#[derive(sqlx::FromRow)]
struct PathSegmentRow {
    name: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- extract_table_references tests --

    #[test]
    fn extract_simple_table_ref() {
        let content = "The data is in `billing.subscriptions` table.";
        let refs = extract_table_references(content);
        assert_eq!(refs, vec!["billing.subscriptions"]);
    }

    #[test]
    fn extract_three_part_table_ref() {
        let content = "Query `project.dataset.orders` for results.";
        let refs = extract_table_references(content);
        assert_eq!(refs, vec!["project.dataset.orders"]);
    }

    #[test]
    fn extract_multiple_refs_deduped() {
        let content = "Join `billing.subscriptions` with `billing.invoices`. \
                        Also check `billing.subscriptions` again.";
        let refs = extract_table_references(content);
        assert_eq!(refs, vec!["billing.invoices", "billing.subscriptions"]);
    }

    #[test]
    fn no_refs_for_plain_backtick_words() {
        let content = "The `amount` column is in cents. Use `status = 'active'`.";
        let refs = extract_table_references(content);
        assert!(refs.is_empty());
    }

    #[test]
    fn no_refs_for_code_blocks() {
        // Backtick-wrapped identifiers inside code blocks should still be caught
        // since we're doing simple regex matching (this is by design).
        let content = "```sql\nSELECT * FROM `public.orders`\n```";
        let refs = extract_table_references(content);
        assert_eq!(refs, vec!["public.orders"]);
    }

    // -- split_into_chunks tests --

    #[test]
    fn short_text_single_chunk() {
        let text = "Hello world";
        let chunks = split_into_chunks(text, 2000, 400);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world");
    }

    #[test]
    fn long_text_multiple_chunks() {
        // Create text that's definitely longer than chunk size
        let text = "A ".repeat(1500); // 3000 chars
        let chunks = split_into_chunks(&text, 2000, 400);
        assert!(chunks.len() >= 2, "Expected >= 2 chunks, got {}", chunks.len());
    }

    #[test]
    fn chunks_cover_all_content() {
        let text = "Word. ".repeat(500); // 3000 chars
        let chunks = split_into_chunks(&text, 2000, 400);
        // Verify no content is lost: the first chunk's start and last chunk's end
        // should cover the original text
        assert!(chunks[0].starts_with("Word. "));
        assert!(chunks.last().unwrap().ends_with("Word. "));
    }

    #[test]
    fn empty_text_no_chunks() {
        let chunks = split_into_chunks("", 2000, 400);
        let expected: Vec<String> = vec![];
        assert_eq!(chunks, expected);
    }

    // -- hash_content tests --

    #[test]
    fn hash_is_deterministic() {
        let h1 = hash_content("test content");
        let h2 = hash_content("test content");
        assert_eq!(h1, h2);
    }

    #[test]
    fn different_content_different_hash() {
        let h1 = hash_content("content a");
        let h2 = hash_content("content b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_is_16_hex_chars() {
        let h = hash_content("hello");
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
