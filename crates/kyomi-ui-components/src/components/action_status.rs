// SPDX-License-Identifier: AGPL-3.0-or-later

//! Action status indicator — shows saving/saved/error state.
//!
//! Derives state from a Leptos `Action`'s pending and value signals.
//! Used alongside auto-save patterns in settings cards.

use leptos::prelude::*;

/// Displays save status (saving/saved/error) from an action.
///
/// Place this next to a card title to show feedback when the user
/// changes a setting and it auto-saves.
#[component]
pub fn ActionStatus<I: 'static, O: Clone + 'static>(
    action: Action<I, Result<O, ServerFnError>>,
) -> impl IntoView {
    let pending = action.pending();
    let value = action.value();

    move || {
        if pending.get() {
            view! {
                <span class="inline-flex items-center gap-1 text-xs text-muted-foreground">"Saving..."</span>
            }
            .into_any()
        } else {
            match value.get() {
                Some(Ok(_)) => view! {
                    <span class="inline-flex items-center gap-1 text-xs text-success-foreground">"Saved"</span>
                }
                .into_any(),
                Some(Err(e)) => {
                    let msg = e.to_string();
                    view! {
                        <span class="inline-flex items-center gap-1 text-xs text-error-foreground">{msg}</span>
                    }
                    .into_any()
                }
                None => view! { <span></span> }.into_any(),
            }
        }
    }
}
