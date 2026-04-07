// SPDX-License-Identifier: AGPL-3.0-or-later

//! Status bar between the SQL code editor and the results pane.
//!
//! React reference: `apps/frontend/src/components/SQLEditor.jsx` (lines 709-766)
//!
//! Layout:
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────┐
//! │ [dry run status]                                  Ln 1, Col 1      │
//! └──────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! - Left side: dry run validation status (spinner, checkmark, or error)
//! - Right side: cursor position (line and column)

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::Spinner;
use crate::server_fns::sql_editor::DryRunResult;

// ─── Dry run status display ──────────────────────────────────────────────────

/// The current dry run validation state, used to drive the status bar display.
#[derive(Clone, Debug, PartialEq)]
pub enum DryRunStatus {
    /// A dry run request is in flight.
    Validating,
    /// The dry run completed with a result.
    Complete(DryRunResult),
    /// No dry run has been performed yet (e.g. empty query, no datasource).
    Idle,
}

// ─── Status Bar component ────────────────────────────────────────────────────

/// Status bar displaying dry run validation status, cursor position, and Run Query button.
///
/// Matches the React status bar layout: `[dry run status] ... [Run Query button]`.
/// React reference: `apps/frontend/src/components/SQLEditor.jsx` (lines 709-766).
#[component]
pub fn StatusBar(
    /// Current dry run status — drives the left side of the status bar.
    #[prop(into)]
    dry_run_status: Signal<DryRunStatus>,
    /// Cursor line number (1-indexed).
    #[prop(into)]
    cursor_line: Signal<usize>,
    /// Cursor column number (1-indexed).
    #[prop(into)]
    cursor_col: Signal<usize>,
    /// Whether a query is currently running.
    #[prop(into)]
    query_running: Signal<bool>,
    /// Whether the Run Query button should be disabled (e.g. no datasource selected).
    #[prop(into)]
    run_disabled: Signal<bool>,
    /// Called when the user clicks "Run Query".
    on_run_query: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="px-4 py-2 border-t border-border bg-muted flex-shrink-0 overflow-hidden flex items-center justify-between" role="status" aria-live="polite">
            // Left side: dry run status
            <div class="flex-1 min-w-0">
                {move || {
                    let status = dry_run_status.get();
                    match status {
                        DryRunStatus::Validating => {
                            view! {
                                <div class="flex items-center gap-2 text-xs text-muted-foreground">
                                    <Spinner class="text-muted-foreground" />
                                    <span>"Validating query..."</span>
                                </div>
                            }.into_any()
                        }
                        DryRunStatus::Complete(result) => {
                            if result.valid {
                                view! {
                                    <div class="flex items-center gap-2 text-xs" style="min-height: 20px">
                                        <Icon icon=icondata_lu::LuCircleCheck attr:class="h-5 w-5 text-success-foreground" />
                                        <span class="text-muted-foreground">{result.message}</span>
                                    </div>
                                }.into_any()
                            } else {
                                let message = result.message.clone();
                                let is_auth_error = {
                                    let lower = message.to_lowercase();
                                    lower.contains("authentication")
                                        || lower.contains("unauthorized")
                                        || lower.contains("credentials")
                                        || lower.contains("oauth")
                                        || lower.contains("401")
                                };

                                if is_auth_error {
                                    view! {
                                        <div class="flex items-center gap-2 text-xs" style="min-height: 20px">
                                            <Icon icon=icondata_lu::LuTriangleAlert attr:class="h-5 w-5 text-warning-foreground flex-shrink-0" />
                                            <span class="text-warning-foreground text-xs truncate">
                                                "Authentication required - check datasource credentials in Settings"
                                            </span>
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="flex items-center gap-2 text-xs" style="min-height: 20px">
                                            <Icon icon=icondata_lu::LuTriangleAlert attr:class="h-5 w-5 text-error-foreground flex-shrink-0" />
                                            <span class="text-error-foreground text-xs truncate">
                                                {message}
                                            </span>
                                        </div>
                                    }.into_any()
                                }
                            }
                        }
                        DryRunStatus::Idle => {
                            // Empty placeholder to maintain consistent height.
                            view! {
                                <div class="text-xs" style="min-height: 20px"></div>
                            }.into_any()
                        }
                    }
                }}
            </div>

            // Right side: cursor position + Run Query button
            <div class="flex items-center gap-3 flex-shrink-0 ml-4">
                <div class="text-xs text-muted-foreground font-mono">
                    {move || format!("Ln {}, Col {}", cursor_line.get(), cursor_col.get())}
                </div>
                <button
                    on:click=move |_| on_run_query.run(())
                    disabled=move || query_running.get() || run_disabled.get()
                    class="px-3 py-1.5 text-xs font-medium bg-primary text-primary-foreground hover:bg-primary/90 rounded-md transition-colors disabled:opacity-50 disabled:cursor-not-allowed flex-shrink-0"
                >
                    {move || if query_running.get() { "Running..." } else { "Run Query" }}
                </button>
            </div>
        </div>
    }
}
