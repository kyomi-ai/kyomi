// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Code Editor — wraps kode-leptos `CodeEditor` with SQL-specific features.
//!
//! React reference: `apps/frontend/src/components/MonacoSQLEditor.jsx` (~406 lines)
//!
//! Features:
//! - kode-leptos `CodeEditor` with `Language::Sql`
//! - Keyboard shortcut: Cmd/Ctrl+Enter to run query
//! - Cursor position display (line:column) via `EditorHandle`
//! - Error markers (squiggly underlines) from dry run results
//! - Fallback error display in the status bar when position info unavailable
//! - Partial dry run: validates selected text when a selection exists
//! - Debounced dry run validation (1 second after typing stops)
//! - WASM-only rendering with SSR placeholder

#[cfg(target_arch = "wasm32")]
use std::sync::Arc;

use leptos::prelude::*;

use crate::server_fns::sql_editor::DryRunResult;
#[cfg(target_arch = "wasm32")]
use crate::server_fns::sql_editor::dry_run_sql;

use super::status_bar::{DryRunStatus, StatusBar};

// ─── Kyomi editor themes ────────────────────────────────────────────────────

/// Kyomi editor theme — uses CSS custom properties from the active design system
/// so the editor automatically adapts when colors change. Only `selection`,
/// `current_line`, and `syntax` differ between dark/light.

/// Apply Kyomi design system overrides to a kode theme.
/// Sets CSS custom property references for colors, typography, toolbar, and headings.
#[cfg(target_arch = "wasm32")]
fn apply_kyomi_overrides(theme: &mut kode_leptos::Theme) {
    // Core colors — CSS vars adapt to light/dark automatically
    theme.bg = "var(--color-card)";
    theme.fg = "var(--color-foreground)";
    theme.fg_bright = "var(--color-foreground)";
    theme.fg_dim = "var(--color-muted-foreground)";
    theme.cursor = "var(--color-foreground)";
    theme.gutter_fg = "var(--color-muted-foreground)";
    theme.gutter_border = "var(--color-border)";
    theme.border = "var(--color-border)";
    theme.accent = "var(--color-primary)";
    theme.marker_error = "var(--color-error-foreground)";
    theme.marker_warning = "var(--color-warning-foreground)";
    theme.marker_info = "var(--color-info-foreground)";
    theme.marker_hint = "var(--color-muted-foreground)";
    theme.code_fg = "var(--color-foreground)";
    theme.link = "var(--color-primary)";
    // Typography
    theme.content_font_family = Some("'DM Sans', system-ui, sans-serif");
    theme.heading_font_family = Some("'Instrument Serif', Georgia, serif");
    theme.code_font_family = Some("'Geist Mono', ui-monospace, monospace");
    // Toolbar — layout and styling
    theme.toolbar_bg = Some("var(--color-muted)");
    theme.toolbar_border_color = Some("var(--color-border)");
    theme.toolbar_padding = Some("12px");
    theme.toolbar_gap = Some("4px");
    theme.toolbar_wrap = Some("nowrap");
    theme.toolbar_scrollbar_color = Some("var(--color-border) transparent");
    // Toolbar buttons
    theme.toolbar_button_padding = Some("4px 8px");
    theme.toolbar_button_font_size = Some("12px");
    theme.toolbar_button_gap = Some("4px");
    theme.toolbar_button_border = Some("none");
    theme.toolbar_button_hover_bg = Some("var(--color-accent)");
    theme.toolbar_button_selected_bg = Some("var(--color-primary)");
    theme.toolbar_button_selected_color = Some("var(--color-primary-foreground)");
    // Toolbar separator
    theme.toolbar_separator_height = Some("16px");
    theme.toolbar_separator_margin = Some("0 4px");
    // Content area
    theme.content_padding = Some("1.5rem");
    theme.content_max_width = Some("100%");
    theme.content_line_height = Some("1.8");
    theme.container_padding = Some("0");
    // Headings — Instrument Serif is weight 400, no border-bottom
    theme.heading_font_weight = Some("400");
    theme.h1_border_width = Some("0");
    theme.h2_border_width = Some("0");
}

#[cfg(target_arch = "wasm32")]
fn kyomi_dark_theme() -> kode_leptos::Theme {
    let mut theme = kode_leptos::Theme::dark();
    apply_kyomi_overrides(&mut theme);
    theme.selection = "rgba(59, 89, 152, 0.6)";
    theme.current_line = "color-mix(in oklab, var(--color-border) 50%, transparent)";
    theme.bg_highlight = "var(--color-border)";
    theme.bg_hover = "color-mix(in oklab, var(--color-border) 80%, white)";
    theme.syntax = kode_leptos::SyntaxTheme::OneDark;
    theme
}

#[cfg(target_arch = "wasm32")]
fn kyomi_light_theme() -> kode_leptos::Theme {
    let mut theme = kode_leptos::Theme::light();
    apply_kyomi_overrides(&mut theme);
    theme.selection = "rgba(201, 214, 232, 0.6)";
    theme.current_line = "color-mix(in oklab, var(--color-muted) 50%, transparent)";
    theme.bg_highlight = "var(--color-muted)";
    theme.bg_hover = "var(--color-secondary)";
    theme.syntax = kode_leptos::SyntaxTheme::GithubLight;
    theme
}

/// Returns a reactive Signal that switches between kyomi dark/light themes
/// based on the current effective theme from ThemeState context.
#[cfg(target_arch = "wasm32")]
pub fn use_editor_theme() -> Signal<kode_leptos::Theme> {
    let theme_state = crate::components::theme::use_theme();
    Signal::derive(move || {
        let is_dark = theme_state
            .map(|s| s.effective.get() == "dark")
            .unwrap_or(true);
        if is_dark {
            kyomi_dark_theme()
        } else {
            kyomi_light_theme()
        }
    })
}

// ─── Debounced dry run hook ──────────────────────────────────────────────────

/// Creates debounced dry run validation that watches query text changes.
///
/// React reference: `apps/frontend/src/hooks/useSQLDryRun.js`
///
/// - Watches `query_text` for changes
/// - After 1 second of no changes, calls `dry_run_sql()` server function
/// - If the editor has a text selection, validates only the selected text
///   (matching React's `getSelectedOrFullText()` pattern)
/// - Updates the `dry_run_status` signal with results
/// - On error with position info, sets error markers on the editor
/// - On valid result or text change, clears markers
/// - Cancels pending requests when user types again
///
/// The debounce timer is managed via `gloo_timers::callback::Timeout` which
/// auto-cancels when dropped (the previous timeout is replaced by a new one).
#[cfg(target_arch = "wasm32")]
fn use_debounced_dry_run(
    query_text: Signal<String>,
    datasource_slug: Signal<Option<String>>,
    editor_handle: RwSignal<Option<kode_leptos::EditorHandle>>,
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

        // Clear markers on every text change (the editor also auto-clears
        // internally, but we clear here to handle the explicit dry run flow).
        if let Some(handle) = editor_handle.get_untracked() {
            handle.clear_markers();
        }

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

        // Schedule dry run after 1 second of inactivity (matches React's debounce).
        let pending_clone = pending.clone();
        let timeout = Timeout::new(1_000, move || {
            // Clear the pending handle since this callback is now executing.
            pending_clone.set(None);

            // If the editor has selected text, validate only the selection
            // (matches React's getSelectedOrFullText() pattern).
            let validate_sql = editor_handle
                .get_untracked()
                .and_then(|h| h.selected_text())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or(sql_trimmed);

            // Mark as validating.
            dry_run_status.set(DryRunStatus::Validating);

            // Spawn the server function call.
            leptos::task::spawn_local(async move {
                match dry_run_sql(slug, validate_sql).await {
                    Ok(result) => {
                        // Set error markers when position info is available.
                        apply_dry_run_markers(editor_handle, &result);
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

/// Apply error markers to the editor based on dry run results.
///
/// When the dry run fails and provides line/column position info, we set a
/// `Marker` with `MarkerSeverity::Error` at that position so kode-leptos
/// renders squiggly underlines. If position info is not available, the error
/// is shown only in the status bar (fallback).
#[cfg(target_arch = "wasm32")]
fn apply_dry_run_markers(
    editor_handle: RwSignal<Option<kode_leptos::EditorHandle>>,
    result: &DryRunResult,
) {
    let Some(handle) = editor_handle.get_untracked() else {
        return;
    };

    if result.valid {
        handle.clear_markers();
        return;
    }

    // Only set markers when we have position info. DryRunResult uses
    // 1-indexed line/column; kode-core Position is 0-indexed.
    let Some(line_1) = result.line else {
        return;
    };

    let line = line_1.saturating_sub(1) as usize;
    let col = result.column.unwrap_or(1).saturating_sub(1) as usize;

    use kode_leptos::{Marker, MarkerSeverity, Position};

    // Mark from the error column to end of line. We use a large end column
    // value; the editor will clamp it to the actual line length.
    let marker = Marker {
        start: Position::new(line, col),
        end: Position::new(line, usize::MAX),
        message: result.message.clone(),
        severity: MarkerSeverity::Error,
    };

    handle.set_markers(vec![marker]);
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

/// Tracks cursor position from the `EditorHandle` by polling at 200ms intervals.
///
/// Uses `handle.cursor()` for accurate position data (0-indexed internally,
/// returned as 1-indexed to match the React status bar display).
#[cfg(target_arch = "wasm32")]
fn use_cursor_position(
    editor_handle: RwSignal<Option<kode_leptos::EditorHandle>>,
) -> (RwSignal<usize>, RwSignal<usize>) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let line = RwSignal::new(1usize);
    let col = RwSignal::new(1usize);

    Effect::new(move |_| {
        let Some(window) = web_sys::window() else { return };

        let closure = Closure::wrap(Box::new(move || {
            let Some(handle) = editor_handle.get_untracked() else {
                return;
            };

            let pos = handle.cursor();
            // Convert from 0-indexed to 1-indexed for display.
            let new_line = pos.line + 1;
            let new_col = pos.col + 1;

            if line.get_untracked() != new_line {
                line.set(new_line);
            }
            if col.get_untracked() != new_col {
                col.set(new_col);
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
    /// Whether a query is currently running (for the Run Query button).
    #[prop(optional, into)]
    query_running: Signal<bool>,
    /// Whether the Run Query button should be disabled (e.g. no datasource).
    #[prop(optional, into)]
    run_disabled: Signal<bool>,
    /// Called when the user clicks "Run Query" in the status bar.
    #[prop(optional)]
    on_run_query: Option<Callback<()>>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use kode_leptos::{CodeEditor, EditorHandle, Language};

        let content_signal: Signal<String> = content.into();

        // Retrieve the editor handle signal from context (provided by SqlEditorPage).
        let editor_handle: RwSignal<Option<EditorHandle>> =
            use_context::<RwSignal<Option<EditorHandle>>>()
                .unwrap_or_else(|| RwSignal::new(None));

        // Set up the on_change callback to write back to the RwSignal.
        let on_change: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |new_text: String| {
            content.set(new_text);
        });

        // on_ready callback — store the handle in the context signal so both
        // this component and the parent (SqlEditorPage) can use it.
        let on_ready: Arc<dyn Fn(EditorHandle) + Send + Sync> =
            Arc::new(move |handle: EditorHandle| {
                editor_handle.set(Some(handle));
            });

        // Set up keyboard shortcut for Cmd/Ctrl+Enter.
        use_run_shortcut(on_run, content_signal);

        // Set up cursor position tracking via EditorHandle.
        let (cursor_line, cursor_col) = use_cursor_position(editor_handle);

        // Reactive theme that follows Kyomi's light/dark mode.
        let editor_theme = use_editor_theme();

        // Set up dry run status — either from external prop or internal debounce.
        let ds_slug = datasource_slug.unwrap_or_else(|| Signal::stored(None));

        let dry_run_status: Signal<DryRunStatus> = if let Some(external_result) = dry_run_result {
            // External dry run result provided — convert to DryRunStatus.
            Signal::derive(move || match external_result.get() {
                Some(result) => DryRunStatus::Complete(result),
                None => DryRunStatus::Idle,
            })
        } else {
            // Use internal debounced dry run (with selection + marker support).
            let status = use_debounced_dry_run(content_signal, ds_slug, editor_handle);
            status.into()
        };

        view! {
            <div class="flex flex-col h-full">
                // Editor area — takes remaining space
                <div class="flex-1 min-h-0 overflow-hidden">
                    <CodeEditor
                        language=Signal::stored(Language::Sql)
                        content=content_signal
                        theme=editor_theme
                        on_change=on_change
                        on_ready=on_ready
                    />
                </div>

                // Status bar (dry run + cursor position + Run Query button)
                <StatusBar
                    dry_run_status=dry_run_status
                    cursor_line=Signal::derive(move || cursor_line.get())
                    cursor_col=Signal::derive(move || cursor_col.get())
                    query_running=query_running
                    run_disabled=run_disabled
                    on_run_query=on_run_query.unwrap_or_else(|| Callback::new(|_| {}))
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
                    query_running=query_running
                    run_disabled=run_disabled
                    on_run_query=on_run_query.unwrap_or_else(|| Callback::new(|_| {}))
                />
            </div>
        }
        .into_any()
    }
}
