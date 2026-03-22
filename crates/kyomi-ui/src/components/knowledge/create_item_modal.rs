// SPDX-License-Identifier: AGPL-3.0-or-later

//! Create/rename knowledge item modal.
//!
//! Matches `apps/frontend/src/components/CreateKnowledgeItemModal.jsx` exactly.
//!
//! A small modal with a single text input for naming a new file/folder or
//! renaming an existing one. Validates that the name is non-empty before
//! allowing submission.
//!
//! Usage:
//! ```ignore
//! let (show, set_show) = signal(false);
//! let on_close = Callback::new(move |()| set_show.set(false));
//! let on_submit = Callback::new(move |name: String| {
//!     // create or rename the item
//! });
//!
//! view! {
//!     <CreateKnowledgeItemModal
//!         show=show
//!         on_close=on_close
//!         on_submit=on_submit
//!         title="New File"
//!     />
//! }
//! ```

use leptos::ev;
use leptos::prelude::*;

use crate::components::button::{Button, ButtonVariant};
use crate::components::input::INPUT_CLASS;
use crate::components::modal::{Modal, ModalSize};

/// Modal for creating or renaming a knowledge file/folder.
///
/// React reference: `apps/frontend/src/components/CreateKnowledgeItemModal.jsx`
#[component]
pub fn CreateKnowledgeItemModal(
    /// Whether the modal is visible.
    #[prop(into)]
    show: Signal<bool>,
    /// Called to close the modal.
    on_close: Callback<()>,
    /// Called with the trimmed name on submit.
    on_submit: Callback<String>,
    /// Modal title (e.g. "New File", "Rename").
    #[prop(into)]
    title: String,
    /// Pre-filled value (for rename mode).
    #[prop(into, default = String::new())]
    default_value: String,
    /// Submit button label.
    #[prop(into, default = "Create".to_string())]
    submit_label: String,
) -> impl IntoView {
    let (name, set_name) = signal(default_value.clone());
    let input_ref = NodeRef::<leptos::html::Input>::new();

    // React: reset name + focus/select when modal opens.
    let default_for_effect = default_value.clone();
    Effect::new(move |_| {
        if show.get() {
            set_name.set(default_for_effect.clone());

            // Focus and select after DOM update (only in browser).
            #[cfg(feature = "hydrate")]
            {
                let node = input_ref.get();
                if let Some(el) = node {
                    // Use request_animation_frame to ensure the DOM is painted.
                    let el_clone = el.clone();
                    leptos::prelude::request_animation_frame(move || {
                        let _ = el_clone.focus();
                        let _ = el_clone.select();
                    });
                }
            }
        }
    });

    // Submit handler — trim, validate, call callbacks.
    let on_close_submit = on_close.clone();
    let handle_submit = move || {
        let trimmed = name.get_untracked().trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        on_submit.run(trimmed);
        on_close_submit.run(());
    };

    // Enter key submits.
    let handle_submit_key = handle_submit.clone();
    let handle_keydown = move |ev: ev::KeyboardEvent| {
        if ev.key() == "Enter" {
            ev.prevent_default();
            (handle_submit_key.clone())();
        }
    };

    // Disabled state — submit button disabled when name is blank.
    let is_disabled = Memo::new(move |_| name.get().trim().is_empty());

    let submit_label_footer = submit_label.clone();
    let handle_submit_btn = handle_submit.clone();
    let on_close_cancel = on_close.clone();

    view! {
        <Modal
            show=show
            on_close=on_close
            title=title
            size=ModalSize::Sm
            footer=ChildrenFn::to_children(move || {
                let submit_label = submit_label_footer.clone();
                let handle_submit_btn = handle_submit_btn.clone();
                let on_close_cancel = on_close_cancel.clone();
                let disabled = is_disabled.get();

                view! {
                    <Button variant=ButtonVariant::Outline on:click=move |_| on_close_cancel.run(())>
                        "Cancel"
                    </Button>
                    <Button disabled=disabled on:click=move |_| (handle_submit_btn.clone())()>
                        {submit_label.clone()}
                    </Button>
                }
                .into_any()
            })
        >
            <input
                type="text"
                class=INPUT_CLASS
                node_ref=input_ref
                prop:value=move || name.get()
                on:input=move |ev| set_name.set(event_target_value(&ev))
                on:keydown=handle_keydown
                placeholder="Enter name..."
            />
        </Modal>
    }
}
