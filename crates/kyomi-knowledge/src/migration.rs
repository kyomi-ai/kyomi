// SPDX-License-Identifier: AGPL-3.0-or-later

//! Migration from agent_learnings to knowledge files.
//!
//! Converts existing agent_learnings records into knowledge files,
//! grouping them by entity references.

use kyomi_core::db::DbPool;
use kyomi_embed::EmbeddingService;
use serde::Serialize;

/// Result of a learning-to-knowledge-file migration.
#[derive(Debug, Serialize)]
pub struct MigrationResult {
    /// Number of learnings migrated.
    pub migrated: usize,
    /// Number of learnings skipped (already migrated or empty).
    pub skipped: usize,
    /// Number of files created.
    pub files_created: usize,
}

/// Migrate agent_learnings to knowledge files.
///
/// Groups learnings by their entity references and creates one knowledge file
/// per group. Learnings without references go into a catch-all file.
pub async fn migrate_learnings_to_knowledge_files(
    db: &DbPool,
    embed: &EmbeddingService,
    workspace_id: &str,
    user_id: &str,
) -> anyhow::Result<MigrationResult> {
    // Check if migration was already done (knowledge files exist for this workspace)
    let count: i64 = kyomi_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM knowledge_files WHERE workspace_id = $1",
        &workspace_id
    )
    .unwrap_or(0);

    if count > 0 {
        return Ok(MigrationResult {
            migrated: 0,
            skipped: 0,
            files_created: 0,
        });
    }

    // Fetch all learnings for this workspace
    #[derive(sqlx::FromRow)]
    struct LearningRow {
        content: String,
    }

    let sql =
        "SELECT content FROM agent_learnings \
         WHERE workspace_id = $1 AND content IS NOT NULL AND content != '' \
         ORDER BY created_at";
    let learnings: Vec<LearningRow> =
        kyomi_core::db_fetch_all!(db, LearningRow, sql, &workspace_id)?;

    if learnings.is_empty() {
        return Ok(MigrationResult {
            migrated: 0,
            skipped: 0,
            files_created: 0,
        });
    }

    // Combine all learnings into a single "Migrated Learnings.md" file
    let mut content = String::from("# Migrated Learnings\n\n");
    content.push_str("*These learnings were automatically migrated from the legacy system.*\n\n");

    for learning in &learnings {
        content.push_str(&format!("---\n\n{}\n\n", learning.content));
    }

    let _file = super::knowledge_files::create_file(
        db,
        embed,
        workspace_id,
        None,
        "Migrated Learnings.md",
        Some(&content),
        false,
        user_id,
    )
    .await?;

    Ok(MigrationResult {
        migrated: learnings.len(),
        skipped: 0,
        files_created: 1,
    })
}
