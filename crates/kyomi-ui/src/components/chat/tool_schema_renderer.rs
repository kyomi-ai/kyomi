// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tool Schema Renderer — displays tool call results in the chat thinking tracker.
//!
//! Ported from `apps/frontend/src/components/ToolSchemaRenderer.jsx` (~2000 lines, 29 renderers).
//! Each tool has a dedicated renderer; the router dispatches by `schema.tool`.

use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use leptos_icons::Icon;
use serde_json::Value;

use crate::utils::cron::{describe_cron, get_tz_offset_minutes};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Extract a string field from a JSON value.
fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// Extract a u64 field from a JSON value.
fn u64_field(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

/// Extract an f64 field from a JSON value.
fn f64_field(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

/// Extract a bool field from a JSON value.
fn bool_field(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

/// Extract an array field from a JSON value.
fn array_field<'a>(v: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    v.get(key).and_then(Value::as_array)
}

/// Check if a JSON value is an object with a given key.
fn has_key(v: &Value, key: &str) -> bool {
    v.is_object() && v.get(key).is_some()
}

/// Extract an error message from output, handling nested JSON in `data` field.
fn extract_error(output: &Value) -> String {
    // Try to extract error from output.data JSON string first
    if let Some(data) = str_field(output, "data") {
        let json_string = if let Some(idx) = data.find("\n\n\nYou ONLY have access to the following tools") {
            &data[..idx]
        } else {
            data
        };
        if let Ok(parsed) = serde_json::from_str::<Value>(json_string)
            && let Some(err) = str_field(&parsed, "error")
        {
            return err.to_string();
        }
    }
    // Fallback to direct error field
    str_field(output, "error")
        .unwrap_or("Unknown error")
        .to_string()
}

/// Render an error block with standard styling.
fn error_block(title: &str, message: String) -> AnyView {
    let title = title.to_string();
    view! {
        <div class="bg-error p-2 rounded border border-error-border">
            <div class="font-medium text-error-foreground text-xs">{title}</div>
            <div class="text-error-foreground text-xs">{message}</div>
        </div>
    }
    .into_any()
}

/// Render a success block with standard styling.
fn success_block(message: String) -> AnyView {
    view! {
        <div class="bg-success p-2 rounded border border-success-border">
            <div class="text-success-foreground text-xs flex items-center gap-1">
                <span class="font-medium">{message}</span>
            </div>
        </div>
    }
    .into_any()
}

/// Render a warning/upgrade-required block.
fn warning_block(title: &str, message: String) -> AnyView {
    let title = title.to_string();
    view! {
        <div class="bg-warning p-2 rounded border border-warning-border">
            <div class="font-medium text-warning-foreground text-xs">{title}</div>
            <div class="mt-1 text-warning-foreground text-xs">{message}</div>
        </div>
    }
    .into_any()
}

/// Render a section label.
fn section_label(text: &str) -> AnyView {
    let text = text.to_string();
    view! {
        <span class="font-medium text-foreground text-xs">{text}</span>
    }
    .into_any()
}

/// Render a monospace code block (for SQL, YAML, etc).
fn code_block(code: &str) -> AnyView {
    let code = code.to_string();
    view! {
        <div class="mt-1 bg-muted p-2 rounded">
            <pre class="text-xs font-mono text-foreground whitespace-pre-wrap break-all">{code}</pre>
        </div>
    }
    .into_any()
}

/// Render a metric card (label + value in a bordered box).
fn metric_card(label: &str, value: String) -> AnyView {
    let label = label.to_string();
    view! {
        <div class="bg-muted p-2 rounded border border-border">
            <div class="font-medium text-foreground text-xs">{label}</div>
            <div class="text-muted-foreground text-xs">{value}</div>
        </div>
    }
    .into_any()
}

/// Render a datasource badge.
fn datasource_badge(name: &str) -> AnyView {
    let text = format!("Datasource: {name}");
    view! {
        <span class="inline-block px-2 py-1 bg-accent rounded border border-border text-foreground text-xs font-medium">
            {text}
        </span>
    }
    .into_any()
}

/// Render a compact data table with header and rows.
///
/// `cols` is a list of column names. `rows` is a list of rows, each row being
/// a list of cell values. `max_rows` limits how many rows are displayed.
fn compact_data_table(
    cols: Vec<String>,
    rows: Vec<Vec<String>>,
    max_rows: usize,
) -> impl IntoView {
    let display_rows: Vec<Vec<String>> = rows.into_iter().take(max_rows).collect();

    view! {
        <div class="bg-card border border-border rounded text-xs overflow-x-auto">
            <table class="w-full">
                <thead class="bg-accent">
                    <tr>
                        {cols.iter().map(|col| {
                            let col = col.clone();
                            view! {
                                <th class="px-2 py-1 text-left font-medium text-foreground border-r border-border last:border-r-0">
                                    {col}
                                </th>
                            }
                        }).collect_view()}
                    </tr>
                </thead>
                <tbody>
                    {display_rows.iter().enumerate().map(|(row_idx, row)| {
                        let bg = if row_idx % 2 == 0 { "bg-card" } else { "bg-muted" };
                        let cells = row.clone();
                        view! {
                            <tr class=bg>
                                {cells.into_iter().map(|cell| {
                                    view! {
                                        <td class="px-2 py-1 text-muted-foreground font-mono border-r border-border last:border-r-0">
                                            {cell}
                                        </td>
                                    }
                                }).collect_view()}
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
        </div>
    }
}

/// Convert columnar data `{col1: [v1, v2], col2: [v3, v4]}` to row-based format.
fn columnar_to_rows(data: &Value, cols: &[String], max_rows: usize) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for row_idx in 0..max_rows {
        let mut row = Vec::new();
        for col in cols {
            let cell = data
                .get(col.as_str())
                .and_then(|arr| arr.get(row_idx))
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            row.push(cell);
        }
        rows.push(row);
    }
    rows
}

/// Extract column names from the `cols` array (handles both string and object formats).
fn extract_col_names(cols: &[Value]) -> Vec<String> {
    cols.iter()
        .map(|col| match col {
            Value::String(s) => s.clone(),
            Value::Object(obj) => obj
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        })
        .collect()
}

/// Render a key-value info row inside a muted container.
fn info_row(label: &str, value: &str) -> AnyView {
    let label = label.to_string();
    let value = value.to_string();
    view! {
        <div class="text-xs">
            <span class="text-muted-foreground">{label}": "</span>
            <span class="font-medium">{value}</span>
        </div>
    }
    .into_any()
}

/// Render a key-value info row with monospace value.
fn info_row_mono(label: &str, value: &str) -> AnyView {
    let label = label.to_string();
    let value = value.to_string();
    view! {
        <div class="text-xs">
            <span class="text-muted-foreground">{label}": "</span>
            <span class="font-mono text-xs">{value}</span>
        </div>
    }
    .into_any()
}

/// Describe a cron schedule using the browser's timezone.
fn describe_cron_local(schedule: &str) -> String {
    let offset = get_tz_offset_minutes();
    let desc = describe_cron(schedule, offset);
    desc.description
}

// ---------------------------------------------------------------------------
// Tool-specific renderers
// ---------------------------------------------------------------------------

// -- Data Query Renderers (Task 1) --

fn render_cost_estimate(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| {
        has_key(o, "error") || str_field(o, "status").is_some_and(|s| s != "success")
    });

    view! {
        <div class="space-y-2">
            {input.and_then(|inp| str_field(inp, "sql")).map(|sql| {
                view! {
                    <div>
                        {section_label("Query:")}
                        {code_block(sql)}
                    </div>
                }
            })}
            {output.map(|out| {
                if !has_error {
                    let cost = str_field(out, "cost").unwrap_or("?").to_string();
                    let size = str_field(out, "size").unwrap_or("?").to_string();
                    let safety = str_field(out, "safety").unwrap_or("?").to_string();
                    let safety_class = match safety.as_str() {
                        "OK" => "bg-muted border-border",
                        "WARNING" => "bg-warning border-warning-border",
                        _ => "bg-error border-error-border",
                    };
                    let safety_text_class = match safety.as_str() {
                        "OK" => "text-foreground",
                        "WARNING" => "text-warning-foreground",
                        _ => "text-error-foreground",
                    };
                    view! {
                        <div>
                            {section_label("Cost Analysis:")}
                            <div class="mt-1 grid grid-cols-3 gap-2 text-xs">
                                {metric_card("Cost", cost)}
                                {metric_card("Size", size)}
                                <div class=format!("p-2 rounded border {safety_class}")>
                                    <div class=format!("font-medium text-xs {safety_text_class}")>"Safety"</div>
                                    <div class=format!("text-xs {safety_text_class}")>{safety}</div>
                                </div>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    error_block("Error", extract_error(out)).into_any()
                }
            })}
        </div>
    }
}

fn render_query(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let sql = str_field(inp, "sql").unwrap_or("").to_string();
                let datasource = str_field(inp, "datasource").map(String::from);
                let limit = u64_field(inp, "limit").unwrap_or(0);
                let allows_large = bool_field(inp, "allows_large_query").unwrap_or(false);
                let console_url = output.and_then(|o| str_field(o, "console_url")).map(String::from);

                view! {
                    <div>
                        {section_label("Query:")}
                        {code_block(&sql)}
                        <div class="mt-1 flex gap-2 text-xs text-muted-foreground flex-wrap items-center">
                            {datasource.map(|ds| datasource_badge(&ds).into_any())}
                            <span>{format!("Limit: {} rows", limit)}</span>
                            {allows_large.then(|| view! {
                                <span class="text-warning-foreground">"Large query allowed"</span>
                            })}
                        </div>
                        {console_url.map(|url| view! {
                            <div class="mt-1">
                                <a
                                    href=url
                                    target="_blank"
                                    rel="noopener noreferrer"
                                    class="text-xs text-primary hover:text-primary/80 underline inline-flex items-center gap-1"
                                >
                                    "View in BigQuery Console"
                                </a>
                            </div>
                        })}
                    </div>
                }
            })}
            {output.map(|out| {
                // React: !(hasKey(output, 'error')) && !output.status → success
                let is_success = !has_key(out, "error") && str_field(out, "status").is_none();
                if is_success {
                    let rows = u64_field(out, "rows").unwrap_or(0);
                    let truncated = bool_field(out, "truncated").unwrap_or(false);
                    let cols_val = array_field(out, "cols").cloned().unwrap_or_default();
                    let col_names = extract_col_names(&cols_val);
                    let data = out.get("data");

                    let has_data = data.is_some() && !col_names.is_empty();
                    let display_rows = if has_data {
                        let max = if truncated { 20.min(rows as usize) } else { rows as usize };
                        columnar_to_rows(data.unwrap(), &col_names, max)
                    } else {
                        vec![]
                    };

                    view! {
                        <div>
                            {section_label("Results:")}
                            <div class="mt-1 space-y-2">
                                <div class="grid grid-cols-2 gap-2 text-xs">
                                    {metric_card("Rows Returned", rows.to_string())}
                                    {metric_card("Preview Limit", if truncated { "20 (truncated)".into() } else { "Full result".into() })}
                                </div>
                                {has_data.then(|| {
                                    let col_names_clone = col_names.clone();
                                    view! {
                                        <div>
                                            <div class="font-medium text-foreground text-xs mb-1">"Preview:"</div>
                                            {compact_data_table(col_names_clone, display_rows, 20)}
                                            {truncated.then(|| view! {
                                                <div class="text-xs text-muted-foreground mt-1">
                                                    "Query returned more than 20 rows. Create a ChartML visualization to see the full dataset."
                                                </div>
                                            })}
                                        </div>
                                    }
                                })}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div>
                            {error_block("Query Failed", extract_error(out))}
                            {str_field(out, "execution_time").map(|t| {
                                let t = t.to_string();
                                view! {
                                    <div class="text-xs text-muted-foreground mt-1">{format!("Execution time: {t}")}</div>
                                }
                            })}
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_validate_sql(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    view! {
        <div class="space-y-2">
            {input.and_then(|inp| str_field(inp, "sql")).map(|sql| {
                view! {
                    <div>
                        {section_label("SQL Query:")}
                        {code_block(sql)}
                    </div>
                }
            })}
            {output.map(|out| {
                let success = bool_field(out, "success").unwrap_or(false);
                if success {
                    let cost_gb = f64_field(out, "query_cost_gb");
                    view! {
                        <div>
                            {section_label("Validation Result:")}
                            <div class="mt-1 space-y-2">
                                {success_block("SQL is valid".into())}
                                {cost_gb.map(|c| metric_card("Query Cost", format!("{:.2} GB", c)))}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    let msg = str_field(out, "error_message").unwrap_or("Unknown validation error").to_string();
                    view! {
                        <div>
                            {section_label("Validation Result:")}
                            <div class="mt-1">
                                {error_block("Validation Failed", msg)}
                            </div>
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_sample(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    view! {
        <div class="space-y-2">
            {input.and_then(|inp| str_field(inp, "datasource")).map(datasource_badge)}
            {input.map(|inp| {
                let table_name = str_field(inp, "table_name").unwrap_or("").to_string();
                let sample_rows = u64_field(inp, "sample_rows").unwrap_or(5);
                let days_back = u64_field(inp, "days_back");
                let console_url = output.and_then(|o| str_field(o, "console_url")).map(String::from);

                view! {
                    <div>
                        {section_label("Table:")}
                        <div class="mt-1 bg-muted p-2 rounded">
                            <div class="text-xs text-foreground font-mono">{table_name}</div>
                        </div>
                        {console_url.map(|url| view! {
                            <div class="mt-1">
                                <a href=url target="_blank" rel="noopener noreferrer"
                                    class="text-xs text-primary hover:text-primary/80 underline inline-flex items-center gap-1">
                                    "View in BigQuery Console"
                                </a>
                            </div>
                        })}
                        <div class="mt-1 bg-muted p-2 rounded">
                            <div class="flex gap-2 text-xs text-muted-foreground">
                                <span>{format!("Rows: {}", sample_rows)}</span>
                                {days_back.map(|d| view! { <span>{format!("Days back: {}", d)}</span> })}
                            </div>
                        </div>
                    </div>
                }
            })}
            {output.map(|out| {
                // React: !(hasKey(output, 'error')) && !output.status → success
                let is_success = !has_key(out, "error") && str_field(out, "status").is_none();
                if is_success {
                    let rows = u64_field(out, "rows").unwrap_or(0);
                    let table_rows = u64_field(out, "table_rows");
                    let truncated = bool_field(out, "truncated").unwrap_or(false);
                    let cols_val = array_field(out, "cols").cloned().unwrap_or_default();
                    let col_names = extract_col_names(&cols_val);
                    let data = out.get("data");
                    let has_data = data.is_some() && !col_names.is_empty();
                    let display_rows = if has_data {
                        let max = if truncated { 20.min(rows as usize) } else { rows as usize };
                        columnar_to_rows(data.unwrap(), &col_names, max)
                    } else {
                        vec![]
                    };

                    view! {
                        <div>
                            {section_label("Sample Data:")}
                            <div class="mt-1 space-y-2">
                                <div class="grid grid-cols-2 gap-2 text-xs">
                                    {metric_card("Sampled Rows", rows.to_string())}
                                    {metric_card("Table Total", table_rows.map(|r| r.to_string()).unwrap_or_else(|| "Unknown".into()))}
                                </div>
                                {has_data.then(|| {
                                    let col_names_clone = col_names.clone();
                                    view! {
                                        <div>
                                            <div class="font-medium text-foreground text-xs mb-1">"Sample Rows:"</div>
                                            {compact_data_table(col_names_clone, display_rows, 20)}
                                        </div>
                                    }
                                })}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    let err = str_field(out, "error").unwrap_or("Unknown error");
                    let is_too_large = err.contains("exceeds") && err.contains("GB limit");
                    let title = if is_too_large { "Query Blocked - Too Large" } else { "Sampling Failed" };
                    let err_msg = err.to_string();
                    let console_url = str_field(out, "console_url").map(String::from);

                    view! {
                        <div class="bg-error p-2 rounded border border-error-border">
                            <div class="font-medium text-error-foreground text-xs">{title.to_string()}</div>
                            <div class="text-error-foreground text-xs">{err_msg}</div>
                            {is_too_large.then(|| view! {
                                <div class="mt-2 text-xs text-error-foreground">
                                    "Tip: This table is too large to sample with SELECT *. Try querying specific columns or use filters to reduce data scanned."
                                </div>
                            })}
                            {console_url.map(|url| view! {
                                <div class="mt-1">
                                    <a href=url target="_blank" rel="noopener noreferrer"
                                        class="text-xs text-primary hover:text-primary/80 underline inline-flex items-center gap-1">
                                        "View in BigQuery Console"
                                    </a>
                                </div>
                            })}
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

// -- Catalog & Knowledge Renderers (Task 2) --

fn render_table_info(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let datasource = input.and_then(|i| str_field(i, "datasource")).map(String::from);
    let table_name = output.and_then(|o| str_field(o, "table")).map(String::from);
    let table_desc = output.and_then(|o| str_field(o, "desc")).map(String::from);
    let row_count = output.and_then(|o| u64_field(o, "rows"));
    let columns = output.and_then(|o| array_field(o, "cols")).cloned().unwrap_or_default();
    let learnings = output.and_then(|o| array_field(o, "learnings")).cloned().unwrap_or_default();

    view! {
        <div class="space-y-2">
            {datasource.as_deref().map(datasource_badge)}
            {table_name.map(|name| view! {
                <div>
                    {section_label("Table:")}
                    <div class="mt-1 text-xs text-muted-foreground font-mono bg-muted p-1 rounded">{name}</div>
                </div>
            })}
            {render_learnings(&learnings)}
            {(!has_error).then(|| {
                let cols_len = columns.len();
                view! {
                    <div>
                        {section_label("Table Information:")}
                        <div class="mt-1 bg-muted p-3 rounded text-xs space-y-2">
                            {row_count.map(|r| view! {
                                <div class="text-muted-foreground">
                                    <span class="font-medium">"Rows: "</span>{r.to_string()}
                                </div>
                            })}
                            {(cols_len > 0).then(|| {
                                let show = columns.iter().take(10).map(|col| {
                                    let name = str_field(col, "name").unwrap_or("").to_string();
                                    let typ = str_field(col, "type").unwrap_or("").to_string();
                                    let desc = str_field(col, "desc").map(String::from);
                                    (name, typ, desc)
                                }).collect::<Vec<_>>();
                                let remaining = cols_len.saturating_sub(10);
                                view! {
                                    <div>
                                        <span class="font-medium text-foreground">{format!("Columns ({}):", cols_len)}</span>
                                        <div class="text-muted-foreground mt-1 space-y-1">
                                            {show.into_iter().map(|(name, typ, desc)| view! {
                                                <div class="flex gap-2">
                                                    <span class="font-mono">{name}</span>
                                                    <span class="text-muted-foreground">{format!("({})", typ)}</span>
                                                    {desc.map(|d| view! { <span class="text-muted-foreground">{format!("- {}", d)}</span> })}
                                                </div>
                                            }).collect_view()}
                                            {(remaining > 0).then(|| view! {
                                                <div class="text-muted-foreground italic">{format!("...and {} more columns", remaining)}</div>
                                            })}
                                        </div>
                                    </div>
                                }
                            })}
                            {table_desc.filter(|d| !d.trim().is_empty()).map(|d| view! {
                                <div class="border-t border-border pt-2 mt-2">
                                    <span class="font-medium text-foreground">"Description:"</span>
                                    <div class="text-muted-foreground mt-1">{d}</div>
                                </div>
                            })}
                        </div>
                    </div>
                }
            })}
            {has_error.then(|| {
                let err = output.and_then(|o| str_field(o, "error")).unwrap_or("Unknown error").to_string();
                error_block("Error", err)
            })}
        </div>
    }
}

/// Render accumulated knowledge/learnings section (shared by table info and search).
fn render_learnings(learnings: &[Value]) -> AnyView {
    if learnings.is_empty() {
        return view! { <div /> }.into_any();
    }
    let count = learnings.len();
    let show: Vec<_> = learnings.iter().take(3).map(|l| {
        let insight = str_field(l, "insight").unwrap_or("").to_string();
        let context = str_field(l, "context").map(String::from);
        let similarity = f64_field(l, "similarity").unwrap_or(0.0);
        let ds_specific = bool_field(l, "datasource_specific").unwrap_or(false);
        (insight, context, similarity, ds_specific)
    }).collect();
    let remaining = count.saturating_sub(3);

    view! {
        <div>
            <div class="font-medium text-foreground text-xs mb-1 flex items-center gap-1">
                <span>{format!("Accumulated Knowledge ({})", count)}</span>
            </div>
            <div class="space-y-1">
                {show.into_iter().map(|(insight, context, similarity, ds_specific)| view! {
                    <div class="bg-info border border-info-border rounded p-2 text-xs">
                        <div class="flex items-start justify-between">
                            <div class="flex-1">
                                <div class="text-foreground font-medium">{insight}</div>
                                {context.map(|c| view! {
                                    <div class="text-muted-foreground text-xs mt-1 italic">{c}</div>
                                })}
                            </div>
                            <div class="text-right ml-2 flex-shrink-0">
                                <div class="font-medium text-info-foreground text-xs">
                                    {format!("{}%", (similarity * 100.0).round() as u32)}
                                </div>
                                <div class="text-xs text-muted-foreground">
                                    {if ds_specific { "datasource" } else { "global" }}
                                </div>
                            </div>
                        </div>
                    </div>
                }).collect_view()}
                {(remaining > 0).then(|| view! {
                    <div class="text-xs text-muted-foreground text-center">
                        {format!("...and {} more", remaining)}
                    </div>
                })}
            </div>
        </div>
    }.into_any()
}

fn render_search_catalog(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    // Handle both compact {count, tables} and verbose {status, results_count, results}
    let is_compact = output.is_some_and(|o| has_key(o, "tables") && !has_key(o, "status"));
    let is_verbose = output.is_some_and(|o| str_field(o, "status") == Some("success"));
    let is_success = is_compact || is_verbose;

    let results_count = output.and_then(|o| {
        if is_compact { u64_field(o, "count") } else { u64_field(o, "results_count") }
    }).unwrap_or(0);

    let results: Vec<Value> = if is_compact {
        output.and_then(|o| array_field(o, "tables"))
            .cloned()
            .unwrap_or_default()
    } else {
        output.and_then(|o| array_field(o, "results"))
            .cloned()
            .unwrap_or_default()
    };

    let message = output.and_then(|o| {
        if is_compact {
            u64_field(o, "count").map(|c| format!("Found {} matching tables", c))
        } else {
            str_field(o, "message").map(String::from)
        }
    }).unwrap_or_default();

    let learnings = output.and_then(|o| array_field(o, "learnings")).cloned().unwrap_or_default();

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let query = str_field(inp, "query").unwrap_or("").to_string();
                let limit = u64_field(inp, "limit").unwrap_or(20);
                let datasource = str_field(inp, "datasource").map(String::from);

                view! {
                    <div>
                        {section_label("Search Query:")}
                        <div class="mt-1 bg-muted p-2 rounded">
                            <div class="text-xs text-foreground font-medium">{format!("\"{}\"", query)}</div>
                            <div class="mt-1 flex gap-2 text-xs text-muted-foreground">
                                <span>{format!("Limit: {} results", limit)}</span>
                                {datasource.map(|ds| view! {
                                    <span class="text-primary">{format!("Datasource: {}", ds)}</span>
                                })}
                            </div>
                        </div>
                    </div>
                }
            })}
            {output.map(|out| {
                if is_success {
                    let show_results: Vec<_> = results.iter().take(5).map(|r| {
                        let table_id = if is_compact {
                            str_field(r, "table").unwrap_or("").to_string()
                        } else {
                            str_field(r, "table_id").unwrap_or("").to_string()
                        };
                        let desc = if is_compact {
                            str_field(r, "desc").unwrap_or("No description available").to_string()
                        } else {
                            str_field(r, "table_description").unwrap_or("No description available").to_string()
                        };
                        let score = if is_compact {
                            f64_field(r, "score").unwrap_or(0.0)
                        } else {
                            f64_field(r, "similarity_score").unwrap_or(0.0)
                        };
                        (table_id, desc, score)
                    }).collect();
                    let remaining = results.len().saturating_sub(5);

                    view! {
                        <div>
                            {section_label("Search Results:")}
                            <div class="mt-1 space-y-2">
                                <div class="bg-muted p-2 rounded border border-border">
                                    <div class="font-medium text-foreground text-xs">{format!("Found {} tables", results_count)}</div>
                                    <div class="text-muted-foreground text-xs">{message.clone()}</div>
                                </div>
                                {render_learnings(&learnings)}
                                {(!show_results.is_empty()).then(|| {
                                    view! {
                                        <div class="space-y-2">
                                            {show_results.into_iter().map(|(table_id, desc, score)| view! {
                                                <div class="bg-card border border-border rounded p-3 text-xs">
                                                    <div class="flex items-start justify-between">
                                                        <div class="flex-1">
                                                            <div class="font-medium text-foreground font-mono">{table_id}</div>
                                                            <div class="text-muted-foreground mt-1">{desc}</div>
                                                        </div>
                                                        <div class="text-right ml-3">
                                                            <div class="font-medium text-foreground">{format!("{}%", (score * 100.0).round() as u32)}</div>
                                                            <div class="text-muted-foreground text-xs">"similarity"</div>
                                                        </div>
                                                    </div>
                                                </div>
                                            }).collect_view()}
                                            {(remaining > 0).then(|| view! {
                                                <div class="text-xs text-muted-foreground text-center">
                                                    {format!("...and {} more results", remaining)}
                                                </div>
                                            })}
                                        </div>
                                    }
                                })}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div>
                            {section_label("Search Results:")}
                            <div class="mt-1">{error_block("Search Failed", extract_error(out))}</div>
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_search_knowledge(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let results = output.and_then(|o| array_field(o, "results")).cloned().unwrap_or_default();

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let query = str_field(inp, "query").unwrap_or("").to_string();
                let datasource = str_field(inp, "datasource").map(String::from);
                let limit = u64_field(inp, "limit");

                view! {
                    <div>
                        {section_label("Search Query:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border">
                            <div class="text-xs text-foreground font-medium">{format!("\"{}\"", query)}</div>
                            <div class="mt-1 flex gap-2 text-xs text-muted-foreground">
                                {datasource.map(|ds| view! { <span class="text-primary">{format!("Datasource: {}", ds)}</span> })}
                                {limit.filter(|&l| l != 10).map(|l| view! { <span>{format!("Limit: {}", l)}</span> })}
                            </div>
                        </div>
                    </div>
                }
            })}
            {output.map(|out| {
                if has_error {
                    view! {
                        <div>
                            {section_label("Results:")}
                            <div class="mt-1">{error_block("Error", str_field(out, "error").unwrap_or("Unknown error").to_string())}</div>
                        </div>
                    }.into_any()
                } else if results.is_empty() {
                    view! {
                        <div>
                            {section_label("Results:")}
                            <div class="mt-1 bg-muted p-2 rounded border border-border text-xs text-muted-foreground">
                                "No results found"
                            </div>
                        </div>
                    }.into_any()
                } else {
                    let found = u64_field(out, "found").unwrap_or(0);
                    let source = str_field(out, "source").map(String::from);
                    let show: Vec<_> = results.iter().take(8).map(|r| {
                        let typ = str_field(r, "type").unwrap_or("").to_string();
                        let id = str_field(r, "id").unwrap_or("").to_string();
                        let text = str_field(r, "text").unwrap_or("").to_string();
                        let score = str_field(r, "score").unwrap_or("").to_string();
                        let matched_cols = array_field(r, "matched_columns").cloned().unwrap_or_default();
                        (typ, id, text, score, matched_cols)
                    }).collect();
                    let remaining = results.len().saturating_sub(8);

                    view! {
                        <div>
                            {section_label("Results:")}
                            <div class="mt-1 space-y-1">
                                <div class="bg-muted p-2 rounded border border-border text-xs text-muted-foreground">
                                    {format!("Found {} result(s)", found)}
                                    {source.map(|s| format!(" via {}", s)).unwrap_or_default()}
                                </div>
                                {show.into_iter().map(|(typ, id, text, score, matched_cols)| {
                                    let badge_class = match typ.as_str() {
                                        "table" => "bg-primary/10 text-primary",
                                        "learning" => "bg-warning/10 text-warning-foreground",
                                        "metric" => "bg-success/10 text-success-foreground",
                                        _ => "bg-muted text-muted-foreground",
                                    };
                                    let type_label = match typ.as_str() {
                                        "table" => "Table",
                                        "learning" => "Learning",
                                        "metric" => "Metric",
                                        other => other,
                                    }.to_string();
                                    let col_names: Vec<String> = matched_cols.iter()
                                        .filter_map(|c| str_field(c, "name").map(String::from))
                                        .collect();

                                    view! {
                                        <div class="bg-card border border-border rounded p-2 text-xs">
                                            <div class="flex items-start justify-between">
                                                <div class="flex-1">
                                                    <div class="flex items-center gap-2">
                                                        <span class=format!("px-1.5 py-0.5 rounded text-xs font-medium {}", badge_class)>
                                                            {type_label}
                                                        </span>
                                                        <span class="font-mono text-foreground text-xs truncate">{id}</span>
                                                    </div>
                                                    <div class="text-foreground mt-1">{text}</div>
                                                    {(!col_names.is_empty()).then(|| {
                                                        let cols_text = col_names.join(", ");
                                                        view! {
                                                            <div class="text-muted-foreground text-xs mt-1">
                                                                {format!("Matched columns: {}", cols_text)}
                                                            </div>
                                                        }
                                                    })}
                                                </div>
                                                <div class="text-right ml-2 flex-shrink-0">
                                                    <div class="text-xs text-muted-foreground">{score}</div>
                                                </div>
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                                {(remaining > 0).then(|| view! {
                                    <div class="text-xs text-muted-foreground text-center">
                                        {format!("...and {} more", remaining)}
                                    </div>
                                })}
                            </div>
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_list_datasources(schema: &Value) -> impl IntoView {
    let output = schema.get("output");
    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let datasources = output.and_then(|o| array_field(o, "datasources")).cloned().unwrap_or_default();

    view! {
        <div class="space-y-2">
            {section_label("Available Datasources:")}
            <div class="mt-1">
                {if has_error {
                    error_block("Error", output.and_then(|o| str_field(o, "error")).unwrap_or("Unknown error").to_string()).into_any()
                } else if datasources.is_empty() {
                    let msg = output.and_then(|o| str_field(o, "message")).unwrap_or("No datasources configured").to_string();
                    view! {
                        <div class="bg-muted p-2 rounded border border-border text-xs text-muted-foreground">{msg}</div>
                    }.into_any()
                } else {
                    view! {
                        <div class="space-y-1">
                            {datasources.iter().map(|ds| {
                                let name = str_field(ds, "name").unwrap_or("").to_string();
                                let slug = str_field(ds, "slug").unwrap_or("").to_string();
                                let ds_type = str_field(ds, "type").unwrap_or("").to_string();
                                let tables = u64_field(ds, "tables_indexed").unwrap_or(0);

                                view! {
                                    <div class="bg-muted p-2 rounded border border-border text-xs flex items-center justify-between">
                                        <div class="flex items-center gap-2">
                                            <span class="font-medium text-foreground">{name}</span>
                                            <span class="text-muted-foreground font-mono text-xs">{format!("({})", slug)}</span>
                                        </div>
                                        <div class="flex items-center gap-3 text-muted-foreground">
                                            <span class="bg-muted px-1.5 py-0.5 rounded text-xs">{ds_type}</span>
                                            <span>{format!("{} tables", tables)}</span>
                                        </div>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

fn render_browse_resources(schema: &Value) -> impl IntoView {
    let _ = schema;
    view! {
        <div class="flex items-center gap-2 text-sm text-muted-foreground">
            <span>"Browsing available documentation resources"</span>
        </div>
    }
}

fn render_read_resource(schema: &Value) -> impl IntoView {
    let uri = schema.get("input").and_then(|i| str_field(i, "uri")).map(String::from);
    view! {
        <div class="flex items-center gap-2 text-sm text-muted-foreground">
            <span>
                "Reading documentation"
                {uri.map(|u| format!(": {}", u)).unwrap_or_default()}
            </span>
        </div>
    }
}

// -- Dashboard Renderers (Task 3) --

fn render_update_dashboard(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| {
        matches!(o, Value::String(_)) || (o.is_object() && (has_key(o, "error") || bool_field(o, "success") == Some(false)))
    });
    let is_success = output.is_some_and(|o| bool_field(o, "success") == Some(true));

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let summary = str_field(inp, "summary").map(String::from);
                let content = str_field(inp, "content").map(String::from);
                view! {
                    <div>
                        {section_label("Dashboard Update:")}
                        <div class="mt-1 bg-muted p-3 rounded border border-border">
                            {summary.map(|s| view! { <div class="text-sm text-foreground mb-2">{s}</div> })}
                            {content.map(|c| {
                                let len = c.len();
                                view! {
                                    <details class="text-xs">
                                        <summary class="cursor-pointer text-muted-foreground hover:text-foreground">
                                            {format!("View content ({} chars)", len)}
                                        </summary>
                                        <pre class="mt-2 p-2 bg-accent rounded text-xs overflow-x-auto max-h-40 overflow-y-auto">{c}</pre>
                                    </details>
                                }
                            })}
                        </div>
                    </div>
                }
            })}
            {is_success.then(|| {
                let msg = output.and_then(|o| str_field(o, "message")).unwrap_or("Dashboard updated successfully").to_string();
                success_block(msg)
            })}
            {has_error.then(|| {
                let msg = output.map(|o| {
                    if let Value::String(s) = o { s.clone() }
                    else { str_field(o, "error").unwrap_or("Failed to update dashboard").to_string() }
                }).unwrap_or_default();
                error_block("Error", msg)
            })}
        </div>
    }
}

fn render_get_chartml_spec(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error") || bool_field(o, "success") == Some(false));
    let is_success = output.is_some_and(|o| bool_field(o, "success") == Some(true));

    view! {
        <div class="space-y-2">
            {input.and_then(|inp| str_field(inp, "section")).map(|section| {
                let section = section.to_string();
                view! {
                    <div>
                        {section_label("ChartML Spec Lookup:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border">
                            <span class="text-sm text-foreground">{format!("Section: {}", section)}</span>
                        </div>
                    </div>
                }
            })}
            {is_success.then(|| {
                let section = output.and_then(|o| str_field(o, "section")).map(String::from);
                let content = output.and_then(|o| str_field(o, "content")).map(String::from);
                let msg = format!("Spec loaded{}", section.map(|s| format!(" ({})", s)).unwrap_or_default());
                view! {
                    <div class="bg-success p-2 rounded border border-success-border">
                        <div class="text-success-foreground text-xs flex items-center gap-1 mb-2">
                            <span class="font-medium">{msg}</span>
                        </div>
                        {content.map(|c| {
                            let len = c.len();
                            let preview = if len > 2000 { format!("{}...", &c[..2000]) } else { c };
                            view! {
                                <details class="text-xs">
                                    <summary class="cursor-pointer text-success-foreground hover:text-success-foreground">
                                        {format!("View content ({} chars)", len)}
                                    </summary>
                                    <pre class="mt-2 p-2 bg-success/10 rounded text-xs overflow-x-auto max-h-40 overflow-y-auto text-success-foreground">{preview}</pre>
                                </details>
                            }
                        })}
                    </div>
                }
            })}
            {has_error.then(|| {
                let msg = output.and_then(|o| str_field(o, "error")).unwrap_or("Failed to load ChartML spec").to_string();
                error_block("Error", msg)
            })}
        </div>
    }
}

fn render_update_chart(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| {
        matches!(o, Value::String(_)) || (o.is_object() && (has_key(o, "error") || bool_field(o, "success") == Some(false)))
    });
    let is_success = output.is_some_and(|o| bool_field(o, "success") == Some(true));

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let summary = str_field(inp, "summary").map(String::from);
                let content = str_field(inp, "content").map(String::from);
                view! {
                    <div>
                        {section_label("Chart Update:")}
                        <div class="mt-1 bg-muted p-3 rounded border border-border">
                            {summary.map(|s| view! { <div class="text-sm text-foreground mb-2">{s}</div> })}
                            {content.map(|c| {
                                let len = c.len();
                                view! {
                                    <details class="text-xs">
                                        <summary class="cursor-pointer text-muted-foreground hover:text-foreground">
                                            {format!("View content ({} chars)", len)}
                                        </summary>
                                        <pre class="mt-2 p-2 bg-accent rounded text-xs overflow-x-auto max-h-40 overflow-y-auto">{c}</pre>
                                    </details>
                                }
                            })}
                        </div>
                    </div>
                }
            })}
            {is_success.then(|| {
                let msg = output.and_then(|o| str_field(o, "message")).unwrap_or("Chart updated successfully").to_string();
                success_block(msg)
            })}
            {has_error.then(|| {
                let msg = output.map(|o| {
                    if let Value::String(s) = o { s.clone() }
                    else { str_field(o, "error").unwrap_or("Failed to update chart").to_string() }
                }).unwrap_or_default();
                error_block("Error", msg)
            })}
        </div>
    }
}

fn render_search_dashboards(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let dashboards = output.and_then(|o| array_field(o, "dashboards")).cloned().unwrap_or_default();

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let query = str_field(inp, "query").map(String::from);
                let sort_by = str_field(inp, "sort_by").map(String::from);
                let limit = u64_field(inp, "limit");
                let top_popular = bool_field(inp, "top_popular").unwrap_or(false);

                view! {
                    <div>
                        {section_label("Searching Dashboards:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
                            {query.map(|q| info_row("Query", &q))}
                            {sort_by.map(|s| info_row("Sort by", &s))}
                            {limit.map(|l| info_row("Limit", &l.to_string()))}
                            {top_popular.then(|| info_row("Mode", "Top 10 Popular"))}
                        </div>
                    </div>
                }
            })}
            {output.map(|_out| {
                if has_error {
                    error_block("Error", output.and_then(|o| str_field(o, "error")).unwrap_or("Unknown error").to_string()).into_any()
                } else if dashboards.is_empty() {
                    view! {
                        <div class="bg-accent p-2 rounded border border-input text-xs">
                            <div class="text-muted-foreground">"No dashboards found"</div>
                        </div>
                    }.into_any()
                } else {
                    let count = dashboards.len();
                    let total = output.and_then(|o| u64_field(o, "total_workspace_documents"));

                    view! {
                        <div class="bg-success p-2 rounded border border-success-border text-xs space-y-2">
                            <div class="font-medium text-success-foreground">
                                {format!("Found {} dashboard{}", count, if count != 1 { "s" } else { "" })}
                            </div>
                            {total.map(|t| view! {
                                <div class="text-muted-foreground text-xs">{format!("Total dashboards in workspace: {}", t)}</div>
                            })}
                            <div class="space-y-2 mt-2 max-h-60 overflow-y-auto">
                                {dashboards.iter().map(|d| {
                                    let title = str_field(d, "title").unwrap_or("").to_string();
                                    let content = str_field(d, "content").map(String::from);
                                    let dash_id = str_field(d, "dashboard_id").unwrap_or("").to_string();
                                    let total_views = u64_field(d, "total_views");
                                    let recent_views = u64_field(d, "recent_views");

                                    view! {
                                        <div class="bg-card p-2 rounded border border-border">
                                            <div class="font-medium text-foreground">{title}</div>
                                            {content.map(|c| view! {
                                                <div class="text-muted-foreground text-xs mt-1 line-clamp-2">{c}</div>
                                            })}
                                            <div class="text-muted-foreground text-xs mt-1 space-y-0.5">
                                                <div>"ID: "<span class="font-mono text-xs">{dash_id}</span></div>
                                                {total_views.map(|tv| {
                                                    let rv = recent_views.unwrap_or(0);
                                                    view! { <div>{format!("Views: {} total, {} recent", tv, rv)}</div> }
                                                })}
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_get_dashboard_info(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let is_success = output.is_some_and(|o| bool_field(o, "success") == Some(true));

    view! {
        <div class="space-y-2">
            {input.and_then(|inp| str_field(inp, "dashboard_id")).map(|id| {
                let id = id.to_string();
                view! {
                    <div>
                        {section_label("Getting Dashboard Info:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs">
                            <span class="text-muted-foreground">"Dashboard ID: "</span>
                            <span class="font-mono">{id}</span>
                        </div>
                    </div>
                }
            })}
            {output.map(|out| {
                if has_error {
                    error_block("Error", str_field(out, "error").unwrap_or("Unknown error").to_string()).into_any()
                } else if is_success {
                    let msg = str_field(out, "message").unwrap_or("Dashboard Retrieved").to_string();
                    let title = str_field(out, "title").unwrap_or("").to_string();
                    let dash_id = str_field(out, "dashboard_id").unwrap_or("").to_string();
                    let content = str_field(out, "content").map(String::from);
                    let last_change = str_field(out, "last_change_summary").map(String::from);

                    view! {
                        <div class="bg-success p-2 rounded border border-success-border text-xs space-y-2">
                            <div class="font-medium text-success-foreground">{msg}</div>
                            <div class="bg-card p-2 rounded border border-border space-y-2">
                                <div class="font-medium text-foreground">{title}</div>
                                <div class="text-muted-foreground text-xs space-y-0.5">
                                    <div>"ID: "<span class="font-mono">{dash_id}</span></div>
                                    {last_change.map(|lc| view! { <div>{format!("Last change: {}", lc)}</div> })}
                                </div>
                                {content.map(|c| view! {
                                    <div class="bg-muted p-2 rounded border border-border text-xs max-h-40 overflow-y-auto">
                                        <div class="text-muted-foreground mb-1">"Content:"</div>
                                        <pre class="whitespace-pre-wrap font-mono text-xs">{c}</pre>
                                    </div>
                                })}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="bg-accent p-2 rounded border border-input text-xs">
                            <div class="text-muted-foreground">"No dashboard data returned"</div>
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_create_dashboard(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let upgrade_required = output.and_then(|o| bool_field(o, "upgrade_required")).unwrap_or(false);
    let is_success = output.is_some_and(|o| bool_field(o, "success") == Some(true));

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let title = str_field(inp, "title").unwrap_or("").to_string();
                let content = str_field(inp, "content").map(|c| {
                    if c.len() > 100 { format!("{}...", &c[..100]) } else { c.to_string() }
                });
                let verified = bool_field(inp, "verified_no_duplicates").unwrap_or(false);
                view! {
                    <div>
                        {section_label("Creating Dashboard:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
                            {info_row("Title", &title)}
                            {content.map(|c| info_row("Content", &c))}
                            {verified.then(|| view! { <div class="text-success-foreground text-xs">"Verified no duplicates"</div> })}
                        </div>
                    </div>
                }
            })}
            {has_error.then(|| {
                let msg = output.and_then(|o| str_field(o, "error")).unwrap_or("Unknown error").to_string();
                if upgrade_required {
                    warning_block("Upgrade Required", msg).into_any()
                } else {
                    error_block("Error", msg).into_any()
                }
            })}
            {is_success.then(|| {
                let msg = output.and_then(|o| str_field(o, "message")).unwrap_or("Dashboard Created").to_string();
                let title = output.and_then(|o| str_field(o, "title")).unwrap_or("").to_string();
                let dash_id = output.and_then(|o| str_field(o, "dashboard_id")).unwrap_or("").to_string();
                view! {
                    <div class="bg-success p-2 rounded border border-success-border text-xs space-y-2">
                        <div class="font-medium text-success-foreground">{msg}</div>
                        <div class="bg-card p-2 rounded border border-border">
                            <div class="font-medium text-foreground">{title}</div>
                            <div class="text-muted-foreground text-xs mt-1">
                                "ID: "<span class="font-mono">{dash_id}</span>
                            </div>
                        </div>
                    </div>
                }
            })}
        </div>
    }
}

fn render_modify_dashboard(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let is_success = output.is_some_and(|o| bool_field(o, "success") == Some(true));

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let dash_id = str_field(inp, "dashboard_id").unwrap_or("").to_string();
                let title = str_field(inp, "title").map(String::from);
                let content = str_field(inp, "content").map(|c| {
                    if c.len() > 100 { format!("{}...", &c[..100]) } else { c.to_string() }
                });
                let change_summary = str_field(inp, "change_summary").map(String::from);
                view! {
                    <div>
                        {section_label("Modifying Dashboard:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
                            {info_row_mono("Dashboard ID", &dash_id)}
                            {title.map(|t| info_row("New Title", &t))}
                            {content.map(|c| info_row("New Content", &c))}
                            {change_summary.map(|s| info_row("Summary", &s))}
                        </div>
                    </div>
                }
            })}
            {has_error.then(|| {
                error_block("Error", output.and_then(|o| str_field(o, "error")).unwrap_or("Unknown error").to_string())
            })}
            {is_success.then(|| {
                let msg = output.and_then(|o| str_field(o, "message")).unwrap_or("Dashboard Updated").to_string();
                let title = output.and_then(|o| str_field(o, "title")).unwrap_or("").to_string();
                let dash_id = output.and_then(|o| str_field(o, "dashboard_id")).unwrap_or("").to_string();
                view! {
                    <div class="bg-success p-2 rounded border border-success-border text-xs space-y-2">
                        <div class="font-medium text-success-foreground">{msg}</div>
                        <div class="bg-card p-2 rounded border border-border">
                            <div class="font-medium text-foreground">{title}</div>
                            <div class="text-muted-foreground text-xs mt-1">"ID: "<span class="font-mono">{dash_id}</span></div>
                        </div>
                    </div>
                }
            })}
        </div>
    }
}

fn render_delete_dashboard(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let is_success = output.is_some_and(|o| bool_field(o, "success") == Some(true));

    view! {
        <div class="space-y-2">
            {input.and_then(|inp| str_field(inp, "dashboard_id")).map(|id| {
                let id = id.to_string();
                view! {
                    <div>
                        {section_label("Deleting Dashboard:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs">
                            <span class="text-muted-foreground">"Dashboard ID: "</span>
                            <span class="font-mono">{id}</span>
                        </div>
                    </div>
                }
            })}
            {has_error.then(|| {
                error_block("Error", output.and_then(|o| str_field(o, "error")).unwrap_or("Unknown error").to_string())
            })}
            {is_success.then(|| {
                let msg = output.and_then(|o| str_field(o, "message")).unwrap_or("Dashboard Deleted").to_string();
                let dash_id = output.and_then(|o| str_field(o, "dashboard_id")).unwrap_or("").to_string();
                view! {
                    <div class="bg-success p-2 rounded border border-success-border text-xs">
                        <div class="font-medium text-success-foreground">{msg}</div>
                        <div class="text-muted-foreground text-xs mt-1">"ID: "<span class="font-mono">{dash_id}</span></div>
                    </div>
                }
            })}
        </div>
    }
}

// -- Watch Renderers (Task 4) --

fn render_create_watch(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let upgrade_required = output.and_then(|o| bool_field(o, "upgrade_required")).unwrap_or(false);

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let name = str_field(inp, "name").map(String::from);
                let schedule = str_field(inp, "schedule").map(describe_cron_local);
                let prompt = str_field(inp, "prompt").map(String::from);
                let queries = array_field(inp, "queries").cloned().unwrap_or_default();

                view! {
                    <div>
                        {section_label("Creating Watch:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
                            {name.map(|n| info_row("Name", &n))}
                            {schedule.map(|s| info_row("Schedule", &s))}
                            {prompt.map(|p| view! { <div class="text-xs"><span class="text-muted-foreground">"Prompt: "</span><span class="text-foreground">{p}</span></div> })}
                            {(!queries.is_empty()).then(|| {
                                let count = queries.len();
                                view! {
                                    <div>
                                        <span class="text-muted-foreground">{format!("Reference Queries ({}):", count)}</span>
                                        <div class="mt-1 space-y-1">
                                            {queries.iter().map(|q| {
                                                let comment = str_field(q, "comment").unwrap_or("").to_string();
                                                let ds = str_field(q, "datasource").map(String::from);
                                                view! {
                                                    <div class="bg-card p-1 rounded border border-border">
                                                        <div class="text-foreground font-medium">{comment}</div>
                                                        {ds.map(|d| view! { <div class="text-muted-foreground text-xs">{format!("Datasource: {}", d)}</div> })}
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>
                                }
                            })}
                        </div>
                    </div>
                }
            })}
            {output.map(|out| {
                if has_error {
                    let msg = str_field(out, "message").or_else(|| str_field(out, "error")).unwrap_or("Unknown error").to_string();
                    if upgrade_required {
                        warning_block("Upgrade Required", msg).into_any()
                    } else {
                        error_block("Error", msg).into_any()
                    }
                } else {
                    let name = str_field(out, "name").map(String::from);
                    let schedule = str_field(out, "schedule").map(describe_cron_local);
                    let next_run = str_field(out, "next_run_at").map(String::from);

                    view! {
                        <div class="bg-success p-2 rounded border border-success-border text-xs">
                            <div class="font-medium text-success-foreground">"Watch Created"</div>
                            {name.map(|n| view! { <div class="mt-1 text-success-foreground">{format!("Name: {}", n)}</div> })}
                            {schedule.map(|s| view! { <div class="text-success-foreground">{format!("Schedule: {}", s)}</div> })}
                            {next_run.map(|nr| view! { <div class="text-success-foreground">{format!("Next run: {}", nr)}</div> })}
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_preview_watch(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let name = str_field(inp, "name").map(String::from);
                let schedule = str_field(inp, "schedule").map(describe_cron_local);
                let prompt = str_field(inp, "prompt").map(String::from);

                view! {
                    <div>
                        {section_label("Watch Preview:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
                            {name.map(|n| info_row("Name", &n))}
                            {schedule.map(|s| info_row("Schedule", &s))}
                            {prompt.map(|p| view! { <div class="text-xs"><span class="text-muted-foreground">"Prompt: "</span><span class="text-foreground">{p}</span></div> })}
                        </div>
                    </div>
                }
            })}
            {output.map(|out| {
                if has_error {
                    error_block("Error", str_field(out, "error").unwrap_or("Unknown error").to_string()).into_any()
                } else {
                    let preview_schedule = out.get("preview").and_then(|p| str_field(p, "schedule")).map(describe_cron_local);
                    let msg = str_field(out, "message").map(String::from);

                    view! {
                        <div class="bg-primary/10 p-2 rounded border border-primary/20 text-xs">
                            <div class="font-medium text-primary">"Preview Generated"</div>
                            {preview_schedule.map(|s| view! { <div class="mt-1 text-foreground">{format!("Schedule: {}", s)}</div> })}
                            {msg.map(|m| view! { <div class="mt-1 text-muted-foreground text-xs">{m}</div> })}
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_update_watch(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let watch_id = str_field(inp, "watch_id").map(String::from);
                let name = str_field(inp, "name").map(String::from);
                let schedule = str_field(inp, "schedule").map(describe_cron_local);
                let prompt = str_field(inp, "prompt").map(String::from);
                let enabled = bool_field(inp, "enabled");

                view! {
                    <div>
                        {section_label("Updating Watch:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
                            {watch_id.map(|id| info_row_mono("Watch ID", &id))}
                            {name.map(|n| info_row("Name", &n))}
                            {schedule.map(|s| info_row("Schedule", &s))}
                            {prompt.map(|p| view! { <div class="text-xs"><span class="text-muted-foreground">"Prompt: "</span><span class="text-foreground">{p}</span></div> })}
                            {enabled.map(|e| info_row("Enabled", if e { "Yes" } else { "No" }))}
                        </div>
                    </div>
                }
            })}
            {output.map(|out| {
                if has_error {
                    error_block("Error", str_field(out, "error").unwrap_or("Unknown error").to_string()).into_any()
                } else {
                    let name = str_field(out, "name").map(String::from);
                    let schedule = str_field(out, "schedule").map(describe_cron_local);
                    let next_run = str_field(out, "next_run_at").map(String::from);

                    view! {
                        <div class="bg-success p-2 rounded border border-success-border text-xs">
                            <div class="font-medium text-success-foreground">"Watch Updated"</div>
                            {name.map(|n| view! { <div class="mt-1 text-success-foreground">{format!("Name: {}", n)}</div> })}
                            {schedule.map(|s| view! { <div class="text-success-foreground">{format!("Schedule: {}", s)}</div> })}
                            {next_run.map(|nr| view! { <div class="text-success-foreground">{format!("Next run: {}", nr)}</div> })}
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_search_watches(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let watches = output.and_then(|o| array_field(o, "watches")).cloned().unwrap_or_default();

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let query = str_field(inp, "query").map(String::from);
                let limit = u64_field(inp, "limit");

                view! {
                    <div>
                        {section_label("Searching Watches:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
                            {query.map(|q| info_row("Query", &q))}
                            {limit.map(|l| info_row("Limit", &l.to_string()))}
                        </div>
                    </div>
                }
            })}
            {output.map(|_out| {
                if has_error {
                    error_block("Error", output.and_then(|o| str_field(o, "error")).unwrap_or("Unknown error").to_string()).into_any()
                } else if watches.is_empty() {
                    let msg = output.and_then(|o| str_field(o, "message")).map(String::from);
                    view! {
                        <div class="bg-accent p-2 rounded border border-input text-xs">
                            <div class="text-muted-foreground">"No watches found"</div>
                            {msg.map(|m| view! { <div class="mt-1 text-muted-foreground text-xs">{m}</div> })}
                        </div>
                    }.into_any()
                } else {
                    let count = watches.len();
                    let total = output.and_then(|o| u64_field(o, "total_workspace_watches"));

                    view! {
                        <div class="bg-success p-2 rounded border border-success-border text-xs space-y-2">
                            <div class="font-medium text-success-foreground">
                                {format!("Found {} watch{}", count, if count != 1 { "es" } else { "" })}
                            </div>
                            {total.map(|t| view! {
                                <div class="text-muted-foreground text-xs">{format!("Total watches in workspace: {}", t)}</div>
                            })}
                            <div class="space-y-2 mt-2 max-h-60 overflow-y-auto">
                                {watches.iter().map(|w| {
                                    let name = str_field(w, "name").unwrap_or("").to_string();
                                    let prompt = str_field(w, "prompt").unwrap_or("").to_string();
                                    let schedule = str_field(w, "schedule").map(describe_cron_local).unwrap_or_default();
                                    let status = str_field(w, "status").unwrap_or("").to_string();
                                    let status_class = if status == "active" { "text-success-foreground" } else { "text-muted-foreground" };
                                    let queries_count = array_field(w, "queries").map(|q| q.len()).unwrap_or(0);

                                    view! {
                                        <div class="bg-card p-2 rounded border border-border">
                                            <div class="font-medium text-foreground">{name}</div>
                                            <div class="text-muted-foreground text-xs mt-1">{prompt}</div>
                                            <div class="text-muted-foreground text-xs mt-1 space-y-0.5">
                                                <div>{format!("Schedule: {}", schedule)}</div>
                                                <div>"Status: "<span class=status_class>{status}</span></div>
                                                {(queries_count > 0).then(|| view! {
                                                    <div class="text-muted-foreground">{format!("{} reference queries", queries_count)}</div>
                                                })}
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_delete_watch(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");
    let has_error = output.is_some_and(|o| has_key(o, "error"));

    view! {
        <div class="space-y-2">
            {input.and_then(|inp| str_field(inp, "watch_id")).map(|id| {
                let id = id.to_string();
                view! {
                    <div>
                        {section_label("Deleting Watch:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
                            {info_row_mono("Watch ID", &id)}
                        </div>
                    </div>
                }
            })}
            {output.map(|out| {
                if has_error {
                    error_block("Error", str_field(out, "error").unwrap_or("Unknown error").to_string()).into_any()
                } else {
                    let msg = str_field(out, "message").unwrap_or("Watch deleted").to_string();
                    success_block(format!("Watch Deleted — {}", msg)).into_any()
                }
            })}
        </div>
    }
}

fn render_trigger_watch(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");
    let has_error = output.is_some_and(|o| has_key(o, "error"));

    view! {
        <div class="space-y-2">
            {input.and_then(|inp| str_field(inp, "watch_id")).map(|id| {
                let id = id.to_string();
                view! {
                    <div>
                        {section_label("Triggering Watch:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
                            {info_row_mono("Watch ID", &id)}
                        </div>
                    </div>
                }
            })}
            {output.map(|out| {
                if has_error {
                    error_block("Error", str_field(out, "error").unwrap_or("Unknown error").to_string()).into_any()
                } else {
                    let name = str_field(out, "name").map(String::from);
                    let msg = str_field(out, "message").map(String::from);
                    let scheduled_for = str_field(out, "scheduled_for").map(String::from);

                    view! {
                        <div class="bg-success p-2 rounded border border-success-border text-xs">
                            <div class="font-medium text-success-foreground">"Watch Triggered"</div>
                            {name.map(|n| view! { <div class="mt-1 text-success-foreground">{format!("Watch: {}", n)}</div> })}
                            {msg.map(|m| view! { <div class="text-success-foreground">{m}</div> })}
                            {scheduled_for.map(|s| view! { <div class="text-success-foreground">{format!("Scheduled for: {}", s)}</div> })}
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_watch_info(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");
    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let watch = output.and_then(|o| o.get("watch"));
    let executions = output.and_then(|o| array_field(o, "recent_executions")).cloned().unwrap_or_default();

    view! {
        <div class="space-y-2">
            {input.and_then(|inp| str_field(inp, "watch_id")).map(|id| {
                let id = id.to_string();
                view! {
                    <div>
                        {section_label("Getting Watch Info:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs">
                            <span class="text-muted-foreground">"Watch ID: "</span>
                            <span class="font-mono">{id}</span>
                        </div>
                    </div>
                }
            })}
            {output.map(|_out| {
                if has_error {
                    error_block("Error", output.and_then(|o| str_field(o, "error")).unwrap_or("Unknown error").to_string()).into_any()
                } else if let Some(w) = watch {
                    let name = str_field(w, "name").unwrap_or("").to_string();
                    let prompt = str_field(w, "prompt").unwrap_or("").to_string();
                    let mode = str_field(w, "mode").unwrap_or("").to_string();
                    let schedule = str_field(w, "schedule").map(describe_cron_local).unwrap_or_default();
                    let enabled = bool_field(w, "enabled").unwrap_or(false);
                    let status_class = if enabled { "text-success-foreground" } else { "text-muted-foreground" };
                    let status_text = if enabled { "Active" } else { "Paused" };

                    view! {
                        <div class="bg-success p-2 rounded border border-success-border text-xs space-y-3">
                            <div>
                                <div class="font-medium text-success-foreground">"Watch Found"</div>
                                <div class="bg-card p-2 rounded border border-border mt-2 space-y-1">
                                    <div class="font-medium text-foreground">{name}</div>
                                    <div class="text-muted-foreground text-xs">{prompt}</div>
                                    <div class="text-muted-foreground text-xs space-y-0.5 mt-2">
                                        <div>"Mode: "<span class="font-medium">{mode}</span></div>
                                        <div>{format!("Schedule: {}", schedule)}</div>
                                        <div>"Status: "<span class=status_class>{status_text.to_string()}</span></div>
                                    </div>
                                </div>
                            </div>
                            {(!executions.is_empty()).then(|| {
                                let count = executions.len();
                                view! {
                                    <div>
                                        <div class="font-medium text-foreground text-xs mb-1">{format!("Recent Executions ({})", count)}</div>
                                        <div class="space-y-1 max-h-40 overflow-y-auto">
                                            {executions.iter().map(|exec| {
                                                let status = str_field(exec, "status").unwrap_or("").to_string();
                                                let alert_triggered = bool_field(exec, "alert_triggered").unwrap_or(false);
                                                let alert_title = str_field(exec, "alert_title").map(String::from);
                                                let error_message = str_field(exec, "error_message").map(String::from);

                                                let status_icon = match status.as_str() {
                                                    "success" => view! { <Icon icon=icondata_lu::LuCircleCheck width="12" height="12" attr:class="text-success-foreground" /> }.into_any(),
                                                    "error" => view! { <Icon icon=icondata_lu::LuCircleX width="12" height="12" attr:class="text-error-foreground" /> }.into_any(),
                                                    _ => view! { <Icon icon=icondata_lu::LuCircle width="12" height="12" attr:class="text-muted-foreground" /> }.into_any(),
                                                };

                                                view! {
                                                    <div class="bg-card p-2 rounded border border-border text-xs">
                                                        <div class="flex items-center gap-2">
                                                            {status_icon}
                                                            {alert_triggered.then(|| view! {
                                                                <span class="bg-primary/10 text-primary px-1 rounded text-xs">"alerted"</span>
                                                            })}
                                                        </div>
                                                        {alert_title.map(|t| view! { <div class="mt-1 font-medium text-foreground">{t}</div> })}
                                                        {error_message.map(|e| view! { <div class="mt-1 text-error-foreground">{e}</div> })}
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>
                                }
                            })}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="bg-accent p-2 rounded border border-input text-xs">
                            <div class="text-muted-foreground">"No watch data returned"</div>
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

// -- Chart & Misc Renderers (Task 5) --

fn render_validate_chartml(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");

    view! {
        <div class="space-y-2">
            {input.and_then(|inp| str_field(inp, "chartml").or(Some("No ChartML provided"))).map(|chartml| {
                view! {
                    <div>
                        {section_label("ChartML:")}
                        {code_block(chartml)}
                    </div>
                }
            })}
            {output.map(|out| {
                let success = bool_field(out, "success").unwrap_or(false);
                if success {
                    let query_cost = f64_field(out, "query_cost");
                    let bytes_scanned = u64_field(out, "bytes_scanned");
                    let has_cost_info = query_cost.is_some() || bytes_scanned.is_some();

                    view! {
                        <div>
                            {section_label("Validation Result:")}
                            <div class="mt-1 space-y-2">
                                {success_block("ChartML is valid".into())}
                                {has_cost_info.then(|| {
                                    view! {
                                        <div class="grid grid-cols-2 gap-2 text-xs">
                                            {query_cost.map(|c| metric_card("Query Cost", format!("{:.2} GB", c)))}
                                            {bytes_scanned.map(|b| {
                                                let formatted = if b > 1_000_000 {
                                                    format!("{:.1} MB", b as f64 / 1_000_000.0)
                                                } else if b > 1_000 {
                                                    format!("{:.1} KB", b as f64 / 1_000.0)
                                                } else {
                                                    format!("{} B", b)
                                                };
                                                metric_card("Bytes Scanned", formatted)
                                            })}
                                        </div>
                                    }
                                })}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    let msg = str_field(out, "error_message").unwrap_or("Unknown validation error").to_string();
                    view! {
                        <div>
                            {section_label("Validation Result:")}
                            <div class="mt-1">{error_block("Validation Failed", msg)}</div>
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_forecast_data(schema: &Value) -> impl IntoView {
    let input = schema.get("input");
    let output = schema.get("output");
    let has_error = output.is_some_and(|o| has_key(o, "error"));

    view! {
        <div class="space-y-2">
            {input.map(|inp| {
                let datasource = str_field(inp, "datasource").map(String::from);
                let query = str_field(inp, "query").map(String::from);
                let timestamp = str_field(inp, "timestamp").map(String::from);
                let value = str_field(inp, "value").map(String::from);
                let model = str_field(inp, "model").map(String::from);
                let horizon = u64_field(inp, "horizon");
                let confidence = f64_field(inp, "confidence_level");

                view! {
                    <div>
                        {section_label("Forecast Parameters:")}
                        <div class="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
                            {datasource.map(|ds| info_row("Datasource", &ds))}
                            {query.map(|q| view! {
                                <div>
                                    <span class="text-muted-foreground">"Query:"</span>
                                    {code_block(&q)}
                                </div>
                            })}
                            {timestamp.map(|t| info_row("Timestamp column", &t))}
                            {value.map(|v| info_row("Value column", &v))}
                            {model.map(|m| info_row("Model", &m))}
                            {horizon.map(|h| info_row("Horizon", &format!("{} periods", h)))}
                            {confidence.map(|c| info_row("Confidence", &format!("{}%", (c * 100.0).round() as u32)))}
                        </div>
                    </div>
                }
            })}
            {output.map(|out| {
                if has_error {
                    view! {
                        <div>{error_block("Forecast Failed", str_field(out, "error").unwrap_or("Unknown error").to_string())}</div>
                    }.into_any()
                } else if out.get("groups").is_some() {
                    // Grouped forecast
                    let summary = str_field(out, "summary").map(String::from);
                    let groups = out.get("groups").and_then(Value::as_object).cloned().unwrap_or_default();
                    let total = groups.len();
                    let show_groups: Vec<_> = groups.iter().take(5).map(|(k, v)| {
                        let err = str_field(v, "error").map(String::from);
                        let model_used = str_field(v, "model_used").unwrap_or("?").to_string();
                        let data_points = u64_field(v, "data_points").unwrap_or(0);
                        let forecast_len = array_field(v, "forecast").map(|f| f.len()).unwrap_or(0);
                        (k.clone(), err, model_used, data_points, forecast_len)
                    }).collect();
                    let remaining = total.saturating_sub(5);

                    view! {
                        <div class="bg-success p-2 rounded border border-success-border text-xs space-y-2">
                            <div class="font-medium text-success-foreground">"Grouped Forecast Complete"</div>
                            {summary.map(|s| view! { <div class="text-muted-foreground">{s}</div> })}
                            {show_groups.into_iter().map(|(key, err, model_used, data_points, forecast_len)| view! {
                                <div class="bg-card p-2 rounded border border-border">
                                    <div class="font-medium text-foreground">{key}</div>
                                    {if let Some(e) = err {
                                        view! { <div class="text-error-foreground text-xs mt-1">{e}</div> }.into_any()
                                    } else {
                                        view! {
                                            <div class="text-muted-foreground text-xs mt-1">
                                                {format!("Model: {} | {} data points | {} forecasted", model_used, data_points, forecast_len)}
                                            </div>
                                        }.into_any()
                                    }}
                                </div>
                            }).collect_view()}
                            {(remaining > 0).then(|| view! {
                                <div class="text-xs text-muted-foreground text-center">
                                    {format!("...and {} more groups", remaining)}
                                </div>
                            })}
                        </div>
                    }.into_any()
                } else {
                    // Single forecast
                    let summary = str_field(out, "summary").map(String::from);
                    let model_used = str_field(out, "model_used").map(String::from);
                    let data_points = u64_field(out, "data_points");
                    let forecast = array_field(out, "forecast").cloned().unwrap_or_default();

                    view! {
                        <div class="bg-success p-2 rounded border border-success-border text-xs space-y-2">
                            <div class="font-medium text-success-foreground">"Forecast Complete"</div>
                            {summary.map(|s| view! { <div class="text-muted-foreground">{s}</div> })}
                            <div class="grid grid-cols-2 gap-2">
                                {model_used.map(|m| metric_card("Model", m))}
                                {data_points.map(|d| metric_card("Data Points", d.to_string()))}
                            </div>
                            {(!forecast.is_empty()).then(|| {
                                let cols = vec!["Period".into(), "Forecast".into(), "Lower".into(), "Upper".into()];
                                let rows: Vec<Vec<String>> = forecast.iter().take(10).map(|point| {
                                    let period = str_field(point, "timestamp")
                                        .map(String::from)
                                        .or_else(|| u64_field(point, "step").map(|s| format!("Step {}", s)))
                                        .unwrap_or_default();
                                    let fc = f64_field(point, "forecast").map(|f| format!("{:.2}", f)).unwrap_or_default();
                                    let lower = f64_field(point, "lower_bound").map(|f| format!("{:.2}", f)).unwrap_or_default();
                                    let upper = f64_field(point, "upper_bound").map(|f| format!("{:.2}", f)).unwrap_or_default();
                                    vec![period, fc, lower, upper]
                                }).collect();
                                let total_count = forecast.len();
                                let remaining = total_count.saturating_sub(10);

                                view! {
                                    <div>
                                        <div class="text-muted-foreground text-xs mb-1">"Predictions:"</div>
                                        {compact_data_table(cols, rows, 10)}
                                        {(remaining > 0).then(|| view! {
                                            <div class="text-xs text-muted-foreground text-center py-1">
                                                {format!("...and {} more periods", remaining)}
                                            </div>
                                        })}
                                    </div>
                                }
                            })}
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

fn render_workspace_info(schema: &Value) -> impl IntoView {
    let output = schema.get("output");
    let has_error = output.is_some_and(|o| has_key(o, "error"));
    let has_workspace = output.is_some_and(|o| str_field(o, "workspace_name").is_some());

    view! {
        <div class="space-y-2">
            {output.map(|out| {
                if has_error {
                    error_block("Error", str_field(out, "error").unwrap_or("Unknown error").to_string()).into_any()
                } else if has_workspace {
                    let name = str_field(out, "workspace_name").unwrap_or("").to_string();
                    let msg = str_field(out, "message").unwrap_or("Workspace Info Retrieved").to_string();
                    let member_count = u64_field(out, "member_count").unwrap_or(0);
                    let email = str_field(out, "current_user_email").map(String::from);
                    let members = array_field(out, "members").cloned().unwrap_or_default();

                    view! {
                        <div class="bg-success p-2 rounded border border-success-border text-xs space-y-2">
                            <div class="font-medium text-success-foreground">{msg}</div>
                            <div class="bg-card p-2 rounded border border-border space-y-2">
                                <div class="font-medium text-foreground">{name}</div>
                                <div class="text-muted-foreground text-xs space-y-0.5">
                                    <div>{format!("Members: {}", member_count)}</div>
                                    {email.map(|e| view! { <div>"Your email: "<span class="font-mono">{e}</span></div> })}
                                </div>
                                {(!members.is_empty()).then(|| view! {
                                    <div class="bg-muted p-2 rounded border border-border text-xs max-h-40 overflow-y-auto">
                                        <div class="text-muted-foreground mb-1">"Members:"</div>
                                        <div class="space-y-1">
                                            {members.iter().map(|m| {
                                                let m_name = str_field(m, "name").unwrap_or("").to_string();
                                                let role = str_field(m, "role").unwrap_or("").to_string();
                                                let m_email = str_field(m, "email").unwrap_or("").to_string();
                                                let is_current = bool_field(m, "is_current_user").unwrap_or(false);
                                                view! {
                                                    <div class="flex items-center gap-2">
                                                        <span class="font-medium">{m_name}</span>
                                                        <span class="text-muted-foreground">{format!("({})", role)}</span>
                                                        <span class="font-mono text-muted-foreground">{m_email}</span>
                                                        {is_current.then(|| view! {
                                                            <span class="text-xs bg-primary/10 text-primary px-1 rounded">"you"</span>
                                                        })}
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>
                                })}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="bg-accent p-2 rounded border border-input text-xs">
                            <div class="text-muted-foreground">"No workspace data returned"</div>
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}

// -- Generic Fallback --

/// Tool labels for generic tools that only need a one-line description.
fn generic_tool_label(tool: &str) -> &str {
    match tool {
        "list_knowledge_files" => "Browsing knowledge files",
        "read_knowledge_file" => "Reading knowledge file",
        "write_knowledge_file" => "Writing knowledge file",
        "edit_knowledge_file" => "Editing knowledge file",
        "browse_catalog" => "Browsing catalog",
        _ => tool,
    }
}

fn render_generic(schema: &Value) -> impl IntoView {
    let tool = str_field(schema, "tool").unwrap_or("");
    let label = generic_tool_label(tool).to_string();
    let path = schema.get("input").and_then(|i| {
        str_field(i, "path").or_else(|| str_field(i, "datasource"))
    }).map(String::from);

    view! {
        <div class="flex items-center gap-2 text-sm text-muted-foreground">
            <span>
                {label}
                {path.map(|p| format!(": {}", p)).unwrap_or_default()}
            </span>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Main tool schema renderer — dispatches to tool-specific renderers by `schema.tool`.
///
/// Called from `agent_thinking.rs` to render tool execution results inline in the
/// thinking event list.
pub fn render_tool_schema(schema: Value) -> impl IntoView {
    let tool = match str_field(&schema, "tool") {
        Some(t) => t.to_string(),
        None => return view! { <span /> }.into_any(),
    };

    match tool.as_str() {
        // Data query renderers
        "estimate_query_cost" | "bigquery_cost_estimate" => {
            render_cost_estimate(&schema).into_any()
        }
        "query_datasource" | "bigquery_query" => render_query(&schema).into_any(),
        "validate_sql" => render_validate_sql(&schema).into_any(),
        "sample_table" | "bigquery_sample" => render_sample(&schema).into_any(),

        // Catalog & knowledge renderers
        "get_table_info" | "bigquery_table_info" => render_table_info(&schema).into_any(),
        "bigquery_search" | "search_catalog" => render_search_catalog(&schema).into_any(),
        "search_knowledge" => render_search_knowledge(&schema).into_any(),
        "list_datasources" => render_list_datasources(&schema).into_any(),
        "browse_resources" => render_browse_resources(&schema).into_any(),
        "read_resource" => render_read_resource(&schema).into_any(),

        // Dashboard renderers
        "update_dashboard" => render_update_dashboard(&schema).into_any(),
        "get_chartml_spec" => render_get_chartml_spec(&schema).into_any(),
        "update_chart" => render_update_chart(&schema).into_any(),
        "search_dashboards" => render_search_dashboards(&schema).into_any(),
        "get_dashboard_info" => render_get_dashboard_info(&schema).into_any(),
        "create_dashboard" => render_create_dashboard(&schema).into_any(),
        "modify_dashboard" => render_modify_dashboard(&schema).into_any(),
        "delete_dashboard" => render_delete_dashboard(&schema).into_any(),

        // Watch renderers
        "create_watch" => render_create_watch(&schema).into_any(),
        "preview_watch" => render_preview_watch(&schema).into_any(),
        "update_watch" => render_update_watch(&schema).into_any(),
        "search_watches" => render_search_watches(&schema).into_any(),
        "delete_watch" => render_delete_watch(&schema).into_any(),
        "trigger_watch" => render_trigger_watch(&schema).into_any(),
        "get_watch_info" => render_watch_info(&schema).into_any(),

        // Chart & misc renderers
        "validate_chartml" => render_validate_chartml(&schema).into_any(),
        "forecast_data" => render_forecast_data(&schema).into_any(),
        "get_workspace_info" => render_workspace_info(&schema).into_any(),

        // Generic fallback
        "list_knowledge_files" | "read_knowledge_file" | "write_knowledge_file"
        | "edit_knowledge_file" | "browse_catalog" => render_generic(&schema).into_any(),

        // Unknown tool — render generic with tool name
        _ => render_generic(&schema).into_any(),
    }
}
