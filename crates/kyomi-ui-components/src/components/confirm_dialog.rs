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
    /// If true, confirm button uses destructive (red) styling.
    #[prop(default = true)]
    destructive: bool,
    /// Called when the user confirms.
    on_confirm: Callback<()>,
    /// Called when the user cancels (or clicks backdrop).
    on_cancel: Callback<()>,
) -> impl IntoView {
    // Match Button component variant classes exactly (from button.jsx)
    let confirm_btn_class = if destructive {
        "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-destructive text-destructive-foreground shadow-sm hover:bg-destructive/90"
    } else {
        "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 bg-primary text-primary-foreground shadow hover:bg-primary/90"
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
