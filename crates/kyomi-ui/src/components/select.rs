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
use leptos_icons::Icon;

/// Classes kept for backward compatibility with code that uses raw `<select>` elements
/// with `SELECT_CLASS` and `CHEVRON_STYLE` directly.
pub const SELECT_CLASS: &str = "flex h-9 w-full items-center justify-between whitespace-nowrap rounded-md border border-input bg-transparent px-3 py-2 text-sm text-foreground shadow-sm ring-offset-background focus:outline-none focus:ring-1 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50 appearance-none cursor-pointer";

/// Inline chevron SVG as background image (for raw `<select>` elements).
pub const CHEVRON_STYLE: &str = "background-image: url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%2364748b' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E\"); background-repeat: no-repeat; background-position: right 0.75rem center; padding-right: 2.5rem;";

// ---------------------------------------------------------------------------
// CSS class constants copied verbatim from the React Select component
// ---------------------------------------------------------------------------

/// SelectTrigger classes from React source.
const TRIGGER_CLASS: &str = "flex h-9 w-full items-center justify-between whitespace-nowrap rounded-md border border-input bg-transparent px-3 py-2 text-sm text-foreground shadow-sm ring-offset-background focus:outline-none focus:ring-1 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50 cursor-pointer";

/// SelectContent classes from React source (positioned absolutely below trigger).
const CONTENT_CLASS: &str = "absolute left-0 top-full mt-1 z-[1100] max-h-96 min-w-[8rem] w-full overflow-y-auto overflow-x-hidden rounded-md border border-border bg-popover text-popover-foreground shadow-md p-1 animate-slide-fade-in";

/// SelectItem classes from React source.
const ITEM_CLASS: &str = "relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 pl-2 pr-8 text-sm outline-none transition-colors hover:bg-accent hover:text-accent-foreground";

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
#[component]
pub fn StyledSelect(
    #[prop(into)] value: String,
    options: Vec<(&'static str, &'static str)>,
    on_change: impl Fn(String) + 'static + Send + Sync,
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);
    let (selected, set_selected) = signal(value);
    let container_ref = NodeRef::<leptos::html::Div>::new();

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

    // Toggle dropdown on trigger click
    let on_trigger_click = move |_| {
        set_is_open.update(|open| *open = !*open);
    };

    // Close on Escape key
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape" {
            set_is_open.set(false);
        }
    };

    // Click-outside detection: register a window click listener when open.
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;

        let cleanup: StoredValue<Option<SendWrapper<Box<dyn FnOnce()>>>> =
            StoredValue::new(None);

        Effect::new(move |_| {
            // Clean up any previous listener first
            if let Some(teardown) = cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }

            if is_open.get() {
                let window = web_sys::window().expect("window");
                let container_el = container_ref.get();

                let cb = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
                    if let Some(target) = ev.target() {
                        let target_node: web_sys::Node = target.unchecked_into();
                        if let Some(ref el) = container_el {
                            let html_el: &web_sys::HtmlElement = el;
                            let node: &web_sys::Node = html_el.as_ref();
                            if !node.contains(Some(&target_node)) {
                                set_is_open.set(false);
                            }
                        } else {
                            set_is_open.set(false);
                        }
                    }
                });

                // Use capture phase so we get the event before it reaches the trigger
                let _ = window.add_event_listener_with_callback_and_bool(
                    "click",
                    cb.as_ref().unchecked_ref(),
                    true,
                );

                let window_clone = window.clone();
                let cb_ref: js_sys::Function =
                    cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
                // Keep the Closure alive in the teardown (not leaked via forget).
                let teardown: Box<dyn FnOnce()> = Box::new(move || {
                    let _ = window_clone.remove_event_listener_with_callback_and_bool(
                        "click",
                        &cb_ref,
                        true,
                    );
                    drop(cb);
                });

                cleanup.set_value(Some(SendWrapper::new(teardown)));
            }
        });

        // Cleanup on unmount
        on_cleanup(move || {
            if let Some(teardown) = cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }
        });
    }

    let on_change = std::sync::Arc::new(on_change);

    view! {
        <div
            node_ref=container_ref
            class="relative w-full"
            on:keydown=on_keydown
        >
            // Trigger button
            <button
                type="button"
                class=TRIGGER_CLASS
                on:click=on_trigger_click
                aria-expanded=move || is_open.get().to_string()
                aria-haspopup="listbox"
            >
                <span class="line-clamp-1">{move || display_label.get()}</span>
                <Icon icon=icondata_lu::LuChevronDown attr:class=CHEVRON_CLASS/>
            </button>

            // Dropdown content
            {move || {
                let on_change = on_change.clone();
                is_open.get().then(|| {
                    let items = options.iter().map(|(val, label)| {
                        let val_owned = val.to_string();
                        let val_for_check = val.to_string();
                        let val_for_click = val.to_string();
                        let on_change = on_change.clone();
                        view! {
                            <div
                                class=ITEM_CLASS
                                role="option"
                                aria-selected=move || (selected.get() == val_for_check).to_string()
                                on:click=move |_| {
                                    let v = val_for_click.clone();
                                    set_selected.set(v.clone());
                                    on_change(v);
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
                                                    <Icon icon=icondata_lu::LuCheck attr:class="h-4 w-4"/>
                                                }
                                            })
                                        }
                                    }
                                </span>
                            </div>
                        }
                    }).collect_view();

                    view! {
                        <div
                            class=CONTENT_CLASS
                            role="listbox"
                        >
                            {items}
                        </div>
                    }
                })
            }}
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
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);
    let container_ref = NodeRef::<leptos::html::Div>::new();
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

    let on_trigger_click = move |_| {
        set_is_open.update(|open| *open = !*open);
    };

    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape" {
            set_is_open.set(false);
        }
    };

    // Click-outside detection
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;

        let cleanup: StoredValue<Option<SendWrapper<Box<dyn FnOnce()>>>> =
            StoredValue::new(None);

        Effect::new(move |_| {
            if let Some(teardown) = cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }

            if is_open.get() {
                let window = web_sys::window().expect("window");
                let container_el = container_ref.get();

                let cb = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
                    if let Some(target) = ev.target() {
                        let target_node: web_sys::Node = target.unchecked_into();
                        if let Some(ref el) = container_el {
                            let html_el: &web_sys::HtmlElement = el;
                            let node: &web_sys::Node = html_el.as_ref();
                            if !node.contains(Some(&target_node)) {
                                set_is_open.set(false);
                            }
                        } else {
                            set_is_open.set(false);
                        }
                    }
                });

                let _ = window.add_event_listener_with_callback_and_bool(
                    "click",
                    cb.as_ref().unchecked_ref(),
                    true,
                );

                let window_clone = window.clone();
                let cb_ref: js_sys::Function =
                    cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
                let teardown: Box<dyn FnOnce()> = Box::new(move || {
                    let _ = window_clone.remove_event_listener_with_callback_and_bool(
                        "click",
                        &cb_ref,
                        true,
                    );
                    drop(cb);
                });
                cleanup.set_value(Some(SendWrapper::new(teardown)));
            }
        });

        on_cleanup(move || {
            if let Some(teardown) = cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }
        });
    }

    let on_change = std::sync::Arc::new(on_change);
    let placeholder_for_class = placeholder.clone();

    view! {
        <div
            node_ref=container_ref
            class="relative w-full"
            on:keydown=on_keydown
        >
            <button
                type="button"
                class=TRIGGER_CLASS
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
                <Icon icon=icondata_lu::LuChevronDown attr:class=CHEVRON_CLASS/>
            </button>

            {move || {
                let on_change = on_change.clone();
                is_open.get().then(|| {
                    let current_options = options.get();
                    let items = current_options.into_iter().map(|(val_str, label_str)| {
                        let val_for_check = val_str.clone();
                        let val_for_click = val_str.clone();
                        let val_for_icon = val_str.clone();
                        let on_change = on_change.clone();
                        view! {
                            <div
                                class=ITEM_CLASS
                                role="option"
                                aria-selected=move || (value.get() == val_for_check).to_string()
                                on:click=move |_| {
                                    on_change(val_for_click.clone());
                                    set_is_open.set(false);
                                }
                            >
                                {label_str}
                                <span class=CHECK_CLASS>
                                    {move || {
                                        (value.get() == val_for_icon).then(|| {
                                            view! {
                                                <Icon icon=icondata_lu::LuCheck attr:class="h-4 w-4"/>
                                            }
                                        })
                                    }}
                                </span>
                            </div>
                        }
                    }).collect_view();

                    view! {
                        <div class=CONTENT_CLASS role="listbox">
                            {items}
                        </div>
                    }
                })
            }}
        </div>
    }
}
