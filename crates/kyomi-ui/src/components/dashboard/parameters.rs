// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard parameter controls — matches
//! `apps/frontend/src/components/DashboardParameters.jsx`.
//!
//! Renders interactive parameter controls (select, multiselect, daterange,
//! number, text) in a responsive 12-column grid layout. Each parameter type
//! is rendered with the same CSS classes as the React frontend.

use std::collections::HashMap;

use leptos::prelude::*;

use crate::components::select::Select;
use crate::parser::ParamDef;

// ---------------------------------------------------------------------------
// CSS class constants — copied verbatim from React DashboardParameters.jsx
// ---------------------------------------------------------------------------

/// Outer container: `bg-card border border-border rounded-lg p-4 mb-6`.
const CONTAINER_CLASS: &str = "dashboard-filters bg-card border border-border rounded-lg p-4 mb-6";

/// Grid wrapper: 12-column grid with gap-4.
const GRID_CLASS: &str = "grid grid-cols-12 gap-4";

/// Label class from React: `block text-xs font-medium text-muted-foreground mb-1`.
const PARAM_LABEL_CLASS: &str = "block text-xs font-medium text-muted-foreground mb-1";

/// Date / number / text input class — from React DashboardParameters (daterange, number, text).
/// Note: React uses slightly different classes from INPUT_CLASS for these controls.
const PARAM_INPUT_CLASS: &str = "w-full px-3 py-2 bg-background border border-input rounded-md text-sm hover:border-ring/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

/// Date input inside daterange (uses flex-1 instead of w-full).
const DATE_INPUT_CLASS: &str = "flex-1 px-3 py-2 bg-background border border-input rounded-md text-sm hover:border-ring/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

// ---------------------------------------------------------------------------
// Helper: extract string options from Vec<serde_json::Value>
// ---------------------------------------------------------------------------

/// Convert `serde_json::Value` items to display strings.
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// Extract string options from a ParamDef's options field.
fn extract_options(param: &ParamDef) -> Vec<String> {
    param
        .options
        .as_ref()
        .map(|opts| opts.iter().map(value_to_string).collect())
        .unwrap_or_default()
}

/// Extract default value as a string.
fn default_as_string(param: &ParamDef) -> String {
    param
        .default
        .as_ref()
        .map(value_to_string)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Col-span calculation — mirrors React getColSpanClass
// ---------------------------------------------------------------------------

/// Compute the responsive col-span CSS class for a parameter cell.
///
/// Logic from React `DashboardParameters.jsx`:
/// - If explicit `param.layout.col_span` exists, use it for the `md:` breakpoint.
/// - Otherwise auto-calculate: 1 param → 12, 2 → 6, 3 → 4, 4+ → 3.
/// - Mobile is always `col-span-12`.
fn get_col_span_class(param: &ParamDef, total_params: usize) -> &'static str {
    if let Some(ref layout) = param.layout
        && let Some(cs) = layout.col_span
    {
        return match cs {
            1 => "col-span-12 md:col-span-1",
            2 => "col-span-12 md:col-span-2",
            3 => "col-span-12 md:col-span-3",
            4 => "col-span-12 md:col-span-4",
            5 => "col-span-12 md:col-span-5",
            6 => "col-span-12 md:col-span-6",
            7 => "col-span-12 md:col-span-7",
            8 => "col-span-12 md:col-span-8",
            9 => "col-span-12 md:col-span-9",
            10 => "col-span-12 md:col-span-10",
            11 => "col-span-12 md:col-span-11",
            12 => "col-span-12",
            _ => "col-span-12 md:col-span-3",
        };
    }

    let auto = match total_params {
        1 => 12,
        2 => 6,
        3 => 4,
        _ => 3,
    };

    match auto {
        3 => "col-span-12 md:col-span-3",
        4 => "col-span-12 md:col-span-4",
        6 => "col-span-12 md:col-span-6",
        12 => "col-span-12",
        _ => "col-span-12 md:col-span-3",
    }
}

// ---------------------------------------------------------------------------
// DashboardParameters — main component
// ---------------------------------------------------------------------------

/// Dashboard parameter controls panel.
///
/// Renders a responsive grid of parameter controls (select, multiselect,
/// daterange, number, text) matching the React `DashboardParameters` component.
///
/// # Parameters
/// - `params` — parameter definitions from the parsed ChartML document.
/// - `values` — reactive map of current parameter values (keyed by param ID).
/// - `set_values` — write signal to update the values map.
#[component]
pub fn DashboardParameters(
    params: Vec<ParamDef>,
    #[prop(into)] values: Signal<HashMap<String, String>>,
    set_values: WriteSignal<HashMap<String, String>>,
) -> impl IntoView {
    let total_params = params.len();

    // Initialize defaults for params that don't have values yet.
    // Runs once on mount, similar to the React useEffect.
    {
        let params_for_init = params.clone();
        Effect::new(move |prev: Option<bool>| {
            // Only run once (on first render).
            if prev.is_some() {
                return true;
            }
            let current = values.get();
            let mut updated = current.clone();
            let mut has_new = false;

            for param in &params_for_init {
                if !updated.contains_key(&param.id)
                    && let Some(ref default) = param.default
                {
                    // For daterange, store as {id}_start and {id}_end
                    if param.param_type == "daterange" {
                        if let Some(arr) = default.as_array() {
                            if arr.len() >= 2 {
                                let start_key = format!("{}_start", param.id);
                                let end_key = format!("{}_end", param.id);
                                if let std::collections::hash_map::Entry::Vacant(e) = updated.entry(start_key) {
                                    e.insert(value_to_string(&arr[0]));
                                    has_new = true;
                                }
                                if let std::collections::hash_map::Entry::Vacant(e) = updated.entry(end_key) {
                                    e.insert(value_to_string(&arr[1]));
                                    has_new = true;
                                }
                            }
                        } else {
                            // Single default for daterange — store as the param id
                            updated.insert(param.id.clone(), value_to_string(default));
                            has_new = true;
                        }
                    } else if param.param_type == "multiselect" {
                        // Multiselect default is an array → join as comma-separated
                        if let Some(arr) = default.as_array() {
                            let csv = arr.iter().map(value_to_string).collect::<Vec<_>>().join(",");
                            updated.insert(param.id.clone(), csv);
                            has_new = true;
                        } else {
                            updated.insert(param.id.clone(), value_to_string(default));
                            has_new = true;
                        }
                    } else {
                        updated.insert(param.id.clone(), value_to_string(default));
                        has_new = true;
                    }
                }
            }

            if has_new {
                set_values.set(updated);
            }
            true
        });
    }

    let param_views = params
        .into_iter()
        .map(|param| {
            let col_class = get_col_span_class(&param, total_params);
            let label_text = param
                .label
                .clone()
                .unwrap_or_else(|| param.id.clone());

            match param.param_type.as_str() {
                "select" => {
                    let param_id = param.id.clone();
                    let options = extract_options(&param);
                    let default_val = default_as_string(&param);

                    // Build reactive value signal for this param
                    let param_id_for_value = param_id.clone();
                    let value_signal = Signal::derive(move || {
                        values
                            .get()
                            .get(&param_id_for_value)
                            .cloned()
                            .unwrap_or_else(|| default_val.clone())
                    });

                    // Build options signal for Select
                    let options_signal = Signal::derive(move || {
                        options.iter().map(|o| (o.clone(), o.clone())).collect::<Vec<_>>()
                    });

                    let param_id_for_change = param_id.clone();
                    let on_select_change = move |new_val: String| {
                        let mut map = values.get();
                        map.insert(param_id_for_change.clone(), new_val);
                        set_values.set(map);
                    };

                    view! {
                        <div class=col_class>
                            <label class=PARAM_LABEL_CLASS>{label_text}</label>
                            <Select
                                value=value_signal
                                options=options_signal
                                on_change=on_select_change
                                placeholder="Select..."
                            />
                        </div>
                    }
                    .into_any()
                }

                "multiselect" => {
                    let param_id = param.id.clone();
                    let options = extract_options(&param);
                    let default_val = default_as_string(&param);

                    let param_id_for_value = param_id.clone();
                    let current_value_signal = Signal::derive(move || {
                        values
                            .get()
                            .get(&param_id_for_value)
                            .cloned()
                            .unwrap_or_else(|| default_val.clone())
                    });

                    let param_id_for_change = param_id.clone();
                    let on_multi_change = move |new_val: String| {
                        let mut map = values.get();
                        map.insert(param_id_for_change.clone(), new_val);
                        set_values.set(map);
                    };

                    let multi_options_signal = Signal::derive(move || {
                        options.iter().map(|o| (o.clone(), o.clone())).collect::<Vec<_>>()
                    });

                    view! {
                        <div class=col_class>
                            <label class=PARAM_LABEL_CLASS>{label_text}</label>
                            <Select
                                value=current_value_signal
                                options=multi_options_signal
                                on_change=on_multi_change
                                multi=true
                                placeholder="Select..."
                            />
                        </div>
                    }
                    .into_any()
                }

                "daterange" => {
                    let param_id = param.id.clone();
                    let default = param.default.clone();

                    // Extract default start/end from array default
                    let (default_start, default_end) = match &default {
                        Some(serde_json::Value::Array(arr)) if arr.len() >= 2 => {
                            (value_to_string(&arr[0]), value_to_string(&arr[1]))
                        }
                        _ => (String::new(), String::new()),
                    };

                    let start_key = format!("{}_start", param_id);
                    let end_key = format!("{}_end", param_id);

                    let start_key_for_read = start_key.clone();
                    let default_start_clone = default_start.clone();
                    let start_value = Signal::derive(move || {
                        values
                            .get()
                            .get(&start_key_for_read)
                            .cloned()
                            .unwrap_or_else(|| default_start_clone.clone())
                    });

                    let end_key_for_read = end_key.clone();
                    let default_end_clone = default_end.clone();
                    let end_value = Signal::derive(move || {
                        values
                            .get()
                            .get(&end_key_for_read)
                            .cloned()
                            .unwrap_or_else(|| default_end_clone.clone())
                    });

                    let start_key_for_change = start_key.clone();
                    let end_key_for_change = end_key.clone();

                    view! {
                        <div class=col_class>
                            <label class=PARAM_LABEL_CLASS>{label_text}</label>
                            <div class="flex items-center gap-2">
                                <input
                                    type="date"
                                    class=DATE_INPUT_CLASS
                                    prop:value=move || start_value.get()
                                    on:change=move |ev| {
                                        let new_val = event_target_value(&ev);
                                        let mut map = values.get();
                                        map.insert(start_key_for_change.clone(), new_val);
                                        set_values.set(map);
                                    }
                                />
                                <span class="text-xs text-muted-foreground">"to"</span>
                                <input
                                    type="date"
                                    class=DATE_INPUT_CLASS
                                    prop:value=move || end_value.get()
                                    on:change=move |ev| {
                                        let new_val = event_target_value(&ev);
                                        let mut map = values.get();
                                        map.insert(end_key_for_change.clone(), new_val);
                                        set_values.set(map);
                                    }
                                />
                            </div>
                        </div>
                    }
                    .into_any()
                }

                "number" => {
                    let param_id = param.id.clone();
                    let default_val = default_as_string(&param);

                    let param_id_for_read = param_id.clone();
                    let number_value = Signal::derive(move || {
                        values
                            .get()
                            .get(&param_id_for_read)
                            .cloned()
                            .unwrap_or_else(|| default_val.clone())
                    });

                    let param_id_for_change = param_id.clone();

                    view! {
                        <div class=col_class>
                            <label class=PARAM_LABEL_CLASS>{label_text}</label>
                            <input
                                type="number"
                                class=PARAM_INPUT_CLASS
                                prop:value=move || number_value.get()
                                on:change=move |ev| {
                                    let new_val = event_target_value(&ev);
                                    let mut map = values.get();
                                    map.insert(param_id_for_change.clone(), new_val);
                                    set_values.set(map);
                                }
                            />
                        </div>
                    }
                    .into_any()
                }

                "text" => {
                    let param_id = param.id.clone();
                    let default_val = default_as_string(&param);
                    let placeholder = param.placeholder.clone().unwrap_or_default();

                    let param_id_for_read = param_id.clone();
                    let text_value = Signal::derive(move || {
                        values
                            .get()
                            .get(&param_id_for_read)
                            .cloned()
                            .unwrap_or_else(|| default_val.clone())
                    });

                    let param_id_for_change = param_id.clone();

                    view! {
                        <div class=col_class>
                            <label class=PARAM_LABEL_CLASS>{label_text}</label>
                            <input
                                type="text"
                                class=PARAM_INPUT_CLASS
                                placeholder=placeholder
                                prop:value=move || text_value.get()
                                on:input=move |ev| {
                                    let new_val = event_target_value(&ev);
                                    let mut map = values.get();
                                    map.insert(param_id_for_change.clone(), new_val);
                                    set_values.set(map);
                                }
                            />
                        </div>
                    }
                    .into_any()
                }

                unknown => {
                    let msg = format!("Unknown: {}", unknown);
                    view! {
                        <div class=col_class>
                            <p class="text-xs text-error-foreground">{msg}</p>
                        </div>
                    }
                    .into_any()
                }
            }
        })
        .collect_view();

    view! {
        <div class=CONTAINER_CLASS>
            <div class=GRID_CLASS>
                {param_views}
            </div>
        </div>
    }
}
