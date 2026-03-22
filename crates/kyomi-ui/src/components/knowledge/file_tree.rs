// SPDX-License-Identifier: AGPL-3.0-or-later

//! KnowledgeFileTree — collapsible file tree sidebar with search, context menu,
//! and drag-and-drop.
//!
//! Ported from `apps/frontend/src/components/KnowledgeFileTree.jsx` (541 lines).
//!
//! Features:
//! - Collapsible tree with folder/file icons
//! - Server-side content search with 300ms debounce, client-side fallback
//! - Right-click context menu: rename, move to (submenu), delete
//! - Native HTML5 drag-and-drop for reordering/moving entries

use std::collections::HashSet;

use leptos::prelude::*;

use crate::components::knowledge::tree_types::{
    build_tree, flatten_tree, get_descendant_ids, get_folder_targets,
};
use crate::server_fns::knowledge::search_knowledge_files;
use crate::types::{KnowledgeSearchResult, KnowledgeTreeEntry};

// ─── SVG Icons ──────────────────────────────────────────────────────────────

/// ChevronRight icon (Lucide).
#[component]
fn ChevronRightIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m9 18 6-6-6-6"/>
        </svg>
    }
}

/// ChevronDown icon (Lucide).
#[component]
fn ChevronDownIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m6 9 6 6 6-6"/>
        </svg>
    }
}

/// FolderOpen icon (Lucide).
#[component]
fn FolderOpenIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>
        </svg>
    }
}

/// Folder icon (Lucide).
#[component]
fn FolderIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>
        </svg>
    }
}

/// FileText icon (Lucide).
#[component]
fn FileTextIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/>
            <path d="M14 2v4a2 2 0 0 0 2 2h4"/>
            <path d="M10 9H8"/>
            <path d="M16 13H8"/>
            <path d="M16 17H8"/>
        </svg>
    }
}

/// Search icon (Lucide).
#[component]
fn SearchIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="8"/>
            <path d="m21 21-4.3-4.3"/>
        </svg>
    }
}

/// Plus icon (Lucide).
#[component]
fn PlusIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 12h14"/>
            <path d="M12 5v14"/>
        </svg>
    }
}

/// GripVertical icon (Lucide).
#[component]
fn GripVerticalIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="9" cy="12" r="1"/>
            <circle cx="9" cy="5" r="1"/>
            <circle cx="9" cy="19" r="1"/>
            <circle cx="15" cy="12" r="1"/>
            <circle cx="15" cy="5" r="1"/>
            <circle cx="15" cy="19" r="1"/>
        </svg>
    }
}

/// Pencil icon (Lucide).
#[component]
fn PencilIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/>
            <path d="m15 5 4 4"/>
        </svg>
    }
}

/// FolderInput icon (Lucide).
#[component]
fn FolderInputIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M2 9V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H20a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-1"/>
            <path d="M2 13h10"/>
            <path d="m9 16 3-3-3-3"/>
        </svg>
    }
}

/// Trash2 icon (Lucide).
#[component]
fn Trash2Icon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M3 6h18"/>
            <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/>
            <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>
            <line x1="10" x2="10" y1="11" y2="17"/>
            <line x1="14" x2="14" y1="11" y2="17"/>
        </svg>
    }
}

/// XMark icon (Lucide) for search clear.
#[component]
fn XMarkIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M18 6 6 18"/>
            <path d="m6 6 12 12"/>
        </svg>
    }
}

// ─── Context Menu ───────────────────────────────────────────────────────────

/// State for the context menu: mouse coordinates + target entry.
#[derive(Clone)]
struct ContextMenuState {
    x: i32,
    y: i32,
    entry: KnowledgeTreeEntry,
}

/// Right-click context menu for tree entries.
///
/// Matches `ContextMenu` in `KnowledgeFileTree.jsx` (lines 69-167).
/// Fixed-position at mouse coords. Closes on click-outside or Escape.
#[component]
fn TreeContextMenu(
    state: ContextMenuState,
    /// All entries (for building move targets).
    entries: Signal<Vec<KnowledgeTreeEntry>>,
    on_rename: Callback<KnowledgeTreeEntry>,
    on_delete: Callback<String>,
    on_move: Callback<(String, Option<String>, i32)>,
    on_close: Callback<()>,
) -> impl IntoView {
    let menu_ref = NodeRef::<leptos::html::Div>::new();
    let (show_move_submenu, set_show_move_submenu) = signal(false);

    let entry = state.entry.clone();
    let entry_id = entry.id.clone();
    let entry_for_rename = entry.clone();
    let entry_id_for_delete = entry_id.clone();
    let entry_id_for_move = entry_id.clone();
    let entry_parent_id = entry.parent_id.clone();

    // Build move targets: "/ (root)" + all valid folders excluding self, descendants,
    // and current parent (matching React lines 90-106).
    let move_targets = Memo::new(move |_| {
        let all = entries.get();
        let folder_targets = get_folder_targets(&all, &entry_id_for_move);

        let mut targets: Vec<(Option<String>, String)> = Vec::new();

        // Always show root option (matching React lines 101-102).
        // If entry is already at root this is a no-op the server handles gracefully.
        targets.push((None, "/ (root)".to_string()));

        for (id, path) in folder_targets {
            // Exclude current parent
            if Some(id.as_str()) == entry_parent_id.as_deref() {
                continue;
            }
            targets.push((Some(id), path));
        }

        targets
    });

    // Click-outside and Escape key detection
    #[cfg(feature = "hydrate")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;

        let cleanup: StoredValue<Option<SendWrapper<Box<dyn FnOnce()>>>> =
            StoredValue::new(None);

        let on_close_click = on_close.clone();
        let on_close_escape = on_close.clone();

        Effect::new(move |_| {
            // Clean up any previous listeners
            if let Some(teardown) = cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }

            let window = web_sys::window().expect("window");
            let menu_el = menu_ref.get();

            let on_close_click = on_close_click.clone();
            let click_cb =
                Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
                    if let Some(target) = ev.target() {
                        let target_node: web_sys::Node = target.unchecked_into();
                        if let Some(ref el) = menu_el {
                            let html_el: &web_sys::HtmlElement = el;
                            let node: &web_sys::Node = html_el.as_ref();
                            if !node.contains(Some(&target_node)) {
                                on_close_click.run(());
                            }
                        } else {
                            on_close_click.run(());
                        }
                    }
                });

            let on_close_escape = on_close_escape.clone();
            let keydown_cb =
                Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(
                    move |ev: web_sys::KeyboardEvent| {
                        if ev.key() == "Escape" {
                            on_close_escape.run(());
                        }
                    },
                );

            let _ = window.add_event_listener_with_callback(
                "mousedown",
                click_cb.as_ref().unchecked_ref(),
            );
            let _ = window.add_event_listener_with_callback(
                "keydown",
                keydown_cb.as_ref().unchecked_ref(),
            );

            let window_clone = window.clone();
            let click_ref: js_sys::Function =
                click_cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let keydown_ref: js_sys::Function =
                keydown_cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            click_cb.forget();
            keydown_cb.forget();

            let teardown: Box<dyn FnOnce()> = Box::new(move || {
                let _ = window_clone
                    .remove_event_listener_with_callback("mousedown", &click_ref);
                let _ = window_clone
                    .remove_event_listener_with_callback("keydown", &keydown_ref);
            });
            cleanup.set_value(Some(SendWrapper::new(teardown)));
        });

        on_cleanup(move || {
            if let Some(teardown) = cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }
        });
    }

    let style = format!("left: {}px; top: {}px;", state.x, state.y);

    let on_close_rename = on_close.clone();
    let on_close_delete = on_close.clone();

    view! {
        <div
            node_ref=menu_ref
            class="fixed z-[1200] min-w-[160px] rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-md"
            style=style
        >
            // Rename
            <button
                class="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
                on:click=move |_| {
                    on_rename.run(entry_for_rename.clone());
                    on_close_rename.run(());
                }
            >
                <PencilIcon class="w-3.5 h-3.5".to_string() />
                "Rename"
            </button>

            // Move to (with submenu)
            <div
                class="relative"
                on:mouseenter=move |_| set_show_move_submenu.set(true)
                on:mouseleave=move |_| set_show_move_submenu.set(false)
            >
                <button
                    class="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground justify-between"
                >
                    <span class="flex items-center gap-2">
                        <FolderInputIcon class="w-3.5 h-3.5".to_string() />
                        "Move to"
                    </span>
                    <ChevronRightIcon class="w-3 h-3".to_string() />
                </button>
                <Show when=move || show_move_submenu.get()>
                    <div class="absolute left-full top-0 ml-1 min-w-[140px] rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-md">
                        {move || {
                            let targets = move_targets.get();
                            if targets.is_empty() {
                                view! {
                                    <div class="px-2 py-1.5 text-xs text-muted-foreground">
                                        "No folders available"
                                    </div>
                                }.into_any()
                            } else {
                                let entry_id = entry_id_for_move.clone();
                                let on_move = on_move.clone();
                                let on_close = on_close.clone();
                                targets.into_iter().map(move |(folder_id, name)| {
                                    let entry_id = entry_id.clone();
                                    let folder_id = folder_id.clone();
                                    let on_move = on_move.clone();
                                    let on_close = on_close.clone();
                                    view! {
                                        <button
                                            class="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
                                            on:click=move |_| {
                                                on_move.run((entry_id.clone(), folder_id.clone(), 0));
                                                on_close.run(());
                                            }
                                        >
                                            <FolderIcon class="w-3.5 h-3.5 text-warning-foreground".to_string() />
                                            {name.clone()}
                                        </button>
                                    }
                                }).collect_view().into_any()
                            }
                        }}
                    </div>
                </Show>
            </div>

            // Separator
            <div class="my-1 h-px bg-muted" />

            // Delete
            <button
                class="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-sm text-destructive hover:bg-destructive/10"
                on:click=move |_| {
                    on_delete.run(entry_id.clone());
                    on_close_delete.run(());
                }
            >
                <Trash2Icon class="w-3.5 h-3.5".to_string() />
                "Delete"
            </button>
        </div>
    }
}

// ─── Main Component ─────────────────────────────────────────────────────────

/// Knowledge file tree sidebar with search, context menu, and drag-and-drop.
///
/// React reference: `apps/frontend/src/components/KnowledgeFileTree.jsx`
#[component]
pub fn KnowledgeFileTree(
    #[prop(into)] entries: Signal<Vec<KnowledgeTreeEntry>>,
    #[prop(into)] selected_id: Signal<Option<String>>,
    on_select: Callback<KnowledgeTreeEntry>,
    on_create_file: Callback<Option<String>>,
    on_create_folder: Callback<Option<String>>,
    on_rename: Callback<KnowledgeTreeEntry>,
    on_delete: Callback<String>,
    on_move: Callback<(String, Option<String>, i32)>,
) -> impl IntoView {
    // ── State signals ────────────────────────────────────────────────────
    let expanded_folders: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let context_menu: RwSignal<Option<ContextMenuState>> = RwSignal::new(None);
    let (search_filter, set_search_filter) = signal(String::new());
    let (search_results, set_search_results) =
        signal::<Option<Vec<KnowledgeSearchResult>>>(None);
    let dragged_id: RwSignal<Option<String>> = RwSignal::new(None);
    let drag_over_id: RwSignal<Option<String>> = RwSignal::new(None);

    // ── Tree building ────────────────────────────────────────────────────
    let tree = Memo::new(move |_| build_tree(&entries.get()));

    // Flatten tree respecting expanded folders
    let flat_entries = Memo::new(move |_| {
        flatten_tree(&tree.get(), &expanded_folders.get())
    });

    // ── Toggle expand ────────────────────────────────────────────────────
    let toggle_expand = Callback::new(move |id: String| {
        expanded_folders.update(|set| {
            if !set.remove(&id) {
                set.insert(id);
            }
        });
    });

    // ── Search with 300ms debounce ───────────────────────────────────────
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;

        let debounce_handle: StoredValue<Option<i32>> = StoredValue::new(None);

        Effect::new(move |_| {
            let query = search_filter.get();

            // Clear any pending debounce timer
            if let Some(handle) = debounce_handle.get_value() {
                let window = web_sys::window().expect("window");
                window.clear_timeout_with_handle(handle);
                debounce_handle.set_value(None);
            }

            if query.trim().is_empty() {
                set_search_results.set(None);
                return;
            }

            // Don't clear results eagerly — keep showing previous results
            // while the debounce timer runs. React does not set searchResults
            // to null on each keystroke, only when query is empty.

            let window = web_sys::window().expect("window");
            let cb = Closure::once(move || {
                let entries_snapshot = entries.get_untracked();
                leptos::task::spawn_local(async move {
                    match search_knowledge_files(query.clone()).await {
                        Ok(results) => {
                            set_search_results.set(Some(results));
                        }
                        Err(_) => {
                            // Fall back to client-side name filter
                            let lower = query.to_lowercase();
                            let filtered: Vec<KnowledgeSearchResult> = entries_snapshot
                                .iter()
                                .filter(|e| e.name.to_lowercase().contains(&lower))
                                .map(|e| KnowledgeSearchResult {
                                    id: e.id.clone(),
                                    parent_id: e.parent_id.clone(),
                                    name: e.name.clone(),
                                    is_folder: e.is_folder,
                                    content_preview: None,
                                })
                                .collect();
                            set_search_results.set(Some(filtered));
                        }
                    }
                });
            });

            let handle = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    cb.as_ref().unchecked_ref(),
                    300,
                )
                .expect("set_timeout");
            cb.forget();
            debounce_handle.set_value(Some(handle));
        });
    }

    // ── Build path for search results ────────────────────────────────────
    let get_entry_path = move |entry: &KnowledgeSearchResult| -> Option<String> {
        let all = entries.get_untracked();
        // Walk up parent chain
        let mut parts = Vec::new();
        let mut current_id = entry.parent_id.clone();
        let entry_map: std::collections::HashMap<&str, &KnowledgeTreeEntry> =
            all.iter().map(|e| (e.id.as_str(), e)).collect();

        while let Some(ref pid) = current_id {
            if let Some(parent) = entry_map.get(pid.as_str()) {
                parts.push(parent.name.clone());
                current_id = parent.parent_id.clone();
            } else {
                break;
            }
        }

        if parts.is_empty() {
            None
        } else {
            parts.reverse();
            Some(parts.join(" / "))
        }
    };

    // ── Drag event helpers ───────────────────────────────────────────────
    //
    // These use `web_sys::Event` for the event parameter since `DragEvent`
    // inherits from `Event` and we only need `prevent_default()`. This
    // avoids requiring the `DragEvent` feature in the crate's web-sys.

    // Drop on a tree entry (folder target). Takes the target entry as argument.
    // Uses Callback so it can be invoked from multiple For-loop iterations.
    let do_drop_on_entry = Callback::new(move |target_entry: KnowledgeTreeEntry| {
        drag_over_id.set(None);

        let dragged = dragged_id.get_untracked();
        let Some(ref source_id) = dragged else {
            return;
        };

        // Don't drop on self
        if source_id == &target_entry.id {
            return;
        }

        // Only drop on folders
        if !target_entry.is_folder {
            return;
        }

        // Don't drop a folder into its own descendant
        let all = entries.get_untracked();
        let descendants = get_descendant_ids(&all, source_id);
        if descendants.contains(&target_entry.id) {
            return;
        }

        // Don't move if already in that folder
        let source_entry = all.iter().find(|e| &e.id == source_id);
        if let Some(src) = source_entry {
            if src.parent_id.as_deref() == Some(&target_entry.id) {
                return;
            }
        }

        on_move.run((
            source_id.clone(),
            Some(target_entry.id.clone()),
            0,
        ));

        // Auto-expand target folder
        expanded_folders.update(|set| {
            set.insert(target_entry.id.clone());
        });

        dragged_id.set(None);
    });

    // Drop on sidebar background -> move to root
    let do_drop_root = Callback::new(move |()| {
        drag_over_id.set(None);

        let dragged = dragged_id.get_untracked();
        let Some(ref source_id) = dragged else {
            return;
        };

        // Don't move if already at root
        let all = entries.get_untracked();
        let source_entry = all.iter().find(|e| &e.id == source_id);
        if let Some(src) = source_entry {
            if src.parent_id.is_none() {
                dragged_id.set(None);
                return;
            }
        }

        on_move.run((source_id.clone(), None, 0));
        dragged_id.set(None);
    });

    // ── Render ───────────────────────────────────────────────────────────
    let is_searching = move || !search_filter.get().trim().is_empty();

    view! {
        <div class="flex flex-col h-full">
            // Header
            <div class="px-3 py-2 border-b border-border flex items-center justify-between">
                <span class="text-sm font-medium text-foreground">"Files"</span>
                <div class="flex items-center gap-1">
                    <button
                        class="inline-flex items-center justify-center rounded-md text-sm font-medium transition-colors hover:bg-accent hover:text-accent-foreground h-6 w-6"
                        title="New File"
                        on:click=move |_| on_create_file.run(None)
                    >
                        <PlusIcon class="w-3.5 h-3.5".to_string() />
                    </button>
                    <button
                        class="inline-flex items-center justify-center rounded-md text-sm font-medium transition-colors hover:bg-accent hover:text-accent-foreground h-6 w-6"
                        title="New Folder"
                        on:click=move |_| on_create_folder.run(None)
                    >
                        <FolderIcon class="w-3.5 h-3.5".to_string() />
                    </button>
                </div>
            </div>

            // Search
            <div class="px-3 py-2">
                <div class="relative">
                    <SearchIcon class="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground".to_string() />
                    <input
                        type="text"
                        placeholder="Search files..."
                        class="flex h-7 w-full rounded-md border border-input bg-transparent px-3 py-1 text-xs shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring pl-7"
                        prop:value=move || search_filter.get()
                        on:input=move |ev| {
                            set_search_filter.set(event_target_value(&ev));
                        }
                    />
                    <Show when=move || !search_filter.get().is_empty()>
                        <button
                            class="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
                            on:click=move |_| set_search_filter.set(String::new())
                        >
                            <XMarkIcon class="w-3 h-3".to_string() />
                        </button>
                    </Show>
                </div>
            </div>

            // Tree / Search results
            <div
                class="flex-1 overflow-y-auto py-1"
                on:dragover=move |ev: web_sys::DragEvent| ev.prevent_default()
                on:drop=move |ev: web_sys::DragEvent| {
                    ev.prevent_default();
                    do_drop_root.run(());
                }
            >
                <Show
                    when=is_searching
                    fallback=move || {
                        // Normal tree view (or empty state)
                        let entries_empty = move || entries.get().is_empty();
                        view! {
                            <Show
                                when=move || !entries_empty()
                                fallback=move || view! {
                                    <div class="px-3 py-8 text-center text-muted-foreground text-xs">
                                        "No knowledge files yet. Click + to create one."
                                    </div>
                                }
                            >
                                <For
                                    each=move || flat_entries.get()
                                    key=|(entry, _, _)| entry.id.clone()
                                    let:item
                                >
                                    {
                                        let (entry, depth, _is_last) = item;
                                        let entry_clone = entry.clone();
                                        let entry_for_click = entry.clone();
                                        let entry_for_ctx = entry.clone();
                                        let entry_for_drop = entry.clone();
                                        let entry_id = entry.id.clone();
                                        let entry_id_for_drag = entry.id.clone();
                                        let entry_id_for_drag_over = entry.id.clone();
                                        let is_folder = entry.is_folder;
                                        let name = entry.name.clone();

                                        let padding_left = format!("padding-left: {}px;", depth * 16 + 8);

                                        let row_class = move || {
                                            let sel = selected_id.get();
                                            let is_selected = sel.as_deref() == Some(entry_id.as_str());
                                            let is_drag_over = drag_over_id.get().as_deref() == Some(entry_clone.id.as_str()) && entry_clone.is_folder;
                                            let is_dragging = dragged_id.get().as_deref() == Some(entry_clone.id.as_str());

                                            let mut cls = String::from("flex items-center gap-1 px-2 py-1 cursor-pointer rounded text-sm group");

                                            if is_selected {
                                                cls.push_str(" bg-accent text-accent-foreground");
                                            } else {
                                                cls.push_str(" text-foreground");
                                            }

                                            if is_drag_over {
                                                cls.push_str(" bg-primary/10 ring-1 ring-primary/30");
                                            }

                                            if is_dragging {
                                                cls.push_str(" opacity-40");
                                            }

                                            cls.push_str(" hover:bg-accent/50");
                                            cls
                                        };

                                        view! {
                                            <div
                                                class=row_class
                                                style=padding_left
                                                draggable="true"
                                                on:click=move |_| {
                                                    if entry_for_click.is_folder {
                                                        toggle_expand.run(entry_for_click.id.clone());
                                                    } else {
                                                        on_select.run(entry_for_click.clone());
                                                    }
                                                }
                                                on:contextmenu=move |ev: web_sys::MouseEvent| {
                                                    ev.prevent_default();
                                                    ev.stop_propagation();
                                                    context_menu.set(Some(ContextMenuState {
                                                        x: ev.client_x(),
                                                        y: ev.client_y(),
                                                        entry: entry_for_ctx.clone(),
                                                    }));
                                                }
                                                on:dragstart=move |_| {
                                                    dragged_id.set(Some(entry_id_for_drag.clone()));
                                                }
                                                on:dragover=move |ev: web_sys::DragEvent| {
                                                    if is_folder {
                                                        ev.prevent_default();
                                                        drag_over_id.set(Some(entry_id_for_drag_over.clone()));
                                                    }
                                                }
                                                on:dragleave=move |_| {
                                                    drag_over_id.set(None);
                                                }
                                                on:dragend=move |_| {
                                                    dragged_id.set(None);
                                                    drag_over_id.set(None);
                                                }
                                                on:drop=move |ev: web_sys::DragEvent| {
                                                    ev.prevent_default();
                                                    do_drop_on_entry.run(entry_for_drop.clone());
                                                }
                                            >
                                                // Drag handle (visible on hover)
                                                <GripVerticalIcon class="w-3 h-3 flex-shrink-0 text-muted-foreground/50 opacity-0 group-hover:opacity-100 cursor-grab".to_string() />

                                                {if is_folder {
                                                    let entry_id_expand = entry.id.clone();
                                                    let is_expanded = move || expanded_folders.get().contains(&entry_id_expand);
                                                    view! {
                                                        <Show
                                                            when=is_expanded
                                                            fallback=move || view! {
                                                                <ChevronRightIcon class="w-3.5 h-3.5 flex-shrink-0 text-muted-foreground".to_string() />
                                                            }
                                                        >
                                                            <ChevronDownIcon class="w-3.5 h-3.5 flex-shrink-0 text-muted-foreground".to_string() />
                                                        </Show>
                                                        <Show
                                                            when=is_expanded
                                                            fallback=move || view! {
                                                                <FolderIcon class="w-4 h-4 flex-shrink-0 text-warning-foreground".to_string() />
                                                            }
                                                        >
                                                            <FolderOpenIcon class="w-4 h-4 flex-shrink-0 text-warning-foreground".to_string() />
                                                        </Show>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <span class="w-3.5" />
                                                        <FileTextIcon class="w-4 h-4 flex-shrink-0 text-muted-foreground".to_string() />
                                                    }.into_any()
                                                }}

                                                <span class="truncate flex-1">{name}</span>
                                            </div>
                                        }
                                    }
                                </For>
                            </Show>
                        }
                    }
                >
                    // Search results (flat list)
                    {move || {
                        let results = search_results.get();
                        match results {
                            None => {
                                // Still searching
                                view! {
                                    <div class="px-3 py-4 text-center text-muted-foreground text-xs">
                                        "Searching..."
                                    </div>
                                }.into_any()
                            }
                            Some(results) if results.is_empty() => {
                                view! {
                                    <div class="px-3 py-8 text-center text-muted-foreground text-xs">
                                        "No files match your search."
                                    </div>
                                }.into_any()
                            }
                            Some(results) => {
                                results.into_iter().map(|result| {
                                    let folder_path = get_entry_path(&result);
                                    let result_id = result.id.clone();
                                    let result_name = result.name.clone();
                                    let result_is_folder = result.is_folder;
                                    let result_for_click = result.clone();
                                    let result_for_ctx = result.clone();

                                    let row_class = move || {
                                        let sel = selected_id.get();
                                        if sel.as_deref() == Some(result_id.as_str()) {
                                            "flex items-center gap-1 px-3 py-1.5 cursor-pointer rounded text-sm hover:bg-accent/50 bg-accent text-accent-foreground"
                                        } else {
                                            "flex items-center gap-1 px-3 py-1.5 cursor-pointer rounded text-sm hover:bg-accent/50 text-foreground"
                                        }
                                    };

                                    view! {
                                        <div
                                            class=row_class
                                            on:click=move |_| {
                                                if !result_for_click.is_folder {
                                                    // Convert search result back to tree entry for on_select
                                                    let entry = KnowledgeTreeEntry {
                                                        id: result_for_click.id.clone(),
                                                        parent_id: result_for_click.parent_id.clone(),
                                                        name: result_for_click.name.clone(),
                                                        is_folder: result_for_click.is_folder,
                                                        sort_order: 0,
                                                        updated_at: String::new(),
                                                        updated_by: None,
                                                    };
                                                    on_select.run(entry);
                                                }
                                            }
                                            on:contextmenu=move |ev| {
                                                ev.prevent_default();
                                                ev.stop_propagation();
                                                let entry = KnowledgeTreeEntry {
                                                    id: result_for_ctx.id.clone(),
                                                    parent_id: result_for_ctx.parent_id.clone(),
                                                    name: result_for_ctx.name.clone(),
                                                    is_folder: result_for_ctx.is_folder,
                                                    sort_order: 0,
                                                    updated_at: String::new(),
                                                    updated_by: None,
                                                };
                                                context_menu.set(Some(ContextMenuState {
                                                    x: ev.client_x(),
                                                    y: ev.client_y(),
                                                    entry,
                                                }));
                                            }
                                        >
                                            {if result_is_folder {
                                                view! { <FolderIcon class="w-4 h-4 flex-shrink-0 text-warning-foreground".to_string() /> }.into_any()
                                            } else {
                                                view! { <FileTextIcon class="w-4 h-4 flex-shrink-0 text-muted-foreground".to_string() /> }.into_any()
                                            }}
                                            <div class="flex flex-col min-w-0 flex-1">
                                                <span class="truncate">{result_name}</span>
                                                {folder_path.map(|path| view! {
                                                    <span class="text-xs text-muted-foreground truncate">{path}</span>
                                                })}
                                            </div>
                                        </div>
                                    }
                                }).collect_view().into_any()
                            }
                        }
                    }}
                </Show>
            </div>

            // Context menu
            <Show when=move || context_menu.get().is_some()>
                {move || {
                    let state = context_menu.get().unwrap();
                    view! {
                        <TreeContextMenu
                            state=state
                            entries=entries
                            on_rename=on_rename
                            on_delete=on_delete
                            on_move=on_move
                            on_close=Callback::new(move |()| context_menu.set(None))
                        />
                    }
                }}
            </Show>
        </div>
    }
}
