// SPDX-License-Identifier: AGPL-3.0-or-later

//! Toast notification system.
//!
//! Provides global toast notifications via a thread-local singleton.
//! Usage:
//! ```ignore
//! // At app root:
//! view! { <ToastProvider/> }
//!
//! // Anywhere in the app:
//! toast_success("Saved successfully");
//! toast_error("Something went wrong");
//! ```

use leptos::prelude::*;

/// Container class for the toast notification stack.
///
/// `z-[1080]` — see the "Stacking / Z-Index Scale" table in `DESIGN.md`
/// (KYO-441). Sits above `ConfirmDialog`'s `z-[1060]` so feedback raised
/// during a confirmation is still seen, and below Tooltip's `z-[1100]`. A
/// bare literal here — not a shared enum like `ModalLayer` — because
/// `Toast` has exactly one stacking value; introducing an enum for a
/// single constant would be a parallel abstraction with no second variant
/// to justify it.
const CONTAINER_CLASS: &str = "fixed top-4 right-4 z-[1080] flex flex-col gap-2 max-w-sm";

/// Toast severity level.
#[derive(Clone, Debug, PartialEq)]
pub enum ToastVariant {
    Success,
    Error,
    Info,
}

/// A single toast notification.
#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub message: String,
    pub variant: ToastVariant,
}

/// Signal holding the current list of toasts.
#[derive(Clone, Copy)]
struct ToastState {
    toasts: RwSignal<Vec<Toast>>,
    next_id: RwSignal<u64>,
}

// Thread-local storage for ToastState so toast functions work inside
// spawn_local async blocks where the reactive owner (and thus use_context)
// is unavailable after .await points.
thread_local! {
    static GLOBAL_TOAST_STATE: std::cell::Cell<Option<ToastState>> = const { std::cell::Cell::new(None) };
}

/// Add a toast and auto-dismiss after a delay.
fn add_toast(variant: ToastVariant, message: impl Into<String>) {
    let Some(state) = GLOBAL_TOAST_STATE.get() else {
        return;
    };

    let id = state.next_id.get_untracked();
    state.next_id.set(id + 1);

    let toast = Toast {
        id,
        message: message.into(),
        variant: variant.clone(),
    };

    state.toasts.update(|toasts| toasts.push(toast));

    // Auto-dismiss
    let dismiss_ms = match variant {
        ToastVariant::Success => 3000,
        ToastVariant::Info => 4000,
        ToastVariant::Error => 5000,
    };

    // Auto-dismiss via set_timeout (browser-only)
    set_timeout(
        move || {
            state
                .toasts
                .try_update(|toasts| toasts.retain(|t| t.id != id));
        },
        std::time::Duration::from_millis(dismiss_ms),
    );
}

/// Show a success toast.
pub fn toast_success(message: impl Into<String>) {
    add_toast(ToastVariant::Success, message);
}

/// Show an error toast.
pub fn toast_error(message: impl Into<String>) {
    add_toast(ToastVariant::Error, message);
}

/// Show an info toast.
pub fn toast_info(message: impl Into<String>) {
    add_toast(ToastVariant::Info, message);
}

/// Toast provider + container. Mount once at the app root.
#[component]
pub fn ToastProvider(children: Children) -> impl IntoView {
    let state = ToastState {
        toasts: RwSignal::new(Vec::new()),
        next_id: RwSignal::new(0),
    };
    GLOBAL_TOAST_STATE.set(Some(state));

    view! {
        {children()}
        <ToastContainer state=state/>
    }
}

/// Renders the toast notifications in the bottom-right corner.
#[component]
fn ToastContainer(state: ToastState) -> impl IntoView {
    view! {
        <div class=CONTAINER_CLASS>
            <For
                each=move || state.toasts.get()
                key=|toast| toast.id
                let:toast
            >
                <ToastItem toast=toast state=state/>
            </For>
        </div>
    }
}

/// A single toast notification item.
#[component]
fn ToastItem(toast: Toast, state: ToastState) -> impl IntoView {
    let (bg, border, text_color) = match toast.variant {
        ToastVariant::Success => (
            "bg-success",
            "border-success-border",
            "text-success-foreground",
        ),
        ToastVariant::Error => (
            "bg-error",
            "border-error-border",
            "text-error-foreground",
        ),
        ToastVariant::Info => ("bg-info", "border-info-border", "text-info-foreground"),
    };

    let id = toast.id;

    view! {
        <div class=format!(
            "flex items-center gap-2 px-4 py-3 rounded-lg border shadow-lg animate-slide-in-right {bg} {border} {text_color}"
        )>
            <p class="text-sm font-medium flex-1">{toast.message.clone()}</p>
            <button
                class="text-current opacity-60 hover:opacity-100 transition-opacity"
                on:click=move |_| {
                    state.toasts.update(|toasts| toasts.retain(|t| t.id != id));
                }
            >
                "x"
            </button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_clears_modal_elevated_layer() {
        // KYO-441: a Toast raised while a Modal is open (possibly already
        // at ModalLayer::Elevated, z-[1050]) must paint above it, or the
        // toast is invisible — reproduced via elementFromPoint() before
        // this fix, returning the modal backdrop instead of the toast.
        assert!(
            CONTAINER_CLASS.contains("z-[1080]"),
            "expected container class to carry z-[1080], got {CONTAINER_CLASS:?}"
        );
    }

    #[test]
    fn container_no_longer_uses_pre_kyo_441_z_50() {
        assert!(
            !CONTAINER_CLASS.contains("z-50"),
            "z-50 sat below Modal's z-[1000]; KYO-441 must remove it entirely, got {CONTAINER_CLASS:?}"
        );
    }

    #[test]
    fn container_paints_above_confirm_dialog_and_below_tooltip() {
        // Kept as literals (not imported) so this test fails loudly if any
        // of the three components' z-index drifts independently. See the
        // "Stacking / Z-Index Scale" table in DESIGN.md.
        let confirm_dialog = 1060; // confirm_dialog.rs BACKDROP_CLASS
        let toast = 1080;
        let tooltip = 1100; // tooltip.rs CONTENT_CLASS
        assert!(
            toast > confirm_dialog,
            "Toast ({toast}) must paint above ConfirmDialog ({confirm_dialog}) so feedback raised \
             during a confirmation is still seen"
        );
        assert!(
            toast < tooltip,
            "Toast ({toast}) must stay below Tooltip ({tooltip})"
        );
    }
}
