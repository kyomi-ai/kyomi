// SPDX-License-Identifier: AGPL-3.0-or-later

//! ExecutionLogViewer component — matches
//! `apps/frontend/src/components/watches/ExecutionLogViewer.jsx` exactly.
//!
//! Shows watch execution as a chat-like conversation:
//! - Watch prompt as a user message bubble (right-aligned, primary color)
//! - Agent response as an assistant message card with markdown content
//! - Summary header with status badge, timestamp, and duration
//! - Error message display if execution errored

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::dashboard::MarkdownRenderer;
use crate::components::{Badge, BadgeVariant, Spinner};
use crate::types::WatchExecutionItem;

use super::ExecutionSelector;

// ---------------------------------------------------------------------------
// Status helpers
// ---------------------------------------------------------------------------

/// Get the status badge view for an execution status.
///
/// React mapping:
/// - success -> Badge variant="success" "Alert Triggered"
/// - no_alert -> Badge variant="secondary" "No Alert"
/// - error -> Badge variant="destructive" "Error"
/// - running -> Badge variant="info" with Spinner + "Running"
/// - default -> Badge variant="outline" {status}
///
/// The Leptos Badge only has Default, Secondary, Destructive, Warning, Outline.
/// We map success -> Default, info/running -> Default.
fn status_badge_view(status: &str) -> impl IntoView + use<> {
    match status {
        "success" => view! {
            <Badge variant=BadgeVariant::Default>"Alert Triggered"</Badge>
        }
        .into_any(),
        "no_alert" => view! {
            <Badge variant=BadgeVariant::Secondary>"No Alert"</Badge>
        }
        .into_any(),
        "error" => view! {
            <Badge variant=BadgeVariant::Destructive>"Error"</Badge>
        }
        .into_any(),
        "running" => view! {
            <Badge variant=BadgeVariant::Default>
                <Spinner class="mr-1"/>
                "Running"
            </Badge>
        }
        .into_any(),
        other => {
            let label = other.to_string();
            view! {
                <Badge variant=BadgeVariant::Outline>{label}</Badge>
            }
            .into_any()
        }
    }
}

// ---------------------------------------------------------------------------
// Date/Duration formatting helpers
// ---------------------------------------------------------------------------

/// Format duration between two RFC 3339 timestamps.
fn format_duration(started_at: &str, completed_at: &str) -> Option<String> {
    if started_at.is_empty() || completed_at.is_empty() {
        return None;
    }

    #[cfg(target_arch = "wasm32")]
    {
        let start = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(started_at));
        let end = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(completed_at));
        let start_ms = start.get_time();
        let end_ms = end.get_time();
        if start_ms.is_nan() || end_ms.is_nan() {
            return None;
        }
        let duration_ms = (end_ms - start_ms) as u64;
        if duration_ms < 1000 {
            Some(format!("{duration_ms}ms"))
        } else {
            Some(format!("{:.1}s", duration_ms as f64 / 1000.0))
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // Server-side: attempt simple parsing
        let _ = (started_at, completed_at);
        None
    }
}

/// Format a timestamp for display.
fn format_timestamp(date_str: &str) -> String {
    if date_str.is_empty() {
        return String::new();
    }

    #[cfg(target_arch = "wasm32")]
    {
        let js_date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(date_str));
        if js_date.get_time().is_nan() {
            return String::new();
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

/// Shows watch execution as a chat-like conversation.
///
/// Displays:
/// - Watch prompt as a user message bubble
/// - Agent response as an assistant message bubble
/// - Execution selector when multiple executions exist
/// - Summary header with status, timestamp, and duration
/// - Error message display
///
/// Ported from `apps/frontend/src/components/watches/ExecutionLogViewer.jsx`.
#[component]
pub fn ExecutionLogViewer(
    /// List of executions (without trace).
    executions: Vec<WatchExecutionItem>,
    /// Full execution with trace (the currently selected one).
    #[prop(into)]
    selected_execution: Signal<Option<WatchExecutionItem>>,
    /// Callback when user selects a different execution run.
    on_select_execution: Callback<i32>,
    /// Whether execution data is currently loading.
    #[prop(into)]
    is_loading: Signal<bool>,
    /// The watch's monitoring instruction prompt.
    watch_prompt: String,
) -> impl IntoView {
    let executions_len = executions.len();
    let executions_for_selector = executions.clone();

    // No executions at all
    if executions.is_empty() {
        return view! {
            {move || {
                if is_loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-8 gap-2 text-muted-foreground">
                            <Spinner/>
                            <span>"Loading execution..."</span>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="text-center py-8 text-muted-foreground">
                            "No executions yet. Run the watch to see execution logs."
                        </div>
                    }.into_any()
                }
            }}
        }
        .into_any();
    }

    let watch_prompt_clone = watch_prompt.clone();

    view! {
        <div class="space-y-4">
            // Loading state — exclusive: when loading with no selection, show only spinner
            // Matches React's early-return pattern where spinner replaces all content
            {move || {
                if is_loading.get() && selected_execution.get().is_none() {
                    return view! {
                        <div class="flex items-center justify-center py-8 gap-2 text-muted-foreground">
                            <Spinner/>
                            <span>"Loading execution..."</span>
                        </div>
                    }.into_any();
                }

                let execs = executions_for_selector.clone();
                view! {
                    // Execution Selector (only when multiple executions)
                    {(executions_len > 1).then(|| {
                        let execs = execs.clone();
                        view! {
                            <ExecutionSelector
                                executions=execs
                                selected_id=Signal::derive(move || {
                                    selected_execution.get().map(|e| e.id)
                                })
                                on_select=on_select_execution
                            />
                        }
                    })}
                }.into_any()
            }}

            // Summary Header + Error + Chat view (only when execution is selected)
            {move || {
                let watch_prompt_inner = watch_prompt_clone.clone();
                selected_execution.get().map(|exec| {
                    let status = exec.status.clone();
                    let error_message = exec.error_message.clone();
                    let agent_response = exec.agent_response.clone();
                    let started_at = exec.started_at.clone();
                    let completed_at = exec.completed_at.clone().unwrap_or_default();

                    let duration = format_duration(&started_at, &completed_at);
                    let timestamp = format_timestamp(&started_at);

                    let prompt_to_show = if !watch_prompt_inner.is_empty() {
                        watch_prompt_inner.clone()
                    } else {
                        "Watch monitoring instruction".to_string()
                    };

                    let status_badge = status_badge_view(&status);

                    view! {
                        // Summary Header
                        <div class="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
                            <div class="flex items-center gap-3">
                                {status_badge}
                                <span class="text-xs text-muted-foreground">
                                    {timestamp}
                                </span>
                            </div>
                            {duration.map(|d| view! {
                                <span class="text-xs text-muted-foreground">{d}</span>
                            })}
                        </div>

                        // Error Message
                        {error_message.clone().map(|err_msg| view! {
                            <div class="p-3 bg-error/10 border border-error-border rounded-lg">
                                <div class="flex items-start gap-2">
                                    <Icon icon=phosphor_leptos::X_CIRCLE attr:class="h-4 w-4 text-error-foreground mt-0.5 shrink-0"/>
                                    <div>
                                        <p class="text-sm font-medium text-error-foreground">"Execution Error"</p>
                                        <p class="text-sm text-error-foreground/80 mt-1">{err_msg}</p>
                                    </div>
                                </div>
                            </div>
                        })}

                        // Chat-like Conversation View
                        <div class="space-y-4 py-4">
                            // User Message - Watch Prompt
                            <div class="flex flex-col items-end">
                                <div class="max-w-sm sm:max-w-md lg:max-w-lg xl:max-w-2xl px-4 py-3 text-primary-foreground bg-primary rounded-2xl shadow-sm text-sm">
                                    {prompt_to_show}
                                </div>
                            </div>

                            // Assistant Message - Response
                            <div class="flex flex-col items-start">
                                <div class="w-full px-6 py-4 bg-card border border-border rounded-2xl shadow-sm overflow-hidden">
                                    {if let Some(response) = agent_response {
                                        view! {
                                            <MarkdownRenderer
                                                content=Signal::derive(move || response.clone())
                                            />
                                        }.into_any()
                                    } else if status == "running" {
                                        view! {
                                            <div class="flex items-center gap-2 text-muted-foreground">
                                                <Spinner/>
                                                <span class="text-sm">"Processing..."</span>
                                            </div>
                                        }.into_any()
                                    } else if error_message.is_none() {
                                        view! {
                                            <p class="text-sm text-muted-foreground italic">
                                                "No response generated"
                                            </p>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }}
                                </div>
                            </div>
                        </div>
                    }
                })
            }}
        </div>
    }
    .into_any()
}
