// SPDX-License-Identifier: AGPL-3.0-or-later

//! Custom Select dropdown component matching the shadcn/Radix Select from
//! `apps/frontend/src/components/ui/select.jsx`.
//!
//! Replaces the native `<select>` with a fully custom dropdown that visually
//! matches the React frontend. Uses absolute positioning within a relative
//! container (no portals).
//!
//! ## Click-outside detection
//!
//! When the dropdown is open, a `click` listener is added to `window` that
//! checks whether the event target is inside the component container. If not,
//! the dropdown is closed. The listener is cleaned up when the dropdown closes
//! or the component is unmounted.

use leptos::prelude::*;
use phosphor_leptos::Icon;

// ---------------------------------------------------------------------------
// CSS class constants copied verbatim from the React Select component
// ---------------------------------------------------------------------------

/// SelectTrigger classes from React source.
const TRIGGER_CLASS: &str = "flex h-9 w-full items-center justify-between whitespace-nowrap rounded-md border border-input bg-transparent px-3 py-2 text-sm text-foreground shadow-sm ring-offset-background focus:outline-none focus:ring-1 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50 cursor-pointer";

/// SelectContent classes — visual chrome only. Positioning is handled by
/// the shared `Popover` helper, which portals the content to `document.body`
/// and flips/shifts to stay inside the viewport.
const CONTENT_CLASS: &str = "max-h-[min(40vh,25rem)] min-w-[8rem] overflow-y-auto overflow-x-hidden scrollbar-thin rounded-md border border-border bg-popover text-popover-foreground shadow-md p-1 animate-slide-fade-in";

/// SelectItem classes from React source.
const ITEM_CLASS: &str = "relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 pl-2 pr-8 text-sm outline-none transition-colors hover:bg-secondary hover:text-accent-foreground";

/// Check icon positioning (absolute right-2).
const CHECK_CLASS: &str = "absolute right-2 flex h-3.5 w-3.5 items-center justify-center";

/// Chevron icon classes on the trigger button.
const CHEVRON_CLASS: &str = "h-4 w-4 opacity-50";

/// A custom Select dropdown that visually matches the shadcn/Radix Select component.
///
/// # Props
/// - `value` — the currently selected value.
/// - `options` — `Vec<(&'static str, &'static str)>` of `(value, label)` pairs.
/// - `on_change` — callback fired when the user selects an option.
/// - `disabled` — optional signal; when true the trigger is disabled and the dropdown will not open.
#[component]
pub fn StyledSelect(
    #[prop(into)] value: String,
    options: Vec<(&'static str, &'static str)>,
    on_change: impl Fn(String) + 'static + Send + Sync,
    #[prop(optional, into)]
    disabled: Option<Signal<bool>>,
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);
    let (selected, set_selected) = signal(value);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    // Find the label for the currently selected value
    let options_clone = options.clone();
    let display_label = Memo::new(move |_| {
        let val = selected.get();
        options_clone
            .iter()
            .find(|(v, _)| *v == val.as_str())
            .map(|(_, l)| l.to_string())
            .unwrap_or_else(|| val.clone())
    });

    // Toggle dropdown on trigger click — no-op when disabled.
    let on_trigger_click = move |_| {
        if disabled.map(|d| d.get()).unwrap_or(false) {
            return;
        }
        set_is_open.update(|open| *open = !*open);
    };

    let on_change_stored: StoredValue<std::sync::Arc<dyn Fn(String) + Send + Sync>> =
        StoredValue::new(std::sync::Arc::new(on_change));
    let options_stored = StoredValue::new(options);

    view! {
        <div node_ref=trigger_ref class="w-full">
            <button
                type="button"
                class=TRIGGER_CLASS
                disabled=move || disabled.map(|d| d.get()).unwrap_or(false)
                on:click=on_trigger_click
                aria-expanded=move || is_open.get().to_string()
                aria-haspopup="listbox"
            >
                <span class="line-clamp-1">{move || display_label.get()}</span>
                <Icon icon=phosphor_leptos::CARET_DOWN attr:class=CHEVRON_CLASS/>
            </button>

            <crate::components::popover::Popover
                trigger_ref=trigger_ref
                open=Signal::derive(move || is_open.get())
                on_close=Callback::new(move |()| set_is_open.set(false))
                placement=crate::components::popover::Placement::BOTTOM_START
                match_width=true
                class=CONTENT_CLASS
            >
                <div role="listbox">
                    {move || options_stored.with_value(|opts| {
                        opts.iter().map(|(val, label)| {
                            let val_owned = val.to_string();
                            let val_for_check = val.to_string();
                            let val_for_click = val.to_string();
                            view! {
                                <div
                                    class=ITEM_CLASS
                                    role="option"
                                    aria-selected=move || (selected.get() == val_for_check).to_string()
                                    on:click=move |_| {
                                        let v = val_for_click.clone();
                                        set_selected.set(v.clone());
                                        on_change_stored.with_value(|cb| cb(v));
                                        set_is_open.set(false);
                                    }
                                >
                                    {*label}
                                    <span class=CHECK_CLASS>
                                        {
                                            let val_check = val_owned.clone();
                                            move || {
                                                (selected.get() == val_check).then(|| {
                                                    view! {
                                                        <Icon icon=phosphor_leptos::CHECK attr:class="h-4 w-4"/>
                                                    }
                                                })
                                            }
                                        }
                                    </span>
                                </div>
                            }
                        }).collect_view()
                    })}
                </div>
            </crate::components::popover::Popover>
        </div>
    }
}

// ---------------------------------------------------------------------------
// DynSelect — reactive version for dynamic String options
// ---------------------------------------------------------------------------

/// Like `StyledSelect` but accepts reactive `Signal` props for both value and
/// options. Use this when options come from server responses or change at
/// runtime.
#[component]
pub fn DynSelect(
    /// Current value (reactive).
    value: Signal<String>,
    /// Options list (reactive). Each entry is `(value, label)`.
    options: Signal<Vec<(String, String)>>,
    /// Callback when the user picks an option.
    on_change: impl Fn(String) + 'static + Send + Sync,
    /// Placeholder shown when value is empty.
    #[prop(optional, into)]
    placeholder: Option<String>,
    /// When true the trigger is disabled and the dropdown will not open.
    #[prop(optional, into)]
    disabled: Option<Signal<bool>>,
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();
    let placeholder = placeholder.unwrap_or_default();

    let placeholder_for_label = placeholder.clone();
    let display_label = Memo::new(move |_| {
        let val = value.get();
        if val.is_empty() {
            return placeholder_for_label.clone();
        }
        options
            .get()
            .iter()
            .find(|(v, _)| *v == val)
            .map(|(_, l)| l.clone())
            .unwrap_or_else(|| val.clone())
    });

    // Toggle dropdown on trigger click — no-op when disabled.
    let on_trigger_click = move |_| {
        if disabled.map(|d| d.get()).unwrap_or(false) {
            return;
        }
        set_is_open.update(|open| *open = !*open);
    };

    let on_change_stored: StoredValue<std::sync::Arc<dyn Fn(String) + Send + Sync>> =
        StoredValue::new(std::sync::Arc::new(on_change));
    let placeholder_for_class = placeholder.clone();

    view! {
        <div node_ref=trigger_ref class="w-full">
            <button
                type="button"
                class=TRIGGER_CLASS
                disabled=move || disabled.map(|d| d.get()).unwrap_or(false)
                on:click=on_trigger_click
                aria-expanded=move || is_open.get().to_string()
                aria-haspopup="listbox"
            >
                <span class=move || {
                    if value.get().is_empty() && !placeholder_for_class.is_empty() {
                        "line-clamp-1 text-muted-foreground"
                    } else {
                        "line-clamp-1"
                    }
                }>{move || display_label.get()}</span>
                <Icon icon=phosphor_leptos::CARET_DOWN attr:class=CHEVRON_CLASS/>
            </button>

            <crate::components::popover::Popover
                trigger_ref=trigger_ref
                open=Signal::derive(move || is_open.get())
                on_close=Callback::new(move |()| set_is_open.set(false))
                placement=crate::components::popover::Placement::BOTTOM_START
                match_width=true
                class=CONTENT_CLASS
            >
                <div role="listbox">
                    {move || {
                        options.get().into_iter().map(|(val_str, label_str)| {
                            let val_for_check = val_str.clone();
                            let val_for_click = val_str.clone();
                            let val_for_icon = val_str.clone();
                            view! {
                                <div
                                    class=ITEM_CLASS
                                    role="option"
                                    aria-selected=move || (value.get() == val_for_check).to_string()
                                    on:click=move |_| {
                                        let v = val_for_click.clone();
                                        on_change_stored.with_value(|cb| cb(v));
                                        set_is_open.set(false);
                                    }
                                >
                                    {label_str}
                                    <span class=CHECK_CLASS>
                                        {move || {
                                            (value.get() == val_for_icon).then(|| {
                                                view! {
                                                    <Icon icon=phosphor_leptos::CHECK attr:class="h-4 w-4"/>
                                                }
                                            })
                                        }}
                                    </span>
                                </div>
                            }
                        }).collect_view()
                    }}
                </div>
            </crate::components::popover::Popover>
        </div>
    }
}
