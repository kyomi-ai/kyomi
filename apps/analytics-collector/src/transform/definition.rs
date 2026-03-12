use std::collections::BTreeMap;
use serde::Deserialize;

/// A parsed transform definition loaded from a YAML file.
#[derive(Debug, Clone)]
pub struct TransformDef {
    pub name: String,
    pub source: String,
    pub key: String,
    /// Event field used for chronological ordering (e.g., "timestamp").
    /// When set, `earliest`/`latest` map to `argMin`/`argMax`.
    /// When absent, they map to `any`/`anyLast`.
    pub order_by: Option<String>,
    /// ClickHouse type of the `order_by` column (e.g., "DateTime64(3)").
    /// Required when `order_by` is set — used in AggregateFunction type declarations.
    pub order_by_type: Option<String>,
    pub columns: Vec<ColumnDef>,
}

/// A single column in a transform.
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub ch_type: String,
    pub strategy: Strategy,
}

/// The field-level strategy for a transform column.
#[derive(Debug, Clone, PartialEq)]
pub enum Strategy {
    /// Capture the first value seen for this key (by order_by field or insertion order).
    Earliest(Source),
    /// Always use the most recent value (by order_by field or insertion order).
    Latest(Source),
    /// Count events per key.
    Count,
    /// Sum a numeric field.
    Sum(Source),
    /// Minimum value of a field.
    Min(Source),
    /// Maximum value of a field.
    Max(Source),
}

/// Where to read the value from an event.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A top-level event field (e.g., "pathname", "visitor_id").
    Field(String),
    /// Extract a key from the properties map: json_extract(field, '$.path').
    JsonExtract { field: String, path: String },
}

/// Raw YAML structure for deserialization.
#[derive(Deserialize)]
struct RawTransformDef {
    transform: String,
    source: String,
    key: String,
    #[serde(default)]
    order_by: Option<String>,
    #[serde(default)]
    order_by_type: Option<String>,
    columns: BTreeMap<String, RawColumnDef>,
}

#[derive(Deserialize)]
struct RawColumnDef {
    #[serde(rename = "type")]
    ch_type: String,
    strategy: String,
}

/// Parse a strategy string like "earliest(pathname)", "count()", "sum(amount)",
/// or "latest(json_extract(properties, '$.plan'))".
fn parse_strategy(s: &str) -> Result<Strategy, String> {
    let s = s.trim();
    if s == "count()" {
        return Ok(Strategy::Count);
    }
    if let Some(inner) = s.strip_prefix("earliest(").and_then(|s| s.strip_suffix(')')) {
        Ok(Strategy::Earliest(parse_source(inner)?))
    } else if let Some(inner) = s.strip_prefix("latest(").and_then(|s| s.strip_suffix(')')) {
        Ok(Strategy::Latest(parse_source(inner)?))
    } else if let Some(inner) = s.strip_prefix("sum(").and_then(|s| s.strip_suffix(')')) {
        Ok(Strategy::Sum(parse_source(inner)?))
    } else if let Some(inner) = s.strip_prefix("min(").and_then(|s| s.strip_suffix(')')) {
        Ok(Strategy::Min(parse_source(inner)?))
    } else if let Some(inner) = s.strip_prefix("max(").and_then(|s| s.strip_suffix(')')) {
        Ok(Strategy::Max(parse_source(inner)?))
    } else {
        Err(format!("Unknown strategy: {s}"))
    }
}

/// Parse a source expression: either a field name or json_extract(field, '$.path').
fn parse_source(s: &str) -> Result<Source, String> {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix("json_extract(").and_then(|s| s.strip_suffix(')')) {
        // Parse: field, '$.path'
        let parts: Vec<&str> = inner.splitn(2, ',').collect();
        if parts.len() != 2 {
            return Err(format!("json_extract requires two arguments: {s}"));
        }
        let field = parts[0].trim().to_string();
        let path = parts[1].trim().trim_matches('\'').to_string();
        Ok(Source::JsonExtract { field, path })
    } else {
        Ok(Source::Field(s.to_string()))
    }
}

/// Load a transform definition from a YAML string.
pub fn parse_transform(yaml: &str) -> Result<TransformDef, String> {
    let raw: RawTransformDef =
        serde_yaml::from_str(yaml).map_err(|e| format!("YAML parse error: {e}"))?;

    let mut columns = Vec::new();
    for (name, col) in raw.columns {
        let strategy = parse_strategy(&col.strategy)?;
        columns.push(ColumnDef {
            name,
            ch_type: col.ch_type,
            strategy,
        });
    }

    Ok(TransformDef {
        name: raw.transform,
        source: raw.source,
        key: raw.key,
        order_by: raw.order_by,
        order_by_type: raw.order_by_type,
        columns,
    })
}

/// Load all transform definitions from YAML files in a directory.
pub fn load_transforms_from_dir(dir: &std::path::Path) -> Result<Vec<TransformDef>, String> {
    let mut transforms = Vec::new();
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("Failed to read transforms directory {}: {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            let yaml = std::fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
            let def = parse_transform(&yaml)?;
            tracing::info!(name = %def.name, key = %def.key, columns = def.columns.len(), "Loaded transform");
            transforms.push(def);
        }
    }

    Ok(transforms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_strategy_earliest_field() {
        let s = parse_strategy("earliest(pathname)").unwrap();
        assert_eq!(s, Strategy::Earliest(Source::Field("pathname".into())));
    }

    #[test]
    fn test_parse_strategy_latest_field() {
        let s = parse_strategy("latest(user_id)").unwrap();
        assert_eq!(s, Strategy::Latest(Source::Field("user_id".into())));
    }

    #[test]
    fn test_parse_strategy_json_extract() {
        let s = parse_strategy("latest(json_extract(properties, '$.plan'))").unwrap();
        assert_eq!(
            s,
            Strategy::Latest(Source::JsonExtract {
                field: "properties".into(),
                path: "$.plan".into(),
            })
        );
    }

    #[test]
    fn test_parse_strategy_earliest_json_extract() {
        let s = parse_strategy("earliest(json_extract(properties, '$.ab_variant'))").unwrap();
        assert_eq!(
            s,
            Strategy::Earliest(Source::JsonExtract {
                field: "properties".into(),
                path: "$.ab_variant".into(),
            })
        );
    }

    #[test]
    fn test_parse_strategy_count() {
        let s = parse_strategy("count()").unwrap();
        assert_eq!(s, Strategy::Count);
    }

    #[test]
    fn test_parse_strategy_sum() {
        let s = parse_strategy("sum(amount)").unwrap();
        assert_eq!(s, Strategy::Sum(Source::Field("amount".into())));
    }

    #[test]
    fn test_parse_strategy_min() {
        let s = parse_strategy("min(timestamp)").unwrap();
        assert_eq!(s, Strategy::Min(Source::Field("timestamp".into())));
    }

    #[test]
    fn test_parse_strategy_max() {
        let s = parse_strategy("max(timestamp)").unwrap();
        assert_eq!(s, Strategy::Max(Source::Field("timestamp".into())));
    }

    #[test]
    fn test_parse_strategy_unknown() {
        assert!(parse_strategy("avg(count)").is_err());
    }

    #[test]
    fn test_parse_sessions_yaml() {
        let yaml = include_str!("../../transforms/sessions.yaml");
        let def = parse_transform(yaml).unwrap();
        assert_eq!(def.name, "sessions");
        assert_eq!(def.source, "events");
        assert_eq!(def.key, "session_id");
        assert_eq!(def.order_by.as_deref(), Some("timestamp"));
        assert_eq!(def.order_by_type.as_deref(), Some("DateTime64(3)"));
        assert!(def.columns.len() > 10);

        // Check entry_page is earliest(pathname)
        let entry_page = def.columns.iter().find(|c| c.name == "entry_page").unwrap();
        assert_eq!(entry_page.strategy, Strategy::Earliest(Source::Field("pathname".into())));
        assert_eq!(entry_page.ch_type, "String");

        // Check exit_page is latest(pathname)
        let exit_page = def.columns.iter().find(|c| c.name == "exit_page").unwrap();
        assert_eq!(exit_page.strategy, Strategy::Latest(Source::Field("pathname".into())));

        // Check events is count()
        let events = def.columns.iter().find(|c| c.name == "events").unwrap();
        assert_eq!(events.strategy, Strategy::Count);

        // Check started_at is min(timestamp)
        let started_at = def.columns.iter().find(|c| c.name == "started_at").unwrap();
        assert_eq!(started_at.strategy, Strategy::Min(Source::Field("timestamp".into())));
    }

    #[test]
    fn test_parse_visitors_yaml() {
        let yaml = include_str!("../../transforms/visitors.yaml");
        let def = parse_transform(yaml).unwrap();
        assert_eq!(def.name, "visitors");
        assert_eq!(def.key, "visitor_id");
        assert_eq!(def.order_by.as_deref(), Some("timestamp"));
        assert_eq!(def.order_by_type.as_deref(), Some("DateTime64(3)"));

        // first_seen is min(timestamp)
        let first_seen = def.columns.iter().find(|c| c.name == "first_seen").unwrap();
        assert_eq!(first_seen.strategy, Strategy::Min(Source::Field("timestamp".into())));

        // browser is latest (current state)
        let browser = def.columns.iter().find(|c| c.name == "browser").unwrap();
        assert_eq!(browser.strategy, Strategy::Latest(Source::Field("browser".into())));

        // events is count()
        let events = def.columns.iter().find(|c| c.name == "events").unwrap();
        assert_eq!(events.strategy, Strategy::Count);
    }

    #[test]
    fn test_parse_transform_with_order_by() {
        let yaml = r#"
transform: test
source: events
key: session_id
order_by: timestamp

columns:
  entry_page:
    type: String
    strategy: "earliest(pathname)"
  count:
    type: UInt64
    strategy: "count()"
"#;
        let def = parse_transform(yaml).unwrap();
        assert_eq!(def.order_by.as_deref(), Some("timestamp"));
        assert_eq!(def.columns.len(), 2);
    }

    #[test]
    fn test_parse_transform_without_order_by() {
        let yaml = r#"
transform: test
source: events
key: session_id

columns:
  entry_page:
    type: String
    strategy: "earliest(pathname)"
"#;
        let def = parse_transform(yaml).unwrap();
        assert!(def.order_by.is_none());
    }
}
