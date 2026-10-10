// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard viewer page — read-only view of a single dashboard.
//!
//! Route: `/dashboard/:id`
//!
//! Full-featured dashboard viewer matching every feature in the React
//! `apps/frontend/src/pages/DashboardViewer.jsx`:
//! - Inline-editable title
//! - Parameter system with DashboardParameters
//! - Refresh All, Export PDF, History, Set Default buttons
//! - Mobile overflow menu
//! - Version preview with warning banner
//! - SaveDashboardModal and ChartInfoModal integration
//! - Footer with created/updated timestamps

use std::collections::HashMap;

use kyomi_types::Permission;
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::chartml_provider::{DashboardChartProviders, RefreshAllSignal};
use crate::components::dashboard::{
    ChartInfoModal, CopilotSidebar, DashboardParameters, HistoryPanel, MarkdownRenderer,
    SaveDashboardModal,
};
#[cfg(target_arch = "wasm32")]
use crate::components::toast::toast_error;
use crate::components::{
    Button, ButtonLink, ButtonSize, ButtonVariant, DetailPageSkeleton, Skeleton, ToggleButton,
};
use crate::parser::parse_markdown_chartml;
use crate::query_cache::QueryCache;
use crate::server_fns::context::UserContext;
use crate::server_fns::copilot::get_dashboard_copilot_access;
use crate::server_fns::dashboards::{
    get_dashboard, get_user_default_dashboard, get_workspace_default_dashboard,
    set_user_default_dashboard, set_workspace_default_dashboard, update_dashboard,
};
use phosphor_leptos::Icon;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

// ─── Relative time formatting ───────────────────────────────────────────────

/// Format an RFC 3339 timestamp as a localized date string.
///
/// Matches the React `new Date(dashboard.created_at).toLocaleDateString()` pattern.
fn format_date(iso: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    dt.format("%d/%m/%Y").to_string()
}

#[cfg(test)]
mod parameter_tests {
    use super::reconcile_parameters;
    use std::collections::HashMap;

    #[test]
    fn preserves_selected_filter_when_document_changes() {
        let previous = HashMap::from([
            ("region".to_string(), "West".to_string()),
            ("removed".to_string(), "old".to_string()),
        ]);
        let content = r#"```chartml
type: params
version: 1
params:
  - id: region
    type: select
    label: Region
    default: East
    options: [East, West]
  - id: year
    type: select
    label: Year
    default: "2026"
```"#;

        let values = reconcile_parameters(content, &previous);
        assert_eq!(values.get("region").map(String::as_str), Some("West"));
        assert_eq!(values.get("year").map(String::as_str), Some("2026"));
        assert!(!values.contains_key("removed"));
    }

    #[test]
    fn resets_filter_when_new_options_exclude_old_choice() {
        let previous = HashMap::from([("region".to_string(), "West".to_string())]);
        let content = r#"```chartml
type: params
version: 1
params:
  - id: region
    type: select
    label: Region
    default: East
    options: [East, North]
```"#;

        let values = reconcile_parameters(content, &previous);
        assert_eq!(values.get("region").map(String::as_str), Some("East"));
    }
}

/// Keep values the viewer chose when the saved document is refreshed, while
/// dropping parameters that no longer exist and seeding newly added ones.
fn reconcile_parameters(
    content: &str,
    previous: &HashMap<String, String>,
) -> HashMap<String, String> {
    fn as_text(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(value) => value.clone(),
            serde_json::Value::Number(value) => value.to_string(),
            serde_json::Value::Bool(value) => value.to_string(),
            other => other.to_string(),
        }
    }

    let mut values = HashMap::new();
    for group in parse_markdown_chartml(content).params {
        for param in group.params {
            if values.contains_key(&param.id) {
                continue;
            }
            if param.param_type == "daterange" {
                let default = param.default.as_ref().and_then(|value| value.as_array());
                for (index, suffix) in ["start", "end"].into_iter().enumerate() {
                    let key = format!("{}_{}", param.id, suffix);
                    if let Some(value) = previous.get(&key).cloned().or_else(|| {
                        default.and_then(|values| values.get(index)).map(as_text)
                    }) {
                        values.insert(key, value);
                    }
                }
                if let Some(value) = previous.get(&param.id).cloned().or_else(|| {
                    param.default.as_ref().filter(|value| !value.is_array()).map(as_text)
                }) {
                    values.insert(param.id, value);
                }
                continue;
            }

            let options = param.options.as_ref().map(|options| options.iter().map(as_text).collect::<Vec<_>>());
            let is_valid = |value: &str| match (param.param_type.as_str(), options.as_deref()) {
                ("select", Some(options)) if !options.is_empty() => options.iter().any(|option| option == value),
                ("multiselect", Some(options)) if !options.is_empty() => value.split(',').all(|part| options.iter().any(|option| option == part.trim())),
                _ => true,
            };
            let selected = previous.get(&param.id).filter(|value| is_valid(value)).cloned();
            let fallback = param.default.as_ref().map(|default| {
                if param.param_type == "multiselect" {
                    default.as_array().map(|items| items.iter().map(as_text).collect::<Vec<_>>().join(","))
                        .unwrap_or_else(|| as_text(default))
                } else {
                    as_text(default)
                }
            }).filter(|value| is_valid(value));
            if let Some(value) = selected.or(fallback).or_else(|| {
                if param.param_type == "select" { options.as_ref().and_then(|values| values.first().cloned()) } else { None }
            }) {
                values.insert(param.id, value);
            }
        }
    }
    values
}

// ─── Inline Editable Title ──────────────────────────────────────────────────

/// Inline editable title matching React `InlineEditableTitle`.
///
/// Click to edit, blur or Enter to save, Escape to cancel.
#[component]
fn InlineEditableTitle(
    /// Current title value.
    #[prop(into)]
    value: Signal<String>,
    /// Called with the new title when the user confirms the edit.
    on_save: Callback<String>,
) -> impl IntoView {
    let (editing, set_editing) = signal(false);
    let (draft, set_draft) = signal(String::new());
    let input_ref = NodeRef::<leptos::html::Input>::new();

    // When entering edit mode, populate the draft and focus
    let start_editing = move |_: leptos::ev::MouseEvent| {
        set_draft.set(value.get());
        set_editing.set(true);

        // Focus the input on next tick (after it renders)
        #[cfg(target_arch = "wasm32")]
        {
            let input_ref = input_ref;
            leptos::task::spawn_local(async move {
                // Yield to let the DOM update
                gloo_timers::future::TimeoutFuture::new(0).await;
                if let Some(el) = input_ref.get() {
                    let _ = el.focus();
                    el.select();
                }
            });
        }
    };

    let finish_editing = move || {
        let new_title = draft.get();
        let trimmed = new_title.trim().to_string();
        if !trimmed.is_empty() && trimmed != value.get() {
            on_save.run(trimmed);
        }
        set_editing.set(false);
    };

    let on_blur = move |_: leptos::ev::FocusEvent| {
        finish_editing();
    };

    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" {
            ev.prevent_default();
            finish_editing();
        } else if ev.key() == "Escape" {
            set_editing.set(false);
        }
    };

    view! {
        <div class="min-w-0 flex-1">
            {move || {
                if editing.get() {
                    view! {
                        <input
                            node_ref=input_ref
                            type="text"
                            class="text-3xl font-display text-foreground bg-transparent border-b-2 border-primary outline-none w-full"
                            prop:value=move || draft.get()
                            on:input=move |ev| set_draft.set(event_target_value(&ev))
                            on:blur=on_blur
                            on:keydown=on_keydown
                        />
                    }.into_any()
                } else {
                    // h1 is the page-level landmark per DESIGN.md §Accessibility
                    // landmark rules. Click behavior is preserved.
                    view! {
                        <h1
                            class="text-3xl font-display text-foreground truncate cursor-pointer hover:text-primary transition-colors block m-0"
                            on:click=start_editing
                            title="Click to edit title"
                        >
                            {move || value.get()}
                        </h1>
                    }.into_any()
                }
            }}
        </div>
    }
}

// ─── Not-found redirect ─────────────────────────────────────────────────────

/// Immediately redirect to the given `href`, replacing the current history entry.
///
/// Used in the dashboard viewer's `Err` branch when the server returns a
/// "not found" error: instead of showing a dead-end error page, we navigate
/// the user to the list view so they can pick a valid dashboard. The redirect
/// uses `replace: true` so the deleted-dashboard URL doesn't persist in
/// browser history and the user can still press Back.
///
/// Navigation is performed inside `Effect::new` rather than directly in the
/// component body, because this component is rendered inside a reactive view
/// closure (`{move || ...}`) — calling `navigate()` during that closure's
/// execution would be in the render phase. `Effect::new` runs after the
/// current render completes, which is the correct hook for side effects that
/// trigger navigation.
#[component]
fn NotFoundRedirect(href: String) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        let navigate = leptos_router::hooks::use_navigate();
        Effect::new(move |_| {
            navigate(
                &href,
                leptos_router::NavigateOptions {
                    replace: true,
                    ..Default::default()
                },
            );
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = href;
    // Minimal placeholder — the Effect fires immediately after mount, so the
    // user sees this for at most one frame before being redirected.
    view! { <div></div> }
}

// ─── Main component ─────────────────────────────────────────────────────────

/// Read-only dashboard viewer page.
///
/// Extracts `id` from the URL path params, fetches the dashboard detail
/// via `get_dashboard`, and renders the full viewer with toolbar, parameters,
/// content, history panel, modals, and footer.
#[component]
pub fn DashboardViewerPage() -> impl IntoView {
    // The chartml 5.0 resolver's KyomiDatasourceProvider + per-workspace
    // IndexedDB cache backend are wired by `DashboardChartProviders` (used
    // around the `MarkdownRenderer` below) once the workspace id is known.
    // We can't call `provide_context` here because the workspace id loads
    // asynchronously via the user-context resource.

    let params = use_params_map();
    let dashboard_id = Memo::new(move |_| {
        params.get().get("id").unwrap_or_default()
    });

    // This page is mounted at both /dashboard/:id and /knowledge/:id.
    // Derive URL base + "list" target from the current pathname so the
    // back button and edit link keep the user inside the right section.
    let location = leptos_router::hooks::use_location();
    let is_knowledge = Memo::new(move |_| location.pathname.get().starts_with("/knowledge"));
    // Knowledge documents don't participate in the default-dashboard system,
    // so hide the "Set as My Default" / "Set Workspace Default" toggles there.
    // Same pattern as `can_set_workspace_default` below: read once into a plain bool captured by
    // the view closures (the toolbar action closures are FnOnce, so reactive
    // `move || ...` wrappers around them won't compile).
    let show_default_toggles = Memo::new(move |_| !is_knowledge.get());
    let list_href = move || {
        if is_knowledge.get() {
            "/knowledge"
        } else {
            "/dashboards"
        }
    };
    let back_aria = move || {
        if is_knowledge.get() {
            "Back to knowledge"
        } else {
            "Back to dashboards"
        }
    };
    let not_found_label = move || {
        if is_knowledge.get() {
            "Knowledge Document Not Found"
        } else {
            "Dashboard Not Found"
        }
    };
    let back_label = move || {
        if is_knowledge.get() {
            "Back to Knowledge"
        } else {
            "Back to Dashboards"
        }
    };
    let base_path = move || {
        if is_knowledge.get() {
            "/knowledge"
        } else {
            "/dashboard"
        }
    };
    // Singular nouns for button labels, empty states, and PDF fallback filename.
    let edit_label = move || {
        if is_knowledge.get() {
            "Edit Document"
        } else {
            "Edit Dashboard"
        }
    };
    let empty_title = move || {
        if is_knowledge.get() {
            "This document is empty"
        } else {
            "This dashboard is empty"
        }
    };
    let empty_hint = move || {
        if is_knowledge.get() {
            "Click \"Edit Document\" to add content and charts"
        } else {
            "Click \"Edit Dashboard\" to add content and charts"
        }
    };
    // Used only inside the WASM-gated PDF download block below.
    #[cfg(target_arch = "wasm32")]
    let pdf_fallback_name = move || {
        if is_knowledge.get() {
            "Document.pdf"
        } else {
            "Dashboard.pdf"
        }
    };

    // ── User context (roles, capabilities) ──────────────────────────────
    // Provided by the parent Layout.
    let user_ctx_resource = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

    // ── Fetch dashboard detail ──────────────────────────────────────────
    let dashboard_resource = Resource::new(move || dashboard_id.get(), get_dashboard);
    let refresh_pending = RwSignal::new(false);
    let refresh_error = RwSignal::new(None::<String>);
    let receipt_sync_generation = RwSignal::new(0_u32);
    let request_dashboard_refresh = Callback::new(move |()| {
        refresh_pending.try_set(true);
        refresh_error.try_set(None);
        receipt_sync_generation.try_update(|generation| *generation += 1);
        dashboard_resource.refetch();
    });
    let copilot_access = Resource::new(
        move || (dashboard_id.get(), is_knowledge.get()),
        |(id, knowledge)| async move {
            let access = if knowledge { None } else { Some(get_dashboard_copilot_access(id.clone()).await) };
            (id, access)
        },
    );

    // ── Tier 2 cache: cached dashboard detail signal (KYO-215) ──────────
    // Populated from IndexedDB on WASM before the server response arrives,
    // giving the page instant content on revisit.  Stays None on SSR.
    // Uses spawn_local (not Resource::new) to avoid desyncing resource IDs.
    let cached_dashboard: RwSignal<Option<crate::server_fns::dashboards::DashboardDetail>> =
        RwSignal::new(None);

    // On WASM: read cache immediately when component mounts.  We read the
    // user context untracked so this fires once on mount, not on every ctx
    // change.
    #[cfg(target_arch = "wasm32")]
    {
        let dash_id = dashboard_id.get_untracked();
        let ws_id = user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.workspace_id)
            .unwrap_or_else(|| "default".to_string());
        leptos::task::spawn_local(async move {
            if let Ok(db) = crate::cache::db::init_cache_db(&ws_id).await
                && let Ok(entries) = crate::cache::db::read_all(
                    &db,
                    kyomi_types::sync::entity_types::DASHBOARD_DETAIL,
                    &ws_id,
                )
                .await
                && let Some((_id, json, _ts)) = entries.iter().find(|(id, _, _)| id == &dash_id)
                && let Ok(detail) =
                    serde_json::from_str::<crate::server_fns::dashboards::DashboardDetail>(json)
                && dashboard_id.try_get_untracked().as_deref() == Some(dash_id.as_str())
            {
                cached_dashboard.try_set(Some(detail));
            }
        });
    }

    // On WASM: when the server response resolves, write it to IndexedDB so the
    // next visit can use it immediately.
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        let Some(Ok(ref detail)) = dashboard_resource.get() else {
            return;
        };
        let detail_clone = detail.clone();
        // Read untracked — workspace_id doesn't change, and tracking it here
        // would re-run this write effect on every user-ctx change.
        let ws_id = user_ctx_resource
            .get_untracked()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.workspace_id)
            .unwrap_or_else(|| "default".to_string());
        leptos::task::spawn_local(async move {
            if let Ok(db) = crate::cache::db::init_cache_db(&ws_id).await {
                match serde_json::to_string(&detail_clone) {
                    Ok(json) => {
                        if let Err(e) = crate::cache::db::upsert(
                            &db,
                            kyomi_types::sync::entity_types::DASHBOARD_DETAIL,
                            &detail_clone.dashboard_id,
                            &ws_id,
                            &json,
                            &detail_clone.updated_at,
                        )
                        .await
                        {
                            tracing::warn!(
                                dashboard_id = %detail_clone.dashboard_id,
                                "dashboard_detail cache write failed: {e}"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            dashboard_id = %detail_clone.dashboard_id,
                            "dashboard_detail serialization failed: {e}"
                        );
                    }
                }
            }
        });
    });

    // ── Default dashboard state ─────────────────────────────────────────
    let user_default_resource = Resource::new(|| (), |_| get_user_default_dashboard());
    let workspace_default_resource = Resource::new(|| (), |_| get_workspace_default_dashboard());

    // ── History panel state ─────────────────────────────────────────────
    let (history_open, set_history_open) = signal(false);
    let (copilot_open, set_copilot_open) = signal(false);
    let current_copilot_access = Signal::derive(move || {
        copilot_access.try_get().flatten().and_then(|(id, access)| {
            (Some(id) == dashboard_id.try_get()).then_some(access).flatten()
        })
    });
    let copilot_read_only = Signal::derive(move || {
        !current_copilot_access.try_get().flatten().and_then(Result::ok).is_some_and(|access| access.can_edit)
    });
    let copilot_access_error = Signal::derive(move || {
        current_copilot_access.try_get().flatten().and_then(Result::err).map(|error| {
            format!("Could not confirm edit access: {error}. Copilot is in question-only mode.")
        })
    });
    let retry_copilot_access = Callback::new(move |()| copilot_access.refetch());

    // ── Preview state (from HistoryPanel on_preview) ────────────────────
    let (preview_content, set_preview_content) = signal(Option::<String>::None);

    // ── Parameter values ────────────────────────────────────────────────
    let (param_values, set_param_values) = signal(HashMap::<String, String>::new());
    let (params_initialized, set_params_initialized) = signal(false);
    let (title_override, set_title_override) = signal(Option::<String>::None);
    let last_parameter_content = RwSignal::new(None::<String>);

    // Keep this state above the resource-rendered subtree. A dashboard update
    // refetches that subtree; neither the chosen filters nor the chat session
    // should be recreated by the response.
    let current_dashboard = RwSignal::new(None::<crate::server_fns::dashboards::DashboardDetail>);
    let content_scroll_ref = NodeRef::<leptos::html::Div>::new();
    let saved_scroll_top = RwSignal::new(0_i32);
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if let Some(element) = content_scroll_ref.get() {
            element.set_scroll_top(saved_scroll_top.get_untracked());
        }
    });
    let last_viewed_dashboard_id = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        let id = dashboard_id.get();
        if last_viewed_dashboard_id.get_untracked().as_deref() == Some(id.as_str()) {
            return;
        }
        last_viewed_dashboard_id.set(Some(id));
        cached_dashboard.set(None);
        current_dashboard.set(None);
        saved_scroll_top.set(0);
        #[cfg(target_arch = "wasm32")]
        if let Some(element) = content_scroll_ref.get_untracked() {
            element.set_scroll_top(0);
        }
        refresh_pending.set(false);
        refresh_error.set(None);
        last_parameter_content.set(None);
        set_param_values.set(HashMap::new());
        set_params_initialized.set(false);
        set_preview_content.set(None);
        set_history_open.set(false);
        set_copilot_open.set(false);
        set_title_override.set(None);
    });
    Effect::new(move |_| {
        let dashboard = match dashboard_resource.get() {
            Some(Ok(dashboard)) => dashboard,
            Some(Err(error)) => {
                refresh_pending.set(false);
                refresh_error.set(Some(format!("Dashboard refresh failed: {error}")));
                return;
            }
            None => return,
        };
        if dashboard.dashboard_id != dashboard_id.get() {
            return;
        }
        refresh_pending.set(false);
        refresh_error.set(None);
        if last_parameter_content.get_untracked().as_deref() != Some(dashboard.content.as_str()) {
            let next = reconcile_parameters(&dashboard.content, &param_values.get_untracked());
            set_param_values.set(next);
            set_params_initialized.set(true);
            last_parameter_content.set(Some(dashboard.content.clone()));
        }
        if title_override.get_untracked().as_deref() == Some(dashboard.title.as_str()) {
            set_title_override.set(None);
        }
        current_dashboard.set(Some(dashboard));
    });
    let copilot_content = Signal::derive(move || {
        preview_content
            .try_get().flatten()
            .or_else(|| current_dashboard.try_get().flatten().map(|dashboard| dashboard.content))
            .unwrap_or_default()
    });
    let copilot_view_context = Signal::derive(move || {
        let Some(dashboard) = current_dashboard.try_get().flatten() else {
            return String::new();
        };
        let mut context = serde_json::json!({
            "active_filters": param_values.try_get().unwrap_or_default(),
            "dashboard_updated_at": dashboard.updated_at,
            "view_observed_at_utc": chrono::Utc::now().to_rfc3339(),
            "chart_data_freshness": "not verified by this viewer",
            "viewer_timezone": crate::utils::time::get_user_timezone(),
        })
        .to_string();
        if let Some(preview) = preview_content.try_get().flatten() {
            context.push_str("\nHistorical preview content:\n");
            context.push_str(&preview);
        }
        context
    });
    let copilot_filter_summary = Signal::derive(move || {
        let mut filters = param_values.try_get().unwrap_or_default().into_iter().collect::<Vec<_>>();
        filters.sort_by(|left, right| left.0.cmp(&right.0));
        if filters.is_empty() { return "Active filters: none".to_string(); }
        let extra = filters.len().saturating_sub(3);
        let mut summary = filters.into_iter().take(3)
            .map(|(name, value)| format!("{name}: {value}"))
            .collect::<Vec<_>>().join(" · ");
        if extra > 0 { summary.push_str(&format!(" · +{extra} more")); }
        format!("Active filters: {summary}")
    });
    let no_pending_editor_save: crate::components::chat::BeforeSendHook = std::sync::Arc::new(move || {
        let confirmed_reader = current_copilot_access.get_untracked()
            .and_then(Result::ok).is_some_and(|access| !access.can_edit);
        let question_only = preview_content.get_untracked().is_some() || confirmed_reader;
        let ready = question_only || (current_dashboard.get_untracked().is_some()
            && !refresh_pending.get_untracked() && refresh_error.get_untracked().is_none());
        Box::pin(async move { ready })
    });

    // ── Modal states ────────────────────────────────────────────────────
    let (save_modal_open, set_save_modal_open) = signal(false);
    let (save_modal_yaml, set_save_modal_yaml) = signal(String::new());
    let (chart_info_open, set_chart_info_open) = signal(false);
    let (chart_info_yaml, set_chart_info_yaml) = signal(String::new());

    // ── Mobile overflow menu ────────────────────────────────────────────
    let (overflow_open, set_overflow_open) = signal(false);

    // ── PDF export loading state ────────────────────────────────────────
    let (is_exporting, set_is_exporting) = signal(false);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = set_is_exporting;

    // ── Dashboard-wide refresh signal ───────────────────────────────────
    // Provided here (above `DashboardChartProviders`) so the toolbar's
    // "Refresh All" button — which sits as a sibling of the providers, not
    // a descendant — can share the same `RwSignal` that every nested
    // `ChartBlock` reads from context. Bumping `refresh_all` propagates
    // through `ChartBlock`'s `combined_refresh` derived signal into
    // `chartml_leptos::ChartMLChart`'s `refresh_trigger` prop, which
    // invalidates each spec source's resolver key and re-runs the fetch
    // pipeline. Replaces the legacy `dashboard-refresh-all` CustomEvent
    // dispatch (which had no listener) and the per-chart
    // `resolver.invalidate_all()` round-trip in `ChartBlock`.
    let refresh_all: RefreshAllSignal = RwSignal::new(0_u32);
    provide_context(refresh_all);

    // ── Title update Action ─────────────────────────────────────────────
    // Created at component scope so its lifecycle matches the component.
    // Dispatched from the on_title_save callback with (dashboard_id, new_title).
    // On error, reverts the optimistic title override.
    let update_title_action = Action::new(|(did, new_title): &(String, String)| {
        let did = did.clone();
        let new_title = new_title.clone();
        async move { update_dashboard(did, Some(new_title), None, None).await }
    });

    Effect::new(move |_| {
        if let Some(Err(e)) = update_title_action.value().get() {
            set_title_override.set(None);
            leptos::logging::error!("Failed to update title: {}", e);
        }
    });

    // Layout-level QueryCache — toggling the per-user or per-workspace
    // default dashboard must invalidate the `landing_config` entry so the
    // home-page redirect and sidebar "Dashboards" link (KYO-111) pick up
    // the new default without a full browser refresh (KYO-127).
    let query_cache = expect_context::<QueryCache>();

    // ── Toggle user-default action ──────────────────────────────────────
    // Input: (dashboard_id, is_currently_default) — the bool is captured at
    // dispatch time so the async body sees the value the user acted on, not
    // whatever the signal says after the server round-trip completes.
    let toggle_user_default_action =
        Action::new(move |(did, currently_default): &(String, bool)| {
            let new_id = if *currently_default {
                None
            } else {
                Some(did.clone())
            };
            async move { set_user_default_dashboard(new_id).await }
        });

    Effect::new(move |_| {
        if let Some(result) = toggle_user_default_action.value().get() {
            match result {
                Ok(()) => {
                    user_default_resource.refetch();
                    query_cache.invalidate("landing_config");
                }
                Err(e) => {
                    leptos::logging::error!("Failed to set user default: {}", e);
                }
            }
        }
    });

    // ── Toggle workspace-default action ─────────────────────────────────
    // Same dispatch-time value threading pattern as toggle_user_default_action.
    let toggle_ws_default_action = Action::new(move |(did, currently_default): &(String, bool)| {
        let new_id = if *currently_default {
            None
        } else {
            Some(did.clone())
        };
        async move { set_workspace_default_dashboard(new_id).await }
    });

    Effect::new(move |_| {
        if let Some(result) = toggle_ws_default_action.value().get() {
            match result {
                Ok(()) => {
                    workspace_default_resource.refetch();
                    query_cache.invalidate("landing_config");
                }
                Err(e) => {
                    leptos::logging::error!("Failed to set workspace default: {}", e);
                }
            }
        }
    });

    // ── Ask-about-chart action ──────────────────────────────────────────
    // Stores chart context on the server then navigates to /chat?chart=<id>.
    let navigate_for_ask = leptos_router::hooks::use_navigate();
    let ask_about_chart_action = Action::new(move |chart_md: &String| {
        let chart_md = chart_md.clone();
        async move {
            crate::server_fns::chat::store_chart_context_for_ask(
                chart_md,
                "Chart Exploration".to_string(),
            )
            .await
        }
    });

    {
        let navigate_for_ask = navigate_for_ask.clone();
        Effect::new(move |_| {
            if let Some(result) = ask_about_chart_action.value().get() {
                match result {
                    Ok(chart_id) => {
                        navigate_for_ask(
                            &format!("/chat?chart={chart_id}"),
                            leptos_router::NavigateOptions::default(),
                        );
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to store chart context for ask");
                    }
                }
            }
        });
    }

    // ── WebSocket subscription: dashboard_update ─────────────────────────
    // When another user or agent updates/deletes the currently viewed
    // dashboard, react in real-time: refetch on "updated", navigate away
    // on "deleted".
    #[cfg(target_arch = "wasm32")]
    {
        use crate::components::chat::websocket_client::WebSocketContext;
        let ws_ctx = use_context::<WebSocketContext>();
        let navigate_ws = leptos_router::hooks::use_navigate();

        let ws_ctx_for_effect = ws_ctx.clone();
        Effect::new(move |_| {
            let Some(ws) = ws_ctx_for_effect.as_ref().cloned() else {
                return;
            };
            let navigate = navigate_ws.clone();

            let unsub = ws.subscribe("dashboard_update", move |msg| {
                let data = match &msg.data {
                    Some(d) => d,
                    None => return,
                };

                let event_dashboard_id = match data.get("dashboard_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return,
                };

                // Only process events for the currently viewed dashboard
                let current_id = dashboard_id.get_untracked();
                if event_dashboard_id != current_id {
                    return;
                }

                let action = data.get("action").and_then(|v| v.as_str()).unwrap_or("");

                match action {
                    "updated" => {
                        request_dashboard_refresh.try_run(());
                    }
                    "deleted" => {
                        navigate(list_href(), leptos_router::NavigateOptions::default());
                    }
                    _ => {}
                }
            });

            let unsub = send_wrapper::SendWrapper::new(unsub);
            on_cleanup(move || {
                unsub.take()();
            });
        });
    }

    view! {
        <div class="flex h-full min-w-0 overflow-hidden">
        <div class="flex-1 min-w-0 overflow-hidden">
        <Transition fallback=move || view! { <DetailPageSkeleton /> }>
            {move || {
                let dashboard_result = dashboard_resource.get();
                let user_ctx_result = user_ctx_resource.get();

                // Tier 2 cache (KYO-215): if the server resource hasn't resolved yet
                // but user context is ready and we have a cached dashboard, render it
                // immediately — no skeleton flash for returning visitors.
                let (dashboard_result, user_ctx_result) = match (dashboard_result, user_ctx_result) {
                    (Some(d), Some(u)) => (d, u),
                    (None, Some(u)) => {
                        // Server not ready yet — use cached version if available.
                        match cached_dashboard.get().filter(|cached| cached.dashboard_id == dashboard_id.get()) {
                            Some(cached) => (Ok(cached), u),
                            None => return None,
                        }
                    }
                    _ => return None,
                };
                if dashboard_result.as_ref().ok().is_some_and(|dashboard| dashboard.dashboard_id != dashboard_id.get()) {
                    return None;
                }

                // Get user context (gracefully handle errors)
                let user_ctx = user_ctx_result.ok();

                // `set_workspace_default_dashboard` gates on
                // `ac.require(Permission::SetWorkspaceDefaults, ...)` — see
                // server_fns/dashboards.rs. Mirror that here so non-admins never
                // see a toggle that will 403.
                let can_set_workspace_default = user_ctx.as_ref()
                    .map(|ctx| ctx.can(Permission::SetWorkspaceDefaults))
                    .unwrap_or(false);

                // Read the route-derived gate once so the non-reactive view
                // closures below (several are FnOnce) can capture a plain bool.
                // Route changes remount this component, so a one-shot read is
                // correct here — same pattern as `can_set_workspace_default`.
                let show_default_toggles = show_default_toggles.get();

                let pdf_export_enabled = user_ctx.as_ref()
                    .and_then(|ctx| ctx.capabilities.get("pdf_export_enabled"))
                    .copied()
                    .unwrap_or(false);

                let chart_palette = user_ctx.as_ref()
                    .map(|ctx| ctx.chart_palette.clone())
                    .unwrap_or_else(|| "kyomi".to_string());

                // Workspace UUID used by `KyomiDatasourceProvider` to namespace
                // every cache entry (cross-workspace isolation) and by
                // `IndexedDbBackend` to scope the persistent tier-2 cache so
                // multiple users on the same browser cannot read each other's
                // cached query results. Falls back to "default" only when the
                // user context lacks a workspace_id (single-tenant deployments
                // and the legacy free-tier path) — in that case the namespace
                // simply doesn't isolate, which matches what the legacy
                // bespoke fetch path did.
                let workspace_id = user_ctx.as_ref()
                    .and_then(|ctx| ctx.workspace_id.clone())
                    .unwrap_or_else(|| "default".to_string());

                Some(match dashboard_result {
                    Err(e) => {
                        let err_msg = e.to_string();
                        if err_msg.to_lowercase().contains("not found") {
                            // Dashboard was deleted — redirect to the list so
                            // the user can pick a valid one. replace:true keeps
                            // the deleted URL out of browser history.
                            view! { <NotFoundRedirect href=list_href().to_string() /> }.into_any()
                        } else {
                            view! {
                                <div class="flex h-full items-center justify-center bg-background">
                                    <div class="text-center">
                                        <h2 class="text-lg font-semibold text-foreground mb-4">
                                            {not_found_label()}
                                        </h2>
                                        <p class="text-muted-foreground mb-6">
                                            {err_msg}
                                        </p>
                                        <div class="flex items-center justify-center gap-3">
                                            <Button
                                                variant=ButtonVariant::Secondary
                                                disabled=Signal::derive(move || refresh_pending.try_get().unwrap_or(false))
                                                on:click=move |_| request_dashboard_refresh.run(())
                                            >"Retry"</Button>
                                            <ButtonLink href=list_href().to_string()>
                                                {back_label()}
                                            </ButtonLink>
                                        </div>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }
                    Ok(dashboard) => {
                        let did = dashboard.dashboard_id.clone();
                        let did_for_pdf = did.clone();
                        let did_for_history = did.clone();
                        let did_for_user_default = did.clone();
                        let did_for_ws_default = did.clone();
                        let did_for_user_default_mobile = did.clone();
                        let did_for_ws_default_mobile = did.clone();
                        let did_for_title = did.clone();

                        let content = dashboard.content.clone();
                        let edit_href = format!("{}/{}/edit", base_path(), did);
                        let edit_href_empty = edit_href.clone();
                        let created_at = dashboard.created_at.clone();
                        let updated_at = dashboard.updated_at.clone();
                        let original_title = dashboard.title.clone();

                        // ── Title signal ────────────────────────────────
                        let title_signal = Signal::derive({
                            let original = original_title.clone();
                            move || {
                                title_override.get().unwrap_or_else(|| original.clone())
                            }
                        });

                        // ── Title save handler ─────────────────────────
                        let on_title_save = Callback::new({
                            let did_for_title = did_for_title.clone();
                            move |new_title: String| {
                                let did = did_for_title.clone();
                                // Optimistic update — revert in the Effect above if the server call fails.
                                set_title_override.set(Some(new_title.clone()));
                                update_title_action.dispatch((did, new_title));
                            }
                        });

                        // ── Refresh All handler ────────────────────────
                        // Bump the dashboard-wide `RefreshAllSignal` (provided
                        // via context at the top of this component). Every
                        // nested `ChartBlock` folds the bumped value into its
                        // `ChartMLChart`'s `refresh_trigger`, which invalidates
                        // each chart's resolver cache keys and re-runs the
                        // fetch pipeline.
                        let on_refresh_all = move |_: leptos::ev::MouseEvent| {
                            refresh_all.update(|n| *n += 1);
                        };

                        // ── PDF export handler ─────────────────────────
                        let did_for_pdf = StoredValue::new(did_for_pdf);
                        #[cfg(not(target_arch = "wasm32"))]
                        let _ = did_for_pdf;
                        let on_download_pdf = move |_: leptos::ev::MouseEvent| {
                            #[cfg(target_arch = "wasm32")]
                            {
                                if is_exporting.get() { return; }
                                set_is_exporting.set(true);
                                let Some(did) = did_for_pdf.try_get_value() else { return };
                                let params = param_values.get();
                                let title = title_signal.get();

                                leptos::task::spawn_local(async move {
                                    let mut url = format!("/api/v1/dashboards/{}/export/pdf", did);
                                    if !params.is_empty()
                                        && let Ok(json) = serde_json::to_string(&params) {
                                            let encoded = js_sys::encode_uri_component(&json);
                                            url = format!("{}?parameters={}", url, encoded);
                                        }

                                    // Use fetch API with credentials to download as blob
                                    if let Some(window) = web_sys::window() {
                                        let opts = web_sys::RequestInit::new();
                                        opts.set_method("GET");
                                        opts.set_credentials(web_sys::RequestCredentials::Include);

                                        let promise = window.fetch_with_str_and_init(&url, &opts);
                                        match wasm_bindgen_futures::JsFuture::from(promise).await {
                                            Ok(resp) => {
                                                let resp: web_sys::Response = resp.unchecked_into();
                                                if resp.ok() {
                                                    if let Ok(blob_promise) = resp.blob()
                                                        && let Ok(blob) = wasm_bindgen_futures::JsFuture::from(blob_promise).await {
                                                            let blob: web_sys::Blob = blob.unchecked_into();
                                                            let blob_url = web_sys::Url::create_object_url_with_blob(&blob).unwrap_or_default();
                                                            let Some(document) = window.document() else { return };
                                                            let Ok(a) = document.create_element("a") else { return };
                                                            let _ = a.set_attribute("href", &blob_url);

                                                            // Derive filename from Content-Disposition header or title
                                                            let filename = resp.headers().get("content-disposition").ok().flatten()
                                                                .and_then(|cd| {
                                                                    // Parse: attachment; filename="Some_Title.pdf"
                                                                    cd.split(';')
                                                                        .find_map(|part| {
                                                                            let part = part.trim();
                                                                            part.strip_prefix("filename=")
                                                                                .map(|v| v.trim_matches('"').to_string())
                                                                        })
                                                                })
                                                                .unwrap_or_else(|| {
                                                                    if title.is_empty() {
                                                                        pdf_fallback_name().to_string()
                                                                    } else {
                                                                        // Sanitize title: keep alphanumeric, spaces, hyphens
                                                                        let safe: String = title.chars()
                                                                            .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
                                                                            .collect();
                                                                        let safe = safe.split_whitespace().collect::<Vec<_>>().join("_");
                                                                        format!("{}.pdf", safe)
                                                                    }
                                                                });

                                                            let _ = a.set_attribute("download", &filename);
                                                            if let Ok(html_a) = a.dyn_into::<web_sys::HtmlElement>()
                                                                && let Some(body) = document.body() {
                                                                    let _ = body.append_child(&html_a);
                                                                    html_a.click();
                                                                    let _ = body.remove_child(&html_a);
                                                                }
                                                            let _ = web_sys::Url::revoke_object_url(&blob_url);
                                                        }
                                                } else {
                                                    let status = resp.status();
                                                    // KYO-806: raw fetch, bypasses
                                                    // PaywallAwareClient — check here.
                                                    crate::utils::billing_lapse::check_rest_response_status(status);
                                                    let message = if status == 403 {
                                                        "PDF export requires a paid plan".to_string()
                                                    } else {
                                                        // Try to extract detail from JSON error body
                                                        let mut msg = format!("PDF export failed (HTTP {})", status);
                                                        if let Ok(text_promise) = resp.text()
                                                            && let Ok(text_val) = wasm_bindgen_futures::JsFuture::from(text_promise).await
                                                                && let Some(text) = text_val.as_string()
                                                                    && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text)
                                                                        && let Some(detail) = parsed.get("detail").or(parsed.get("message")).and_then(|v| v.as_str()) {
                                                                            msg = format!("PDF export failed: {}", detail);
                                                                        }
                                                        msg
                                                    };
                                                    leptos::logging::error!("{}", message);
                                                    toast_error(message);
                                                }
                                            }
                                            Err(e) => {
                                                leptos::logging::error!("PDF export error: {:?}", e);
                                                toast_error("Failed to export PDF");
                                            }
                                        }
                                    }

                                    set_is_exporting.try_set(false);
                                });
                            }
                        };
                        let on_download_pdf_mobile = on_download_pdf;

                        // ── History panel callbacks ────────────────────
                        let on_history_close = Callback::new(move |()| set_history_open.set(false));
                        let on_history_preview = Callback::new(move |content: Option<String>| {
                            set_preview_content.set(content);
                        });
                        let on_history_restore = Callback::new(move |()| {
                            set_history_open.set(false);
                            set_preview_content.set(None);
                            request_dashboard_refresh.run(());
                        });

                        // ── Chart action callbacks ─────────────────────
                        let on_save_to_dashboard = Callback::new(move |yaml: String| {
                            set_save_modal_yaml.set(yaml);
                            set_save_modal_open.set(true);
                        });

                        let on_chart_info = Callback::new(move |yaml: String| {
                            set_chart_info_yaml.set(yaml);
                            set_chart_info_open.set(true);
                        });

                        let on_ask_about_chart = Callback::new(move |chart_md: String| {
                            ask_about_chart_action.dispatch(chart_md);
                        });

                        // ── Default dashboard toggle handlers ──────────
                        let is_user_default = {
                            let did = did_for_user_default.clone();
                            Signal::derive(move || {
                                user_default_resource.get()
                                    .and_then(|r| r.ok())
                                    .flatten()
                                    .map(|id| id == did)
                                    .unwrap_or(false)
                            })
                        };

                        let is_workspace_default = {
                            let did = did_for_ws_default.clone();
                            Signal::derive(move || {
                                workspace_default_resource.get()
                                    .and_then(|r| r.ok())
                                    .flatten()
                                    .map(|id| id == did)
                                    .unwrap_or(false)
                            })
                        };

                        let toggle_user_default = {
                            let did = did_for_user_default.clone();
                            move |_: leptos::ev::MouseEvent| {
                                if toggle_user_default_action.pending().get_untracked() { return; }
                                let currently_default = is_user_default.get_untracked();
                                toggle_user_default_action.dispatch((did.clone(), currently_default));
                            }
                        };
                        let toggle_user_default_mobile = {
                            let did = did_for_user_default_mobile.clone();
                            move |_: leptos::ev::MouseEvent| {
                                if toggle_user_default_action.pending().get_untracked() { return; }
                                let currently_default = is_user_default.get_untracked();
                                set_overflow_open.set(false);
                                toggle_user_default_action.dispatch((did.clone(), currently_default));
                            }
                        };

                        let toggle_ws_default = {
                            let did = did_for_ws_default.clone();
                            move |_: leptos::ev::MouseEvent| {
                                if toggle_ws_default_action.pending().get_untracked() { return; }
                                let currently_default = is_workspace_default.get_untracked();
                                toggle_ws_default_action.dispatch((did.clone(), currently_default));
                            }
                        };
                        let toggle_ws_default_mobile = {
                            let did = did_for_ws_default_mobile.clone();
                            move |_: leptos::ev::MouseEvent| {
                                if toggle_ws_default_action.pending().get_untracked() { return; }
                                let currently_default = is_workspace_default.get_untracked();
                                set_overflow_open.set(false);
                                toggle_ws_default_action.dispatch((did.clone(), currently_default));
                            }
                        };

                        // ── Close overflow on click outside ────────────
                        #[cfg(target_arch = "wasm32")]
                        {
                            Effect::new(move |_| {
                                if overflow_open.get() {
                                    // Auto-close after a short delay on next click
                                }
                            });
                        }

                        // ── Parse dashboard params for DashboardParameters ──
                        let parsed_for_params = parse_markdown_chartml(&content);
                        let dashboard_params: Vec<crate::parser::ParamDef> = parsed_for_params
                            .params
                            .iter()
                            .flat_map(|g| g.params.clone())
                            .collect();
                        let has_params = !dashboard_params.is_empty();

                        // ── Content signal for MarkdownRenderer ─────────
                        let content_for_renderer = content.clone();
                        let display_content = Signal::derive(move || {
                            preview_content.get().unwrap_or_else(|| content_for_renderer.clone())
                        });

                        // ── Saved modal callbacks ───────────────────────
                        let on_save_modal_close = Callback::new(move |()| {
                            set_save_modal_open.set(false);
                        });

                        let on_save_modal_saved = Callback::new(move |_dashboard_id: String| {
                            set_save_modal_open.set(false);
                        });

                        let on_chart_info_close = Callback::new(move |()| {
                            set_chart_info_open.set(false);
                        });

                        // ── Format dates for footer ─────────────────────
                        let created_date = format_date(&created_at);
                        let updated_date = format_date(&updated_at);

                        view! {
                            <div class="flex flex-col h-full bg-background overflow-hidden @container" style:flex-direction="column">
                                // ─── Header / Toolbar ───────────────────
                                <div class="page-header h-16 bg-background px-4 md:px-6 flex-shrink-0 flex items-center justify-between">
                                    // Left: back button + editable title
                                    <div class="flex items-center gap-4 flex-1 min-w-0 overflow-hidden">
                                        <ButtonLink href=list_href().to_string() variant=ButtonVariant::Ghost size=ButtonSize::Icon class="flex-shrink-0 text-muted-foreground hover:text-foreground" aria_label=back_aria().to_string()>
                                            <Icon icon=phosphor_leptos::CARET_LEFT size="18px" />
                                        </ButtonLink>

                                        <InlineEditableTitle
                                            value=title_signal
                                            on_save=on_title_save
                                        />
                                    </div>

                                    // Right: action buttons
                                    <div class="flex items-center gap-1 @6xl:gap-2 flex-shrink-0">
                                        // Refresh All
                                        <Button variant=ButtonVariant::Secondary size=ButtonSize::Sm aria_label="Refresh all charts" on:click=on_refresh_all>
                                            <Icon icon=phosphor_leptos::ARROWS_CLOCKWISE size="14px" />
                                            <span class="hidden @6xl:inline whitespace-nowrap">"Refresh All"</span>
                                        </Button>

                                        <Show when=move || !is_knowledge.get()>
                                        <ToggleButton
                                            variant=Signal::derive(move || if copilot_open.get() { ButtonVariant::Active } else { ButtonVariant::Secondary })
                                            size=ButtonSize::Sm
                                            aria_label="Toggle Copilot"
                                            on:click=move |_| {
                                                set_copilot_open.update(|open| *open = !*open);
                                            }
                                        >
                                            <Icon icon=phosphor_leptos::SPARKLE size="14px" />
                                            <span class="hidden @6xl:inline whitespace-nowrap">"Copilot"</span>
                                        </ToggleButton>
                                        </Show>

                                        // Download PDF — visible when toolbar has room
                                        {pdf_export_enabled.then(|| {
                                            let on_download = on_download_pdf;
                                            view! {
                                                <div class="hidden @3xl:flex">
                                                    <Button variant=ButtonVariant::Secondary size=ButtonSize::Sm aria_label="Download PDF" disabled=Signal::derive(move || is_exporting.get()) on:click=on_download>
                                                        <Icon icon=phosphor_leptos::DOWNLOAD_SIMPLE size="14px" />
                                                        <span class="hidden @6xl:inline whitespace-nowrap">
                                                            {move || if is_exporting.get() { "Exporting..." } else { "Download PDF" }}
                                                        </span>
                                                    </Button>
                                                </div>
                                            }
                                        })}

                                        // History — visible when toolbar has room
                                        <div class="hidden @3xl:flex">
                                            <ToggleButton
                                                variant=Signal::derive(move || if history_open.get() { ButtonVariant::Active } else { ButtonVariant::Secondary })
                                                size=ButtonSize::Sm
                                                aria_label="Toggle version history"
                                                on:click=move |_| set_history_open.update(|o| *o = !*o)
                                            >
                                                <Icon icon=phosphor_leptos::CLOCK size="14px" />
                                                <span class="hidden @6xl:inline whitespace-nowrap">"History"</span>
                                            </ToggleButton>
                                        </div>

                                        // Set as My Default — visible when toolbar has room, dashboards only
                                        {show_default_toggles.then(|| view! {
                                            <div class="hidden @3xl:flex">
                                                <ToggleButton
                                                    variant=Signal::derive(move || if is_user_default.get() { ButtonVariant::Active } else { ButtonVariant::Secondary })
                                                    size=ButtonSize::Sm
                                                    aria_label=Signal::derive(move || if is_user_default.get() { "Remove as my default".to_string() } else { "Set as my default".to_string() })
                                                    disabled=Signal::derive(move || toggle_user_default_action.pending().get())
                                                    on:click=toggle_user_default
                                                >
                                                    <Icon icon=phosphor_leptos::STAR size="14px" />
                                                    <span class="hidden @6xl:inline whitespace-nowrap">
                                                        {move || if is_user_default.get() { "My Default" } else { "Set as My Default" }}
                                                    </span>
                                                </ToggleButton>
                                            </div>
                                        })}

                                        // Set Workspace Default — visible when toolbar has room, admin only, dashboards only
                                        {(show_default_toggles && can_set_workspace_default).then(|| {
                                            view! {
                                                <div class="hidden @3xl:flex">
                                                    <ToggleButton
                                                        variant=Signal::derive(move || if is_workspace_default.get() { ButtonVariant::Active } else { ButtonVariant::Secondary })
                                                        size=ButtonSize::Sm
                                                        aria_label=Signal::derive(move || if is_workspace_default.get() { "Remove as workspace default".to_string() } else { "Set as workspace default".to_string() })
                                                        disabled=Signal::derive(move || toggle_ws_default_action.pending().get())
                                                        on:click=toggle_ws_default
                                                    >
                                                        <Icon icon=phosphor_leptos::HOUSE size="14px" />
                                                        <span class="hidden @6xl:inline whitespace-nowrap">
                                                            {move || if is_workspace_default.get() { "Workspace Default" } else { "Set Workspace Default" }}
                                                        </span>
                                                    </ToggleButton>
                                                </div>
                                            }
                                        })}

                                        // ─── Mobile overflow menu ───────
                                        <div class="relative flex @3xl:hidden">
                                            <Button variant=ButtonVariant::Secondary size=ButtonSize::Icon aria_label="More actions" on:click=move |_| set_overflow_open.update(|o| *o = !*o)>
                                                <Icon icon=phosphor_leptos::DOTS_THREE_VERTICAL size="14px" />
                                            </Button>

                                            {move || overflow_open.get().then(|| {
                                                let on_download_pdf_m = on_download_pdf_mobile;
                                                let toggle_user_m = toggle_user_default_mobile.clone();
                                                let toggle_ws_m = toggle_ws_default_mobile.clone();

                                                view! {
                                                    <div class="absolute right-0 top-full mt-1 w-56 bg-popover border border-border rounded-md shadow-lg z-50">
                                                        // Download PDF (mobile)
                                                        {pdf_export_enabled.then(|| {
                                                            view! {
                                                                <button
                                                                    class="menu-item"
                                                                    disabled=move || is_exporting.get()
                                                                    on:click=move |ev| { set_overflow_open.set(false); on_download_pdf_m(ev); }
                                                                >
                                                                    <Icon icon=phosphor_leptos::DOWNLOAD_SIMPLE size="14px" />
                                                                    {move || if is_exporting.get() { "Exporting..." } else { "Download PDF" }}
                                                                </button>
                                                            }
                                                        })}

                                                        // History (mobile)
                                                        <button
                                                            class="menu-item"
                                                            on:click=move |_| { set_overflow_open.set(false); set_history_open.update(|o| *o = !*o); }
                                                        >
                                                            <Icon icon=phosphor_leptos::CLOCK size="14px" />
                                                            {move || if history_open.get() { "Close History" } else { "Version History" }}
                                                        </button>

                                                        // Set as My Default (mobile) — dashboards only.
                                                        // The preceding divider separates History from the default-toggle
                                                        // group, so we gate it alongside the toggle to avoid an orphan rule.
                                                        {show_default_toggles.then(|| view! {
                                                            <div class="border-t border-border my-1" />
                                                            <button
                                                                class="menu-item"
                                                                disabled=move || toggle_user_default_action.pending().get()
                                                                on:click=toggle_user_m
                                                            >
                                                                <Icon icon=phosphor_leptos::STAR size="14px" />
                                                                {move || if is_user_default.get() { "Remove My Default" } else { "Set as My Default" }}
                                                            </button>
                                                        })}

                                                        // Set Workspace Default (mobile, admin only, dashboards only)
                                                        {(show_default_toggles && can_set_workspace_default).then(|| {
                                                            view! {
                                                                <div class="border-t border-border my-1" />
                                                                <button
                                                                    class="menu-item"
                                                                    disabled=move || toggle_ws_default_action.pending().get()
                                                                    on:click=toggle_ws_m
                                                                >
                                                                    <Icon icon=phosphor_leptos::HOUSE size="14px" />
                                                                    {move || if is_workspace_default.get() { "Remove Workspace Default" } else { "Set Workspace Default" }}
                                                                </button>
                                                            }
                                                        })}
                                                    </div>
                                                }
                                            })}
                                        </div>

                                        // Edit Dashboard — always visible (primary action)
                                        <ButtonLink href=edit_href size=ButtonSize::Sm>
                                            <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="14px" />
                                            <span class="hidden @6xl:inline whitespace-nowrap">{edit_label()}</span>
                                        </ButtonLink>
                                    </div>
                                </div>

                                // ─── Content area with optional History panel ───
                                <div class="flex-1 overflow-hidden flex">
                                    // Main content
                                    <div
                                        class="flex-1 overflow-y-auto p-4 md:p-6 bg-background"
                                        node_ref=content_scroll_ref
                                        on:scroll=move |_| {
                                            #[cfg(target_arch = "wasm32")]
                                            if let Some(element) = content_scroll_ref.get_untracked() {
                                                saved_scroll_top.set(element.scroll_top());
                                            }
                                        }
                                    >
                                        // Dashboard parameters (above content card)
                                        {has_params.then(|| {
                                            view! {
                                                <DashboardParameters
                                                    params=dashboard_params.clone()
                                                    values=Signal::derive(move || param_values.get())
                                                    set_values=set_param_values
                                                />
                                            }
                                        })}

                                        <div class="dashboard-content min-h-full w-full max-w-6xl mx-auto">
                                            // Preview banner
                                            {move || preview_content.get().is_some().then(|| {
                                                view! {
                                                    <div class="px-4 py-2 bg-warning border-b border-warning-border flex items-center justify-between">
                                                        <div class="flex items-center gap-2">
                                                            <span class="text-sm font-medium text-warning-foreground">
                                                                "Previewing historical version"
                                                            </span>
                                                        </div>
                                                        <span class="text-xs text-warning-foreground">"Read-only"</span>
                                                    </div>
                                                }
                                            })}

                                            <div class="p-4 md:p-6">
                                                {move || {
                                                    let content_str = display_content.get();
                                                    let is_previewing = preview_content.get().is_some();

                                                    if content_str.trim().is_empty() {
                                                        view! {
                                                            <div class="w-full text-center py-16">
                                                                <div class="w-24 h-24 mx-auto text-muted-foreground mb-6 flex items-center justify-center">
                                                                    <Icon icon=phosphor_leptos::FILE_TEXT size="64px" />
                                                                </div>
                                                                <h3 class="text-xl font-semibold text-foreground mb-2">
                                                                    {empty_title()}
                                                                </h3>
                                                                <p class="text-muted-foreground mb-6">
                                                                    {empty_hint()}
                                                                </p>
                                                                <ButtonLink href=edit_href_empty.clone()>
                                                                    <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="14px" />
                                                                    {edit_label()}
                                                                </ButtonLink>
                                                            </div>
                                                        }.into_any()
                                                    } else if params_initialized.get() || is_previewing {
                                                        // Clone captures up front so the surrounding reactive
                                                        // closure can stay `FnMut` even though the inner
                                                        // `ChildrenFn` body needs to be `Fn` (callable many
                                                        // times). Both `palette` and `ws_id` are cloned again
                                                        // inside the body so the body itself doesn't move
                                                        // anything out of its environment.
                                                        let palette = chart_palette.clone();
                                                        let ws_id = workspace_id.clone();
                                                        view! {
                                                            <div class="animate-fade-in">
                                                                <DashboardChartProviders workspace_id=ws_id.clone()>
                                                                    <MarkdownRenderer
                                                                        content=display_content
                                                                        parameters=Signal::derive(move || param_values.get())
                                                                        on_save_to_dashboard=on_save_to_dashboard
                                                                        on_chart_info=on_chart_info
                                                                        on_ask_about_chart=on_ask_about_chart
                                                                        chart_palette=palette.clone()
                                                                    />
                                                                </DashboardChartProviders>
                                                            </div>
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <div class="space-y-6 max-w-[860px]">
                                                                // Heading skeleton
                                                                <Skeleton class="h-8 w-2/5" />
                                                                // Description skeleton
                                                                <Skeleton class="h-4 w-3/5" />
                                                                // Chart area skeleton
                                                                <div class="border border-border rounded-md">
                                                                    <div class="px-5 py-4 border-b border-border flex items-center justify-between">
                                                                        <Skeleton class="h-4 w-1/4" />
                                                                        <Skeleton class="h-4 w-16" />
                                                                    </div>
                                                                    <div class="p-6">
                                                                        <Skeleton class="h-48 w-full" />
                                                                    </div>
                                                                </div>
                                                                // Second section heading
                                                                <Skeleton class="h-6 w-1/3" />
                                                                <Skeleton class="h-4 w-2/5" />
                                                            </div>
                                                        }.into_any()
                                                    }
                                                }}
                                            </div>
                                        </div>
                                    </div>

                                    // History panel (slides alongside content)
                                    <HistoryPanel
                                        dashboard_id=did_for_history.clone()
                                        open=Signal::derive(move || history_open.get())
                                        on_close=on_history_close
                                        on_preview=on_history_preview
                                        on_restore=on_history_restore
                                        on_ask_copilot=Callback::new(move |()| set_copilot_open.set(true))
                                        show_ask_copilot=Signal::derive(move || !is_knowledge.try_get().unwrap_or(false))
                                    />
                                </div>

                                // ─── Footer with metadata ───────────────
                                <div class="metadata-footer bg-background px-4 md:px-6 py-3 flex-shrink-0">
                                    <div class="flex items-center justify-between text-xs text-muted-foreground">
                                        <div class="flex items-center gap-4">
                                            <div class="flex items-center gap-1">
                                                <Icon icon=phosphor_leptos::CLOCK size="14px" />
                                                "Created " {created_date}
                                            </div>
                                            <div class="flex items-center gap-1">
                                                <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="14px" />
                                                "Last updated " {updated_date}
                                            </div>
                                        </div>
                                    </div>
                                </div>

                                // ─── Modals ─────────────────────────────
                                <SaveDashboardModal
                                    open=Signal::derive(move || save_modal_open.get())
                                    chart_yaml=save_modal_yaml
                                    on_close=on_save_modal_close
                                    on_saved=on_save_modal_saved
                                />
                                <ChartInfoModal
                                    open=Signal::derive(move || chart_info_open.get())
                                    yaml=chart_info_yaml
                                    on_close=on_chart_info_close
                                />
                            </div>
                        }.into_any()
                    }
                })
            }}
        </Transition>
        </div>
        <For
            each=move || if is_knowledge.get() { vec![] } else { vec![dashboard_id.get()] }
            key=|id| id.clone()
            children=move |id| view! { <CopilotSidebar
            dashboard_id=id
            dashboard_content=copilot_content
            open=Signal::derive(move || copilot_open.try_get().unwrap_or(false))
            on_close=Callback::new(move |()| set_copilot_open.set(false))
            before_send=no_pending_editor_save.clone()
            context_name="dashboard".to_string()
            view_context=copilot_view_context
            read_only=copilot_read_only
            historical_preview=Signal::derive(move || preview_content.try_get().flatten().is_some())
            access_error=copilot_access_error
            on_retry_access=retry_copilot_access
            on_saved_change=request_dashboard_refresh
            receipt_refresh=Signal::derive(move || receipt_sync_generation.try_get().unwrap_or(0))
            filter_summary=copilot_filter_summary
            refresh_error=Signal::derive(move || refresh_error.try_get().flatten())
            on_retry_refresh=request_dashboard_refresh
            on_open_history=Callback::new(move |()| {
                set_copilot_open.set(false);
                set_history_open.set(true);
            })
        /> }
        />
        </div>
    }
}
