use crate::transform::definition::{Source, Strategy, TransformDef};

/// Generate the ClickHouse type for a Materialized View column.
///
/// With `order_by` set, `earliest`/`latest` use `AggregateFunction(argMin/argMax, T, OB_TYPE)`
/// because they must track both the value and the ordering column.
/// `OB_TYPE` must match the actual ClickHouse type of the `order_by` column (e.g., DateTime64(3)).
/// Without `order_by`, they use `SimpleAggregateFunction(any/anyLast, T)`.
///
/// `count()` and `sum()` use `SimpleAggregateFunction(sum, T)`.
/// `min()` and `max()` use `SimpleAggregateFunction(min/max, T)`.
pub fn mv_column_type(strategy: &Strategy, ch_type: &str, order_by_type: &Option<String>) -> String {
    match strategy {
        Strategy::Earliest(_) => {
            if let Some(ob_type) = order_by_type {
                format!("AggregateFunction(argMin, {ch_type}, {ob_type})")
            } else {
                format!("SimpleAggregateFunction(any, {ch_type})")
            }
        }
        Strategy::Latest(_) => {
            if let Some(ob_type) = order_by_type {
                format!("AggregateFunction(argMax, {ch_type}, {ob_type})")
            } else {
                format!("SimpleAggregateFunction(anyLast, {ch_type})")
            }
        }
        Strategy::Count => "SimpleAggregateFunction(sum, UInt64)".to_string(),
        Strategy::Sum(_) => format!("SimpleAggregateFunction(sum, {ch_type})"),
        Strategy::Min(_) => format!("SimpleAggregateFunction(min, {ch_type})"),
        Strategy::Max(_) => format!("SimpleAggregateFunction(max, {ch_type})"),
    }
}

/// Generate the SELECT expression for a column in the Materialized View.
pub fn mv_select_expr(
    strategy: &Strategy,
    source: &Source,
    order_by_field: &Option<String>,
) -> String {
    let source_expr = source_to_sql(source);

    let raw_expr = match strategy {
        Strategy::Earliest(_) => {
            if let Some(ob) = order_by_field {
                format!("argMinState({source_expr}, {ob})")
            } else {
                format!("any({source_expr})")
            }
        }
        Strategy::Latest(_) => {
            if let Some(ob) = order_by_field {
                format!("argMaxState({source_expr}, {ob})")
            } else {
                format!("anyLast({source_expr})")
            }
        }
        Strategy::Count => "count()".to_string(),
        Strategy::Sum(_) => format!("sum({source_expr})"),
        Strategy::Min(_) => format!("min({source_expr})"),
        Strategy::Max(_) => format!("max({source_expr})"),
    };

    raw_expr
}

/// Generate the SELECT expression for a column in the public View.
///
/// `AggregateFunction` columns (argMin/argMax with order_by) use `-Merge` combinators.
/// `SimpleAggregateFunction` columns use plain aggregate functions — ClickHouse does
/// NOT support `-Merge` on SimpleAggregateFunction types.
pub fn view_select_expr(strategy: &Strategy, col_name: &str, order_by: &Option<String>) -> String {
    match strategy {
        Strategy::Earliest(_) => {
            if order_by.is_some() {
                format!("argMinMerge({col_name}) AS {col_name}")
            } else {
                format!("any({col_name}) AS {col_name}")
            }
        }
        Strategy::Latest(_) => {
            if order_by.is_some() {
                format!("argMaxMerge({col_name}) AS {col_name}")
            } else {
                format!("anyLast({col_name}) AS {col_name}")
            }
        }
        Strategy::Count => format!("sum({col_name}) AS {col_name}"),
        Strategy::Sum(_) => format!("sum({col_name}) AS {col_name}"),
        Strategy::Min(_) => format!("min({col_name}) AS {col_name}"),
        Strategy::Max(_) => format!("max({col_name}) AS {col_name}"),
    }
}

/// Convert a Source to its SQL expression in the MV SELECT.
fn source_to_sql(source: &Source) -> String {
    match source {
        Source::Field(name) => name.clone(),
        Source::JsonExtract { field: _, path } => {
            // Path format: $.key — strip the $. prefix for ClickHouse's JSONExtractString
            let json_key = path.strip_prefix("$.").unwrap_or(path);
            format!("JSONExtractString(properties, '{json_key}')")
        }
    }
}

/// Extract the Source from a Strategy (for strategies that have one).
fn strategy_source(strategy: &Strategy) -> Option<&Source> {
    match strategy {
        Strategy::Earliest(s)
        | Strategy::Latest(s)
        | Strategy::Sum(s)
        | Strategy::Min(s)
        | Strategy::Max(s) => Some(s),
        Strategy::Count => None,
    }
}

/// Generate CREATE MATERIALIZED VIEW DDL.
///
/// Creates `_{name}` as a Materialized View with AggregatingMergeTree engine,
/// triggered by INSERT to the source table (e.g., `events`).
pub fn create_mv_ddl(database: &str, def: &TransformDef) -> String {
    let mv_name = format!("{database}._{}", def.name);
    let source_table = format!("{database}.{}", def.source);
    let key = &def.key;

    // Build column definitions for the MV engine
    let mut col_defs = vec![format!("    {key} String")];
    for col in &def.columns {
        let col_type = mv_column_type(&col.strategy, &col.ch_type, &def.order_by_type);
        col_defs.push(format!("    {} {}", col.name, col_type));
    }

    // Build SELECT expressions
    let mut select_exprs = vec![key.clone()];
    for col in &def.columns {
        let source = strategy_source(&col.strategy)
            .cloned()
            .unwrap_or(Source::Field(String::new()));
        let expr = mv_select_expr(&col.strategy, &source, &def.order_by);
        select_exprs.push(format!("{expr} AS {}", col.name));
    }

    format!(
        "CREATE MATERIALIZED VIEW IF NOT EXISTS {mv_name}\n(\n{}\n)\n\
         ENGINE = AggregatingMergeTree()\n\
         ORDER BY ({key})\n\
         AS SELECT\n    {}\n\
         FROM {source_table}\n\
         GROUP BY {key}",
        col_defs.join(",\n"),
        select_exprs.join(",\n    "),
    )
}

/// Generate CREATE OR REPLACE VIEW DDL for the public view.
///
/// Reads from `_{name}` with `-Merge` combinators + GROUP BY.
pub fn create_view_ddl(database: &str, def: &TransformDef) -> String {
    let view_name = format!("{database}.{}", def.name);
    let mv_name = format!("{database}._{}", def.name);
    let key = &def.key;

    let mut select_exprs = vec![key.clone()];
    for col in &def.columns {
        select_exprs.push(view_select_expr(&col.strategy, &col.name, &def.order_by));
    }

    format!(
        "CREATE OR REPLACE VIEW {view_name} AS\n\
         SELECT\n    {}\n\
         FROM {mv_name}\n\
         GROUP BY {key}",
        select_exprs.join(",\n    "),
    )
}

/// Generate INSERT ... SELECT for backfilling an MV after creation.
///
/// Reads directly from the source table and aggregates into the MV's storage.
pub fn backfill_sql(database: &str, def: &TransformDef) -> String {
    let mv_name = format!("{database}._{}", def.name);
    let source_table = format!("{database}.{}", def.source);
    let key = &def.key;

    let mut select_exprs = vec![key.clone()];
    for col in &def.columns {
        let source = strategy_source(&col.strategy)
            .cloned()
            .unwrap_or(Source::Field(String::new()));
        let expr = mv_select_expr(&col.strategy, &source, &def.order_by);
        select_exprs.push(expr);
    }

    format!(
        "INSERT INTO {mv_name}\n\
         SELECT\n    {}\n\
         FROM {source_table}\n\
         GROUP BY {key}",
        select_exprs.join(",\n    "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::definition::*;

    fn test_def() -> TransformDef {
        TransformDef {
            name: "sessions".into(),
            source: "events".into(),
            key: "session_id".into(),
            order_by: Some("timestamp".into()),
            order_by_type: Some("DateTime64(3)".into()),
            columns: vec![
                ColumnDef {
                    name: "visitor_id".into(),
                    ch_type: "String".into(),
                    strategy: Strategy::Earliest(Source::Field("visitor_id".into())),
                },
                ColumnDef {
                    name: "started_at".into(),
                    ch_type: "DateTime64(3)".into(),
                    strategy: Strategy::Min(Source::Field("timestamp".into())),
                },
                ColumnDef {
                    name: "exit_page".into(),
                    ch_type: "String".into(),
                    strategy: Strategy::Latest(Source::Field("pathname".into())),
                },
                ColumnDef {
                    name: "events".into(),
                    ch_type: "UInt64".into(),
                    strategy: Strategy::Count,
                },
                ColumnDef {
                    name: "screen_width".into(),
                    ch_type: "UInt16".into(),
                    strategy: Strategy::Earliest(Source::Field("screen_width".into())),
                },
            ],
        }
    }

    fn test_def_no_order_by() -> TransformDef {
        TransformDef {
            name: "sessions".into(),
            source: "events".into(),
            key: "session_id".into(),
            order_by: None,
            order_by_type: None,
            columns: vec![
                ColumnDef {
                    name: "entry_page".into(),
                    ch_type: "String".into(),
                    strategy: Strategy::Earliest(Source::Field("pathname".into())),
                },
                ColumnDef {
                    name: "exit_page".into(),
                    ch_type: "String".into(),
                    strategy: Strategy::Latest(Source::Field("pathname".into())),
                },
            ],
        }
    }

    // --- mv_column_type tests ---

    #[test]
    fn test_mv_column_type_earliest_with_order_by() {
        let order_by_type = Some("DateTime64(3)".into());
        assert_eq!(
            mv_column_type(&Strategy::Earliest(Source::Field("x".into())), "String", &order_by_type),
            "AggregateFunction(argMin, String, DateTime64(3))"
        );
    }

    #[test]
    fn test_mv_column_type_earliest_without_order_by() {
        assert_eq!(
            mv_column_type(&Strategy::Earliest(Source::Field("x".into())), "String", &None),
            "SimpleAggregateFunction(any, String)"
        );
    }

    #[test]
    fn test_mv_column_type_latest_with_order_by() {
        let order_by_type = Some("DateTime64(3)".into());
        assert_eq!(
            mv_column_type(&Strategy::Latest(Source::Field("x".into())), "String", &order_by_type),
            "AggregateFunction(argMax, String, DateTime64(3))"
        );
    }

    #[test]
    fn test_mv_column_type_latest_without_order_by() {
        assert_eq!(
            mv_column_type(&Strategy::Latest(Source::Field("x".into())), "String", &None),
            "SimpleAggregateFunction(anyLast, String)"
        );
    }

    #[test]
    fn test_mv_column_type_count() {
        assert_eq!(
            mv_column_type(&Strategy::Count, "UInt64", &None),
            "SimpleAggregateFunction(sum, UInt64)"
        );
    }

    #[test]
    fn test_mv_column_type_sum() {
        assert_eq!(
            mv_column_type(&Strategy::Sum(Source::Field("x".into())), "UInt64", &None),
            "SimpleAggregateFunction(sum, UInt64)"
        );
    }

    #[test]
    fn test_mv_column_type_min() {
        assert_eq!(
            mv_column_type(&Strategy::Min(Source::Field("x".into())), "DateTime64(3)", &None),
            "SimpleAggregateFunction(min, DateTime64(3))"
        );
    }

    #[test]
    fn test_mv_column_type_max() {
        assert_eq!(
            mv_column_type(&Strategy::Max(Source::Field("x".into())), "UInt64", &None),
            "SimpleAggregateFunction(max, UInt64)"
        );
    }

    // --- mv_select_expr tests ---

    #[test]
    fn test_mv_select_expr_earliest_with_order_by() {
        let ob = Some("timestamp".into());
        let expr = mv_select_expr(
            &Strategy::Earliest(Source::Field("visitor_id".into())),
            &Source::Field("visitor_id".into()),
            &ob,
        );
        assert_eq!(expr, "argMinState(visitor_id, timestamp)");
    }

    #[test]
    fn test_mv_select_expr_earliest_without_order_by() {
        let expr = mv_select_expr(
            &Strategy::Earliest(Source::Field("visitor_id".into())),
            &Source::Field("visitor_id".into()),
            &None,
        );
        assert_eq!(expr, "any(visitor_id)");
    }

    #[test]
    fn test_mv_select_expr_latest_with_order_by() {
        let ob = Some("timestamp".into());
        let expr = mv_select_expr(
            &Strategy::Latest(Source::Field("pathname".into())),
            &Source::Field("pathname".into()),
            &ob,
        );
        assert_eq!(expr, "argMaxState(pathname, timestamp)");
    }

    #[test]
    fn test_mv_select_expr_count() {
        let expr = mv_select_expr(
            &Strategy::Count,
            &Source::Field(String::new()),
            &None,
        );
        assert_eq!(expr, "count()");
    }

    #[test]
    fn test_mv_select_expr_min_datetime64() {
        let expr = mv_select_expr(
            &Strategy::Min(Source::Field("timestamp".into())),
            &Source::Field("timestamp".into()),
            &None,
        );
        assert_eq!(expr, "min(timestamp)");
    }

    #[test]
    fn test_mv_select_expr_max_datetime64() {
        let expr = mv_select_expr(
            &Strategy::Max(Source::Field("timestamp".into())),
            &Source::Field("timestamp".into()),
            &None,
        );
        assert_eq!(expr, "max(timestamp)");
    }

    #[test]
    fn test_mv_select_expr_min_non_datetime() {
        let expr = mv_select_expr(
            &Strategy::Min(Source::Field("amount".into())),
            &Source::Field("amount".into()),
            &None,
        );
        assert_eq!(expr, "min(amount)");
    }

    #[test]
    fn test_mv_select_expr_sum() {
        let expr = mv_select_expr(
            &Strategy::Sum(Source::Field("amount".into())),
            &Source::Field("amount".into()),
            &None,
        );
        assert_eq!(expr, "sum(amount)");
    }

    #[test]
    fn test_mv_select_expr_json_extract() {
        let ob = Some("timestamp".into());
        let source = Source::JsonExtract {
            field: "properties".into(),
            path: "$.plan".into(),
        };
        let expr = mv_select_expr(
            &Strategy::Earliest(source.clone()),
            &source,
            &ob,
        );
        assert_eq!(expr, "argMinState(JSONExtractString(properties, 'plan'), timestamp)");
    }

    // --- view_select_expr tests ---

    #[test]
    fn test_view_select_expr_earliest_with_order_by() {
        let ob = Some("timestamp".into());
        let expr = view_select_expr(
            &Strategy::Earliest(Source::Field("x".into())),
            "visitor_id",
            &ob,
        );
        assert_eq!(expr, "argMinMerge(visitor_id) AS visitor_id");
    }

    #[test]
    fn test_view_select_expr_earliest_without_order_by() {
        let expr = view_select_expr(
            &Strategy::Earliest(Source::Field("x".into())),
            "visitor_id",
            &None,
        );
        assert_eq!(expr, "any(visitor_id) AS visitor_id");
    }

    #[test]
    fn test_view_select_expr_latest_with_order_by() {
        let ob = Some("timestamp".into());
        let expr = view_select_expr(
            &Strategy::Latest(Source::Field("x".into())),
            "exit_page",
            &ob,
        );
        assert_eq!(expr, "argMaxMerge(exit_page) AS exit_page");
    }

    #[test]
    fn test_view_select_expr_count() {
        let expr = view_select_expr(&Strategy::Count, "events", &None);
        assert_eq!(expr, "sum(events) AS events");
    }

    #[test]
    fn test_view_select_expr_min() {
        let expr = view_select_expr(
            &Strategy::Min(Source::Field("x".into())),
            "started_at",
            &None,
        );
        assert_eq!(expr, "min(started_at) AS started_at");
    }

    // --- create_mv_ddl tests ---

    #[test]
    fn test_create_mv_ddl() {
        let ddl = create_mv_ddl("site_abc", &test_def());
        assert!(ddl.contains("CREATE MATERIALIZED VIEW IF NOT EXISTS site_abc._sessions"));
        assert!(ddl.contains("ENGINE = AggregatingMergeTree()"));
        assert!(ddl.contains("ORDER BY (session_id)"));
        // Explicit column type declarations
        assert!(ddl.contains("session_id String"));
        assert!(ddl.contains("visitor_id AggregateFunction(argMin, String, DateTime64(3))"));
        assert!(ddl.contains("started_at SimpleAggregateFunction(min, DateTime64(3))"));
        assert!(ddl.contains("exit_page AggregateFunction(argMax, String, DateTime64(3))"));
        assert!(ddl.contains("events SimpleAggregateFunction(sum, UInt64)"));
        assert!(ddl.contains("screen_width AggregateFunction(argMin, UInt16, DateTime64(3))"));
        // SELECT expressions
        assert!(ddl.contains("argMinState(visitor_id, timestamp) AS visitor_id"));
        assert!(ddl.contains("min(timestamp) AS started_at"));
        assert!(ddl.contains("argMaxState(pathname, timestamp) AS exit_page"));
        assert!(ddl.contains("count() AS events"));
        assert!(ddl.contains("FROM site_abc.events"));
        assert!(ddl.contains("GROUP BY session_id"));
    }

    #[test]
    fn test_create_mv_ddl_no_order_by() {
        let ddl = create_mv_ddl("site_abc", &test_def_no_order_by());
        // Explicit column type declarations
        assert!(ddl.contains("entry_page SimpleAggregateFunction(any, String)"));
        assert!(ddl.contains("exit_page SimpleAggregateFunction(anyLast, String)"));
        // SELECT expressions
        assert!(ddl.contains("any(pathname) AS entry_page"));
        assert!(ddl.contains("anyLast(pathname) AS exit_page"));
    }

    #[test]
    fn test_create_mv_ddl_column_defs_before_engine() {
        // Regression: col_defs must appear between MV name and ENGINE.
        // Without this, ClickHouse infers plain types (UInt64) instead of
        // SimpleAggregateFunction types, causing data corruption on merge.
        let ddl = create_mv_ddl("site_abc", &test_def());
        let engine_pos = ddl.find("ENGINE").expect("DDL must contain ENGINE");
        let col_def_pos = ddl.find("events SimpleAggregateFunction(sum, UInt64)")
            .expect("DDL must contain explicit events column type");
        assert!(col_def_pos < engine_pos, "Column definitions must appear before ENGINE");
    }

    // --- create_view_ddl tests ---

    #[test]
    fn test_create_view_ddl() {
        let ddl = create_view_ddl("site_abc", &test_def());
        assert!(ddl.contains("CREATE OR REPLACE VIEW site_abc.sessions"));
        assert!(ddl.contains("FROM site_abc._sessions"));
        assert!(ddl.contains("argMinMerge(visitor_id) AS visitor_id"));
        assert!(ddl.contains("min(started_at) AS started_at"));
        assert!(ddl.contains("argMaxMerge(exit_page) AS exit_page"));
        assert!(ddl.contains("sum(events) AS events"));
        assert!(ddl.contains("GROUP BY session_id"));
    }

    #[test]
    fn test_create_view_ddl_no_order_by() {
        let ddl = create_view_ddl("site_abc", &test_def_no_order_by());
        assert!(ddl.contains("any(entry_page) AS entry_page"));
        assert!(ddl.contains("anyLast(exit_page) AS exit_page"));
    }

    // --- backfill_sql tests ---

    #[test]
    fn test_backfill_sql() {
        let sql = backfill_sql("site_abc", &test_def());
        assert!(sql.contains("INSERT INTO site_abc._sessions"));
        assert!(sql.contains("FROM site_abc.events"));
        assert!(sql.contains("argMinState(visitor_id, timestamp)"));
        assert!(sql.contains("min(timestamp)"));
        assert!(sql.contains("argMaxState(pathname, timestamp)"));
        assert!(sql.contains("count()"));
        assert!(sql.contains("GROUP BY session_id"));
    }
}
