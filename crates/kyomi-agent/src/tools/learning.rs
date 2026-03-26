// SPDX-License-Identifier: AGPL-3.0-or-later

//! Learning tools — save and search workspace knowledge.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// SaveLearningTool
// ---------------------------------------------------------------------------

/// Save knowledge about how to navigate the data warehouse.
///
/// Calls Claude Haiku to audit the learning before saving (duplicate/quality check).
pub struct SaveLearningTool;

#[async_trait]
impl AgentTool for SaveLearningTool {
    fn name(&self) -> &str {
        "save_learning"
    }

    fn description(&self) -> &str {
        "Save knowledge about how to navigate the data warehouse. NOT for saving \
         analysis results or business insights. \
         IMPORTANT: If your learning mentions specific tables or columns, you MUST: \
         (1) verify they exist using get_table_info first, \
         (2) include related_tables with fully qualified names (e.g. [\"public.orders\"]), \
         (3) include related_columns for key columns (e.g. [\"public.orders.total_amount\"]). \
         Learnings referencing tables without this metadata will be rejected."
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(false),
            ..Default::default()
        })
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "insight": {
                    "type": "string",
                    "description": "What you learned"
                },
                "context": {
                    "type": "string",
                    "description": "What taught you this"
                },
                "scope": {
                    "type": "string",
                    "enum": ["workspace", "user"],
                    "default": "workspace"
                },
                "learning_type": {
                    "type": "string",
                    "enum": ["learning", "metric", "preference"],
                    "default": "learning",
                    "description": "Type of knowledge: 'learning' for data navigation, 'metric' for metric definitions, 'preference' for user preferences (e.g., chart style, formatting)"
                },
                "supersedes_learning_id": {
                    "type": "string",
                    "description": "ID of learning to replace"
                },
                "datasource": {
                    "type": "string",
                    "description": "Datasource slug if specific"
                },
                "reference_queries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "sql": { "type": "string" },
                            "datasource": { "type": "string" },
                            "comment": { "type": "string" }
                        },
                        "required": ["sql"]
                    }
                },
                "related_tables": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Fully qualified table names referenced (e.g. [\"public.orders\", \"sales.transactions\"])"
                },
                "related_columns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Important columns in table.column format (e.g. [\"public.orders.total_amount\"])"
                },
                "related_metrics": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Metric names discussed (e.g. [\"MRR\", \"churn_rate\"])"
                },
                "metric_name": {
                    "type": "string",
                    "description": "Canonical metric name (when learning_type=\"metric\")"
                },
                "metric_formula": {
                    "type": "string",
                    "description": "How the metric is calculated (when learning_type=\"metric\")"
                },
                "metric_unit": {
                    "type": "string",
                    "description": "Unit of measurement: USD, %, count, etc. (when learning_type=\"metric\")"
                }
            },
            "required": ["insight"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let insight = args
            .get("insight")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'insight'".into())
            })?;
        let context = args.get("context").and_then(|v| v.as_str());
        let scope = args
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("workspace");
        let raw_learning_type = args
            .get("learning_type")
            .and_then(|v| v.as_str())
            .unwrap_or("learning");
        // Map old learning_type values to simplified enum for backward compat
        let learning_type = match raw_learning_type {
            "navigation" | "event_context" => "learning",
            other => other,
        };
        let supersedes_id = args
            .get("supersedes_learning_id")
            .and_then(|v| v.as_str());
        let datasource_slug = args.get("datasource").and_then(|v| v.as_str());
        let reference_queries = args.get("reference_queries");

        // Collect structured metadata for graph edge creation
        let structured_metadata = {
            let mut meta = serde_json::Map::new();
            if let Some(tables) = args.get("related_tables")
                && tables.is_array() && !tables.as_array().unwrap().is_empty()
            {
                meta.insert("related_tables".into(), tables.clone());
            }
            if let Some(columns) = args.get("related_columns")
                && columns.is_array() && !columns.as_array().unwrap().is_empty()
            {
                meta.insert("related_columns".into(), columns.clone());
            }
            if let Some(metrics) = args.get("related_metrics")
                && metrics.is_array() && !metrics.as_array().unwrap().is_empty()
            {
                meta.insert("related_metrics".into(), metrics.clone());
            }
            if let Some(name) = args.get("metric_name").and_then(|v| v.as_str())
                && !name.is_empty()
            {
                meta.insert("metric_name".into(), serde_json::Value::String(name.to_string()));
            }
            if let Some(formula) = args.get("metric_formula").and_then(|v| v.as_str())
                && !formula.is_empty()
            {
                meta.insert("metric_formula".into(), serde_json::Value::String(formula.to_string()));
            }
            if let Some(unit) = args.get("metric_unit").and_then(|v| v.as_str())
                && !unit.is_empty()
            {
                meta.insert("metric_unit".into(), serde_json::Value::String(unit.to_string()));
            }
            if meta.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(meta))
            }
        };

        // Resolve datasource slug to ID if provided
        let datasource_config_id = if let Some(slug) = datasource_slug {
            let ds = kyomi_auth::datasource_service::resolve_datasource(
                &ctx.db,
                slug,
                &ctx.workspace_id,
                false,
            )
            .await?;
            Some(ds.id)
        } else {
            None
        };

        // ------------------------------------------------------------------
        // Catalog validation: verify related_tables / related_columns / SQL
        // references actually exist in datasource_table_cache.
        // Skip for preferences (they don't reference tables).
        // ------------------------------------------------------------------
        if learning_type != "preference" {
            // Collect explicitly provided table names
            let related_tables: Vec<String> = args
                .get("related_tables")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            // Collect explicitly provided column references (table.column)
            let related_columns: Vec<String> = args
                .get("related_columns")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            // Extract table references from reference_queries SQL
            let mut sql_tables: Vec<String> = Vec::new();
            if let Some(ref_queries) = args.get("reference_queries").and_then(|v| v.as_array()) {
                for query_val in ref_queries {
                    if let Some(sql) = query_val.get("sql").and_then(|v| v.as_str()) {
                        let refs = kyomi_knowledge::sql_references::extract_references(sql);
                        sql_tables.extend(refs.tables);
                    }
                }
            }

            // Only validate if there's something to check
            let has_tables = !related_tables.is_empty() || !sql_tables.is_empty();
            let has_columns = !related_columns.is_empty();

            if has_tables || has_columns {
                // Fetch all non-archived table names + metadata for this workspace
                #[derive(sqlx::FromRow)]
                struct CatalogRow {
                    full_name: String,
                    table_metadata: Option<serde_json::Value>,
                }

                let is_pg = ctx.db.is_postgres();
                let bool_false = kyomi_core::sql_compat::bool_false(is_pg);
                let catalog_sql = format!(
                    "SELECT \
                        CONCAT_WS('.', NULLIF(tc.project_id, ''), NULLIF(tc.dataset_id, ''), tc.table_id) AS full_name, \
                        tc.table_metadata \
                    FROM datasource_table_cache tc \
                    WHERE tc.workspace_id = $1 AND tc.is_archived = {bool_false}"
                );
                let catalog_rows: Vec<CatalogRow> = kyomi_core::db_fetch_all!(
                    ctx.db, CatalogRow,
                    &catalog_sql,
                    &ctx.workspace_id
                )
                .unwrap_or_default();

                // Build lookup: lowercase full_name -> columns set
                let table_names: HashSet<String> = catalog_rows
                    .iter()
                    .map(|r| r.full_name.to_lowercase())
                    .collect();

                let table_columns: HashMap<String, HashSet<String>> = catalog_rows
                    .iter()
                    .map(|r| {
                        let cols: HashSet<String> = r
                            .table_metadata
                            .as_ref()
                            .and_then(|m| m.get("columns"))
                            .and_then(|c| c.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|col| {
                                        col.get("name")
                                            .and_then(|n| n.as_str())
                                            .map(|s| s.to_lowercase())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        (r.full_name.to_lowercase(), cols)
                    })
                    .collect();

                // Helper: check if a name matches any catalog table
                // Supports exact match and suffix match (e.g. "orders" matches "public.orders")
                let table_exists = |name: &str| -> bool {
                    let lower = name.to_lowercase();
                    if table_names.contains(&lower) {
                        return true;
                    }
                    // Suffix match: "orders" matches "public.orders"
                    let suffix = format!(".{}", lower);
                    table_names.iter().any(|t| t.ends_with(&suffix))
                };

                // Resolve a name to its canonical catalog name
                let resolve_table = |name: &str| -> Option<String> {
                    let lower = name.to_lowercase();
                    if table_names.contains(&lower) {
                        return Some(lower);
                    }
                    let suffix = format!(".{}", lower);
                    table_names.iter().find(|t| t.ends_with(&suffix)).cloned()
                };

                // Validate related_tables
                let mut missing_tables: Vec<String> = Vec::new();
                for table in &related_tables {
                    if !table_exists(table) {
                        missing_tables.push(table.clone());
                    }
                }

                // Validate SQL-extracted tables
                for table in &sql_tables {
                    if !table_exists(table) && !missing_tables.contains(table) {
                        missing_tables.push(table.clone());
                    }
                }

                // Validate related_columns (format: table.column or schema.table.column)
                let mut missing_columns: Vec<String> = Vec::new();
                for col_ref in &related_columns {
                    // Split at last '.' to get (table_part, column_name)
                    if let Some(dot_pos) = col_ref.rfind('.') {
                        let table_part = &col_ref[..dot_pos];
                        let col_name = &col_ref[dot_pos + 1..];

                        if let Some(canonical) = resolve_table(table_part) {
                            // Table exists — check column
                            if let Some(cols) = table_columns.get(&canonical) {
                                if !cols.contains(&col_name.to_lowercase()) {
                                    missing_columns.push(col_ref.clone());
                                }
                            } else {
                                missing_columns.push(col_ref.clone());
                            }
                        } else {
                            // Table itself doesn't exist; report as missing column
                            // (the table will also be in missing_tables if it was in related_tables)
                            missing_columns.push(col_ref.clone());
                        }
                    } else {
                        // No dot — invalid column reference format
                        missing_columns.push(col_ref.clone());
                    }
                }

                if !missing_tables.is_empty() || !missing_columns.is_empty() {
                    let mut reason_parts: Vec<String> = Vec::new();
                    if !missing_tables.is_empty() {
                        reason_parts.push(format!(
                            "The following tables were not found in the catalog: [{}]",
                            missing_tables.join(", ")
                        ));
                    }
                    if !missing_columns.is_empty() {
                        reason_parts.push(format!(
                            "The following columns were not found: [{}]",
                            missing_columns.join(", ")
                        ));
                    }
                    reason_parts
                        .push("Use get_table_info to verify table and column names before saving.".to_string());
                    let reason = reason_parts.join(". ");

                    return Ok(serde_json::json!({
                        "success": false,
                        "rejected": true,
                        "reason": reason,
                        "message": "Learning rejected: referenced tables/columns not found in catalog",
                    })
                    .to_string());
                }
            }
        }

        // LLM audit: check for duplicates and validate
        let mut similar = kyomi_auth::learning_service::get_relevant_learnings_hybrid(
            &ctx.db,
            ctx.embedding.wait_ready().await?,
            &ctx.workspace_id,
            insight,
            Some(&ctx.user_id),
            5,
            0.5,
            0.7,
            0.3,
        )
        .await
        .unwrap_or_default();

        // Exclude the learning being superseded from duplicate detection —
        // the caller explicitly intends to replace it.
        if let Some(old_id) = supersedes_id {
            similar.retain(|l| l.learning.learning_id != old_id);
        }

        let existing_text = if similar.is_empty() {
            "No similar learnings found.".to_string()
        } else {
            similar
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    format!(
                        "{}. [{}] (sim={:.2}): {}",
                        i + 1,
                        l.learning.learning_id,
                        l.similarity,
                        l.learning.insight
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let context_str = context.unwrap_or("None");

        // Compute metadata counts for the audit prompt
        let table_count = args
            .get("related_tables")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let column_count = args
            .get("related_columns")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let has_ref_queries = args
            .get("reference_queries")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());

        let audit_prompt = format!(
            "You are a strict learning auditor. Evaluate if this learning should be saved.\n\n\
             PROPOSED LEARNING:\n\"{insight}\"\n\n\
             CONTEXT: {context_str}\n\
             LEARNING TYPE: {learning_type}\n\
             STRUCTURED METADATA: {table_count} related tables, {column_count} related columns, \
             reference_queries: {has_ref_queries}\n\n\
             EXISTING SIMILAR LEARNINGS:\n{existing_text}\n\n\
             APPROVE if:\n\
             - It's data navigation knowledge (which tables, fields, query patterns)\n\
             - It's a metric definition (how a metric is calculated)\n\
             - It's a user preference (chart style, formatting, visualization preferences)\n\
             - It's a user correction about their data warehouse\n\n\
             REJECT if:\n\
             - It's a session-specific context or analysis result\n\
             - It duplicates an existing learning (similarity >= 0.8)\n\
             - It's general programming knowledge, not specific to this workspace\n\
             - It describes what data shows, not how to find/present data\n\
             - It mentions specific table or column names (like schema.table_name) but \
             related_tables/related_columns metadata was not provided (0 tables, 0 columns) \
             and no reference_queries with SQL were included\n\n\
             Respond with EXACTLY one line:\nAPPROVED\nor\nREJECTED: <reason>"
        );

        // Call a cheap/fast model for audit
        let mut audit_config = crate::resolve_provider_config(&ctx.config)?;
        // For Anthropic, override to the cheap audit model; for others, use default.
        if audit_config.provider == crate::ProviderKind::Anthropic {
            audit_config.model = Some(crate::AUDIT_MODEL.to_string());
        }
        let audit_client = crate::create_provider(audit_config)?;

        let audit_messages = vec![crate::types::Message::user(&audit_prompt)];
        let audit_response = audit_client
            .complete(
                &audit_messages,
                &[],
                None,
                256,
                &std::collections::HashMap::new(),
            )
            .await?;

        let verdict = audit_response.content.trim().to_string();
        if verdict.starts_with("REJECTED") {
            let reason = verdict
                .strip_prefix("REJECTED:")
                .map(|s| s.trim())
                .unwrap_or("No reason given");
            return Ok(serde_json::json!({
                "success": false,
                "rejected": true,
                "reason": reason,
                "message": format!("Learning rejected: {reason}"),
            })
            .to_string());
        }

        // Save the learning
        let session_id = ctx.session_id.as_deref().unwrap_or("");
        let learning_id = kyomi_auth::learning_service::save_learning(
            &ctx.db,
            ctx.embedding.wait_ready().await?,
            &ctx.workspace_id,
            &ctx.user_id,
            session_id,
            insight,
            context,
            scope,
            datasource_config_id.as_deref(),
            learning_type,
            reference_queries,
            structured_metadata.as_ref(),
        )
        .await?;

        // Handle supersedes
        if let Some(old_id) = supersedes_id {
            let _supersede_result = kyomi_auth::learning_service::supersede_learning(
                &ctx.db,
                old_id,
                &learning_id,
                &ctx.workspace_id,
            )
            .await;

            // No graph cleanup needed — cascade deletes handle learning_references
            // and the old learning's embedding stays until the row is deleted.
        }

        // Populate embedding + references for the newly saved learning (fire-and-forget)
        if let Ok(embed) = ctx.embedding.wait_ready().await {
            match kyomi_knowledge::populate::populate_learning_embedding(
                &ctx.db,
                embed,
                &learning_id,
            )
            .await
            {
                Ok(()) => {
                    tracing::debug!(learning_id = %learning_id, "Learning embedding populated");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Learning embedding population failed, continuing"
                    );
                }
            }

            // Materialize learning references (table/column/metric edges)
            if let Err(e) = kyomi_knowledge::references::materialize_learning_references(
                &ctx.db,
                &learning_id,
                &ctx.workspace_id,
                None,
            )
            .await
            {
                tracing::warn!(
                    error = %e,
                    "Learning reference materialization failed, continuing"
                );
            }
        }

        Ok(serde_json::json!({
            "success": true,
            "learning_id": learning_id,
            "message": "Learning saved successfully",
        })
        .to_string())
    }
}

