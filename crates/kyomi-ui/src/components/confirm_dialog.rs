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
                class="fixed inset-0 z-50 bg-[var(--color-overlay)] flex items-center justify-center animate-fade-in-fast"
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
