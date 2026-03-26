// SPDX-License-Identifier: AGPL-3.0-or-later

//! Analytics settings page — manage analytics sites and view event usage.
//!
//! Replaces `apps/frontend/src/components/settings/AnalyticsSettings.jsx` (438 lines).
//!
//! Features:
//! - Analytics usage overview (events this month / limit)
//! - Sites list with inline create/edit form
//! - Each site shows: name, domain badges, tracking snippet with copy button, datasource link
//! - Create site form: name + domains + datasource slug inputs
//! - Edit site: inline form replacing the display
//! - Delete site with confirmation
//! - Auto-generates datasource slug from site name

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, ConfirmDialog, Label, Skeleton, INPUT_CLASS,
};
use crate::server_fns::analytics::*;
use crate::server_fns::context::UserContext;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Generate a datasource slug from a site name.
///
/// Matches the React `generateSlug` function exactly.
fn generate_slug(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse multiple dashes and trim leading/trailing dashes
    let trimmed = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!("{trimmed}-analytics")
}

// ─────────────────────────────────────────────────────────────────────────────
// Main Page
// ─────────────────────────────────────────────────────────────────────────────

/// Analytics settings page content.
#[component]
pub fn AnalyticsPage() -> impl IntoView {
    // Read the UserContext provided by SettingsShell to check deployment mode.
    let user_ctx = expect_context::<Resource<Result<UserContext, ServerFnError>>>();

    let sites_resource = Resource::new(|| (), |_| list_analytics_sites());
    let usage_resource = Resource::new(|| (), |_| get_analytics_usage());

    view! {
        <div class="p-6">
            <h2 class="text-xl font-semibold text-foreground mb-6">"Analytics"</h2>

            // If the user context indicates self-hosted mode, show an informational
            // message instead of making ClickHouse-backed server calls.
            {move || {
                if let Some(Ok(ctx)) = user_ctx.get() {
                    if ctx.is_self_hosted {
                        return view! {
                            <Card>
                                <CardContent>
                                    <p class="text-muted-foreground py-6">
                                        "Analytics requires a Postgres and ClickHouse configuration. \
                                         Not available in self-hosted mode with SQLite."
                                    </p>
                                </CardContent>
                            </Card>
                        }.into_any();
                    }
                }
                view! {
                    <Transition fallback=move || view! { <AnalyticsLoadingSkeleton/> }>
                        {move || Suspend::new(async move {
                            match sites_resource.await {
                                Ok(sites) => {
                                    let usage = usage_resource.await.ok();
                                    view! {
                                        <AnalyticsContent
                                            initial_sites=sites
                                            usage=usage
                                            sites_resource=sites_resource
                                        />
                                    }.into_any()
                                }
                                Err(_) => {
                                    // Match React: show the page layout with empty state even
                                    // when the backend errors (e.g. no ClickHouse configured).
                                    // React shows the error as a toast but keeps the UI visible.
                                    view! {
                                        <AnalyticsContent
                                            initial_sites=vec![]
                                            usage=None
                                            sites_resource=sites_resource
                                        />
                                    }.into_any()
                                }
                            }
                        })}
                    </Transition>
                }.into_any()
            }}
        </div>
    }
}

/// Loading skeleton shown while data is being fetched.
#[component]
fn AnalyticsLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="space-y-6" style:display="block">
            <div class="flex items-center justify-between">
                <div>
                    <Skeleton class="h-5 w-32"/>
                    <Skeleton class="h-4 w-64 mt-1"/>
                </div>
                <Skeleton class="h-9 w-24"/>
            </div>
            <Card>
                <CardContent>
                    <div class="space-y-2 pt-6">
                        <Skeleton class="h-4 w-full"/>
                        <Skeleton class="h-2 w-full"/>
                    </div>
                </CardContent>
            </Card>
            <Card>
                <CardHeader>
                    <Skeleton class="h-5 w-48"/>
                    <Skeleton class="h-4 w-32 mt-1"/>
                </CardHeader>
                <CardContent>
                    <Skeleton class="h-20 w-full"/>
                </CardContent>
            </Card>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Content
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn AnalyticsContent(
    initial_sites: Vec<AnalyticsSiteData>,
    usage: Option<AnalyticsUsageData>,
    sites_resource: Resource<Result<Vec<AnalyticsSiteData>, ServerFnError>>,
) -> impl IntoView {
    // Reactive state
    let (sites, set_sites) = signal(initial_sites);
    let (show_form, set_show_form) = signal(false);
    let (editing_site_id, set_editing_site_id) = signal(Option::<String>::None);
    let (saving, set_saving) = signal(false);

    // Form fields
    let (form_name, set_form_name) = signal(String::new());
    let (form_domains, set_form_domains) = signal(String::new());
    let (form_datasource_slug, set_form_datasource_slug) = signal(String::new());
    let (datasource_slug_edited, set_datasource_slug_edited) = signal(false);

    // Confirm dialog state
    let (dialog_open, set_dialog_open) = signal(false);
    let (delete_target, set_delete_target) = signal(Option::<(String, String)>::None);

    let reset_form = move || {
        set_form_name.set(String::new());
        set_form_domains.set(String::new());
        set_form_datasource_slug.set(String::new());
        set_datasource_slug_edited.set(false);
        set_show_form.set(false);
        set_editing_site_id.set(None);
    };

    // Create site action
    let create_action = Action::new(move |(name, domains, slug): &(String, String, Option<String>)| {
        let name = name.clone();
        let domains = domains.clone();
        let slug = slug.clone();
        async move { create_analytics_site(name, domains, slug).await }
    });

    // Update site action
    let update_action = Action::new(
        move |(id, name, domains, slug): &(String, String, String, Option<String>)| {
            let id = id.clone();
            let name = name.clone();
            let domains = domains.clone();
            let slug = slug.clone();
            async move { update_analytics_site(id, name, domains, slug).await }
        },
    );

    // Delete site action
    let delete_action = Action::new(move |id: &String| {
        let id = id.clone();
        async move { delete_analytics_site(id).await }
    });

    // Effect: handle create result
    Effect::new(move || {
        if let Some(result) = create_action.value().get() {
            set_saving.set(false);
            match result {
                Ok(_) => {
                    reset_form();
                    sites_resource.refetch();
                }
                Err(_) => {} // Error is shown via toast-like mechanism
            }
        }
    });

    // Effect: update sites list when resource refetches
    Effect::new(move || {
        if let Some(Ok(new_sites)) = sites_resource.get() {
            set_sites.set(new_sites);
        }
    });

    // Effect: handle update result
    Effect::new(move || {
        if let Some(result) = update_action.value().get() {
            set_saving.set(false);
            match result {
                Ok(_) => {
                    reset_form();
                    sites_resource.refetch();
                }
                Err(_) => {}
            }
        }
    });

    // Effect: handle delete result
    Effect::new(move || {
        if let Some(result) = delete_action.value().get() {
            if result.is_ok() {
                sites_resource.refetch();
            }
        }
    });

    // Handlers
    let handle_create = move |_| {
        let name = form_name.get();
        let domains = form_domains.get();
        if name.trim().is_empty() {
            return;
        }
        if domains.split(',').map(|d| d.trim()).filter(|d| !d.is_empty()).count() == 0 {
            return;
        }
        set_saving.set(true);
        let slug = {
            let s = form_datasource_slug.get();
            if s.is_empty() { None } else { Some(s) }
        };
        create_action.dispatch((name, domains, slug));
    };

    let handle_update = move |_| {
        let Some(site_id) = editing_site_id.get() else {
            return;
        };
        let name = form_name.get();
        let domains = form_domains.get();
        if name.trim().is_empty() {
            return;
        }
        if domains.split(',').map(|d| d.trim()).filter(|d| !d.is_empty()).count() == 0 {
            return;
        }
        set_saving.set(true);
        let slug = if datasource_slug_edited.get() {
            let s = form_datasource_slug.get();
            if s.is_empty() { None } else { Some(s) }
        } else {
            None
        };
        update_action.dispatch((site_id, name, domains, slug));
    };

    let start_editing = move |site: AnalyticsSiteData| {
        set_editing_site_id.set(Some(site.id.clone()));
        set_form_name.set(site.name.clone());
        set_form_domains.set(site.allowed_domains.join(", "));
        set_form_datasource_slug.set(site.datasource_slug.clone().unwrap_or_default());
        set_datasource_slug_edited.set(true);
        set_show_form.set(false);
    };

    let request_delete = move |id: String, name: String| {
        set_delete_target.set(Some((id, name)));
        set_dialog_open.set(true);
    };

    let on_confirm_delete = Callback::new(move |()| {
        set_dialog_open.set(false);
        if let Some((id, _)) = delete_target.get() {
            delete_action.dispatch(id);
        }
    });

    let on_cancel_delete = Callback::new(move |()| {
        set_dialog_open.set(false);
        set_delete_target.set(None);
    });

    let handle_cancel_form = move |_| {
        reset_form();
    };

    let handle_show_form = move |_| {
        set_show_form.set(true);
        set_editing_site_id.set(None);
        set_form_name.set(String::new());
        set_form_domains.set(String::new());
        set_form_datasource_slug.set(String::new());
        set_datasource_slug_edited.set(false);
    };

    view! {
        <div class="space-y-6">
            // Header with Add button
            <div class="flex items-center justify-between">
                <div>
                    <h3 class="text-lg font-medium text-foreground">"Analytics Sites"</h3>
                    <p class="text-sm text-muted-foreground">
                        "Install analytics on your websites to track visitor data."
                    </p>
                </div>
                <Show when=move || !show_form.get() && editing_site_id.get().is_none()>
                    <Button on:click=handle_show_form>
                        <span class="flex items-center gap-2">
                            <Icon icon=icondata_lu::LuPlus width="16" height="16"/>
                            "Add Site"
                        </span>
                    </Button>
                </Show>
            </div>

            // Event usage bar
            {usage.map(|u| {
                if u.events_limit > 0 {
                    view! { <UsageBar usage=u/> }.into_any()
                } else {
                    view! { <span class="hidden"></span> }.into_any()
                }
            })}

            // Error messages from actions
            {move || create_action.value().get().and_then(|r| r.err()).map(|e| {
                view! {
                    <div class="rounded-lg border border-destructive/50 bg-destructive/10 p-3 text-sm text-error-foreground">
                        {format!("Failed to create site: {e}")}
                    </div>
                }
            })}
            {move || update_action.value().get().and_then(|r| r.err()).map(|e| {
                view! {
                    <div class="rounded-lg border border-destructive/50 bg-destructive/10 p-3 text-sm text-error-foreground">
                        {format!("Failed to update site: {e}")}
                    </div>
                }
            })}
            {move || delete_action.value().get().and_then(|r| r.err()).map(|e| {
                view! {
                    <div class="rounded-lg border border-destructive/50 bg-destructive/10 p-3 text-sm text-error-foreground">
                        {format!("Failed to delete site: {e}")}
                    </div>
                }
            })}

            // Inline create form
            <Show when=move || show_form.get() && editing_site_id.get().is_none()>
                <SiteForm
                    title="New Analytics Site"
                    description="Add a new site to start tracking analytics."
                    form_name=form_name
                    set_form_name=set_form_name
                    form_domains=form_domains
                    set_form_domains=set_form_domains
                    form_datasource_slug=form_datasource_slug
                    set_form_datasource_slug=set_form_datasource_slug
                    datasource_slug_edited=datasource_slug_edited
                    set_datasource_slug_edited=set_datasource_slug_edited
                    is_editing=false
                    saving=saving
                    on_submit=handle_create
                    on_cancel=handle_cancel_form
                />
            </Show>

            // Inline edit form
            <Show when=move || editing_site_id.get().is_some()>
                <SiteForm
                    title="Edit Site"
                    description="Update the site name or allowed domains."
                    form_name=form_name
                    set_form_name=set_form_name
                    form_domains=form_domains
                    set_form_domains=set_form_domains
                    form_datasource_slug=form_datasource_slug
                    set_form_datasource_slug=set_form_datasource_slug
                    datasource_slug_edited=datasource_slug_edited
                    set_datasource_slug_edited=set_datasource_slug_edited
                    is_editing=true
                    saving=saving
                    on_submit=handle_update
                    on_cancel=handle_cancel_form
                />
            </Show>

            // Site list
            <For
                each=move || {
                    let editing_id = editing_site_id.get();
                    sites.get().into_iter().filter(move |s| {
                        editing_id.as_ref().map_or(true, |eid| eid != &s.id)
                    }).collect::<Vec<_>>()
                }
                key=|site| site.id.clone()
                let:site
            >
                {
                    let site_for_edit = site.clone();
                    let site_for_delete_id = site.id.clone();
                    let site_for_delete_name = site.name.clone();
                    view! {
                        <SiteCard
                            site=site
                            on_edit=move |_| start_editing(site_for_edit.clone())
                            on_delete=move |_| request_delete(site_for_delete_id.clone(), site_for_delete_name.clone())
                        />
                    }
                }
            </For>

            // Empty state
            <Show when=move || sites.get().is_empty() && !show_form.get() && editing_site_id.get().is_none()>
                <Card>
                    <CardContent>
                        <div class="py-12 text-center">
                            <span class="flex justify-center mb-3 text-muted-foreground">
                                <Icon icon=icondata_lu::LuGlobe width="40" height="40"/>
                            </span>
                            <p class="text-muted-foreground">
                                "No analytics sites yet. Add one to start tracking visitor data."
                            </p>
                        </div>
                    </CardContent>
                </Card>
            </Show>
        </div>

        // Confirm Dialog
        <ConfirmDialog
            open=Signal::from(dialog_open)
            title="Delete Analytics Site?"
            message=delete_target.get_untracked().map_or(String::new(), |(_, name)| format!("Delete \"{name}\"? This cannot be undone."))
            confirm_text="Delete"
            on_confirm=on_confirm_delete
            on_cancel=on_cancel_delete
        />
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Usage Bar
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn UsageBar(usage: AnalyticsUsageData) -> impl IntoView {
    let bar_class = match usage.status.as_str() {
        "blocked" | "exceeded" => "h-2 rounded-full transition-all bg-error-foreground",
        "warning" => "h-2 rounded-full transition-all bg-warning-foreground",
        _ => "h-2 rounded-full transition-all bg-success-foreground",
    };
    let bar_width = format!("{}%", usage.usage_percent.min(100.0));

    view! {
        <Card>
            <CardContent>
                <div class="pt-6">
                    <div class="flex justify-between mb-2">
                        <span class="text-sm font-medium text-foreground">
                            "Event Usage This Month"
                        </span>
                        <span class="text-sm font-medium text-foreground">
                            {format!(
                                "{} / {} ({:.1}%)",
                                format_number(usage.events_used),
                                format_number(usage.events_limit),
                                usage.usage_percent,
                            )}
                        </span>
                    </div>
                    <div class="w-full bg-muted rounded-full h-2">
                        <div
                            class=bar_class
                            style:width=bar_width
                        />
                    </div>
                    {(usage.status == "blocked").then(|| view! {
                        <p class="text-sm text-error-foreground mt-2">
                            "Event quota exceeded. Analytics events are being dropped."
                        </p>
                    })}
                    {(usage.status == "exceeded").then(|| view! {
                        <p class="text-sm text-error-foreground mt-2">
                            "Event quota reached. Events are still accepted during the grace period."
                        </p>
                    })}
                </div>
            </CardContent>
        </Card>
    }
}

/// Format a number with commas (e.g. 1234567 -> "1,234,567").
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Site Form (create / edit)
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SiteForm(
    title: &'static str,
    description: &'static str,
    form_name: ReadSignal<String>,
    set_form_name: WriteSignal<String>,
    form_domains: ReadSignal<String>,
    set_form_domains: WriteSignal<String>,
    form_datasource_slug: ReadSignal<String>,
    set_form_datasource_slug: WriteSignal<String>,
    datasource_slug_edited: ReadSignal<bool>,
    set_datasource_slug_edited: WriteSignal<bool>,
    is_editing: bool,
    saving: ReadSignal<bool>,
    on_submit: impl Fn(web_sys::MouseEvent) + Send + 'static,
    on_cancel: impl Fn(web_sys::MouseEvent) + Send + 'static,
) -> impl IntoView {
    let slug_hint = if is_editing {
        "Rename the datasource slug. Existing queries and dashboards using the old slug will break."
    } else {
        "This creates a queryable datasource in your workspace. Use this slug to reference it in queries and dashboards."
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle>{title}</CardTitle>
                <CardDescription>{description}</CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    <div class="space-y-2">
                        <Label>"Site Name"</Label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            placeholder="e.g. My Website"
                            prop:value=move || form_name.get()
                            on:input=move |ev| {
                                let name = event_target_value(&ev);
                                set_form_name.set(name.clone());
                                if !is_editing && !datasource_slug_edited.get() {
                                    set_form_datasource_slug.set(generate_slug(&name));
                                }
                            }
                        />
                    </div>
                    <div class="space-y-2">
                        <Label>"Allowed Domains"</Label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            placeholder="e.g. example.com, app.example.com"
                            prop:value=move || form_domains.get()
                            on:input=move |ev| set_form_domains.set(event_target_value(&ev))
                        />
                        <p class="text-xs text-muted-foreground">
                            "Comma-separated list of domains that are allowed to send analytics events."
                        </p>
                    </div>
                    <div class="space-y-2">
                        <Label>"Datasource Slug"</Label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            placeholder="e.g. my-website-analytics"
                            prop:value=move || form_datasource_slug.get()
                            on:input=move |ev| {
                                set_form_datasource_slug.set(event_target_value(&ev));
                                set_datasource_slug_edited.set(true);
                            }
                        />
                        <p class="text-xs text-muted-foreground">
                            {slug_hint}
                        </p>
                    </div>
                    <div class="flex items-center gap-2 pt-2">
                        <Button disabled=saving.get_untracked() on:click=on_submit>
                            {if is_editing { "Save Changes" } else { "Create Site" }}
                        </Button>
                        <Button variant=ButtonVariant::Outline on:click=on_cancel>
                            "Cancel"
                        </Button>
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Site Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SiteCard(
    site: AnalyticsSiteData,
    on_edit: impl Fn(web_sys::MouseEvent) + Send + 'static,
    on_delete: impl Fn(web_sys::MouseEvent) + Send + 'static,
) -> impl IntoView {
    let snippet = site.snippet.clone();
    let snippet_for_copy = site.snippet.clone();
    let created_date = format_date(&site.created_at);
    let site_name = site.name.clone();
    let domains = site.allowed_domains.clone();
    let datasource_slug = site.datasource_slug.clone();

    let domain_badges = domains
        .into_iter()
        .map(|domain| {
            view! {
                <Badge variant=BadgeVariant::Secondary>
                    {domain}
                </Badge>
            }
        })
        .collect_view();

    let datasource_label = datasource_slug.map(|slug| {
        view! {
            <p class="text-xs text-muted-foreground flex items-center gap-1">
                <span class="inline-flex">
                    <Icon icon=icondata_lu::LuDatabase width="12" height="12"/>
                </span>
                "Datasource: "
                <span class="font-mono">{slug}</span>
            </p>
        }
    });

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div class="space-y-1">
                        <CardTitle>
                            <span class="flex items-center gap-2">
                                <span class="text-muted-foreground">
                                    <Icon icon=icondata_lu::LuGlobe width="16" height="16"/>
                                </span>
                                {site_name}
                            </span>
                        </CardTitle>
                        <div class="flex flex-wrap items-center gap-1.5">
                            {domain_badges}
                        </div>
                        {datasource_label}
                    </div>
                    <div class="flex items-center gap-1">
                        <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm on:click=on_edit>
                            <span class="text-muted-foreground">
                                <Icon icon=icondata_lu::LuPencil width="16" height="16"/>
                            </span>
                        </Button>
                        <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm on:click=on_delete>
                            <span class="text-muted-foreground">
                                <Icon icon=icondata_lu::LuTrash2 width="16" height="16"/>
                            </span>
                        </Button>
                    </div>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-3">
                    <div class="flex items-center gap-2 text-sm text-muted-foreground">
                        <span class="inline-flex">
                            <Icon icon=icondata_lu::LuCode width="16" height="16"/>
                        </span>
                        <span>"Tracking snippet"</span>
                    </div>
                    <div class="relative">
                        <pre class="bg-muted rounded-lg p-3 text-sm font-mono overflow-x-auto pr-12 text-foreground">
                            {snippet}
                        </pre>
                        <CopyButton text=snippet_for_copy/>
                    </div>
                    <p class="text-xs text-muted-foreground">
                        {format!("Created {created_date}")}
                    </p>
                </div>
            </CardContent>
        </Card>
    }
}

/// Format an RFC3339 date string to a locale-style display format.
fn format_date(iso_str: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso_str) {
        dt.format("%b %-d, %Y").to_string()
    } else {
        iso_str.to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Copy Button (reuses the pattern from profile.rs McpConnectionCard)
// ─────────────────────────────────────────────────────────────────────────────

/// Small copy-to-clipboard button positioned absolutely within a relative parent.
#[component]
fn CopyButton(text: String) -> impl IntoView {
    let (copied, set_copied) = signal(false);
    let text = text.clone();

    let on_click = move |_| {
        let text = text.clone();
        let set_copied = set_copied;

        #[cfg(target_arch = "wasm32")]
        {
            leptos::task::spawn_local(async move {
                if let Some(window) = web_sys::window() {
                    let clipboard = window.navigator().clipboard();
                    let promise = clipboard.write_text(&text);
                    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                    set_copied.set(true);
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    set_copied.set(false);
                }
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (text, set_copied);
        }
    };

    view! {
        <button
            class="absolute top-2 right-2 p-1.5 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
            on:click=on_click
            title="Copy to clipboard"
        >
            {move || {
                if copied.get() {
                    view! { <Icon icon=icondata_lu::LuCopyCheck width="16" height="16"/> }.into_any()
                } else {
                    view! { <Icon icon=icondata_lu::LuCopy width="16" height="16"/> }.into_any()
                }
            }}
        </button>
    }
}
