// SPDX-License-Identifier: AGPL-3.0-or-later

//! Full-screen billing paywall (KYO-806).
//!
//! Rendered by `Layout` INSTEAD of the sidebar + page content whenever the
//! workspace's billing has lapsed — never as a page mounted under the
//! normal route tree, so it can never leak read-only access to the app and
//! never rewrites the address bar (see `Layout`'s `show_paywall`/`<Show>`).
//!
//! Contents, per KYO-806's acceptance criteria:
//! - What happened — copy keyed on the server-computed [`BillingLapseReason`]
//!   ([`lapse_copy`]), never derived from a status string on the client.
//!   Rendered as [`AuthLayout`]'s own title/subtitle (derived reactively from
//!   `get_billing_paywall` by [`BillingPaywall`]) rather than a second `<h1>`
//!   inside the content slot — one heading on the page, matching every other
//!   `AuthLayout` consumer (login, signup, recovery).
//! - A primary action for the workspace owner (`can_manage_billing`):
//!   [`PaywallAction::RecoverPayment`] pays the open invoice on the existing
//!   subscription immediately, [`PaywallAction::Subscribe`] starts a new one
//!   — the server already decided which via `get_billing_paywall`
//!   (`kyomi_auth::subscription_service::checkout_path`); this component
//!   never re-derives that choice.
//! - For a non-owner: "Ask {owner} to update billing" — no pay button.
//! - A workspace switcher, only if the caller belongs to more than one
//!   workspace.
//! - Log out.

use leptos::prelude::*;
use phosphor_leptos::Icon;

use kyomi_types::BillingLapseReason;

use crate::components::toast::toast_error;
use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Modal, ModalSize,
};
use crate::pages::auth::auth_layout::AuthLayout;
use crate::server_fns::billing::{
    create_checkout, get_billing_paywall, start_payment_recovery,
    BillingPaywall as BillingPaywallData, CheckoutOutcome, PaywallAction,
};
// Only referenced from the wasm32-only bodies of `mount_recovery_checkout`/
// `mount_subscribe_checkout` below — importing unconditionally warns
// "unused" on the ssr/native build, which never actually drives an embedded
// checkout to completion.
#[cfg(target_arch = "wasm32")]
use crate::server_fns::billing::{
    complete_payment_recovery, sync_checkout_subscription, PaymentRecoveryOutcome,
};
use crate::server_fns::security::logout;
use crate::server_fns::workspace::{list_my_workspaces, switch_workspace};
use crate::utils::billing_lapse::refetch_billing_state;

/// The paywall's own mount target for the embedded Stripe checkout form —
/// distinct from `pages/settings/billing.rs`'s `#stripe-checkout-mount` so
/// the two IDs can never collide (they're never mounted at the same time in
/// practice — the paywall replaces the settings page entirely — but nothing
/// enforces that at compile time, so distinct ids cost nothing and remove
/// the question).
const CHECKOUT_MOUNT_ID: &str = "paywall-stripe-checkout-mount";

/// "What happened" copy, keyed on the server-computed [`BillingLapseReason`]
/// (KYO-806). Pure and unit-tested (`cargo test -p kyomi-ui --features
/// ssr`) — the client never re-derives a reason from `subscription_status`;
/// this function only maps an already-decided reason to display copy.
///
/// Takes a real [`BillingLapseReason`], not `Option` — `get_billing_paywall`
/// returning `reason: None` means the workspace isn't actually lapsed (the
/// optimistic flag was stale), and [`BillingPaywall`] handles that case
/// itself (clearing the flag and refetching, rendering the loading state
/// instead) before this function is ever called. There is no "not lapsed"
/// copy to fall back to here because that state never reaches `lapse_copy`.
fn lapse_copy(reason: BillingLapseReason) -> (&'static str, &'static str) {
    match reason {
        BillingLapseReason::TrialEnded => (
            "Your trial has ended",
            "Add a payment method to keep going — your workspace's dashboards, chats, and \
             knowledge are all exactly where you left them.",
        ),
        BillingLapseReason::PaymentFailed => (
            "Your payment failed",
            "We weren't able to charge your card for this billing period. Update your payment \
             method to pick up right where you left off.",
        ),
        BillingLapseReason::SubscriptionEnded => (
            "Your subscription has ended",
            "Resubscribe to keep going — your workspace's dashboards, chats, and knowledge are \
             all exactly where you left them.",
        ),
    }
}

/// Owner-only CTA label for [`PaywallAction`] — pure, unit-tested alongside
/// [`lapse_copy`].
fn action_label(action: PaywallAction) -> &'static str {
    match action {
        PaywallAction::RecoverPayment => "Update payment method",
        PaywallAction::Subscribe => "Subscribe now",
    }
}

/// Writable signals the owner's pay-flow drives after the CTA is clicked —
/// everything past "click the CTA" until either success (handled by
/// [`refetch_billing_state`], not a signal here: once the server confirms
/// `billing_lapsed = false`, `Layout` unmounts this component entirely) or
/// a state the owner must act on.
///
/// Bundled into a struct (mirrors `pages/settings/billing.rs`'s
/// `CheckoutContext`) so the mount helpers stay below clippy's
/// argument-count threshold. Deliberately four plain signals rather than
/// one state enum: an enum's variants are only ever constructed inside the
/// `#[cfg(target_arch = "wasm32")]` bodies below (SSR never drives an
/// embedded checkout to completion), which `cargo check --features ssr`
/// correctly flags as "never constructed" dead code for an enum variant —
/// plain signal writes carry no such per-variant liveness analysis.
#[derive(Clone, Copy)]
struct PayFlowSignals {
    checkout_open: WriteSignal<bool>,
    declined_message: WriteSignal<Option<String>>,
    needs_action_shown: WriteSignal<bool>,
    needs_action_url: WriteSignal<Option<String>>,
    checkout_handle:
        StoredValue<Option<send_wrapper::SendWrapper<crate::utils::stripe::EmbeddedCheckoutHandle>>>,
}

impl PayFlowSignals {
    /// Back to the pre-CTA-click state: modal closed, no error/needs-action
    /// banner, checkout handle dropped (which unmounts the Stripe form).
    /// `try_set` throughout: called both synchronously (Modal's `on_close`)
    /// and from inside a `spawn_local` closure that may outlive the
    /// component if the paywall unmounts mid-flight (e.g. `Layout` swaps to
    /// the app shell the instant `refetch_billing_state` resolves
    /// `billing_lapsed = false`).
    fn reset(&self) {
        self.checkout_open.try_set(false);
        self.declined_message.try_set(None);
        self.needs_action_shown.try_set(false);
        self.needs_action_url.try_set(None);
        self.checkout_handle.set_value(None);
    }

    /// Used on both targets — the `#[cfg(not(target_arch = "wasm32"))]`
    /// stub branches of `mount_recovery_checkout`/`mount_subscribe_checkout`
    /// call this too (see their doc comments), so it's a real codepath
    /// under `cargo check --features ssr`, not dead code kept alive only by
    /// wasm32-only callers.
    fn show_declined(&self, message: String) {
        self.reset();
        self.declined_message.try_set(Some(message));
    }
}

/// Shared loading view — the `<Transition>` fallback while `get_billing_paywall`
/// is in flight, and also what [`BillingPaywall`] renders for the one-tick
/// window where the server reports `reason: None` (see `lapse_copy`'s doc
/// comment): both are "we don't have anything to show yet, a fetch is in
/// flight" states, so they share one view instead of two copies of the same
/// markup.
fn paywall_loading() -> impl IntoView {
    view! {
        <div class="text-center py-8 space-y-4">
            <img src="/kyomi_animated_logo.svg" alt="Loading" class="w-12 h-12 mx-auto"/>
            <p class="text-muted-foreground">"Checking your workspace's billing..."</p>
        </div>
    }
}

#[component]
pub fn BillingPaywall() -> impl IntoView {
    let paywall = LocalResource::new(get_billing_paywall);

    // AuthLayout's title/subtitle ARE the lapse heading/explanation — derived
    // reactively from the resource so they update the instant it resolves,
    // and empty while it's still loading (AuthLayout renders an empty `<h1>`/
    // `<p>` for that one tick, which is invisible and briefly-lived, same as
    // every other AuthLayout consumer waiting on its own data). Kept as two
    // small derived signals rather than one `(String, String)` tuple signal
    // so each prop only re-renders the DOM node it actually owns.
    let title = Signal::derive(move || {
        paywall
            .try_get()
            .flatten()
            .and_then(|r| r.ok())
            .and_then(|pw| pw.reason)
            .map(|reason| lapse_copy(reason).0.to_string())
            .unwrap_or_default()
    });
    let subtitle = Signal::derive(move || {
        paywall
            .try_get()
            .flatten()
            .and_then(|r| r.ok())
            .and_then(|pw| pw.reason)
            .map(|reason| lapse_copy(reason).1.to_string())
            .unwrap_or_default()
    });

    // Passed down to `PaywallContent` so its "needs action" retry button
    // (KYO-806 F7) can re-fetch this exact resource, not just kick off
    // Layout's `user_info`/`user_ctx` refetch via `refetch_billing_state`.
    let refetch_paywall = Callback::new(move |()| {
        paywall.refetch();
    });

    view! {
        <AuthLayout title=title subtitle=subtitle>
            <Transition fallback=paywall_loading>
                {move || Suspend::new(async move {
                    match paywall.await {
                        Ok(pw) => match pw.reason {
                            Some(_) => view! {
                                <PaywallContent paywall=pw refetch_paywall=refetch_paywall/>
                            }.into_any(),
                            None => {
                                // The server says this workspace is NOT
                                // lapsed — the optimistic flag was stale
                                // (payment just landed, possibly from
                                // another tab). Clear it and kick off the
                                // authoritative refetch; `Layout` swaps this
                                // component out for the real app shell once
                                // `billing_lapsed` resolves to `false`.
                                // Render the loading state in the meantime,
                                // not fallback copy that would misdescribe
                                // an active workspace as needing billing
                                // action.
                                crate::utils::billing_lapse::clear_optimistic_lapsed();
                                refetch_billing_state();
                                paywall_loading().into_any()
                            }
                        },
                        Err(e) => view! {
                            <Alert variant=AlertVariant::Error>
                                <AlertDescription>
                                    {format!("Couldn't load billing information: {e}")}
                                </AlertDescription>
                            </Alert>
                        }.into_any(),
                    }
                })}
            </Transition>
        </AuthLayout>
    }
}

#[component]
fn PaywallContent(
    paywall: BillingPaywallData,
    /// Re-fetches [`BillingPaywall`]'s own `get_billing_paywall` resource —
    /// see [`BillingPaywall`]'s doc comment on where this is constructed,
    /// and the "needs action" retry button below for its one call site.
    refetch_paywall: Callback<()>,
) -> impl IntoView {
    let can_manage = paywall.can_manage_billing;
    let action = paywall.action;
    let seat_count = paywall.seat_count;

    let (checkout_open, set_checkout_open) = signal(false);
    let (declined_message, set_declined_message) = signal(Option::<String>::None);
    let (needs_action_shown, set_needs_action_shown) = signal(false);
    let (needs_action_url, set_needs_action_url) = signal(Option::<String>::None);
    // Stripe checkout handle — kept alive while the modal is open, dropped
    // (which unmounts the form) on close or completion.
    let checkout_handle: StoredValue<
        Option<send_wrapper::SendWrapper<crate::utils::stripe::EmbeddedCheckoutHandle>>,
    > = StoredValue::new(None);

    let signals = PayFlowSignals {
        checkout_open: set_checkout_open,
        declined_message: set_declined_message,
        needs_action_shown: set_needs_action_shown,
        needs_action_url: set_needs_action_url,
        checkout_handle,
    };

    let start_pay = Action::new(move |_: &()| {
        async move {
            signals.reset();
            match action {
                PaywallAction::RecoverPayment => {
                    match start_payment_recovery().await {
                        Ok(session) => {
                            set_checkout_open.set(true);
                            mount_recovery_checkout(session.client_secret, session.session_id, signals);
                        }
                        Err(e) => {
                            toast_error(format!("Failed to start payment recovery: {e}"));
                        }
                    }
                }
                PaywallAction::Subscribe => {
                    match create_checkout(seat_count).await {
                        Ok(CheckoutOutcome::Embedded(session)) => {
                            set_checkout_open.set(true);
                            mount_subscribe_checkout(session.client_secret, session.session_id, signals);
                        }
                        Ok(CheckoutOutcome::Modified(_)) => {
                            // An existing subscription was reactivated
                            // directly — no checkout needed. Refetch is the
                            // authority; this can't happen for a genuinely
                            // lapsed workspace today (Subscribe only routes
                            // here for cancelled-with-no-subscription or an
                            // expired no-Stripe trial), but handling it
                            // uniformly costs nothing and closes the class
                            // for any future CheckoutPath change.
                            refetch_billing_state();
                        }
                        Err(e) => {
                            toast_error(format!("Failed to start checkout: {e}"));
                        }
                    }
                }
            }
        }
    });

    view! {
        <div class="space-y-6">
            // Heading/explanation live in `AuthLayout`'s title/subtitle now
            // (see `BillingPaywall`) — this content slot holds only what's
            // specific to the workspace and the pay flow.
            {paywall.workspace_name.clone().map(|name| view! {
                <p class="text-sm text-muted-foreground">
                    "Workspace: " <span class="font-medium text-foreground">{name}</span>
                </p>
            })}

            {if can_manage {
                view! {
                    <div class="space-y-4">
                        {move || declined_message.get().map(|message| view! {
                            <Alert variant=AlertVariant::Error>
                                <AlertDescription>{message}</AlertDescription>
                            </Alert>
                        })}
                        {move || needs_action_shown.get().then(|| view! {
                            <Alert variant=AlertVariant::Warning>
                                <AlertDescription>
                                    <div class="space-y-3">
                                        {move || match needs_action_url.get() {
                                            Some(url) => view! {
                                                <span>
                                                    "This payment needs one more step. "
                                                    <a href=url target="_blank" rel="noopener noreferrer" class="underline font-medium">
                                                        "Complete it on Stripe's secure page"
                                                    </a>
                                                    "."
                                                </span>
                                            }.into_any(),
                                            None => view! {
                                                <span>"This payment needs one more step to complete. Please try again."</span>
                                            }.into_any(),
                                        }}
                                        // KYO-806 F7: 3-D Secure (or similar)
                                        // completion happens on Stripe's own
                                        // hosted page in a new tab — this tab
                                        // has no way to learn it finished.
                                        // Give the owner an explicit way back
                                        // in rather than leaving them stuck
                                        // until they reload: re-check both
                                        // Layout's billing state (in case
                                        // another signal already resolved
                                        // this) and this component's own
                                        // paywall data.
                                        <Button
                                            variant=ButtonVariant::Outline
                                            size=ButtonSize::Sm
                                            on:click=move |_| {
                                                refetch_billing_state();
                                                refetch_paywall.run(());
                                            }
                                        >
                                            "I've completed it — check again"
                                        </Button>
                                    </div>
                                </AlertDescription>
                            </Alert>
                        })}

                        <Button
                            variant=ButtonVariant::Default
                            size=ButtonSize::Lg
                            class="w-full justify-center"
                            disabled=Signal::derive(move || start_pay.pending().try_get().unwrap_or(false))
                            on:click=move |_| { start_pay.dispatch(()); }
                        >
                            {move || if start_pay.pending().get() {
                                "Starting checkout...".to_string()
                            } else {
                                action_label(action).to_string()
                            }}
                        </Button>
                    </div>
                }.into_any()
            } else {
                let contact = paywall.owner_name.clone()
                    .or_else(|| paywall.owner_email.clone())
                    .unwrap_or_else(|| "your workspace owner".to_string());
                view! {
                    <Alert variant=AlertVariant::Info>
                        <AlertDescription>
                            {format!("Ask {contact} to update billing — only the workspace owner can manage the subscription.")}
                        </AlertDescription>
                    </Alert>
                }.into_any()
            }}

            <WorkspaceSwitcher/>

            <LogoutLink/>

            <Modal
                show=Signal::derive(move || checkout_open.try_get().unwrap_or(false))
                on_close=Callback::new(move |_| signals.reset())
                title="Complete Payment".to_string()
                size=ModalSize::Lg
            >
                <div id=CHECKOUT_MOUNT_ID class="min-h-[400px]"/>
            </Modal>
        </div>
    }
}

/// Mount the embedded checkout for [`PaywallAction::RecoverPayment`]. On
/// completion, calls `complete_payment_recovery` and maps its
/// [`PaymentRecoveryOutcome`] onto `signals` — `Recovered` calls
/// [`refetch_billing_state`] (the server's `billing_lapsed` becoming
/// `false` is what actually dismisses this component, via `Layout`).
fn mount_recovery_checkout(client_secret: String, session_id: String, signals: PayFlowSignals) {
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let mount_selector = format!("#{CHECKOUT_MOUNT_ID}");
        let sid = session_id.clone();
        let result = crate::utils::stripe::fetch_key_and_mount_embedded_checkout(
            &client_secret,
            &mount_selector,
            move || {
                let sid = sid.clone();
                leptos::task::spawn_local(async move {
                    match complete_payment_recovery(sid).await {
                        Ok(PaymentRecoveryOutcome::Recovered) => {
                            signals.reset();
                            refetch_billing_state();
                        }
                        Ok(PaymentRecoveryOutcome::NeedsAction { hosted_invoice_url }) => {
                            signals.reset();
                            signals.needs_action_shown.try_set(true);
                            signals.needs_action_url.try_set(hosted_invoice_url);
                        }
                        Ok(PaymentRecoveryOutcome::Declined { message }) => {
                            signals.show_declined(message);
                        }
                        Err(e) => {
                            signals.show_declined(format!("Failed to confirm payment: {e}"));
                        }
                    }
                });
            },
        )
        .await;

        match result {
            Ok(handle) => {
                signals
                    .checkout_handle
                    .set_value(Some(send_wrapper::SendWrapper::new(handle)));
            }
            Err(e) => {
                signals.show_declined(format!("Failed to mount checkout form: {e}"));
            }
        }
    });
    // Mirrors `utils/stripe.rs`'s ssr stub for `mount_embedded_checkout` —
    // same message, same reasoning: embedded checkout is a browser-only
    // concept, so the honest answer on any other target is a Declined
    // state the owner can plainly see, not a silent no-op. Also keeps
    // `PayFlowSignals::show_declined` a real (not dead) codepath on every
    // target `cargo check` compiles this crate for.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (client_secret, session_id);
        signals.show_declined("Embedded checkout is only available in the browser.".to_string());
    }
}

/// Mount the embedded checkout for [`PaywallAction::Subscribe`]. On
/// completion, calls `sync_checkout_subscription` (so the workspace's new
/// subscription is visible immediately rather than waiting on the Stripe
/// webhook) and then [`refetch_billing_state`].
fn mount_subscribe_checkout(client_secret: String, session_id: String, signals: PayFlowSignals) {
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        let mount_selector = format!("#{CHECKOUT_MOUNT_ID}");
        let sid = session_id.clone();
        let result = crate::utils::stripe::fetch_key_and_mount_embedded_checkout(
            &client_secret,
            &mount_selector,
            move || {
                let sid = sid.clone();
                leptos::task::spawn_local(async move {
                    signals.reset();
                    if let Err(e) = sync_checkout_subscription(sid).await {
                        toast_error(format!("Payment completed, but syncing failed: {e}"));
                    }
                    refetch_billing_state();
                });
            },
        )
        .await;

        match result {
            Ok(handle) => {
                signals
                    .checkout_handle
                    .set_value(Some(send_wrapper::SendWrapper::new(handle)));
            }
            Err(e) => {
                signals.show_declined(format!("Failed to mount checkout form: {e}"));
            }
        }
    });
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (client_secret, session_id);
        signals.show_declined("Embedded checkout is only available in the browser.".to_string());
    }
}

/// Workspace switcher — only rendered when the caller belongs to more than
/// one workspace. Same `list_my_workspaces`/`switch_workspace` server fns
/// (both allow-lapsed, KYO-805) and reload-on-success pattern
/// `components/layout.rs`'s sidebar switcher uses, so a member of several
/// workspaces trapped in a lapsed one can switch to one that isn't rather
/// than being stuck.
#[component]
fn WorkspaceSwitcher() -> impl IntoView {
    let workspaces = LocalResource::new(list_my_workspaces);

    let switch_action = Action::new(|ws_id: &String| {
        let ws_id = ws_id.clone();
        async move { switch_workspace(ws_id).await }
    });

    Effect::new(move |_| {
        if let Some(result) = switch_action.value().get() {
            match result {
                Ok(()) => {
                    #[cfg(target_arch = "wasm32")]
                    if let Some(win) = web_sys::window() {
                        let _ = win.location().reload();
                    }
                }
                Err(e) => {
                    toast_error(format!("Failed to switch workspace: {e}"));
                }
            }
        }
    });

    view! {
        <Transition fallback=|| ()>
            {move || Suspend::new(async move {
                match workspaces.await {
                    Ok(list) if list.len() > 1 => Some(view! {
                        <div class="border-t border-border pt-4 space-y-2">
                            <p class="text-xs font-medium text-muted-foreground uppercase tracking-wide">
                                "Switch workspace"
                            </p>
                            <div class="space-y-1">
                                {list.into_iter().filter(|w| !w.is_active).map(|w| {
                                    let ws_id = w.workspace_id.clone();
                                    view! {
                                        // DESIGN.md "Use Components, Not Raw
                                        // HTML": `Button` owns color/padding/
                                        // radius/font; `class` here is layout
                                        // only (full width). The label sits
                                        // in a `flex-1` span so it fills the
                                        // row and pushes the icon to the far
                                        // edge — achieves the same visual
                                        // result as `justify-between` without
                                        // fighting `Button`'s own `justify-
                                        // center` base class.
                                        <Button
                                            variant=ButtonVariant::GhostMuted
                                            size=ButtonSize::Sm
                                            class="w-full"
                                            disabled=Signal::derive(move || switch_action.pending().try_get().unwrap_or(false))
                                            on:click=move |_| { switch_action.dispatch(ws_id.clone()); }
                                        >
                                            <span class="flex-1 text-left">{w.name}</span>
                                            <Icon icon=phosphor_leptos::ARROW_RIGHT size="14px"/>
                                        </Button>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    }.into_any()),
                    _ => None,
                }
            })}
        </Transition>
    }
}

/// Log out — reuses the exact same `logout()` server fn call
/// `components/layout.rs`'s user menu uses.
#[component]
fn LogoutLink() -> impl IntoView {
    let logout_action = Action::new(move |_: &()| async move {
        let _ = logout().await;
        #[cfg(target_arch = "wasm32")]
        if let Some(win) = web_sys::window() {
            let _ = win.location().set_href("/login");
        }
    });

    view! {
        <div class="border-t border-border pt-4">
            // DESIGN.md "Use Components, Not Raw HTML" — see
            // `WorkspaceSwitcher`'s comment above for the same rule applied
            // to the switcher rows. `ButtonSize::Sm`'s padding also gives
            // this a WCAG 2.5.5 AAA-sized hit target, matching the pattern
            // `AuthLayout`'s own footer links already use.
            <Button
                variant=ButtonVariant::Link
                size=ButtonSize::Sm
                on:click=move |_| { logout_action.dispatch(()); }
            >
                "Log out"
            </Button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trial_ended_copy_mentions_trial() {
        let (heading, _) = lapse_copy(BillingLapseReason::TrialEnded);
        assert_eq!(heading, "Your trial has ended");
    }

    #[test]
    fn payment_failed_copy_mentions_payment() {
        let (heading, _) = lapse_copy(BillingLapseReason::PaymentFailed);
        assert_eq!(heading, "Your payment failed");
    }

    #[test]
    fn subscription_ended_copy_mentions_subscription() {
        let (heading, _) = lapse_copy(BillingLapseReason::SubscriptionEnded);
        assert_eq!(heading, "Your subscription has ended");
    }

    #[test]
    fn every_reason_maps_to_distinct_copy() {
        let reasons = [
            BillingLapseReason::TrialEnded,
            BillingLapseReason::PaymentFailed,
            BillingLapseReason::SubscriptionEnded,
        ];
        let headings: std::collections::HashSet<_> =
            reasons.iter().map(|r| lapse_copy(*r).0).collect();
        assert_eq!(headings.len(), reasons.len(), "every reason must render distinct copy");
    }

    #[test]
    fn recover_payment_action_label_is_update_not_subscribe() {
        // RecoverPayment must never say "Subscribe" — that would suggest a
        // second subscription is being created (KYO-806 A6's exact bug).
        assert_eq!(action_label(PaywallAction::RecoverPayment), "Update payment method");
    }

    #[test]
    fn subscribe_action_label_says_subscribe() {
        assert_eq!(action_label(PaywallAction::Subscribe), "Subscribe now");
    }
}
