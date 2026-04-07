// SPDX-License-Identifier: AGPL-3.0-or-later

//! Knowledge file editor with auto-save, conflict detection, and live preview.
//!
//! Mirrors `apps/frontend/src/components/KnowledgeFileEditor.jsx` (283 lines).
//! Uses a two-panel source editor (Kode + MarkdownRenderer) with a visual mode
//! stub, matching the pattern in `dashboard_editor.rs`.

use std::sync::Arc;

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::dashboard::MarkdownRenderer;
use crate::components::toast::toast_error;
use crate::server_fns::knowledge::get_knowledge_file;
#[cfg(target_arch = "wasm32")]
use crate::server_fns::knowledge::update_knowledge_file;
use crate::types::KnowledgeTreeEntry;

// ─────────────────────────────────────────────────────────────────────────────
// Enums
// ─────────────────────────────────────────────────────────────────────────────

/// Save state machine for the file editor. `Saving` and `Saved` are only
/// constructed on the WASM target (via the debounced auto-save), but the
/// view matches all variants on both targets for exhaustiveness.
#[derive(Clone, Copy, PartialEq)]
enum SaveStatus {
    Idle,
    Saving,
    Saved,
    Conflict,
}

#[derive(Clone, Copy, PartialEq)]
enum EditorMode {
    Source,
    Visual,
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Format an RFC 3339 timestamp into a short human-readable form.
///
/// "2026-03-22T10:30:00+00:00" → "Mar 22 10:30"
/// Falls back to the raw string if parsing fails.
fn format_timestamp(rfc3339: &str) -> String {
    // Parse "YYYY-MM-DDTHH:MM:SS..." — we only need date + time parts.
    // Uses `get()` instead of direct indexing to avoid panics on non-ASCII input.
    if rfc3339.len() >= 16 {
        let month = match rfc3339.get(5..7) {
            Some("01") => "Jan",
            Some("02") => "Feb",
            Some("03") => "Mar",
            Some("04") => "Apr",
            Some("05") => "May",
            Some("06") => "Jun",
            Some("07") => "Jul",
            Some("08") => "Aug",
            Some("09") => "Sep",
            Some("10") => "Oct",
            Some("11") => "Nov",
            Some("12") => "Dec",
            _ => return rfc3339.to_string(),
        };
        let day = match rfc3339.get(8..10) {
            Some(d) => d,
            None => return rfc3339.to_string(),
        };
        let time = match rfc3339.get(11..16) {
            Some(t) => t,
            None => return rfc3339.to_string(),
        };
        // Strip leading zero from day
        let day = day.trim_start_matches('0');
        format!("{month} {day} {time}")
    } else {
        rfc3339.to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main component
// ─────────────────────────────────────────────────────────────────────────────

/// Knowledge file editor with auto-save, conflict detection, and live preview.
///
/// Shows a toolbar with file path, save status, metadata, and mode toggle,
/// followed by a two-panel editor (source + preview) or a visual mode stub.
#[component]
pub fn KnowledgeFileEditor(
    #[prop(into)] selected_file: Signal<Option<KnowledgeTreeEntry>>,
    #[prop(into)] file_path: Signal<String>,
    on_saved: Callback<()>,
) -> impl IntoView {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = &on_saved;
    // ── State signals ─────────────────────────────────────────────────────
    let (content, set_content) = signal(String::new());
    let (content_hash, set_content_hash) = signal(Option::<String>::None);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = content_hash;
    let (save_status, set_save_status) = signal(SaveStatus::Idle);
    let (updated_at, set_updated_at) = signal(Option::<String>::None);
    let (updated_by, set_updated_by) = signal(Option::<String>::None);
    // Default to Source mode (React defaults to Visual, but Visual is currently a stub
    // so Source provides a better initial experience).
    let (mode, set_mode) = signal(EditorMode::Source);
    let (is_loading, set_is_loading) = signal(false);
    let loaded_file_id: StoredValue<Option<String>> = StoredValue::new(None);

    // ── Debounce handle for auto-save (WASM only) ────────────────────────
    #[cfg(target_arch = "wasm32")]
    let debounce_handle: StoredValue<
        Option<send_wrapper::SendWrapper<gloo_timers::callback::Timeout>>,
    > = StoredValue::new(None);

    // Helper: cancel pending debounce timer.
    // This closure is `Copy` because `StoredValue` is `Copy` when `T: 'static`,
    // which lets it be captured by multiple closures without `Clone`.
    #[cfg(target_arch = "wasm32")]
    let cancel_debounce = move || {
        debounce_handle.update_value(|h| {
            drop(h.take());
        });
    };

    // ── File loading effect ───────────────────────────────────────────────
    Effect::new(move |_| {
        let file = selected_file.get();

        // Cancel pending auto-save timer on file switch
        #[cfg(target_arch = "wasm32")]
        cancel_debounce();

        match file {
            None => {
                // No file selected — clear all state
                set_content.set(String::new());
                set_content_hash.set(None);
                set_save_status.set(SaveStatus::Idle);
                set_updated_at.set(None);
                set_updated_by.set(None);
                set_is_loading.set(false);
                loaded_file_id.set_value(None);
            }
            Some(ref entry) if entry.is_folder => {
                // Folder selected — clear all state
                set_content.set(String::new());
                set_content_hash.set(None);
                set_save_status.set(SaveStatus::Idle);
                set_updated_at.set(None);
                set_updated_by.set(None);
                set_is_loading.set(false);
                loaded_file_id.set_value(None);
            }
            Some(ref entry) => {
                let file_id = entry.id.clone();
                loaded_file_id.set_value(Some(file_id.clone()));
                set_is_loading.set(true);
                set_save_status.set(SaveStatus::Idle);

                let fid = file_id.clone();
                leptos::task::spawn_local(async move {
                    match get_knowledge_file(fid.clone()).await {
                        Ok(detail) => {
                            // Guard against stale response
                            let current_id = loaded_file_id.get_value();
                            if current_id.as_deref() != Some(&fid) {
                                return;
                            }
                            set_content.set(detail.content.unwrap_or_default());
                            set_content_hash.set(detail.content_hash);
                            set_updated_at.set(Some(detail.updated_at));
                            set_updated_by.set(detail.updated_by);
                            set_is_loading.set(false);
                        }
                        Err(e) => {
                            let current_id = loaded_file_id.get_value();
                            if current_id.as_deref() != Some(&fid) {
                                return;
                            }
                            toast_error(format!("Failed to load file: {e}"));
                            set_is_loading.set(false);
                        }
                    }
                });
            }
        }
    });

    // ── Save function (WASM only — called from debounced auto-save) ─────
    #[cfg(target_arch = "wasm32")]
    let do_save = move |content_to_save: String, hash_to_send: Option<String>| {
        let file = selected_file.get_untracked();
        let file_id = match file {
            Some(ref f) if !f.is_folder => f.id.clone(),
            _ => return,
        };

        set_save_status.set(SaveStatus::Saving);

        leptos::task::spawn_local(async move {
            match update_knowledge_file(
                file_id.clone(),
                Some(content_to_save),
                hash_to_send,
                None,
                None,
                None,
            )
            .await
            {
                Ok(detail) => {
                    // Guard against stale save
                    let current_id = loaded_file_id.get_value();
                    if current_id.as_deref() != Some(&file_id) {
                        return;
                    }
                    set_content_hash.set(detail.content_hash);
                    set_updated_at.set(Some(detail.updated_at));
                    set_updated_by.set(detail.updated_by);
                    set_save_status.set(SaveStatus::Saved);
                    on_saved.run(());
                }
                Err(e) => {
                    let current_id = loaded_file_id.get_value();
                    if current_id.as_deref() != Some(&file_id) {
                        return;
                    }
                    let msg = e.to_string();
                    // CAS conflict protocol: server fn returns "CONFLICT:" prefix
                    // when content_hash doesn't match. See update_knowledge_file()
                    // in server_fns/knowledge.rs.
                    if msg.contains("CONFLICT:") {
                        set_save_status.set(SaveStatus::Conflict);
                    } else {
                        set_save_status.set(SaveStatus::Idle);
                        toast_error(format!("Failed to save file: {msg}"));
                    }
                }
            }
        });
    };

    // ── Reload handler (after conflict) ───────────────────────────────────
    let reload_file = move |_| {
        let file = selected_file.get_untracked();
        let file_id = match file {
            Some(ref f) if !f.is_folder => f.id.clone(),
            _ => return,
        };

        leptos::task::spawn_local(async move {
            match get_knowledge_file(file_id.clone()).await {
                Ok(detail) => {
                    let current_id = loaded_file_id.get_value();
                    if current_id.as_deref() != Some(&file_id) {
                        return;
                    }
                    set_content.set(detail.content.unwrap_or_default());
                    set_content_hash.set(detail.content_hash);
                    set_updated_at.set(Some(detail.updated_at));
                    set_updated_by.set(detail.updated_by);
                    set_save_status.set(SaveStatus::Idle);
                }
                Err(e) => {
                    toast_error(format!("Failed to reload file: {e}"));
                }
            }
        });
    };

    // ── onChange handler with debounced auto-save ──────────────────────────
    let on_change: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |new_content: String| {
        // Don't accept changes during conflict.
        // React uses readOnly prop on the editor to disable input visually;
        // kode-leptos CodeEditor doesn't expose a read_only prop, so we
        // silently discard changes here instead. The conflict banner is still
        // visible and the user can click "Reload" to resolve.
        if save_status.get_untracked() == SaveStatus::Conflict {
            return;
        }

        set_content.set(new_content);
        set_save_status.set(SaveStatus::Idle);

        #[cfg(target_arch = "wasm32")]
        {
            use send_wrapper::SendWrapper;

            // Cancel any pending debounce
            cancel_debounce();

            // Schedule save after 1500ms of inactivity.
            // Read latest values at fire time to avoid stale content_hash.
            let handle = gloo_timers::callback::Timeout::new(1_500, move || {
                let current_content = content.get_untracked();
                let current_hash = content_hash.get_untracked();
                do_save(current_content, current_hash);
            });

            debounce_handle.set_value(Some(SendWrapper::new(handle)));
        }
    });

    // ── Cleanup debounce timer on unmount ─────────────────────────────────
    #[cfg(target_arch = "wasm32")]
    on_cleanup(move || {
        cancel_debounce();
    });

    // ── Render ────────────────────────────────────────────────────────────
    let on_change_for_editor = StoredValue::new(on_change.clone());
    view! {
        <Show
            when=move || selected_file.get().is_some()
            fallback=|| view! {
                <div class="flex-1 flex items-center justify-center text-muted-foreground text-sm">
                    "Select a file to edit"
                </div>
            }
        >
            <Show
                when=move || {
                    selected_file.get().map(|f| !f.is_folder).unwrap_or(false)
                }
                fallback=|| view! {
                    <div class="flex-1 flex items-center justify-center text-muted-foreground text-sm">
                        "Select a file to view its contents"
                    </div>
                }
            >
                <Show
                    when=move || !is_loading.get()
                    fallback=|| view! {
                        <div class="flex-1 flex items-center justify-center text-muted-foreground text-sm">
                            <span class="w-4 h-4 animate-spin mr-2"><Icon icon=icondata_lu::LuLoader width="16" height="16" /></span>
                            "Loading..."
                        </div>
                    }
                >
                    <div class="flex-1 flex flex-col" style="height: 100%; overflow: hidden;">
                        // ── Toolbar ──────────────────────────────────
                        <div class="flex items-center justify-between px-4 py-2 border-b border-border bg-card flex-shrink-0">
                            // Left: file path + save status
                            <div class="flex items-center gap-2">
                                <span class="text-sm font-medium text-foreground truncate max-w-[400px]">
                                    {move || {
                                        let path = file_path.get();
                                        if path.is_empty() {
                                            selected_file.get().map(|f| f.name.clone()).unwrap_or_default()
                                        } else {
                                            path
                                        }
                                    }}
                                </span>
                                // Save status indicator
                                {move || {
                                    let status = save_status.get();
                                    if status == SaveStatus::Saving {
                                        Some(view! {
                                            <span class="flex items-center gap-1 text-xs text-muted-foreground">
                                                <span class="w-3 h-3 animate-spin"><Icon icon=icondata_lu::LuLoader width="12" height="12" /></span>
                                                "Saving..."
                                            </span>
                                        }.into_any())
                                    } else if status == SaveStatus::Saved {
                                        Some(view! {
                                            <span class="flex items-center gap-1 text-xs text-success-foreground">
                                                <span class="w-3 h-3"><Icon icon=icondata_lu::LuCheck width="12" height="12" /></span>
                                                "Saved"
                                            </span>
                                        }.into_any())
                                    } else if status == SaveStatus::Conflict {
                                        Some(view! {
                                            <span class="flex items-center gap-1 text-xs text-destructive">
                                                <span class="w-3 h-3"><Icon icon=icondata_lu::LuTriangleAlert width="12" height="12" /></span>
                                                "Conflict!"
                                            </span>
                                        }.into_any())
                                    } else {
                                        None
                                    }
                                }}
                            </div>
                            // Right: metadata + mode toggle
                            <div class="flex items-center gap-2">
                                {move || {
                                    let at = updated_at.get();
                                    let by = updated_by.get();
                                    at.map(|t| {
                                        // Format RFC 3339 timestamp for human readability:
                                        // "2026-03-22T10:30:00Z" → "Mar 22 10:30"
                                        let display_time = format_timestamp(&t);
                                        let by_str = by.map(|b| format!(" by {b}")).unwrap_or_default();
                                        view! {
                                            <span class="text-xs text-muted-foreground hidden md:inline">
                                                {format!("Updated {display_time}{by_str}")}
                                            </span>
                                        }
                                    })
                                }}
                                // Mode toggle buttons
                                <div class="flex items-center border border-border rounded-md overflow-hidden">
                                    <button
                                        class=move || {
                                            let base = "inline-flex items-center justify-center h-7 px-2 text-sm font-medium transition-colors rounded-none";
                                            if mode.get() == EditorMode::Visual {
                                                format!("{base} bg-secondary text-secondary-foreground")
                                            } else {
                                                format!("{base} bg-transparent text-muted-foreground hover:bg-secondary hover:text-accent-foreground")
                                            }
                                        }
                                        on:click=move |_| set_mode.set(EditorMode::Visual)
                                    >
                                        <span class="w-3.5 h-3.5 mr-1"><Icon icon=icondata_lu::LuEye width="14" height="14" /></span>
                                        "Visual"
                                    </button>
                                    <button
                                        class=move || {
                                            let base = "inline-flex items-center justify-center h-7 px-2 text-sm font-medium transition-colors rounded-none";
                                            if mode.get() == EditorMode::Source {
                                                format!("{base} bg-secondary text-secondary-foreground")
                                            } else {
                                                format!("{base} bg-transparent text-muted-foreground hover:bg-secondary hover:text-accent-foreground")
                                            }
                                        }
                                        on:click=move |_| set_mode.set(EditorMode::Source)
                                    >
                                        <span class="w-3.5 h-3.5 mr-1"><Icon icon=icondata_lu::LuCode width="14" height="14" /></span>
                                        "Source"
                                    </button>
                                </div>
                            </div>
                        </div>

                        // ── Conflict banner ──────────────────────────
                        {move || {
                            (save_status.get() == SaveStatus::Conflict).then(|| view! {
                                <div class="flex items-center gap-2 p-3 border-b border-warning-border bg-warning flex-shrink-0">
                                    <span class="w-4 h-4 text-warning-foreground flex-shrink-0"><Icon icon=icondata_lu::LuTriangleAlert width="16" height="16" /></span>
                                    <span class="text-sm font-medium text-warning-foreground flex-1">
                                        "This file was modified by another user."
                                    </span>
                                    <button
                                        class="px-3 py-1 text-sm font-medium text-warning-foreground bg-warning-foreground/10 hover:bg-warning-foreground/20 rounded-md transition-colors"
                                        on:click=reload_file
                                    >
                                        "Reload"
                                    </button>
                                </div>
                            })
                        }}

                        // ── Editor area ──────────────────────────────
                        {move || {
                            let on_change_clone = on_change_for_editor.get_value();
                            if mode.get() == EditorMode::Source {
                                view! {
                                    <div class="flex flex-1 min-h-0 overflow-hidden">
                                        // Left panel: Kode editor
                                        <div class="flex-1 min-h-0 overflow-hidden border-r border-border">
                                            <KnowledgeCodeEditor
                                                content=Signal::derive(move || content.get())
                                                on_change=on_change_clone
                                            />
                                        </div>
                                        // Right panel: Live preview
                                        <div class="flex-1 min-h-0 overflow-y-auto flex flex-col">
                                            <div class="p-6 max-w-4xl mx-auto flex-1">
                                                <MarkdownRenderer content=Signal::derive(move || content.get()) />
                                            </div>
                                        </div>
                                    </div>
                                }.into_any()
                            } else {
                                // Visual mode: stub
                                view! {
                                    <div class="flex-1 flex items-center justify-center bg-muted">
                                        <div class="text-center max-w-md px-6">
                                            <span class="w-12 h-12 mx-auto text-muted-foreground mb-4"><Icon icon=icondata_lu::LuEye width="48" height="48" /></span>
                                            <h3 class="text-lg font-semibold text-foreground mb-2">
                                                "Visual editing coming soon"
                                            </h3>
                                            <p class="text-sm text-muted-foreground">
                                                "Use Source mode for now to edit your knowledge files with Markdown."
                                            </p>
                                            <button
                                                class="mt-4 px-4 py-2 text-sm font-medium text-primary bg-primary/10 hover:bg-primary/20 rounded-lg transition-colors"
                                                on:click=move |_| set_mode.set(EditorMode::Source)
                                            >
                                                "Switch to Source"
                                            </button>
                                        </div>
                                    </div>
                                }.into_any()
                            }
                        }}
                    </div>
                </Show>
            </Show>
        </Show>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Kode CodeEditor wrapper
// ─────────────────────────────────────────────────────────────────────────────

/// Wrapper for the Kode `CodeEditor` that handles conditional compilation.
///
/// On WASM targets, renders the Kode `CodeEditor` with Markdown highlighting.
/// On server (SSR), renders a placeholder since Kode requires browser DOM APIs.
///
/// Matches `DashboardCodeEditor` in `dashboard_editor.rs` (lines 846-884).
#[component]
fn KnowledgeCodeEditor(
    #[prop(into)]
    content: Signal<String>,
    on_change: Arc<dyn Fn(String) + Send + Sync>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use kode_leptos::{CodeEditor, Language};

        view! {
            <CodeEditor
                language=Signal::stored(Language::Markdown)
                content=content
                on_change=on_change
            />
        }
        .into_any()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = content;
        let _ = on_change;

        view! {
            <div class="flex-1 bg-muted p-4 text-muted-foreground">
                "Loading editor..."
            </div>
        }
        .into_any()
    }
}


#[cfg(test)]
mod tests {
    use super::format_timestamp;

    #[test]
    fn formats_standard_rfc3339() {
        assert_eq!(
            format_timestamp("2026-03-22T10:30:00+00:00"),
            "Mar 22 10:30"
        );
    }

    #[test]
    fn formats_utc_z_suffix() {
        assert_eq!(
            format_timestamp("2026-03-22T10:30:00Z"),
            "Mar 22 10:30"
        );
    }

    #[test]
    fn strips_leading_zero_from_day() {
        assert_eq!(format_timestamp("2026-01-05T09:00:00Z"), "Jan 5 09:00");
    }

    #[test]
    fn falls_back_on_short_input() {
        assert_eq!(format_timestamp("2026-03"), "2026-03");
    }

    #[test]
    fn falls_back_on_invalid_month() {
        assert_eq!(
            format_timestamp("2026-13-01T00:00:00Z"),
            "2026-13-01T00:00:00Z"
        );
    }
}
