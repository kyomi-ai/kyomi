// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modal component — matches `apps/frontend/src/components/Modal.jsx` exactly.
//!
//! A center-overlay modal with backdrop, configurable sizes, close button,
//! and optional header/content/footer structure.
//!
//! Backdrop: `bg-[var(--color-overlay)]`, no blur. Shadow: `shadow-xl`.
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
use phosphor_leptos::Icon;
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

/// Which stacking layer a modal's backdrop paints on.
///
/// Every `Modal` used to hardcode `z-[1000]` on its backdrop, so any two
/// modals open at once fell through to DOM order to decide which painted on
/// top — the caller had no way to ask for "on top of another modal" (KYO-434:
/// the feedback modal opened from inside the Add Datasource modal rendered
/// *behind* it, because `Sidebar`, which owns `FeedbackModal`, renders before
/// `<main>`, which owns the datasource modal).
///
/// See the "Stacking / Z-Index Scale" table in `DESIGN.md` for the full
/// picture across components (Toast, ConfirmDialog, Tooltip, etc.) — this
/// enum only covers `Modal`'s own two layers, and [`Self::Elevated`] must
/// stay below Tooltip's `z-[1100]` so tooltips still render above modals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModalLayer {
    /// `z-[1000]` — the layer every `Modal` used before this enum existed.
    /// Changing this value restacks every modal in the app; don't.
    #[default]
    Base,
    /// `z-[1050]` — for a modal that must open on top of another `Modal`
    /// (e.g. `FeedbackModal` opened from inside the Add Datasource modal).
    /// Stays below Tooltip's `z-[1100]`.
    Elevated,
}

impl ModalLayer {
    /// Returns the Tailwind z-index class for this layer.
    fn class(self) -> &'static str {
        match self {
            Self::Base => "z-[1000]",
            Self::Elevated => "z-[1050]",
        }
    }
}

/// A center-overlay modal component.
///
/// React reference: `apps/frontend/src/components/Modal.jsx`
///
/// Structure:
/// - Backdrop overlay: `fixed inset-0 flex items-center justify-center z-[1000]` + `bg-[var(--color-overlay)]`
/// - Modal container: `bg-background text-foreground rounded-lg shadow-xl` + size class
/// - Header: title + close button, separated by `border-b border-border`
/// - Content: scrollable area with optional `min_height` to prevent layout shift during loading
/// - Footer: optional action buttons, separated by `border-t border-border`
#[component]
pub fn Modal(
    /// Whether the modal is visible.
    #[prop(into)]
    show: Signal<bool>,
    /// Called on backdrop click, close button click, or Escape key.
    on_close: Callback<()>,
    /// Modal title displayed in the header. Accepts a string literal, owned
    /// `String`, signal, or closure — rendered reactively so callers can
    /// update the header live (e.g. while the user edits a title field).
    #[prop(into)]
    title: MaybeProp<String>,
    /// Modal size — controls max-width. Default: Lg (896px).
    #[prop(default = ModalSize::Lg)]
    size: ModalSize,
    /// Stacking layer — controls the backdrop's z-index. Default: `ModalLayer::Base`
    /// (`z-[1000]`, today's behavior for every existing caller). Pass
    /// `ModalLayer::Elevated` only when this modal must be able to open on
    /// top of another already-open `Modal`.
    #[prop(default = ModalLayer::Base)]
    layer: ModalLayer,
    /// Optional minimum height for the scrollable content area (e.g. `"320px"`).
    /// Prevents layout shift when async content transitions from skeleton to real
    /// data — the modal body maintains this minimum size regardless of loading state.
    /// Does not cap the maximum height; content can still expand beyond this value.
    #[prop(optional, into)]
    content_min_height: Option<String>,
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
        "bg-background text-foreground rounded-lg shadow-xl animate-zoom-fade-in {} w-full mx-2 sm:mx-4 max-h-[95vh] sm:max-h-[90vh] flex flex-col",
        size.class()
    );

    // Build the inline style for the content area's min-height, if provided.
    let content_style = content_min_height
        .map(|h| format!("min-height: {h}"))
        .unwrap_or_default();

    // Backdrop z-index — Base (z-[1000], today's default) unless the caller
    // opted into ModalLayer::Elevated (z-[1050]) to open on top of another
    // Modal. See ModalLayer's doc comment and DESIGN.md's stacking scale.
    let overlay_class = format!(
        "fixed inset-0 flex items-center justify-center {} font-sans bg-[var(--color-overlay)] animate-fade-in-fast",
        layer.class()
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
            //   + `background-color: var(--color-overlay)` which is `rgba(0,0,0,0.5)` → `bg-[var(--color-overlay)]`
            // z-index comes from `layer` (ModalLayer::Base → z-[1000] by default; see overlay_class above).
            <div
                class=overlay_class.clone()
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
                            {move || title.get().unwrap_or_default()}
                        </h2>
                        // Close button
                        // React: Button variant="ghost" size="icon" → ghost icon button classes
                        <button
                            class="inline-flex items-center justify-center rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 w-9 hover:bg-secondary hover:text-accent-foreground text-muted-foreground hover:text-foreground"
                            on:click=move |_| on_close.run(())
                            aria-label="Close"
                        >
                            <Icon icon=phosphor_leptos::X size="24px" />
                        </button>
                    </div>

                    // Content — scrollable
                    // React: `p-4 sm:p-6 overflow-y-auto flex-1`
                    // The `content_style` applies an optional min-height so skeleton
                    // placeholders prevent layout shift when async data loads.
                    <div class="p-4 sm:p-6 overflow-y-auto flex-1" style=content_style.clone()>
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Extracts the number inside a Tailwind arbitrary z-index class like
    /// `"z-[1050]"`. Panics with a clear message if the class isn't in that
    /// shape — that would mean the format itself changed underneath these
    /// tests, which is worth knowing loudly rather than silently.
    fn z_index_value(class: &str) -> u32 {
        let inner = class
            .strip_prefix("z-[")
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or_else(|| panic!("expected a `z-[N]` class, got {class:?}"));
        inner
            .parse()
            .unwrap_or_else(|_| panic!("expected a numeric z-index, got {inner:?} from {class:?}"))
    }

    #[test]
    fn default_layer_is_base() {
        // KYO-434: a silent change to Modal's default layer would restack
        // every modal in the app, since every existing caller relies on
        // getting Base without asking for it.
        assert_eq!(ModalLayer::default(), ModalLayer::Base);
    }

    #[test]
    fn base_layer_preserves_todays_z_1000() {
        assert_eq!(
            ModalLayer::Base.class(),
            "z-[1000]",
            "Base must keep the exact z-index every Modal caller had before ModalLayer existed"
        );
    }

    #[test]
    fn elevated_layer_paints_above_base_and_below_tooltip() {
        let base = z_index_value(ModalLayer::Base.class());
        let elevated = z_index_value(ModalLayer::Elevated.class());
        // Tooltip's z-index — crates/kyomi-ui-components/src/components/tooltip.rs
        // `CONTENT_CLASS`. Kept as a literal (not imported) so this test
        // fails loudly if either component's z-index drifts independently.
        let tooltip = 1100;
        assert!(
            elevated > base,
            "Elevated ({elevated}) must paint above Base ({base}) or modal-over-modal stacking (KYO-434) regresses"
        );
        assert!(
            elevated < tooltip,
            "Elevated ({elevated}) must stay below Tooltip's z-[1100] ({tooltip}) so tooltips still render above modals"
        );
    }
}
