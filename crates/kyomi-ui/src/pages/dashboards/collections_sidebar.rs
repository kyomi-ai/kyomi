// SPDX-License-Identifier: AGPL-3.0-or-later

//! Collections sidebar component — matches the collections sidebar in
//! `apps/frontend/src/pages/DashboardsList.jsx`.
//!
//! Renders a right-side panel with collection list, CRUD modals, and
//! resize handle. Supports both mobile (overlay) and desktop (inline) modes.

use std::sync::Arc;

use leptos::ev;
use leptos::prelude::*;

use phosphor_leptos::Icon;
use crate::components::{
    Button, ButtonSize, ButtonVariant, ConfirmDialog, Label, Modal, ModalSize, RightPanel,
    Switch, INPUT_CLASS,
};
use crate::query_cache::{use_query, QueryCache};
use crate::server_fns::collections::{
    create_collection, delete_collection, list_collections, update_collection, CollectionItem,
};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Preset collection colors — warm editorial palette matching DESIGN.md.
const PRESET_COLORS: &[(&str, &str)] = &[
    ("Amber", "#D97706"),
    ("Sienna", "#A0522D"),
    ("Sage", "#6B8F71"),
    ("Slate", "#5C6C8A"),
    ("Dusty Rose", "#B07080"),
    ("Clay", "#B8704A"),
    ("Lichen", "#4F8080"),
    ("Dusk", "#7E6B8A"),
];

/// Default sidebar width in pixels.
const DEFAULT_WIDTH: f64 = 320.0;

/// Minimum sidebar width in pixels.
const MIN_WIDTH: f64 = 280.0;

/// Maximum sidebar width in pixels.
const MAX_WIDTH: f64 = 480.0;

// ─── Collection item row ────────────────────────────────────────────────────

/// A single collection row in the sidebar list.
///
/// React classes copied verbatim from `DashboardsList.jsx`.
#[component]
fn CollectionRow(
    collection: CollectionItem,
    #[prop(into)] active_collection_id: Signal<Option<String>>,
    on_click: Callback<String>,
    on_edit: Callback<CollectionItem>,
    on_delete: Callback<CollectionItem>,
) -> impl IntoView {
    let coll_id = collection.collection_id.clone();
    let coll_id_for_outer = coll_id.clone();
    let coll_id_for_btn = coll_id.clone();
    let coll_id_for_click = coll_id.clone();
    let color = collection.color.clone().unwrap_or_else(|| "#D97706".to_string());
    let color_style = format!("background-color: {color}");
    let name = collection.name.clone();
    let count = collection.dashboards.len();
    let coll_for_edit = collection.clone();
    let coll_for_delete = collection;

    // React: `group relative ${activeCollectionId === collection.id ? 'bg-primary/10' : 'hover:bg-secondary'}`
    let outer_class = move || {
        let active = active_collection_id.get().as_deref() == Some(coll_id_for_outer.as_str());
        if active {
            "group relative bg-primary/10"
        } else {
            "group relative hover:bg-secondary transition-colors"
        }
    };

    // React: `w-full flex items-center gap-3 px-4 py-3 text-left transition-colors ${active ? 'text-foreground font-medium' : 'text-foreground'}`
    let btn_class = move || {
        let active = active_collection_id.get().as_deref() == Some(coll_id_for_btn.as_str());
        if active {
            "w-full flex items-center gap-3 px-4 py-3 text-left transition-colors text-foreground font-medium"
        } else {
            "w-full flex items-center gap-3 px-4 py-3 text-left transition-colors text-foreground"
        }
    };

    view! {
        <div class=outer_class>
            <button
                class=btn_class
                on:click=move |_| on_click.run(coll_id_for_click.clone())
            >
                // React: `w-3 h-3 rounded-full flex-shrink-0` with inline backgroundColor
                <div
                    class="w-3 h-3 rounded-full flex-shrink-0"
                    style=color_style.clone()
                />
                // React: `flex-1 truncate`
                <span class="flex-1 truncate">{name.clone()}</span>
                <span class="text-sm text-muted-foreground group-hover:invisible">{count}</span>
            </button>

            // Quick Actions (visible on hover)
            <div class="absolute right-2 top-1/2 -translate-y-1/2 opacity-0 group-hover:opacity-100 transition-opacity flex gap-1">
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Icon
                    aria_label="Edit collection"
                    class="h-7 w-7"
                    on:click=move |ev: web_sys::MouseEvent| {
                        ev.stop_propagation();
                        on_edit.run(coll_for_edit.clone());
                    }
                >
                    <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="14px" />
                </Button>
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Icon
                    aria_label="Delete collection"
                    class="h-7 w-7"
                    on:click=move |ev: web_sys::MouseEvent| {
                        ev.stop_propagation();
                        on_delete.run(coll_for_delete.clone());
                    }
                >
                    <Icon icon=phosphor_leptos::TRASH size="14px" />
                </Button>
            </div>
        </div>
    }
}

// ─── Collection list (shared between mobile + desktop) ──────────────────────

/// The scrollable list area containing "All Dashboards" + grouped collections.
///
/// Extracted to avoid duplicating the list between mobile and desktop layouts.
#[component]
fn CollectionList(
    collections: Vec<CollectionItem>,
    #[prop(into)] active_collection_id: Signal<Option<String>>,
    #[prop(into)] dashboard_count: Signal<usize>,
    on_collection_click: Callback<String>,
    on_all_click: Callback<()>,
    on_edit: Callback<CollectionItem>,
    on_delete: Callback<CollectionItem>,
    #[prop(default = "Dashboards".to_string())]
    type_name_plural: String,
) -> impl IntoView {
    let public_collections: Vec<_> = collections.iter().filter(|c| c.is_public).cloned().collect();
    let private_collections: Vec<_> = collections.iter().filter(|c| !c.is_public).cloned().collect();
    let has_public = !public_collections.is_empty();
    let has_private = !private_collections.is_empty();
    let has_any = !collections.is_empty();

    let all_btn_class = move || {
        if active_collection_id.get().is_none() {
            "w-full flex items-center gap-3 px-4 py-3 text-left transition-colors bg-primary/10 text-primary font-medium"
        } else {
            "w-full flex items-center gap-3 px-4 py-3 text-left transition-colors text-foreground hover:bg-secondary"
        }
    };

    view! {
        <div class="flex-1 overflow-y-auto">
            // "All Dashboards" button
            <button
                class=all_btn_class
                on:click=move |_| on_all_click.run(())
            >
                <Icon icon=phosphor_leptos::SQUARES_FOUR size="18px" />
                <span class="flex-1">{format!("All {type_name_plural}")}</span>
                <span class="text-sm text-muted-foreground">{move || dashboard_count.get()}</span>
            </button>

            // Collections grouped by Public / Private
            {if has_any {
                Some(view! {
                    // React: `py-2`
                    <div class="py-2">
                        // Public Collections Section
                        {if has_public {
                            Some(view! {
                                // React: `px-4 py-2 text-xs font-semibold text-muted-foreground uppercase tracking-wider flex items-center gap-2`
                                <div class="px-4 py-2 text-xs font-semibold text-muted-foreground uppercase tracking-wider flex items-center gap-2">
                                    <Icon icon=phosphor_leptos::GLOBE size="12px" />
                                    "Public Collections"
                                </div>
                                {public_collections.into_iter().map(|c| {
                                    view! {
                                        <CollectionRow
                                            collection=c
                                            active_collection_id=active_collection_id
                                            on_click=on_collection_click
                                            on_edit=on_edit
                                            on_delete=on_delete
                                        />
                                    }
                                }).collect_view()}
                            })
                        } else {
                            None
                        }}

                        // Private Collections Section
                        {if has_private {
                            Some(view! {
                                // React: `px-4 py-2 text-xs font-semibold text-muted-foreground uppercase tracking-wider flex items-center gap-2 mt-2`
                                <div class="px-4 py-2 text-xs font-semibold text-muted-foreground uppercase tracking-wider flex items-center gap-2 mt-2">
                                    <Icon icon=phosphor_leptos::LOCK size="12px" />
                                    "Private Collections"
                                </div>
                                {private_collections.into_iter().map(|c| {
                                    view! {
                                        <CollectionRow
                                            collection=c
                                            active_collection_id=active_collection_id
                                            on_click=on_collection_click
                                            on_edit=on_edit
                                            on_delete=on_delete
                                        />
                                    }
                                }).collect_view()}
                            })
                        } else {
                            None
                        }}
                    </div>
                })
            } else {
                None
            }}
        </div>
    }
}

// ─── Color picker ───────────────────────────────────────────────────────────

/// Preset color picker with custom hex input.
#[component]
fn ColorPicker(
    #[prop(into)] value: Signal<String>,
    on_change: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="space-y-2">
            // Preset color swatches
            <div class="flex flex-wrap gap-2">
                {PRESET_COLORS.iter().map(|&(label, hex)| {
                    let hex_str = hex.to_string();
                    let style = format!("background-color: {hex}");
                    view! {
                        <button
                            type="button"
                            title=label
                            class=move || {
                                let selected = value.get() == hex_str;
                                if selected {
                                    "w-8 h-8 rounded-full ring-2 ring-primary ring-offset-2 ring-offset-background transition-all"
                                } else {
                                    "w-8 h-8 rounded-full hover:scale-110 transition-all"
                                }
                            }
                            style=style
                            on:click={
                                let hex_str = hex.to_string();
                                move |_| on_change.run(hex_str.clone())
                            }
                        />
                    }
                }).collect_view()}
            </div>

            // Custom hex input
            // React: color picker + text input side by side
            <div class="flex items-center gap-3">
                <input
                    type="color"
                    prop:value=move || value.get()
                    class="h-10 w-20 rounded-lg border border-input cursor-pointer"
                    on:input=move |ev| {
                        on_change.run(event_target_value(&ev));
                    }
                />
                <input
                    type="text"
                    prop:value=move || value.get()
                    class="flex-1 px-3 py-1 rounded-md border border-input bg-transparent text-foreground shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring font-mono text-sm"
                    placeholder="#D97706"
                    on:input=move |ev| {
                        on_change.run(event_target_value(&ev));
                    }
                />
            </div>
        </div>
    }
}

// ─── Collection modal ───────────────────────────────────────────────────────

/// Create/Edit collection modal.
///
/// Re-uses the existing `Modal` component.
#[component]
fn CollectionModal(
    #[prop(into)] show: Signal<bool>,
    on_close: Callback<()>,
    /// None for create, Some for edit. Accepts a signal so the component does
    /// not need to be re-mounted when switching between create and edit modes —
    /// avoiding disposal of scoped reactive signals and preventing WASM panics.
    #[prop(into)]
    editing: Signal<Option<CollectionItem>>,
    on_saved: Callback<()>,
    /// doc_type for new collections ("dashboard" or "knowledge").
    #[prop(default = "dashboard".to_string())]
    doc_type: String,
    /// Lowercase plural ("dashboards" or "documents") for placeholder text.
    #[prop(default = "dashboards".to_string())]
    type_name_lower: String,
) -> impl IntoView {
    let doc_type_stored = StoredValue::new(Some(doc_type));

    let (name, set_name) = signal(String::new());
    let (description, set_description) = signal(String::new());
    let (color, set_color) = signal("#D97706".to_string());
    let (is_public, set_is_public) = signal(false);

    // Reactively reset form fields whenever the `editing` signal changes.
    // This replaces the old one-shot initialisation from `editing.as_ref()`
    // so the form is always in sync with the current editing target.
    Effect::new(move |_| {
        let ed = editing.get();
        if let Some(c) = ed {
            set_name.set(c.name.clone());
            set_description.set(c.description.clone().unwrap_or_default());
            set_color.set(c.color.clone().unwrap_or_else(|| "#D97706".to_string()));
            set_is_public.set(c.is_public);
        } else {
            set_name.set(String::new());
            set_description.set(String::new());
            set_color.set("#D97706".to_string());
            set_is_public.set(false);
        }
    });

    let is_edit = Signal::derive(move || editing.get().is_some());
    let editing_id = Signal::derive(move || editing.get().map(|c| c.collection_id.clone()));
    let title = Signal::derive(move || {
        if is_edit.get() {
            "Edit Collection".to_string()
        } else {
            "Create Collection".to_string()
        }
    });
    let submit_text = Signal::derive(move || {
        if is_edit.get() {
            "Update Collection"
        } else {
            "Create Collection"
        }
    });

    let color_signal: Signal<String> = color.into();
    let is_public_signal: Signal<bool> = is_public.into();

    // (editing_id, name, description, color, is_public, doc_type)
    // editing_id = None means create, Some(id) means update.
    type CollectionSaveInput = (Option<String>, String, String, String, bool, Option<String>);

    let save_action = Action::new(|input: &CollectionSaveInput| {
        let (editing_id, name_val, desc_val, color_val, is_pub_val, dt) = input.clone();
        async move {
            let desc = if desc_val.is_empty() { None } else { Some(desc_val) };
            let result: Result<(), ServerFnError> = if let Some(id) = editing_id {
                update_collection(
                    id,
                    Some(name_val),
                    desc,
                    Some(color_val),
                    Some(is_pub_val),
                )
                .await
                .map(|_| ())
            } else {
                create_collection(
                    name_val,
                    desc,
                    Some(color_val),
                    Some(is_pub_val),
                    dt,
                )
                .await
                .map(|_| ())
            };
            result
        }
    });

    Effect::new(move |_| {
        if let Some(result) = save_action.value().get() {
            match result {
                Ok(()) => {
                    on_saved.run(());
                    on_close.run(());
                }
                Err(e) => {
                    leptos::logging::error!("Collection save failed: {e}");
                }
            }
        }
    });

    let handle_submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();

        let Some(name_val) = name.try_get_untracked() else { return };
        let Some(desc_val) = description.try_get_untracked() else { return };
        let Some(color_val) = color.try_get_untracked() else { return };
        let Some(is_pub_val) = is_public.try_get_untracked() else { return };
        let editing_id_val = editing_id.try_get_untracked().flatten();
        let dt = doc_type_stored.get_value();

        save_action.dispatch((editing_id_val, name_val, desc_val, color_val, is_pub_val, dt));
    };

    let on_color_change = Callback::new(move |hex: String| {
        set_color.set(hex);
    });

    let on_public_change = Callback::new(move |val: bool| {
        set_is_public.set(val);
    });

    // Footer with Cancel + Submit buttons
    let footer: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
        view! {
            <Button
                variant=ButtonVariant::Secondary
                class="flex-1"
                on:click=move |_| on_close.run(())
            >
                "Cancel"
            </Button>
            <Button
                class="flex-1"
                disabled=Signal::derive(move || save_action.pending().get())
                on:click=move |_| {
                    // Trigger the form submit by dispatching a submit event
                    #[cfg(target_arch = "wasm32")]
                    if let Some(form) = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|d| d.get_element_by_id("collection-form"))
                    {
                        use wasm_bindgen::JsCast;
                        if let Ok(form) = form.dyn_into::<web_sys::HtmlFormElement>() {
                            let _ = form.request_submit();
                        }
                    }
                }
            >
                {move || if save_action.pending().get() { "Saving..." } else { submit_text.get() }}
            </Button>
        }.into_any()
    });

    view! {
        <Modal
            show=show
            on_close=on_close
            title=Signal::derive(move || title.get())
            size=ModalSize::Md
            footer=footer
        >
            <form id="collection-form" on:submit=handle_submit class="space-y-4">
                // Name
                <div>
                    <Label html_for="coll-name">"Name *"</Label>
                    <input
                        id="coll-name"
                        type="text"
                        class=INPUT_CLASS
                        placeholder=format!("Marketing {type_name_lower}")
                        required=true
                        prop:value=move || name.get()
                        on:input=move |ev| set_name.set(event_target_value(&ev))
                    />
                </div>

                // Description
                <div>
                    <Label html_for="coll-desc">"Description"</Label>
                    <textarea
                        id="coll-desc"
                        class="flex w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring resize-none"
                        placeholder=format!("{type_name_lower} for marketing team analytics")
                        rows="3"
                        prop:value=move || description.get()
                        on:input=move |ev| set_description.set(event_target_value(&ev))
                    />
                </div>

                // Color
                <div>
                    <Label>"Color"</Label>
                    <div class="mt-2">
                        <ColorPicker value=color_signal on_change=on_color_change />
                    </div>
                </div>

                // Public toggle
                <div class="flex items-center gap-3">
                    <Switch
                        checked=is_public_signal
                        on_change=on_public_change
                    />
                    <div class="flex-1">
                        <span class="block text-sm font-medium text-foreground">
                            "Make collection public"
                        </span>
                        <span class="block text-xs text-muted-foreground">
                            "Public collections are visible to all workspace members"
                        </span>
                    </div>
                </div>
            </form>
        </Modal>
    }
}

// ─── Main component ─────────────────────────────────────────────────────────

/// Collections sidebar — right-side panel with collection list and CRUD.
///
/// Matches the collections sidebar embedded in `DashboardsList.jsx`.
///
/// - Mobile: fixed overlay with backdrop
/// - Desktop: inline resizable sidebar with drag handle
#[component]
pub fn CollectionsSidebar(
    /// Whether the sidebar is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Write signal to toggle open/closed.
    set_open: WriteSignal<bool>,
    /// Currently active collection ID (None = all dashboards).
    #[prop(into)]
    active_collection_id: Signal<Option<String>>,
    /// Write signal for active collection.
    set_active_collection_id: WriteSignal<Option<String>>,
    /// Callback after any collection change (to refetch dashboard list).
    on_collections_changed: Callback<()>,
    /// Optional document type filter. When set, only collections containing
    /// documents of this type are shown (e.g. `"dashboard"` or `"knowledge"`).
    /// Default (None) shows all collections.
    #[prop(optional, into)]
    doc_type: Option<String>,
) -> impl IntoView {
    // Type-aware labels — one place to change if renaming
    let (type_name_plural, type_name_lower) = match doc_type.as_deref() {
        Some("knowledge") => ("Documents", "documents"),
        _ => ("Dashboards", "dashboards"),
    };
    let doc_type_for_modal = StoredValue::new(doc_type.clone());

    // Width owned by caller so RightPanel can animate/resize it.
    let sidebar_width = RwSignal::new(DEFAULT_WIDTH);

    // Collection data — filtered by doc_type. Backed by the Layout-level
    // QueryCache so the same `(name, doc_type)` key is shared with the host
    // page (DashboardsList / KnowledgePage), eliminating duplicate fetches.
    let query_cache = expect_context::<QueryCache>();
    let doc_type_filter = doc_type.clone();
    let collections_resource = use_query(
        "collections",
        {
            let dt = doc_type_filter.clone();
            move || dt.clone()
        },
        |dt: Option<String>| list_collections(dt),
    );

    // Mutations (create / update / delete) call into this — the
    // Layout-level cache fans out to every cached `collections` entry,
    // regardless of which doc_type variant they hold.
    let refetch_collections = move || {
        query_cache.invalidate("collections");
    };

    // Modal state
    let (show_modal, set_show_modal) = signal(false);
    let (editing_collection, set_editing_collection) = signal::<Option<CollectionItem>>(None);

    // Confirm dialog state
    let (confirm_open, set_confirm_open) = signal(false);
    let (confirm_title, set_confirm_title) = signal(String::new());
    let (confirm_message, set_confirm_message) = signal(String::new());
    let (deleting_id, set_deleting_id) = signal::<Option<String>>(None);

    // Dashboard count placeholder — parent would need to pass this
    // For now, we can't know the total count from inside the sidebar.
    // Use 0 as the count is displayed by the parent anyway.
    let dashboard_count = Signal::derive(move || 0usize);

    // ── Handlers ────────────────────────────────────────────────────────

    let handle_close = Callback::new(move |()| {
        set_open.set(false);
    });

    let handle_new = Callback::new(move |()| {
        set_editing_collection.set(None);
        set_show_modal.set(true);
    });

    let handle_collection_click = Callback::new(move |id: String| {
        let Some(current) = active_collection_id.try_get_untracked() else { return };
        if current.as_deref() == Some(&id) {
            // Clicking active collection clears filter
            set_active_collection_id.set(None);
        } else {
            set_active_collection_id.set(Some(id));
        }
    });

    let handle_all_click = Callback::new(move |()| {
        set_active_collection_id.set(None);
    });

    let handle_edit = Callback::new(move |coll: CollectionItem| {
        set_editing_collection.set(Some(coll));
        set_show_modal.set(true);
    });

    let handle_delete = Callback::new(move |coll: CollectionItem| {
        set_confirm_title.set("Delete Collection?".to_string());
        set_confirm_message.set(format!(
            "Are you sure you want to delete \"{}\"? {type_name_plural} will not be deleted.",
            coll.name
        ));
        set_deleting_id.set(Some(coll.collection_id));
        set_confirm_open.set(true);
    });

    let on_confirm_delete = Callback::new(move |()| {
        set_confirm_open.set(false);
        if let Some(id) = deleting_id.try_get_untracked().flatten() {
            let active = active_collection_id.try_get_untracked().unwrap_or(None);
            let on_changed = on_collections_changed;
            leptos::task::spawn_local(async move {
                if let Err(e) = delete_collection(id.clone()).await {
                    leptos::logging::error!("Delete collection failed: {e}");
                    return;
                }
                // If deleted collection was active, clear filter
                if active.as_deref() == Some(&id) {
                    set_active_collection_id.try_set(None);
                }
                on_changed.run(());
            });
            // Optimistic refetch
            refetch_collections();
            set_deleting_id.set(None);
        }
    });

    let on_cancel_delete = Callback::new(move |()| {
        set_confirm_open.set(false);
        set_deleting_id.set(None);
    });

    let on_modal_close = Callback::new(move |()| {
        set_show_modal.set(false);
        set_editing_collection.set(None);
    });

    let on_modal_saved = Callback::new(move |()| {
        refetch_collections();
        on_collections_changed.run(());
    });

    // ── Render ───────────────────────────────────────────────────────────

    // Footer slot: "New Collection" primary action.
    let footer_fn: ChildrenFn = Arc::new(move || {
        view! {
            <div class="p-4">
                <Button
                    class="w-full"
                    on:click=move |_| handle_new.run(())
                >
                    <Icon icon=phosphor_leptos::PLUS size="14px" />
                    "New Collection"
                </Button>
            </div>
        }
        .into_any()
    });

    view! {
        <RightPanel
            open=open
            on_close=handle_close
            width=sidebar_width
            min_width=MIN_WIDTH
            max_width=MAX_WIDTH
            title="Collections".to_string()
            close_label="Close collections".to_string()
            footer=footer_fn
        >
            {move || {
                let collections = collections_resource.get()
                    .and_then(|r| r.ok())
                    .unwrap_or_default();
                view! {
                    <CollectionList
                        collections=collections
                        active_collection_id=active_collection_id
                        dashboard_count=dashboard_count
                        on_collection_click=handle_collection_click
                        on_all_click=handle_all_click
                        on_edit=handle_edit
                        on_delete=handle_delete
                        type_name_plural=type_name_plural.to_string()
                    />
                }
            }}
        </RightPanel>

        // Modals — rendered as static siblings so their internal reactive
        // signals are never disposed between edit/create cycles. The
        // `CollectionModal` now accepts `editing` as a `Signal` and resets
        // its form fields via an `Effect` when the signal changes, and
        // `ConfirmDialog` already accepts `MaybeProp<String>` for title and
        // message so signals are read reactively inside it.
        <CollectionModal
            show=Signal::from(show_modal)
            on_close=on_modal_close
            editing=Signal::derive(move || editing_collection.get())
            on_saved=on_modal_saved
            doc_type=doc_type_for_modal.get_value().unwrap_or_else(|| "dashboard".to_string())
            type_name_lower=type_name_lower.to_string()
        />
        <ConfirmDialog
            open=Signal::from(confirm_open)
            title=confirm_title
            message=confirm_message
            confirm_text="Delete Collection"
            on_confirm=on_confirm_delete
            on_cancel=on_cancel_delete
        />
    }
}
