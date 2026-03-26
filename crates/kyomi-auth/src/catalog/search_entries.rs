// SPDX-License-Identifier: AGPL-3.0-or-later

//! Search entry generation — 4-tier weighted entries for semantic search.
//!
//! Mirrors Python's `BaseSQLCatalogIndexer._create_search_entries()`.

use super::types::{ColumnEntry, SearchEntry};

/// Search entry weights — controls ranking priority in semantic search.
pub const WEIGHT_SCHEMA_TABLE: f64 = 1.0;
pub const WEIGHT_TABLE_NAME: f64 = 0.9;
pub const WEIGHT_COLUMN_NAME: f64 = 0.6;
pub const WEIGHT_COLUMN_DESCRIPTION: f64 = 0.4;

/// Generate search entries for a table and its columns.
///
/// Creates a multi-tier set of search entries with different weights:
/// 1. "schema_table" (weight 1.0) — full qualified name like "schema.table"
/// 2. "table_name" (weight 0.9) — table name alone
/// 3. "column_name" (weight 0.6) — individual column names
/// 4. "column_description" (weight 0.4) — column descriptions (if available)
///
/// The `dataset_id` is the container name (schema, database, dataset).
/// The `table_id` is the full qualified table identifier.
pub fn create_search_entries(
    dataset_id: &str,
    table_name: &str,
    table_id: &str,
    columns: &[ColumnEntry],
) -> Vec<SearchEntry> {
    let mut entries = Vec::new();

    // Level 1: schema_table (dataset.table)
    entries.push(SearchEntry {
        text: format!("{dataset_id}.{table_name}"),
        table_id: table_id.to_string(),
        entry_type: "schema_table".into(),
        weight: WEIGHT_SCHEMA_TABLE,
        column_name: None,
    });

    // Level 2: table_name (table name only)
    entries.push(SearchEntry {
        text: table_name.to_string(),
        table_id: table_id.to_string(),
        entry_type: "table_name".into(),
        weight: WEIGHT_TABLE_NAME,
        column_name: None,
    });

    // Level 3 & 4: columns
    for col in columns {
        // Level 3: column_name
        entries.push(SearchEntry {
            text: col.name.clone(),
            table_id: table_id.to_string(),
            entry_type: "column_name".into(),
            weight: WEIGHT_COLUMN_NAME,
            column_name: Some(col.name.clone()),
        });

        // Level 4: column_description (only if description is present and non-empty)
        if let Some(ref desc) = col.description
            && !desc.trim().is_empty()
        {
            entries.push(SearchEntry {
                text: desc.clone(),
                table_id: table_id.to_string(),
                entry_type: "column_description".into(),
                weight: WEIGHT_COLUMN_DESCRIPTION,
                column_name: Some(col.name.clone()),
            });
        }
    }

    entries
}

/// Compute a schema signature for change detection.
///
/// The signature is a sorted set of (name, type, description) tuples.
/// If the signature hasn't changed since last indexing AND embeddings exist,
/// re-embedding can be skipped.
pub fn compute_schema_signature(columns: &[ColumnEntry]) -> Vec<(String, String, String)> {
    let mut sig: Vec<(String, String, String)> = columns
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                c.col_type.clone().unwrap_or_default(),
                c.description.clone().unwrap_or_default(),
            )
        })
        .collect();
    sig.sort();
    sig
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_entries_for_table_with_columns() {
        let columns = vec![
            ColumnEntry {
                name: "id".into(),
                col_type: Some("INTEGER".into()),
                native_type: None,
                description: None,
            },
            ColumnEntry {
                name: "name".into(),
                col_type: Some("VARCHAR".into()),
                native_type: None,
                description: Some("User's display name".into()),
            },
        ];

        let entries = create_search_entries("public", "users", "public.users", &columns);

        // 2 base entries + 2 column names + 1 column description = 5
        assert_eq!(entries.len(), 5);

        assert_eq!(entries[0].entry_type, "schema_table");
        assert_eq!(entries[0].text, "public.users");
        assert_eq!(entries[0].weight, WEIGHT_SCHEMA_TABLE);

        assert_eq!(entries[1].entry_type, "table_name");
        assert_eq!(entries[1].text, "users");
        assert_eq!(entries[1].weight, WEIGHT_TABLE_NAME);

        assert_eq!(entries[2].entry_type, "column_name");
        assert_eq!(entries[2].text, "id");
        assert_eq!(entries[2].weight, WEIGHT_COLUMN_NAME);

        assert_eq!(entries[3].entry_type, "column_name");
        assert_eq!(entries[3].text, "name");

        assert_eq!(entries[4].entry_type, "column_description");
        assert_eq!(entries[4].text, "User's display name");
        assert_eq!(entries[4].weight, WEIGHT_COLUMN_DESCRIPTION);
    }

    #[test]
    fn skips_empty_descriptions() {
        let columns = vec![ColumnEntry {
            name: "id".into(),
            col_type: Some("INT".into()),
            native_type: None,
            description: Some("  ".into()), // whitespace-only
        }];

        let entries = create_search_entries("public", "t", "public.t", &columns);
        // schema_table + table_name + column_name = 3 (no column_description)
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn schema_signature_is_stable() {
        let cols = vec![
            ColumnEntry {
                name: "b".into(),
                col_type: Some("INT".into()),
                native_type: None,
                description: None,
            },
            ColumnEntry {
                name: "a".into(),
                col_type: Some("VARCHAR".into()),
                native_type: None,
                description: Some("desc".into()),
            },
        ];

        let sig1 = compute_schema_signature(&cols);
        let sig2 = compute_schema_signature(&cols);
        assert_eq!(sig1, sig2);

        // Should be sorted by name
        assert_eq!(sig1[0].0, "a");
        assert_eq!(sig1[1].0, "b");
    }
}
