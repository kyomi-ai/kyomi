// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team management page — workspace member and invitation management.
//!
//! Replaces `apps/frontend/src/components/settings/TeamManagement.jsx`.
//! All data fetching uses server functions instead of REST API calls.

use std::sync::Arc;

use leptos::prelude::*;

use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, ConfirmDialog, Modal, ModalSize,
    INPUT_CLASS,
};
use crate::server_fns::context::get_user_context;
use crate::server_fns::team::*;
use crate::types::{OwnershipTransferData, TeamInvitation, TeamMember};

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn TeamPage() -> impl IntoView {
    // Load user context for current user ID
    let user_ctx = Resource::new(|| (), |_| get_user_context());

    // Resources for members, invitations, transfers
    let (members_version, set_members_version) = signal(0u32);
    let (invitations_version, set_invitations_version) = signal(0u32);
    let (transfers_version, set_transfers_version) = signal(0u32);

    let members = Resource::new(
        move || members_version.get(),
        |_| list_workspace_members(),
    );
    let invitations = Resource::new(
        move || invitations_version.get(),
        |_| list_workspace_invitations(),
    );
    let transfers = Resource::new(
        move || transfers_version.get(),
        |_| list_ownership_transfers(),
    );

    // Invite modal state
    let (show_invite_modal, set_show_invite_modal) = signal(false);
    let (invite_email, set_invite_email) = signal(String::new());
    let (invite_role, set_invite_role) = signal("user".to_string());

    // Confirm dialog state
    let dialog_open = RwSignal::new(false);
    let (dialog_title, set_dialog_title) = signal(String::new());
    let (dialog_message, set_dialog_message) = signal(String::new());
    let (dialog_confirm_text, set_dialog_confirm_text) = signal("Confirm".to_string());
    let (pending_action, set_pending_action) =
        signal(Option::<PendingAction>::None);

    // Actions
    let invite_action = Action::new(move |(email, role): &(String, String)| {
        let email = email.clone();
        let role = role.clone();
        async move { invite_member(email, role).await }
    });

    let cancel_invite_action = Action::new(move |id: &String| {
        let id = id.clone();
        async move { cancel_invitation(id).await }
    });

    let update_role_action = Action::new(move |(user_id, role): &(String, String)| {
        let user_id = user_id.clone();
        let role = role.clone();
        async move { update_member_role(user_id, role).await }
    });

    let remove_action = Action::new(move |user_id: &String| {
        let user_id = user_id.clone();
        async move { remove_member(user_id).await }
    });

    let cancel_transfer_action = Action::new(move |id: &String| {
        let id = id.clone();
        async move { cancel_ownership_transfer(id).await }
    });

    // React to action completions — refresh relevant data
    Effect::new(move || {
        if let Some(result) = invite_action.value().get() {
            if result.is_ok() {
                set_show_invite_modal.set(false);
                set_invite_email.set(String::new());
                set_invite_role.set("user".to_string());
                set_invitations_version.update(|v| *v += 1);
            }
        }
    });

    Effect::new(move || {
        if let Some(result) = cancel_invite_action.value().get() {
            if result.is_ok() {
                set_invitations_version.update(|v| *v += 1);
            }
        }
    });

    Effect::new(move || {
        if let Some(result) = update_role_action.value().get() {
            if result.is_ok() {
                set_members_version.update(|v| *v += 1);
            }
        }
    });

    Effect::new(move || {
        if let Some(result) = remove_action.value().get() {
            if result.is_ok() {
                set_members_version.update(|v| *v += 1);
            }
        }
    });

    Effect::new(move || {
        if let Some(result) = cancel_transfer_action.value().get() {
            if result.is_ok() {
                set_transfers_version.update(|v| *v += 1);
            }
        }
    });

    // Confirm dialog callbacks
    let on_confirm = Callback::new(move |()| {
        dialog_open.set(false);
        if let Some(action) = pending_action.get_untracked() {
            match action {
                PendingAction::CancelInvitation(id) => {
                    cancel_invite_action.dispatch(id);
                }
                PendingAction::RemoveMember(id) => {
                    remove_action.dispatch(id);
                }
                PendingAction::CancelTransfer(id) => {
                    cancel_transfer_action.dispatch(id);
                }
            }
        }
        set_pending_action.set(None);
    });

    let on_cancel = Callback::new(move |()| {
        dialog_open.set(false);
        set_pending_action.set(None);
    });

    // Helper closures for opening confirm dialogs
    let request_cancel_invitation = move |id: String| {
        set_dialog_title.set("Cancel Invitation?".to_string());
        set_dialog_message.set("Are you sure you want to cancel this invitation?".to_string());
        set_dialog_confirm_text.set("Cancel Invitation".to_string());
        set_pending_action.set(Some(PendingAction::CancelInvitation(id)));
        dialog_open.set(true);
    };

    let request_remove_member = move |id: String| {
        set_dialog_title.set("Remove Team Member?".to_string());
        set_dialog_message.set(
            "Are you sure you want to remove this member from the workspace?".to_string(),
        );
        set_dialog_confirm_text.set("Remove Member".to_string());
        set_pending_action.set(Some(PendingAction::RemoveMember(id)));
        dialog_open.set(true);
    };

    let request_cancel_transfer = move |id: String| {
        set_dialog_title.set("Cancel Ownership Transfer?".to_string());
        set_dialog_message.set(
            "Are you sure you want to cancel this ownership transfer request?".to_string(),
        );
        set_dialog_confirm_text.set("Cancel Transfer".to_string());
        set_pending_action.set(Some(PendingAction::CancelTransfer(id)));
        dialog_open.set(true);
    };

    // Modal close handler
    let on_close_modal = Callback::new(move |()| {
        set_show_invite_modal.set(false);
        set_invite_email.set(String::new());
        set_invite_role.set("user".to_string());
    });

    // Modal footer — extracted to avoid type issues in view! macro
    let modal_footer: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
        let email_empty = invite_email.get().is_empty();
        view! {
            <Button
                variant=ButtonVariant::Outline
                on:click=move |_| {
                    set_show_invite_modal.set(false);
                    set_invite_email.set(String::new());
                    set_invite_role.set("user".to_string());
                }
            >
                "Cancel"
            </Button>
            <Button
                variant=ButtonVariant::Default
                on:click=move |_| {
                    let e = invite_email.get_untracked();
                    let r = invite_role.get_untracked();
                    if !e.is_empty() {
                        invite_action.dispatch((e, r));
                    }
                }
                disabled=email_empty
            >
                "Send Invitation"
            </Button>
        }.into_any()
    });

    view! {
        <div class="p-6" style="display: block; padding: 1.5rem;">
            // Header
            <div class="flex flex-col sm:flex-row sm:justify-between sm:items-center gap-4 mb-6">
                <div class="min-w-0">
                    <h2 class="text-lg sm:text-xl font-semibold text-foreground mb-1 sm:mb-2">
                        "Team Members"
                    </h2>
                    <p class="text-xs sm:text-sm text-muted-foreground">
                        "Invite team members to collaborate."
                    </p>
                </div>
                <div class="flex gap-2 flex-shrink-0">
                    // Transfer Ownership button — visible to owner only
                    {move || {
                        let current_user_id = user_ctx.get()
                            .and_then(|r| r.ok())
                            .map(|u| u.user_id.clone())
                            .unwrap_or_default();
                        let is_owner = members.get()
                            .and_then(|r| r.ok())
                            .map(|m| m.iter().any(|member| member.user_id == current_user_id && member.is_owner))
                            .unwrap_or(false);

                        if is_owner {
                            view! {
                                <Button
                                    variant=ButtonVariant::Outline
                                    attr:title="Transfer Ownership"
                                >
                                    <span class="inline-flex items-center gap-0 sm:gap-2">
                                        <span class="inline-flex">
                                            <leptos_icons::Icon icon=icondata_lu::LuArrowLeftRight width="16" height="16"/>
                                        </span>
                                        <span class="hidden sm:inline">"Transfer Ownership"</span>
                                    </span>
                                </Button>
                            }.into_any()
                        } else {
                            view! { <span></span> }.into_any()
                        }
                    }}
                    <Button
                        variant=ButtonVariant::Default
                        on:click=move |_| set_show_invite_modal.set(true)
                        attr:title="Invite Member"
                    >
                        <span class="inline-flex items-center gap-0 sm:gap-2">
                            <span class="inline-flex">
                                <leptos_icons::Icon icon=icondata_lu::LuUserPlus width="16" height="16"/>
                            </span>
                            <span class="hidden sm:inline">"Invite Member"</span>
                        </span>
                    </Button>
                </div>
            </div>

            // Pending Invitations
            <div class="mb-6">
                <h3 class="text-base sm:text-lg font-semibold text-foreground mb-4">
                    "Pending Invitations"
                </h3>
                <Suspense fallback=move || view! {
                    <div class="text-center py-8">
                        <span class="h-8 w-8 animate-spin rounded-full border-2 border-muted-foreground border-t-transparent inline-block"></span>
                        <p class="text-muted-foreground mt-2">"Loading invitations..."</p>
                    </div>
                }>
                    {move || {
                        let request_cancel = request_cancel_invitation.clone();
                        invitations.get().map(|result| match result {
                            Ok(invs) if invs.is_empty() => {
                                view! {
                                    <div class="text-center py-8 bg-muted rounded-lg border-2 border-dashed border-border">
                                        <p class="text-muted-foreground">"No pending invitations"</p>
                                    </div>
                                }.into_any()
                            },
                            Ok(invs) => {
                                view! {
                                    <div class="space-y-3">
                                        {invs.into_iter().map(|inv| {
                                            let cancel = request_cancel.clone();
                                            view! { <InvitationRow invitation=inv on_cancel=cancel/> }
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            },
                            Err(e) => {
                                let msg = e.to_string();
                                view! {
                                    <p class="text-error-foreground text-sm">{msg}</p>
                                }.into_any()
                            },
                        })
                    }}
                </Suspense>
            </div>

            // Pending Ownership Transfers
            <Suspense fallback=|| ()>
                {move || {
                    let current_user_id = user_ctx.get()
                        .and_then(|r| r.ok())
                        .map(|u| u.user_id.clone())
                        .unwrap_or_default();
                    let is_owner = members.get()
                        .and_then(|r| r.ok())
                        .map(|m| m.iter().any(|member| member.user_id == current_user_id && member.is_owner))
                        .unwrap_or(false);

                    transfers.get().map(|result| match result {
                        Ok(t) if !t.is_empty() => {
                            let cancel_fn = request_cancel_transfer.clone();
                            let title = if is_owner {
                                "Pending Ownership Transfers"
                            } else {
                                "Ownership Transfer Offers"
                            };
                            view! {
                                <div class="mb-6">
                                    <h3 class="text-lg font-semibold text-foreground mb-4">{title}</h3>
                                    <div class="space-y-4">
                                        {t.into_iter().map(|transfer| {
                                            let cancel = cancel_fn.clone();
                                            view! { <TransferRow transfer=transfer on_cancel=cancel/> }
                                        }).collect_view()}
                                    </div>
                                </div>
                            }.into_any()
                        },
                        _ => view! { <span></span> }.into_any(),
                    })
                }}
            </Suspense>

            // Workspace Members
            <div class="mb-6">
                <h3 class="text-base sm:text-lg font-semibold text-foreground mb-4">
                    "Workspace Members"
                </h3>
                <Suspense fallback=move || view! {
                    <div class="text-center py-8">
                        <span class="h-8 w-8 animate-spin rounded-full border-2 border-muted-foreground border-t-transparent inline-block"></span>
                        <p class="text-muted-foreground mt-2">"Loading members..."</p>
                    </div>
                }>
                    {move || {
                        let current_user_id = user_ctx.get()
                            .and_then(|r| r.ok())
                            .map(|u| u.user_id.clone())
                            .unwrap_or_default();
                        let remove_fn = request_remove_member.clone();

                        members.get().map(|result| match result {
                            Ok(m) if m.is_empty() => {
                                view! {
                                    <div class="text-center py-8 bg-muted rounded-lg border-2 border-dashed border-border">
                                        <p class="text-muted-foreground">"No members found"</p>
                                    </div>
                                }.into_any()
                            },
                            Ok(m) => {
                                view! {
                                    <div class="space-y-3">
                                        {m.into_iter().map(|member| {
                                            let uid = current_user_id.clone();
                                            let remove = remove_fn.clone();
                                            let role_action = update_role_action;
                                            view! {
                                                <MemberRow
                                                    member=member
                                                    current_user_id=uid
                                                    on_remove=remove
                                                    update_role_action=role_action
                                                />
                                            }
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            },
                            Err(e) => {
                                let msg = e.to_string();
                                view! {
                                    <p class="text-error-foreground text-sm">{msg}</p>
                                }.into_any()
                            },
                        })
                    }}
                </Suspense>
            </div>

            // Invite Member Modal
            <Modal
                show=Signal::from(show_invite_modal)
                on_close=on_close_modal
                title="Invite Team Member"
                size=ModalSize::Md
                footer=modal_footer.clone()
            >
                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-1">
                            "Email Address"
                        </label>
                        <input
                            type="email"
                            class=INPUT_CLASS
                            placeholder="colleague@example.com"
                            prop:value=invite_email
                            on:input=move |ev| set_invite_email.set(event_target_value(&ev))
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-1">
                            "Role"
                        </label>
                        <crate::components::StyledSelect
                            value=invite_role.get_untracked()
                            options=vec![
                                ("user", "User - Full feature access"),
                                ("admin", "Admin - Can manage workspace settings"),
                            ]
                            on_change=move |val| set_invite_role.set(val)
                        />
                    </div>

                    // Show invite action error
                    {move || {
                        invite_action.value().get().and_then(|r| r.err()).map(|e| {
                            let msg = e.to_string();
                            view! {
                                <crate::components::Alert variant=crate::components::AlertVariant::Error>
                                    <crate::components::AlertDescription>{msg}</crate::components::AlertDescription>
                                </crate::components::Alert>
                            }
                        })
                    }}
                </div>
            </Modal>

            // Confirm Dialog
            <ConfirmDialog
                open=Signal::from(dialog_open)
                title=dialog_title.get_untracked()
                message=dialog_message.get_untracked()
                confirm_text=dialog_confirm_text.get_untracked()
                on_confirm=on_confirm
                on_cancel=on_cancel
            />
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pending action enum for confirm dialog
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
enum PendingAction {
    CancelInvitation(String),
    RemoveMember(String),
    CancelTransfer(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// Invitation Row
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn InvitationRow(
    invitation: TeamInvitation,
    on_cancel: impl Fn(String) + Clone + 'static,
) -> impl IntoView {
    let inv_id = invitation.invitation_id.clone();
    let badge_variant = if invitation.role == "workspace_admin" {
        BadgeVariant::Secondary
    } else {
        BadgeVariant::Default
    };
    let role_display = if invitation.role == "workspace_admin" {
        "admin"
    } else {
        "user"
    };

    // Format dates
    let created = format_date(&invitation.created_at);
    let expires = format_date(&invitation.expires_at);

    view! {
        <div class="border border-border rounded-lg p-3 sm:p-4 bg-background hover:bg-muted/50 transition-colors">
            <div class="flex flex-col sm:flex-row sm:items-center gap-3 sm:gap-4">
                // Invitation info
                <div class="flex-1 min-w-0">
                    <div class="flex flex-wrap items-center gap-2 mb-1">
                        <span class="text-sm font-medium text-foreground truncate">
                            {invitation.email}
                        </span>
                        <Badge variant=badge_variant class="flex-shrink-0">
                            {role_display}
                        </Badge>
                    </div>
                    <div class="text-xs text-muted-foreground">
                        "Invited " {created}
                        <span class="mx-1">" \u{2022} "</span>
                        "Expires " {expires}
                    </div>
                </div>

                // Cancel button
                <div class="flex items-center pt-2 sm:pt-0 border-t sm:border-t-0 border-border flex-shrink-0">
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on:click=move |_| on_cancel(inv_id.clone())
                        attr:title="Cancel invitation"
                    >
                        <span class="inline-flex">
                            <leptos_icons::Icon icon=icondata_lu::LuTrash2 width="16" height="16"/>
                        </span>
                    </Button>
                </div>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Transfer Row
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn TransferRow(
    transfer: OwnershipTransferData,
    on_cancel: impl Fn(String) + Clone + 'static,
) -> impl IntoView {
    let transfer_id = transfer.transfer_id.clone();
    let created = format_date(&transfer.created_at);
    let expires = format_date(&transfer.expires_at);

    if transfer.is_recipient {
        // Recipient view — prominent call to action
        view! {
            <div class="border rounded-lg p-4 border-primary bg-primary/5">
                <div class="space-y-4">
                    <div class="flex items-start gap-3">
                        <span class="inline-flex mt-1 text-primary flex-shrink-0">
                            <leptos_icons::Icon icon=icondata_lu::LuArrowLeftRight width="20" height="20"/>
                        </span>
                        <div class="flex-1">
                            <h4 class="font-semibold text-foreground mb-1">
                                "You've been offered workspace ownership"
                            </h4>
                            <p class="text-sm text-muted-foreground mb-2">
                                {transfer.from_user_email.clone()}
                                " wants to transfer ownership of this workspace to you."
                            </p>
                            <div class="flex items-center gap-4 text-xs text-muted-foreground">
                                <span>"Requested: " {created}</span>
                                <span>"Expires: " {expires}</span>
                            </div>
                        </div>
                    </div>
                    <div class="flex gap-2">
                        <Button variant=ButtonVariant::Default>
                            "Review & Accept"
                        </Button>
                    </div>
                </div>
            </div>
        }
        .into_any()
    } else if transfer.is_initiator {
        // Initiator view — simple row
        view! {
            <div class="border rounded-lg p-4 border-border bg-background">
                <div class="flex items-center justify-between">
                    <div class="flex-1">
                        <div class="text-sm font-medium text-foreground">
                            "Pending transfer to " {transfer.to_user_email.clone()}
                        </div>
                        <div class="flex items-center gap-4 text-xs text-muted-foreground mt-1">
                            <span>"Requested: " {created}</span>
                            <span>"Expires: " {expires}</span>
                        </div>
                    </div>
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on:click=move |_| on_cancel(transfer_id.clone())
                        attr:title="Cancel transfer"
                    >
                        <span class="inline-flex">
                            <leptos_icons::Icon icon=icondata_lu::LuTrash2 width="16" height="16"/>
                        </span>
                    </Button>
                </div>
            </div>
        }
        .into_any()
    } else {
        view! { <span></span> }.into_any()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Member Row
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn MemberRow(
    member: TeamMember,
    current_user_id: String,
    on_remove: impl Fn(String) + Clone + 'static,
    update_role_action: Action<(String, String), Result<(), ServerFnError>>,
) -> impl IntoView {
    let member_id_for_remove = member.user_id.clone();
    let member_id_for_role = member.user_id.clone();
    let is_self = member.user_id == current_user_id;
    let display_name = member
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| member.email.clone());
    let initial = member
        .email
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let joined = format_date(&member.joined_at);

    // Map DB role to display role for the select
    let display_role = if member.role == "workspace_admin" {
        "admin"
    } else {
        "user"
    };

    view! {
        <div class="border border-border rounded-lg p-3 sm:p-4 bg-background hover:bg-muted/50 transition-colors">
            <div class="flex flex-col sm:flex-row sm:items-center gap-3 sm:gap-4">
                // Member info
                <div class="flex items-center gap-3 flex-1 min-w-0">
                    <div class="h-8 w-8 rounded-full bg-primary/10 flex items-center justify-center text-primary font-medium flex-shrink-0">
                        {initial}
                    </div>
                    <div class="min-w-0 flex-1">
                        <div class="flex flex-wrap items-center gap-2">
                            <span class="text-sm font-medium text-foreground truncate">
                                {display_name}
                            </span>
                            {member.is_owner.then(|| view! {
                                <Badge variant=BadgeVariant::Default class="text-xs flex-shrink-0">
                                    "Owner"
                                </Badge>
                            })}
                        </div>
                        <div class="text-xs sm:text-sm text-muted-foreground truncate">
                            {member.email.clone()}
                        </div>
                    </div>
                </div>

                // Controls for non-owners
                {if !member.is_owner {
                    view! {
                        <div class="flex items-center gap-2 sm:gap-3 pt-2 sm:pt-0 border-t sm:border-t-0 border-border flex-shrink-0">
                            // Role select
                            <div class="w-[100px] sm:w-[120px]">
                                <crate::components::StyledSelect
                                    value=display_role.to_string()
                                    options=vec![("user", "User"), ("admin", "Admin")]
                                    on_change=move |val| {
                                        let uid = member_id_for_role.clone();
                                        update_role_action.dispatch((uid, val));
                                    }
                                />
                            </div>

                            // Joined date (desktop only)
                            <span class="hidden sm:inline text-xs text-muted-foreground whitespace-nowrap">
                                "Joined " {joined.clone()}
                            </span>

                            // Remove button (not for self)
                            {if !is_self {
                                let remove = on_remove.clone();
                                let mid = member_id_for_remove.clone();
                                view! {
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::Sm
                                        on:click=move |_| remove(mid.clone())
                                        attr:title="Remove member"
                                    >
                                        <span class="inline-flex">
                                            <leptos_icons::Icon icon=icondata_lu::LuTrash2 width="16" height="16"/>
                                        </span>
                                    </Button>
                                }.into_any()
                            } else {
                                view! { <span></span> }.into_any()
                            }}
                        </div>
                    }.into_any()
                } else {
                    // Owner — just show joined date
                    view! {
                        <span class="hidden sm:inline text-xs text-muted-foreground whitespace-nowrap flex-shrink-0">
                            "Joined " {joined.clone()}
                        </span>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Format an RFC 3339 date string to a short locale-style date.
///
/// Falls back to the raw string if parsing fails.
fn format_date(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|dt| dt.format("%b %d, %Y").to_string())
        .unwrap_or_else(|_| rfc3339.to_string())
}
