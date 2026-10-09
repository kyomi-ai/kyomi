// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared, fail-closed ChartML schema and SQL validation.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, sync::OnceLock};

// README_SCHEMA.md declares this Kyomi master authoritative, including SQL and
// plugin extensions beyond the locked chartml-core 5.1.12 renderer schema.
// Serve this exact asset to clients and use its minified equivalent in prompts.
pub const SCHEMA: &str = include_str!("../../../data/chartml-spec/chartml_schema.json");
static VALIDATOR: OnceLock<Result<jsonschema::Validator, String>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub block: usize,
    pub component: Option<usize>,
    pub stage: String,
    pub instance_path: String,
    pub message: String,
}
impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Block {}", self.block)?;
        if let Some(component) = self.component {
            write!(f, ", component {component}")?;
        }
        write!(
            f,
            " {} {}: {}",
            self.stage, self.instance_path, self.message
        )
    }
}
fn diagnostic(
    block: usize,
    component: Option<usize>,
    stage: &'static str,
    path: &str,
    message: String,
) -> Diagnostic {
    Diagnostic {
        block,
        component,
        stage: stage.into(),
        instance_path: path.into(),
        message,
    }
}
fn closing_fence(text: &str) -> Option<usize> {
    text.match_indices("```").find_map(|(i, _)| {
        if i > 0 && text.as_bytes()[i - 1] != b'\n' {
            return None;
        }
        match text.as_bytes().get(i + 3) {
            None | Some(b'\n' | b'\r' | b' ') => Some(i),
            _ => None,
        }
    })
}
/// ChartML bodies and full fence spans in the renderer's document order.
/// Scan all fences so ChartML-looking text inside other code is never executable.
pub fn markdown_spans(content: &str) -> Vec<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let mut result = Vec::new();
    let mut offset = 0;
    while offset < content.len() {
        let remaining = &content[offset..];
        let opening = remaining.match_indices("```").find_map(|(i, _)| {
            if i > 0 && remaining.as_bytes()[i - 1] != b'\n' {
                return None;
            }
            let after = &remaining[i + 3..];
            let end = after.find('\n').unwrap_or(after.len());
            let language = after[..end].trim();
            if language.is_empty()
                && (end == after.len() || closing_fence(&after[end + 1..]).is_none())
            {
                return None;
            }
            Some((i, language, i + 3 + end + usize::from(end < after.len())))
        });
        let Some((start, language, body_start)) = opening else {
            break;
        };
        let inner = &remaining[body_start..];
        let close = closing_fence(inner);
        let body_end = close.map_or(remaining.len(), |i| body_start + i);
        let full_end = close.map_or(remaining.len(), |i| body_start + i + 3);
        if language == "chartml" {
            result.push((
                offset + start..offset + full_end,
                offset + body_start..offset + body_end,
            ));
        }
        // The renderer consumes the entire closing line.
        offset += close.map_or(remaining.len(), |_| {
            remaining[full_end..]
                .find('\n')
                .map_or(remaining.len(), |i| full_end + i + 1)
        });
    }
    result
}
pub fn markdown_blocks(content: &str) -> Vec<&str> {
    markdown_spans(content)
        .into_iter()
        .map(|(_, body)| &content[body])
        .collect()
}
pub fn strip_markdown_blocks(content: &str, indices: &[usize], replacement: &str) -> String {
    let mut result = String::new();
    let mut end = 0;
    for (i, (span, _)) in markdown_spans(content).into_iter().enumerate() {
        if indices.contains(&i) {
            result.push_str(&content[end..span.start]);
            result.push_str(replacement);
            end = span.end;
        }
    }
    result.push_str(&content[end..]);
    result
}
fn compile_schema(schema: &Value) -> Result<jsonschema::Validator, String> {
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .build(schema)
        .map_err(|e| e.to_string())
}
fn json_value(value: serde_yaml::Value, path: &str) -> Result<Value, (String, String)> {
    let error = |message: &str| (path.to_string(), message.to_string());
    match value {
        serde_yaml::Value::Number(ref number)
            if number.as_f64().is_some_and(|f| !f.is_finite()) =>
        {
            Err(error("Non-finite YAML numbers are not JSON-compatible"))
        }
        serde_yaml::Value::Tagged(_) => Err(error("YAML tags are not JSON-compatible")),
        serde_yaml::Value::Mapping(map) => {
            let mut result = serde_json::Map::new();
            for (key, value) in map {
                let serde_yaml::Value::String(key) = key else {
                    return Err(error("YAML mapping keys must be strings"));
                };
                let child_path = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                result.insert(key, json_value(value, &child_path)?);
            }
            Ok(Value::Object(result))
        }
        serde_yaml::Value::Sequence(values) => values
            .into_iter()
            .enumerate()
            .map(|(index, value)| json_value(value, &format!("{path}/{index}")))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        value => {
            serde_json::to_value(value).map_err(|e| error(&format!("Non-JSON YAML value: {e}")))
        }
    }
}
fn parse_block(yaml: &str, block: usize) -> Result<Value, Vec<Diagnostic>> {
    let value = serde_yaml::from_str(yaml)
        .map_err(|e| vec![diagnostic(block, None, "yaml", "", e.to_string())])?;
    let array = matches!(value, serde_yaml::Value::Sequence(_));
    let value = json_value(value, "").map_err(|(path, message)| {
        let component = if array {
            path.split('/')
                .nth(1)
                .and_then(|s| s.parse::<usize>().ok())
                .map(|i| i + 1)
        } else {
            Some(1)
        };
        vec![diagnostic(block, component, "conversion", &path, message)]
    })?;
    Ok(value)
}
pub fn validate_block(yaml: &str, block: usize) -> Result<Value, Vec<Diagnostic>> {
    let value = parse_block(yaml, block)?;
    let validator = VALIDATOR
        .get_or_init(|| {
            let schema: Value = serde_json::from_str(SCHEMA).map_err(|e| e.to_string())?;
            compile_schema(&schema)
        })
        .as_ref()
        .map_err(|e| {
            vec![diagnostic(
                block,
                None,
                "schema_initialization",
                "",
                e.clone(),
            )]
        })?;
    fn collect(
        error: &jsonschema::ValidationError<'_>,
        value: &Value,
        block: usize,
        errors: &mut Vec<Diagnostic>,
    ) {
        match error.kind() {
            jsonschema::error::ValidationErrorKind::AnyOf { context }
            | jsonschema::error::ValidationErrorKind::OneOfNotValid { context } => {
                for branch in context {
                    for error in branch {
                        collect(error, value, block, errors);
                    }
                }
            }
            _ => {
                let path = error.instance_path().to_string();
                let component = if value.is_array() {
                    path.split('/')
                        .nth(1)
                        .and_then(|s| s.parse::<usize>().ok())
                        .map(|i| i + 1)
                } else {
                    Some(1)
                };
                errors.push(diagnostic(
                    block,
                    component,
                    "schema",
                    &path,
                    error.to_string(),
                ));
            }
        }
    }
    let mut errors = Vec::new();
    for error in validator.iter_errors(&value) {
        collect(&error, &value, block, &mut errors);
    }
    if errors.is_empty() {
        Ok(value)
    } else {
        Err(errors)
    }
}
pub fn validate_markdown_schema(content: &str) -> Vec<Diagnostic> {
    markdown_blocks(content)
        .iter()
        .enumerate()
        .flat_map(|(i, b)| validate_block(b, i + 1).err().unwrap_or_default())
        .collect()
}

#[derive(Debug, Clone)]
pub struct SqlSource {
    pub block: usize,
    pub component: usize,
    pub instance_path: String,
    pub datasource: Option<String>,
    pub name: Option<String>,
    pub sql: String,
}
fn named_data_map(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value.as_object().filter(|map| {
        !["datasource", "provider", "rows", "url"]
            .iter()
            .any(|key| map.contains_key(*key))
    })
}
// Dashboard viewer consumes standalone params groups, first default per id wins.
// Chart-local params do not populate its parameter signal and cannot override it.
fn parameter_defaults(component: &Value, values: &mut std::collections::BTreeMap<String, String>) {
    if let Some(params) = component["params"].as_array() {
        for param in params {
            if let (Some(id), Some(default)) = (param["id"].as_str(), param.get("default"))
                && let std::collections::btree_map::Entry::Vacant(entry) =
                    values.entry(id.to_string())
            {
                entry.insert(
                    default
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| default.to_string()),
                );
            }
        }
    }
}
fn replace_curly_defaults(
    text: &str,
    defaults: &std::collections::BTreeMap<String, String>,
) -> String {
    let mut text = text.to_string();
    for (key, value) in defaults {
        text = text.replace(&format!("{{{{{key}}}}}"), value);
    }
    text
}
// Mirrors locked chartml-core 5.1.12 params::resolve_param_references:
// exact double-quoted refs, inline defaults last-wins, longer keys first,
// String/JSON YAML replacement without coercing numbers into query strings.
fn resolve_quoted_inline_defaults(yaml: &str, value: &Value) -> String {
    fn yaml_value(value: &Value) -> String {
        match value {
            Value::String(s) => format!("\"{s}\""),
            Value::Array(values) => format!(
                "[{}]",
                values.iter().map(yaml_value).collect::<Vec<_>>().join(", ")
            ),
            value => value.to_string(),
        }
    }
    let mut result = yaml.to_string();
    if !value.is_object() {
        return result;
    }
    let mut defaults = std::collections::BTreeMap::new();
    if let Some(params) = value["params"].as_array() {
        for param in params {
            if let (Some(id), Some(default)) = (param["id"].as_str(), param.get("default")) {
                defaults.insert(id, default);
            }
        }
    }
    let mut keys: Vec<_> = defaults.keys().collect();
    keys.sort_by_key(|key| std::cmp::Reverse(key.len()));
    for key in keys {
        result = result.replace(&format!("\"${key}\""), &yaml_value(defaults[key]));
    }
    result
}
fn check_sql_parameters(text: &str) -> Result<String, SqlFailure> {
    let text = text.to_string();
    if text.contains("{{")
        || text.contains("}}")
        || text.strip_prefix('$').is_some_and(|id| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        })
    {
        return Err(SqlFailure::new(
            "sql_parameters",
            "Unresolved parameter placeholder; provide an applicable default before SQL validation",
        ));
    }
    Ok(text)
}
/// Enumerate SQL source definitions once, including named sources reused by charts.
pub fn sql_sources(value: &Value, block: usize) -> Vec<SqlSource> {
    let components: Vec<&Value> = value
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_else(|| vec![value]);
    let mut sources = Vec::new();
    for (i, component) in components.iter().enumerate() {
        let (source, path) = if component["type"] == "source" {
            (*component, "")
        } else {
            (&component["data"], "/data")
        };
        let candidates = if component["type"] != "source"
            && let Some(map) = named_data_map(source)
        {
            map.iter()
                .map(|(name, source)| {
                    (
                        source,
                        format!("/data/{}", name.replace('~', "~0").replace('/', "~1")),
                    )
                })
                .collect::<Vec<_>>()
        } else {
            vec![(source, path.to_string())]
        };
        for (source, path) in candidates {
            if let Some(sql) = source["query"].as_str() {
                sources.push(SqlSource {
                    block,
                    component: i + 1,
                    instance_path: format!(
                        "{}{path}/query",
                        if value.is_array() {
                            format!("/{i}")
                        } else {
                            String::new()
                        }
                    ),
                    datasource: source["datasource"].as_str().map(str::to_owned),
                    name: (component["type"] == "source")
                        .then(|| component["name"].as_str().map(str::to_owned))
                        .flatten(),
                    sql: sql.into(),
                });
            }
        }
    }
    sources
}
#[derive(Debug)]
pub struct SqlFailure {
    pub stage: &'static str,
    pub message: String,
}
impl SqlFailure {
    pub fn new(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }
}
impl From<String> for SqlFailure {
    fn from(message: String) -> Self {
        Self::new("sql_dry_run", message)
    }
}
impl From<&str> for SqlFailure {
    fn from(message: &str) -> Self {
        Self::new("sql_dry_run", message)
    }
}
/// Each block passes schema validation before its SQL can access a datasource. No execution fallback.
pub async fn validate_blocks<F, Fut>(blocks: &[&str], mut dry_run: F) -> Vec<Diagnostic>
where
    F: FnMut(SqlSource) -> Fut,
    Fut: Future<Output = Result<(), SqlFailure>>,
{
    let mut errors = Vec::new();
    let mut failed = std::collections::HashSet::new();
    loop {
        let retained: Vec<_> = blocks
            .iter()
            .enumerate()
            .filter(|(index, _)| !failed.contains(&(index + 1)))
            .collect();
        let bodies: Vec<_> = retained.iter().map(|(_, block)| **block).collect();
        let indices: Vec<_> = retained.iter().map(|(index, _)| index + 1).collect();
        let round = validate_round(&bodies, &indices, &mut dry_run).await;
        if round.is_empty() {
            break;
        }
        for error in round {
            failed.insert(error.block);
            errors.push(error);
        }
    }
    errors
}
/// Every round rebuilds document defaults/source definitions from retained fences.
/// A changed fallback default must be resolved and dry-run again before retention.
async fn validate_round<F, Fut>(
    blocks: &[&str],
    indices: &[usize],
    dry_run: &mut F,
) -> Vec<Diagnostic>
where
    F: FnMut(SqlSource) -> Fut,
    Fut: Future<Output = Result<(), SqlFailure>>,
{
    let mut errors = Vec::new();
    let mut sources = Vec::new();
    let mut names = std::collections::HashMap::new();
    let mut failed_names = std::collections::HashMap::new();
    let mut references = Vec::new();
    let mut defaults = std::collections::BTreeMap::new();
    let mut parsed = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        match parse_block(block, indices[i]) {
            Ok(value) => {
                let components: Vec<&Value> = value
                    .as_array()
                    .map(|a| a.iter().collect())
                    .unwrap_or_else(|| vec![&value]);
                for component in components {
                    if component["type"] == "params"
                        && validate_block(&component.to_string(), indices[i]).is_ok()
                    {
                        parameter_defaults(component, &mut defaults);
                    }
                }
                parsed.push((indices[i], *block));
            }
            Err(e) => errors.extend(e),
        }
    }
    for (block_index, block) in parsed {
        let curly_resolved = replace_curly_defaults(block, &defaults);
        let value = match parse_block(&curly_resolved, block_index) {
            Ok(value) => value,
            Err(e) => {
                errors.extend(e);
                continue;
            }
        };
        let rendered = resolve_quoted_inline_defaults(&curly_resolved, &value);
        match validate_block(&rendered, block_index) {
            Ok(value) => {
                sources.extend(sql_sources(&value, block_index));
                let components: Vec<&Value> = value
                    .as_array()
                    .map(|a| a.iter().collect())
                    .unwrap_or_else(|| vec![&value]);
                for (component_index, component) in components.iter().enumerate() {
                    if component["type"] == "source" {
                        if let Some(name) = component["name"].as_str() {
                            let definitions =
                                names.entry(name.to_string()).or_insert_with(Vec::new);
                            definitions.push((block_index, component_index + 1));
                        }
                    } else if component["type"] == "chart" {
                        let path = if value.is_array() {
                            format!("/{component_index}/data")
                        } else {
                            "/data".into()
                        };
                        if let Some(name) = component["data"].as_str() {
                            references.push((
                                block_index,
                                component_index + 1,
                                path.clone(),
                                name.to_string(),
                            ));
                        }
                        if let Some(data) = named_data_map(&component["data"]) {
                            for (key, source) in data {
                                if let Some(name) = source.as_str() {
                                    references.push((
                                        block_index,
                                        component_index + 1,
                                        format!(
                                            "{path}/{}",
                                            key.replace('~', "~0").replace('/', "~1")
                                        ),
                                        name.to_string(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => errors.extend(e),
        }
    }
    for (name, definitions) in &names {
        if definitions.len() > 1 {
            for &(block, component) in definitions {
                errors.push(diagnostic(
                    block,
                    Some(component),
                    "source_resolution",
                    "/name",
                    format!("Named source '{name}' has duplicate definitions"),
                ));
            }
            failed_names.insert(
                name.clone(),
                ("source_resolution", "duplicate definitions".to_string()),
            );
        }
    }
    for mut source in sources {
        let resolution = check_sql_parameters(&source.sql).and_then(|sql| {
            source.sql = sql;
            if let Some(ds) = &source.datasource {
                source.datasource = Some(check_sql_parameters(ds)?);
            }
            Ok(())
        });
        let result = if let Err(error) = resolution {
            Err(error)
        } else if source.datasource.is_none() {
            Err(SqlFailure::new(
                "sql_unavailable",
                "SQL source has no selected datasource; dry-run unavailable",
            ))
        } else {
            dry_run(source.clone()).await
        };
        if let Err(e) = result {
            if let Some(name) = &source.name {
                failed_names.insert(name.clone(), (e.stage, e.message.clone()));
            }
            errors.push(diagnostic(
                source.block,
                Some(source.component),
                e.stage,
                &source.instance_path,
                e.message,
            ));
        }
    }
    for (block, component, path, name) in &references {
        if let Some((stage, message)) = failed_names.get(name) {
            errors.push(diagnostic(
                *block,
                Some(*component),
                stage,
                path,
                format!("Named source '{name}' failed validation: {message}"),
            ));
        } else if !names.contains_key(name) {
            errors.push(diagnostic(
                *block,
                Some(*component),
                "source_resolution",
                path,
                format!("Named source '{name}' is unavailable in this validation document"),
            ));
        }
    }
    errors
}

#[cfg(test)]
#[path = "chartml_validation_tests.rs"]
mod tests;
