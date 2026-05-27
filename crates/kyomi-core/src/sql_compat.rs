// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL dialect helpers for Postgres vs SQLite compatibility.
//!
//! These functions return SQL fragments appropriate for the active database backend.
//! All take a `is_pg: bool` parameter — `true` for Postgres, `false` for SQLite.

/// SQL expression for current timestamp.
/// Postgres: `NOW()`, SQLite: `datetime('now')`
pub fn now(is_pg: bool) -> &'static str {
    if is_pg { "NOW()" } else { "datetime('now')" }
}

/// Cast expression to text.
/// Postgres: `expr::text`, SQLite: `expr` (TEXT columns don't need casting)
pub fn cast_to_text(is_pg: bool, expr: &str) -> String {
    if is_pg {
        format!("{expr}::text")
    } else {
        expr.to_string()
    }
}

/// Boolean literal for use in SQL strings.
/// Postgres: `TRUE`/`FALSE`, SQLite: `1`/`0`
pub fn bool_true(is_pg: bool) -> &'static str {
    if is_pg { "TRUE" } else { "1" }
}

/// Boolean literal `FALSE` / `0` for use in SQL strings.
pub fn bool_false(is_pg: bool) -> &'static str {
    if is_pg { "FALSE" } else { "0" }
}

/// Interval subtraction: column < NOW() - N days.
/// The `param` argument is a SQL bind parameter placeholder (e.g. `$1`).
/// Postgres: `column < NOW() - make_interval(days => $1::int4)`
/// SQLite: `column < datetime('now', '-' || $1 || ' days')`
///
/// The `::int4` cast is required because `make_interval(days => ...)` expects
/// `int4`, but the retention_days parameter is bound as `i64` (bigint). Without
/// the explicit cast, Postgres 15 raises a type mismatch error at runtime.
pub fn ago_days(is_pg: bool, column: &str, param: &str) -> String {
    if is_pg {
        format!("{column} < NOW() - make_interval(days => {param}::int4)")
    } else {
        format!("{column} < datetime('now', '-' || {param} || ' days')")
    }
}

/// UUID type cast.
/// Postgres: `$1::uuid`, SQLite: `$1` (UUIDs stored as TEXT)
pub fn cast_to_uuid(is_pg: bool, param: &str) -> String {
    if is_pg {
        format!("{param}::uuid")
    } else {
        param.to_string()
    }
}

/// JSON type cast.
/// Postgres: `$1::json`, SQLite: `$1` (JSON stored as TEXT)
pub fn cast_to_json(is_pg: bool, param: &str) -> String {
    if is_pg {
        format!("{param}::json")
    } else {
        param.to_string()
    }
}

/// ILIKE (case-insensitive LIKE).
/// Postgres: `column ILIKE $1`, SQLite: `column LIKE $1` (SQLite LIKE is case-insensitive for ASCII)
pub fn ilike(is_pg: bool, column: &str, param: &str) -> String {
    if is_pg {
        format!("{column} ILIKE {param}")
    } else {
        format!("{column} LIKE {param}")
    }
}

/// Coalesce with timestamp default.
/// For expressions like `COALESCE(column, NOW())`.
pub fn coalesce_now(is_pg: bool, column: &str) -> String {
    format!("COALESCE({column}, {})", now(is_pg))
}

/// Array contains check.
/// Postgres: `$1 = ANY(column)`, SQLite: `$1 IN (SELECT value FROM json_each(column))`
/// Note: SQLite arrays are stored as JSON arrays; uses json_each for proper matching.
pub fn any_in_array(is_pg: bool, param: &str, column: &str) -> String {
    if is_pg {
        format!("{param} = ANY({column})")
    } else {
        format!("{param} IN (SELECT value FROM json_each({column}))")
    }
}

/// Timestamp comparison: is the column older than N seconds ago?
/// Postgres: `column < NOW() - INTERVAL 'N seconds'`
/// SQLite: `column < datetime('now', '-N seconds')`
pub fn ago_seconds(is_pg: bool, column: &str, seconds: i64) -> String {
    if is_pg {
        format!("{column} < NOW() - INTERVAL '{seconds} seconds'")
    } else {
        format!("{column} < datetime('now', '-{seconds} seconds')")
    }
}

/// Timestamp comparison with bind parameter for seconds.
/// Postgres: `column < NOW() - make_interval(secs => $1::double precision)`
/// SQLite: `column < datetime('now', '-' || $1 || ' seconds')`
///
/// The Postgres `make_interval(secs => ...)` requires `double precision`.
pub fn ago_seconds_param(is_pg: bool, column: &str, param: &str) -> String {
    if is_pg {
        format!("{column} < NOW() - make_interval(secs => {param}::double precision)")
    } else {
        format!("{column} < datetime('now', '-' || {param} || ' seconds')")
    }
}

/// Timestamp addition: column + N hours.
/// Used for expiry calculations.
/// Postgres: `column + INTERVAL 'N hours'`
/// SQLite: `datetime(column, '+N hours')`
pub fn add_hours(is_pg: bool, column: &str, hours: i64) -> String {
    if is_pg {
        format!("{column} + INTERVAL '{hours} hours'")
    } else {
        format!("datetime({column}, '+{hours} hours')")
    }
}

/// Timestamp addition: column + N days.
/// Postgres: `column + INTERVAL 'N days'`
/// SQLite: `datetime(column, '+N days')`
pub fn add_days(is_pg: bool, column: &str, days: i64) -> String {
    if is_pg {
        format!("{column} + INTERVAL '{days} days'")
    } else {
        format!("datetime({column}, '+{days} days')")
    }
}

/// JSON field extraction as text.
/// Postgres: `column->>'field'`
/// SQLite: `json_extract(column, '$.field')`
///
/// # Safety
///
/// `field` should be a programmer-supplied identifier (never user input).
/// Single quotes are escaped as defense-in-depth.
pub fn json_extract_text(is_pg: bool, column: &str, field: &str) -> String {
    let escaped = field.replace('\'', "''");
    if is_pg {
        format!("{column}->>'{escaped}'")
    } else {
        format!("json_extract({column}, '$.{escaped}')")
    }
}

/// Build a dotted full table name expression: `project.dataset.table`
/// Skips empty components (avoids leading dots).
///
/// Postgres: `CONCAT_WS('.', NULLIF(project_id, ''), NULLIF(dataset_id, ''), table_id)`
/// SQLite: string concatenation with CASE to skip empty parts.
pub fn full_table_name_expr(is_pg: bool) -> &'static str {
    if is_pg {
        "CONCAT_WS('.', NULLIF(project_id, ''), NULLIF(dataset_id, ''), table_id)"
    } else {
        "CASE WHEN project_id != '' THEN project_id || '.' ELSE '' END || CASE WHEN dataset_id != '' THEN dataset_id || '.' ELSE '' END || table_id"
    }
}

/// Same as [`full_table_name_expr`] but with a table alias prefix on the columns.
///
/// E.g. `full_table_name_expr_prefixed(true, "dtc")` produces
/// `CONCAT_WS('.', NULLIF(dtc.project_id, ''), NULLIF(dtc.dataset_id, ''), dtc.table_id)`.
pub fn full_table_name_expr_prefixed(is_pg: bool, prefix: &str) -> String {
    if is_pg {
        format!(
            "CONCAT_WS('.', NULLIF({prefix}.project_id, ''), NULLIF({prefix}.dataset_id, ''), {prefix}.table_id)"
        )
    } else {
        format!(
            "CASE WHEN {prefix}.project_id != '' THEN {prefix}.project_id || '.' ELSE '' END || CASE WHEN {prefix}.dataset_id != '' THEN {prefix}.dataset_id || '.' ELSE '' END || {prefix}.table_id"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now_postgres() {
        assert_eq!(now(true), "NOW()");
    }

    #[test]
    fn test_now_sqlite() {
        assert_eq!(now(false), "datetime('now')");
    }

    #[test]
    fn test_cast_to_text_postgres() {
        assert_eq!(cast_to_text(true, "column"), "column::text");
    }

    #[test]
    fn test_cast_to_text_sqlite() {
        assert_eq!(cast_to_text(false, "column"), "column");
    }

    #[test]
    fn test_bool_literals() {
        assert_eq!(bool_true(true), "TRUE");
        assert_eq!(bool_true(false), "1");
        assert_eq!(bool_false(true), "FALSE");
        assert_eq!(bool_false(false), "0");
    }

    #[test]
    fn test_ago_days_postgres() {
        assert_eq!(
            ago_days(true, "created_at", "$1"),
            "created_at < NOW() - make_interval(days => $1::int4)"
        );
    }

    #[test]
    fn test_ago_days_sqlite() {
        assert_eq!(
            ago_days(false, "created_at", "$1"),
            "created_at < datetime('now', '-' || $1 || ' days')"
        );
    }

    #[test]
    fn test_cast_to_uuid() {
        assert_eq!(cast_to_uuid(true, "$1"), "$1::uuid");
        assert_eq!(cast_to_uuid(false, "$1"), "$1");
    }

    #[test]
    fn test_cast_to_json() {
        assert_eq!(cast_to_json(true, "$1"), "$1::json");
        assert_eq!(cast_to_json(false, "$1"), "$1");
    }

    #[test]
    fn test_ilike() {
        assert_eq!(ilike(true, "name", "$1"), "name ILIKE $1");
        assert_eq!(ilike(false, "name", "$1"), "name LIKE $1");
    }

    #[test]
    fn test_json_extract_text() {
        assert_eq!(json_extract_text(true, "config", "host"), "config->>'host'");
        assert_eq!(json_extract_text(false, "config", "host"), "json_extract(config, '$.host')");
    }

    #[test]
    fn test_any_in_array() {
        assert_eq!(any_in_array(true, "$1", "tags"), "$1 = ANY(tags)");
        assert_eq!(any_in_array(false, "$1", "tags"), "$1 IN (SELECT value FROM json_each(tags))");
    }

    #[test]
    fn test_add_hours() {
        assert_eq!(add_hours(true, "created_at", 24), "created_at + INTERVAL '24 hours'");
        assert_eq!(add_hours(false, "created_at", 24), "datetime(created_at, '+24 hours')");
    }

    #[test]
    fn test_add_days() {
        assert_eq!(add_days(true, "expires_at", 30), "expires_at + INTERVAL '30 days'");
        assert_eq!(add_days(false, "expires_at", 30), "datetime(expires_at, '+30 days')");
    }

    #[test]
    fn test_coalesce_now() {
        assert_eq!(coalesce_now(true, "col"), "COALESCE(col, NOW())");
        assert_eq!(coalesce_now(false, "col"), "COALESCE(col, datetime('now'))");
    }

    #[test]
    fn test_ago_seconds() {
        assert_eq!(ago_seconds(true, "ts", 60), "ts < NOW() - INTERVAL '60 seconds'");
        assert_eq!(ago_seconds(false, "ts", 60), "ts < datetime('now', '-60 seconds')");
    }

    #[test]
    fn test_ago_seconds_param() {
        assert_eq!(ago_seconds_param(true, "ts", "$1"), "ts < NOW() - make_interval(secs => $1::double precision)");
        assert_eq!(ago_seconds_param(false, "ts", "$1"), "ts < datetime('now', '-' || $1 || ' seconds')");
    }

    #[test]
    fn test_full_table_name_expr() {
        assert_eq!(
            full_table_name_expr(true),
            "CONCAT_WS('.', NULLIF(project_id, ''), NULLIF(dataset_id, ''), table_id)"
        );
        assert!(full_table_name_expr(false).contains("CASE WHEN project_id"));
    }

    #[test]
    fn test_full_table_name_expr_prefixed() {
        let pg = full_table_name_expr_prefixed(true, "tc");
        assert!(pg.contains("tc.project_id"));
        assert!(pg.contains("tc.dataset_id"));
        assert!(pg.contains("tc.table_id"));

        let sqlite = full_table_name_expr_prefixed(false, "tc");
        assert!(sqlite.contains("tc.project_id"));
        assert!(sqlite.contains("tc.table_id"));
    }
}
