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
//! Graceful degradation: if SMTP is not configured, `send_email` logs a warning
//! and returns `false` — it never fails the calling operation.

use lettre::{
    message::{header::ContentType, Attachment, Body, Mailbox, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

/// Kyomi logo PNG, embedded at compile time.
static LOGO_BYTES: &[u8] =
    include_bytes!("../../../assets/kyomi_email_logo.png");

/// Content-ID used in `<img src="cid:kyomi_logo">`.
const LOGO_CID: &str = "kyomi_logo";

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

        let configured = smtp_host.is_some() && smtp_user.is_some() && smtp_password.is_some();
        if !configured {
            tracing::warn!(
                "SMTP not configured. Email sending will be disabled. \
                 Set SMTP_HOST, SMTP_PORT, SMTP_USER, SMTP_PASSWORD in .env"
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

    /// Check if SMTP is configured (all required env vars are set).
    pub fn is_configured(&self) -> bool {
        self.smtp_host.is_some() && self.smtp_user.is_some() && self.smtp_password.is_some()
    }

    /// Send an email via SMTP.
    ///
    /// Returns `true` if the email was sent successfully, `false` otherwise.
    /// Never panics or returns an error — logs warnings on failure.
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
    ) -> bool {
        if !self.is_configured() {
            tracing::warn!(
                to = %to_email,
                "SMTP not configured. Skipping email."
            );
            return false;
        }

        let (Some(smtp_host), Some(smtp_user), Some(smtp_password)) = (
            self.smtp_host.as_deref(),
            self.smtp_user.as_deref(),
            self.smtp_password.as_deref(),
        ) else {
            tracing::error!("SMTP config missing despite is_configured() check");
            return false;
        };

        // Build the From mailbox
        let from_mailbox: Mailbox = match format!("{} <{}>", self.from_name, self.from_email).parse()
        {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(
                    "Failed to parse From address '{}': {}",
                    self.from_email,
                    e
                );
                return false;
            }
        };

        // Build the To mailbox
        let to_mailbox: Mailbox = match to_email.parse() {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Failed to parse To address '{}': {}", to_email, e);
                return false;
            }
        };

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

        // Wrap in multipart/related so the inline logo CID resolves
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

        let message = match builder.multipart(related) {
            Ok(msg) => msg,
            Err(e) => {
                tracing::error!(to = %to_email, "Failed to build email message: {}", e);
                return false;
            }
        };

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

        let mailer = match mailer_result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(to = %to_email, "Failed to create SMTP transport: {}", e);
                return false;
            }
        };

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
                e.is_transient() || e.is_timeout()
            },
        )
        .await;

        match send_result {
            Ok(_) => {
                tracing::info!(to = %to_email, subject = %subject, "Email sent successfully");
                true
            }
            Err(e) => {
                tracing::error!(to = %to_email, "Failed to send email: {}", e);
                false
            }
        }
    }

    /// Send a workspace invitation email.
    ///
    /// Returns `true` if sent successfully.
    pub async fn send_workspace_invitation(
        &self,
        email: &str,
        workspace_name: &str,
        inviter_name: &str,
        role: &str,
    ) -> bool {
        let role_display = if role == "admin" {
            "an Admin"
        } else {
            "a Member"
        };

        let frontend_url = &self.frontend_url;
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
                <strong>Log in to Kyomi</strong> - Sign in with the email address this invitation was sent to ({email})
            </div>
            <div class="feature">
                <strong>Accept the invitation</strong> - You'll see a prompt to accept when you log in
            </div>
            <div class="feature">
                <strong>Start collaborating</strong> - Access shared dashboards and insights with your team
            </div>
        </div>

        <div class="cta">
            <a href="{frontend_url}/login" class="button">Log In to Accept</a>
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

1. Log in to Kyomi - Sign in with the email address this invitation was sent to ({email})
2. Accept the invitation - You'll see a prompt to accept when you log in
3. Start collaborating - Access shared dashboards and insights with your team

Log in to accept: {frontend_url}/login

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

    /// Send a passkey recovery email.
    ///
    /// Returns `true` if sent successfully.
    pub async fn send_passkey_recovery(
        &self,
        email: &str,
        name: &str,
        recovery_link: &str,
    ) -> bool {
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
    /// Returns `true` if sent successfully.
    pub async fn send_account_recovery(
        &self,
        email: &str,
        name: &str,
        recovery_link: &str,
    ) -> bool {
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
    /// Returns `true` if sent successfully.
    pub async fn send_verification_email(
        &self,
        email: &str,
        name: &str,
        verification_link: &str,
    ) -> bool {
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

    /// Send a welcome email to a new newsletter subscriber.
    ///
    /// Returns `true` if sent successfully.
    pub async fn send_subscription_welcome(
        &self,
        email: &str,
    ) -> bool {
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
    ) -> bool {
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
            <img src="cid:kyomi_logo" alt="Kyomi" style="height: 48px; width: auto;">
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_service_not_configured_by_default() {
        // Without SMTP env vars, service should report not configured.
        // This test is safe because CI/dev environments don't set SMTP vars.
        let service = EmailService {
            smtp_host: None,
            smtp_port: 587,
            smtp_user: None,
            smtp_password: None,
            from_email: "noreply@kyomi.ai".to_string(),
            from_name: "Kyomi".to_string(),
            frontend_url: "https://app.kyomi.ai".to_string(),
        };
        assert!(!service.is_configured());
    }

    #[test]
    fn email_service_configured_when_all_vars_set() {
        let service = EmailService {
            smtp_host: Some("smtp.example.com".to_string()),
            smtp_port: 587,
            smtp_user: Some("user@example.com".to_string()),
            smtp_password: Some("password".to_string()),
            from_email: "noreply@kyomi.ai".to_string(),
            from_name: "Kyomi".to_string(),
            frontend_url: "https://app.kyomi.ai".to_string(),
        };
        assert!(service.is_configured());
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

    #[tokio::test]
    async fn send_email_returns_false_when_not_configured() {
        let service = EmailService {
            smtp_host: None,
            smtp_port: 587,
            smtp_user: None,
            smtp_password: None,
            from_email: "noreply@kyomi.ai".to_string(),
            from_name: "Kyomi".to_string(),
            frontend_url: "https://app.kyomi.ai".to_string(),
        };

        let result = service
            .send_email("test@example.com", "Test", "<p>Hi</p>", None, None, &[])
            .await;
        assert!(!result);
    }

    #[tokio::test]
    async fn send_workspace_invitation_returns_false_when_not_configured() {
        let service = EmailService {
            smtp_host: None,
            smtp_port: 587,
            smtp_user: None,
            smtp_password: None,
            from_email: "noreply@kyomi.ai".to_string(),
            from_name: "Kyomi".to_string(),
            frontend_url: "https://app.kyomi.ai".to_string(),
        };

        let result = service
            .send_workspace_invitation("test@example.com", "My Workspace", "Jane Doe", "admin")
            .await;
        assert!(!result);
    }
}
