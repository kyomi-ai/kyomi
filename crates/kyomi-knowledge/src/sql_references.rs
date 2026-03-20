// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL reference extraction -- parse SQL to find table references.
//!
//! Uses `sqlparser` to parse agent-generated SQL (BigQuery, Postgres dialects)
//! and extract table names (FROM, JOIN).
//!
//! Ported from `kyomi-graph::sql_references` to be self-contained in this crate.

use sqlparser::ast::{visit_relations, Statement};
use sqlparser::dialect::{BigQueryDialect, GenericDialect, PostgreSqlDialect};
use sqlparser::parser::Parser;
use std::collections::HashSet;

/// References extracted from a SQL query.
#[derive(Debug, Clone, Default)]
pub struct SqlReferences {
    pub tables: Vec<String>,
}

/// Extract table references from a SQL query string.
///
/// Tries BigQuery dialect first (handles backtick-quoted names), then Postgres,
/// then generic. Returns empty references on parse failure.
pub fn extract_references(sql: &str) -> SqlReferences {
    let statements = Parser::parse_sql(&BigQueryDialect {}, sql)
        .or_else(|_| Parser::parse_sql(&PostgreSqlDialect {}, sql))
        .or_else(|_| Parser::parse_sql(&GenericDialect {}, sql));

    let statements = match statements {
        Ok(stmts) => stmts,
        Err(_) => return SqlReferences::default(),
    };

    let mut tables = HashSet::new();

    for stmt in &statements {
        extract_from_statement(stmt, &mut tables);
    }

    SqlReferences {
        tables: tables.into_iter().collect(),
    }
}

fn extract_from_statement(stmt: &Statement, tables: &mut HashSet<String>) {
    let _ = visit_relations(stmt, |relation| {
        let parts: Vec<&str> = relation
            .0
            .iter()
            .filter_map(|part| part.as_ident().map(|id| id.value.as_str()))
            .collect();
        if !parts.is_empty() {
            tables.insert(parts.join("."));
        }
        core::ops::ControlFlow::<()>::Continue(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_select() {
        let sql = "SELECT name, email FROM users WHERE active = true";
        let refs = extract_references(sql);
        assert!(
            refs.tables.contains(&"users".to_string()),
            "tables: {:?}",
            refs.tables
        );
    }

    #[test]
    fn join_query() {
        let sql =
            "SELECT o.id, c.name FROM orders o JOIN customers c ON o.customer_id = c.id";
        let refs = extract_references(sql);
        assert!(
            refs.tables.contains(&"orders".to_string()),
            "tables: {:?}",
            refs.tables
        );
        assert!(
            refs.tables.contains(&"customers".to_string()),
            "tables: {:?}",
            refs.tables
        );
    }

    #[test]
    fn bigquery_multipart_names() {
        let sql = "SELECT amount, status FROM `myproject.billing.subscriptions` WHERE status != 'cancelled'";
        let refs = extract_references(sql);
        assert!(
            refs.tables
                .contains(&"myproject.billing.subscriptions".to_string()),
            "tables: {:?}",
            refs.tables
        );
    }

    #[test]
    fn invalid_sql_returns_empty() {
        let sql = "NOT VALID SQL AT ALL !!!";
        let refs = extract_references(sql);
        assert!(refs.tables.is_empty());
    }
}
