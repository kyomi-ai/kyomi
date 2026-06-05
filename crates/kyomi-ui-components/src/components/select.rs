// SPDX-License-Identifier: AGPL-3.0-or-later

//! Unified Select component — single/multi, static/dynamic, searchable.
//!
//! Replaces the previous `StyledSelect`, `DynSelect`, and `MultiSelectDropdown`
//! with one component that covers all use cases via props.

use leptos::prelude::*;
use phosphor_leptos::Icon;

// ---------------------------------------------------------------------------
// CSS class constants — shared shadcn/Radix styling
// ---------------------------------------------------------------------------

const TRIGGER_CLASS: &str = "flex h-9 w-full items-center justify-between whitespace-nowrap rounded-md border border-input bg-transparent px-3 py-2 text-sm text-foreground shadow-sm ring-offset-background focus:outline-none focus:ring-1 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50 cursor-pointer";

const CONTENT_CLASS: &str = "max-h-[min(40vh,25rem)] min-w-[8rem] overflow-hidden scrollbar-thin rounded-md border border-border bg-popover text-popover-foreground shadow-md animate-slide-fade-in";

const ITEM_CLASS: &str = "relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 pl-2 pr-8 text-sm outline-none transition-colors hover:bg-secondary hover:text-accent-foreground";

const CHECK_CLASS: &str = "absolute right-2 flex h-3.5 w-3.5 items-center justify-center";

const CHEVRON_CLASS: &str = "h-4 w-4 opacity-50";

const SEARCH_CLASS: &str = "w-full px-3 py-2 text-sm bg-transparent text-foreground placeholder:text-muted-foreground border-b border-border focus:outline-none";

// ---------------------------------------------------------------------------
// Select — unified component
// ---------------------------------------------------------------------------

/// Unified select dropdown with optional search and multi-select support.
///
/// # Single select (default)
/// Renders a dropdown with a list of options. Clicking an option selects it
/// and closes the dropdown. The trigger shows the selected option's label.
///
/// # Searchable
/// When `searchable=true`, a text input appears at the top of the dropdown
/// for client-side filtering. Auto-focuses on open, clears on close.
///
/// # Multi-select
/// When `multi=true`, options have checkboxes and clicking toggles selection
/// without closing the dropdown. The `value` signal holds comma-separated
/// selected values. The trigger shows "{N} selected".
#[component]
pub fn Select(
    /// Current value (reactive). For multi-select, comma-separated.
    value: Signal<String>,
    /// Options list (reactive). Each entry is `(value, label)`.
    options: Signal<Vec<(String, String)>>,
    /// Callback when the user picks an option.
    on_change: impl Fn(String) + 'static + Send + Sync,
    /// Show search input in dropdown for filtering.
    #[prop(default = false)]
    searchable: bool,
    /// Allow multiple selections (comma-separated value).
    #[prop(default = false)]
    multi: bool,
    /// Placeholder shown when value is empty.
    #[prop(optional, into)]
    placeholder: Option<String>,
    /// When true the trigger is disabled and the dropdown will not open.
    #[prop(optional, into)]
    disabled: Option<Signal<bool>>,
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);
    let (search_query, set_search_query) = signal(String::new());
    let trigger_ref = NodeRef::<leptos::html::Div>::new();
    let search_ref = NodeRef::<leptos::html::Input>::new();
    let placeholder = placeholder.unwrap_or_default();

    let filtered_options = Memo::new(move |_| {
        let query = search_query.get().to_lowercase();
        let opts = options.get();
        if query.is_empty() {
            opts
        } else {
            opts.into_iter()
                .filter(|(_, label)| label.to_lowercase().contains(&query))
                .collect()
        }
    });

    // For multi-select: parse comma-separated values into a set
    let selected_set = Memo::new(move |_| {
        let val = value.get();
        if val.is_empty() {
            Vec::<String>::new()
        } else {
            val.split(',').map(|s| s.to_string()).collect::<Vec<_>>()
        }
    });

    // Display label for the trigger button
    let placeholder_for_label = placeholder.clone();
    let display_label = Memo::new(move |_| {
        let val = value.get();
        if val.is_empty() {
            return placeholder_for_label.clone();
        }
        if multi {
            let count = selected_set.get().len();
            if count == 0 {
                placeholder_for_label.clone()
            } else {
                format!("{count} selected")
            }
        } else {
            options
                .get()
                .iter()
                .find(|(v, _)| *v == val)
                .map(|(_, l)| l.clone())
                .unwrap_or_else(|| val.clone())
        }
    });

    let on_trigger_click = move |_| {
        if disabled.map(|d| d.get()).unwrap_or(false) {
            return;
        }
        set_is_open.update(|open| *open = !*open);
    };

    // Clear search when dropdown closes
    Effect::new(move |_| {
        if !is_open.get() {
            set_search_query.set(String::new());
        }
    });

    // Auto-focus search input when dropdown opens
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        Effect::new(move |_| {
            if is_open.get() && searchable {
                let search_ref = search_ref;
                let Some(window) = web_sys::window() else { return };
                let cb = Closure::once_into_js(move || {
                    if let Some(el) = search_ref.get_untracked() {
                        let _ = el.focus();
                    }
                });
                let _ = window.request_animation_frame(cb.unchecked_ref());
            }
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = search_ref;

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
                // Search input (when searchable)
                {searchable.then(|| view! {
                    <input
                        node_ref=search_ref
                        type="text"
                        class=SEARCH_CLASS
                        placeholder="Search..."
                        prop:value=move || search_query.get()
                        on:input=move |ev| {
                            set_search_query.set(event_target_value(&ev));
                        }
                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                            if ev.key() == "Escape" {
                                set_is_open.set(false);
                            }
                        }
                    />
                })}

                // Options list
                <div role="listbox" class=if searchable {
                    "overflow-y-auto overflow-x-hidden max-h-[min(calc(40vh-2.5rem),calc(25rem-2.5rem))]"
                } else {
                    "overflow-y-auto overflow-x-hidden max-h-[min(40vh,25rem)]"
                }>
                    {move || {
                        let opts: Vec<(String, String)> = filtered_options.get();
                        if opts.is_empty() && searchable {
                            return view! {
                                <div class="py-6 text-center text-sm text-muted-foreground">
                                    "No results found"
                                </div>
                            }.into_any();
                        }
                        opts.into_iter().map(|(val_str, label_str)| {
                            let val_for_check = val_str.clone();
                            let val_for_click = val_str.clone();
                            let val_for_icon = val_str.clone();
                            if multi {
                                view! {
                                    <div
                                        class=ITEM_CLASS
                                        role="option"
                                        aria-selected=move || selected_set.get().contains(&val_for_check).to_string()
                                        on:click=move |_| {
                                            let current = selected_set.get_untracked();
                                            let new_values: Vec<String> = if current.contains(&val_for_click) {
                                                current.into_iter().filter(|v| *v != val_for_click).collect()
                                            } else {
                                                let mut v = current;
                                                v.push(val_for_click.clone());
                                                v
                                            };
                                            on_change_stored.with_value(|cb| cb(new_values.join(",")));
                                        }
                                    >
                                        <span class=CHECK_CLASS>
                                            {move || {
                                                selected_set.get().contains(&val_for_icon).then(|| {
                                                    view! {
                                                        <Icon icon=phosphor_leptos::CHECK attr:class="h-4 w-4"/>
                                                    }
                                                })
                                            }}
                                        </span>
                                        {label_str}
                                    </div>
                                }.into_any()
                            } else {
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
                                }.into_any()
                            }
                        }).collect_view().into_any()
                    }}
                </div>
            </crate::components::popover::Popover>
        </div>
    }
}

// ---------------------------------------------------------------------------
// StaticSelect — convenience wrapper for static option lists
// ---------------------------------------------------------------------------

/// Convenience wrapper for selects with static, known-at-compile-time options.
///
/// Internally creates signals and delegates to [`Select`]. Use this when your
/// options are fixed (e.g. sort orders, roles, page sizes).
#[component]
pub fn StaticSelect(
    #[prop(into)] value: String,
    options: Vec<(&'static str, &'static str)>,
    on_change: impl Fn(String) + 'static + Send + Sync,
    #[prop(optional, into)]
    disabled: Option<Signal<bool>>,
) -> impl IntoView {
    let (selected, set_selected) = signal(value);
    let options_vec: Vec<(String, String)> = options
        .iter()
        .map(|(v, l)| (v.to_string(), l.to_string()))
        .collect();
    let options_signal = Signal::derive(move || options_vec.clone());
    let value_signal = Signal::derive(move || selected.get());
    let on_change_fn = move |v: String| {
        set_selected.set(v.clone());
        on_change(v);
    };

    match disabled {
        Some(sig) => view! {
            <Select value=value_signal options=options_signal on_change=on_change_fn disabled=sig />
        }.into_any(),
        None => view! {
            <Select value=value_signal options=options_signal on_change=on_change_fn />
        }.into_any(),
    }
}
