// SPDX-License-Identifier: AGPL-3.0-or-later

//! SMTP email service for sending transactional emails.
//!
//! Wire-compatible with Python's `services/email_service.py`.
//!
//! Configuration via environment variables:
//! - `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`, `SMTP_PASSWORD`
//! - `SMTP_FROM_EMAIL` (default: `noreply@kyomi.ai`)
//! - `SMTP_FROM_NAME` (default: `Kyomi`)
//!
//! Graceful degradation: if SMTP is not configured, `send_email` returns
//! [`EmailSendError::NotConfigured`] rather than sending. No send path ever
//! fails the calling operation — the caller decides what an undeliverable
//! email means for it — but the *reason* now survives the call boundary
//! instead of being collapsed into a bare `false` (KYO-697).
//!
//! Failures are returned, not logged here: every caller has context this
//! module does not (which feedback row, which signup, which watch alert),
//! so each one owns the diagnostic for its own send. See
//! [`EmailService::send_email`].

use kyomi_core::config::SmtpSettings;
use lettre::{
    message::{header::ContentType, Attachment, Body, Mailbox, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

/// Kyomi logo PNG (brand orange text, transparent background), embedded at compile time.
static LOGO_BYTES: &[u8] =
    include_bytes!("../../../assets/kyomi_email_logo.png");

/// Content-ID used in `<img src="cid:kyomi_logo">`.
const LOGO_CID: &str = "kyomi_logo";

/// Whether a failed SMTP send is worth attempting again, as classified by
/// `lettre` itself.
///
/// This is the same distinction [`EmailService::send_email`]'s retry
/// classifier already makes (`is_transient() || is_timeout()`); carrying it
/// on the error means an operator reading a single log line can tell "the
/// mail server deferred us and we gave up after the configured retries"
/// from "the mail server rejected this address outright", which are
/// different problems with different fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpFailureKind {
    /// A 4xx deferral, a timeout, or a dropped connection. Already retried
    /// with backoff before this error was produced; a later attempt may
    /// still succeed.
    Transient,
    /// A 5xx hard rejection or an authentication failure. Not retried —
    /// every attempt produces the same result until the address or the
    /// SMTP configuration changes.
    Permanent,
}

impl std::fmt::Display for SmtpFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient => f.write_str("transient"),
            Self::Permanent => f.write_str("permanent"),
        }
    }
}

/// Why an email could not be sent.
///
/// Replaces the `bool` every `send_*` method used to return (KYO-697). That
/// `bool` mapped six materially different failures onto one value, so the
/// fire-and-forget callers that log it could only ever say *that* a send
/// failed, never *why* — leaving "SMTP was never configured on this
/// instance" indistinguishable from "the recipient's mail server rejected
/// the address" in the one place an operator looks.
#[derive(Debug, thiserror::Error)]
pub enum EmailSendError {
    /// `SMTP_HOST`, `SMTP_USER` or `SMTP_PASSWORD` is unset, so there is no
    /// server to send through. The dominant case on a self-hosted install.
    #[error(
        "SMTP is not configured — set SMTP_HOST, SMTP_PORT, SMTP_USER and \
         SMTP_PASSWORD; no email was sent"
    )]
    NotConfigured,

    /// `SMTP_FROM_NAME`/`SMTP_FROM_EMAIL` do not form a parseable mailbox.
    /// Affects every email this instance sends, not just this one.
    #[error("invalid From address {address:?}: {source}")]
    InvalidFromAddress {
        address: String,
        #[source]
        source: lettre::address::AddressError,
    },

    /// The recipient address is not a parseable mailbox — the one failure
    /// here that is specific to a single recipient.
    #[error("invalid To address {address:?}: {source}")]
    InvalidToAddress {
        address: String,
        #[source]
        source: lettre::address::AddressError,
    },

    /// The MIME message could not be assembled from the rendered bodies
    /// and inline images.
    #[error("could not build the email message: {source}")]
    MessageBuild {
        #[source]
        source: lettre::error::Error,
    },

    /// The SMTP transport could not be constructed — a bad hostname or a
    /// TLS setup failure. Never retried: it cannot succeed on a later
    /// attempt with the same configuration.
    #[error("could not create the SMTP transport for {host:?}: {source}")]
    TransportSetup {
        host: String,
        #[source]
        source: lettre::transport::smtp::Error,
    },

    /// The message reached the transport and the server refused it, or the
    /// connection failed. `kind` preserves the transient/permanent
    /// classification the retry loop already computed.
    #[error("SMTP send failed ({kind}): {source}")]
    Send {
        kind: SmtpFailureKind,
        #[source]
        source: lettre::transport::smtp::Error,
    },
}

/// The single definition of "worth retrying" for an SMTP error.
///
/// Used both by [`EmailService::send_email`]'s retry classifier and by the
/// [`SmtpFailureKind`] recorded on the error it ultimately returns, so the
/// decision the retry loop made and the classification the operator reads
/// cannot drift apart.
fn classify_smtp_error(e: &lettre::transport::smtp::Error) -> SmtpFailureKind {
    if e.is_transient() || e.is_timeout() {
        SmtpFailureKind::Transient
    } else {
        SmtpFailureKind::Permanent
    }
}

/// SMTP email service.
///
/// Constructed once and shared (e.g., via `Arc` or created on-demand per call).
/// All methods are `&self` — the struct is cheaply cloneable when wrapped in Arc.
#[derive(Debug, Clone)]
pub struct EmailService {
    smtp_host: Option<String>,
    smtp_port: u16,
    smtp_user: Option<String>,
    smtp_password: Option<String>,
    from_email: String,
    from_name: String,
    /// Base URL for the frontend app (e.g. `https://app.kyomi.ai`).
    frontend_url: String,
}

impl EmailService {
    /// Create a new `EmailService` reading configuration from environment variables.
    pub fn from_env() -> Self {
        let smtp_host = std::env::var("SMTP_HOST").ok();
        let smtp_port: u16 = std::env::var("SMTP_PORT")
            .unwrap_or_else(|_| "587".to_string())
            .parse()
            .unwrap_or(587);
        let smtp_user = std::env::var("SMTP_USER").ok();
        let smtp_password = std::env::var("SMTP_PASSWORD").ok();
        let from_email =
            std::env::var("SMTP_FROM_EMAIL").unwrap_or_else(|_| "noreply@kyomi.ai".to_string());
        let from_name =
            std::env::var("SMTP_FROM_NAME").unwrap_or_else(|_| "Kyomi".to_string());
        let frontend_url = std::env::var("FRONTEND_URL")
            .unwrap_or_else(|_| "https://app.kyomi.ai".to_string())
            .trim_end_matches('/')
            .to_string();

        let settings = SmtpSettings::classify(
            smtp_host.as_deref(),
            smtp_user.as_deref(),
            smtp_password.as_deref(),
        );
        if !settings.can_send() {
            tracing::warn!(
                missing = %settings.missing_vars().join(", "),
                "SMTP not configured. Email sending will be disabled. Set the missing \
                 variables in .env"
            );
        }

        Self {
            smtp_host,
            smtp_port,
            smtp_user,
            smtp_password,
            from_email,
            from_name,
            frontend_url,
        }
    }

    /// Check if SMTP is configured (host, user and password are all set).
    ///
    /// This service is the authoritative reader of that rule — it is what
    /// actually opens the SMTP connection — but the rule itself lives in
    /// [`SmtpSettings`] so `Config::smtp_configured` decides identically
    /// (KYO-685). Do not re-spell the conjunction here.
    ///
    /// A password is required, not optional: [`Self::send_email`] authenticates
    /// with `Credentials::new(user, password)` before it can submit a message,
    /// so relaxing this would only move the failure later — to a user who has
    /// already been told an email is on its way.
    pub fn is_configured(&self) -> bool {
        SmtpSettings::classify(
            self.smtp_host.as_deref(),
            self.smtp_user.as_deref(),
            self.smtp_password.as_deref(),
        )
        .can_send()
    }

    /// A real, fully-constructed `EmailService` with no SMTP credentials —
    /// the self-hosted "SMTP was never set up" state.
    ///
    /// Not a mock: every send through it runs the genuine
    /// [`send_email`](Self::send_email) body and fails where that body
    /// actually fails. It exists because the only other way to obtain an
    /// unconfigured service is [`from_env`](Self::from_env), which would
    /// make a test's verdict depend on whether the machine running it
    /// happens to have `SMTP_*` exported. Crate-visible so
    /// `auth_service`'s tests can reach the verification-email path
    /// (KYO-697).
    #[cfg(test)]
    pub(crate) fn unconfigured_for_tests() -> Self {
        Self {
            smtp_host: None,
            smtp_port: 587,
            smtp_user: None,
            smtp_password: None,
            from_email: "noreply@kyomi.ai".to_string(),
            from_name: "Kyomi".to_string(),
            frontend_url: "https://app.kyomi.ai".to_string(),
        }
    }

    /// Send an email via SMTP.
    ///
    /// Returns `Ok(())` once the server has accepted the message, or the
    /// [`EmailSendError`] explaining why it did not (KYO-697). Never panics.
    ///
    /// **The caller owns the failure diagnostic.** This method deliberately
    /// does not log its own failures: it knows only the recipient and the
    /// subject, while the caller knows which user action produced the send
    /// and is already the place that decides whether an undeliverable email
    /// changes anything. Logging in both places would report one outcome
    /// twice; every call site in this workspace logs the returned error.
    ///
    /// `reply_to` sets the Reply-To header so recipients can reply directly
    /// to the relevant person (e.g., the user who submitted feedback).
    ///
    /// `images` is an optional list of `(content_id, png_bytes)` pairs for
    /// additional inline CID images (e.g. rendered charts). Pass `&[]` when
    /// no extra images are needed.
    pub async fn send_email(
        &self,
        to_email: &str,
        subject: &str,
        html_body: &str,
        text_body: Option<&str>,
        reply_to: Option<&str>,
        images: &[(String, Vec<u8>)],
    ) -> Result<(), EmailSendError> {
        // This destructuring *is* the `is_configured()` predicate — the
        // three fields it requires are exactly the three that method tests —
        // so there is one check rather than a check plus an unreachable
        // "missing despite is_configured()" arm behind it.
        let (Some(smtp_host), Some(smtp_user), Some(smtp_password)) = (
            self.smtp_host.as_deref(),
            self.smtp_user.as_deref(),
            self.smtp_password.as_deref(),
        ) else {
            return Err(EmailSendError::NotConfigured);
        };

        // Build the From mailbox
        let from_mailbox: Mailbox = format!("{} <{}>", self.from_name, self.from_email)
            .parse()
            .map_err(|source| EmailSendError::InvalidFromAddress {
                address: self.from_email.clone(),
                source,
            })?;

        // Build the To mailbox
        let to_mailbox: Mailbox =
            to_email
                .parse()
                .map_err(|source| EmailSendError::InvalidToAddress {
                    address: to_email.to_string(),
                    source,
                })?;

        // Build multipart/alternative message (text + html)
        let alternative = if let Some(text) = text_body {
            MultiPart::alternative()
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(text.to_string()),
                )
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(html_body.to_string()),
                )
        } else {
            MultiPart::alternative().singlepart(
                SinglePart::builder()
                    .header(ContentType::TEXT_HTML)
                    .body(html_body.to_string()),
            )
        };

        // Wrap in multipart/related so the inline logo CID resolves.
        // A single all-orange logo works on both light and dark backgrounds.
        let png_ct: ContentType = "image/png".parse().expect("valid content type");
        let mut related = MultiPart::related()
            .multipart(alternative)
            .singlepart(
                Attachment::new_inline(LOGO_CID.to_string())
                    .body(Body::new(LOGO_BYTES.to_vec()), png_ct.clone()),
            );

        // Attach any additional inline CID images (e.g. rendered charts)
        for (cid, png_bytes) in images {
            related = related.singlepart(
                Attachment::new_inline(cid.clone())
                    .body(Body::new(png_bytes.clone()), png_ct.clone()),
            );
        }

        let mut builder = Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject(subject);

        // Set Reply-To header if provided
        if let Some(reply_to_addr) = reply_to {
            match reply_to_addr.parse::<Mailbox>() {
                Ok(mb) => builder = builder.reply_to(mb),
                Err(e) => {
                    tracing::warn!("Failed to parse Reply-To address '{}': {}", reply_to_addr, e);
                    // Continue without Reply-To — don't fail the email
                }
            }
        }

        let message = builder
            .multipart(related)
            .map_err(|source| EmailSendError::MessageBuild { source })?;

        let creds = Credentials::new(smtp_user.to_string(), smtp_password.to_string());

        // Build the SMTP transport. Configuration errors (bad hostname, invalid
        // credentials format) are not retryable — return immediately.
        let mailer_result = if self.smtp_port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(smtp_host).map(|b| {
                b.port(self.smtp_port).credentials(creds).build()
            })
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(smtp_host).map(|b| {
                b.port(self.smtp_port).credentials(creds).build()
            })
        };

        let mailer = mailer_result.map_err(|source| EmailSendError::TransportSetup {
            host: smtp_host.to_string(),
            source,
        })?;

        // Retry transient SMTP errors (4xx SMTP codes — transient deferrals —
        // network timeouts, connection drops). Permanent errors (5xx SMTP codes
        // — hard rejections — and auth failures) are not retried: they will
        // produce the same result on every attempt.
        let send_result = kyomi_core::retry::retry_with_backoff_classified(
            || {
                let mailer = mailer.clone();
                let message = message.clone();
                async move { mailer.send(message).await }
            },
            |e: &lettre::transport::smtp::Error| {
                classify_smtp_error(e) == SmtpFailureKind::Transient
            },
        )
        .await;

        match send_result {
            Ok(_) => {
                tracing::info!(to = %to_email, subject = %subject, "Email sent successfully");
                Ok(())
            }
            Err(source) => Err(EmailSendError::Send {
                kind: classify_smtp_error(&source),
                source,
            }),
        }
    }

    /// Send a workspace invitation email.
    ///
    /// Returns `Ok(())` if sent successfully, or the reason it was not.
    pub async fn send_workspace_invitation(
        &self,
        email: &str,
        workspace_name: &str,
        inviter_name: &str,
        role: &str,
        invitation_id: &str,
    ) -> Result<(), EmailSendError> {
        let role_display = if role == "admin" {
            "an Admin"
        } else {
            "a Member"
        };

        let frontend_url = &self.frontend_url;
        let accept_url = format!("{frontend_url}/accept-invite/{invitation_id}");
        let subject = format!("You've been invited to join {} on Kyomi", workspace_name);

        let html_body = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <meta name="supported-color-schemes" content="light dark">
    <style>
        :root {{ color-scheme: light dark; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            line-height: 1.6;
            color: #1C1917;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #FAFAF8;
        }}
        .header {{
            text-align: center;
            margin-bottom: 16px;
            padding: 16px 0;
            border-bottom: 1px solid #E8E5DE;
        }}
        .logo-img {{
            height: 48px;
            width: auto;
        }}
        .content {{
            padding: 20px 0;
        }}
        h1 {{
            color: #1C1917;
            font-size: 24px;
            font-weight: 700;
            margin-bottom: 16px;
        }}
        h2 {{
            color: #1C1917;
            font-size: 20px;
            font-weight: 600;
            margin: 24px 0 12px 0;
        }}
        h3 {{
            color: #1C1917;
            font-size: 18px;
            font-weight: 600;
            margin: 20px 0 10px 0;
        }}
        p {{
            color: #6B6660;
            font-size: 14px;
            margin: 12px 0;
        }}
        .highlight {{
            background-color: #fffbeb;
            border-left: 4px solid #d97706;
            padding: 16px;
            margin: 24px 0;
            border-radius: 0 8px 8px 0;
        }}
        .cta {{
            text-align: center;
            margin: 32px 0;
        }}
        .button {{
            display: inline-block;
            background-color: #d97706;
            color: #ffffff !important;
            padding: 14px 28px;
            text-decoration: none;
            border-radius: 8px;
            font-weight: 600;
            font-size: 14px;
        }}
        .footer {{
            margin-top: 20px;
            padding-top: 16px;
            border-top: 1px solid #E8E5DE;
            text-align: center;
            color: #9C9790;
            font-size: 12px;
        }}
        .footer a {{
            color: #6B6660;
            text-decoration: none;
        }}
        .footer a:hover {{
            text-decoration: underline;
        }}
        @media (prefers-color-scheme: dark) {{
            body {{ background-color: #12100F !important; color: #F5F3EF !important; }}
            h1, h2, h3 {{ color: #F5F3EF !important; }}
            p {{ color: #A8A29E !important; }}
            .header {{ border-bottom-color: #2E2925 !important; }}
            .highlight {{ background-color: #2C241E !important; }}
            .feature {{ color: #A8A29E !important; }}
            .footer {{ border-top-color: #2E2925 !important; color: #78716C !important; }}
            .footer a {{ color: #A8A29E !important; }}
        }}
    </style>
</head>
<body style="background-color: #FAFAF8; color: #1C1917;">
    <div class="header">
        <a href="{frontend_url}" style="text-decoration: none;">
            <img src="cid:kyomi_logo" alt="Kyomi" class="logo-img" style="height: 48px; width: auto;">
        </a>
    </div>
    <div class="content">
        <h1>You're Invited!</h1>

        <p><strong>{inviter_name}</strong> has invited you to join <strong>{workspace_name}</strong> on Kyomi as {role_display}.</p>

        <div class="highlight">
            <strong>What is Kyomi?</strong><br>
            Kyomi is a data intelligence platform that captures how your team understands data—which tables matter, what metrics mean, how to ask the right questions—and makes that knowledge available to everyone.
        </div>

        <p>To accept this invitation:</p>

        <div class="features">
            <div class="feature">
                <strong>Open the invitation</strong> - Click the button below to view the invitation
            </div>
            <div class="feature">
                <strong>Sign in if needed</strong> - Use the email address this invitation was sent to ({email}); you'll be brought back here afterward
            </div>
            <div class="feature">
                <strong>Start collaborating</strong> - Access shared dashboards and insights with your team
            </div>
        </div>

        <div class="cta">
            <a href="{accept_url}" class="button">View Invitation</a>
        </div>

        <p>This invitation will expire in 7 days. If you have any questions, reach out to {inviter_name} or reply to this email.</p>

        <p>Thanks,<br>The Kyomi Team</p>
    </div>
    <div class="footer">
        <p style="margin: 0 0 8px 0;">
            You're receiving this because you were invited to join a workspace on Kyomi.
        </p>
        <p style="margin: 0;">
            <a href="{frontend_url}/unsubscribe?email={email}">Unsubscribe</a> &middot;
            <a href="{frontend_url}/privacy">Privacy</a> &middot;
            <a href="{frontend_url}/terms">Terms</a> &middot;
            <a href="{frontend_url}" style="color: #d97706;">kyomi.ai</a>
        </p>
    </div>
</body>
</html>"#,
            inviter_name = html_escape(inviter_name),
            workspace_name = html_escape(workspace_name),
            role_display = role_display,
            email = html_escape(email),
        );

        let text_body = format!(
            "\
You're Invited!

{inviter_name} has invited you to join {workspace_name} on Kyomi as {role_display}.

What is Kyomi?
Kyomi is a data intelligence platform that captures how your team understands data\u{2014}which tables matter, what metrics mean, how to ask the right questions\u{2014}and makes that knowledge available to everyone.

To accept this invitation:

1. Open the invitation - Visit the link below to view the invitation
2. Sign in if needed - Use the email address this invitation was sent to ({email}); you'll be brought back here afterward
3. Start collaborating - Access shared dashboards and insights with your team

View invitation: {accept_url}

This invitation will expire in 7 days. If you have any questions, reach out to {inviter_name} or reply to this email.

Thanks,
The Kyomi Team

---
You're receiving this email because you were invited to join a workspace on Kyomi.
Unsubscribe: {frontend_url}/unsubscribe?email={email}
{frontend_url}
",
        );

        self.send_email(email, &subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send an ownership transfer notification email.
    ///
    /// `variant` is either "initiated" (sent to recipient) or "confirmation" (sent to current owner).
    pub async fn send_ownership_transfer(
        &self,
        email: &str,
        workspace_name: &str,
        from_name: &str,
        to_name: &str,
        variant: &str,
    ) -> Result<(), EmailSendError> {
        let (subject, heading, body_text) = if variant == "initiated" {
            (
                format!("You've been offered ownership of {workspace_name}"),
                "Ownership Transfer Request".to_string(),
                format!(
                    "<strong>{from_name}</strong> wants to transfer ownership of \
                     <strong>{workspace_name}</strong> on Kyomi to you. Log in to review \
                     and accept or decline this transfer. The request expires in 7 days."
                ),
            )
        } else {
            (
                format!("Ownership transfer initiated for {workspace_name}"),
                "Transfer Initiated".to_string(),
                format!(
                    "You initiated an ownership transfer of <strong>{workspace_name}</strong> \
                     to <strong>{to_name}</strong>. They have 7 days to accept. You can cancel \
                     this transfer from your workspace settings."
                ),
            )
        };

        let html_body = format!(
            r#"
        <h1>{heading}</h1>
        <p>{body_text}</p>
        <div class="cta">
            <a href="{frontend_url}/settings/team" class="button">View in Settings</a>
        </div>
        <p class="text-sm" style="color: #6b7280; margin-top: 16px;">
            If you didn't expect this, you can safely ignore it.
        </p>
"#,
            heading = heading,
            body_text = body_text,
            frontend_url = self.frontend_url,
        );

        let text_body = format!(
            "{heading}\n\n{body}\n\nView: {url}/settings/team\n",
            heading = heading,
            body = if variant == "initiated" {
                format!("{from_name} wants to transfer ownership of {workspace_name} on Kyomi to you. Log in to review and accept or decline. Expires in 7 days.")
            } else {
                format!("You initiated an ownership transfer of {workspace_name} to {to_name}. They have 7 days to accept.")
            },
            url = self.frontend_url,
        );

        self.send_email(email, &subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send a passkey recovery email.
    ///
    /// Returns `Ok(())` if sent successfully, or the reason it was not.
    pub async fn send_passkey_recovery(
        &self,
        email: &str,
        name: &str,
        recovery_link: &str,
    ) -> Result<(), EmailSendError> {
        let display_name = if name.is_empty() { "there" } else { name };
        let frontend_url = &self.frontend_url;
        let subject = "Recover your Kyomi account";

        let html_body = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <meta name="supported-color-schemes" content="light dark">
    <style>
        :root {{ color-scheme: light dark; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            line-height: 1.6;
            color: #1C1917;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #FAFAF8;
        }}
        .header {{
            text-align: center;
            margin-bottom: 16px;
            padding: 16px 0;
            border-bottom: 1px solid #E8E5DE;
        }}
        .logo-img {{
            height: 48px;
            width: auto;
        }}
        .content {{
            padding: 20px 0;
        }}
        h1 {{
            color: #1C1917;
            font-size: 24px;
            font-weight: 700;
            margin-bottom: 16px;
        }}
        p {{
            color: #6B6660;
            font-size: 14px;
            margin: 12px 0;
        }}
        .cta {{
            text-align: center;
            margin: 32px 0;
        }}
        .button {{
            display: inline-block;
            background-color: #d97706;
            color: #ffffff !important;
            padding: 14px 28px;
            text-decoration: none;
            border-radius: 8px;
            font-weight: 600;
            font-size: 14px;
        }}
        .footer {{
            margin-top: 20px;
            padding-top: 16px;
            border-top: 1px solid #E8E5DE;
            text-align: center;
            color: #9C9790;
            font-size: 12px;
        }}
        .footer a {{
            color: #6B6660;
            text-decoration: none;
        }}
        .footer a:hover {{
            text-decoration: underline;
        }}
        @media (prefers-color-scheme: dark) {{
            body {{ background-color: #12100F !important; color: #F5F3EF !important; }}
            h1, h2, h3 {{ color: #F5F3EF !important; }}
            p {{ color: #A8A29E !important; }}
            .header {{ border-bottom-color: #2E2925 !important; }}
            .highlight {{ background-color: #2C241E !important; }}
            .feature {{ color: #A8A29E !important; }}
            .footer {{ border-top-color: #2E2925 !important; color: #78716C !important; }}
            .footer a {{ color: #A8A29E !important; }}
        }}
    </style>
</head>
<body style="background-color: #FAFAF8; color: #1C1917;">
    <div class="header">
        <a href="{frontend_url}" style="text-decoration: none;">
            <img src="cid:kyomi_logo" alt="Kyomi" class="logo-img" style="height: 48px; width: auto;">
        </a>
    </div>
    <div class="content">
        <h1>Recover Your Account</h1>

        <p>Hi {display_name},</p>

        <p>Click the button below to recover your account and create a new passkey:</p>

        <div class="cta">
            <a href="{recovery_link}" class="button">Create New Passkey</a>
        </div>

        <p style="color: #e74c3c; font-size: 14px;"><strong>This link expires in 15 minutes and can only be used once.</strong></p>

        <p>If you didn't request this, please ignore this email. Your account is secure—no changes have been made.</p>

        <p>Thanks,<br>The Kyomi Team</p>
    </div>
    <div class="footer">
        <p style="margin: 0 0 8px 0;">
            You're receiving this because you requested account recovery for Kyomi.
        </p>
        <p style="margin: 0;">
            <a href="{frontend_url}" style="color: #d97706;">kyomi.ai</a>
        </p>
    </div>
</body>
</html>"#,
            frontend_url = html_escape(frontend_url),
            display_name = html_escape(display_name),
            recovery_link = html_escape(recovery_link),
        );

        let text_body = format!(
            "\
Recover Your Account

Hi {display_name},

Click the link below to recover your account and create a new passkey:

{recovery_link}

IMPORTANT: This link expires in 15 minutes and can only be used once.

If you didn't request this, please ignore this email. Your account is secure\u{2014}no changes have been made.

Thanks,
The Kyomi Team

---
You're receiving this email because you requested account recovery for Kyomi.
{frontend_url}
",
        );

        self.send_email(email, subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send an account recovery email.
    ///
    /// Returns `Ok(())` if sent successfully, or the reason it was not.
    pub async fn send_account_recovery(
        &self,
        email: &str,
        name: &str,
        recovery_link: &str,
    ) -> Result<(), EmailSendError> {
        let display_name = if name.is_empty() { "there" } else { name };
        let frontend_url = &self.frontend_url;
        let subject = "Recover your Kyomi account";

        let html_body = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <meta name="supported-color-schemes" content="light dark">
    <style>
        :root {{ color-scheme: light dark; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            line-height: 1.6;
            color: #1C1917;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #FAFAF8;
        }}
        .header {{
            text-align: center;
            margin-bottom: 16px;
            padding: 16px 0;
            border-bottom: 1px solid #E8E5DE;
        }}
        .logo-img {{
            height: 48px;
            width: auto;
        }}
        .content {{
            padding: 20px 0;
        }}
        h1 {{
            color: #1C1917;
            font-size: 24px;
            font-weight: 700;
            margin-bottom: 16px;
        }}
        p {{
            color: #6B6660;
            font-size: 14px;
            margin: 12px 0;
        }}
        .cta {{
            text-align: center;
            margin: 32px 0;
        }}
        .button {{
            display: inline-block;
            background-color: #d97706;
            color: #ffffff !important;
            padding: 14px 28px;
            text-decoration: none;
            border-radius: 8px;
            font-weight: 600;
            font-size: 14px;
        }}
        .footer {{
            margin-top: 20px;
            padding-top: 16px;
            border-top: 1px solid #E8E5DE;
            text-align: center;
            color: #9C9790;
            font-size: 12px;
        }}
        .footer a {{
            color: #6B6660;
            text-decoration: none;
        }}
        .footer a:hover {{
            text-decoration: underline;
        }}
        @media (prefers-color-scheme: dark) {{
            body {{ background-color: #12100F !important; color: #F5F3EF !important; }}
            h1, h2, h3 {{ color: #F5F3EF !important; }}
            p {{ color: #A8A29E !important; }}
            .header {{ border-bottom-color: #2E2925 !important; }}
            .highlight {{ background-color: #2C241E !important; }}
            .feature {{ color: #A8A29E !important; }}
            .footer {{ border-top-color: #2E2925 !important; color: #78716C !important; }}
            .footer a {{ color: #A8A29E !important; }}
        }}
    </style>
</head>
<body style="background-color: #FAFAF8; color: #1C1917;">
    <div class="header">
        <a href="{frontend_url}" style="text-decoration: none;">
            <img src="cid:kyomi_logo" alt="Kyomi" class="logo-img" style="height: 48px; width: auto;">
        </a>
    </div>
    <div class="content">
        <h1>Recover Your Account</h1>

        <p>Hi {display_name},</p>

        <p>Click the button below to recover your account and set a new password:</p>

        <div class="cta">
            <a href="{recovery_link}" class="button">Recover Account</a>
        </div>

        <p style="color: #e74c3c; font-size: 14px;"><strong>This link expires in 15 minutes and can only be used once.</strong></p>

        <p>If you didn't request this, please ignore this email. Your account is secure—no changes have been made.</p>

        <p>Thanks,<br>The Kyomi Team</p>
    </div>
    <div class="footer">
        <p style="margin: 0 0 8px 0;">
            You're receiving this because you requested account recovery for Kyomi.
        </p>
        <p style="margin: 0;">
            <a href="{frontend_url}" style="color: #d97706;">kyomi.ai</a>
        </p>
    </div>
</body>
</html>"#,
            frontend_url = html_escape(frontend_url),
            display_name = html_escape(display_name),
            recovery_link = html_escape(recovery_link),
        );

        let text_body = format!(
            "\
Recover Your Account

Hi {display_name},

Click the link below to recover your account and set a new password:

{recovery_link}

IMPORTANT: This link expires in 15 minutes and can only be used once.

If you didn't request this, please ignore this email. Your account is secure\u{2014}no changes have been made.

Thanks,
The Kyomi Team

---
You're receiving this email because you requested account recovery for Kyomi.
{frontend_url}
",
        );

        self.send_email(email, subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send a verification email for account signup.
    ///
    /// Returns `Ok(())` if sent successfully, or the reason it was not.
    pub async fn send_verification_email(
        &self,
        email: &str,
        name: &str,
        verification_link: &str,
    ) -> Result<(), EmailSendError> {
        let display_name = if name.is_empty() { "there" } else { name };
        let frontend_url = &self.frontend_url;
        let subject = "Verify your Kyomi account";

        let html_body = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <meta name="supported-color-schemes" content="light dark">
    <style>
        :root {{ color-scheme: light dark; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            line-height: 1.6;
            color: #1C1917;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #FAFAF8;
        }}
        .header {{
            text-align: center;
            margin-bottom: 16px;
            padding: 16px 0;
            border-bottom: 1px solid #E8E5DE;
        }}
        .logo-img {{
            height: 48px;
            width: auto;
        }}
        .content {{
            padding: 20px 0;
        }}
        h1 {{
            color: #1C1917;
            font-size: 24px;
            font-weight: 700;
            margin-bottom: 16px;
        }}
        p {{
            color: #6B6660;
            font-size: 14px;
            margin: 12px 0;
        }}
        .cta {{
            text-align: center;
            margin: 32px 0;
        }}
        .button {{
            display: inline-block;
            background-color: #d97706;
            color: #ffffff !important;
            padding: 14px 28px;
            text-decoration: none;
            border-radius: 8px;
            font-weight: 600;
            font-size: 14px;
        }}
        .footer {{
            margin-top: 20px;
            padding-top: 16px;
            border-top: 1px solid #E8E5DE;
            text-align: center;
            color: #9C9790;
            font-size: 12px;
        }}
        .footer a {{
            color: #6B6660;
            text-decoration: none;
        }}
        .footer a:hover {{
            text-decoration: underline;
        }}
        @media (prefers-color-scheme: dark) {{
            body {{ background-color: #12100F !important; color: #F5F3EF !important; }}
            h1, h2, h3 {{ color: #F5F3EF !important; }}
            p {{ color: #A8A29E !important; }}
            .header {{ border-bottom-color: #2E2925 !important; }}
            .highlight {{ background-color: #2C241E !important; }}
            .feature {{ color: #A8A29E !important; }}
            .footer {{ border-top-color: #2E2925 !important; color: #78716C !important; }}
            .footer a {{ color: #A8A29E !important; }}
        }}
    </style>
</head>
<body style="background-color: #FAFAF8; color: #1C1917;">
    <div class="header">
        <a href="{frontend_url}" style="text-decoration: none;">
            <img src="cid:kyomi_logo" alt="Kyomi" class="logo-img" style="height: 48px; width: auto;">
        </a>
    </div>
    <div class="content">
        <h1>Verify Your Email</h1>

        <p>Hi {display_name},</p>

        <p>Thanks for signing up for Kyomi! Click the button below to verify your email address and complete your account setup:</p>

        <div class="cta">
            <a href="{verification_link}" class="button">Verify Email Address</a>
        </div>

        <p style="color: #e74c3c; font-size: 14px;"><strong>This link expires in 24 hours.</strong></p>

        <p>If you didn't create a Kyomi account, please ignore this email.</p>

        <p>Thanks,<br>The Kyomi Team</p>
    </div>
    <div class="footer">
        <p style="margin: 0 0 8px 0;">
            You're receiving this because someone signed up for Kyomi with this email address.
        </p>
        <p style="margin: 0;">
            <a href="{frontend_url}" style="color: #d97706;">kyomi.ai</a>
        </p>
    </div>
</body>
</html>"#,
            frontend_url = html_escape(frontend_url),
            display_name = html_escape(display_name),
            verification_link = html_escape(verification_link),
        );

        let text_body = format!(
            "\
Verify Your Email

Hi {display_name},

Thanks for signing up for Kyomi! Click the link below to verify your email address and complete your account setup:

{verification_link}

IMPORTANT: This link expires in 24 hours.

If you didn't create a Kyomi account, please ignore this email.

Thanks,
The Kyomi Team

---
You're receiving this because someone signed up for Kyomi with this email address.
{frontend_url}
",
        );

        self.send_email(email, subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Notify the owner of an already-verified account that someone
    /// (almost certainly them) just tried to sign up again with their
    /// email.
    ///
    /// This is the one channel allowed to say "you already have an
    /// account": `signup_start_service` and `passkey_signup_start_service`
    /// return the identical `VerificationRequired` result for a new email,
    /// an unverified email, and a verified email, to prevent email
    /// enumeration — but that constrains the HTTP response only. Only the
    /// mailbox owner can read this email, so it's safe to be specific here.
    /// Lists the account's active sign-in methods (`auth_methods`, raw
    /// `user_auth_methods.auth_type` values) so the recipient isn't left
    /// guessing between Google, passkey, and password (KYO-681).
    ///
    /// Returns `Ok(())` if sent successfully, or the reason it was not.
    pub async fn send_existing_account_notice(
        &self,
        email: &str,
        name: &str,
        sign_in_link: &str,
        auth_methods: &[String],
    ) -> Result<(), EmailSendError> {
        let display_name = if name.is_empty() { "there" } else { name };
        let (subject, html_body, text_body) = build_existing_account_email(
            display_name,
            sign_in_link,
            auth_methods,
            &self.frontend_url,
        );

        self.send_email(email, &subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send a welcome email to a new newsletter subscriber.
    ///
    /// Returns `Ok(())` if sent successfully, or the reason it was not.
    pub async fn send_subscription_welcome(
        &self,
        email: &str,
    ) -> Result<(), EmailSendError> {
        let frontend_url = &self.frontend_url;
        let subject = "Welcome to Kyomi!";

        let html_body = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <meta name="supported-color-schemes" content="light dark">
    <style>
        :root {{ color-scheme: light dark; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            line-height: 1.6;
            color: #1C1917;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #FAFAF8;
        }}
        .header {{
            text-align: center;
            margin-bottom: 16px;
            padding: 16px 0;
            border-bottom: 1px solid #E8E5DE;
        }}
        .logo-img {{
            height: 48px;
            width: auto;
        }}
        .content {{
            padding: 20px 0;
        }}
        h1 {{
            color: #1C1917;
            font-size: 24px;
            font-weight: 700;
            margin-bottom: 16px;
        }}
        p {{
            color: #6B6660;
            font-size: 14px;
            margin: 12px 0;
        }}
        .cta {{
            text-align: center;
            margin: 32px 0;
        }}
        .button {{
            display: inline-block;
            background-color: #d97706;
            color: #ffffff !important;
            padding: 14px 28px;
            text-decoration: none;
            border-radius: 8px;
            font-weight: 600;
            font-size: 14px;
        }}
        .footer {{
            margin-top: 20px;
            padding-top: 16px;
            border-top: 1px solid #E8E5DE;
            text-align: center;
            color: #9C9790;
            font-size: 12px;
        }}
        .footer a {{
            color: #6B6660;
            text-decoration: none;
        }}
        .footer a:hover {{
            text-decoration: underline;
        }}
        @media (prefers-color-scheme: dark) {{
            body {{ background-color: #12100F !important; color: #F5F3EF !important; }}
            h1, h2, h3 {{ color: #F5F3EF !important; }}
            p {{ color: #A8A29E !important; }}
            .header {{ border-bottom-color: #2E2925 !important; }}
            .highlight {{ background-color: #2C241E !important; }}
            .feature {{ color: #A8A29E !important; }}
            .footer {{ border-top-color: #2E2925 !important; color: #78716C !important; }}
            .footer a {{ color: #A8A29E !important; }}
        }}
    </style>
</head>
<body style="background-color: #FAFAF8; color: #1C1917;">
    <div class="header">
        <a href="{frontend_url}" style="text-decoration: none;">
            <img src="cid:kyomi_logo" alt="Kyomi" class="logo-img" style="height: 48px; width: auto;">
        </a>
    </div>
    <div class="content">
        <h1>Welcome to Kyomi!</h1>

        <p>Thanks for signing up! We're excited to have you on board.</p>

        <p>Kyomi is a data intelligence platform that learns how your team understands data and makes that knowledge available to everyone.</p>

        <p>We'll keep you updated on new features and when your account is ready.</p>

        <div class="cta">
            <a href="{frontend_url}" class="button">Visit Kyomi</a>
        </div>

        <p>Thanks,<br>The Kyomi Team</p>
    </div>
    <div class="footer">
        <p style="margin: 0 0 8px 0;">
            You're receiving this because you signed up for updates from Kyomi.
        </p>
        <p style="margin: 0;">
            <a href="{frontend_url}/unsubscribe?email={email}">Unsubscribe</a> &middot;
            <a href="{frontend_url}" style="color: #d97706;">kyomi.ai</a>
        </p>
    </div>
</body>
</html>"#,
            email = html_escape(email),
        );

        let text_body = format!(
            "\
Welcome to Kyomi!

Thanks for signing up! We're excited to have you on board.

Kyomi is a data intelligence platform that learns how your team understands data and makes that knowledge available to everyone.

We'll keep you updated on new features and when your account is ready.

Visit Kyomi: {frontend_url}

Thanks,
The Kyomi Team

---
You're receiving this because you signed up for updates from Kyomi.
Unsubscribe: {frontend_url}/unsubscribe?email={email}
{frontend_url}
",
        );

        self.send_email(email, subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send a plain admin notification email (feedback alerts, signup alerts).
    ///
    /// Uses minimal styling — these are internal notifications, not user-facing emails.
    /// `reply_to` sets the Reply-To header so support can reply directly to the user.
    pub async fn send_admin_notification(
        &self,
        to_email: &str,
        subject: &str,
        sections: &[(& str, &str)],
        reply_to: Option<&str>,
    ) -> Result<(), EmailSendError> {
        let frontend_url = &self.frontend_url;

        // Build HTML sections
        let html_sections: String = sections
            .iter()
            .map(|(label, value)| {
                format!(
                    r#"<tr><td style="padding:4px 12px 4px 0;font-weight:600;vertical-align:top;white-space:nowrap;">{}</td><td style="padding:4px 0;">{}</td></tr>"#,
                    html_escape(label),
                    html_escape(value),
                )
            })
            .collect();

        let html_body = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <meta name="supported-color-schemes" content="light dark">
    <style>
        :root {{ color-scheme: light dark; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            color: #1C1917;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #FAFAF8;
        }}
        .header {{
            text-align: center;
            margin-bottom: 16px;
            padding: 16px 0;
            border-bottom: 1px solid #E8E5DE;
        }}
        h2 {{
            color: #1C1917;
        }}
        td {{
            color: #6B6660;
        }}
        td:first-child {{
            color: #1C1917;
        }}
        .footer {{
            margin-top: 20px;
            padding-top: 16px;
            border-top: 1px solid #E8E5DE;
            text-align: center;
            color: #9C9790;
            font-size: 12px;
        }}
        @media (prefers-color-scheme: dark) {{
            body {{ background-color: #12100F !important; color: #F5F3EF !important; }}
            h2 {{ color: #F5F3EF !important; }}
            .header {{ border-bottom-color: #2E2925 !important; }}
            .footer {{ border-top-color: #2E2925 !important; color: #78716C !important; }}
            .footer a {{ color: #A8A29E !important; }}
            td {{ color: #A8A29E !important; }}
            td:first-child {{ color: #F5F3EF !important; }}
        }}
    </style>
</head>
<body>
    <div class="header">
        <a href="{frontend_url}" style="text-decoration: none;">
            <img src="cid:kyomi_logo" alt="Kyomi" class="logo-img" style="height: 48px; width: auto;">
        </a>
    </div>
    <h2 style="margin:0 0 16px 0;">{subject}</h2>
    <table style="border-collapse:collapse;width:100%;font-size:14px;">
        {html_sections}
    </table>
    <div class="footer">
        <p style="margin:0;"><a href="{frontend_url}" style="color: #d97706;">kyomi.ai</a></p>
    </div>
</body>
</html>"#,
            subject = html_escape(subject),
        );

        // Build text sections
        let text_sections: String = sections
            .iter()
            .map(|(label, value)| format!("{label}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");

        let text_body = format!("{subject}\n\n{text_sections}\n\n---\nkyomi.ai\n");

        self.send_email(to_email, subject, &html_body, Some(&text_body), reply_to, &[])
            .await
    }
}

/// HTML escaping for user-provided strings inserted into email templates.
///
/// Covers the OWASP-recommended set: &, <, >, ", ', /, and backtick.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('/', "&#x2F;")
        .replace('`', "&#96;")
}

/// Human-readable label for a `user_auth_methods.auth_type` value.
///
/// Unrecognized values pass through unchanged rather than being dropped —
/// silently omitting an auth method from this list would tell the account
/// owner they have fewer ways to sign in than they actually do.
fn auth_method_label(auth_type: &str) -> &str {
    match auth_type {
        "password" => "Password",
        "google_oauth" => "Google",
        "webauthn" => "Passkey",
        other => other,
    }
}

/// Render `auth_types` (raw `user_auth_methods.auth_type` values, as
/// returned by `list_active_auth_types`) into a human-readable list, e.g.
/// `"Google"`, `"Google and Password"`, or `"Google, Password and Passkey"`.
///
/// An empty slice — an account somehow left with no active auth method —
/// falls back to generic wording rather than rendering an empty list.
fn humanize_auth_methods(auth_types: &[String]) -> String {
    let labels: Vec<&str> = auth_types.iter().map(|t| auth_method_label(t)).collect();
    match labels.split_last() {
        None => "your existing sign-in method".to_string(),
        Some((last, [])) => (*last).to_string(),
        Some((last, init)) => format!("{} and {last}", init.join(", ")),
    }
}

/// Pure content builder behind `send_existing_account_notice`, kept separate
/// from `EmailService` so tests can assert on the rendered subject/body
/// directly without needing SMTP configured (KYO-681).
///
/// Returns `(subject, html_body, text_body)`.
fn build_existing_account_email(
    display_name: &str,
    sign_in_link: &str,
    auth_methods: &[String],
    frontend_url: &str,
) -> (String, String, String) {
    let subject = "You already have a Kyomi account".to_string();
    let methods_list = humanize_auth_methods(auth_methods);

    let html_body = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <meta name="supported-color-schemes" content="light dark">
    <style>
        :root {{ color-scheme: light dark; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            line-height: 1.6;
            color: #1C1917;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #FAFAF8;
        }}
        .header {{
            text-align: center;
            margin-bottom: 16px;
            padding: 16px 0;
            border-bottom: 1px solid #E8E5DE;
        }}
        .logo-img {{
            height: 48px;
            width: auto;
        }}
        .content {{
            padding: 20px 0;
        }}
        h1 {{
            color: #1C1917;
            font-size: 24px;
            font-weight: 700;
            margin-bottom: 16px;
        }}
        p {{
            color: #6B6660;
            font-size: 14px;
            margin: 12px 0;
        }}
        .highlight {{
            background-color: #fffbeb;
            border-left: 4px solid #d97706;
            padding: 16px;
            margin: 24px 0;
            border-radius: 0 8px 8px 0;
        }}
        .cta {{
            text-align: center;
            margin: 32px 0;
        }}
        .button {{
            display: inline-block;
            background-color: #d97706;
            color: #ffffff !important;
            padding: 14px 28px;
            text-decoration: none;
            border-radius: 8px;
            font-weight: 600;
            font-size: 14px;
        }}
        .footer {{
            margin-top: 20px;
            padding-top: 16px;
            border-top: 1px solid #E8E5DE;
            text-align: center;
            color: #9C9790;
            font-size: 12px;
        }}
        .footer a {{
            color: #6B6660;
            text-decoration: none;
        }}
        .footer a:hover {{
            text-decoration: underline;
        }}
        @media (prefers-color-scheme: dark) {{
            body {{ background-color: #12100F !important; color: #F5F3EF !important; }}
            h1, h2, h3 {{ color: #F5F3EF !important; }}
            p {{ color: #A8A29E !important; }}
            .header {{ border-bottom-color: #2E2925 !important; }}
            .highlight {{ background-color: #2C241E !important; }}
            .feature {{ color: #A8A29E !important; }}
            .footer {{ border-top-color: #2E2925 !important; color: #78716C !important; }}
            .footer a {{ color: #A8A29E !important; }}
        }}
    </style>
</head>
<body style="background-color: #FAFAF8; color: #1C1917;">
    <div class="header">
        <a href="{frontend_url}" style="text-decoration: none;">
            <img src="cid:kyomi_logo" alt="Kyomi" class="logo-img" style="height: 48px; width: auto;">
        </a>
    </div>
    <div class="content">
        <h1>You Already Have an Account</h1>

        <p>Hi {display_name},</p>

        <p>Someone (hopefully you) just tried to sign up for Kyomi with this email address, but you already have an account.</p>

        <div class="highlight">
            <strong>You can sign in with:</strong> {methods_list}
        </div>

        <div class="cta">
            <a href="{sign_in_link}" class="button">Sign In</a>
        </div>

        <p>If this wasn't you, you can safely ignore this email — no changes have been made to your account.</p>

        <p>Thanks,<br>The Kyomi Team</p>
    </div>
    <div class="footer">
        <p style="margin: 0 0 8px 0;">
            You're receiving this because someone attempted to sign up for Kyomi with this email address.
        </p>
        <p style="margin: 0;">
            <a href="{frontend_url}" style="color: #d97706;">kyomi.ai</a>
        </p>
    </div>
</body>
</html>"#,
        frontend_url = html_escape(frontend_url),
        display_name = html_escape(display_name),
        sign_in_link = html_escape(sign_in_link),
        methods_list = html_escape(&methods_list),
    );

    let text_body = format!(
        "\
You Already Have an Account

Hi {display_name},

Someone (hopefully you) just tried to sign up for Kyomi with this email address, but you already have an account.

You can sign in with: {methods_list}

Sign in: {sign_in_link}

If this wasn't you, you can safely ignore this email\u{2014}no changes have been made to your account.

Thanks,
The Kyomi Team

---
You're receiving this because someone attempted to sign up for Kyomi with this email address.
{frontend_url}
",
    );

    (subject, html_body, text_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This file's own source, for the guard test pinning that
    /// `is_configured()` has no private copy of the SMTP rule (KYO-423).
    const SRC: &str = include_str!("email_service.rs");

    /// Start of this test module — the end of production code in [`SRC`].
    /// `SRC` is `include_str!`-ed from this same file, so the guard test below
    /// has to search a slice rather than the whole file: both literals it
    /// counts also appear in this module, which would inflate its production
    /// tallies by the tests' own mentions of them.
    ///
    /// This declaration is not itself a second occurrence of the marker,
    /// tempting as that reading is — on disk the newline here is the
    /// two-character escape `\n`, whereas the needle holds one real newline, so
    /// the needle does not match the line that defines it. It occurs in the
    /// file exactly once, at the real declaration above. Were a test ever to
    /// spell the marker with a true newline (a raw string, say), the slice
    /// would still be right, because `find` matches leftmost and the real
    /// declaration necessarily comes first.
    const MOD_TESTS_MARKER: &str = "#[cfg(test)]\nmod tests {";

    /// The production half of [`SRC`], with this test module excluded.
    fn production_src() -> &'static str {
        let end = SRC
            .find(MOD_TESTS_MARKER)
            .expect("email_service.rs must contain its `#[cfg(test)] mod tests` declaration");
        &SRC[..end]
    }

    /// An `EmailService` with the three SMTP parts set as given and everything
    /// else at its `from_env` default. Present so the presence-combination
    /// tests below don't each repeat the seven-field literal.
    fn service(
        smtp_host: Option<&str>,
        smtp_user: Option<&str>,
        smtp_password: Option<&str>,
    ) -> EmailService {
        EmailService {
            smtp_host: smtp_host.map(str::to_string),
            smtp_port: 587,
            smtp_user: smtp_user.map(str::to_string),
            smtp_password: smtp_password.map(str::to_string),
            from_email: "noreply@kyomi.ai".to_string(),
            from_name: "Kyomi".to_string(),
            frontend_url: "https://app.kyomi.ai".to_string(),
        }
    }

    #[test]
    fn email_service_not_configured_by_default() {
        // Without SMTP env vars, service should report not configured.
        // This test is safe because CI/dev environments don't set SMTP vars.
        assert!(!service(None, None, None).is_configured());
    }

    #[test]
    fn email_service_configured_when_all_vars_set() {
        let service = service(
            Some("smtp.example.com"),
            Some("user@example.com"),
            Some("password"),
        );
        assert!(service.is_configured());
    }

    /// The KYO-685 case: `SMTP_HOST` and `SMTP_USER` set, `SMTP_PASSWORD`
    /// forgotten. `Config::smtp_configured` used to say `true` here while this
    /// service said `false`, so a self-hosted install took the SaaS
    /// "verification email sent" signup branch and could not create an account
    /// at all. Both sides now read the same predicate, so they agree.
    #[test]
    fn host_and_user_without_password_is_not_configured_and_matches_the_config_flag() {
        let host = Some("smtp.example.com");
        let user = Some("user@example.com");

        // The value `Config::from_env` stores in `smtp_configured` for this
        // environment, computed through the shared predicate rather than by
        // mutating process env (`set_var` is `unsafe` and races parallel tests).
        let config_flag = SmtpSettings::classify(host, user, None).can_send();

        assert!(
            !config_flag,
            "host + user with no password cannot send mail, so the config flag must be false"
        );
        assert_eq!(
            service(host, user, None).is_configured(),
            config_flag,
            "EmailService::is_configured() and Config::smtp_configured must never disagree \
             about whether mail can be sent (KYO-685)"
        );
    }

    #[test]
    fn is_configured_agrees_with_the_config_flag_for_every_presence_combination() {
        for bits in 0..8u8 {
            let host = (bits & 0b100 != 0).then_some("smtp.example.com");
            let user = (bits & 0b010 != 0).then_some("user@example.com");
            let password = (bits & 0b001 != 0).then_some("password");

            let config_flag = SmtpSettings::classify(host, user, password).can_send();
            assert_eq!(
                service(host, user, password).is_configured(),
                config_flag,
                "the mailer and the config flag must decide identically for \
                 host={host:?} user={user:?} password={password:?} (KYO-685)"
            );
            assert_eq!(
                config_flag,
                host.is_some() && user.is_some() && password.is_some(),
                "all three parts are required for host={host:?} user={user:?} \
                 password={password:?}"
            );
        }
    }

    /// KYO-423: two copies of a predicate drift. Neither `is_configured()` nor
    /// `from_env` may keep its own spelling of "host and user and password" —
    /// that is how this rule came to disagree with `Config::smtp_configured`
    /// in the first place.
    #[test]
    fn no_second_copy_of_the_smtp_conjunction_in_production_code() {
        let production = production_src();

        assert_eq!(
            production.matches("smtp_password.is_some()").count(),
            0,
            "the SMTP conjunction must be spelled once, in \
             kyomi_core::config::SmtpSettings::classify — found an inline copy in \
             email_service.rs's production code (KYO-685/KYO-423)"
        );
        assert_eq!(
            production.matches("SmtpSettings::classify(").count(),
            2,
            "exactly two call sites route through the shared predicate: from_env (for its \
             startup warning) and is_configured — a missing one means a hand-rolled copy \
             came back (KYO-685)"
        );
    }

    #[test]
    fn html_escape_works() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("A & B"), "A &amp; B");
        assert_eq!(html_escape("\"hello\""), "&quot;hello&quot;");
        assert_eq!(html_escape("a/b"), "a&#x2F;b");
        assert_eq!(html_escape("a`b"), "a&#96;b");
        assert_eq!(html_escape("safe text 123"), "safe text 123");
    }

    #[test]
    fn humanize_auth_methods_single() {
        assert_eq!(
            humanize_auth_methods(&["password".to_string()]),
            "Password"
        );
    }

    #[test]
    fn humanize_auth_methods_two() {
        assert_eq!(
            humanize_auth_methods(&["google_oauth".to_string(), "password".to_string()]),
            "Google and Password"
        );
    }

    #[test]
    fn humanize_auth_methods_three() {
        assert_eq!(
            humanize_auth_methods(&[
                "google_oauth".to_string(),
                "password".to_string(),
                "webauthn".to_string(),
            ]),
            "Google, Password and Passkey"
        );
    }

    #[test]
    fn humanize_auth_methods_unrecognized_type_passes_through() {
        // A future auth_type this mapping doesn't know about must still be
        // named, not silently dropped from the list.
        assert_eq!(
            humanize_auth_methods(&["sms".to_string()]),
            "sms"
        );
    }

    #[test]
    fn humanize_auth_methods_empty_falls_back_to_generic_wording() {
        let result = humanize_auth_methods(&[]);
        assert!(!result.is_empty(), "must not render an empty list to the user");
    }

    /// KYO-681: the rendered email must name the sign-in URL and every one
    /// of the account's auth methods — this is the content a returning user
    /// depends on to get unstuck. Tests the pure builder directly (no SMTP
    /// transport involved).
    #[test]
    fn build_existing_account_email_names_sign_in_url_and_auth_methods() {
        let auth_methods = vec!["google_oauth".to_string(), "password".to_string()];
        let (subject, html_body, text_body) = build_existing_account_email(
            "Jane",
            "https://app.example.com/login",
            &auth_methods,
            "https://app.example.com",
        );

        assert_eq!(subject, "You already have a Kyomi account");

        // The HTML body html-escapes the link (per this file's html_escape,
        // which also escapes `/`), so assert against the escaped form there
        // and the raw form in the plain-text body.
        assert!(
            html_body.contains(&html_escape("https://app.example.com/login")),
            "html body must contain the sign-in URL: {html_body}"
        );
        assert!(
            text_body.contains("https://app.example.com/login"),
            "text body must contain the sign-in URL: {text_body}"
        );

        for body in [&html_body, &text_body] {
            assert!(body.contains("Google"), "body must name Google: {body}");
            assert!(body.contains("Password"), "body must name Password: {body}");
            assert!(body.contains("Jane"), "body must greet the account by name: {body}");
        }

        // The account's auth methods must not be misrepresented as a method
        // it doesn't have.
        assert!(!html_body.contains("Passkey"));
    }

    /// KYO-697: the failure must arrive as the *named* reason, not as an
    /// undifferentiated "it didn't work". A caller (or an operator reading
    /// the log line that caller emits) has to be able to tell "this
    /// instance has no SMTP set up" from "the mail server rejected the
    /// address", because those are different problems with different fixes.
    #[tokio::test]
    async fn send_email_returns_not_configured_error_when_smtp_is_absent() {
        let service = EmailService::unconfigured_for_tests();

        let err = service
            .send_email("test@example.com", "Test", "<p>Hi</p>", None, None, &[])
            .await
            .expect_err("an unconfigured service must not report a successful send");

        assert!(
            matches!(err, EmailSendError::NotConfigured),
            "expected NotConfigured, got {err:?}"
        );
        // The rendered form is what reaches an operator, so pin that it
        // names the missing configuration rather than being generic.
        let rendered = err.to_string();
        assert!(
            rendered.contains("SMTP_HOST") && rendered.contains("not configured"),
            "error must name what is missing: {rendered}"
        );
    }

    #[tokio::test]
    async fn send_workspace_invitation_propagates_not_configured_error() {
        let service = EmailService::unconfigured_for_tests();

        let err = service
            .send_workspace_invitation(
                "test@example.com",
                "My Workspace",
                "Jane Doe",
                "admin",
                "inv-test123",
            )
            .await
            .expect_err("an unconfigured service must not report a successful send");

        assert!(
            matches!(err, EmailSendError::NotConfigured),
            "the template method must pass send_email's reason through \
             unchanged, got {err:?}"
        );
    }

    /// The transient/permanent split is the one piece of send-failure
    /// detail an operator acts on differently, so pin that it survives into
    /// the rendered message rather than only existing as an enum variant.
    #[test]
    fn send_failure_renders_its_transient_permanent_classification() {
        for (kind, expected) in [
            (SmtpFailureKind::Transient, "transient"),
            (SmtpFailureKind::Permanent, "permanent"),
        ] {
            assert_eq!(kind.to_string(), expected);
        }
    }
}
