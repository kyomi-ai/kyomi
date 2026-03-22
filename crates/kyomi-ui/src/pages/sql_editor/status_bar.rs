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

// ─── SVG Icons ───────────────────────────────────────────────────────────────

/// Checkmark circle icon (Heroicons outline) for valid dry run status.
#[component]
fn CheckCircleIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M9 12.75 11.25 15 15 9.75M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
        </svg>
    }
}

/// Exclamation triangle icon (Heroicons outline) for error/warning dry run status.
#[component]
fn ExclamationTriangleIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />
        </svg>
    }
}

// ─── Status Bar component ────────────────────────────────────────────────────

/// Status bar displaying dry run validation status and cursor position.
///
/// Matches the React status bar styling: `px-4 py-2 border-t border-border
/// bg-muted flex-shrink-0 overflow-hidden flex items-center justify-between`.
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
                                        <CheckCircleIcon class="h-5 w-5 text-success-foreground" />
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
                                            <ExclamationTriangleIcon class="h-5 w-5 text-warning-foreground flex-shrink-0" />
                                            <span class="text-warning-foreground text-xs truncate">
                                                "Authentication required - check datasource credentials in Settings"
                                            </span>
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="flex items-center gap-2 text-xs" style="min-height: 20px">
                                            <ExclamationTriangleIcon class="h-5 w-5 text-error-foreground flex-shrink-0" />
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

            // Right side: cursor position
            <div class="text-xs text-muted-foreground font-mono flex-shrink-0 ml-4">
                {move || format!("Ln {}, Col {}", cursor_line.get(), cursor_col.get())}
            </div>
        </div>
    }
}
