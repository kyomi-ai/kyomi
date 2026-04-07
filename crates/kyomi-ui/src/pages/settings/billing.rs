// SPDX-License-Identifier: AGPL-3.0-or-later

//! Billing settings page — subscription management and plan selection.
//!
//! Replaces `apps/frontend/src/components/BillingPanel.jsx` (887 lines).
//!
//! Features:
//! - Current plan card (tier name, status badge, billing period, next invoice date)
//! - Team size controls (for team tier — increment/decrement with update button)
//! - Plan comparison cards (Free, Pro, Team with feature lists and pricing)
//! - Cancel/Reactivate subscription buttons
//! - Invoice history table
//! - Stripe portal link ("Manage Payment Method")
//! - Checkout redirect handling (checks URL params for ?checkout=success)

use leptos::prelude::*;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, ConfirmDialog, EmptyState, Modal, ModalSize, Skeleton,
    StatusBadge, StatusBadgeVariant,
};
use crate::server_fns::billing::*;
use crate::server_fns::context::UserContext;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Format an ISO date string to "Month Day, Year" display format.
///
/// Matches the React `toLocaleDateString('en-US', { month: 'long', day: 'numeric', year: 'numeric' })`.
fn format_long_date(iso_str: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso_str) {
        dt.format("%B %-d, %Y").to_string()
    } else {
        iso_str.to_string()
    }
}

/// Format an epoch timestamp to "M/D/YYYY" display format.
///
/// Matches the React `new Date(invoice.created * 1000).toLocaleDateString()`.
fn format_epoch_date(epoch: i64) -> String {
    if let Some(dt) = chrono::DateTime::from_timestamp(epoch, 0) {
        dt.format("%-m/%-d/%Y").to_string()
    } else {
        "—".to_string()
    }
}

/// Capitalize the first letter of a string.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            upper + chars.as_str()
        }
    }
}

/// Calculate the next charge amount based on tier, billing cycle, and user limit.
///
/// Matches the React inline calculation in the "Next charge" row.
fn calculate_next_charge(tier: &str, billing_cycle: Option<&str>, user_limit: Option<i32>) -> String {
    let is_annual = billing_cycle == Some("annual");
    let base_price: f64 = match tier {
        "basic" | "starter" => {
            if is_annual { 180.0 } else { 20.0 }
        }
        "pro" => {
            if is_annual { 348.0 } else { 39.0 }
        }
        "team" => {
            let mut base = if is_annual { 1188.0 } else { 129.0 };
            let additional_users = user_limit.unwrap_or(5).max(5) - 5;
            if additional_users > 0 {
                let per_user_cost: f64 = if is_annual { 180.0 } else { 20.0 };
                base += additional_users as f64 * per_user_cost;
            }
            base
        }
        _ => 0.0,
    };
    format!("${:.2}", base_price)
}

// ─────────────────────────────────────────────────────────────────────────────
// SVG icon paths (inline to avoid leptos_icons class prop limitation)
// ─────────────────────────────────────────────────────────────────────────────

/// CreditCard icon (lucide).
fn credit_card_icon() -> impl IntoView {
    view! {
        <svg class="w-4 h-4 mr-2" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round">
            <rect width="20" height="14" x="2" y="5" rx="2"/>
            <line x1="2" x2="22" y1="10" y2="10"/>
        </svg>
    }
}

/// Check icon (lucide) for plan feature lists.
fn check_icon() -> impl IntoView {
    view! {
        <svg class="w-4 h-4 text-primary mt-0.5 flex-shrink-0" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round">
            <path d="M20 6 9 17l-5-5"/>
        </svg>
    }
}

/// Minus icon (lucide).
fn minus_icon() -> impl IntoView {
    view! {
        <svg class="w-4 h-4" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 12h14"/>
        </svg>
    }
}

/// Plus icon (lucide).
fn plus_icon() -> impl IntoView {
    view! {
        <svg class="w-4 h-4" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round">
            <path d="M5 12h14"/>
            <path d="M12 5v14"/>
        </svg>
    }
}

/// Users icon (lucide).
fn users_icon() -> impl IntoView {
    view! {
        <svg class="w-5 h-5" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round">
            <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/>
            <circle cx="9" cy="7" r="4"/>
            <path d="M22 21v-2a4 4 0 0 0-3-3.87"/>
            <path d="M16 3.13a4 4 0 0 1 0 7.75"/>
        </svg>
    }
}

/// FileText icon (lucide).
fn file_text_icon() -> impl IntoView {
    view! {
        <svg class="w-4 h-4" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round">
            <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/>
            <path d="M14 2v4a2 2 0 0 0 2 2h4"/>
            <path d="M10 9H8"/>
            <path d="M16 13H8"/>
            <path d="M16 17H8"/>
        </svg>
    }
}

/// ExternalLink icon (lucide).
fn external_link_icon() -> impl IntoView {
    view! {
        <svg class="w-3 h-3" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round">
            <path d="M15 3h6v6"/>
            <path d="M10 14 21 3"/>
            <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1-2-2h6"/>
        </svg>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

/// Billing settings page content.
#[component]
pub fn BillingPage() -> impl IntoView {
    // Read the UserContext provided by SettingsShell to check deployment mode.
    let user_ctx = expect_context::<Resource<Result<UserContext, ServerFnError>>>();

    // Resources for subscription info (reload via version signal).
    // get_subscription_info() returns Ok(None) when Stripe is not configured —
    // not an error, just "not available for this deployment".
    let (sub_version, set_sub_version) = signal(0u32);
    let subscription = Resource::new(
        move || sub_version.get(),
        |_| get_subscription_info(),
    );

    let invoices = Resource::new(|| (), |_| get_invoices());

    // UI state
    let (error, set_error) = signal(Option::<String>::None);
    let (success, set_success) = signal(Option::<String>::None);
    let (checkout_loading, set_checkout_loading) = signal(false);
    let (show_plans_modal, set_show_plans_modal) = signal(false);
    let (team_size_loading, set_team_size_loading) = signal(false);
    let (desired_team_size, set_desired_team_size) = signal(5i32);

    // Confirm dialog state
    let dialog_open = RwSignal::new(false);
    let (dialog_title, set_dialog_title) = signal(String::new());
    let (dialog_message, set_dialog_message) = signal(String::new());
    let (dialog_confirm_text, set_dialog_confirm_text) = signal("Confirm".to_string());
    let (dialog_destructive, set_dialog_destructive) = signal(false);
    let (pending_confirm_action, set_pending_confirm_action) =
        signal(Option::<ConfirmAction>::None);

    // Sync desired team size when subscription loads
    Effect::new(move || {
        if let Some(Ok(info)) = subscription.get() && let Some(limit) = info.user_limit {
            set_desired_team_size.set(limit);
        }
    });

    // Check for checkout success param on mount
    #[cfg(target_arch = "wasm32")]
    {
        Effect::new(move |_| {
            let window = web_sys::window().unwrap();
            let search = window.location().search().unwrap_or_default();
            if search.contains("checkout=success") {
                set_success.set(Some(
                    "Payment successful! Your subscription is being activated...".to_string(),
                ));

                // Clean URL
                let _ = window
                    .history()
                    .unwrap()
                    .replace_state_with_url(
                        &wasm_bindgen::JsValue::NULL,
                        "",
                        Some("/settings/billing"),
                    );

                // Poll for updated subscription
                let poll_count = std::cell::Cell::new(0u32);
                let interval = gloo_timers::callback::Interval::new(1_000, move || {
                    poll_count.set(poll_count.get() + 1);
                    set_sub_version.update(|v| *v += 1);
                    if poll_count.get() >= 10 {
                        set_success.set(Some(
                            "Subscription is processing. Please refresh the page in a moment."
                                .to_string(),
                        ));
                    }
                });

                // Keep interval alive — wrap in SendWrapper for WASM compatibility
                let interval = send_wrapper::SendWrapper::new(interval);
                leptos::prelude::on_cleanup(move || drop(interval));
            }
        });
    }

    // ── Actions ──────────────────────────────────────────────────────────

    let handle_upgrade = Action::new({
        move |(tier, cycle): &(String, String)| {
            let tier = tier.clone();
            let cycle = cycle.clone();
            async move {
                set_checkout_loading.set(true);
                set_error.set(None);
                match create_checkout(tier, cycle, None).await {
                    Ok(_redirect) => {
                        #[cfg(target_arch = "wasm32")]
                        {
                            let _ = web_sys::window()
                                .unwrap()
                                .location()
                                .set_href(&_redirect.url);
                        }
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to start checkout: {e}")));
                        set_checkout_loading.set(false);
                    }
                }
            }
        }
    });

    let handle_cancel = Action::new({
        move |(): &()| async move {
            match cancel_subscription().await {
                Ok(result) => {
                    set_success.set(Some(result.message));
                    set_sub_version.update(|v| *v += 1);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to cancel subscription: {e}")));
                }
            }
        }
    });

    let handle_reactivate = Action::new({
        move |(): &()| async move {
            match reactivate_subscription().await {
                Ok(result) => {
                    set_success.set(Some(result.message));
                    set_sub_version.update(|v| *v += 1);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to reactivate subscription: {e}")));
                }
            }
        }
    });

    let handle_team_size_update = Action::new({
        move |size: &i32| {
            let size = *size;
            async move {
                set_team_size_loading.set(true);
                set_error.set(None);
                set_success.set(None);
                match update_team_size(size).await {
                    Ok(result) => {
                        set_success.set(Some(result.message));
                        set_sub_version.update(|v| *v += 1);
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to update team size: {e}")));
                    }
                }
                set_team_size_loading.set(false);
            }
        }
    });

    let handle_manage_billing = Action::new({
        move |(): &()| async move {
            set_checkout_loading.set(true);
            set_error.set(None);
            match create_portal_session().await {
                Ok(_redirect) => {
                    #[cfg(target_arch = "wasm32")]
                    {
                        let _ = web_sys::window()
                            .unwrap()
                            .location()
                            .set_href(&_redirect.url);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to open billing portal: {e}")));
                    set_checkout_loading.set(false);
                }
            }
        }
    });

    // ── Confirm dialog callbacks ─────────────────────────────────────────

    let on_confirm = Callback::new(move |()| {
        dialog_open.set(false);
        if let Some(action) = pending_confirm_action.get_untracked() {
            match action {
                ConfirmAction::Cancel => {
                    handle_cancel.dispatch(());
                }
                ConfirmAction::Reactivate => {
                    handle_reactivate.dispatch(());
                }
            }
            set_pending_confirm_action.set(None);
        }
    });

    let on_cancel_dialog = Callback::new(move |()| {
        dialog_open.set(false);
        set_pending_confirm_action.set(None);
    });

    view! {
        <div class="p-6">
        // If the user context indicates self-hosted mode, show an informational
        // message instead of loading Stripe-backed billing data.
        {move || {
            if let Some(Ok(ctx)) = user_ctx.get() && ctx.is_self_hosted {
                return view! {
                    <Card>
                        <CardContent>
                            <p class="text-muted-foreground py-6">
                                "Billing is not available in self-hosted mode."
                            </p>
                        </CardContent>
                    </Card>
                }.into_any();
            }
            view! {
                <div class="space-y-6" style:display="block">
                    // Alerts
                    {move || error.get().map(|msg| view! {
                        <Alert variant=AlertVariant::Error class="mb-4">
                            <AlertDescription>{msg}</AlertDescription>
                        </Alert>
                    })}
                    {move || success.get().map(|msg| view! {
                        <Alert variant=AlertVariant::Success class="mb-4">
                            <AlertDescription>{msg}</AlertDescription>
                        </Alert>
                    })}

                    // Main content
                    <Transition fallback=move || view! {
                        <div class="flex items-center justify-center p-8">
                            <Skeleton class="h-8 w-8 rounded-full"/>
                        </div>
                    }>
                        {move || Suspend::new(async move {
                            match subscription.await {
                                Ok(info) => {
                                    view! {
                                        <BillingContent
                                            info=info
                                            invoices=invoices
                                            checkout_loading=checkout_loading
                                            team_size_loading=team_size_loading
                                            desired_team_size=desired_team_size
                                            set_desired_team_size=set_desired_team_size
                                            show_plans_modal=show_plans_modal
                                            set_show_plans_modal=set_show_plans_modal
                                            handle_upgrade=handle_upgrade
                                            handle_manage_billing=handle_manage_billing
                                            handle_team_size_update=handle_team_size_update
                                            dialog_open=dialog_open
                                            set_dialog_title=set_dialog_title
                                            set_dialog_message=set_dialog_message
                                            set_dialog_confirm_text=set_dialog_confirm_text
                                            set_dialog_destructive=set_dialog_destructive
                                            set_pending_confirm_action=set_pending_confirm_action
                                        />
                                    }.into_any()
                                }
                                Err(e) => {
                                    view! {
                                        <Alert variant=AlertVariant::Error>
                                            <AlertDescription>{format!("Failed to load subscription information: {e}")}</AlertDescription>
                                        </Alert>
                                    }.into_any()
                                }
                            }
                        })}
                    </Transition>

                    // Confirm Dialog
                    <ConfirmDialog
                        open=Signal::from(dialog_open)
                        title=dialog_title.get_untracked()
                        message=dialog_message.get_untracked()
                        confirm_text=dialog_confirm_text.get_untracked()
                        destructive=dialog_destructive.get_untracked()
                        on_confirm=on_confirm
                        on_cancel=on_cancel_dialog
                    />
                </div>
            }.into_any()
        }}
        </div>
    }
}

/// Pending confirm action type.
#[derive(Clone, Copy)]
enum ConfirmAction {
    Cancel,
    Reactivate,
}

// ─────────────────────────────────────────────────────────────────────────────
// Billing content (rendered after subscription info loads)
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn BillingContent(
    info: SubscriptionInfo,
    invoices: Resource<Result<Vec<InvoiceRecord>, ServerFnError>>,
    checkout_loading: ReadSignal<bool>,
    team_size_loading: ReadSignal<bool>,
    desired_team_size: ReadSignal<i32>,
    set_desired_team_size: WriteSignal<i32>,
    show_plans_modal: ReadSignal<bool>,
    set_show_plans_modal: WriteSignal<bool>,
    handle_upgrade: Action<(String, String), ()>,
    handle_manage_billing: Action<(), ()>,
    handle_team_size_update: Action<i32, ()>,
    dialog_open: RwSignal<bool>,
    set_dialog_title: WriteSignal<String>,
    set_dialog_message: WriteSignal<String>,
    set_dialog_confirm_text: WriteSignal<String>,
    set_dialog_destructive: WriteSignal<bool>,
    set_pending_confirm_action: WriteSignal<Option<ConfirmAction>>,
) -> impl IntoView {
    let current_tier = info.tier.clone();
    let billing_cycle = info.billing_cycle.clone();
    let status = info.status.clone();
    let period_end = info.period_end.clone();
    let user_limit = info.user_limit;

    // Pre-clone billing_cycle for team size card (before it's moved into closures)
    let billing_cycle_for_team = billing_cycle.clone();

    // Clones for closures
    let tier_for_header = current_tier.clone();
    let tier_for_status = current_tier.clone();
    let tier_for_actions = current_tier.clone();
    let tier_for_team = current_tier.clone();
    let tier_for_plans = current_tier.clone();
    let tier_for_invoices = current_tier.clone();
    let tier_for_annual_note = current_tier.clone();
    let tier_for_modal = current_tier.clone();
    let billing_cycle_for_header = billing_cycle.clone();
    let billing_cycle_for_modal = billing_cycle.clone();
    let status_for_badge = status.clone();
    let status_for_info = status.clone();
    let status_for_actions = status.clone();
    let status_for_team = status.clone();

    // Modal close callback
    let on_close_modal = Callback::new(move |()| set_show_plans_modal.set(false));

    view! {
        <div class="space-y-6" style:display="block">
            // Current Plan Card
            <Card>
                <CardHeader>
                    <div class="flex items-start justify-between">
                        <div>
                            <CardTitle>"Current Plan"</CardTitle>
                            <CardDescription class="mt-1">
                                {move || {
                                    if tier_for_header == "free" {
                                        "Free Plan".to_string()
                                    } else {
                                        let cycle_label = if billing_cycle_for_header.as_deref() == Some("annual") {
                                            "Annual"
                                        } else {
                                            "Monthly"
                                        };
                                        format!("{} - {}", capitalize_first(&tier_for_header), cycle_label)
                                    }
                                }}
                            </CardDescription>
                        </div>
                        <div class="flex items-center gap-3">
                            // Status Badge
                            {(current_tier != "free" && !status_for_badge.is_empty()).then(|| {
                                let (variant, label) = match status_for_badge.as_str() {
                                    "active" => (StatusBadgeVariant::Success, "Active"),
                                    "cancelled" => (StatusBadgeVariant::Warning, "Cancelled"),
                                    "past_due" => (StatusBadgeVariant::Error, "Past Due"),
                                    _ => (StatusBadgeVariant::Default, status_for_badge.as_str()),
                                };
                                // Need to own the label for the non-static case
                                let label_owned = label.to_string();
                                view! {
                                    <StatusBadge variant=variant>
                                        {label_owned}
                                    </StatusBadge>
                                }
                            })}
                            // Manage Billing Button
                            {(current_tier != "free").then(|| view! {
                                <Button
                                    variant=ButtonVariant::Outline
                                    size=ButtonSize::Sm
                                    disabled=checkout_loading.get_untracked()
                                    on:click=move |_| { handle_manage_billing.dispatch(()); }
                                >
                                    {credit_card_icon()}
                                    "Manage Billing"
                                </Button>
                            })}
                        </div>
                    </div>
                </CardHeader>
                <CardContent>
                    <div class="space-y-4">
                        // Subscription Status Info
                        {(tier_for_status != "free" && period_end.is_some()).then(|| {
                            let period_end_for_active = period_end.clone();
                            let period_end_for_cancelled = period_end.clone();
                            let billing_cycle_cl = billing_cycle.clone();
                            let user_limit_cl = user_limit;
                            let tier_cl = tier_for_status.clone();

                            view! {
                                <div class="bg-muted/50 border border-border rounded-lg p-4 space-y-2">
                                    {(status_for_info == "active").then(move || {
                                        period_end_for_active.as_ref().map(|pe| {
                                            let next_charge = calculate_next_charge(
                                                &tier_cl,
                                                billing_cycle_cl.as_deref(),
                                                user_limit_cl,
                                            );
                                            view! {
                                                <div>
                                                    <div class="flex justify-between text-sm">
                                                        <span class="text-muted-foreground">"Renews on"</span>
                                                        <span class="font-medium text-foreground">
                                                            {format_long_date(pe)}
                                                        </span>
                                                    </div>
                                                    <div class="flex justify-between text-sm">
                                                        <span class="text-muted-foreground">"Next charge"</span>
                                                        <span class="font-medium text-foreground">
                                                            {next_charge}
                                                        </span>
                                                    </div>
                                                </div>
                                            }
                                        })
                                    })}
                                    {(status_for_actions == "cancelled").then(move || {
                                        period_end_for_cancelled.as_ref().map(|pe| {
                                            view! {
                                                <div>
                                                    <div class="flex justify-between text-sm">
                                                        <span class="text-muted-foreground">"Access until"</span>
                                                        <span class="font-medium text-foreground">
                                                            {format_long_date(pe)}
                                                        </span>
                                                    </div>
                                                    <div class="text-sm text-muted-foreground">
                                                        "Your subscription has been cancelled. You'll retain access to paid features until the end of your billing period."
                                                    </div>
                                                </div>
                                            }
                                        })
                                    })}
                                </div>
                            }
                        })}

                        // Action Buttons
                        {(tier_for_actions != "free").then(|| {
                            let status_cl = status.clone();
                            view! {
                                <div class="flex gap-2 mt-4">
                                    {(status_cl == "active").then(|| view! {
                                        <Button
                                            variant=ButtonVariant::Default
                                            on:click=move |_| set_show_plans_modal.set(true)
                                        >
                                            "Change Plan"
                                        </Button>
                                        <Button
                                            variant=ButtonVariant::Outline
                                            on:click=move |_| {
                                                set_dialog_title.set("Cancel Subscription?".to_string());
                                                set_dialog_message.set("Are you sure you want to cancel your subscription? You will keep access until the end of your billing period.".to_string());
                                                set_dialog_confirm_text.set("Cancel Subscription".to_string());
                                                set_dialog_destructive.set(true);
                                                set_pending_confirm_action.set(Some(ConfirmAction::Cancel));
                                                dialog_open.set(true);
                                            }
                                        >
                                            "Cancel Subscription"
                                        </Button>
                                    })}
                                    {(status_cl == "cancelled").then(|| view! {
                                        <Button
                                            variant=ButtonVariant::Default
                                            class="w-full"
                                            on:click=move |_| {
                                                set_dialog_title.set("Reactivate Subscription?".to_string());
                                                set_dialog_message.set("Reactivate your subscription? Your subscription will continue after the current billing period.".to_string());
                                                set_dialog_confirm_text.set("Reactivate".to_string());
                                                set_dialog_destructive.set(false);
                                                set_pending_confirm_action.set(Some(ConfirmAction::Reactivate));
                                                dialog_open.set(true);
                                            }
                                        >
                                            "Reactivate Subscription"
                                        </Button>
                                    })}
                                </div>
                            }
                        })}
                    </div>
                </CardContent>
            </Card>

            // Team Size Management
            {(tier_for_team == "team" && status_for_team == "active").then(|| {
                let billing_cycle_team = billing_cycle_for_team.clone();
                let user_limit_team = user_limit.unwrap_or(5);

                view! {
                    <TeamSizeCard
                        billing_cycle=billing_cycle_team
                        user_limit=user_limit_team
                        desired_team_size=desired_team_size
                        set_desired_team_size=set_desired_team_size
                        team_size_loading=team_size_loading
                        handle_team_size_update=handle_team_size_update
                    />
                }
            })}

            // Available Plans (free tier only)
            {(tier_for_plans == "free").then(|| {
                view! {
                    <div style:display="block">
                        <h3 class="text-lg font-semibold mb-4 text-foreground">"Available Plans"</h3>
                        <div class="grid grid-cols-1 md:grid-cols-3 gap-4" style:display="grid">
                            <PlanCard
                                name="Starter"
                                annual_price="$15"
                                annual_total="$180/year"
                                monthly_price="$20/month"
                                features=starter_features()
                                recommended=false
                                current_tier="free".to_string()
                                current_billing_cycle=None
                                checkout_loading=checkout_loading
                                handle_upgrade=handle_upgrade
                            />
                            <PlanCard
                                name="Pro"
                                annual_price="$29"
                                annual_total="$348/year"
                                monthly_price="$39/month"
                                features=pro_features()
                                recommended=true
                                current_tier="free".to_string()
                                current_billing_cycle=None
                                checkout_loading=checkout_loading
                                handle_upgrade=handle_upgrade
                            />
                            <PlanCard
                                name="Team"
                                annual_price="$99"
                                annual_total="$1,188/year"
                                monthly_price="$129/month"
                                features=team_features()
                                recommended=false
                                current_tier="free".to_string()
                                current_billing_cycle=None
                                checkout_loading=checkout_loading
                                handle_upgrade=handle_upgrade
                            />
                        </div>
                    </div>
                }
            })}

            // Invoices Section
            <InvoicesSection invoices=invoices current_tier=tier_for_invoices.clone()/>

            // Annual billing note (free tier only)
            {(tier_for_annual_note == "free").then(|| view! {
                <Card class="bg-muted/50">
                    <CardContent class="pt-6">
                        <p class="text-sm text-muted-foreground">
                            <strong>"Annual billing saves 25-30%"</strong>
                            " compared to monthly billing."
                        </p>
                    </CardContent>
                </Card>
            })}

            // Change Plan Modal
            <Modal
                show=Signal::from(show_plans_modal)
                on_close=on_close_modal
                title="Change Plan"
                size=ModalSize::Xl
            >
                <p class="text-sm text-muted-foreground mb-6">
                    "Select a new plan to upgrade or downgrade your subscription"
                </p>
                <div style:display="block">
                    <div class="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6" style:display="grid">
                        <PlanCard
                            name="Starter"
                            annual_price="$15"
                            annual_total="$180/year"
                            monthly_price="$20/month"
                            features=starter_features()
                            recommended=false
                            current_tier=tier_for_modal.clone()
                            current_billing_cycle=billing_cycle_for_modal.clone()
                            checkout_loading=checkout_loading
                            handle_upgrade=handle_upgrade
                        />
                        <PlanCard
                            name="Pro"
                            annual_price="$29"
                            annual_total="$348/year"
                            monthly_price="$39/month"
                            features=pro_features()
                            recommended=true
                            current_tier=tier_for_modal.clone()
                            current_billing_cycle=billing_cycle_for_modal.clone()
                            checkout_loading=checkout_loading
                            handle_upgrade=handle_upgrade
                        />
                        <PlanCard
                            name="Team"
                            annual_price="$99"
                            annual_total="$1,188/year"
                            monthly_price="$129/month"
                            features=team_features()
                            recommended=false
                            current_tier=tier_for_modal.clone()
                            current_billing_cycle=billing_cycle_for_modal.clone()
                            checkout_loading=checkout_loading
                            handle_upgrade=handle_upgrade
                        />
                    </div>
                    <Card class="bg-muted/50">
                        <CardContent class="pt-6">
                            <p class="text-sm text-muted-foreground">
                                <strong>"Annual billing saves 25-30%"</strong>
                                " compared to monthly billing."
                            </p>
                        </CardContent>
                    </Card>
                </div>
            </Modal>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Team Size Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn TeamSizeCard(
    billing_cycle: Option<String>,
    user_limit: i32,
    desired_team_size: ReadSignal<i32>,
    set_desired_team_size: WriteSignal<i32>,
    team_size_loading: ReadSignal<bool>,
    handle_team_size_update: Action<i32, ()>,
) -> impl IntoView {
    let is_annual = billing_cycle.as_deref() == Some("annual");
    let per_user_label = if is_annual {
        "$15/month (billed $180/year)"
    } else {
        "$20/month"
    };
    let per_user_short = if is_annual { "$15/mo" } else { "$20/mo" };
    let base_monthly = if is_annual { "$99/mo" } else { "$129/mo" };
    let per_user_cost: f64 = if is_annual { 15.0 } else { 20.0 };
    let base_cost: f64 = if is_annual { 99.0 } else { 129.0 };

    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    {users_icon()}
                    "Team Size"
                </CardTitle>
                <CardDescription>
                    {format!(
                        "Manage your team size. Base plan includes 5 users, additional users are {} each.",
                        per_user_label
                    )}
                </CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    // Current Team Info
                    <div class="bg-muted/50 border border-border rounded-lg p-4">
                        <div class="flex justify-between text-sm mb-2">
                            <span class="text-muted-foreground">"Current team size"</span>
                            <span class="font-medium text-foreground">
                                {format!("{} {}", user_limit, if user_limit == 1 { "user" } else { "users" })}
                            </span>
                        </div>
                        {(user_limit > 5).then(|| view! {
                            <div class="flex justify-between text-sm">
                                <span class="text-muted-foreground">"Additional users"</span>
                                <span class="font-medium text-foreground">
                                    {format!("{} \u{00d7} {}", user_limit - 5, per_user_short)}
                                </span>
                            </div>
                        })}
                    </div>

                    // Team Size Adjuster
                    <div>
                        <label class="text-sm font-medium text-foreground block mb-2">
                            "Adjust team size"
                        </label>
                        <div class="flex items-center gap-3">
                            // Minus button — uses raw <button> for reactive disabled
                            <button
                                class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground h-8 rounded-md px-3 text-xs"
                                disabled=move || desired_team_size.get() <= 5 || team_size_loading.get()
                                on:click=move |_| {
                                    set_desired_team_size.update(|v| *v = (*v - 1).max(5));
                                }
                            >
                                {minus_icon()}
                            </button>
                            <input
                                type="number"
                                prop:value=move || desired_team_size.get().to_string()
                                on:input=move |ev| {
                                    let val: i32 = event_target_value(&ev)
                                        .parse()
                                        .unwrap_or(5)
                                        .max(5);
                                    set_desired_team_size.set(val);
                                }
                                min="5"
                                class="w-20 px-3 py-2 text-center border border-border rounded-md bg-background text-foreground"
                                disabled=move || team_size_loading.get()
                            />
                            // Plus button — uses raw <button> for reactive disabled
                            <button
                                class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground h-8 rounded-md px-3 text-xs"
                                disabled=move || team_size_loading.get()
                                on:click=move |_| {
                                    set_desired_team_size.update(|v| *v += 1);
                                }
                            >
                                {plus_icon()}
                            </button>
                            <span class="text-sm text-muted-foreground">"users"</span>
                        </div>
                    </div>

                    // Cost Preview (when team size differs from current)
                    {move || {
                        let desired = desired_team_size.get();
                        (desired != user_limit).then(|| {
                            let additional = (desired - 5).max(0);
                            let additional_cost = additional as f64 * per_user_cost;
                            let total = base_cost + additional_cost;
                            let is_increase = desired > user_limit;

                            view! {
                                <div class="bg-primary/10 border border-primary/20 rounded-lg p-4">
                                    <div class="text-sm space-y-2">
                                        <div class="flex justify-between">
                                            <span class="text-foreground">"Base Team plan (5 users)"</span>
                                            <span class="font-medium text-foreground">{format!("{}/mo", base_monthly)}</span>
                                        </div>
                                        {(desired > 5).then(|| view! {
                                            <div class="flex justify-between">
                                                <span class="text-foreground">
                                                    {format!("Additional users ({})", desired - 5)}
                                                </span>
                                                <span class="font-medium text-foreground">
                                                    {format!("${:.2}/mo", additional_cost)}
                                                </span>
                                            </div>
                                        })}
                                        <div class="pt-2 border-t border-primary/20 flex justify-between font-semibold">
                                            <span class="text-foreground">"New monthly total"</span>
                                            <span class="text-foreground">
                                                {format!("${:.2}/mo", total)}
                                            </span>
                                        </div>
                                        <p class="text-xs text-muted-foreground pt-2">
                                            {if is_increase {
                                                "You will be charged a prorated amount for the remainder of your billing period."
                                            } else {
                                                "You will receive a prorated credit on your next invoice."
                                            }}
                                        </p>
                                    </div>
                                </div>
                            }
                        })
                    }}

                    // Update Button — uses raw <button> for reactive disabled
                    <button
                        class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 bg-primary text-primary-foreground shadow hover:bg-primary/90 h-9 px-4 py-2 w-full"
                        disabled=move || desired_team_size.get() == user_limit || team_size_loading.get()
                        on:click=move |_| {
                            handle_team_size_update.dispatch(desired_team_size.get_untracked());
                        }
                    >
                        {move || if team_size_loading.get() { "Updating..." } else { "Update Team Size" }}
                    </button>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Plan Card
// ─────────────────────────────────────────────────────────────────────────────

/// Feature lists — shared between inline plan cards and modal plan cards.
fn starter_features() -> Vec<&'static str> {
    vec![
        "AI chat and analysis",
        "30 days query history",
        "Unlimited dashboards",
        "Website analytics (1M events/mo)",
        "MCP support",
        "1 user",
        "Email support",
    ]
}

fn pro_features() -> Vec<&'static str> {
    vec![
        "3x more AI usage vs Starter",
        "Kyomi Watch \u{2014} proactive data monitoring",
        "Website analytics (5M events/mo)",
        "Unlimited query history",
        "PDF dashboard export",
        "1 user",
        "Priority email support",
    ]
}

fn team_features() -> Vec<&'static str> {
    vec![
        "Shared AI pool for team",
        "Kyomi Watch \u{2014} proactive data monitoring",
        "Website analytics (25M events/mo)",
        "Slack integration \u{2014} alerts & @kyomi mentions",
        "Up to 5 users ($15-20/mo per additional)",
        "Dashboard sharing & collaboration",
        "Priority chat support",
    ]
}

/// Individual plan display with pricing and features.
///
/// Matches React `PlanCard` sub-component (lines 804-887).
#[component]
fn PlanCard(
    name: &'static str,
    annual_price: &'static str,
    annual_total: &'static str,
    monthly_price: &'static str,
    features: Vec<&'static str>,
    #[prop(default = false)]
    recommended: bool,
    current_tier: String,
    current_billing_cycle: Option<String>,
    checkout_loading: ReadSignal<bool>,
    handle_upgrade: Action<(String, String), ()>,
) -> impl IntoView {
    let plan_tier = name.to_lowercase();
    let is_current_annual =
        current_tier == plan_tier && current_billing_cycle.as_deref() == Some("annual");
    let is_current_monthly =
        current_tier == plan_tier && current_billing_cycle.as_deref() == Some("monthly");

    let card_class = if recommended {
        "relative border-primary border-2"
    } else {
        "relative"
    };

    // Clone for closures
    let plan_tier_annual = plan_tier.clone();
    let plan_tier_monthly = plan_tier.clone();

    view! {
        <Card class=card_class>
            {recommended.then(|| view! {
                <div class="absolute -top-3 left-1/2 -translate-x-1/2">
                    <span class="bg-primary text-primary-foreground px-3 py-1 rounded-full text-xs font-semibold">
                        "Best Value"
                    </span>
                </div>
            })}
            <CardHeader>
                <CardTitle class="text-xl">{name}</CardTitle>
                <CardDescription>
                    <div class="mt-2">
                        <div class="text-xl font-semibold text-foreground">
                            {annual_price}<span class="text-sm font-normal text-muted-foreground">"/month*"</span>
                        </div>
                        <div class="text-xs text-muted-foreground">{annual_total}</div>
                        <div class="text-sm text-muted-foreground mt-1">
                            {format!("or {}", monthly_price)}
                        </div>
                    </div>
                </CardDescription>
            </CardHeader>
            <CardContent>
                <ul class="space-y-2 mb-6">
                    {features.into_iter().map(|feature| {
                        view! {
                            <li class="flex items-start gap-2 text-sm">
                                {check_icon()}
                                <span class="text-foreground">{feature}</span>
                            </li>
                        }
                    }).collect_view()}
                </ul>

                <div class="space-y-2">
                    <Button
                        variant=if is_current_annual { ButtonVariant::Outline } else { ButtonVariant::Default }
                        class="w-full"
                        disabled=checkout_loading.get_untracked() || is_current_annual
                        on:click=move |_| {
                            handle_upgrade.dispatch((plan_tier_annual.clone(), "annual".to_string()));
                        }
                    >
                        {if is_current_annual {
                            "Current Plan".to_string()
                        } else {
                            "Choose Annual".to_string()
                        }}
                    </Button>
                    <Button
                        variant=if is_current_monthly { ButtonVariant::Default } else { ButtonVariant::Outline }
                        class="w-full"
                        disabled=checkout_loading.get_untracked() || is_current_monthly
                        on:click=move |_| {
                            handle_upgrade.dispatch((plan_tier_monthly.clone(), "monthly".to_string()));
                        }
                    >
                        {if is_current_monthly {
                            "Current Plan".to_string()
                        } else {
                            "Choose Monthly".to_string()
                        }}
                    </Button>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Invoices Section
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn InvoicesSection(
    invoices: Resource<Result<Vec<InvoiceRecord>, ServerFnError>>,
    current_tier: String,
) -> impl IntoView {
    view! {
        <Transition fallback=|| ()>
            {move || {
                let current_tier = current_tier.clone();
                Suspend::new(async move {
                match invoices.await {
                    Ok(inv_list) => {
                        let show_section = !inv_list.is_empty() || current_tier != "free";
                        if !show_section {
                            return view! { <div/> }.into_any();
                        }

                        let title = if current_tier == "free" && !inv_list.is_empty() {
                            "Billing History"
                        } else {
                            "Invoices"
                        };

                        view! {
                            <div>
                                <h3 class="text-lg font-semibold mb-4 text-foreground">{title}</h3>
                                {(current_tier == "free" && !inv_list.is_empty()).then(|| view! {
                                    <p class="text-sm text-muted-foreground mb-4">
                                        "Your subscription has ended. Below are your past invoices for your records."
                                    </p>
                                })}
                                {if inv_list.is_empty() {
                                    view! {
                                        <EmptyState
                                            title="No invoices yet"
                                            description="Invoices will appear here after your first billing cycle."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <Card>
                                            <CardContent class="pt-6">
                                                <div class="overflow-x-auto">
                                                    <table class="w-full">
                                                        <thead>
                                                            <tr class="border-b border-border">
                                                                <th class="text-left py-3 px-4 text-sm font-medium text-muted-foreground">"Date"</th>
                                                                <th class="text-left py-3 px-4 text-sm font-medium text-muted-foreground">"Description"</th>
                                                                <th class="text-left py-3 px-4 text-sm font-medium text-muted-foreground">"Amount"</th>
                                                                <th class="text-left py-3 px-4 text-sm font-medium text-muted-foreground">"Status"</th>
                                                                <th class="text-right py-3 px-4 text-sm font-medium text-muted-foreground">"Invoice"</th>
                                                            </tr>
                                                        </thead>
                                                        <tbody>
                                                            {inv_list.into_iter().map(|invoice| {
                                                                let description = invoice.description.clone()
                                                                    .unwrap_or_else(|| "Subscription".to_string());
                                                                let date = invoice.created
                                                                    .map(format_epoch_date)
                                                                    .unwrap_or_else(|| "\u{2014}".to_string());
                                                                let amount = format!("${:.2}", invoice.amount_paid);
                                                                let status = invoice.status.clone().unwrap_or_default();
                                                                let (badge_variant, badge_label) = match status.as_str() {
                                                                    "paid" => (StatusBadgeVariant::Success, "Paid"),
                                                                    "open" => (StatusBadgeVariant::Warning, "Pending"),
                                                                    _ => (StatusBadgeVariant::Error, "Failed"),
                                                                };
                                                                let pdf_url = invoice.invoice_pdf.clone();

                                                                view! {
                                                                    <tr class="border-b border-border last:border-0">
                                                                        <td class="py-3 px-4 text-sm text-foreground">{date}</td>
                                                                        <td class="py-3 px-4 text-sm text-foreground">{description}</td>
                                                                        <td class="py-3 px-4 text-sm text-foreground">{amount}</td>
                                                                        <td class="py-3 px-4 text-sm">
                                                                            <StatusBadge variant=badge_variant>
                                                                                {badge_label}
                                                                            </StatusBadge>
                                                                        </td>
                                                                        <td class="py-3 px-4 text-sm text-right">
                                                                            {pdf_url.map(|url| view! {
                                                                                <a
                                                                                    href=url
                                                                                    target="_blank"
                                                                                    rel="noopener noreferrer"
                                                                                    class="inline-flex items-center gap-1 text-primary hover:text-primary/80 transition-colors"
                                                                                >
                                                                                    {file_text_icon()}
                                                                                    <span>"PDF"</span>
                                                                                    {external_link_icon()}
                                                                                </a>
                                                                            })}
                                                                        </td>
                                                                    </tr>
                                                                }
                                                            }).collect_view()}
                                                        </tbody>
                                                    </table>
                                                </div>
                                            </CardContent>
                                        </Card>
                                    }.into_any()
                                }}
                            </div>
                        }.into_any()
                    }
                    Err(_) => view! { <div/> }.into_any(),
                }
            })}}
        </Transition>
    }
}
