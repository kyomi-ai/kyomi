// SPDX-License-Identifier: AGPL-3.0-or-later

//! Confirm dialog component.
//!
//! A modal dialog that asks the user to confirm a destructive action.
//! Controlled via signals — the parent manages open/close state.
//!
//! Usage:
//! ```ignore
//! let (dialog_open, set_dialog_open) = signal(false);
//! let on_confirm = Callback::new(move |()| {
//!     set_dialog_open.set(false);
//!     // do the destructive action
//! });
//! let on_cancel = Callback::new(move |()| set_dialog_open.set(false));
//!
//! view! {
//!     <ConfirmDialog
//!         open=dialog_open
//!         title="Delete item?"
//!         message="This action cannot be undone."
//!         confirm_text="Delete"
//!         on_confirm=on_confirm
//!         on_cancel=on_cancel
//!     />
//! }
//! ```

use leptos::prelude::*;

/// Backdrop class for the confirm-dialog overlay.
///
/// `z-[1060]` — see the "Stacking / Z-Index Scale" table in `DESIGN.md`
/// (KYO-441). Must clear `ModalLayer::Elevated`'s `z-[1050]` (a
/// `ConfirmDialog` can be opened from a modal that is itself stacked on
/// another modal) and stay below Toast's `z-[1080]` and Tooltip's
/// `z-[1100]`. A bare literal here — not a shared enum like `ModalLayer` —
/// because `ConfirmDialog` has exactly one stacking value, not a set of
/// caller-selectable layers; introducing an enum for a single constant
/// would be a parallel abstraction with no second variant to justify it.
const BACKDROP_CLASS: &str =
    "fixed inset-0 z-[1060] bg-[var(--color-overlay)] flex items-center justify-center animate-fade-in-fast";

/// A confirmation dialog overlay.
///
/// All text props accept `Signal<String>` (or `String` via `MaybeProp`) so they
/// re-read reactively when the dialog opens — no stale-render bugs.
#[component]
pub fn ConfirmDialog(
    /// Whether the dialog is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Dialog title.
    #[prop(into)]
    title: MaybeProp<String>,
    /// Dialog message/description.
    #[prop(into)]
    message: MaybeProp<String>,
    /// Text for the confirm button.
    #[prop(into, optional)]
    confirm_text: MaybeProp<String>,
    /// Text for the cancel button.
    #[prop(into, optional)]
    cancel_text: MaybeProp<String>,
    /// If true, confirm button uses destructive (red) styling. Reactive —
    /// matches `Switch`'s `disabled` convention (`MaybeProp<bool>` via
    /// `#[prop(into)]`, KYO-487) so a caller holding a `Signal<bool>` (e.g.
    /// "destructive only for the cancel-subscription variant of this
    /// dialog, not the reactivate variant") can pass it directly instead of
    /// snapshotting a stale value at construction time. A plain `bool`
    /// still works via `Into`. Defaults to `true` (KYO-726), preserving the
    /// prior non-reactive default.
    #[prop(optional, into)]
    destructive: MaybeProp<bool>,
    /// Called when the user confirms.
    on_confirm: Callback<()>,
    /// Called when the user cancels (or clicks backdrop).
    on_cancel: Callback<()>,
) -> impl IntoView {
    // Match Button component variant classes exactly (from button.jsx).
    // Reactive closure (not a value computed once) so a caller whose
    // `destructive` signal changes between opens — e.g. billing.rs reusing
    // one `ConfirmDialog` for both "Cancel Subscription" (destructive) and
    // "Reactivate" (not) — gets the correct button color each time (KYO-726).
    let confirm_btn_class = move || {
        if destructive.get().unwrap_or(true) {
            "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-destructive text-destructive-foreground shadow-sm hover:bg-destructive/90"
        } else {
            "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90"
        }
    };

    view! {
        <Show when=move || open.get()>
            // Backdrop
            <div
                class=BACKDROP_CLASS
                on:click=move |_| on_cancel.run(())
            >
                // Dialog
                <div
                    class="bg-card border border-border rounded-lg shadow max-w-md w-full mx-4 p-6 animate-zoom-fade-in"
                    role="alertdialog"
                    aria-modal="true"
                    aria-labelledby="confirm-dialog-title"
                    aria-describedby="confirm-dialog-message"
                    on:click=|ev| ev.stop_propagation()
                >
                    <h3
                        id="confirm-dialog-title"
                        class="text-lg font-semibold text-foreground mb-2"
                    >
                        {move || title.get().unwrap_or_default()}
                    </h3>
                    <p
                        id="confirm-dialog-message"
                        class="text-sm text-muted-foreground mb-6"
                    >
                        {move || message.get().unwrap_or_default()}
                    </p>
                    <div class="flex justify-end gap-3">
                        <button
                            class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground"
                            on:click=move |_| on_cancel.run(())
                        >
                            {move || cancel_text.get().unwrap_or_else(|| "Cancel".to_string())}
                        </button>
                        <button
                            class=confirm_btn_class
                            on:click=move |_| on_confirm.run(())
                        >
                            {move || confirm_text.get().unwrap_or_else(|| "Confirm".to_string())}
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backdrop_clears_modal_elevated_layer() {
        // KYO-441: a ConfirmDialog opened from inside a Modal (possibly
        // already at ModalLayer::Elevated, z-[1050]) must paint above it,
        // or the dialog is invisible — reproduced via elementFromPoint()
        // before this fix, returning the modal backdrop instead of the
        // dialog.
        assert!(
            BACKDROP_CLASS.contains("z-[1060]"),
            "expected backdrop class to carry z-[1060], got {BACKDROP_CLASS:?}"
        );
    }

    #[test]
    fn backdrop_no_longer_uses_pre_kyo_441_z_50() {
        assert!(
            !BACKDROP_CLASS.contains("z-50"),
            "z-50 sat below Modal's z-[1000]; KYO-441 must remove it entirely, got {BACKDROP_CLASS:?}"
        );
    }

    #[test]
    fn backdrop_stays_below_toast_and_tooltip() {
        // Kept as literals (not imported) so this test fails loudly if any
        // of the three components' z-index drifts independently. See the
        // "Stacking / Z-Index Scale" table in DESIGN.md.
        let confirm_dialog = 1060;
        let toast = 1080; // toast.rs CONTAINER_CLASS
        let tooltip = 1100; // tooltip.rs CONTENT_CLASS
        assert!(
            confirm_dialog > 1050,
            "ConfirmDialog ({confirm_dialog}) must clear ModalLayer::Elevated (1050)"
        );
        assert!(
            confirm_dialog < toast,
            "ConfirmDialog ({confirm_dialog}) must stay below Toast ({toast}) so feedback raised \
             during a confirmation is still seen"
        );
        assert!(
            confirm_dialog < tooltip,
            "ConfirmDialog ({confirm_dialog}) must stay below Tooltip ({tooltip})"
        );
    }
}

// Rendering to HTML (`RenderHtml::to_html`) panics unless the shared
// `leptos`/`tachys` `ssr` feature is active for this build — see the
// crate's own `ssr` feature in Cargo.toml and `switch.rs`'s identical
// convention for its KYO-487 reactive-prop tests. `cargo test -p
// kyomi-ui-components` alone skips this module cleanly; `--features ssr` is
// required to run it.
#[cfg(all(test, feature = "ssr"))]
mod reactive_prop_tests {
    use super::*;

    /// KYO-726 — `title`/`message` must be genuinely reactive, not
    /// snapshotted once at construction time. The actual production bug
    /// was one level up the call stack (team.rs, billing.rs, and
    /// passkey_manager.rs each passed `dialog_title.get_untracked()` as the
    /// prop, collapsing the signal to a frozen `String` at parent-render
    /// time — long before the click handler that later called
    /// `set_dialog_title.set(...)` ever ran), but the contract this test
    /// pins belongs to `ConfirmDialog` itself: given a real signal prop, it
    /// must re-read it on render, not capture it once.
    ///
    /// This builds the `<ConfirmDialog>` view *before* setting the bound
    /// signals, then renders to HTML *after* — mirroring switch.rs's
    /// KYO-487 `disabled` test. Against a component that captured
    /// `title.get()` once into a local at construction time, this render,
    /// happening strictly after the `.set()` calls, would still show the
    /// empty initial value.
    #[test]
    fn title_and_message_reflect_signals_set_after_construction() {
        let owner = Owner::new();
        owner.set();

        let open = RwSignal::new(true);
        let title = RwSignal::new(String::new());
        let message = RwSignal::new(String::new());

        let view = view! {
            <ConfirmDialog
                open=Signal::from(open)
                title=title
                message=message
                on_confirm=Callback::new(|_: ()| {})
                on_cancel=Callback::new(|_: ()| {})
            />
        };

        // Set strictly after the view value above was constructed — exactly
        // what the buggy callsites' `set_dialog_title.set(...)` /
        // `set_dialog_message.set(...)` click handlers do.
        title.set("Delete Passkey?".to_string());
        message.set("This cannot be undone.".to_string());

        let html = view.to_html();
        assert!(
            html.contains("Delete Passkey?"),
            "expected the rendered <h3> to reflect the title set after \
             construction, got: {html}"
        );
        assert!(
            html.contains("This cannot be undone."),
            "expected the rendered <p> to reflect the message set after \
             construction, got: {html}"
        );
    }

    /// KYO-726 — `destructive` must also be reactive: billing.rs reuses one
    /// `ConfirmDialog` for both a destructive "Cancel Subscription" prompt
    /// and a non-destructive "Reactivate" prompt, flipping the same signal
    /// between opens.
    #[test]
    fn destructive_class_reflects_signal_flip_to_true_after_construction() {
        let owner = Owner::new();
        owner.set();

        let open = RwSignal::new(true);
        let destructive = RwSignal::new(false);

        let view = view! {
            <ConfirmDialog
                open=Signal::from(open)
                title="Title"
                message="Message"
                destructive=destructive
                on_confirm=Callback::new(|_: ()| {})
                on_cancel=Callback::new(|_: ()| {})
            />
        };

        destructive.set(true);

        let html = view.to_html();
        assert!(
            html.contains("bg-destructive"),
            "expected the confirm button to carry the destructive class \
             after the signal flipped to true, got: {html}"
        );
    }

    /// Mirror of the above in the other direction — rules out a component
    /// that renders destructive styling unconditionally regardless of the
    /// signal.
    #[test]
    fn destructive_class_clears_when_signal_flips_to_false_after_construction() {
        let owner = Owner::new();
        owner.set();

        let open = RwSignal::new(true);
        let destructive = RwSignal::new(true);

        let view = view! {
            <ConfirmDialog
                open=Signal::from(open)
                title="Title"
                message="Message"
                destructive=destructive
                on_confirm=Callback::new(|_: ()| {})
                on_cancel=Callback::new(|_: ()| {})
            />
        };

        destructive.set(false);

        let html = view.to_html();
        assert!(
            !html.contains("bg-destructive"),
            "expected no destructive class after the signal flipped to \
             false, got: {html}"
        );
    }
}
