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
use phosphor_leptos::Icon;
use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, ConfirmDialog, EmptyState, Label, Modal, ModalSize,
    Skeleton, StatusBadge, StatusBadgeVariant, INPUT_CLASS,
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
// Embedded checkout helper
// ─────────────────────────────────────────────────────────────────────────────

/// Writable signals the embedded checkout helper drives while a session is
/// open. Bundled into a struct so `open_embedded_checkout` stays below the
/// clippy argument-count threshold.
#[derive(Clone, Copy)]
struct CheckoutContext {
    set_checkout_open: WriteSignal<bool>,
    set_checkout_session_id: WriteSignal<Option<String>>,
    set_checkout_loading: WriteSignal<bool>,
    set_success: WriteSignal<Option<String>>,
    set_error: WriteSignal<Option<String>>,
    set_sub_version: WriteSignal<u32>,
    checkout_handle: StoredValue<Option<send_wrapper::SendWrapper<crate::utils::stripe::EmbeddedCheckoutHandle>>>,
}

/// Fetch the Stripe publishable key, mount the embedded checkout form,
/// and wire up the onComplete callback.
async fn open_embedded_checkout(
    client_secret: &str,
    session_id: &str,
    ctx: CheckoutContext,
) {
    // Fetch the publishable key from the server
    let pk = match get_stripe_publishable_key().await {
        Ok(Some(pk)) => pk,
        Ok(None) => {
            ctx.set_error.set(Some("Stripe is not configured.".into()));
            ctx.set_checkout_loading.set(false);
            return;
        }
        Err(e) => {
            ctx.set_error.set(Some(format!("Failed to load Stripe config: {e}")));
            ctx.set_checkout_loading.set(false);
            return;
        }
    };

    // Store session ID for the onComplete callback
    let sid = session_id.to_string();
    ctx.set_checkout_session_id.set(Some(sid.clone()));
    ctx.set_checkout_open.set(true);
    ctx.set_checkout_loading.set(false);

    // Mount the embedded checkout form (WASM only)
    #[cfg(target_arch = "wasm32")]
    {
        let client_secret = client_secret.to_string();
        leptos::task::spawn_local(async move {
            // Wait for the mount target to appear in DOM
            gloo_timers::future::TimeoutFuture::new(50).await;

            let result = crate::utils::stripe::mount_embedded_checkout(
                &pk,
                &client_secret,
                "#stripe-checkout-mount",
                move || {
                    let sid = sid.clone();
                    leptos::task::spawn_local(async move {
                        let close = || {
                            ctx.checkout_handle.set_value(None);
                            ctx.set_checkout_open.try_set(false);
                            ctx.set_checkout_session_id.try_set(None);
                        };
                        match get_checkout_session_status(sid).await {
                            Ok(status) if status.status == "complete" => {
                                close();
                                ctx.set_success.try_set(Some(
                                    "Payment successful! Your changes are being applied..."
                                        .to_string(),
                                ));
                                ctx.set_sub_version.try_update(|v| *v += 1);
                            }
                            Ok(status) => {
                                close();
                                ctx.set_error.try_set(Some(format!(
                                    "Checkout session status: {}. Please try again.",
                                    status.status
                                )));
                            }
                            Err(e) => {
                                close();
                                ctx.set_error.try_set(Some(format!(
                                    "Failed to verify payment: {e}"
                                )));
                            }
                        }
                    });
                },
            )
            .await;

            match result {
                Ok(handle) => {
                    // Store the handle to keep the checkout form alive
                    ctx.checkout_handle
                        .set_value(Some(send_wrapper::SendWrapper::new(handle)));
                }
                Err(e) => {
                    ctx.set_checkout_open.try_set(false);
                    ctx.set_checkout_session_id.try_set(None);
                    ctx.set_error
                        .try_set(Some(format!("Failed to mount checkout form: {e}")));
                }
            }
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (
            client_secret,
            ctx.set_checkout_open,
            ctx.set_checkout_session_id,
            ctx.set_checkout_loading,
            ctx.set_success,
            ctx.set_error,
            ctx.set_sub_version,
            ctx.checkout_handle,
            pk,
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

/// Billing settings page content.
#[component]
pub fn BillingPage() -> impl IntoView {
    // Read the UserContext provided by SettingsShell to check deployment mode.
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

    // Resources for subscription info (reload via version signal).
    // Uses LocalResource to avoid SSR running these without auth cookies
    // (SameSite=Strict blocks cookies on cross-site redirects from Stripe/Google).
    let (sub_version, set_sub_version) = signal(0u32);
    let subscription = LocalResource::new(move || {
        let _ = sub_version.get();
        get_subscription_info()
    });

    let invoices = LocalResource::new(get_invoices);

    // UI state
    let (error, set_error) = signal(Option::<String>::None);
    let (success, set_success) = signal(Option::<String>::None);
    let (checkout_loading, set_checkout_loading) = signal(false);

    // Embedded checkout state — when Some, the Stripe checkout form is mounted inline.
    // `set_checkout_session_id` is driven by `open_embedded_checkout` purely to track
    // which session is in flight; nothing in the view reads it, so the read half is
    // deliberately discarded.
    let (checkout_open, set_checkout_open) = signal(false);
    let (_, set_checkout_session_id) = signal(Option::<String>::None);

    // Store the checkout handle to keep it alive while the form is visible.
    // Dropping the handle calls destroy() which unmounts the Stripe form.
    // Uses StoredValue because EmbeddedCheckoutHandle contains JS objects (not Send).
    let checkout_handle: StoredValue<Option<send_wrapper::SendWrapper<crate::utils::stripe::EmbeddedCheckoutHandle>>> =
        StoredValue::new(None);

    let checkout_ctx = CheckoutContext {
        set_checkout_open,
        set_checkout_session_id,
        set_checkout_loading,
        set_success,
        set_error,
        set_sub_version,
        checkout_handle,
    };

    // Confirm dialog state
    let dialog_open = RwSignal::new(false);
    let (dialog_title, set_dialog_title) = signal(String::new());
    let (dialog_message, set_dialog_message) = signal(String::new());
    let (dialog_confirm_text, set_dialog_confirm_text) = signal("Confirm".to_string());
    let (dialog_destructive, set_dialog_destructive) = signal(false);
    let (pending_confirm_action, set_pending_confirm_action) =
        signal(Option::<ConfirmAction>::None);

    // ── Actions ──────────────────────────────────────────────────────────

    let handle_subscribe = Action::new({
        move |quantity: &u64| {
            let quantity = *quantity;
            async move {
                set_checkout_loading.set(true);
                set_error.set(None);
                match create_checkout(quantity).await {
                    Ok(CheckoutOutcome::Embedded(session)) => {
                        open_embedded_checkout(
                            &session.client_secret,
                            &session.session_id,
                            checkout_ctx,
                        )
                        .await;
                    }
                    Ok(CheckoutOutcome::Modified(msg)) => {
                        set_success.set(Some(msg));
                        set_sub_version.update(|v| *v += 1);
                        set_checkout_loading.set(false);
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

    let handle_manage_billing = Action::new({
        move |(): &()| async move {
            set_checkout_loading.set(true);
            set_error.set(None);
            match create_portal_session().await {
                Ok(_redirect) => {
                    #[cfg(target_arch = "wasm32")]
                    {
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href(&_redirect.url);
                        }
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
        move |quantity: &u32| {
            let q = *quantity;
            async move {
                set_checkout_loading.set(true);
                set_error.set(None);
                match purchase_ai_bundle(q).await {
                    Ok(session) => {
                        open_embedded_checkout(
                            &session.client_secret,
                            &session.session_id,
                            checkout_ctx,
                        )
                        .await;
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to start AI bundle purchase: {e}")));
                        set_checkout_loading.set(false);
                    }
                }
            }
        }
    });

    let handle_purchase_analytics = Action::new({
        move |quantity: &u32| {
            let q = *quantity;
            async move {
                set_checkout_loading.set(true);
                set_error.set(None);
                match purchase_analytics_bundle(q).await {
                    Ok(session) => {
                        open_embedded_checkout(
                            &session.client_secret,
                            &session.session_id,
                            checkout_ctx,
                        )
                        .await;
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to start analytics bundle purchase: {e}")));
                        set_checkout_loading.set(false);
                    }
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
        // Check user context for self-hosted mode and owner gate.
        // Billing is owner-only — non-owners see a helpful message instead.
        <Transition fallback=move || view! {
            <div class="flex items-center justify-center p-8">
                <Skeleton class="h-8 w-8 rounded-full"/>
            </div>
        }>
            {move || Suspend::new(async move {
                let ctx_result = user_ctx.await;
                if let Ok(ctx) = ctx_result {
                    if ctx.is_self_hosted {
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
                    if !ctx.is_owner {
                        let is_expired = matches!(
                            ctx.subscription_status.as_str(),
                            "past_due" | "cancelled" | "canceled" | "unpaid" | "incomplete_expired"
                        );
                        let (heading, body) = if is_expired {
                            (
                                "Workspace subscription expired",
                                "Your workspace subscription has lapsed. Contact your workspace owner to reactivate it — only the owner can make billing changes.",
                            )
                        } else {
                            (
                                "Billing is owner-only",
                                "Only the workspace owner can manage subscriptions and purchases. Contact your workspace owner for billing changes.",
                            )
                        };
                        return view! {
                            <Card>
                                <CardContent>
                                    <div class="py-6 text-center space-y-2">
                                        <p class="text-foreground font-medium">{heading}</p>
                                        <p class="text-muted-foreground text-sm">{body}</p>
                                    </div>
                                </CardContent>
                            </Card>
                        }.into_any();
                    }
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

                        // Embedded checkout modal — center-overlay with backdrop
                        <Modal
                            show=Signal::derive(move || checkout_open.get())
                            on_close=Callback::new(move |()| {
                                // Destroy the Stripe form and close the modal
                                checkout_handle.set_value(None);
                                set_checkout_open.set(false);
                                set_checkout_session_id.set(None);
                            })
                            title="Complete Payment".to_string()
                            size=ModalSize::Lg
                        >
                            <div id="stripe-checkout-mount" class="min-h-[400px]"/>
                        </Modal>

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
                                                handle_subscribe=handle_subscribe
                                                handle_manage_billing=handle_manage_billing
                                                handle_purchase_ai=handle_purchase_ai
                                                handle_purchase_analytics=handle_purchase_analytics
                                                dialog_open=dialog_open
                                                set_dialog_title=set_dialog_title
                                                set_dialog_message=set_dialog_message
                                                set_dialog_confirm_text=set_dialog_confirm_text
                                                set_dialog_destructive=set_dialog_destructive
                                                set_pending_confirm_action=set_pending_confirm_action
                                                set_sub_version=set_sub_version
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
            })}
        </Transition>
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
    invoices: LocalResource<Result<Vec<InvoiceRecord>, ServerFnError>>,
    checkout_loading: ReadSignal<bool>,
    handle_subscribe: Action<u64, ()>,
    handle_manage_billing: Action<(), ()>,
    handle_purchase_ai: Action<u32, ()>,
    handle_purchase_analytics: Action<u32, ()>,
    dialog_open: RwSignal<bool>,
    set_dialog_title: WriteSignal<String>,
    set_dialog_message: WriteSignal<String>,
    set_dialog_confirm_text: WriteSignal<String>,
    set_dialog_destructive: WriteSignal<bool>,
    set_pending_confirm_action: WriteSignal<Option<ConfirmAction>>,
    set_sub_version: WriteSignal<u32>,
) -> impl IntoView {
    let current_tier = info.tier.clone();
    let status = info.status.clone();
    let period_end = info.period_end.clone();
    let trial_ends_at = info.trial_ends_at.clone();
    let active_members = info.active_members;
    let user_limit = info.user_limit;

    // Fields for AI and analytics sections
    let ai_token_balance = info.ai_token_balance_cents.unwrap_or(0);
    let analytics_events_used = info.analytics_events_used.unwrap_or(0).max(0) as u64;
    let analytics_bundle_balance = info.analytics_bundle_balance.unwrap_or(0);

    // Clones for closures
    let tier_for_status = current_tier.clone();
    let tier_for_invoices = current_tier.clone();
    let status_for_badge = status.clone();
    let status_for_info = status.clone();
    let status_for_actions = status.clone();

    // Trialing users with a Stripe subscription (period_end present) have a
    // Stripe-managed trial. Those without (no period_end) have an app-managed
    // trial fallback.
    let is_stripe_trial = status == "trialing" && period_end.is_some();
    let is_app_trial = status == "trialing" && period_end.is_none();
    let is_free = current_tier == "free";
    // Subscribed = has a Stripe subscription (active, trialing with Stripe, or cancelled)
    let is_subscribed = !is_free && !is_app_trial;

    view! {
        <div class="space-y-6" style:display="block">
            // Current Plan Card
            <Card>
                <CardHeader>
                    <div class="flex items-start justify-between">
                        <div>
                            <CardTitle>"Current Plan"</CardTitle>
                            <CardDescription class="mt-1">
                                {if is_subscribed || is_app_trial {
                                    format!("Cloud Plan \u{2014} ${:.0}/user/month", PRICE_PER_USER)
                                } else {
                                    "Free Plan".to_string()
                                }}
                            </CardDescription>
                        </div>
                        <div class="flex items-center gap-3">
                            // Status Badge
                            {((is_subscribed || is_app_trial) && !status_for_badge.is_empty()).then(|| {
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
                                    <Icon icon=phosphor_leptos::CREDIT_CARD size="16px" attr:class="mr-2"/>
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
                            let user_count = active_members;

                            view! {
                                <div class="bg-muted/50 border border-border rounded-lg p-4 space-y-2">
                                    // User count row
                                    <div class="flex justify-between text-sm">
                                        <span class="text-muted-foreground">"Active users"</span>
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

                        // Trial nudge banner — shown after day 7 of a Stripe trial
                        {is_stripe_trial.then(|| {
                            let trial_end = trial_ends_at.as_deref()
                                .or(period_end.as_deref())
                                .and_then(|te| chrono::DateTime::parse_from_rfc3339(te).ok());
                            let days_left = trial_end.map(|dt| {
                                dt.signed_duration_since(chrono::Utc::now()).num_days().max(0)
                            }).unwrap_or(0);
                            // Show nudge after day 7 (i.e. < 23 days remaining of 30)
                            (days_left < 23).then(|| view! {
                                <Alert variant=AlertVariant::Info class="mt-2">
                                    <AlertDescription>
                                        "Add a payment method to continue after your trial."
                                    </AlertDescription>
                                </Alert>
                            })
                        })}

                        // Action Buttons
                        {if is_free || is_app_trial {
                            // Free tier or app-managed trial — show subscribe CTA
                            let trial_msg = if is_app_trial {
                                trial_ends_at.as_deref().and_then(|te| {
                                    chrono::DateTime::parse_from_rfc3339(te).ok().map(|dt| {
                                        let days_left = (dt.signed_duration_since(chrono::Utc::now())).num_days().max(0);
                                        if days_left == 0 {
                                            "Your trial has expired. Subscribe to continue using Kyomi.".to_string()
                                        } else {
                                            format!("Trial \u{2014} {days_left} day{} remaining", if days_left == 1 { "" } else { "s" })
                                        }
                                    })
                                }).unwrap_or_else(|| "Trial active".to_string())
                            } else {
                                "Includes a 30-day free trial. All features included.".to_string()
                            };
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
                                        {trial_msg}
                                    </p>
                                </div>
                            }.into_any()
                        } else if is_stripe_trial {
                            // Stripe-managed trial — show "Add Payment Method" (opens portal)
                            // Do NOT show "Cancel Subscription" for trialing users
                            view! {
                                <div class="mt-4">
                                    <Button
                                        variant=ButtonVariant::Default
                                        disabled=checkout_loading.get_untracked()
                                        on:click=move |_| { handle_manage_billing.dispatch(()); }
                                    >
                                        <Icon icon=phosphor_leptos::CREDIT_CARD size="16px" attr:class="mr-2"/>
                                        "Add Payment Method"
                                    </Button>
                                </div>
                            }.into_any()
                        } else {
                            let status_cl = status.clone();
                            view! {
                                <div class="flex gap-2 mt-4">
                                    // Only show Cancel for active (not trialing) subscriptions
                                    {(status_cl == "active").then(|| view! {
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
                                    // Expired/past_due: show subscribe button
                                    {(status_cl == "past_due").then(|| view! {
                                        <Button
                                            variant=ButtonVariant::Default
                                            disabled=checkout_loading.get_untracked()
                                            on:click=move |_| { handle_manage_billing.dispatch(()); }
                                        >
                                            "Update Payment Method"
                                        </Button>
                                    })}
                                </div>
                            }.into_any()
                        }}
                    </div>
                </CardContent>
            </Card>

            // Seat Cap Card (visible for subscribed users)
            {is_subscribed.then(|| {
                view! {
                    <SeatCapCard
                        user_limit=user_limit
                        active_members=active_members
                        set_sub_version=set_sub_version
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
// Seat Cap Card
// ─────────────────────────────────────────────────────────────────────────────

/// DB sentinel that represents "unlimited seats" — see `update_user_limit`.
const UNLIMITED_CAP: i32 = 999_999;

#[component]
fn SeatCapCard(
    user_limit: Option<i32>,
    active_members: i32,
    set_sub_version: WriteSignal<u32>,
) -> impl IntoView {
    let cap = user_limit.unwrap_or(UNLIMITED_CAP);
    let is_unlimited = cap >= UNLIMITED_CAP;

    let (editing, set_editing) = signal(false);
    let (new_cap_input, set_new_cap_input) = signal(cap.to_string());
    let (error_msg, set_error_msg) = signal(Option::<String>::None);

    // Reset input when the displayed value changes (e.g. after save)
    Effect::new(move |_| {
        let _ = editing.get();
        set_new_cap_input.set(cap.to_string());
        set_error_msg.set(None);
    });

    let save_action = Action::new(move |new_limit: &i32| {
        let new_limit = *new_limit;
        async move {
            match update_user_limit(new_limit).await {
                Ok(_) => {
                    set_editing.set(false);
                    set_error_msg.set(None);
                    set_sub_version.update(|v| *v += 1);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    let usage_pct = if is_unlimited || cap <= 0 {
        0.0
    } else {
        ((active_members as f64 / cap as f64) * 100.0).min(100.0)
    };
    let bar_class = if is_unlimited {
        "h-2 rounded-full transition-all bg-success-foreground"
    } else if usage_pct >= 90.0 {
        "h-2 rounded-full transition-all bg-error-foreground"
    } else if usage_pct >= 75.0 {
        "h-2 rounded-full transition-all bg-warning-foreground"
    } else {
        "h-2 rounded-full transition-all bg-success-foreground"
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::USERS size="20px"/>
                    "Seat Cap"
                </CardTitle>
                <CardDescription>
                    {format!(
                        "You're billed for active users only — ${:.0}/user/month. The seat cap limits how many users admins can invite, not how much you're charged.",
                        PRICE_PER_USER
                    )}
                </CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    // Usage bar
                    <div>
                        <div class="flex justify-between items-baseline mb-2">
                            <span class="text-sm font-medium text-foreground">
                                "Active users"
                            </span>
                            <span class="font-mono tabular-nums text-sm font-medium text-foreground">
                                {if is_unlimited {
                                    format!("{active_members} / Unlimited")
                                } else {
                                    format!("{active_members} / {cap}")
                                }}
                            </span>
                        </div>
                        {(!is_unlimited).then(|| view! {
                            <div class="w-full bg-muted rounded-full h-2">
                                <div
                                    class=bar_class
                                    style:width=format!("{}%", usage_pct)
                                />
                            </div>
                        })}
                        <p class="text-xs text-muted-foreground mt-2">
                            {format!(
                                "Current billing: {} active user{} × ${:.0}/month = ${:.0}/month",
                                active_members,
                                if active_members == 1 { "" } else { "s" },
                                PRICE_PER_USER,
                                PRICE_PER_USER * active_members as f64,
                            )}
                        </p>
                    </div>

                    // Edit controls
                    {move || if editing.get() {
                        view! {
                            <div class="space-y-2 pt-3 border-t border-border">
                                <Label>"New seat cap"</Label>
                                <p class="text-xs text-muted-foreground">
                                    "Set to 999,999 for unlimited. This controls how many users can be invited, not what you're billed."
                                </p>
                                <div class="flex gap-2">
                                    <input
                                        type="number"
                                        class=INPUT_CLASS
                                        prop:value=move || new_cap_input.get()
                                        on:input=move |ev| set_new_cap_input.set(event_target_value(&ev))
                                        min=active_members.to_string()
                                    />
                                    <Button
                                        variant=ButtonVariant::Default
                                        on:click=move |_| {
                                            if let Ok(n) = new_cap_input.get_untracked().parse::<i32>() {
                                                save_action.dispatch(n);
                                            } else {
                                                set_error_msg.set(Some("Please enter a valid number".to_string()));
                                            }
                                        }
                                    >
                                        "Save"
                                    </Button>
                                    <Button
                                        variant=ButtonVariant::Outline
                                        on:click=move |_| {
                                            set_editing.set(false);
                                            set_error_msg.set(None);
                                        }
                                    >
                                        "Cancel"
                                    </Button>
                                </div>
                                {move || error_msg.get().map(|msg| view! {
                                    <p class="text-xs text-error-foreground">{msg}</p>
                                })}
                                <div class="flex gap-2">
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::Sm
                                        on:click=move |_| {
                                            save_action.dispatch(UNLIMITED_CAP);
                                        }
                                    >
                                        "Set unlimited"
                                    </Button>
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="pt-3 border-t border-border">
                                <Button
                                    variant=ButtonVariant::Outline
                                    size=ButtonSize::Sm
                                    on:click=move |_| set_editing.set(true)
                                >
                                    <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="14px" attr:class="mr-2"/>
                                    "Change cap"
                                </Button>
                            </div>
                        }.into_any()
                    }}
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AI Credits Card
// ─────────────────────────────────────────────────────────────────────────────

/// Price per AI token bundle (matches AI_BUNDLE_CREDIT_USD default).
const AI_BUNDLE_PRICE: f64 = 10.0;

/// Predefined quantity options for bundle purchases.
/// Quantity options for bundle purchase StaticSelect.
const BUNDLE_QUANTITY_OPTIONS: [(&str, &str); 5] = [
    ("1", "1"), ("2", "2"), ("3", "3"), ("5", "5"), ("10", "10"),
];

#[component]
fn AiCreditsCard(
    token_balance_cents: i64,
    handle_purchase_ai: Action<u32, ()>,
) -> impl IntoView {
    let balance_dollars = token_balance_cents as f64 / 100.0;
    let (ai_quantity, set_ai_quantity) = signal(1u32);

    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::SPARKLE size="20px"/>
                    "AI Credits"
                </CardTitle>
                <CardDescription>
                    "Purchase token bundles to power Kyomi AI. Workspace BYOK (bring your own key) is configured under Settings \u{2192} AI."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
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

                    // Quantity selector + price summary
                    <div class="flex items-center gap-3">
                        <label class="text-sm text-muted-foreground">"Quantity"</label>
                        <div class="w-20">
                            <crate::components::StaticSelect
                                value="1".to_string()
                                options=BUNDLE_QUANTITY_OPTIONS.to_vec()
                                on_change=move |val: String| {
                                    let q: u32 = val.parse().unwrap_or(1);
                                    set_ai_quantity.set(q);
                                }
                            />
                        </div>
                        <span class="text-sm text-foreground">
                            {move || {
                                let q = ai_quantity.get();
                                let total = AI_BUNDLE_PRICE * q as f64;
                                format!("{q} \u{00d7} ${:.0} = ${:.0} of AI credits", AI_BUNDLE_PRICE, total)
                            }}
                        </span>
                    </div>

                    // Purchase button
                    <Button
                        variant=ButtonVariant::Outline
                        class="w-full"
                        disabled=Signal::derive(move || handle_purchase_ai.pending().get())
                        on:click=move |_| { handle_purchase_ai.dispatch(ai_quantity.get_untracked()); }
                    >
                        {move || if handle_purchase_ai.pending().get() { "Loading...".to_string() } else {
                            let q = ai_quantity.get();
                            let total = AI_BUNDLE_PRICE * q as f64;
                            format!("Buy AI Tokens \u{2014} ${:.0}", total)
                        }}
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

/// Events per analytics bundle (matches ANALYTICS_BUNDLE_CREDIT_EVENTS default).
const ANALYTICS_BUNDLE_EVENTS: u64 = 1_000_000;

/// Price per analytics event bundle in USD (matches STRIPE_ANALYTICS_BUNDLE Stripe price).
const ANALYTICS_BUNDLE_PRICE: f64 = 10.0;

#[component]
fn AnalyticsCard(
    events_used: u64,
    bundle_balance: i64,
    handle_purchase_analytics: Action<u32, ()>,
) -> impl IntoView {
    let total_available = ANALYTICS_EVENTS_INCLUDED as i64 + bundle_balance.max(0);
    let usage_pct = if total_available > 0 {
        ((events_used as f64 / total_available as f64) * 100.0).min(100.0)
    } else {
        0.0
    };

    let (analytics_quantity, set_analytics_quantity) = signal(1u32);

    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::CHART_BAR size="20px"/>
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

                    // Quantity selector + summary
                    <div class="flex items-center gap-3">
                        <label class="text-sm text-muted-foreground">"Quantity"</label>
                        <div class="w-20">
                            <crate::components::StaticSelect
                                value="1".to_string()
                                options=BUNDLE_QUANTITY_OPTIONS.to_vec()
                                on_change=move |val: String| {
                                    let q: u32 = val.parse().unwrap_or(1);
                                    set_analytics_quantity.set(q);
                                }
                            />
                        </div>
                        <span class="text-sm text-foreground">
                            {move || {
                                let q = analytics_quantity.get();
                                let total_events = ANALYTICS_BUNDLE_EVENTS * q as u64;
                                let total_price = ANALYTICS_BUNDLE_PRICE * q as f64;
                                format!("{q} \u{00d7} ${:.0} = ${:.0} for {} events", ANALYTICS_BUNDLE_PRICE, total_price, format_number(total_events))
                            }}
                        </span>
                    </div>

                    // Purchase button
                    <Button
                        variant=ButtonVariant::Outline
                        class="w-full"
                        disabled=Signal::derive(move || handle_purchase_analytics.pending().get())
                        on:click=move |_| { handle_purchase_analytics.dispatch(analytics_quantity.get_untracked()); }
                    >
                        {move || if handle_purchase_analytics.pending().get() { "Loading...".to_string() } else {
                            let q = analytics_quantity.get();
                            let total_price = ANALYTICS_BUNDLE_PRICE * q as f64;
                            format!("Buy Event Bundle \u{2014} ${:.0}", total_price)
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
    invoices: LocalResource<Result<Vec<InvoiceRecord>, ServerFnError>>,
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
                                                                                    <Icon icon=phosphor_leptos::FILE_TEXT size="16px"/>
                                                                                    <span>"PDF"</span>
                                                                                    <Icon icon=phosphor_leptos::ARROW_SQUARE_OUT size="12px"/>
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
