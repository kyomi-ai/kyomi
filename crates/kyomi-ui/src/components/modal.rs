// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modal component — matches `apps/frontend/src/components/Modal.jsx` exactly.
//!
//! A center-overlay modal with backdrop, configurable sizes, close button,
//! and optional header/content/footer structure.
//!
//! Backdrop: `bg-black/50`, no blur. Shadow: `shadow-xl`.
//! Sizes: sm (384px), md (448px), lg (896px), xl (1152px), full (95vw).
//!
//! Usage:
//! ```ignore
//! let (show, set_show) = signal(false);
//! let on_close = Callback::new(move |()| set_show.set(false));
//!
//! view! {
//!     <Modal
//!         show=show
//!         on_close=on_close
//!         title="Edit Item"
//!         size=ModalSize::Lg
//!         footer=|| view! {
//!             <button class="...">"Cancel"</button>
//!             <button class="...">"Save"</button>
//!         }
//!     >
//!         <p>"Modal content here."</p>
//!     </Modal>
//! }
//! ```

use leptos::ev;
use leptos::prelude::*;

/// Modal size variants.
///
/// React: `sizeClasses = { sm: 'max-w-sm', md: 'max-w-md', lg: 'max-w-4xl', xl: 'max-w-6xl', full: 'max-w-[95vw]' }`
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModalSize {
    /// 384px — Confirmations, simple forms
    Sm,
    /// 448px — Single-field forms
    Md,
    /// 896px — Default, multi-field forms
    #[default]
    Lg,
    /// 1152px — Complex forms, tables
    Xl,
    /// 95vw — Maximum space needed
    Full,
}

impl ModalSize {
    /// Returns the Tailwind max-width class for this size.
    /// React: `sizeClasses` object in Modal.jsx
    fn class(self) -> &'static str {
        match self {
            Self::Sm => "max-w-sm",
            Self::Md => "max-w-md",
            Self::Lg => "max-w-4xl",
            Self::Xl => "max-w-6xl",
            Self::Full => "max-w-[95vw]",
        }
    }
}

/// A center-overlay modal component.
///
/// React reference: `apps/frontend/src/components/Modal.jsx`
///
/// Structure:
/// - Backdrop overlay: `fixed inset-0 flex items-center justify-center z-[1000]` + `bg-black/50`
/// - Modal container: `bg-background text-foreground rounded-lg shadow-xl` + size class
/// - Header: title + close button, separated by `border-b border-border`
/// - Content: scrollable area
/// - Footer: optional action buttons, separated by `border-t border-border`
#[component]
pub fn Modal(
    /// Whether the modal is visible.
    #[prop(into)]
    show: Signal<bool>,
    /// Called on backdrop click, close button click, or Escape key.
    on_close: Callback<()>,
    /// Modal title displayed in the header.
    #[prop(into)]
    title: String,
    /// Modal size — controls max-width. Default: Lg (896px).
    #[prop(default = ModalSize::Lg)]
    size: ModalSize,
    /// Optional footer content (action buttons, etc.).
    /// Use `ChildrenFn` so the footer can be re-rendered inside `<Show>`.
    #[prop(optional)]
    footer: Option<ChildrenFn>,
    /// Main modal content.
    /// Uses `ChildrenFn` (not `Children`) because content lives inside `<Show>`,
    /// which requires `Fn` (re-callable) rather than `FnOnce`.
    children: ChildrenFn,
) -> impl IntoView {
    // React: `modal-content ${sizeClasses[size]} w-full mx-2 sm:mx-4 max-h-[95vh] sm:max-h-[90vh] flex flex-col`
    // Expanded: `bg-background text-foreground rounded-lg shadow-xl` (from .modal-content in index.css)
    let content_class = format!(
        "bg-background text-foreground rounded-lg shadow-xl {} w-full mx-2 sm:mx-4 max-h-[95vh] sm:max-h-[90vh] flex flex-col",
        size.class()
    );

    // Escape key handler
    let handle_keydown = move |ev: ev::KeyboardEvent| {
        if ev.key() == "Escape" {
            on_close.run(());
        }
    };

    view! {
        <Show when=move || show.get()>
            // Backdrop overlay
            // React: className="modal-overlay" → `fixed inset-0 flex items-center justify-center z-[1000] font-sans`
            //   + `background-color: var(--color-overlay)` which is `rgba(0,0,0,0.5)` → `bg-black/50`
            <div
                class="fixed inset-0 flex items-center justify-center z-[1000] font-sans bg-black/50"
                on:click=move |ev: web_sys::MouseEvent| {
                    // Only close if click is directly on the backdrop, not bubbled from modal content.
                    // React uses mousedown tracking; here we rely on stopPropagation on the content div.
                    let target = ev.target();
                    let current_target = ev.current_target();
                    if target == current_target {
                        on_close.run(());
                    }
                }
                on:keydown=handle_keydown
                tabindex="-1"
            >
                // Modal content container
                <div
                    class=content_class.clone()
                    on:click=|ev: web_sys::MouseEvent| ev.stop_propagation()
                >
                    // Header
                    // React: `px-4 sm:px-6 py-3 sm:py-4 border-b border-border flex items-center justify-between flex-shrink-0`
                    <div class="px-4 sm:px-6 py-3 sm:py-4 border-b border-border flex items-center justify-between flex-shrink-0">
                        // React: `text-lg sm:text-xl font-semibold text-foreground`
                        <h2 class="text-lg sm:text-xl font-semibold text-foreground">
                            {title.clone()}
                        </h2>
                        // Close button
                        // React: Button variant="ghost" size="icon" → ghost icon button classes
                        <button
                            class="inline-flex items-center justify-center rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 w-9 hover:bg-accent hover:text-accent-foreground text-muted-foreground hover:text-foreground"
                            on:click=move |_| on_close.run(())
                            aria-label="Close"
                        >
                            // React: SVG X icon — `w-6 h-6`, stroke path "M6 18L18 6M6 6l12 12"
                            <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                    stroke-width="2"
                                    d="M6 18L18 6M6 6l12 12"
                                />
                            </svg>
                        </button>
                    </div>

                    // Content — scrollable
                    // React: `p-4 sm:p-6 overflow-y-auto flex-1`
                    <div class="p-4 sm:p-6 overflow-y-auto flex-1">
                        {children()}
                    </div>

                    // Footer — optional
                    // React: `px-4 sm:px-6 py-3 sm:py-4 border-t border-border flex justify-end gap-2 flex-shrink-0`
                    {footer.as_ref().map(|f| view! {
                        <div class="px-4 sm:px-6 py-3 sm:py-4 border-t border-border flex justify-end gap-2 flex-shrink-0">
                            {f()}
                        </div>
                    })}
                </div>
            </div>
        </Show>
    }
}
