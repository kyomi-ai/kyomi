// SPDX-License-Identifier: AGPL-3.0-or-later

//! Billing settings page — single-tier Cloud plan at $5/user/month.
//!
//! Replaces the old multi-tier plan comparison UI with a streamlined
//! single-plan view.
//!
//! Features:
//! - Current plan card (Cloud — $5/user/month, status badge, renewal info)
//! - User seats card (adjust seat count, min 1)
//! - AI Credits card (BYOK key status, token bundle balance, purchase)
//! - Analytics card (event usage, bundle balance, purchase)
//! - Invoice history table
//! - Stripe portal link ("Manage Billing")
//! - Checkout redirect handling (checks URL params for ?checkout=success)

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, ConfirmDialog, EmptyState, Skeleton, StatusBadge,
    StatusBadgeVariant,
};
use crate::server_fns::billing::*;
use crate::server_fns::context::UserContext;
/// Included analytics events per month for Cloud subscribers.
/// Mirrors `kyomi_core::capability::ANALYTICS_EVENTS_INCLUDED` — defined here
/// because kyomi-core is SSR-only and not available on the WASM target.
const ANALYTICS_EVENTS_INCLUDED: u64 = 100_000;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Price per user per month (Cloud plan).
const PRICE_PER_USER: f64 = 5.0;

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
        "\u{2014}".to_string()
    }
}

/// Format a dollar amount from cents or a float.
fn format_charge(user_count: i32) -> String {
    let total = PRICE_PER_USER * user_count as f64;
    format!("${:.2}", total)
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
    let (team_size_loading, set_team_size_loading) = signal(false);
    let (desired_team_size, set_desired_team_size) = signal(1i32);

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

    let handle_subscribe = Action::new({
        move |quantity: &u64| {
            let quantity = *quantity;
            async move {
                set_checkout_loading.set(true);
                set_error.set(None);
                match create_checkout(quantity).await {
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

    let handle_purchase_ai = Action::new({
        move |(): &()| async move {
            set_error.set(None);
            match purchase_ai_bundle().await {
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
                    set_error.set(Some(format!("Failed to start AI bundle purchase: {e}")));
                }
            }
        }
    });

    let handle_purchase_analytics = Action::new({
        move |(): &()| async move {
            set_error.set(None);
            match purchase_analytics_bundle().await {
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
                    set_error.set(Some(format!("Failed to start analytics bundle purchase: {e}")));
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
                                            handle_subscribe=handle_subscribe
                                            handle_manage_billing=handle_manage_billing
                                            handle_team_size_update=handle_team_size_update
                                            handle_purchase_ai=handle_purchase_ai
                                            handle_purchase_analytics=handle_purchase_analytics
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
    handle_subscribe: Action<u64, ()>,
    handle_manage_billing: Action<(), ()>,
    handle_team_size_update: Action<i32, ()>,
    handle_purchase_ai: Action<(), ()>,
    handle_purchase_analytics: Action<(), ()>,
    dialog_open: RwSignal<bool>,
    set_dialog_title: WriteSignal<String>,
    set_dialog_message: WriteSignal<String>,
    set_dialog_confirm_text: WriteSignal<String>,
    set_dialog_destructive: WriteSignal<bool>,
    set_pending_confirm_action: WriteSignal<Option<ConfirmAction>>,
) -> impl IntoView {
    let current_tier = info.tier.clone();
    let status = info.status.clone();
    let period_end = info.period_end.clone();
    let user_limit = info.user_limit;

    // Fields for AI and analytics sections
    let ai_token_balance = info.ai_token_balance_cents.unwrap_or(0);
    let analytics_events_used = info.analytics_events_used.unwrap_or(0).max(0) as u64;
    let analytics_bundle_balance = info.analytics_bundle_balance.unwrap_or(0);

    // Clones for closures
    let tier_for_status = current_tier.clone();
    let tier_for_seats = current_tier.clone();
    let tier_for_invoices = current_tier.clone();
    let status_for_badge = status.clone();
    let status_for_info = status.clone();
    let status_for_actions = status.clone();
    let status_for_seats = status.clone();

    let is_subscribed = current_tier == "cloud";
    let is_free = current_tier == "free";

    view! {
        <div class="space-y-6" style:display="block">
            // Current Plan Card
            <Card>
                <CardHeader>
                    <div class="flex items-start justify-between">
                        <div>
                            <CardTitle>"Current Plan"</CardTitle>
                            <CardDescription class="mt-1">
                                {if is_subscribed {
                                    format!("Cloud Plan \u{2014} ${:.0}/user/month", PRICE_PER_USER)
                                } else {
                                    "Free Plan".to_string()
                                }}
                            </CardDescription>
                        </div>
                        <div class="flex items-center gap-3">
                            // Status Badge
                            {(is_subscribed && !status_for_badge.is_empty()).then(|| {
                                let (variant, label) = match status_for_badge.as_str() {
                                    "active" => (StatusBadgeVariant::Success, "Active"),
                                    "trialing" => (StatusBadgeVariant::Info, "Trial"),
                                    "cancelled" => (StatusBadgeVariant::Warning, "Cancelled"),
                                    "past_due" => (StatusBadgeVariant::Error, "Past Due"),
                                    _ => (StatusBadgeVariant::Default, status_for_badge.as_str()),
                                };
                                let label_owned = label.to_string();
                                view! {
                                    <StatusBadge variant=variant>
                                        {label_owned}
                                    </StatusBadge>
                                }
                            })}
                            // Manage Billing Button
                            {is_subscribed.then(|| view! {
                                <Button
                                    variant=ButtonVariant::Outline
                                    size=ButtonSize::Sm
                                    disabled=checkout_loading.get_untracked()
                                    on:click=move |_| { handle_manage_billing.dispatch(()); }
                                >
                                    <Icon icon=icondata_lu::LuCreditCard width="16" height="16" attr:class="mr-2"/>
                                    "Manage Billing"
                                </Button>
                            })}
                        </div>
                    </div>
                </CardHeader>
                <CardContent>
                    <div class="space-y-4">
                        // Subscription details (subscribed users only)
                        {(tier_for_status == "cloud" && period_end.is_some()).then(|| {
                            let period_end_for_active = period_end.clone();
                            let period_end_for_cancelled = period_end.clone();
                            let user_count = user_limit.unwrap_or(1);

                            view! {
                                <div class="bg-muted/50 border border-border rounded-lg p-4 space-y-2">
                                    // User count row
                                    <div class="flex justify-between text-sm">
                                        <span class="text-muted-foreground">"User seats"</span>
                                        <span class="font-medium text-foreground">
                                            {format!("{} {}", user_count, if user_count == 1 { "user" } else { "users" })}
                                        </span>
                                    </div>

                                    {(status_for_info == "active" || status_for_info == "trialing").then(move || {
                                        period_end_for_active.as_ref().map(|pe| {
                                            let next_charge = format_charge(user_count);
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
                                                        "Your subscription has been cancelled. You\u{2019}ll retain access until the end of your billing period."
                                                    </div>
                                                </div>
                                            }
                                        })
                                    })}
                                </div>
                            }
                        })}

                        // Action Buttons
                        {if is_free {
                            // Free tier — show subscribe CTA
                            view! {
                                <div class="mt-4">
                                    <Button
                                        variant=ButtonVariant::Default
                                        disabled=checkout_loading.get_untracked()
                                        on:click=move |_| {
                                            handle_subscribe.dispatch(1);
                                        }
                                    >
                                        {format!("Subscribe \u{2014} ${:.0}/user/month", PRICE_PER_USER)}
                                    </Button>
                                    <p class="text-xs text-muted-foreground mt-2">
                                        "Includes a 30-day free trial. All features included."
                                    </p>
                                </div>
                            }.into_any()
                        } else {
                            let status_cl = status.clone();
                            view! {
                                <div class="flex gap-2 mt-4">
                                    {(status_cl == "active" || status_cl == "trialing").then(|| view! {
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
                            }.into_any()
                        }}
                    </div>
                </CardContent>
            </Card>

            // User Seats Card (visible for active/trialing Cloud subscriptions)
            {(tier_for_seats == "cloud" && (status_for_seats == "active" || status_for_seats == "trialing")).then(|| {
                let user_limit_seats = user_limit.unwrap_or(1);

                view! {
                    <UserSeatsCard
                        user_limit=user_limit_seats
                        desired_team_size=desired_team_size
                        set_desired_team_size=set_desired_team_size
                        team_size_loading=team_size_loading
                        handle_team_size_update=handle_team_size_update
                    />
                }
            })}

            // AI Credits Card (visible for subscribed users)
            {is_subscribed.then(|| {
                view! {
                    <AiCreditsCard
                        token_balance_cents=ai_token_balance
                        handle_purchase_ai=handle_purchase_ai
                    />
                }
            })}

            // Analytics Card (visible for subscribed users)
            {is_subscribed.then(|| {
                view! {
                    <AnalyticsCard
                        events_used=analytics_events_used
                        bundle_balance=analytics_bundle_balance
                        handle_purchase_analytics=handle_purchase_analytics
                    />
                }
            })}

            // Invoices Section
            <InvoicesSection invoices=invoices current_tier=tier_for_invoices.clone()/>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// User Seats Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn UserSeatsCard(
    user_limit: i32,
    desired_team_size: ReadSignal<i32>,
    set_desired_team_size: WriteSignal<i32>,
    team_size_loading: ReadSignal<bool>,
    handle_team_size_update: Action<i32, ()>,
) -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    <Icon icon=icondata_lu::LuUsers width="20" height="20"/>
                    "User Seats"
                </CardTitle>
                <CardDescription>
                    {format!(
                        "Manage your user seats. Each seat costs ${:.0}/month.",
                        PRICE_PER_USER
                    )}
                </CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    // Current seat info
                    <div class="bg-muted/50 border border-border rounded-lg p-4">
                        <div class="flex justify-between text-sm">
                            <span class="text-muted-foreground">"Current seats"</span>
                            <span class="font-medium text-foreground">
                                {format!("{} {}", user_limit, if user_limit == 1 { "user" } else { "users" })}
                            </span>
                        </div>
                        <div class="flex justify-between text-sm mt-1">
                            <span class="text-muted-foreground">"Monthly cost"</span>
                            <span class="font-medium text-foreground">
                                {format_charge(user_limit)}
                            </span>
                        </div>
                    </div>

                    // Seat count adjuster
                    <div>
                        <label class="text-sm font-medium text-foreground block mb-2">
                            "Adjust seats"
                        </label>
                        <div class="flex items-center gap-3">
                            <button
                                class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground h-8 rounded-md px-3 text-xs"
                                disabled=move || desired_team_size.get() <= 1 || team_size_loading.get()
                                on:click=move |_| {
                                    set_desired_team_size.update(|v| *v = (*v - 1).max(1));
                                }
                            >
                                <Icon icon=icondata_lu::LuMinus width="16" height="16"/>
                            </button>
                            <input
                                type="number"
                                prop:value=move || desired_team_size.get().to_string()
                                on:input=move |ev| {
                                    let val: i32 = event_target_value(&ev)
                                        .parse()
                                        .unwrap_or(1)
                                        .max(1);
                                    set_desired_team_size.set(val);
                                }
                                min="1"
                                class="w-20 px-3 py-2 text-center border border-border rounded-md bg-background text-foreground"
                                disabled=move || team_size_loading.get()
                            />
                            <button
                                class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground h-8 rounded-md px-3 text-xs"
                                disabled=move || team_size_loading.get()
                                on:click=move |_| {
                                    set_desired_team_size.update(|v| *v += 1);
                                }
                            >
                                <Icon icon=icondata_lu::LuPlus width="16" height="16"/>
                            </button>
                            <span class="text-sm text-muted-foreground">"users"</span>
                        </div>
                    </div>

                    // Cost preview (when seat count differs from current)
                    {move || {
                        let desired = desired_team_size.get();
                        (desired != user_limit).then(|| {
                            let total = PRICE_PER_USER * desired as f64;
                            let is_increase = desired > user_limit;

                            view! {
                                <div class="bg-primary/10 border border-primary/20 rounded-lg p-4">
                                    <div class="text-sm space-y-2">
                                        <div class="flex justify-between">
                                            <span class="text-foreground">
                                                {format!("{} {} \u{00d7} ${:.0}/mo", desired, if desired == 1 { "user" } else { "users" }, PRICE_PER_USER)}
                                            </span>
                                            <span class="font-medium text-foreground">
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

                    // Update button
                    <button
                        class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 bg-primary text-primary-foreground shadow hover:bg-primary/90 h-9 px-4 py-2 w-full"
                        disabled=move || desired_team_size.get() == user_limit || team_size_loading.get()
                        on:click=move |_| {
                            handle_team_size_update.dispatch(desired_team_size.get_untracked());
                        }
                    >
                        {move || if team_size_loading.get() { "Updating..." } else { "Update Seats" }}
                    </button>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AI Credits Card
// ─────────────────────────────────────────────────────────────────────────────

/// Check localStorage for a BYOK API key.
fn check_byok_key() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        use crate::pages::settings::ai_provider::LLM_CONFIG_STORAGE_KEY;
        let storage = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten());
        if let Some(storage) = storage {
            if let Ok(Some(val)) = storage.get_item(LLM_CONFIG_STORAGE_KEY) {
                if let Ok(parsed) = js_sys::JSON::parse(&val) {
                    let key = js_sys::Reflect::get(
                        &parsed,
                        &wasm_bindgen::JsValue::from_str("api_key"),
                    )
                    .ok()
                    .and_then(|v| v.as_string());
                    return key.map_or(false, |k| !k.is_empty());
                }
            }
        }
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    false
}

#[component]
fn AiCreditsCard(
    token_balance_cents: i64,
    handle_purchase_ai: Action<(), ()>,
) -> impl IntoView {
    let balance_dollars = token_balance_cents as f64 / 100.0;

    // Detect BYOK key from localStorage (client-side only).
    let has_byok_key = Signal::derive(check_byok_key);

    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    <Icon icon=icondata_lu::LuSparkles width="20" height="20"/>
                    "AI Credits"
                </CardTitle>
                <CardDescription>
                    "Use your own API key (BYOK) or purchase token bundles."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    // BYOK Key status
                    <div class="bg-muted/50 border border-border rounded-lg p-4">
                        <div class="flex justify-between items-center text-sm">
                            <div class="flex items-center gap-2">
                                <Icon icon=icondata_lu::LuKey width="16" height="16"/>
                                <span class="text-muted-foreground">"BYOK API Key"</span>
                            </div>
                            {move || if has_byok_key.get() {
                                view! {
                                    <StatusBadge variant=StatusBadgeVariant::Success>
                                        "Configured"
                                    </StatusBadge>
                                }.into_any()
                            } else {
                                view! {
                                    <StatusBadge variant=StatusBadgeVariant::Default>
                                        "Not configured"
                                    </StatusBadge>
                                }.into_any()
                            }}
                        </div>
                    </div>

                    // Token bundle balance
                    <div class="bg-muted/50 border border-border rounded-lg p-4">
                        <div class="flex justify-between items-center text-sm">
                            <span class="text-muted-foreground">"Token Bundle Balance"</span>
                            <span class="font-medium text-foreground">
                                {format!("${:.2} remaining", balance_dollars)}
                            </span>
                        </div>
                        <p class="text-xs text-muted-foreground mt-2">
                            "Token bundles never expire."
                        </p>
                    </div>

                    // Purchase button
                    <Button
                        variant=ButtonVariant::Outline
                        class="w-full"
                        disabled=Signal::derive(move || handle_purchase_ai.pending().get())
                        on:click=move |_| { handle_purchase_ai.dispatch(()); }
                    >
                        {move || if handle_purchase_ai.pending().get() { "Redirecting..." } else { "Buy AI Tokens" }}
                    </Button>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Analytics Card
// ─────────────────────────────────────────────────────────────────────────────

/// Format a large number with comma separators (e.g. 100000 -> "100,000").
pub(crate) fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

#[component]
fn AnalyticsCard(
    events_used: u64,
    bundle_balance: i64,
    handle_purchase_analytics: Action<(), ()>,
) -> impl IntoView {
    let total_available = ANALYTICS_EVENTS_INCLUDED as i64 + bundle_balance.max(0);
    let usage_pct = if total_available > 0 {
        ((events_used as f64 / total_available as f64) * 100.0).min(100.0)
    } else {
        0.0
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    <Icon icon=icondata_lu::LuChartBar width="20" height="20"/>
                    "Analytics"
                </CardTitle>
                <CardDescription>
                    {format!("{} events/month included. Purchase bundles for additional capacity.", format_number(ANALYTICS_EVENTS_INCLUDED))}
                </CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    // Usage this month
                    <div class="bg-muted/50 border border-border rounded-lg p-4 space-y-3">
                        <div class="flex justify-between text-sm">
                            <span class="text-muted-foreground">"Events this month"</span>
                            <span class="font-medium text-foreground">
                                {format!("{} / {}", format_number(events_used), format_number(ANALYTICS_EVENTS_INCLUDED))}
                            </span>
                        </div>
                        // Progress bar
                        <div class="w-full bg-border rounded-full h-2">
                            <div
                                class="bg-primary rounded-full h-2 transition-all"
                                style:width=format!("{}%", usage_pct)
                            />
                        </div>
                    </div>

                    // Bundle balance
                    <div class="bg-muted/50 border border-border rounded-lg p-4">
                        <div class="flex justify-between items-center text-sm">
                            <span class="text-muted-foreground">"Bundle balance"</span>
                            <span class="font-medium text-foreground">
                                {format!("{} events remaining", format_number(bundle_balance.max(0) as u64))}
                            </span>
                        </div>
                        <p class="text-xs text-muted-foreground mt-2">
                            "Event bundles never expire."
                        </p>
                    </div>

                    // Purchase button
                    <Button
                        variant=ButtonVariant::Outline
                        class="w-full"
                        disabled=Signal::derive(move || handle_purchase_analytics.pending().get())
                        on:click=move |_| { handle_purchase_analytics.dispatch(()); }
                    >
                        {move || if handle_purchase_analytics.pending().get() { "Redirecting..." } else { "Buy Event Bundle" }}
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
                                                                                    <Icon icon=icondata_lu::LuFileText width="16" height="16"/>
                                                                                    <span>"PDF"</span>
                                                                                    <Icon icon=icondata_lu::LuExternalLink width="12" height="12"/>
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
