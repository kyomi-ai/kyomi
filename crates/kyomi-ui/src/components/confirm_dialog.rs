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
#[component]
pub fn ConfirmDialog(
    /// Whether the dialog is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Dialog title.
    #[prop(into)]
    title: String,
    /// Dialog message/description.
    #[prop(into)]
    message: String,
    /// Text for the confirm button.
    #[prop(default = "Confirm".to_string(), into)]
    confirm_text: String,
    /// Text for the cancel button.
    #[prop(default = "Cancel".to_string(), into)]
    cancel_text: String,
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
                        {title.clone()}
                    </h3>
                    <p
                        id="confirm-dialog-message"
                        class="text-sm text-muted-foreground mb-6"
                    >
                        {message.clone()}
                    </p>
                    <div class="flex justify-end gap-3">
                        <button
                            class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground"
                            on:click=move |_| on_cancel.run(())
                        >
                            {cancel_text.clone()}
                        </button>
                        <button
                            class=confirm_btn_class
                            on:click=move |_| on_confirm.run(())
                        >
                            {confirm_text.clone()}
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}
