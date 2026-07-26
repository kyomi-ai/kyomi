// SPDX-License-Identifier: AGPL-3.0-or-later

//! Global status bar for pending workspace invitations.
//!
//! Shown at the bottom of the Layout shell when the logged-in user has
//! pending invitations. Uses the shared [`StatusBar`] primitive from
//! `kyomi-ui-components`.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::button::{Button, ButtonSize, ButtonVariant};
use crate::components::status_bar::{StatusBar, StatusBarVariant};
use crate::components::toast::{toast_error, toast_success};
use crate::server_fns::profile::{accept_invitation, decline_invitation, get_pending_invitations};
use crate::types::InvitationData;

/// Indefinite article for a humanized role label — "an Admin", "a Member".
/// Fixes KYO-169: the previous inline check compared the RAW token
/// (`workspace_admin`) against `"admin"` and never matched, so admin invites
/// always rendered "a" instead of "an".
fn role_article(role_label: &str) -> &'static str {
    if role_label.eq_ignore_ascii_case("admin") {
        "an"
    } else {
        "a"
    }
}

/// Global bottom bar showing pending workspace invitations.
///
/// Renders nothing when there are no invitations or while the initial fetch
/// is still in flight — avoids a flash of an empty bar on first load.
#[component]
pub fn InvitationStatusBar() -> impl IntoView {
    // Client-only fetch — LocalResource does not participate in SSR, matching
    // the pattern used by the Layout for sidebar data.
    let invitations_resource = LocalResource::new(get_pending_invitations);

    // Mutable copy for optimistic removal on accept/decline.
    let (inv_list, set_inv_list) = signal(Vec::<InvitationData>::new());

    // Keep the signal in sync when the resource resolves (or re-resolves).
    Effect::new(move |_| {
        if let Some(Ok(invs)) = invitations_resource.get() {
            set_inv_list.set(invs);
        }
    });

    // ── Accept action ───────────────────────────────────────────────────────
    let accept_action = Action::new(|id: &String| {
        let id = id.clone();
        async move { accept_invitation(id).await }
    });

    // Watch accept results.
    Effect::new(move |_| {
        if let Some(result) = accept_action.value().get() {
            match result {
                Ok(()) => {
                    toast_success("Invitation accepted — reloading…");
                    #[cfg(target_arch = "wasm32")]
                    if let Some(win) = web_sys::window() {
                        let _ = win.location().reload();
                    }
                }
                Err(e) => {
                    toast_error(format!("Failed to accept invitation: {e}"));
                    // Re-fetch to restore consistent state.
                    // The resource will re-resolve and sync back into inv_list.
                    invitations_resource.refetch();
                }
            }
        }
    });

    // ── Decline action ──────────────────────────────────────────────────────
    let decline_action = Action::new(|id: &String| {
        let id = id.clone();
        async move { decline_invitation(id).await }
    });

    // Watch decline results.
    Effect::new(move |_| {
        if let Some(result) = decline_action.value().get() {
            match result {
                Ok(()) => {
                    toast_success("Invitation declined");
                }
                Err(e) => {
                    toast_error(format!("Failed to decline invitation: {e}"));
                    invitations_resource.refetch();
                }
            }
        }
    });

    view! {
        {move || {
            let invs = inv_list.get();
            if invs.is_empty() {
                return ().into_any();
            }

            let total = invs.len();

            // Clone all needed fields from the first invitation so we don't
            // borrow across the view boundary.
            let first_id = invs[0].invitation_id.clone();
            let first_role = invs[0].role_display.clone();
            let first_workspace = invs[0]
                .workspace_name
                .as_deref()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or("a workspace")
                .to_string();
            let first_inviter = invs[0]
                .inviter_name
                .as_deref()
                .filter(|n| !n.trim().is_empty())
                .map(|s| s.to_string());

            // Build the message.
            let role_article = role_article(&first_role);

            let message = match first_inviter.as_deref() {
                Some(inviter) if total > 1 => {
                    format!(
                        "You have {total} pending invitations. {inviter} invited you to join {first_workspace} as {role_article} {first_role}.",
                    )
                }
                Some(inviter) => {
                    format!(
                        "{inviter} invited you to join {first_workspace} as {role_article} {first_role}.",
                    )
                }
                None if total > 1 => {
                    format!(
                        "You have {total} pending invitations. You were invited to join {first_workspace} as {role_article} {first_role}.",
                    )
                }
                None => {
                    format!(
                        "You were invited to join {first_workspace} as {role_article} {first_role}.",
                    )
                }
            };

            view! {
                <StatusBar variant=StatusBarVariant::Warning>
                    <div class="flex items-center gap-3 min-w-0">
                        <span class="text-warning-foreground flex-shrink-0">
                            <Icon icon=phosphor_leptos::USERS weight=IconWeight::Regular size="20px"/>
                        </span>
                        <p class="text-sm text-foreground truncate">{message}</p>
                    </div>
                    <div class="flex items-center gap-2 flex-shrink-0">
                        <Button
                            variant=ButtonVariant::Default
                            size=ButtonSize::Sm
                            on:click={
                                let id = first_id.clone();
                                let remove_id = first_id.clone();
                                move |_| {
                                    accept_action.dispatch(id.clone());
                                    set_inv_list.update(|list| {
                                        list.retain(|i| i.invitation_id != remove_id);
                                    });
                                }
                            }
                        >
                            "Accept"
                        </Button>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            on:click={
                                let id = first_id.clone();
                                let remove_id = first_id;
                                move |_| {
                                    decline_action.dispatch(id.clone());
                                    set_inv_list.update(|list| {
                                        list.retain(|i| i.invitation_id != remove_id);
                                    });
                                }
                            }
                        >
                            "Decline"
                        </Button>
                    </div>
                </StatusBar>
            }.into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::role_article;
    #[test]
    fn admin_label_gets_an_article() {
        assert_eq!(role_article("Admin"), "an");
    }
    #[test]
    fn member_and_viewer_labels_get_a_article() {
        assert_eq!(role_article("Member"), "a");
        assert_eq!(role_article("Viewer"), "a");
    }
    #[test]
    fn article_is_case_insensitive() {
        assert_eq!(role_article("admin"), "an");
    }
}
