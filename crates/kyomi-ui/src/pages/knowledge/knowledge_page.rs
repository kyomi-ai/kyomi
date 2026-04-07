// SPDX-License-Identifier: AGPL-3.0-or-later

//! Knowledge page — main layout + CRUD orchestration + mobile responsiveness.
//!
//! Matches `apps/frontend/src/pages/Knowledge.jsx` (272 lines).
//!
//! Layout:
//! ```text
//! ┌────────────────────────────────────────────────┐
//! │ Header: "Knowledge" + New File + New Folder    │
//! ├──────────────┬─────────────────────────────────┤
//! │ File Tree    │ File Editor                     │
//! │ (w-72)       │ (flex-1)                        │
//! │              │                                  │
//! └──────────────┴─────────────────────────────────┘
//! ```
//!
//! On mobile (< md / 768px), the sidebar is hidden by default and toggled via
//! a menu button in the header. Selecting a file auto-closes the sidebar.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::confirm_dialog::ConfirmDialog;
use crate::components::knowledge::{
    build_path, CreateKnowledgeItemModal, KnowledgeFileEditor, KnowledgeFileTree,
};
use crate::components::toast::{toast_error, toast_success};
use crate::components::{Button, ButtonSize, ButtonVariant, Spinner};
use crate::server_fns::knowledge::{
    create_knowledge_file, delete_knowledge_file, list_knowledge_tree, update_knowledge_file,
};
use crate::types::KnowledgeTreeEntry;

// ─── Modal state ────────────────────────────────────────────────────────────

/// Modal variant for create/rename operations.
#[derive(Clone, Debug)]
enum ModalState {
    Hidden,
    CreateFile { parent_id: Option<String> },
    CreateFolder { parent_id: Option<String> },
    Rename { entry: KnowledgeTreeEntry },
}

impl ModalState {
    fn is_visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }

    fn title(&self) -> &str {
        match self {
            Self::Hidden => "",
            Self::CreateFile { .. } => "New File",
            Self::CreateFolder { .. } => "New Folder",
            Self::Rename { .. } => "Rename",
        }
    }

    fn default_value(&self) -> String {
        match self {
            Self::Rename { entry } => entry.name.clone(),
            _ => String::new(),
        }
    }

    fn submit_label(&self) -> &str {
        match self {
            Self::Rename { .. } => "Rename",
            _ => "Create",
        }
    }
}

// ─── Main page component ───────────────────────────────────────────────────

/// Knowledge page — workspace knowledge base with file tree + editor.
///
/// React reference: `apps/frontend/src/pages/Knowledge.jsx`
#[component]
pub fn KnowledgePage() -> impl IntoView {
    // ── Core state ──────────────────────────────────────────────────────
    let (tree_entries, set_tree_entries) = signal(Vec::<KnowledgeTreeEntry>::new());
    let (selected_file, set_selected_file) = signal(Option::<KnowledgeTreeEntry>::None);
    let (selected_file_path, set_selected_file_path) = signal(String::new());
    let (is_loading, set_is_loading) = signal(true);

    // ── Modal state ─────────────────────────────────────────────────────
    let (modal_state, set_modal_state) = signal(ModalState::Hidden);

    // ── Delete confirm dialog state ─────────────────────────────────────
    let (delete_open, set_delete_open) = signal(false);
    let (delete_target, set_delete_target) = signal(Option::<KnowledgeTreeEntry>::None);

    // ── Mobile sidebar toggle ───────────────────────────────────────────
    let (sidebar_open, set_sidebar_open) = signal(false);

    // ── Tree refresh helper ─────────────────────────────────────────────
    let refresh_tree = move || {
        leptos::task::spawn_local(async move {
            match list_knowledge_tree().await {
                Ok(entries) => set_tree_entries.set(entries),
                Err(e) => toast_error(format!("Failed to load knowledge files: {e}")),
            }
        });
    };

    // ── Initial data load ───────────────────────────────────────────────
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match list_knowledge_tree().await {
                Ok(entries) => set_tree_entries.set(entries),
                Err(e) => toast_error(format!("Failed to load knowledge files: {e}")),
            }
            set_is_loading.set(false);
        });
    });

    // ── CRUD callbacks ──────────────────────────────────────────────────

    // 1. Create file — opens modal
    let on_create_file = Callback::new(move |parent_id: Option<String>| {
        set_modal_state.set(ModalState::CreateFile { parent_id });
    });

    // 2. Create folder — opens modal
    let on_create_folder = Callback::new(move |parent_id: Option<String>| {
        set_modal_state.set(ModalState::CreateFolder { parent_id });
    });

    // 3. Rename — opens modal with current name
    let on_rename = Callback::new(move |entry: KnowledgeTreeEntry| {
        set_modal_state.set(ModalState::Rename { entry });
    });

    // 4. Delete — opens confirm dialog
    let on_delete = Callback::new(move |file_id: String| {
        // Find the entry in the current tree to get its name for the dialog.
        let entries = tree_entries.get_untracked();
        if let Some(entry) = entries.iter().find(|e| e.id == file_id) {
            set_delete_target.set(Some(entry.clone()));
            set_delete_open.set(true);
        } else {
            toast_error("Could not find the selected item. Refresh the page and try again.");
        }
    });

    // Delete confirm handler — called when user confirms the dialog.
    let on_delete_confirm = Callback::new(move |()| {
        set_delete_open.set(false);
        if let Some(entry) = delete_target.get_untracked() {
            let entry_id = entry.id.clone();
            let entry_name = entry.name.clone();
            leptos::task::spawn_local(async move {
                match delete_knowledge_file(entry_id.clone()).await {
                    Ok(()) => {
                        // Clear selection if the deleted file was selected.
                        if selected_file.get_untracked().as_ref().map(|f| &f.id)
                            == Some(&entry_id)
                        {
                            set_selected_file.set(None);
                            set_selected_file_path.set(String::new());
                        }
                        refresh_tree();
                        toast_success(format!("Deleted {entry_name}"));
                    }
                    Err(e) => toast_error(format!("Failed to delete: {e}")),
                }
            });
        }
    });

    let on_delete_cancel = Callback::new(move |()| {
        set_delete_open.set(false);
    });

    // 5. Move — directly calls server function
    let on_move = Callback::new(move |(file_id, new_parent_id, sort_order): (String, Option<String>, i32)| {
        leptos::task::spawn_local(async move {
            match update_knowledge_file(
                file_id,
                None,             // content
                None,             // content_hash
                None,             // name
                new_parent_id,    // parent_id
                Some(sort_order), // sort_order
            )
            .await
            {
                Ok(_) => refresh_tree(),
                Err(e) => toast_error(format!("Failed to move: {e}")),
            }
        });
    });

    // 6. File selection
    let on_select = Callback::new(move |entry: KnowledgeTreeEntry| {
        if entry.is_folder {
            return;
        }
        set_selected_file.set(Some(entry.clone()));
        let path = build_path(&tree_entries.get_untracked(), &entry.id);
        set_selected_file_path.set(path);
        // Auto-close sidebar on mobile after file selection.
        set_sidebar_open.set(false);
    });

    // 7. on_saved callback from editor — refresh tree (names may change).
    let on_saved = Callback::new(move |()| {
        refresh_tree();
    });

    // ── Modal submit handler ────────────────────────────────────────────
    let on_modal_submit = Callback::new(move |name: String| {
        let state = modal_state.get_untracked();
        match state {
            ModalState::CreateFile { parent_id } => {
                leptos::task::spawn_local(async move {
                    match create_knowledge_file(name.clone(), parent_id, None, false).await {
                        Ok(_) => {
                            refresh_tree();
                            toast_success(format!("Created {name}"));
                        }
                        Err(e) => toast_error(format!("Failed to create file: {e}")),
                    }
                });
            }
            ModalState::CreateFolder { parent_id } => {
                leptos::task::spawn_local(async move {
                    match create_knowledge_file(name.clone(), parent_id, None, true).await {
                        Ok(_) => {
                            refresh_tree();
                            toast_success(format!("Created {name}"));
                        }
                        Err(e) => toast_error(format!("Failed to create folder: {e}")),
                    }
                });
            }
            ModalState::Rename { entry } => {
                let file_id = entry.id.clone();
                leptos::task::spawn_local(async move {
                    match update_knowledge_file(
                        file_id.clone(),
                        None,              // content
                        None,              // content_hash
                        Some(name.clone()), // name
                        None,              // parent_id
                        None,              // sort_order
                    )
                    .await
                    {
                        Ok(_) => {
                            refresh_tree();
                            // Update selected file if it was the renamed one.
                            if selected_file
                                .get_untracked()
                                .as_ref()
                                .map(|f| &f.id)
                                == Some(&file_id)
                            {
                                set_selected_file.update(|f| {
                                    if let Some(file) = f {
                                        file.name = name.clone();
                                    }
                                });
                            }
                            toast_success(format!("Renamed to {name}"));
                        }
                        Err(e) => toast_error(format!("Failed to rename: {e}")),
                    }
                });
            }
            ModalState::Hidden => {}
        }
        set_modal_state.set(ModalState::Hidden);
    });

    let on_modal_close = Callback::new(move |()| {
        set_modal_state.set(ModalState::Hidden);
    });

    // ── Derived signals for modal props ─────────────────────────────────
    let modal_show = Signal::derive(move || modal_state.get().is_visible());
    // ── Derived signals for child components ────────────────────────────
    let selected_id = Signal::derive(move || selected_file.get().map(|f| f.id));

    // Delete dialog title/message are derived fresh each time the dialog
    // is shown, because ConfirmDialog takes owned Strings (not Signals).
    // We wrap the dialog in a Show to re-mount it with correct props.

    // ── Render ──────────────────────────────────────────────────────────
    view! {
        <div class="flex flex-col h-full bg-muted" style="flex-direction: column;">
            // Header
            <div class="h-16 border-b border-border bg-card px-6 flex-shrink-0 flex items-center justify-between">
                <div class="flex items-center gap-3">
                    // Mobile sidebar toggle button
                    <button
                        class="md:hidden inline-flex items-center justify-center rounded-md text-sm font-medium h-8 w-8 border border-input bg-background text-foreground hover:bg-accent hover:text-accent-foreground transition-colors"
                        on:click=move |_| set_sidebar_open.update(|v| *v = !*v)
                    >
                        <Show
                            when=move || sidebar_open.get()
                            fallback=|| view! { <Icon icon=icondata_lu::LuMenu attr:class="h-4 w-4"/> }
                        >
                            <Icon icon=icondata_lu::LuX attr:class="h-4 w-4"/>
                        </Show>
                    </button>
                    <h1 class="text-2xl font-display text-foreground">"Knowledge"</h1>
                </div>
                <div class="flex items-center gap-2">
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        on:click=move |_| on_create_file.run(None)
                    >
                        <Icon icon=icondata_lu::LuPlus attr:class="h-4 w-4 mr-1"/>
                        <span class="hidden md:inline">"New File"</span>
                    </Button>
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        on:click=move |_| on_create_folder.run(None)
                    >
                        <Icon icon=icondata_lu::LuFolderPlus attr:class="h-4 w-4 mr-1"/>
                        <span class="hidden md:inline">"New Folder"</span>
                    </Button>
                </div>
            </div>

            // Content: sidebar + editor
            <div class="flex-1 flex overflow-hidden">
                // Sidebar — hidden on mobile by default, shown as overlay when toggled
                // Desktop: always visible
                <div class={move || {
                    if sidebar_open.get() {
                        // Mobile open: fixed overlay
                        "fixed inset-0 z-40 md:static md:z-auto w-72 flex-shrink-0 border-r border-border bg-card overflow-y-auto"
                    } else {
                        // Mobile closed: hidden; desktop: always shown
                        "hidden md:block w-72 flex-shrink-0 border-r border-border bg-card overflow-y-auto"
                    }
                }}>
                    // On mobile overlay, add a top offset to clear the header
                    <div class={move || {
                        if sidebar_open.get() {
                            "pt-16 md:pt-0 h-full bg-card"
                        } else {
                            "h-full"
                        }
                    }}>
                        <Show when=move || !is_loading.get() fallback=move || view! {
                            <div class="flex items-center justify-center h-32">
                                <Spinner class="text-muted-foreground"/>
                            </div>
                        }>
                            <KnowledgeFileTree
                                entries=tree_entries
                                selected_id=selected_id
                                on_select=on_select
                                on_create_file=on_create_file
                                on_create_folder=on_create_folder
                                on_rename=on_rename
                                on_delete=on_delete
                                on_move=on_move
                            />
                        </Show>
                    </div>
                </div>

                // Mobile backdrop — shown when sidebar is open on mobile
                <Show when=move || sidebar_open.get()>
                    <div
                        class="fixed inset-0 z-30 bg-[var(--color-overlay)] md:hidden"
                        on:click=move |_| set_sidebar_open.set(false)
                    />
                </Show>

                // Editor pane
                <div class="flex-1 min-h-0 min-w-0 overflow-hidden">
                    <Show when=move || !is_loading.get() fallback=move || view! {
                        <div class="flex items-center justify-center h-full">
                            <Spinner class="text-muted-foreground"/>
                        </div>
                    }>
                        <KnowledgeFileEditor
                            selected_file=selected_file
                            file_path=selected_file_path
                            on_saved=on_saved
                        />
                    </Show>
                </div>
            </div>

            // Create/Rename modal
            //
            // The CreateKnowledgeItemModal takes String props (not Signal), so we
            // conditionally render it based on modal_show to ensure it re-mounts
            // with fresh props whenever the modal state changes.
            <Show when=move || modal_show.get()>
                {move || {
                    let state = modal_state.get();
                    view! {
                        <CreateKnowledgeItemModal
                            show=modal_show
                            on_close=on_modal_close
                            on_submit=on_modal_submit
                            title=state.title().to_string()
                            default_value=state.default_value()
                            submit_label=state.submit_label().to_string()
                        />
                    }
                }}
            </Show>

            // Delete confirm dialog — outer Show forces re-mount so String
            // props (title/message) are freshly evaluated from delete_target
            // each time a new delete is initiated. ConfirmDialog has its own
            // inner Show for visibility, but String props are static per mount.
            <Show when=move || delete_open.get()>
                {move || {
                    let target = delete_target.get();
                    let (title, message) = match target.as_ref() {
                        Some(e) if e.is_folder => (
                            format!("Delete \"{}\"?", e.name),
                            format!("Delete \"{}\" and all its contents? This cannot be undone.", e.name),
                        ),
                        Some(e) => (
                            format!("Delete \"{}\"?", e.name),
                            format!("Delete \"{}\"? This cannot be undone.", e.name),
                        ),
                        None => (String::new(), String::new()),
                    };
                    view! {
                        <ConfirmDialog
                            open=delete_open
                            title=title
                            message=message
                            confirm_text="Delete"
                            on_confirm=on_delete_confirm
                            on_cancel=on_delete_cancel
                        />
                    }
                }}
            </Show>
        </div>
    }
}
