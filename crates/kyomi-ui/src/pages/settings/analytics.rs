// SPDX-License-Identifier: AGPL-3.0-or-later

//! Analytics settings page — manage analytics sites and view event usage.
//!
//! Replaces `apps/frontend/src/components/settings/AnalyticsSettings.jsx` (438 lines).
//!
//! Features:
//! - Analytics usage overview: monthly event quota progress bar (with the
//!   raw percentage shown even past 100%, since a bar can't visually exceed
//!   full), non-expiring bundle reserve balance, and a status banner for
//!   over-quota states (drawing from bundle reserve / quota exceeded with
//!   grace still active / quota and reserve both exhausted). Not shown at
//!   all for self-hosted deployments — there is no quota to report there,
//!   and the page's existing `analytics_access()` gate already prevents
//!   `AnalyticsContent` (and this card) from rendering in that case, so the
//!   card never has to special-case it. A failed fetch renders an explicit
//!   error, never a zeroed-out card.
//! - Sites list with inline create/edit form
//! - Each site shows: name, domain badges, tracking snippet with copy button, datasource link
//! - Create site form: name + domains + datasource slug inputs
//! - Edit site: inline form replacing the display
//! - Delete site with confirmation
//! - Auto-generates datasource slug from site name

use std::sync::Arc;

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::{
    Alert, AlertDescription, AlertVariant, Badge, BadgeVariant, Button, ButtonSize,
    ButtonVariant, Card, CardContent, CardDescription, CardHeader, CardTitle, ConfirmDialog,
    Label, Skeleton, INPUT_CLASS,
};
use crate::server_fns::analytics::*;
use crate::server_fns::context::UserContext;
use crate::utils::permissions::{analytics_access, AnalyticsAccess};

use super::billing::format_number;
use super::usage::{BundleRow, UsageCardShell};

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
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

    let sites_resource = Resource::new(|| (), |_| list_analytics_sites());
    let usage_resource = Resource::new(|| (), |_| get_analytics_usage());

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-6">"Analytics"</h2>
            <Transition fallback=move || view! { <AnalyticsLoadingSkeleton/> }>
                {move || Suspend::new(async move {
                    // Single guard for this page (KYO-260) — matches the same
                    // analytics_access precedence the Settings tab bar and the
                    // datasources-page "Analytics Settings" link consume. An
                    // `Err` from user_ctx fails closed to Denied — it must never
                    // fall through to the sites content below.
                    let access = match user_ctx.await {
                        Ok(ctx) => analytics_access(&ctx),
                        Err(_) => AnalyticsAccess::Denied,
                    };

                    match access {
                        AnalyticsAccess::SelfHosted => {
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
                        AnalyticsAccess::Denied => {
                            return view! {
                                <Card>
                                    <CardContent>
                                        <div class="py-6 text-center space-y-2">
                                            <p class="text-foreground font-medium">"Analytics is admin-only"</p>
                                            <p class="text-muted-foreground text-sm">
                                                "Only workspace admins can manage analytics sites. \
                                                 Contact a workspace admin if you need access."
                                            </p>
                                        </div>
                                    </CardContent>
                                </Card>
                            }.into_any();
                        }
                        AnalyticsAccess::BillingDisabled => {
                            return view! {
                                <Card>
                                    <CardContent>
                                        <p class="text-muted-foreground py-6">
                                            "Analytics is not available for this workspace."
                                        </p>
                                    </CardContent>
                                </Card>
                            }.into_any();
                        }
                        AnalyticsAccess::Allowed => {}
                    }

                    match sites_resource.await {
                        Ok(sites) => {
                            let usage = usage_resource.await;
                            view! {
                                <AnalyticsContent
                                    initial_sites=sites
                                    sites_resource=sites_resource
                                    usage=usage
                                />
                            }.into_any()
                        }
                        Err(_) => {
                            let usage = usage_resource.await;
                            view! {
                                <AnalyticsContent
                                    initial_sites=vec![]
                                    sites_resource=sites_resource
                                    usage=usage
                                />
                            }.into_any()
                        }
                    }
                })}
            </Transition>
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
                <CardHeader>
                    <Skeleton class="h-5 w-40"/>
                    <Skeleton class="h-4 w-56 mt-1"/>
                </CardHeader>
                <CardContent>
                    <div class="space-y-4">
                        <div class="space-y-2">
                            <Skeleton class="h-4 w-full"/>
                            <Skeleton class="h-2 w-full"/>
                        </div>
                        <Skeleton class="h-4 w-3/5"/>
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
    sites_resource: Resource<Result<Vec<AnalyticsSiteData>, ServerFnError>>,
    usage: Result<AnalyticsUsageData, ServerFnError>,
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
            if result.is_ok() {
                reset_form();
                sites_resource.refetch();
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
            if result.is_ok() {
                reset_form();
                sites_resource.refetch();
            }
        }
    });

    // Effect: handle delete result
    Effect::new(move || {
        if let Some(result) = delete_action.value().get()
            && result.is_ok()
        {
            sites_resource.refetch();
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
            // Usage overview — always rendered here because AnalyticsContent
            // only mounts on the AnalyticsAccess::Allowed path (not self-hosted,
            // has ManageAnalytics, billing enabled). No second gate needed.
            {match usage {
                Ok(data) => view! { <AnalyticsUsageCard data=data/> }.into_any(),
                Err(e) => view! { <AnalyticsUsageErrorCard message=e.to_string()/> }.into_any(),
            }}

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
                            <Icon icon=phosphor_leptos::PLUS size="16px"/>
                            "Add Site"
                        </span>
                    </Button>
                </Show>
            </div>

            // Error messages from actions
            {move || create_action.value().get().and_then(|r| r.err()).map(|e| {
                view! {
                    <Alert variant=AlertVariant::Error>
                        <AlertDescription>
                            {format!("Failed to create site: {e}")}
                        </AlertDescription>
                    </Alert>
                }
            })}
            {move || update_action.value().get().and_then(|r| r.err()).map(|e| {
                view! {
                    <Alert variant=AlertVariant::Error>
                        <AlertDescription>
                            {format!("Failed to update site: {e}")}
                        </AlertDescription>
                    </Alert>
                }
            })}
            {move || delete_action.value().get().and_then(|r| r.err()).map(|e| {
                view! {
                    <Alert variant=AlertVariant::Error>
                        <AlertDescription>
                            {format!("Failed to delete site: {e}")}
                        </AlertDescription>
                    </Alert>
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
                        editing_id.as_ref().is_none_or(|eid| eid != &s.id)
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
                                <Icon icon=phosphor_leptos::GLOBE size="40px"/>
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
            message=Signal::derive(move || delete_target.get().map_or(String::new(), |(_, name)| format!("Delete \"{name}\"? This cannot be undone.")))
            confirm_text="Delete"
            on_confirm=on_confirm_delete
            on_cancel=on_cancel_delete
        />
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Usage Overview
// ─────────────────────────────────────────────────────────────────────────────

/// CSS class for the usage progress bar, driven by the server-computed
/// `AnalyticsUsageData::status` rather than re-deriving thresholds from
/// `usage_percent` client-side — the server already knows about the grace
/// allowance and bundle draw-down, which a client-side percentage-only
/// threshold can't reconstruct (a workspace can be over the included quota
/// but not "in trouble" at all, if the bundle reserve is covering it — that
/// is the `"reserve"` state below, distinct from `"warning"`/`"exceeded"`).
fn analytics_status_bar_class(status: &str) -> &'static str {
    match status {
        "warning" => "h-2 rounded-full transition-all bg-warning-foreground",
        "reserve" => "h-2 rounded-full transition-all bg-primary",
        "exceeded" | "blocked" => "h-2 rounded-full transition-all bg-error-foreground",
        // "ok" and any unrecognized status (forward-compat: a server-added
        // status this build doesn't know about should read as calm, not alarm)
        _ => "h-2 rounded-full transition-all bg-success-foreground",
    }
}

/// Status banner shown below the usage card, if any. Returns the
/// `<Alert>` variant to render alongside the message, chosen by severity
/// rather than a flat error/not-error split:
/// - `"reserve"` is `Info` — drawing from a paid, non-expiring bundle is
///   the system working as designed, not a problem needing attention.
/// - `"exceeded"` is `Warning` — over quota and out of reserve, but the
///   grace allowance is still covering writes, so nothing is failing yet.
/// - `"blocked"` is `Error` — grace is also exhausted and events are
///   actively being rejected.
fn analytics_usage_banner(status: &str, over_included: u64) -> Option<(AlertVariant, String)> {
    match status {
        "reserve" => Some((
            AlertVariant::Info,
            format!(
                "Drawing from bundle reserve: {} events used beyond this month's included quota.",
                format_number(over_included)
            ),
        )),
        "exceeded" => Some((
            AlertVariant::Warning,
            "Monthly quota exceeded and no bundle reserve remaining. A grace allowance is \
             still in effect."
                .to_string(),
        )),
        "blocked" => Some((
            AlertVariant::Error,
            "Monthly quota and grace allowance exhausted with no bundle reserve remaining — \
             new analytics events are being rejected."
                .to_string(),
        )),
        _ => None,
    }
}

/// Visual bar width for a usage percentage that may exceed 100 — a bar
/// can't render past its own container, so the width is capped while the
/// adjacent text label still shows the true (uncapped) percentage. Kept as
/// a pure function so the capping behavior is independently testable.
fn usage_bar_width_percent(usage_percent: f64) -> f64 {
    usage_percent.min(100.0)
}

/// Usage overview card — monthly event quota, bundle reserve balance, and
/// an over-quota status banner. Not gated on self-hosted here: the page's
/// `analytics_access()` guard already keeps this component from mounting
/// for self-hosted workspaces (see `AnalyticsPage`), so `data` is only ever
/// constructed from a real Cloud quota — never the zeroed-out struct
/// `get_analytics_usage` returns for self-hosted (that branch is simply
/// never reached from this page).
#[component]
fn AnalyticsUsageCard(data: AnalyticsUsageData) -> impl IntoView {
    let bar_class = analytics_status_bar_class(&data.status);
    let bar_width = format!("{}%", usage_bar_width_percent(data.usage_percent));
    let over_included = data.events_used.saturating_sub(data.events_limit);
    let banner = analytics_usage_banner(&data.status, over_included);
    let events_used = data.events_used;
    let events_limit = data.events_limit;
    let usage_percent = data.usage_percent;
    let stat_value: ChildrenFn = Arc::new(move || {
        view! {
            {format!(
                "{} / {}",
                format_number(events_used),
                format_number(events_limit),
            )}
            <span class="text-muted-foreground ml-1.5">
                {format!("({:.0}%)", usage_percent)}
            </span>
        }
        .into_any()
    });

    view! {
        <UsageCardShell
            icon=phosphor_leptos::PULSE
            title="Event Usage"
            description="Analytics events against your monthly quota and bundle reserve."
            stat_label="Events This Month"
            stat_value=stat_value
            bar_class=bar_class
            bar_width=bar_width
            footnote="Included monthly quota — resets each billing period."
            bundle_row=BundleRow {
                label: "Bundle Reserve",
                description: "Non-expiring. Used after the monthly quota is consumed.",
                value: format!("{} events", format_number(data.bundle_balance)),
            }
        >
            {banner.map(|(variant, message)| view! {
                <Alert variant=variant>
                    <AlertDescription>{message}</AlertDescription>
                </Alert>
            })}
        </UsageCardShell>
    }
}

/// Explicit error state for a failed usage-data fetch — never renders "0 of
/// 0 events" or a blank card, since that would look identical to (and be
/// indistinguishable from) an admin who has genuinely used nothing.
#[component]
fn AnalyticsUsageErrorCard(message: String) -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::PULSE size="20px"/>
                    "Event Usage"
                </CardTitle>
                <CardDescription>
                    "Analytics events against your monthly quota and bundle reserve."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <Alert variant=AlertVariant::Error>
                    <AlertDescription>
                        {format!("Failed to load usage data: {message}")}
                    </AlertDescription>
                </Alert>
            </CardContent>
        </Card>
    }
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
                    <Icon icon=phosphor_leptos::DATABASE size="12px"/>
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
                                    <Icon icon=phosphor_leptos::GLOBE size="16px"/>
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
                                <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="16px"/>
                            </span>
                        </Button>
                        <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm on:click=on_delete>
                            <span class="text-muted-foreground">
                                <Icon icon=phosphor_leptos::TRASH size="16px"/>
                            </span>
                        </Button>
                    </div>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-3">
                    <div class="flex items-center gap-2 text-sm text-muted-foreground">
                        <span class="inline-flex">
                            <Icon icon=phosphor_leptos::CODE size="16px"/>
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
                    set_copied.try_set(true);
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    set_copied.try_set(false);
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
            class="absolute top-2 right-2 p-1.5 rounded-md text-muted-foreground hover:text-foreground hover:bg-secondary transition-colors"
            on:click=on_click
            title="Copy to clipboard"
        >
            {move || {
                if copied.get() {
                    view! { <Icon icon=phosphor_leptos::CLIPBOARD_TEXT size="16px"/> }.into_any()
                } else {
                    view! { <Icon icon=phosphor_leptos::COPY size="16px"/> }.into_any()
                }
            }}
        </button>
    }
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    //! KYO-260 compile-time sanity checks. This file is a Leptos view tree —
    //! its reactive `Suspend` branching can't be exercised as a plain unit
    //! test — so, following the precedent in `datasources.rs` and
    //! `profile.rs` (`tests_part3`), these assert against the source text
    //! itself.
    //!
    //! Before this fix, `AnalyticsPage` guarded only on `is_self_hosted`,
    //! so a non-admin member who reached `/settings/analytics` (via the
    //! datasources-page link, which had its own independent gating bug)
    //! saw an empty shell: every server fn behind the page silently
    //! rejected them, and nothing on the page said why. These tests lock
    //! in that the page now has exactly one guard, and that the guard
    //! covers the denied path explicitly rather than falling through to
    //! the sites content.
    //!
    //! Also covers KYO-278's usage-overview decision logic
    //! (`analytics_status_bar_class`, `analytics_usage_banner`,
    //! `usage_bar_width_percent`), which — unlike the guard above — are
    //! plain pure functions and are tested directly rather than via
    //! source-text assertions.

    use super::*;

    const SRC: &str = include_str!("analytics.rs");

    /// Returns the source slice from the first occurrence of `start` up to
    /// (but not including) the first occurrence of `end` that follows it.
    /// Panics with a clear message if either marker is missing — a missing
    /// marker means the code it was anchoring has been renamed or removed.
    fn extract_between<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
        let start_pos = src
            .find(start)
            .unwrap_or_else(|| panic!("marker not found in analytics.rs: {start:?}"));
        let end_pos = src[start_pos..]
            .find(end)
            .map(|i| start_pos + i)
            .unwrap_or_else(|| panic!("end marker not found after {start:?} in analytics.rs: {end:?}"));
        &src[start_pos..end_pos]
    }

    /// The marker that opens this very `mod tests` block. Slicing `SRC`
    /// up to this marker yields only the production code above it —
    /// required because `SRC` is `include_str!`-ed from this same file, so
    /// counting matches against the *whole* file would also count this
    /// test's own source text (the search-string literal below, this doc
    /// comment) and the assertion could never pass no matter what the
    /// production code does.
    const MOD_TESTS_MARKER: &str = "#[cfg(all(test, feature = \"ssr\"))]\nmod tests {";

    /// There must be exactly one `Suspend::new` guard block in this file.
    /// A second, independent guard would mean the "what does this page
    /// render?" decision is split across two places again — the exact
    /// drift KYO-260 fixes.
    #[test]
    fn there_is_exactly_one_suspend_guard_block() {
        let production_src = SRC
            .split(MOD_TESTS_MARKER)
            .next()
            .expect("MOD_TESTS_MARKER must appear in analytics.rs");

        let count = production_src.matches("Suspend::new(").count();
        assert_eq!(
            count, 1,
            "AnalyticsPage must have exactly one Suspend guard block, found {count} in \
             production code — a second guard reintroduces the KYO-260 drift"
        );
    }

    /// The guard must fail closed: an `Err` from `user_ctx.await` must map
    /// to `AnalyticsAccess::Denied`, never fall through to the sites
    /// content path.
    #[test]
    fn user_ctx_error_fails_closed_to_denied() {
        let guard = extract_between(SRC, "let access = match user_ctx.await {", "};");
        assert!(
            guard.contains("Err(_) => AnalyticsAccess::Denied"),
            "an errored user_ctx must resolve to AnalyticsAccess::Denied, not fall through \
             to the sites content — found: {guard:?}"
        );
    }

    /// The `Denied` arm must actually short-circuit with `return` and
    /// render an explicit access-denied message — not silently continue to
    /// `sites_resource.await`, which is what produced the empty-shell bug
    /// this ticket fixes.
    #[test]
    fn denied_access_renders_explicit_message_and_returns() {
        let arm = extract_between(
            SRC,
            "AnalyticsAccess::Denied => {",
            "AnalyticsAccess::BillingDisabled => {",
        );
        assert!(
            arm.contains("return view! {"),
            "the Denied arm must return early — falling through would still reach \
             sites_resource.await and re-create the empty-shell bug"
        );
        assert!(
            arm.contains("Analytics is admin-only"),
            "the Denied arm must render an explicit access-denied heading, not an empty page"
        );
    }

    /// The `Allowed` arm must be a no-op that lets control fall through to
    /// the existing `sites_resource.await` content path unchanged — the
    /// ticket's requirement that the allowed path is untouched.
    #[test]
    fn allowed_access_falls_through_to_sites_content() {
        const ALLOWED_ARM_MARKER: &str = "AnalyticsAccess::Allowed => {}";
        let arm = extract_between(SRC, ALLOWED_ARM_MARKER, "match sites_resource.await {");
        // extract_between's slice includes the start marker itself, so strip
        // it off before checking what comes *between* the no-op Allowed arm
        // and the pre-existing sites_resource match — otherwise the marker's
        // own letters would always fail the whitespace/brace check below.
        let between = arm.strip_prefix(ALLOWED_ARM_MARKER).unwrap_or_else(|| {
            panic!("extract_between did not return a slice starting with the marker it was given: {arm:?}")
        });
        assert!(
            between.trim().chars().all(|c| c == '}' || c.is_whitespace()),
            "the Allowed arm must fall through directly to sites_resource.await with no \
             extra logic in between — found: {between:?}"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // KYO-278: usage overview decision logic. These test the pure
    // functions directly rather than only asserting markup exists, per
    // CODING_STANDARDS's testing section.
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn bar_class_is_success_for_ok_status() {
        assert_eq!(
            analytics_status_bar_class("ok"),
            "h-2 rounded-full transition-all bg-success-foreground"
        );
    }

    #[test]
    fn bar_class_is_warning_for_warning_status() {
        assert_eq!(
            analytics_status_bar_class("warning"),
            "h-2 rounded-full transition-all bg-warning-foreground"
        );
    }

    #[test]
    fn bar_class_is_primary_for_reserve_status() {
        // "reserve" is deliberately NOT error-colored — drawing from a paid,
        // non-expiring bundle is the system working as designed, not a
        // problem state. This is the exact distinction a client-side
        // percentage-only threshold (as in usage.rs's AnalyticsEventsCard,
        // which renders live on self-hosted deployments via UsagePage —
        // see the self-hosted quota-data notes on get_analytics_usage and
        // get_ai_usage_status in server_fns/) cannot make without
        // re-deriving what the server already knows.
        assert_eq!(
            analytics_status_bar_class("reserve"),
            "h-2 rounded-full transition-all bg-primary"
        );
    }

    #[test]
    fn bar_class_is_error_for_exceeded_and_blocked_status() {
        assert_eq!(
            analytics_status_bar_class("exceeded"),
            "h-2 rounded-full transition-all bg-error-foreground"
        );
        assert_eq!(
            analytics_status_bar_class("blocked"),
            "h-2 rounded-full transition-all bg-error-foreground"
        );
    }

    #[test]
    fn bar_class_defaults_to_success_for_unrecognized_status() {
        // Forward-compat: an unknown status (e.g. a server rollout ahead of
        // this build) must not read as an alarm by default.
        assert_eq!(
            analytics_status_bar_class("some-future-status"),
            "h-2 rounded-full transition-all bg-success-foreground"
        );
    }

    #[test]
    fn banner_is_none_for_ok_and_warning() {
        assert!(analytics_usage_banner("ok", 0).is_none());
        assert!(analytics_usage_banner("warning", 0).is_none());
    }

    #[test]
    fn banner_for_reserve_is_info_not_warning_or_error() {
        let (variant, message) = analytics_usage_banner("reserve", 12_345)
            .expect("reserve status must produce a banner");
        assert!(
            matches!(variant, AlertVariant::Info),
            "reserve is an expected, non-problem state — Info, not Warning or Error"
        );
        assert!(
            message.contains("12,345"),
            "banner must surface the exact overage count: {message:?}"
        );
    }

    #[test]
    fn banner_for_exceeded_is_warning_and_blocked_is_error() {
        let (exceeded_variant, exceeded_message) =
            analytics_usage_banner("exceeded", 500).expect("exceeded status must produce a banner");
        let (blocked_variant, blocked_message) =
            analytics_usage_banner("blocked", 500).expect("blocked status must produce a banner");

        assert!(
            matches!(exceeded_variant, AlertVariant::Warning),
            "exceeded still has the grace allowance covering writes — Warning, not Error"
        );
        assert!(
            matches!(blocked_variant, AlertVariant::Error),
            "blocked means events are actively being rejected — Error"
        );
        assert_ne!(
            exceeded_message, blocked_message,
            "exceeded (grace still active) and blocked (events being rejected) are \
             materially different states and must not share wording"
        );
        assert!(
            blocked_message.contains("rejected"),
            "the blocked message must say events are actually being rejected, not just \
             that the quota was exceeded: {blocked_message:?}"
        );
    }

    #[test]
    fn bar_width_caps_at_100_but_percent_over_100_is_still_a_valid_input() {
        assert_eq!(usage_bar_width_percent(42.0), 42.0);
        assert_eq!(usage_bar_width_percent(100.0), 100.0);
        // usage_percent can legitimately exceed 100 (AnalyticsUsageData's own
        // doc: "0..=100+") — the bar visually caps, but this function's
        // caller in AnalyticsUsageCard still shows the uncapped 142% as text.
        assert_eq!(usage_bar_width_percent(142.0), 100.0);
    }
}
