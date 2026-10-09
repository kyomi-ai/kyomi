// SPDX-License-Identifier: LicenseRef-Alytic-Enterprise

//! KYO-823: the Slack agent pipeline's billing gate.
//!
//! KYO-805 gated every `AuthUser`-authenticated entry point (server fns,
//! REST, MCP, WebSocket sync, the watch/catalog schedulers) on
//! `kyomi_auth::billing_gate::check_workspace_billing_gate`. Slack's routes
//! never go through `AuthUser` — they authenticate with Slack's own request
//! signature and resolve the workspace from the Slack team id — so that
//! gate never ran for them.
//!
//! Only one Slack entry point actually spends anything: the Events API's
//! `app_mention`/direct-message handlers (`crate::routes::handle_app_mention`
//! / `handle_direct_message`), which generate a session title (an LLM call)
//! and run the agent loop (LLM calls plus warehouse SQL) via
//! `kyomi_agent::execution::execute_agent_chat`. `/command` only handles
//! `connect`/`status`/`disconnect`/help and `/interactions` only
//! acknowledges — neither reaches the agent, so neither needs gating; see
//! the KYO-823 ticket notes for the trace that established this.
//!
//! [`admit_slack_agent_request`] is the single admission point for that one
//! path, and [`SlackBillingAdmission`] is a proof token: its only field is
//! private and this module is the sole constructor, so `crate::routes`
//! cannot fabricate one. `run_slack_query` takes `&SlackBillingAdmission` in
//! place of a bare `workspace_id: &str`, which makes "reach the agent from
//! Slack without first passing the gate" a compile error rather than a
//! convention — the shape
//! `docs/standards/code-organization/close-the-class-by-making-the-wrong-call-uncallable.md`
//! asks for.

use chrono::{DateTime, Utc};

use kyomi_auth::billing_gate::{check_workspace_billing_gate, WorkspaceBillingGate};
use kyomi_core::{Config, DbPool};

/// Proof that `workspace_id` passed the KYO-823 billing gate at the moment
/// this was constructed.
///
/// The field is private and the only way to build one is
/// [`admit_slack_agent_request`] returning [`SlackAgentAdmission::Admitted`]
/// — nothing in `crate::routes` can construct this type directly, so a
/// function that requires `&SlackBillingAdmission` (namely
/// `crate::routes::run_slack_query`) cannot be reached without going through
/// the gate first.
pub(crate) struct SlackBillingAdmission {
    workspace_id: String,
}

impl SlackBillingAdmission {
    /// The workspace this admission was granted for.
    pub(crate) fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
}

/// Outcome of [`admit_slack_agent_request`].
pub(crate) enum SlackAgentAdmission {
    /// Billing is open (self-hosted, or a SaaS workspace in good standing)
    /// — proceed, using the enclosed token to reach the agent.
    Admitted(SlackBillingAdmission),
    /// Billing blocks this request. `message` is the exact Slack mrkdwn
    /// text to post back to the requesting user; `outcome` is always
    /// [`WorkspaceBillingGate::Lapsed`] or
    /// [`WorkspaceBillingGate::Unverifiable`] (never `Open` — that maps to
    /// `Admitted` above) and exists only so the caller can log which one
    /// fired without re-deriving it from the message text.
    Refused {
        message: String,
        outcome: WorkspaceBillingGate,
    },
}

/// Check whether `workspace_id` may spend a Slack agent turn (LLM calls,
/// warehouse queries) right now.
///
/// Delegates entirely to
/// `kyomi_auth::billing_gate::check_workspace_billing_gate` — the one
/// definition of "lapsed" shared with every other KYO-805 enforcement
/// point — and never re-derives it from `subscription_status` or any other
/// field itself.
///
/// Matches [`WorkspaceBillingGate`] exhaustively (no `_` arm: it is a
/// 3-variant enum defined in this workspace, not a foreign
/// `#[non_exhaustive]` type, so the compiler enumerates a new variant for
/// us) so an [`WorkspaceBillingGate::Unverifiable`] outcome — the gate
/// couldn't be checked, not that it *was* checked and found lapsed — can
/// never be reported to the Slack user as a billing lapse. See
/// `docs/standards/error-handling/a-catch-all-arm-must-return-the-safe-answer-not-the-common-one.md`.
pub(crate) async fn admit_slack_agent_request(
    db: &DbPool,
    config: &Config,
    workspace_id: &str,
    now: DateTime<Utc>,
) -> SlackAgentAdmission {
    let outcome = check_workspace_billing_gate(db, workspace_id, config.self_hosted, now).await;
    match outcome {
        WorkspaceBillingGate::Open => SlackAgentAdmission::Admitted(SlackBillingAdmission {
            workspace_id: workspace_id.to_string(),
        }),
        WorkspaceBillingGate::Lapsed => SlackAgentAdmission::Refused {
            message: lapsed_message(config),
            outcome,
        },
        WorkspaceBillingGate::Unverifiable => SlackAgentAdmission::Refused {
            message: unverifiable_message(),
            outcome,
        },
    }
}

/// User-facing text for a *confirmed* billing lapse.
///
/// Uses Slack mrkdwn link syntax (`<url|label>`, not `[label](url)`) — see
/// `crate::helpers` for Slack's markdown dialect. `/settings/billing` is
/// under the `Layout` that renders the KYO-806 full-screen paywall for
/// lapsed workspaces, so the link lands the workspace owner exactly where
/// they can act.
///
/// The label is `Settings &gt; Billing`, not a literal `>`: Slack's mrkdwn
/// link token ends at the first unescaped `>`, so a raw `>` inside the label
/// truncates it there — the label would render as "Settings " with " Billing>"
/// leaking as plain text after the link. Slack requires `&`, `<`, `>` in
/// message text to be escaped as `&amp;`/`&lt;`/`&gt;`; `&gt;` still renders
/// as `>` to the reader, so the visible label is still "Settings > Billing".
/// `crate::helpers` has no existing escape helper to reuse — this label is a
/// fixed string, not dynamic content, so it's hardcoded here rather than
/// adding a generic escaping utility for a single static call site.
fn lapsed_message(config: &Config) -> String {
    let billing_url = format!(
        "{}/settings/billing",
        config.frontend_url.trim_end_matches('/')
    );
    format!(
        "This Kyomi workspace's billing needs attention, so I can't answer questions right now. \
         A workspace owner can sort it out in Kyomi: <{billing_url}|Settings &gt; Billing>"
    )
}

/// User-facing text when the gate itself couldn't be verified (DB error, or
/// no workspace row at all).
///
/// Deliberately generic and free of the words "billing"/"payment" — this
/// workspace was never confirmed to be in a billing lapse, only that the
/// check itself failed, and KYO-805's fail-closed discipline blocks it
/// anyway (see `WorkspaceBillingGate::Unverifiable`'s doc comment). Telling
/// the user "billing" here would be exactly the
/// `a-catch-all-arm-must-return-the-safe-answer-not-the-common-one` failure
/// mode in reverse: a confident, specific-sounding claim the branch never
/// actually established.
fn unverifiable_message() -> String {
    "Sorry, I can't answer right now. Please try again shortly.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{insert_user, insert_workspace, set_workspace_billing, test_pool};

    fn assert_admitted(admission: SlackAgentAdmission, expected_workspace_id: &str) {
        match admission {
            SlackAgentAdmission::Admitted(token) => {
                assert_eq!(token.workspace_id(), expected_workspace_id);
            }
            SlackAgentAdmission::Refused { message, outcome } => {
                panic!("expected Admitted, got Refused {{ outcome: {outcome:?}, message: {message:?} }}");
            }
        }
    }

    fn assert_refused_lapsed(admission: SlackAgentAdmission, config: &Config) {
        match admission {
            SlackAgentAdmission::Refused { message, outcome } => {
                assert_eq!(outcome, WorkspaceBillingGate::Lapsed);
                assert_eq!(message, lapsed_message(config));
                let billing_url = format!(
                    "{}/settings/billing",
                    config.frontend_url.trim_end_matches('/')
                );
                assert_lapsed_link_token_well_formed(&message, &billing_url);
            }
            SlackAgentAdmission::Admitted(token) => {
                panic!("expected Refused(Lapsed), got Admitted({})", token.workspace_id());
            }
        }
    }

    /// Assert `message` ends with a well-formed Slack mrkdwn link token for
    /// `billing_url` — i.e. the message contains exactly
    /// `<{billing_url}|Settings &gt; Billing>`, with no raw `>` inside the
    /// label. A raw `>` there would terminate Slack's `<url|label>` token at
    /// that point instead of at the intended closing `>`, truncating the
    /// visible label and leaking the rest as plain text.
    ///
    /// Anchors on the message's *trailing* `|`/`>` (the link is always the
    /// last thing `lapsed_message` appends), not the first `>` after `|`.
    /// Scanning from the first `>` is exactly what Slack's own parser does,
    /// and is exactly where an unescaped `>` inside the label would stop —
    /// which means a first-match scan can never observe the extra `>`, since
    /// it *is* the first match. Anchoring on the real trailing delimiters
    /// instead lets an unescaped `>` show up inside the extracted label,
    /// where this assertion catches it.
    fn assert_lapsed_link_token_well_formed(message: &str, billing_url: &str) {
        let expected_token = format!("<{billing_url}|Settings &gt; Billing>");
        assert!(
            message.ends_with(&expected_token),
            "expected the message to end with the exact mrkdwn link token {expected_token:?}, got: {message}"
        );

        let close = message
            .rfind('>')
            .expect("message must end with the mrkdwn link token's '>'");
        assert_eq!(
            close,
            message.len() - 1,
            "the link token must be the last thing in the message"
        );
        let open_pipe = message
            .rfind('|')
            .expect("message must contain a mrkdwn link token");
        let label = &message[open_pipe + 1..close];
        assert!(
            !label.contains('>'),
            "link label must contain no raw '>' — Slack would truncate the link there, got label: {label:?}"
        );
    }

    #[tokio::test]
    async fn past_due_is_refused_lapsed() {
        let db = test_pool().await;
        insert_user(&db, "user-1").await;
        insert_workspace(&db, "ws-1", "user-1").await;
        set_workspace_billing(&db, "ws-1", "past_due", None, None, None).await;
        let config = Config::test_config();

        let admission = admit_slack_agent_request(&db, &config, "ws-1", Utc::now()).await;
        assert_refused_lapsed(admission, &config);
    }

    #[tokio::test]
    async fn past_due_lapsed_message_handles_trailing_slash_frontend_url() {
        let db = test_pool().await;
        insert_user(&db, "user-1").await;
        insert_workspace(&db, "ws-1", "user-1").await;
        set_workspace_billing(&db, "ws-1", "past_due", None, None, None).await;
        let mut config = Config::test_config();
        config.frontend_url = "https://app.kyomi.ai/".into();

        let admission = admit_slack_agent_request(&db, &config, "ws-1", Utc::now()).await;
        match admission {
            SlackAgentAdmission::Refused { message, .. } => {
                assert!(
                    message.contains("<https://app.kyomi.ai/settings/billing|Settings &gt; Billing>"),
                    "expected a single slash before settings/billing, got: {message}"
                );
                assert_lapsed_link_token_well_formed(&message, "https://app.kyomi.ai/settings/billing");
            }
            SlackAgentAdmission::Admitted(_) => panic!("expected Refused"),
        }
    }

    #[tokio::test]
    async fn expired_no_stripe_trial_is_refused_lapsed() {
        let db = test_pool().await;
        insert_user(&db, "user-1").await;
        insert_workspace(&db, "ws-1", "user-1").await;
        let now = Utc::now();
        set_workspace_billing(
            &db,
            "ws-1",
            "trialing",
            None,
            None,
            Some(now - chrono::Duration::days(1)),
        )
        .await;
        let config = Config::test_config();

        let admission = admit_slack_agent_request(&db, &config, "ws-1", now).await;
        assert_refused_lapsed(admission, &config);
    }

    #[tokio::test]
    async fn cancelled_with_no_grace_period_is_refused_lapsed() {
        let db = test_pool().await;
        insert_user(&db, "user-1").await;
        insert_workspace(&db, "ws-1", "user-1").await;
        set_workspace_billing(&db, "ws-1", "cancelled", None, None, None).await;
        let config = Config::test_config();

        let admission = admit_slack_agent_request(&db, &config, "ws-1", Utc::now()).await;
        assert_refused_lapsed(admission, &config);
    }

    #[tokio::test]
    async fn missing_workspace_is_refused_unverifiable_generic_text() {
        let db = test_pool().await;
        let config = Config::test_config();

        let admission =
            admit_slack_agent_request(&db, &config, "does-not-exist", Utc::now()).await;
        match admission {
            SlackAgentAdmission::Refused { message, outcome } => {
                assert_eq!(outcome, WorkspaceBillingGate::Unverifiable);
                assert_eq!(message, unverifiable_message());
                assert!(
                    !message.to_lowercase().contains("billing"),
                    "unverifiable message must never say 'billing', got: {message}"
                );
                assert!(
                    !message.to_lowercase().contains("payment"),
                    "unverifiable message must never say 'payment', got: {message}"
                );
            }
            SlackAgentAdmission::Admitted(_) => panic!("expected Refused"),
        }
    }

    #[tokio::test]
    async fn active_is_admitted() {
        let db = test_pool().await;
        insert_user(&db, "user-1").await;
        insert_workspace(&db, "ws-1", "user-1").await;
        // insert_workspace leaves subscription_status at its migration
        // default ('active') — no explicit set_workspace_billing call.
        let config = Config::test_config();

        let admission = admit_slack_agent_request(&db, &config, "ws-1", Utc::now()).await;
        assert_admitted(admission, "ws-1");
    }

    #[tokio::test]
    async fn trialing_with_future_trial_end_is_admitted() {
        let db = test_pool().await;
        insert_user(&db, "user-1").await;
        insert_workspace(&db, "ws-1", "user-1").await;
        let now = Utc::now();
        set_workspace_billing(
            &db,
            "ws-1",
            "trialing",
            None,
            None,
            Some(now + chrono::Duration::days(1)),
        )
        .await;
        let config = Config::test_config();

        let admission = admit_slack_agent_request(&db, &config, "ws-1", now).await;
        assert_admitted(admission, "ws-1");
    }

    #[tokio::test]
    async fn cancelled_scheduled_within_grace_period_is_admitted() {
        let db = test_pool().await;
        insert_user(&db, "user-1").await;
        insert_workspace(&db, "ws-1", "user-1").await;
        let now = Utc::now();
        set_workspace_billing(
            &db,
            "ws-1",
            "cancelled",
            Some("sub_live_123"),
            Some(now + chrono::Duration::days(5)),
            None,
        )
        .await;
        let config = Config::test_config();

        let admission = admit_slack_agent_request(&db, &config, "ws-1", now).await;
        assert_admitted(admission, "ws-1");
    }

    #[tokio::test]
    async fn self_hosted_past_due_is_admitted() {
        let db = test_pool().await;
        insert_user(&db, "user-1").await;
        insert_workspace(&db, "ws-1", "user-1").await;
        set_workspace_billing(&db, "ws-1", "past_due", None, None, None).await;
        let mut config = Config::test_config();
        config.self_hosted = true;

        let admission = admit_slack_agent_request(&db, &config, "ws-1", Utc::now()).await;
        assert_admitted(admission, "ws-1");
    }

    // -----------------------------------------------------------------
    // Mutation check (reported, not left behind): temporarily replacing
    // the body of `admit_slack_agent_request` with
    // `SlackAgentAdmission::Admitted(SlackBillingAdmission { workspace_id:
    // workspace_id.to_string() })` unconditionally made
    // `past_due_is_refused_lapsed`, `expired_no_stripe_trial_is_refused_lapsed`,
    // `cancelled_with_no_grace_period_is_refused_lapsed`, and
    // `missing_workspace_is_refused_unverifiable_generic_text` fail with
    // "expected Refused, got Admitted(...)" — confirming these tests are
    // load-bearing. Reverted before committing.
    // -----------------------------------------------------------------
}
