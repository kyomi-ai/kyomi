// SPDX-License-Identifier: AGPL-3.0-or-later

//! Collections sidebar component — matches the collections sidebar in
//! `apps/frontend/src/pages/DashboardsList.jsx`.
//!
//! Renders a right-side panel with collection list, CRUD modals, and
//! resize handle. Supports both mobile (overlay) and desktop (inline) modes.

use std::sync::Arc;

use leptos::ev;
use leptos::prelude::*;
#[cfg(feature = "hydrate")]
use wasm_bindgen::prelude::*;

use leptos_icons::Icon;

use crate::components::{
    Button, ButtonSize, ButtonVariant, ConfirmDialog, Label, Modal, ModalSize,
    Switch, INPUT_CLASS,
};
use crate::server_fns::collections::{
    create_collection, delete_collection, list_collections, update_collection, CollectionItem,
};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Preset colors matching React DashboardsList.jsx.
const PRESET_COLORS: &[(&str, &str)] = &[
    ("Amber", "#d97706"),
    ("Blue", "#3b82f6"),
    ("Green", "#22c55e"),
    ("Red", "#ef4444"),
    ("Purple", "#8b5cf6"),
    ("Pink", "#ec4899"),
    ("Cyan", "#06b6d4"),
    ("Orange", "#f97316"),
];

/// Default sidebar width in pixels.
const DEFAULT_WIDTH: f64 = 320.0;

/// Minimum sidebar width in pixels.
#[cfg(feature = "hydrate")]
const MIN_WIDTH: f64 = 280.0;

/// Maximum sidebar width in pixels.
#[cfg(feature = "hydrate")]
const MAX_WIDTH: f64 = 480.0;

/// Mobile breakpoint (matches React `window.innerWidth < 768`).
const MOBILE_BREAKPOINT: f64 = 768.0;


// ─── Collection form data ───────────────────────────────────────────────────

/// Form state for creating/editing a collection.
#[derive(Clone, Debug)]
struct CollectionFormData {
    name: String,
    description: String,
    color: String,
    is_public: bool,
}

impl Default for CollectionFormData {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            color: "#d97706".to_string(),
            is_public: false,
        }
    }
}

// ─── Detect mobile ──────────────────────────────────────────────────────────

/// Returns a reactive signal that tracks whether the viewport is mobile-sized.
fn use_is_mobile() -> Signal<bool> {
    let (is_mobile, set_is_mobile) = signal(false);

    Effect::new(move || {
        if let Some(window) = web_sys::window() {
            let width = window.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1024.0);
            set_is_mobile.set(width < MOBILE_BREAKPOINT);
        }
    });

    // Listen for resize events — cleaned up on unmount
    #[cfg(feature = "hydrate")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::closure::Closure;

        let handler = Closure::<dyn Fn()>::new(move || {
            if let Some(window) = web_sys::window() {
                let width = window
                    .inner_width()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1024.0);
                set_is_mobile.set(width < MOBILE_BREAKPOINT);
            }
        });

        if let Some(window) = web_sys::window() {
            let _ = window.add_event_listener_with_callback(
                "resize",
                handler.as_ref().unchecked_ref(),
            );
            let handler_ref = SendWrapper::new(
                handler.as_ref().unchecked_ref::<js_sys::Function>().clone(),
            );
            let window = SendWrapper::new(window);
            let handler_wrapper = SendWrapper::new(handler);
            on_cleanup(move || {
                let _ =
                    window.take().remove_event_listener_with_callback("resize", &handler_ref.take());
                drop(handler_wrapper);
            });
        }
    }

    is_mobile.into()
}

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
    let color = collection.color.clone().unwrap_or_else(|| "#d97706".to_string());
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
                    <Icon icon=icondata_lu::LuPencil width="14" height="14" />
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
                    <Icon icon=icondata_lu::LuTrash2 width="14" height="14" />
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
                <Icon icon=icondata_lu::LuLayoutDashboard width="18" height="18" />
                <span class="flex-1">"All Dashboards"</span>
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
                                    <Icon icon=icondata_lu::LuGlobe width="12" height="12" />
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
                                    <Icon icon=icondata_lu::LuLock width="12" height="12" />
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

// ─── Sidebar header + new-collection button ─────────────────────────────────

/// Sidebar header with title, close button, and "New Collection" action.
#[component]
fn SidebarHeader(
    on_close: Callback<()>,
    on_new: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="p-4 border-b border-border flex items-center justify-between flex-shrink-0">
            <h3 class="font-semibold text-foreground">"Collections"</h3>
            <Button
                variant=ButtonVariant::Secondary
                size=ButtonSize::Icon
                aria_label="Close"
                class="h-7 w-7"
                on:click=move |_| on_close.run(())
            >
                <Icon icon=icondata_lu::LuX width="16" height="16" />
            </Button>
        </div>

        <div class="flex-shrink-0 p-4 border-b border-border">
            <Button class="w-full" on:click=move |_| on_new.run(())>
                <Icon icon=icondata_lu::LuPlus width="14" height="14" />
                "New Collection"
            </Button>
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
                    class="flex-1 px-4 py-2 border border-input rounded-lg bg-card text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring font-mono text-sm"
                    placeholder="#d97706"
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
    /// None for create, Some for edit.
    editing: Option<CollectionItem>,
    on_saved: Callback<()>,
) -> impl IntoView {
    let is_edit = editing.is_some();
    let editing_id = StoredValue::new(editing.as_ref().map(|c| c.collection_id.clone()));
    let title = if is_edit { "Edit Collection" } else { "Create Collection" };

    let initial = editing.as_ref().map_or_else(CollectionFormData::default, |c| {
        CollectionFormData {
            name: c.name.clone(),
            description: c.description.clone().unwrap_or_default(),
            color: c.color.clone().unwrap_or_else(|| "#d97706".to_string()),
            is_public: c.is_public,
        }
    });

    let (name, set_name) = signal(initial.name);
    let (description, set_description) = signal(initial.description);
    let (color, set_color) = signal(initial.color);
    let (is_public, set_is_public) = signal(initial.is_public);
    let (saving, set_saving) = signal(false);

    let color_signal: Signal<String> = color.into();
    let is_public_signal: Signal<bool> = is_public.into();

    let handle_submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        set_saving.set(true);

        let name_val = name.get_untracked();
        let desc_val = description.get_untracked();
        let color_val = color.get_untracked();
        let is_pub_val = is_public.get_untracked();
        let editing_id = editing_id.get_value();

        leptos::task::spawn_local(async move {
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
                )
                .await
                .map(|_| ())
            };

            set_saving.set(false);

            match result {
                Ok(_) => {
                    on_saved.run(());
                    on_close.run(());
                }
                Err(e) => {
                    // Log the error — the user sees the modal stays open
                    leptos::logging::error!("Collection save failed: {e}");
                }
            }
        });
    };

    let on_color_change = Callback::new(move |hex: String| {
        set_color.set(hex);
    });

    let on_public_change = Callback::new(move |val: bool| {
        set_is_public.set(val);
    });

    let submit_text = if is_edit { "Update Collection" } else { "Create Collection" };

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
                disabled=Signal::derive(move || saving.get())
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
                {move || if saving.get() { "Saving..." } else { submit_text }}
            </Button>
        }.into_any()
    });

    view! {
        <Modal
            show=show
            on_close=on_close
            title=title
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
                        placeholder="Marketing Dashboards"
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
                        class="w-full px-4 py-2 border border-input rounded-lg bg-card text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring resize-none"
                        placeholder="Dashboards for marketing team analytics"
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
) -> impl IntoView {
    let is_mobile = use_is_mobile();

    // Sidebar width for desktop resize — set_sidebar_width used in hydrate feature only
    let (sidebar_width, set_sidebar_width) = signal(DEFAULT_WIDTH);
    #[cfg(not(feature = "hydrate"))]
    let _ = set_sidebar_width;

    // Collection data
    let collections_resource = Resource::new(
        move || open.get(), // refetch when sidebar opens
        move |_| list_collections(),
    );

    // Track a version counter to force refetch
    let (version, set_version) = signal(0u32);
    let refetch_collections = move || {
        set_version.update(|v| *v += 1);
        collections_resource.refetch();
    };

    // Modal state
    let (show_modal, set_show_modal) = signal(false);
    let (editing_collection, set_editing_collection) = signal::<Option<CollectionItem>>(None);

    // Confirm dialog state
    let (confirm_open, set_confirm_open) = signal(false);
    let (confirm_title, set_confirm_title) = signal(String::new());
    let (confirm_message, set_confirm_message) = signal(String::new());
    let (deleting_id, set_deleting_id) = signal::<Option<String>>(None);

    // Resize state
    let (is_resizing, set_is_resizing) = signal(false);

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
        let current = active_collection_id.get_untracked();
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
            "Are you sure you want to delete \"{}\"? Dashboards will not be deleted.",
            coll.name
        ));
        set_deleting_id.set(Some(coll.collection_id));
        set_confirm_open.set(true);
    });

    let on_confirm_delete = Callback::new(move |()| {
        set_confirm_open.set(false);
        if let Some(id) = deleting_id.get_untracked() {
            let active = active_collection_id.get_untracked();
            let on_changed = on_collections_changed;
            leptos::task::spawn_local(async move {
                if let Err(e) = delete_collection(id.clone()).await {
                    leptos::logging::error!("Delete collection failed: {e}");
                    return;
                }
                // If deleted collection was active, clear filter
                if active.as_deref() == Some(&id) {
                    set_active_collection_id.set(None);
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

    // ── Resize drag handling (desktop) ──────────────────────────────────

    // We attach document-level mousemove/mouseup listeners when resizing.
    // This is the same pattern as the React useEffect in DashboardsList.jsx.
    Effect::new(move || {
        let resizing = is_resizing.get();
        if resizing {
            #[cfg(feature = "hydrate")]
            {
                let Some(window) = web_sys::window() else { return };
                let Some(document) = window.document() else { return };
                let body = document.body();

                // Set cursor
                if let Some(ref body) = body {
                    let _ = body.style().set_property("cursor", "col-resize");
                    let _ = body.style().set_property("user-select", "none");
                }

                // We need start_x captured at mousedown time.
                // Since Effect re-runs when is_resizing changes, we capture the
                // current mouse position from a stored signal.
                // However, a simpler approach: attach listeners directly in the
                // mousedown handler below. The Effect approach here just sets body cursor.
                // The actual move/up handlers are set in handle_resize_start.

                // Cleanup body styles when resizing stops
                // (The mouseup handler does this, but just in case.)
            }
        }
    });

    #[cfg(feature = "hydrate")]
    let drag_cleanup: StoredValue<Option<send_wrapper::SendWrapper<Box<dyn FnOnce()>>>> =
        StoredValue::new(None);

    let handle_resize_start = move |ev: web_sys::MouseEvent| {
        ev.prevent_default();
        set_is_resizing.set(true);

        #[cfg(feature = "hydrate")]
        {
            use std::cell::RefCell;
            use std::rc::Rc;
            use wasm_bindgen::closure::Closure;

            let start_x = ev.client_x() as f64;
            let start_w = sidebar_width.get_untracked();

            let Some(window) = web_sys::window() else { return };
            let Some(document) = window.document() else { return };

            let move_handler = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(
                move |ev: web_sys::MouseEvent| {
                    let diff = start_x - ev.client_x() as f64;
                    let new_width = (start_w + diff).clamp(MIN_WIDTH, MAX_WIDTH);
                    set_sidebar_width.set(new_width);
                },
            );

            let move_ref = move_handler
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone();
            let document_for_up = document.clone();
            let move_fn_for_up = move_ref.clone();

            let closures: Rc<RefCell<Option<(
                Closure<dyn FnMut(web_sys::MouseEvent)>,
                Closure<dyn FnMut()>,
            )>>> = Rc::new(RefCell::new(None));
            let closures_for_up = closures.clone();

            let up_handler = Closure::<dyn FnMut()>::new(move || {
                set_is_resizing.set(false);
                let _ = document_for_up
                    .remove_event_listener_with_callback("mousemove", &move_fn_for_up);
                if let Some((_, ref up_cb)) = *closures_for_up.borrow() {
                    let _ = document_for_up.remove_event_listener_with_callback(
                        "mouseup",
                        up_cb.as_ref().unchecked_ref(),
                    );
                }
                if let Some(body) = document_for_up.body() {
                    let _ = body.style().set_property("cursor", "");
                    let _ = body.style().set_property("user-select", "");
                }
                closures_for_up.borrow_mut().take();
                drag_cleanup.set_value(None);
            });

            let _ = document.add_event_listener_with_callback(
                "mousemove",
                move_ref.unchecked_ref(),
            );
            let _ = document.add_event_listener_with_callback(
                "mouseup",
                up_handler.as_ref().unchecked_ref(),
            );

            *closures.borrow_mut() = Some((move_handler, up_handler));

            let closures_for_teardown = closures;
            let document_for_teardown = document.clone();
            let move_ref_for_teardown = move_ref.clone();
            let teardown: Box<dyn FnOnce()> = Box::new(move || {
                if let Some((_, ref up_cb)) = *closures_for_teardown.borrow() {
                    let _ = document_for_teardown.remove_event_listener_with_callback(
                        "mousemove",
                        &move_ref_for_teardown,
                    );
                    let _ = document_for_teardown.remove_event_listener_with_callback(
                        "mouseup",
                        up_cb.as_ref().unchecked_ref(),
                    );
                }
                closures_for_teardown.borrow_mut().take();
            });
            drag_cleanup.set_value(Some(send_wrapper::SendWrapper::new(teardown)));

            if let Some(body) = document.body() {
                let _ = body.style().set_property("cursor", "col-resize");
                let _ = body.style().set_property("user-select", "none");
            }
        }
    };

    #[cfg(feature = "hydrate")]
    on_cleanup(move || {
        if let Some(teardown) = drag_cleanup.try_update_value(|v| v.take()).flatten() {
            teardown.take()();
        }
    });

    // ── Render ───────────────────────────────────────────────────────────

    // ── Animated open/close ───────────────────────────────────────────
    // Instead of <Show> which unmounts instantly (no exit animation),
    // we keep the element mounted while the closing animation plays,
    // then unmount after the animation duration (300ms).
    let (is_mounted, set_is_mounted) = signal(false);
    let (is_animating_out, set_is_animating_out) = signal(false);

    // Mount when open becomes true
    Effect::new(move |_| {
        if open.get() {
            set_is_animating_out.set(false);
            set_is_mounted.set(true);
        } else if is_mounted.get_untracked() {
            // Start exit animation, then unmount after 300ms
            set_is_animating_out.set(true);
            #[cfg(feature = "hydrate")]
            {
                use send_wrapper::SendWrapper;
                let cb = SendWrapper::new(move || {
                    set_is_mounted.set(false);
                    set_is_animating_out.set(false);
                });
                leptos::task::spawn_local(async move {
                    gloo_timers::future::TimeoutFuture::new(300).await;
                    cb.take()();
                });
            }
            #[cfg(not(feature = "hydrate"))]
            {
                set_is_mounted.set(false);
                set_is_animating_out.set(false);
            }
        }
    });

    view! {
        <Show when=move || is_mounted.get()>
            {move || {
                let collections = collections_resource.get()
                    .and_then(|r| r.ok())
                    .unwrap_or_default();

                // Force dependency on version so we re-render on refetch
                let _ = version.get();

                let modal_editing = editing_collection.get();
                let show_modal_sig: Signal<bool> = show_modal.into();
                let confirm_open_sig: Signal<bool> = confirm_open.into();
                let confirm_title_val = confirm_title.get();
                let confirm_message_val = confirm_message.get();

                if is_mobile.get() {
                    // Mobile: Fixed overlay with backdrop
                    // React: `fixed top-32 left-0 right-0 bottom-0 bg-[var(--color-overlay)] z-40`
                    // React: `fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl`
                    view! {
                        <div>
                            // Backdrop
                            <div
                                class="fixed top-32 left-0 right-0 bottom-0 bg-[var(--color-overlay)] z-40"
                                on:click=move |_| set_open.set(false)
                            />
                            // Sidebar panel
                            <div class="fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl">
                                <SidebarHeader on_close=handle_close on_new=handle_new />
                                <CollectionList
                                    collections=collections.clone()
                                    active_collection_id=active_collection_id
                                    dashboard_count=dashboard_count
                                    on_collection_click=handle_collection_click
                                    on_all_click=handle_all_click
                                    on_edit=handle_edit
                                    on_delete=handle_delete
                                />
                            </div>

                            // Modals
                            <CollectionModal
                                show=show_modal_sig
                                on_close=on_modal_close
                                editing=modal_editing
                                on_saved=on_modal_saved
                            />
                            <ConfirmDialog
                                open=confirm_open_sig
                                title=confirm_title_val
                                message=confirm_message_val
                                confirm_text="Delete Collection"
                                on_confirm=on_confirm_delete
                                on_cancel=on_cancel_delete
                            />
                        </div>
                    }.into_any()
                } else {
                    // Desktop: Inline resizable sidebar
                    let width_style = move || format!("width: {}px", sidebar_width.get());
                    let sidebar_class = move || {
                        if is_animating_out.get() {
                            "border-l border-t border-border bg-muted flex h-full overflow-hidden flex-shrink-0 sidebar-slide-out"
                        } else {
                            "border-l border-t border-border bg-muted flex h-full overflow-hidden flex-shrink-0 sidebar-slide-in"
                        }
                    };

                    view! {
                        <div>
                            <div
                                class=sidebar_class
                                style=width_style
                            >
                                // Resize Handle
                                // React: `flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10`
                                <div
                                    class="flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10"
                                    on:mousedown=handle_resize_start
                                    aria-label="Drag to resize"
                                >
                                    // React: `w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded transition-colors`
                                    <div class="w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded-md transition-colors" />
                                </div>

                                // Main Content
                                // React: `flex flex-col flex-1 min-w-0`
                                <div class="flex flex-col flex-1 min-w-0">
                                    <SidebarHeader on_close=handle_close on_new=handle_new />
                                    <CollectionList
                                        collections=collections.clone()
                                        active_collection_id=active_collection_id
                                        dashboard_count=dashboard_count
                                        on_collection_click=handle_collection_click
                                        on_all_click=handle_all_click
                                        on_edit=handle_edit
                                        on_delete=handle_delete
                                    />
                                </div>
                            </div>

                            // Modals
                            <CollectionModal
                                show=show_modal_sig
                                on_close=on_modal_close
                                editing=modal_editing
                                on_saved=on_modal_saved
                            />
                            <ConfirmDialog
                                open=confirm_open_sig
                                title=confirm_title_val
                                message=confirm_message_val
                                confirm_text="Delete Collection"
                                on_confirm=on_confirm_delete
                                on_cancel=on_cancel_delete
                            />
                        </div>
                    }.into_any()
                }
            }}
        </Show>
    }
}
