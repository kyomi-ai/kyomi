// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Code Editor — wraps kode-leptos `CodeEditor` with SQL-specific features.
//!
//! React reference: `apps/frontend/src/components/MonacoSQLEditor.jsx` (~406 lines)
//!
//! Features:
//! - kode-leptos `CodeEditor` with `Language::Sql`
//! - Keyboard shortcut: Cmd/Ctrl+Enter to run query
//! - Cursor position display (line:column) via signals
//! - Error display in the status bar (from dry run results)
//! - Debounced dry run validation (1 second after typing stops)
//! - WASM-only rendering with SSR placeholder

#[allow(unused_imports)]
use std::sync::Arc;

use leptos::prelude::*;

use crate::server_fns::sql_editor::DryRunResult;
#[allow(unused_imports)]
use crate::server_fns::sql_editor::dry_run_sql;

use super::status_bar::{DryRunStatus, StatusBar};

// ─── Debounced dry run hook ──────────────────────────────────────────────────

/// Creates debounced dry run validation that watches query text changes.
///
/// React reference: `apps/frontend/src/hooks/useSQLDryRun.js`
///
/// - Watches `query_text` for changes
/// - After 1 second of no changes, calls `dry_run_sql()` server function
/// - Updates the `dry_run_status` signal with results
/// - Cancels pending requests when user types again
///
/// The debounce timer is managed via `gloo_timers::callback::Timeout` which
/// auto-cancels when dropped (the previous timeout is replaced by a new one).
#[cfg(target_arch = "wasm32")]
fn use_debounced_dry_run(
    query_text: Signal<String>,
    datasource_slug: Signal<Option<String>>,
) -> RwSignal<DryRunStatus> {
    use gloo_timers::callback::Timeout;
    use send_wrapper::SendWrapper;
    use std::cell::Cell;
    use std::rc::Rc;

    let dry_run_status = RwSignal::new(DryRunStatus::Idle);

    // Hold the pending timeout. Replacing the Cell value drops the previous
    // Timeout, which auto-cancels the callback — matching the React pattern
    // of `clearTimeout(timeoutRef.current)`.
    let pending = Rc::new(Cell::new(None::<SendWrapper<Timeout>>));

    Effect::new(move |_| {
        // Subscribe to reactive signals so this effect re-runs on changes.
        let sql = query_text.get();
        let slug = datasource_slug.get();

        // Cancel any pending dry run by dropping the old timeout.
        pending.set(None);

        // Skip if no SQL or no datasource selected.
        let sql_trimmed = sql.trim().to_string();
        if sql_trimmed.is_empty() {
            dry_run_status.set(DryRunStatus::Idle);
            return;
        }

        let Some(slug) = slug else {
            dry_run_status.set(DryRunStatus::Idle);
            return;
        };

        if slug.is_empty() {
            dry_run_status.set(DryRunStatus::Idle);
            return;
        }

        // NOTE: React's useSQLDryRun reads getSelectedOrFullText() to validate only
        // the selected portion. kode-leptos doesn't expose a selection API yet.
        // When it does, prefer selected text over full text for dry run validation.

        // Schedule dry run after 1 second of inactivity (matches React's debounce).
        let pending_clone = pending.clone();
        let timeout = Timeout::new(1_000, move || {
            // Clear the pending handle since this callback is now executing.
            pending_clone.set(None);

            // Mark as validating.
            dry_run_status.set(DryRunStatus::Validating);

            // Spawn the server function call.
            leptos::task::spawn_local(async move {
                // NOTE: React's MonacoSQLEditor calls editorRef.setErrorMarkers() to show
                // red squiggly underlines at the error location. kode-leptos does not expose
                // an error annotation API. The error location is shown in the status bar instead.
                // When kode-leptos adds error markers, wire them up here using dry_run.line/column.
                match dry_run_sql(slug, sql_trimmed).await {
                    Ok(result) => {
                        dry_run_status.set(DryRunStatus::Complete(result));
                    }
                    Err(e) => {
                        dry_run_status.set(DryRunStatus::Complete(DryRunResult {
                            valid: false,
                            message: format!("Validation error: {e}"),
                            line: None,
                            column: None,
                            bytes_processed: None,
                        }));
                    }
                }
            });
        });

        pending.set(Some(SendWrapper::new(timeout)));
    });

    dry_run_status
}

// ─── Keyboard shortcut hook ──────────────────────────────────────────────────

/// Sets up a global `keydown` listener for Cmd/Ctrl+Enter to run the query.
///
/// This is a document-level listener (not tied to the editor element) because
/// kode-leptos handles its own keyboard events internally and we cannot inject
/// custom key bindings into its textarea.
#[cfg(target_arch = "wasm32")]
fn use_run_shortcut(on_run: Option<Callback<()>>, query_text: Signal<String>) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    Effect::new(move |_| {
        let Some(on_run) = on_run else { return };
        let Some(window) = web_sys::window() else { return };
        let Some(document) = window.document() else { return };

        let query_text = query_text.clone();
        let closure = Closure::wrap(Box::new(move |ev: web_sys::KeyboardEvent| {
            let is_meta = ev.meta_key() || ev.ctrl_key();
            if is_meta && ev.key() == "Enter" {
                let sql = query_text.get_untracked();
                if !sql.trim().is_empty() {
                    ev.prevent_default();
                    on_run.run(());
                }
            }
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);

        let _ = document.add_event_listener_with_callback(
            "keydown",
            closure.as_ref().unchecked_ref(),
        );

        // Move the Closure into the cleanup callback so it stays alive as long as
        // the listener exists, and is properly dropped when the component unmounts
        // (instead of using closure.forget() which permanently leaks memory).
        // SendWrapper is required because Closure is !Send but on_cleanup needs Send+Sync.
        let document_clone = document.clone();
        let closure_ref = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        let closure_wrapper = send_wrapper::SendWrapper::new(closure);
        on_cleanup(move || {
            let _ = document_clone.remove_event_listener_with_callback("keydown", &closure_ref);
            drop(closure_wrapper);
        });
    });
}

// ─── Cursor position tracking ────────────────────────────────────────────────

/// Tracks cursor position from the kode-leptos editor by polling the editor's
/// internal state. kode-leptos does not expose a cursor change callback, so we
/// observe the cursor element position via a periodic check.
///
/// Returns (line, column) signals (1-indexed to match the React display).
#[cfg(target_arch = "wasm32")]
fn use_cursor_position() -> (RwSignal<usize>, RwSignal<usize>) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let line = RwSignal::new(1usize);
    let col = RwSignal::new(1usize);

    // Use a MutationObserver + polling hybrid: poll every 200ms when focused.
    // This is pragmatic given kode-leptos doesn't expose cursor position callbacks.
    Effect::new(move |_| {
        let Some(window) = web_sys::window() else { return };

        let closure = Closure::wrap(Box::new(move || {
            let Some(document) = web_sys::window().and_then(|w| w.document()) else {
                return;
            };

            // Read cursor position from kode-leptos's cursor element.
            // The cursor is positioned absolutely with `top` in increments of 20px
            // (LINE_HEIGHT) and its existence tells us the cursor's line.
            // We also check the hidden textarea's selectionStart for column info.
            if let Some(cursor_el) = document.get_element_by_id("kode-cursor-el") {
                let html_el: &web_sys::HtmlElement = cursor_el.unchecked_ref();
                let style = html_el.style();

                // Parse line from `top` CSS property (each line is 20px)
                if let Ok(top_str) = style.get_property_value("top") {
                    if let Some(top_px) = top_str.strip_suffix("px") {
                        if let Ok(top) = top_px.parse::<f64>() {
                            let new_line = (top / 20.0).round() as usize + 1;
                            if line.get_untracked() != new_line {
                                line.set(new_line);
                            }
                        }
                    }
                }

                // Parse column from `left` CSS property.
                // This is approximate — kode-leptos uses variable-width measurement,
                // but we can get the column from the hidden textarea's selection.
                if let Some(textarea) = document.query_selector(".kode-hidden-textarea").ok().flatten() {
                    let textarea: &web_sys::HtmlTextAreaElement = textarea.unchecked_ref();
                    let value = textarea.value();
                    let sel_start = textarea.selection_start().ok().flatten().unwrap_or(0) as usize;

                    // Count characters from the last newline to selection_start.
                    // This gives us the column position.
                    let text_before = &value[..sel_start.min(value.len())];
                    let new_col = match text_before.rfind('\n') {
                        Some(pos) => sel_start - pos,
                        None => sel_start + 1,
                    };
                    if col.get_untracked() != new_col {
                        col.set(new_col);
                    }
                }
            }
        }) as Box<dyn FnMut()>);

        let interval_id = window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                200,
            )
            .unwrap_or(0);

        // Move the Closure into the cleanup callback so it stays alive as long as
        // the interval exists, and is properly dropped when the component unmounts
        // (instead of using closure.forget() which permanently leaks memory).
        // SendWrapper is required because Closure is !Send but on_cleanup needs Send+Sync.
        let closure_wrapper = send_wrapper::SendWrapper::new(closure);
        on_cleanup(move || {
            if let Some(window) = web_sys::window() {
                window.clear_interval_with_handle(interval_id);
            }
            drop(closure_wrapper);
        });
    });

    (line, col)
}

// ─── SqlCodeEditor component ─────────────────────────────────────────────────

/// SQL Code Editor wrapping kode-leptos `CodeEditor` with SQL-specific features.
///
/// Renders the kode-leptos `CodeEditor` on WASM targets with SQL highlighting,
/// keyboard shortcuts (Cmd/Ctrl+Enter), cursor position tracking, and debounced
/// dry run validation. On SSR, renders a loading placeholder.
///
/// The `dry_run_result` prop is read-only and used to display error information
/// in the integrated status bar. If not provided, an internal dry run system
/// is used (requires `datasource_slug` to be set).
#[component]
pub fn SqlCodeEditor(
    /// The SQL text content — two-way bound via `RwSignal`.
    content: RwSignal<String>,
    /// Callback invoked when Cmd/Ctrl+Enter is pressed (run query).
    #[prop(optional)]
    on_run: Option<Callback<()>>,
    /// External dry run result to display in the status bar.
    /// When provided, the internal debounced dry run is bypassed.
    #[prop(optional, into)]
    dry_run_result: Option<Signal<Option<DryRunResult>>>,
    /// Datasource slug for internal dry run validation.
    /// Required when `dry_run_result` is not provided.
    #[prop(optional, into)]
    datasource_slug: Option<Signal<Option<String>>>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use kode_leptos::{CodeEditor, Language};

        let content_signal: Signal<String> = content.into();

        // Set up the on_change callback to write back to the RwSignal.
        let on_change: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |new_text: String| {
            content.set(new_text);
        });

        // Set up keyboard shortcut for Cmd/Ctrl+Enter.
        use_run_shortcut(on_run, content_signal);

        // Set up cursor position tracking.
        let (cursor_line, cursor_col) = use_cursor_position();

        // Set up dry run status — either from external prop or internal debounce.
        let ds_slug = datasource_slug.unwrap_or_else(|| Signal::stored(None));

        let dry_run_status: Signal<DryRunStatus> = if let Some(external_result) = dry_run_result {
            // External dry run result provided — convert to DryRunStatus.
            Signal::derive(move || match external_result.get() {
                Some(result) => DryRunStatus::Complete(result),
                None => DryRunStatus::Idle,
            })
        } else {
            // Use internal debounced dry run.
            let status = use_debounced_dry_run(content_signal, ds_slug);
            status.into()
        };

        view! {
            <div class="flex flex-col h-full">
                // Editor area — takes remaining space
                <div class="flex-1 min-h-0 overflow-hidden">
                    <CodeEditor
                        language=Signal::stored(Language::Sql)
                        content=content_signal
                        on_change=on_change
                    />
                </div>

                // Status bar (dry run + cursor position)
                <StatusBar
                    dry_run_status=dry_run_status
                    cursor_line=Signal::derive(move || cursor_line.get())
                    cursor_col=Signal::derive(move || cursor_col.get())
                />
            </div>
        }
        .into_any()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // SSR placeholder — the real editor loads after hydration.
        let _ = (content, on_run, dry_run_result, datasource_slug);

        view! {
            <div class="flex flex-col h-full">
                <div class="flex-1 bg-muted p-4 text-muted-foreground">
                    "Loading SQL editor..."
                </div>
                <StatusBar
                    dry_run_status=Signal::stored(DryRunStatus::Idle)
                    cursor_line=Signal::stored(1usize)
                    cursor_col=Signal::stored(1usize)
                />
            </div>
        }
        .into_any()
    }
}
