// SPDX-License-Identifier: AGPL-3.0-or-later

//! ExecutionSelector component — matches
//! `apps/frontend/src/components/watches/ExecutionSelector.jsx` exactly.
//!
//! Dropdown to select which watch execution run to view. Shows all
//! executions in reverse chronological order with status badge and timestamp.

use leptos::prelude::*;

use crate::components::Select;
use crate::types::WatchExecutionItem;

// ---------------------------------------------------------------------------
// Status helpers (match the React source exactly)
// ---------------------------------------------------------------------------

/// Map execution status to a Badge variant.
///
/// React uses: success -> "success", error -> "destructive", no_alert -> "secondary",
/// running -> "info", default -> "outline".
///
/// The Leptos Badge component has: Default, Secondary, Destructive, Warning, Outline.
/// It does not have "success" or "info" variants. We map:
/// - success -> Default (primary/amber — the closest active variant)
///
/// Map execution status to a human-readable label (matching React exactly).
fn get_status_label(status: &str) -> &'static str {
    match status {
        "success" => "Alert",
        "no_alert" => "No Alert",
        "error" => "Error",
        "running" => "Running",
        _ => "Unknown",
    }
}

/// Format a date string for display (matching React's toLocaleString format).
///
/// Input is an RFC 3339 string. Output: "Mon DD, HH:MM AM/PM" style.
/// Since we don't have full locale support in WASM, we format manually.
fn format_date(date_str: &str) -> String {
    if date_str.is_empty() {
        return "Unknown".into();
    }

    // Try to use JS Date for proper locale formatting in WASM
    #[cfg(target_arch = "wasm32")]
    {
        let js_date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(date_str));
        if js_date.get_time().is_nan() {
            return "Unknown".into();
        }

        let month = match js_date.get_month() {
            0 => "Jan",
            1 => "Feb",
            2 => "Mar",
            3 => "Apr",
            4 => "May",
            5 => "Jun",
            6 => "Jul",
            7 => "Aug",
            8 => "Sep",
            9 => "Oct",
            10 => "Nov",
            11 => "Dec",
            _ => "???",
        };
        let day = js_date.get_date();
        let hours = js_date.get_hours();
        let minutes = js_date.get_minutes();
        let hour_12 = if hours == 0 {
            12
        } else if hours > 12 {
            hours - 12
        } else {
            hours
        };
        let ampm = if hours < 12 { "AM" } else { "PM" };

        format!("{month} {day}, {hour_12}:{minutes:02} {ampm}")
    }

    // Server-side fallback: just return the raw string truncated
    #[cfg(not(target_arch = "wasm32"))]
    {
        if date_str.len() > 16 {
            date_str[..16].to_string()
        } else {
            date_str.to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Dropdown to select which watch execution run to view.
///
/// Shows all executions in reverse chronological order with status badge
/// and timestamp. The first execution is labeled "(latest)".
///
/// Ported from `apps/frontend/src/components/watches/ExecutionSelector.jsx`.
#[component]
pub fn ExecutionSelector(
    /// List of executions (without trace).
    executions: Vec<WatchExecutionItem>,
    /// Currently selected execution ID.
    #[prop(into)]
    selected_id: Signal<Option<i32>>,
    /// Callback when user selects a different execution.
    on_select: Callback<i32>,
) -> impl IntoView {
    if executions.is_empty() {
        return ().into_any();
    }

    // Default to first execution if none selected
    let first_id = executions[0].id;

    // Build a display string per execution for the select dropdown.
    // Select uses (value, label) pairs of Strings.
    let options: Vec<(String, String)> = executions
        .iter()
        .enumerate()
        .map(|(index, exec)| {
            let status_label = get_status_label(&exec.status);
            let date_label = format_date(&exec.started_at);
            let latest_suffix = if index == 0 { " (latest)" } else { "" };
            let label = format!("{status_label} - {date_label}{latest_suffix}");
            (exec.id.to_string(), label)
        })
        .collect();

    let options_signal = Signal::derive(move || options.clone());

    let current_value = Signal::derive(move || {
        selected_id
            .get()
            .unwrap_or(first_id)
            .to_string()
    });

    view! {
        <div class="flex items-center gap-2">
            <span class="text-sm text-muted-foreground">"Execution run:"</span>
            <div class="w-[280px]">
                <Select
                    value=current_value
                    options=options_signal
                    on_change=move |v: String| {
                        if let Ok(id) = v.parse::<i32>() {
                            on_select.run(id);
                        }
                    }
                    placeholder="Select execution"
                />
            </div>
        </div>
    }
    .into_any()
}
