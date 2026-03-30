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

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::dashboard::{
    ChartInfoModal, HistoryPanel, MarkdownRenderer, DashboardParameters, SaveDashboardModal,
};
use crate::components::Spinner;
#[cfg(target_arch = "wasm32")]
use crate::components::toast::toast_error;
use crate::parser::parse_markdown_chartml;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
use crate::server_fns::dashboards::{
    get_dashboard, get_user_default_dashboard, get_workspace_default_dashboard,
    set_user_default_dashboard, set_workspace_default_dashboard, update_dashboard,
};
use crate::server_fns::context::get_user_context;

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
                    let _ = el.select();
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
                            class="text-lg font-semibold text-foreground bg-transparent border-b-2 border-primary outline-none w-full"
                            prop:value=move || draft.get()
                            on:input=move |ev| set_draft.set(event_target_value(&ev))
                            on:blur=on_blur
                            on:keydown=on_keydown
                        />
                    }.into_any()
                } else {
                    view! {
                        <h1
                            class="text-lg font-semibold text-foreground truncate cursor-pointer hover:text-primary transition-colors"
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

// ─── Main component ─────────────────────────────────────────────────────────

/// Read-only dashboard viewer page.
///
/// Extracts `id` from the URL path params, fetches the dashboard detail
/// via `get_dashboard`, and renders the full viewer with toolbar, parameters,
/// content, history panel, modals, and footer.
#[component]
pub fn DashboardViewerPage() -> impl IntoView {
    let params = use_params_map();
    let dashboard_id = Memo::new(move |_| {
        params.get().get("id").unwrap_or_default()
    });

    // ── User context (roles, capabilities) ──────────────────────────────
    let user_ctx_resource = Resource::new(|| (), |_| get_user_context());

    // ── Fetch dashboard detail ──────────────────────────────────────────
    let dashboard_resource = Resource::new(
        move || dashboard_id.get(),
        get_dashboard,
    );

    // ── Default dashboard state ─────────────────────────────────────────
    let user_default_resource = Resource::new(|| (), |_| get_user_default_dashboard());
    let workspace_default_resource = Resource::new(|| (), |_| get_workspace_default_dashboard());

    // ── History panel state ─────────────────────────────────────────────
    let (history_open, set_history_open) = signal(false);

    // ── Preview state (from HistoryPanel on_preview) ────────────────────
    let (preview_content, set_preview_content) = signal(Option::<String>::None);

    // ── Parameter values ────────────────────────────────────────────────
    let (param_values, set_param_values) = signal(HashMap::<String, String>::new());
    let (params_initialized, set_params_initialized) = signal(false);

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

    // ── Title editing state (for optimistic update) ─────────────────────
    let (title_override, set_title_override) = signal(Option::<String>::None);

    // ── Set user default action ─────────────────────────────────────────
    let (setting_user_default, set_setting_user_default) = signal(false);
    let (setting_ws_default, set_setting_ws_default) = signal(false);

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

                let event_dashboard_id =
                    match data.get("dashboard_id").and_then(|v| v.as_str()) {
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
                        dashboard_resource.refetch();
                    }
                    "deleted" => {
                        navigate("/dashboards", leptos_router::NavigateOptions::default());
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
        <Transition fallback=move || view! {
            <div class="flex h-full items-center justify-center bg-muted">
                <Spinner class="h-8 w-8 text-muted-foreground" />
            </div>
        }>
            {move || {
                let dashboard_result = dashboard_resource.get();
                let user_ctx_result = user_ctx_resource.get();

                // Wait for both resources
                let (dashboard_result, user_ctx_result) = match (dashboard_result, user_ctx_result) {
                    (Some(d), Some(u)) => (d, u),
                    _ => return None,
                };

                // Get user context (gracefully handle errors)
                let user_ctx = user_ctx_result.ok();

                let is_admin = user_ctx.as_ref()
                    .map(|ctx| ctx.workspace_roles.contains(&"workspace_admin".to_string()))
                    .unwrap_or(false);

                let pdf_export_enabled = user_ctx.as_ref()
                    .and_then(|ctx| ctx.capabilities.get("pdf_export_enabled"))
                    .copied()
                    .unwrap_or(false);

                let chart_palette = user_ctx.as_ref()
                    .map(|ctx| ctx.chart_palette.clone())
                    .unwrap_or_else(|| "balanced".to_string());

                Some(match dashboard_result {
                    Err(e) => {
                        view! {
                            <div class="flex h-full items-center justify-center bg-muted">
                                <div class="text-center">
                                    <h2 class="text-lg font-semibold text-foreground mb-4">
                                        "Dashboard Not Found"
                                    </h2>
                                    <p class="text-muted-foreground mb-6">
                                        {e.to_string()}
                                    </p>
                                    <a
                                        href="/dashboards"
                                        class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90"
                                    >
                                        "Back to Dashboards"
                                    </a>
                                </div>
                            </div>
                        }.into_any()
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
                        let edit_href = format!("/dashboard/{}/edit", did);
                        let edit_href_empty = edit_href.clone();
                        let created_at = dashboard.created_at.clone();
                        let updated_at = dashboard.updated_at.clone();
                        let original_title = dashboard.title.clone();

                        // Initialize parameters from parsed content
                        {
                            let content_for_params = content.clone();
                            Effect::new(move |prev: Option<bool>| {
                                if prev.is_some() {
                                    return true;
                                }

                                let parsed = parse_markdown_chartml(&content_for_params);
                                let mut initial_values = HashMap::new();

                                // Process dashboard-level parameters
                                for group in &parsed.params {
                                    for param in &group.params {
                                        if initial_values.contains_key(&param.id) {
                                            continue;
                                        }
                                        if let Some(ref default) = param.default {
                                            let val = match default {
                                                serde_json::Value::String(s) => s.clone(),
                                                serde_json::Value::Number(n) => n.to_string(),
                                                serde_json::Value::Bool(b) => b.to_string(),
                                                other => other.to_string(),
                                            };
                                            initial_values.insert(param.id.clone(), val);
                                        }
                                    }
                                }

                                set_param_values.set(initial_values);
                                set_params_initialized.set(true);
                                true
                            });
                        }

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
                                let new_title_clone = new_title.clone();
                                set_title_override.set(Some(new_title.clone()));
                                leptos::task::spawn_local(async move {
                                    if let Err(e) = update_dashboard(
                                        did,
                                        Some(new_title_clone),
                                        None,
                                        None,
                                    ).await {
                                        // Revert on error
                                        set_title_override.set(None);
                                        leptos::logging::error!("Failed to update title: {}", e);
                                    }
                                });
                            }
                        });

                        // ── Refresh All handler ────────────────────────
                        let on_refresh_all = move |_: leptos::ev::MouseEvent| {
                            #[cfg(target_arch = "wasm32")]
                            if let Some(window) = web_sys::window() {
                                let event = web_sys::CustomEvent::new("dashboard-refresh-all").unwrap();
                                let _ = window.dispatch_event(&event);
                            }
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
                                let did = did_for_pdf.get_value();
                                let params = param_values.get();
                                let title = title_signal.get();

                                leptos::task::spawn_local(async move {
                                    let mut url = format!("/api/v1/dashboards/{}/export/pdf", did);
                                    if !params.is_empty() {
                                        if let Ok(json) = serde_json::to_string(&params) {
                                            let encoded = js_sys::encode_uri_component(&json);
                                            url = format!("{}?parameters={}", url, encoded);
                                        }
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
                                                    if let Ok(blob_promise) = resp.blob() {
                                                        if let Ok(blob) = wasm_bindgen_futures::JsFuture::from(blob_promise).await {
                                                            let blob: web_sys::Blob = blob.unchecked_into();
                                                            let blob_url = web_sys::Url::create_object_url_with_blob(&blob).unwrap_or_default();
                                                            let document = window.document().unwrap();
                                                            let a = document.create_element("a").unwrap();
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
                                                                        "Dashboard.pdf".to_string()
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
                                                            if let Ok(html_a) = a.dyn_into::<web_sys::HtmlElement>() {
                                                                if let Some(body) = document.body() {
                                                                    let _ = body.append_child(&html_a);
                                                                    let _ = html_a.click();
                                                                    let _ = body.remove_child(&html_a);
                                                                }
                                                            }
                                                            let _ = web_sys::Url::revoke_object_url(&blob_url);
                                                        }
                                                    }
                                                } else {
                                                    let status = resp.status();
                                                    let message = if status == 403 {
                                                        "PDF export requires a paid plan".to_string()
                                                    } else {
                                                        // Try to extract detail from JSON error body
                                                        let mut msg = format!("PDF export failed (HTTP {})", status);
                                                        if let Ok(text_promise) = resp.text() {
                                                            if let Ok(text_val) = wasm_bindgen_futures::JsFuture::from(text_promise).await {
                                                                if let Some(text) = text_val.as_string() {
                                                                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                                                                        if let Some(detail) = parsed.get("detail").or(parsed.get("message")).and_then(|v| v.as_str()) {
                                                                            msg = format!("PDF export failed: {}", detail);
                                                                        }
                                                                    }
                                                                }
                                                            }
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

                                    set_is_exporting.set(false);
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
                            dashboard_resource.refetch();
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
                            // Navigate to chat with chart context — matches React's handleAskAboutChart
                            let nav = leptos_router::hooks::use_navigate();
                            nav(
                                &format!("/chat?chart={}", js_sys::encode_uri_component(&chart_md)),
                                leptos_router::NavigateOptions::default(),
                            );
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
                                if setting_user_default.get() { return; }
                                let did = did.clone();
                                set_setting_user_default.set(true);
                                leptos::task::spawn_local(async move {
                                    let new_id = if is_user_default.get() {
                                        None
                                    } else {
                                        Some(did)
                                    };
                                    if let Err(e) = set_user_default_dashboard(new_id).await {
                                        leptos::logging::error!("Failed to set user default: {}", e);
                                    }
                                    user_default_resource.refetch();
                                    set_setting_user_default.set(false);
                                });
                            }
                        };
                        let toggle_user_default_mobile = {
                            let did = did_for_user_default_mobile.clone();
                            move |_: leptos::ev::MouseEvent| {
                                if setting_user_default.get() { return; }
                                let did = did.clone();
                                set_setting_user_default.set(true);
                                set_overflow_open.set(false);
                                leptos::task::spawn_local(async move {
                                    let new_id = if is_user_default.get() {
                                        None
                                    } else {
                                        Some(did)
                                    };
                                    if let Err(e) = set_user_default_dashboard(new_id).await {
                                        leptos::logging::error!("Failed to set user default: {}", e);
                                    }
                                    user_default_resource.refetch();
                                    set_setting_user_default.set(false);
                                });
                            }
                        };

                        let toggle_ws_default = {
                            let did = did_for_ws_default.clone();
                            move |_: leptos::ev::MouseEvent| {
                                if setting_ws_default.get() { return; }
                                let did = did.clone();
                                set_setting_ws_default.set(true);
                                leptos::task::spawn_local(async move {
                                    let new_id = if is_workspace_default.get() {
                                        None
                                    } else {
                                        Some(did)
                                    };
                                    if let Err(e) = set_workspace_default_dashboard(new_id).await {
                                        leptos::logging::error!("Failed to set workspace default: {}", e);
                                    }
                                    workspace_default_resource.refetch();
                                    set_setting_ws_default.set(false);
                                });
                            }
                        };
                        let toggle_ws_default_mobile = {
                            let did = did_for_ws_default_mobile.clone();
                            move |_: leptos::ev::MouseEvent| {
                                if setting_ws_default.get() { return; }
                                let did = did.clone();
                                set_setting_ws_default.set(true);
                                set_overflow_open.set(false);
                                leptos::task::spawn_local(async move {
                                    let new_id = if is_workspace_default.get() {
                                        None
                                    } else {
                                        Some(did)
                                    };
                                    if let Err(e) = set_workspace_default_dashboard(new_id).await {
                                        leptos::logging::error!("Failed to set workspace default: {}", e);
                                    }
                                    workspace_default_resource.refetch();
                                    set_setting_ws_default.set(false);
                                });
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
                            <div class="flex flex-col h-full bg-muted overflow-hidden" style:flex-direction="column">
                                // ─── Header / Toolbar ───────────────────
                                <div class="h-16 bg-card border-b border-border px-4 md:px-6 flex-shrink-0 flex items-center justify-between">
                                    // Left: back button + editable title
                                    <div class="flex items-center gap-4 flex-1 min-w-0 overflow-hidden">
                                        <a
                                            href="/dashboards"
                                            class="p-2 text-muted-foreground hover:text-foreground hover:bg-accent rounded-lg transition-colors flex-shrink-0"
                                            aria-label="Back to dashboards"
                                        >
                                            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path
                                                    stroke-linecap="round"
                                                    stroke-linejoin="round"
                                                    stroke-width="2"
                                                    d="M15 19l-7-7 7-7"
                                                />
                                            </svg>
                                        </a>

                                        <InlineEditableTitle
                                            value=title_signal
                                            on_save=on_title_save
                                        />
                                    </div>

                                    // Right: action buttons
                                    <div class="flex items-center gap-1 xl:gap-2 flex-shrink-0">
                                        // Refresh All
                                        <button
                                            class="flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium text-foreground bg-card border border-border hover:bg-accent rounded-lg transition-colors"
                                            aria-label="Refresh all charts"
                                            on:click=on_refresh_all
                                        >
                                            // ArrowPathIcon
                                            <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M16.023 9.348h4.992v-.001M2.985 19.644v-4.992m0 0h4.992m-4.993 0l3.181 3.183a8.25 8.25 0 0013.803-3.7M4.031 9.865a8.25 8.25 0 0113.803-3.7l3.181 3.182M2.985 19.644l3.181-3.182" />
                                            </svg>
                                            <span class="hidden xl:inline whitespace-nowrap">"Refresh All"</span>
                                        </button>

                                        // Download PDF — desktop only
                                        {pdf_export_enabled.then(|| {
                                            let on_download = on_download_pdf;
                                            view! {
                                                <button
                                                    class=move || format!(
                                                        "hidden md:flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium text-foreground bg-card border border-border hover:bg-accent rounded-lg transition-colors {}",
                                                        if is_exporting.get() { "opacity-50 cursor-not-allowed" } else { "" }
                                                    )
                                                    aria-label="Download PDF"
                                                    disabled=move || is_exporting.get()
                                                    on:click=on_download
                                                >
                                                    // ArrowDownTrayIcon
                                                    <svg class=move || format!("w-4 h-4 flex-shrink-0 {}", if is_exporting.get() { "animate-pulse" } else { "" }) fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 16.5v2.25A2.25 2.25 0 005.25 21h13.5A2.25 2.25 0 0021 18.75V16.5M16.5 12L12 16.5m0 0L7.5 12m4.5 4.5V3" />
                                                    </svg>
                                                    <span class="hidden xl:inline whitespace-nowrap">
                                                        {move || if is_exporting.get() { "Exporting..." } else { "Download PDF" }}
                                                    </span>
                                                </button>
                                            }
                                        })}

                                        // History — desktop only
                                        <button
                                            class=move || format!(
                                                "hidden md:flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium rounded-lg transition-colors {}",
                                                if history_open.get() {
                                                    "bg-primary/10 text-primary border border-primary/20"
                                                } else {
                                                    "text-foreground bg-card border border-border hover:bg-accent"
                                                }
                                            )
                                            aria-label="Toggle version history"
                                            on:click=move |_| set_history_open.update(|o| *o = !*o)
                                        >
                                            // ClockIcon
                                            <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
                                            </svg>
                                            <span class="hidden xl:inline whitespace-nowrap">"History"</span>
                                        </button>

                                        // Set as My Default — desktop only
                                        <button
                                            class=move || format!(
                                                "hidden md:flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium rounded-lg transition-colors {} {}",
                                                if is_user_default.get() {
                                                    "bg-primary/10 text-primary border border-primary/20"
                                                } else {
                                                    "text-foreground bg-card border border-border hover:bg-accent"
                                                },
                                                if setting_user_default.get() { "opacity-50 cursor-not-allowed" } else { "" }
                                            )
                                            aria-label=move || if is_user_default.get() { "Remove as my default" } else { "Set as my default" }
                                            disabled=move || setting_user_default.get()
                                            on:click=toggle_user_default
                                        >
                                            // Star icon
                                            <svg
                                                class="w-4 h-4 flex-shrink-0"
                                                fill=move || if is_user_default.get() { "currentColor" } else { "none" }
                                                stroke="currentColor"
                                                viewBox="0 0 24 24"
                                            >
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                                            </svg>
                                            <span class="hidden xl:inline whitespace-nowrap">
                                                {move || if is_user_default.get() { "My Default" } else { "Set as My Default" }}
                                            </span>
                                        </button>

                                        // Set Workspace Default — desktop only, admin only
                                        {is_admin.then(|| {
                                            view! {
                                                <button
                                                    class=move || format!(
                                                        "hidden md:flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium rounded-lg transition-colors {} {}",
                                                        if is_workspace_default.get() {
                                                            "bg-primary/10 text-primary border border-primary/20"
                                                        } else {
                                                            "text-foreground bg-card border border-border hover:bg-accent"
                                                        },
                                                        if setting_ws_default.get() { "opacity-50 cursor-not-allowed" } else { "" }
                                                    )
                                                    aria-label=move || if is_workspace_default.get() { "Remove as workspace default" } else { "Set as workspace default" }
                                                    disabled=move || setting_ws_default.get()
                                                    on:click=toggle_ws_default
                                                >
                                                    // Home icon
                                                    <svg
                                                        class="w-4 h-4 flex-shrink-0"
                                                        fill=move || if is_workspace_default.get() { "currentColor" } else { "none" }
                                                        stroke="currentColor"
                                                        viewBox="0 0 24 24"
                                                    >
                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6" />
                                                    </svg>
                                                    <span class="hidden xl:inline whitespace-nowrap">
                                                        {move || if is_workspace_default.get() { "Workspace Default" } else { "Set Workspace Default" }}
                                                    </span>
                                                </button>
                                            }
                                        })}

                                        // ─── Mobile overflow menu ───────
                                        <div class="relative flex md:hidden">
                                            <button
                                                class="flex items-center justify-center p-2 text-foreground bg-card border border-border hover:bg-accent rounded-lg transition-colors"
                                                aria-label="More actions"
                                                on:click=move |_| set_overflow_open.update(|o| *o = !*o)
                                            >
                                                // EllipsisVerticalIcon
                                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 6.75a.75.75 0 110-1.5.75.75 0 010 1.5zM12 12.75a.75.75 0 110-1.5.75.75 0 010 1.5zM12 18.75a.75.75 0 110-1.5.75.75 0 010 1.5z" />
                                                </svg>
                                            </button>

                                            {move || overflow_open.get().then(|| {
                                                let on_download_pdf_m = on_download_pdf_mobile;
                                                let toggle_user_m = toggle_user_default_mobile.clone();
                                                let toggle_ws_m = toggle_ws_default_mobile.clone();

                                                view! {
                                                    <div class="absolute right-0 top-full mt-1 w-56 bg-popover border border-border rounded-lg shadow-lg z-50">
                                                        // Download PDF (mobile)
                                                        {pdf_export_enabled.then(|| {
                                                            view! {
                                                                <button
                                                                    class="flex items-center w-full px-3 py-2 text-sm text-foreground hover:bg-accent transition-colors"
                                                                    disabled=move || is_exporting.get()
                                                                    on:click=move |ev| {
                                                                        set_overflow_open.set(false);
                                                                        on_download_pdf_m(ev);
                                                                    }
                                                                >
                                                                    <svg class="w-4 h-4 mr-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 16.5v2.25A2.25 2.25 0 005.25 21h13.5A2.25 2.25 0 0021 18.75V16.5M16.5 12L12 16.5m0 0L7.5 12m4.5 4.5V3" />
                                                                    </svg>
                                                                    {move || if is_exporting.get() { "Exporting..." } else { "Download PDF" }}
                                                                </button>
                                                            }
                                                        })}

                                                        // History (mobile)
                                                        <button
                                                            class="flex items-center w-full px-3 py-2 text-sm text-foreground hover:bg-accent transition-colors"
                                                            on:click=move |_| {
                                                                set_overflow_open.set(false);
                                                                set_history_open.update(|o| *o = !*o);
                                                            }
                                                        >
                                                            <svg class="w-4 h-4 mr-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
                                                            </svg>
                                                            {move || if history_open.get() { "Close History" } else { "Version History" }}
                                                        </button>

                                                        <div class="border-t border-border my-1" />

                                                        // Set as My Default (mobile)
                                                        <button
                                                            class="flex items-center w-full px-3 py-2 text-sm text-foreground hover:bg-accent transition-colors"
                                                            disabled=move || setting_user_default.get()
                                                            on:click=toggle_user_m
                                                        >
                                                            <svg
                                                                class="w-4 h-4 mr-2"
                                                                fill=move || if is_user_default.get() { "currentColor" } else { "none" }
                                                                stroke="currentColor"
                                                                viewBox="0 0 24 24"
                                                            >
                                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                                                            </svg>
                                                            {move || if is_user_default.get() { "Remove My Default" } else { "Set as My Default" }}
                                                        </button>

                                                        // Set Workspace Default (mobile, admin only)
                                                        {is_admin.then(|| {
                                                            view! {
                                                                <div class="border-t border-border my-1" />
                                                                <button
                                                                    class="flex items-center w-full px-3 py-2 text-sm text-foreground hover:bg-accent transition-colors"
                                                                    disabled=move || setting_ws_default.get()
                                                                    on:click=toggle_ws_m
                                                                >
                                                                    <svg
                                                                        class="w-4 h-4 mr-2"
                                                                        fill=move || if is_workspace_default.get() { "currentColor" } else { "none" }
                                                                        stroke="currentColor"
                                                                        viewBox="0 0 24 24"
                                                                    >
                                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6" />
                                                                    </svg>
                                                                    {move || if is_workspace_default.get() { "Remove Workspace Default" } else { "Set Workspace Default" }}
                                                                </button>
                                                            }
                                                        })}
                                                    </div>
                                                }
                                            })}
                                        </div>

                                        // Edit Dashboard — always visible (primary action)
                                        <a
                                            href=edit_href
                                            class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90"
                                        >
                                            <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                                            </svg>
                                            <span class="hidden xl:inline whitespace-nowrap">"Edit Dashboard"</span>
                                        </a>
                                    </div>
                                </div>

                                // ─── Content area with optional History panel ───
                                <div class="flex-1 overflow-hidden flex">
                                    // Main content
                                    <div class="flex-1 overflow-y-auto p-4 md:p-6 bg-muted">
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

                                        <div class="bg-card rounded-lg border border-border shadow min-h-full">
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
                                                                <svg class="w-24 h-24 mx-auto text-muted-foreground mb-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                    <path
                                                                        stroke-linecap="round"
                                                                        stroke-linejoin="round"
                                                                        stroke-width="1.5"
                                                                        d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
                                                                    />
                                                                </svg>
                                                                <h3 class="text-xl font-semibold text-foreground mb-2">
                                                                    "This dashboard is empty"
                                                                </h3>
                                                                <p class="text-muted-foreground mb-6">
                                                                    "Click \"Edit Dashboard\" to add content and charts"
                                                                </p>
                                                                <a
                                                                    href=edit_href_empty.clone()
                                                                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90"
                                                                >
                                                                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                                                                    </svg>
                                                                    "Edit Dashboard"
                                                                </a>
                                                            </div>
                                                        }.into_any()
                                                    } else if params_initialized.get() || is_previewing {
                                                        view! {
                                                            <MarkdownRenderer
                                                                content=display_content
                                                                parameters=Signal::derive(move || param_values.get())
                                                                on_save_to_dashboard=on_save_to_dashboard
                                                                on_chart_info=on_chart_info
                                                                on_ask_about_chart=on_ask_about_chart
                                                                chart_palette=chart_palette.clone()
                                                            />
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <div class="flex h-64 items-center justify-center">
                                                                <Spinner class="h-8 w-8 text-muted-foreground" />
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
                                    />
                                </div>

                                // ─── Footer with metadata ───────────────
                                <div class="bg-card border-t border-border px-4 md:px-6 py-3 flex-shrink-0">
                                    <div class="flex items-center justify-between text-xs text-muted-foreground">
                                        <div class="flex items-center gap-4">
                                            <div class="flex items-center gap-1">
                                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
                                                </svg>
                                                "Created " {created_date}
                                            </div>
                                            <div class="flex items-center gap-1">
                                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                                                </svg>
                                                "Last updated " {updated_date}
                                            </div>
                                        </div>
                                    </div>
                                </div>

                                // ─── Modals ─────────────────────────────
                                <SaveDashboardModal
                                    open=Signal::derive(move || save_modal_open.get())
                                    chart_yaml=save_modal_yaml.get_untracked()
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
    }
}
