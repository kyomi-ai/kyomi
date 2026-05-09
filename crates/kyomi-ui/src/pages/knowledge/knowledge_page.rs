// SPDX-License-Identifier: AGPL-3.0-or-later

//! Knowledge page — thin wrapper around shared document components.
//!
//! Uses `DocumentCardGrid`, `SearchSortBar`, and `CollectionsSidebar` for
//! the reusable UI, adding only knowledge-specific logic: create action,
//! empty state text, and WebSocket subscription.
//!
//! Knowledge documents use `DocType::Knowledge` and are stored in the same
//! `dashboards` table. Clicking a card navigates to `/dashboard/{id}/edit`.
//!
//! ## Unified list-page filter skeleton (F-010)
//!
//! Shares the page-header + `SearchSortBar` skeleton with `dashboards_list.rs`
//! and `chat_list.rs`. Knowledge intentionally omits the chip row — there is
//! no scope dimension (e.g. All / Mine / Shared) that applies to the
//! single-user knowledge model. See `dashboards_list.rs` for the full
//! skeleton documentation.

use std::sync::Arc;

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};
use crate::components::documents::{DocumentCardGrid, DocumentCardGridSkeleton, SearchSortBar};
use crate::components::{
    Button, ButtonSize, ButtonVariant, ConfirmDialog, EmptyState, Spinner, ToggleButton,
};
use crate::pages::dashboards::CollectionsSidebar;
use crate::query_cache::{use_query, QueryCache};
use crate::server_fns::collections::{list_collections, CollectionItem};
use crate::server_fns::dashboards::DashboardListItem;
use crate::components::toast::{toast_error, toast_success};
use crate::server_fns::knowledge::{create_knowledge_doc, delete_knowledge_doc};

// ─────────────────────────────────────────────────────────────────────────────
// Main page component
// ─────────────────────────────────────────────────────────────────────────────

/// Knowledge page — card grid of knowledge documents with search, sort,
/// collections, and CRUD.
#[component]
pub fn KnowledgePage() -> impl IntoView {
    // ── Collection sidebar integration points ───────────────────────────
    let (collections_open, set_collections_open) = signal(false);
    let (active_collection_id, set_active_collection_id) = signal(Option::<String>::None);

    // ── Search + sort signals ───────────────────────────────────────────
    let (query_signal, set_query_signal) = signal(Option::<String>::None);
    let (sort_signal, set_sort_signal) = signal("recent".to_string());

    // ── Data fetching ───────────────────────────────────────────────────
    // Knowledge docs come from the SyncStore (populated from IndexedDB on
    // startup and kept current by the sync engine). Client-side
    // search/sort replaces the server-side query so list page navigation
    // is instant on return visits (KYO-169).
    let query_cache = expect_context::<QueryCache>();
    let sync_store = expect_context::<crate::cache::store::SyncStore>();
    let all_knowledge_docs = sync_store.knowledge_docs();

    let store_initialized = sync_store.initialized();

    // Client-side search + sort derived from the in-memory store.
    let knowledge_signal = Signal::derive(move || {
        let mut items = all_knowledge_docs.get();
        if let Some(q) = query_signal.try_get().flatten() {
            let q_lower = q.to_lowercase();
            items.retain(|d| {
                d.title.to_lowercase().contains(&q_lower)
                    || d.summary.as_deref().unwrap_or("").to_lowercase().contains(&q_lower)
            });
        }
        let sort = sort_signal.try_get().unwrap_or_default();
        match sort.as_str() {
            "updated_at" | "recent" | "" => items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
            "created_at" => items.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
            "title" => items.sort_by(|a, b| a.title.cmp(&b.title)),
            _ => items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
        }
        items
    });

    // Collections list, scoped to knowledge docs. Deps include the doc_type
    // so this entry stays distinct from the dashboards page's collections.
    let collections_resource = use_query(
        "collections",
        || Some("knowledge".to_string()),
        |dt: Option<String>| list_collections(dt),
    );

    // ── Delete confirmation ─────────────────────────────────────────────
    let (confirm_open, set_confirm_open) = signal(false);
    let (deleting_doc, set_deleting_doc) =
        signal(Option::<(String, String)>::None); // (id, title)

    let on_confirm_delete = Callback::new(move |()| {
        set_confirm_open.set(false);
        if let Some((doc_id, _title)) = deleting_doc.try_get_untracked().flatten() {
            leptos::task::spawn_local(async move {
                match delete_knowledge_doc(doc_id).await {
                    Ok(()) => toast_success("Document deleted"),
                    Err(e) => toast_error(format!("Failed to delete document: {e}")),
                }
                // Sync engine handles cache updates via WebSocket — no manual
                // invalidation needed for knowledge docs (KYO-169).
            });
        }
    });

    let on_cancel_delete = Callback::new(move |()| {
        set_confirm_open.set(false);
        set_deleting_doc.set(None);
    });

    // ── Create new knowledge document ───────────────────────────────────
    let (creating, set_creating) = signal(false);
    let navigate_create = StoredValue::new(leptos_router::hooks::use_navigate());

    let handle_create = move |_| {
        set_creating.set(true);
        let nav = navigate_create.get_value();
        leptos::task::spawn_local(async move {
            match create_knowledge_doc("Untitled Document".to_string(), None).await {
                Ok(doc_id) => {
                    let url = format!("/knowledge/{doc_id}/edit");
                    nav(&url, leptos_router::NavigateOptions::default());
                }
                Err(e) => {
                    leptos::logging::error!("Failed to create knowledge doc: {e}");
                    set_creating.try_set(false);
                }
            }
        });
    };

    // ── Filter documents by active collection ───────────────────────────
    // Returns None while the SyncStore has not yet been initialized (shows
    // the loading skeleton). Once initialized, returns the list (possibly
    // empty) filtered by the active collection.
    let filtered_docs = move || -> Option<Vec<DashboardListItem>> {
        if !store_initialized.get() {
            return None;
        }
        let docs = knowledge_signal.get();
        let active_id = active_collection_id.get();

        if let Some(ref coll_id) = active_id {
            let collection_doc_ids: std::collections::HashSet<String> =
                collections_resource
                    .get()
                    .and_then(|r| r.ok())
                    .unwrap_or_default()
                    .iter()
                    .filter(|c| c.collection_id == *coll_id)
                    .flat_map(|c| c.dashboards.iter().map(|d| d.dashboard_id.clone()))
                    .collect();

            Some(
                docs.into_iter()
                    .filter(|d| collection_doc_ids.contains(&d.dashboard_id))
                    .collect(),
            )
        } else {
            Some(docs)
        }
    };

    let get_collections = move || -> Vec<CollectionItem> {
        collections_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default()
    };

    // `dashboard_update` WebSocket subscription lives at the Layout level
    // (see `QueryCacheWsBridge` in `components/layout.rs`) so list caches
    // stay fresh across navigation — KYO-9.

    view! {
        <div class="flex flex-col h-full bg-background">
            // Row 1: Title + action buttons
            <div class="page-header h-16 px-4 md:px-6 flex-shrink-0 flex items-center justify-between">
                <h1 class="text-3xl font-display text-foreground">"Knowledge"</h1>

                <div class="flex items-center gap-2">
                    // Collections sidebar toggle
                    <ToggleButton
                        variant=Signal::derive(move || {
                            if collections_open.get() {
                                ButtonVariant::Active
                            } else {
                                ButtonVariant::Secondary
                            }
                        })
                        size=ButtonSize::Sm
                        aria_label=MaybeProp::from(Some("Manage Collections".to_string()))
                        on:click=move |_| set_collections_open.update(|v| *v = !*v)
                    >
                        <Icon icon=phosphor_leptos::STACK size="16px" />
                        <span class="hidden sm:inline">"Collections"</span>
                    </ToggleButton>

                    // Create Knowledge Document
                    <Button
                        size=ButtonSize::Sm
                        on:click=handle_create
                        disabled=Signal::derive(move || creating.get())
                    >
                        <Show
                            when=move || !creating.get()
                            fallback=|| view! { <Spinner class="text-primary-foreground" /> }
                        >
                            <Icon icon=phosphor_leptos::PLUS size="14px" />
                        </Show>
                        <span class="hidden sm:inline whitespace-nowrap">"New Document"</span>
                    </Button>
                </div>
            </div>

            // Row 2: Search + sort
            <SearchSortBar
                on_search=Callback::new(move |q| set_query_signal.set(q))
                on_sort=Callback::new(move |s| set_sort_signal.set(s))
                storage_key="kyomi_knowledge_sort"
                placeholder="Search knowledge..."
            />

            // Content area
            <div class="flex flex-1 min-h-0">
                // Main Content — Knowledge Grid
                <div class="flex-1 overflow-y-auto @container">
                    <div class="p-4 md:p-6">
                        <Transition fallback=move || view! { <DocumentCardGridSkeleton /> }>
                            {move || {
                                let collections = get_collections();

                                filtered_docs().map(|docs| {
                                    if docs.is_empty() {
                                        let create_cb = Callback::new(handle_create);
                                        view! {
                                            <KnowledgeEmptyState
                                                has_search=Signal::derive(move || query_signal.get().is_some())
                                                on_create=create_cb
                                            />
                                        }.into_any()
                                    } else {
                                        view! {
                                            <DocumentCardGrid
                                                dashboards=docs
                                                collections=collections
                                                on_delete=Callback::new(move |(id, title): (String, String)| {
                                                    set_deleting_doc.set(Some((id, title)));
                                                    set_confirm_open.set(true);
                                                })
                                                base_path="/knowledge"
                                            />
                                        }.into_any()
                                    }
                                })
                            }}
                        </Transition>
                    </div>
                </div>

                // Right sidebar — collections
                <CollectionsSidebar
                    open=Signal::derive(move || collections_open.get())
                    set_open=set_collections_open
                    active_collection_id=Signal::derive(move || active_collection_id.get())
                    set_active_collection_id=set_active_collection_id
                    on_collections_changed=Callback::new(move |()| {
                        query_cache.invalidate("collections");
                        // Knowledge docs list is kept current by the sync engine (KYO-169).
                    })
                    doc_type="knowledge".to_string()
                />
            </div>

            // Confirm dialog for delete
            <ConfirmDialog
                open=Signal::derive(move || confirm_open.get())
                title=Signal::derive(move || "Delete Document?".to_string())
                message=Signal::derive(move || {
                    deleting_doc.get()
                        .map(|(_, title)| format!("Are you sure you want to delete \"{title}\"? This action cannot be undone."))
                        .unwrap_or_default()
                })
                confirm_text="Delete"
                on_confirm=on_confirm_delete
                on_cancel=on_cancel_delete
            />
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sub-components (knowledge-specific)
// ─────────────────────────────────────────────────────────────────────────────

/// Knowledge icon for empty states.
#[component]
fn KnowledgeIcon() -> impl IntoView {
    view! {
        <Icon icon=phosphor_leptos::BOOK_OPEN weight=IconWeight::Duotone size="64px" />
    }
}

/// Empty state for the knowledge page.
#[component]
fn KnowledgeEmptyState(
    has_search: Signal<bool>,
    on_create: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    view! {
        {move || {
            if has_search.get() {
                view! {
                    <EmptyState
                        icon=Arc::new(|| view! { <KnowledgeIcon /> }.into_any())
                        title="No matching documents"
                        description="No knowledge documents found for your search. Try a different search term."
                    />
                }.into_any()
            } else {
                view! {
                    <EmptyState
                        icon=Arc::new(|| view! { <KnowledgeIcon /> }.into_any())
                        title="No knowledge documents yet"
                        description="What does your team need to know? Create your first knowledge document."
                        action=Arc::new(move || view! {
                            <Button on:click=move |ev| on_create.run(ev)>
                                <Icon icon=phosphor_leptos::PLUS size="14px" />
                                "Create Your First Document"
                            </Button>
                        }.into_any())
                    />
                }.into_any()
            }
        }}
    }
}
