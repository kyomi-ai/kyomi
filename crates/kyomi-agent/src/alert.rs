// SPDX-License-Identifier: AGPL-3.0-or-later

//! Alert delivery — sends watch alerts/reports via WebSocket, email, and Web Push.
//!
//! This module orchestrates alert delivery across all configured channels.
//! Each channel is best-effort: failure in one channel does not affect others.
//!
//! Delivery channels:
//! 1. **WebSocket** — Always sent. Toast notification + badge update.
//! 2. **Email** — If `watch.alert_emails_enabled` and `watch.alert_emails` is set.
//! 3. **Web Push** — If VAPID is configured and user has push subscriptions.
//!
//! Slack delivery was moved to the `kyomi-slack` enterprise crate (Phase 12).
//! It is invoked via the `PlatformRegistry` when wired in (Task 5).

use std::sync::{Arc, LazyLock};

use crate::tools::QueryContext;
use kyomi_core::platform::PlatformRegistry;

// ---------------------------------------------------------------------------
// Embedded assets
// ---------------------------------------------------------------------------

/// Kyomi email logo PNG, embedded at compile time (~7 KB).
static LOGO_BYTES: &[u8] = include_bytes!("../../../assets/kyomi_email_logo.png");

/// Content-ID used for the logo in email `<img src="cid:...">` references.
const LOGO_CID: &str = "kyomi_logo";
use kyomi_auth::websocket::helpers as ws_helpers;
use kyomi_auth::websocket::WebSocketManager;
use kyomi_core::models::Watch;
use kyomi_core::{Config, DbPool, WatchMode};
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct EmailRow {
    email: String,
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// Deliver a watch alert/report via all configured channels.
///
/// Channels (all best-effort):
/// 1. **WebSocket** — Always sent. Toast notification + badge update.
/// 2. **Messaging platforms** — Via `PlatformRegistry` and `watch_alert_channels` table.
/// 3. **Email** — If `watch.alert_emails_enabled` and `watch.alert_emails` set.
/// 4. **Web Push** — If VAPID is configured and user has push subscriptions.
///
/// Each channel is independent — failure in one does not affect the others.
/// Errors are logged but never propagated.
#[allow(clippy::too_many_arguments)]
pub async fn deliver_watch_alert(
    db: &DbPool,
    encryption_key: &Arc<[u8; 32]>,
    ws_manager: &WebSocketManager,
    config: &Arc<Config>,
    connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
    platforms: &Arc<PlatformRegistry>,
    watch: &Watch,
    execution_id: i32,
    alert_title: &str,
    summary: &str,
    message: &str,
    mode: WatchMode,
) {
    // 1. WebSocket: Always send the alert notification.
    // Use summary for clean plain-text preview; fall back to truncated markdown for
    // backward compat during rolling deploys where summary might be empty.
    let preview = if summary.is_empty() {
        truncate_preview(message, 120)
    } else {
        truncate_preview(summary, 120)
    };

    ws_helpers::send_watch_alert(
        ws_manager,
        &watch.created_by,
        &watch.watch_id,
        &watch.name,
        &execution_id.to_string(),
        &preview,
        summary,
    )
    .await;

    // Build a lightweight query context shared by email and platform paths.
    let query_ctx = QueryContext {
        db: db.clone(),
        user_id: watch.created_by.clone(),
        workspace_id: watch.workspace_id.clone(),
        encryption_key: encryption_key.clone(),
        config: config.clone(),
        connect_registry,
    };

    // 2. Platform alert delivery (Slack, Teams, etc.) via PlatformRegistry.
    match kyomi_core::platform::get_watch_alert_channels(db, &watch.watch_id).await {
        Ok(channels) => {
            for channel in channels {
                if let Some(platform) = platforms.get(&channel.channel_type) {
                    let payload = kyomi_core::platform::AlertPayload {
                        watch_name: watch.name.clone(),
                        alert_title: alert_title.to_string(),
                        markdown: message.to_string(),
                        charts: vec![],
                        execution_id,
                        web_url: format!(
                            "{}/watches/alerts?alert={}",
                            config.frontend_url, execution_id
                        ),
                        mode,
                        creator_user_id: watch.created_by.clone(),
                        workspace_id: watch.workspace_id.clone(),
                    };
                    if let Err(e) = platform.send_alert(&channel.channel_id, &payload).await {
                        warn!(
                            platform = %channel.channel_type,
                            channel_id = %channel.channel_id,
                            error = %e,
                            "Platform alert delivery failed"
                        );
                    }
                }
            }
        }
        Err(e) => {
            warn!(
                watch_id = %watch.watch_id,
                error = %e,
                "Failed to load watch alert channels"
            );
        }
    }

    // 3. Email: If watch has email alerts enabled and recipients configured.
    if watch.alert_emails_enabled
        && let Some(ref emails_str) = watch.alert_emails
    {
        let email_list: Vec<&str> = emails_str
            .split(',')
            .map(|e| e.trim())
            .filter(|e| !e.is_empty())
            .collect();

        if !email_list.is_empty() {
            // Self-hosted edition gates for email alerts.
            if config.self_hosted && !config.is_enterprise() {
                tracing::debug!(
                    watch_id = %watch.watch_id,
                    "Email alerts not available in Community edition"
                );
            } else if config.self_hosted && !config.smtp_configured() {
                tracing::warn!(
                    watch_id = %watch.watch_id,
                    "Watch email alerts skipped: SMTP not configured"
                );
            } else {
            // Look up creator email for attribution
            let creator_email = lookup_creator_email(db, &watch.created_by).await;

            let email_service = EmailService::from_env();
            if email_service.is_configured() {
                let success = email_service
                    .send_watch_alert_emails(
                        &email_list,
                        &watch.name,
                        alert_title,
                        message,
                        execution_id,
                        &config.frontend_url,
                        creator_email.as_deref(),
                        mode,
                        &config.chart_renderer_url,
                        &query_ctx,
                    )
                    .await;

                let log_type = if mode == WatchMode::Report { "report" } else { "alert" };
                if success {
                    info!(
                        watch_id = %watch.watch_id,
                        recipients = email_list.len(),
                        "Email {log_type} sent to {} recipient(s)",
                        email_list.len()
                    );
                } else {
                    warn!(
                        watch_id = %watch.watch_id,
                        "Some email notifications failed to send"
                    );
                }
            } else {
                warn!(
                    watch_id = %watch.watch_id,
                    "Email alerts configured but SMTP is not set up"
                );
            }
            } // end else (edition gate passed)
        }
    }

    // 4. Web Push: Send to all subscribed devices for the watch creator.
    if let Some(vapid_config) = build_vapid_config(config) {
        let is_report = mode == WatchMode::Report;
        let push_payload = crate::web_push::PushPayload {
            notification_type: if is_report {
                "watch_report".into()
            } else {
                "watch_alert".into()
            },
            watch_id: watch.watch_id.clone(),
            watch_name: watch.name.clone(),
            execution_id,
            title: alert_title.to_string(),
            body: preview.clone(),  // Uses summary (clean plain text) when available
            url: format!("/watches/alerts?alert={execution_id}"),
            icon: "/kyomi_icon_192.png".to_string(),
        };

        let http_client = reqwest::Client::new();
        let push_count = crate::web_push::send_push_notifications(
            db,
            &http_client,
            &vapid_config,
            &watch.created_by,
            &push_payload,
        )
        .await;

        if push_count > 0 {
            let log_type = if is_report { "report" } else { "alert" };
            info!(
                watch_id = %watch.watch_id,
                devices = push_count,
                "Sent push {log_type} to {push_count} device(s)"
            );
        }
    }

    let log_type = if mode == WatchMode::Report { "report" } else { "alert" };
    info!(
        watch_id = %watch.watch_id,
        execution_id = execution_id,
        alert_title = %alert_title,
        mode = %mode,
        "Delivered watch {log_type}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Truncate a message to `max_chars` characters (Unicode-safe) and append "..." if truncated.
fn truncate_preview(message: &str, max_chars: usize) -> String {
    if message.len() <= max_chars {
        // Fast path: ASCII-only or short enough
        let char_count = message.chars().count();
        if char_count <= max_chars {
            return message.to_string();
        }
    }

    // Unicode-safe truncation
    let boundary = message
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(message.len());

    if boundary >= message.len() {
        message.to_string()
    } else {
        format!("{}...", &message[..boundary])
    }
}

/// Build a VAPID config from the application config, if VAPID keys are configured.
///
/// Returns `None` if either `vapid_private_key` or `vapid_contact` is not set.
fn build_vapid_config(config: &Config) -> Option<crate::web_push::VapidConfig> {
    let private_key = config.vapid_private_key.as_ref()?;
    let contact = config.vapid_contact.as_ref()?;

    match crate::web_push::VapidConfig::from_config(private_key, contact) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            error!(error = %e, "Failed to parse VAPID config");
            None
        }
    }
}

/// Look up the creator's email address for email attribution.
async fn lookup_creator_email(db: &DbPool, user_id: &str) -> Option<String> {
    let row: Option<EmailRow> = kyomi_core::db_fetch_optional!(
        db, EmailRow,
        "SELECT email FROM users WHERE user_id = $1",
        user_id
    )
    .ok()?;
    row.map(|r| r.email)
}

// ---------------------------------------------------------------------------
// Email delivery
// ---------------------------------------------------------------------------

/// SMTP email service for sending watch alerts.
///
/// Initialized from environment variables:
/// - `SMTP_HOST` — SMTP server hostname
/// - `SMTP_PORT` — SMTP port (587 for STARTTLS, 465 for SMTPS)
/// - `SMTP_USER` — SMTP username
/// - `SMTP_PASSWORD` — SMTP password
/// - `SMTP_FROM_EMAIL` — Sender email address (default: `noreply@kyomi.ai`)
/// - `SMTP_FROM_NAME` — Sender display name (default: `Kyomi`)
pub struct EmailService {
    host: Option<String>,
    port: u16,
    user: Option<String>,
    password: Option<String>,
    from_email: String,
    from_name: String,
}

impl EmailService {
    /// Create an `EmailService` from environment variables.
    pub fn from_env() -> Self {
        Self {
            host: std::env::var("SMTP_HOST").ok(),
            port: std::env::var("SMTP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(587),
            user: std::env::var("SMTP_USER").ok(),
            password: std::env::var("SMTP_PASSWORD").ok(),
            from_email: std::env::var("SMTP_FROM_EMAIL")
                .unwrap_or_else(|_| "noreply@kyomi.ai".into()),
            from_name: std::env::var("SMTP_FROM_NAME")
                .unwrap_or_else(|_| "Kyomi".into()),
        }
    }

    /// Check if all required SMTP configuration is present.
    pub fn is_configured(&self) -> bool {
        self.host.is_some() && self.user.is_some() && self.password.is_some()
    }

    /// Send a single email via SMTP with optional inline CID images.
    ///
    /// Supports STARTTLS (port 587) and direct TLS/SSL (port 465).
    /// The logo image is always attached as a CID inline image.
    /// Additional images (e.g. rendered charts) can be passed via `images`.
    ///
    /// Returns `true` on success, `false` on failure.
    async fn send_email(
        &self,
        to_email: &str,
        subject: &str,
        html_body: &str,
        images: &[(String, Vec<u8>)],
    ) -> bool {
        use lettre::message::{Attachment, Body, Mailbox, MultiPart, SinglePart};
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, AsyncTransport, Message};

        let Some(ref host) = self.host else {
            warn!("SMTP not configured, skipping email");
            return false;
        };
        let Some(ref user) = self.user else {
            return false;
        };
        let Some(ref password) = self.password else {
            return false;
        };

        // Build the email message
        let from_mailbox: Mailbox = match format!("{} <{}>", self.from_name, self.from_email).parse()
        {
            Ok(m) => m,
            Err(e) => {
                error!(error = %e, "Invalid sender email address");
                return false;
            }
        };

        let to_mailbox: Mailbox = match to_email.parse() {
            Ok(m) => m,
            Err(e) => {
                error!(to = %to_email, error = %e, "Invalid recipient email address");
                return false;
            }
        };

        // Build multipart/related: HTML + inline images (logo + charts)
        let content_type: lettre::message::header::ContentType =
            "image/png".parse().expect("valid content type");

        let mut related = MultiPart::related()
            .singlepart(SinglePart::html(html_body.to_string()));

        // Always attach the Kyomi logo
        related = related.singlepart(
            Attachment::new_inline(LOGO_CID.to_string())
                .body(Body::new(LOGO_BYTES.to_vec()), content_type.clone()),
        );

        // Attach chart images
        for (cid, png_bytes) in images {
            related = related.singlepart(
                Attachment::new_inline(cid.clone())
                    .body(Body::new(png_bytes.clone()), content_type.clone()),
            );
        }

        let email = match Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject(subject)
            .multipart(related)
        {
            Ok(e) => e,
            Err(e) => {
                error!(error = %e, "Failed to build email message");
                return false;
            }
        };

        let credentials = Credentials::new(user.clone(), password.clone());

        // Build the transport — use SMTPS for port 465, STARTTLS for everything else
        let send_result = if self.port == 465 {
            let transport = match AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(host) {
                Ok(builder) => builder
                    .credentials(credentials)
                    .port(self.port)
                    .build(),
                Err(e) => {
                    error!(host = %host, error = %e, "Failed to create SMTP transport");
                    return false;
                }
            };
            transport.send(email).await
        } else {
            let transport = match AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(host)
            {
                Ok(builder) => builder
                    .credentials(credentials)
                    .port(self.port)
                    .build(),
                Err(e) => {
                    error!(host = %host, error = %e, "Failed to create SMTP STARTTLS transport");
                    return false;
                }
            };
            transport.send(email).await
        };

        match send_result {
            Ok(_response) => {
                info!(to = %to_email, "Email sent successfully");
                true
            }
            Err(e) => {
                error!(to = %to_email, error = %e, "Failed to send email");
                false
            }
        }
    }

    /// Send watch alert/report emails to all recipients.
    ///
    /// Renders ChartML blocks to inline images (via chart-renderer) and attaches
    /// the Kyomi logo as a CID image. Returns `true` if at least one email was
    /// sent successfully.
    #[allow(clippy::too_many_arguments)]
    async fn send_watch_alert_emails(
        &self,
        emails: &[&str],
        watch_name: &str,
        alert_title: &str,
        message: &str,
        execution_id: i32,
        frontend_url: &str,
        creator_email: Option<&str>,
        mode: WatchMode,
        chart_renderer_url: &str,
        query_ctx: &QueryContext,
    ) -> bool {
        if emails.is_empty() {
            return false;
        }

        // Process the message once (render charts, convert markdown → HTML).
        let (message_html, chart_images) =
            process_message_for_email(message, chart_renderer_url, query_ctx).await;

        let mut any_success = false;

        for &email in emails {
            let (subject, html_body) = build_watch_alert_email(
                email,
                watch_name,
                alert_title,
                &message_html,
                execution_id,
                frontend_url,
                creator_email,
                mode,
            );

            if self.send_email(email, &subject, &html_body, &chart_images).await {
                any_success = true;
            }
        }

        any_success
    }
}

// ---------------------------------------------------------------------------
// Email message processing — ChartML → rendered CID images
// ---------------------------------------------------------------------------

/// Maximum number of charts to render for a single email.
const MAX_EMAIL_CHARTS: usize = 3;

/// Email chart render dimensions (px).
const EMAIL_CHART_WIDTH: u32 = 600;
const EMAIL_CHART_HEIGHT: u32 = 350;

/// Regex to capture the *content* inside ` ```chartml ... ``` ` fenced blocks.
static RE_CHARTML_CAPTURE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?s)```chartml\s*\n([\s\S]*?)\n```").expect("valid regex")
});

/// Process a watch message for email delivery.
///
/// 1. Extract ChartML blocks from the message.
/// 2. **Resolve data queries** — execute SQL against datasources to populate inline rows.
/// 3. Render up to [`MAX_EMAIL_CHARTS`] charts via the chart-renderer service.
/// 4. Replace rendered blocks with `<img src="cid:...">` HTML.
/// 5. Replace any remaining (unrendered) ChartML/YAML-chart blocks with placeholder text.
/// 6. Convert the result through [`markdown_to_simple_html`].
///
/// Returns `(html_content, cid_images)`.
async fn process_message_for_email(
    message: &str,
    chart_renderer_url: &str,
    query_ctx: &QueryContext,
) -> (String, Vec<(String, Vec<u8>)>) {
    use crate::tools::chart_data_resolver;
    use crate::tools::chart_palettes;
    use crate::tools::chart_renderer::ChartRendererClient;

    let mut images: Vec<(String, Vec<u8>)> = Vec::new();
    let mut processed = message.to_string();

    // Collect all chartml blocks with their byte ranges and YAML content.
    let blocks: Vec<(std::ops::Range<usize>, String)> = RE_CHARTML_CAPTURE
        .captures_iter(message)
        .filter_map(|cap| {
            let full_match = cap.get(0)?;
            let yaml_content = cap[1].to_string();
            Some((full_match.start()..full_match.end(), yaml_content))
        })
        .collect();

    if !blocks.is_empty() {
        // Get user's palette preference for chart rendering.
        let user_palette = chart_palettes::get_user_palette(&query_ctx.db, &query_ctx.user_id).await;

        // Create renderer and health-check if URL is configured.
        let renderer = if !chart_renderer_url.is_empty() {
            match ChartRendererClient::new(chart_renderer_url) {
                Ok(r) if r.health_check().await => Some(r),
                Ok(_) => {
                    warn!("Chart renderer unavailable — charts will be replaced with placeholders");
                    None
                }
                Err(e) => {
                    warn!(error = %e, "Failed to create chart renderer client");
                    None
                }
            }
        } else {
            None
        };

        // Process blocks in reverse order so byte offsets stay valid after replacements.
        for (idx, (range, yaml_content)) in blocks.iter().enumerate().rev() {
            if renderer.is_none() || idx >= MAX_EMAIL_CHARTS {
                // Replace with placeholder
                processed.replace_range(
                    range.clone(),
                    "[Chart available - view in Kyomi]",
                );
                continue;
            }
            let Some(renderer) = renderer.as_ref() else {
                processed.replace_range(
                    range.clone(),
                    "[Chart available - view in Kyomi]",
                );
                continue;
            };

            // Parse YAML → serde_json::Value for the renderer.
            let spec: serde_json::Value = match serde_yaml::from_str::<serde_json::Value>(yaml_content) {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "Failed to parse ChartML YAML for email rendering");
                    processed.replace_range(
                        range.clone(),
                        "[Chart available - view in Kyomi]",
                    );
                    continue;
                }
            };

            // Resolve data queries → inline rows (execute SQL against datasources).
            // The chart renderer only works with inline data, so we must resolve
            // any `data.datasource` + `data.query` references first.
            let resolved_spec = match chart_data_resolver::resolve_chart_data(&spec, query_ctx).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, chart_idx = idx, "Failed to resolve chart data for email — using placeholder");
                    processed.replace_range(
                        range.clone(),
                        "[Chart available - view in Kyomi]",
                    );
                    continue;
                }
            };

            // Extract a title for the alt text.
            let chart_title = resolved_spec
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Chart");

            // Render the chart to PNG (now with inline data + user palette).
            match renderer.render_chart(&resolved_spec, EMAIL_CHART_WIDTH, EMAIL_CHART_HEIGHT, Some(&user_palette), None).await {
                Ok(png_bytes) => {
                    let cid = format!(
                        "chart_{}_{}", idx, &uuid::Uuid::new_v4().to_string()[..8]
                    );
                    let img_html = format!(
                        concat!(
                            r#"<div style="margin: 20px 0; text-align: center;">"#,
                            r#"<img src="cid:{cid}" alt="{title}" "#,
                            r#"style="max-width: 100%; height: auto; border-radius: 8px; "#,
                            r#"box-shadow: 0 2px 8px rgba(0,0,0,0.1);">"#,
                            r#"<p style="color: #6b7280; font-size: 12px; margin-top: 8px;">{title}</p>"#,
                            r#"</div>"#,
                        ),
                        cid = cid,
                        title = chart_title,
                    );
                    processed.replace_range(range.clone(), &img_html);
                    images.push((cid, png_bytes));
                }
                Err(e) => {
                    warn!(error = %e, chart_idx = idx, "Failed to render chart for email");
                    processed.replace_range(
                        range.clone(),
                        "[Chart available - view in Kyomi]",
                    );
                }
            }
        }
    }

    // Strip any remaining YAML chart blocks (```yaml ... visualize: ...```)
    let processed = RE_YAML_CHART
        .replace_all(&processed, "[Chart available - view in Kyomi]")
        .into_owned();

    // Convert markdown to HTML
    let html = markdown_to_simple_html(&processed);

    (html, images)
}

// ---------------------------------------------------------------------------
// Email template building
// ---------------------------------------------------------------------------

/// Build a watch alert/report email.
///
/// The `message_html` parameter should already be processed (ChartML rendered
/// or stripped, markdown converted to HTML) via [`process_message_for_email`].
///
/// Returns `(subject, html_body)`.
/// Ports the Python `build_watch_alert` template from `email_templates/templates.py`.
#[allow(clippy::too_many_arguments)]
fn build_watch_alert_email(
    recipient_email: &str,
    watch_name: &str,
    alert_title: &str,
    message_html: &str,
    execution_id: i32,
    frontend_url: &str,
    configured_by_email: Option<&str>,
    mode: WatchMode,
) -> (String, String) {
    let is_report = mode == WatchMode::Report;
    let type_label = if is_report { "Report" } else { "Alert" };
    let type_label_lower = if is_report { "report" } else { "alert" };

    // Subject line
    let subject_text = if alert_title.is_empty() {
        format!("{type_label}: {watch_name}")
    } else {
        alert_title.to_string()
    };
    let emoji = if is_report { "📊" } else { "🔔" };
    let subject = format!("{emoji} {subject_text}");

    let view_url = format!("{frontend_url}/watches/alerts?alert={execution_id}");

    // Attribution text (matches Python build_watch_alert_with_content wording)
    let (attribution_html, footer_reason) = match configured_by_email {
        Some(configured_by) if configured_by.to_lowercase() != recipient_email.to_lowercase() => {
            (
                format!(
                    "{configured_by} configured this {type_label_lower} to be sent to you."
                ),
                format!(
                    "{configured_by} added you to the \"{watch_name}\" watch {type_label_lower}s"
                ),
            )
        }
        _ => (
            format!(
                "You configured email {type_label_lower}s for \"{watch_name}\"."
            ),
            format!(
                "you enabled email {type_label_lower}s for \"{watch_name}\""
            ),
        ),
    };

    // Accent color: green for reports, amber for alerts
    let accent_color = if is_report { "#059669" } else { "#d97706" };

    // Badge background: light green for reports, warm amber for alerts
    let badge_bg = if is_report { "#ecfdf5" } else { "#fef3c7" };

    // Title text (no emoji — emoji goes in the badge only)
    let title_text = if alert_title.is_empty() {
        format!("Watch {type_label}")
    } else {
        alert_title.to_string()
    };

    // Build HTML email — matches Python build_watch_alert_with_content + build_html_email wrapper
    let html_body = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light">
    <meta name="supported-color-schemes" content="light">
    <style>
        :root {{ color-scheme: light; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            line-height: 1.6;
            color: #374151;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #ffffff;
        }}
        .header {{
            text-align: center;
            margin-bottom: 16px;
            padding: 16px 0;
            border-bottom: 1px solid #e5e7eb;
        }}
        .logo-img {{
            height: 48px;
            width: auto;
        }}
        .content {{
            padding: 20px 0;
        }}
        h1 {{
            color: #111827;
            font-size: 24px;
            font-weight: 700;
            margin-bottom: 16px;
        }}
        h2 {{
            color: #1f2937;
            font-size: 20px;
            font-weight: 600;
            margin: 24px 0 12px 0;
        }}
        h3 {{
            color: #374151;
            font-size: 18px;
            font-weight: 600;
            margin: 20px 0 10px 0;
        }}
        p {{
            color: #4b5563;
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
            border-top: 1px solid #e5e7eb;
            text-align: center;
            color: #9ca3af;
            font-size: 12px;
        }}
        .footer a {{
            color: #6b7280;
            text-decoration: none;
        }}
        .footer a:hover {{
            text-decoration: underline;
        }}
    </style>
</head>
<body style="background-color: #ffffff !important; color: #374151;">
    <div class="header">
        <a href="https://kyomi.ai" style="text-decoration: none;">
            <img src="cid:kyomi_logo" alt="Kyomi" class="logo-img" style="height: 48px; width: auto;">
        </a>
    </div>

    <div class="content">
        <!-- Header with badge -->
        <div style="margin-bottom: 24px;">
            <span style="display: inline-block; background: {badge_bg}; color: {accent_color}; padding: 4px 12px; border-radius: 16px; font-size: 12px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.5px;">
                {emoji} {type_label}
            </span>
        </div>

        <!-- Title -->
        <h1 style="color: #111827; font-size: 24px; font-weight: 700; margin: 0 0 8px 0; line-height: 1.3;">
            {title_text}
        </h1>

        <!-- Watch name -->
        <p style="color: #6b7280; font-size: 14px; margin: 0 0 24px 0;">
            From: <strong style="color: #374151;">{watch_name}</strong>
        </p>

        <!-- Content card -->
        <div style="background: #ffffff; border: 1px solid #e5e7eb; border-radius: 12px; padding: 24px; margin: 0 0 24px 0;">
            {message_html}
        </div>

        <!-- CTA Button -->
        <div style="text-align: center; margin: 32px 0;">
            <a href="{view_url}" style="display: inline-block; background: {accent_color}; color: #ffffff; padding: 14px 32px; text-decoration: none; border-radius: 8px; font-weight: 600; font-size: 15px;">
                View Full {type_label} in Kyomi
            </a>
        </div>

        <!-- Attribution -->
        <p style="color: #9ca3af; font-size: 13px; text-align: center; margin: 24px 0 0 0;">
            {attribution_html}
        </p>
    </div>

    <div class="footer">
        <p style="margin: 0 0 8px 0;">You're receiving this because {footer_reason}.</p>
        <p style="margin: 0;">
            <a href="https://kyomi.ai/privacy">Privacy</a> &middot;
            <a href="https://kyomi.ai/terms">Terms</a> &middot;
            <a href="https://kyomi.ai" style="color: #d97706;">kyomi.ai</a>
        </p>
    </div>
</body>
</html>"#
    );

    (subject, html_body)
}

// Cached regexes for email content processing.
static RE_YAML_CHART: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?s)```yaml\n.*?visualize:.*?```").expect("valid regex"));
static RE_BOLD: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\*\*(.+?)\*\*").expect("valid regex"));
static RE_ITALIC: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\*([^*]+)\*").expect("valid regex"));
static RE_H3: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^### (.+)$").expect("valid regex"));
static RE_H2: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^## (.+)$").expect("valid regex"));
static RE_H1: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^# (.+)$").expect("valid regex"));
static RE_LIST: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)(^- .+$(\n^- .+$)*)").expect("valid regex"));
static RE_NUMBERED_LIST: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)(^\d+\. .+$(\n^\d+\. .+$)*)").expect("valid regex"));
static RE_INLINE_CODE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"`([^`]+)`").expect("valid regex"));
static RE_LINK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\[(.+?)\]\((.+?)\)").expect("valid regex"));
static RE_HR: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^---+$").expect("valid regex"));
/// Matches a markdown table: header row, separator row, and one or more data rows.
/// Allows optional trailing whitespace after `|` on each line (LLMs sometimes add it).
static RE_TABLE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)(\|[^\n]+\|[ \t]*\n\|[-:| \t]+\|[ \t]*\n(?:\|[^\n]+\|[ \t]*\n?)+)")
        .expect("valid regex")
});

/// Convert a single markdown table to an HTML table with inline styles for email.
fn markdown_table_to_html(table_text: &str) -> String {
    let lines: Vec<&str> = table_text
        .trim()
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.len() < 2 {
        return table_text.to_string();
    }

    let header_line = lines[0];
    if !header_line.starts_with('|') {
        return table_text.to_string();
    }

    // Parse header cells: split by |, skip first and last empty segments
    let headers: Vec<&str> = header_line
        .split('|')
        .skip(1)
        .collect::<Vec<_>>()
        .split_last()
        .map(|(_, rest)| rest.iter().map(|c| c.trim()).collect())
        .unwrap_or_default();

    if headers.is_empty() {
        return table_text.to_string();
    }

    // Skip separator line (line 1), parse data rows (line 2+)
    let rows: Vec<Vec<&str>> = lines[2..]
        .iter()
        .filter(|line| line.starts_with('|'))
        .map(|line| {
            let cells: Vec<&str> = line.split('|').skip(1).collect::<Vec<_>>();
            // Drop last empty segment from trailing |
            if cells.last().map(|c| c.trim().is_empty()).unwrap_or(false) {
                cells[..cells.len() - 1].iter().map(|c| c.trim()).collect()
            } else {
                cells.iter().map(|c| c.trim()).collect()
            }
        })
        .collect();

    // Build HTML table with inline styles (email compatibility).
    // IMPORTANT: No newlines in output — `markdown_to_simple_html` converts
    // newlines to `<br>` tags, which would inject whitespace inside the table.
    let mut html = String::from(
        r#"<table style="border-collapse: collapse; width: 100%; margin: 16px 0; font-size: 14px;"><thead><tr style="background-color: #f8fafc; border-bottom: 2px solid #e2e8f0;">"#,
    );

    for header in &headers {
        html.push_str(&format!(
            "<th style=\"padding: 12px 16px; text-align: left; font-weight: 600; color: #374151;\">{header}</th>"
        ));
    }

    html.push_str("</tr></thead><tbody>");

    for (i, row) in rows.iter().enumerate() {
        let bg = if i % 2 == 0 { "#ffffff" } else { "#f9fafb" };
        html.push_str(&format!(
            "<tr style=\"background-color: {bg}; border-bottom: 1px solid #e5e7eb;\">"
        ));
        for cell in row {
            html.push_str(&format!(
                "<td style=\"padding: 10px 16px; color: #4b5563;\">{cell}</td>"
            ));
        }
        html.push_str("</tr>");
    }

    html.push_str("</tbody></table>");
    html
}

/// Convert basic markdown to simple HTML suitable for email.
///
/// Handles: tables, bold, italic, headers, bullet and numbered lists,
/// inline code, links, horizontal rules, and line breaks.
fn markdown_to_simple_html(text: &str) -> String {
    // 1. Convert markdown tables to HTML first (before other processing)
    let mut result = RE_TABLE
        .replace_all(text, |caps: &regex::Captures| {
            markdown_table_to_html(&caps[1])
        })
        .into_owned();

    // Bold: **text** -> <strong>text</strong>
    // We use a placeholder to avoid the italic regex from matching the bold markers.
    result = RE_BOLD
        .replace_all(&result, "\x01STRONG_START\x01$1\x01STRONG_END\x01")
        .into_owned();

    // Italic: *text* -> <em>text</em> (safe now that bold markers are replaced)
    result = RE_ITALIC.replace_all(&result, "<em>$1</em>").into_owned();

    // Restore bold markers
    result = result.replace("\x01STRONG_START\x01", "<strong>");
    result = result.replace("\x01STRONG_END\x01", "</strong>");

    // Headers: ### -> h4, ## -> h3, # -> h2
    result = RE_H3.replace_all(&result, "<h4>$1</h4>").into_owned();
    result = RE_H2.replace_all(&result, "<h3>$1</h3>").into_owned();
    result = RE_H1.replace_all(&result, "<h2>$1</h2>").into_owned();

    // Numbered list items: wrap consecutive runs in <ol>
    result = RE_NUMBERED_LIST
        .replace_all(&result, |caps: &regex::Captures| {
            let items_str = &caps[0];
            let lis: String = items_str
                .lines()
                .map(|line| {
                    // Strip leading "N. " prefix
                    let content = line
                        .find(". ")
                        .map(|pos| &line[pos + 2..])
                        .unwrap_or(line);
                    format!("<li>{content}</li>")
                })
                .collect::<Vec<_>>()
                .join("");
            format!("<ol>{lis}</ol>")
        })
        .into_owned();

    // Bullet list items: - item -> <ul><li>item</li></ul>
    // We wrap each run of consecutive list items in a single <ul> block.
    result = RE_LIST
        .replace_all(&result, |caps: &regex::Captures| {
            let items_str = &caps[0];
            let items: Vec<&str> = items_str.lines().collect();
            let lis: String = items
                .iter()
                .map(|line| {
                    let content = line.strip_prefix("- ").unwrap_or(line);
                    format!("<li>{content}</li>")
                })
                .collect::<Vec<_>>()
                .join("");
            format!("<ul>{lis}</ul>")
        })
        .into_owned();

    // Inline code: `code` -> <code>code</code>
    result = RE_INLINE_CODE.replace_all(&result, "<code>$1</code>").into_owned();

    // Links: [text](url) -> <a href="url">text</a>
    result = RE_LINK
        .replace_all(&result, r#"<a href="$2">$1</a>"#)
        .into_owned();

    // Horizontal rules: --- -> <hr>
    result = RE_HR
        .replace_all(
            &result,
            r#"<hr style="border: none; border-top: 1px solid #e5e7eb; margin: 24px 0;">"#,
        )
        .into_owned();

    // Paragraph breaks (double newline -> </p><p>)
    result = result.replace("\n\n", "</p><p>");

    // Single newlines -> <br>
    result = result.replace('\n', "<br>");

    format!("<p>{result}</p>")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Preview truncation --

    #[test]
    fn preview_truncation_short_message() {
        let message = "Short message";
        let preview = truncate_preview(message, 200);
        assert_eq!(preview, "Short message");
    }

    #[test]
    fn preview_truncation_long_message() {
        let message = "A".repeat(300);
        let preview = truncate_preview(&message, 200);
        assert_eq!(preview.len(), 203); // 200 'A's + "..."
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn preview_truncation_unicode() {
        // Each of these is a multi-byte character
        let message = "日".repeat(250);
        let preview = truncate_preview(&message, 200);
        // Should truncate at 200 chars (not bytes), then add "..."
        assert!(preview.ends_with("..."));
        // Count characters (excluding "...")
        let content = preview.trim_end_matches("...");
        assert_eq!(content.chars().count(), 200);
    }

    #[test]
    fn preview_exact_length() {
        let message = "A".repeat(200);
        let preview = truncate_preview(&message, 200);
        assert_eq!(preview, message); // No truncation needed
    }

    // -- Email template building --

    #[test]
    fn email_alert_mode() {
        let (subject, html) = build_watch_alert_email(
            "user@example.com",
            "Revenue Monitor",
            "Revenue Down 15%",
            "Revenue dropped significantly",
            123,
            "https://app.kyomi.ai",
            None,
            WatchMode::Alert,
        );

        assert!(subject.contains("Revenue Down 15%"));
        assert!(subject.contains("🔔"));
        assert!(html.contains("Revenue Monitor"));
        assert!(html.contains("Revenue dropped significantly"));
        assert!(html.contains("View Full Alert in Kyomi"));
        assert!(html.contains("alert=123"));
    }

    #[test]
    fn email_report_mode() {
        let (subject, html) = build_watch_alert_email(
            "user@example.com",
            "Daily Report",
            "Daily Summary",
            "Sales: $100K",
            456,
            "https://app.kyomi.ai",
            None,
            WatchMode::Report,
        );

        assert!(subject.contains("📊"));
        assert!(subject.contains("Daily Summary"));
        assert!(html.contains("report"));
        assert!(html.contains("#059669")); // Green accent for reports
    }

    #[test]
    fn email_with_different_configurer() {
        let (_, html) = build_watch_alert_email(
            "recipient@example.com",
            "Watch",
            "Alert",
            "Message",
            1,
            "https://app.kyomi.ai",
            Some("admin@example.com"),
            WatchMode::Alert,
        );

        assert!(html.contains("admin@example.com configured"));
    }

    #[test]
    fn email_with_same_configurer() {
        let (_, html) = build_watch_alert_email(
            "user@example.com",
            "Watch",
            "Alert",
            "Message",
            1,
            "https://app.kyomi.ai",
            Some("user@example.com"),
            WatchMode::Alert,
        );

        assert!(html.contains("You configured"));
    }

    #[test]
    fn email_empty_title_uses_watch_name() {
        let (subject, _) = build_watch_alert_email(
            "user@example.com",
            "My Watch",
            "",
            "Message",
            1,
            "https://app.kyomi.ai",
            None,
            WatchMode::Alert,
        );

        assert!(subject.contains("Alert: My Watch"));
    }

    // -- Markdown to HTML --

    #[test]
    fn markdown_to_html_bold() {
        let result = markdown_to_simple_html("This is **bold** text");
        assert!(result.contains("<strong>bold</strong>"));
    }

    #[test]
    fn markdown_to_html_headers() {
        let result = markdown_to_simple_html("## My Header");
        assert!(result.contains("<h3>My Header</h3>"));
    }

    #[test]
    fn markdown_to_html_list_items() {
        let result = markdown_to_simple_html("- Item one\n- Item two");
        assert!(result.contains("<ul><li>Item one</li><li>Item two</li></ul>"));
    }

    #[test]
    fn markdown_to_html_inline_code() {
        let result = markdown_to_simple_html("Use `SELECT * FROM table`");
        assert!(result.contains("<code>SELECT * FROM table</code>"));
    }

    #[test]
    fn markdown_to_html_links() {
        let result = markdown_to_simple_html("[Kyomi](https://kyomi.ai)");
        assert!(result.contains(r#"<a href="https://kyomi.ai">Kyomi</a>"#));
    }

    #[test]
    fn markdown_to_html_paragraph_breaks() {
        let result = markdown_to_simple_html("First paragraph\n\nSecond paragraph");
        assert!(result.contains("</p><p>"));
    }

    // -- EmailService --

    #[test]
    fn email_service_not_configured_without_env() {
        // Clear env vars that might be set
        let service = EmailService {
            host: None,
            port: 587,
            user: None,
            password: None,
            from_email: "noreply@kyomi.ai".into(),
            from_name: "Kyomi".into(),
        };
        assert!(!service.is_configured());
    }

    #[test]
    fn email_service_configured_with_all_vars() {
        let service = EmailService {
            host: Some("smtp.example.com".into()),
            port: 587,
            user: Some("user".into()),
            password: Some("pass".into()),
            from_email: "noreply@kyomi.ai".into(),
            from_name: "Kyomi".into(),
        };
        assert!(service.is_configured());
    }

    // -- Additional markdown_to_simple_html edge cases --

    #[test]
    fn markdown_to_html_italic() {
        let result = markdown_to_simple_html("This is *italic* text");
        assert!(result.contains("<em>italic</em>"));
    }

    #[test]
    fn markdown_to_html_bold_and_italic() {
        let result = markdown_to_simple_html("This is **bold** and *italic* text");
        assert!(result.contains("<strong>bold</strong>"));
        assert!(result.contains("<em>italic</em>"));
    }

    #[test]
    fn markdown_to_html_single_newline_becomes_br() {
        let result = markdown_to_simple_html("Line one\nLine two");
        assert!(result.contains("<br>"));
    }

    #[test]
    fn markdown_to_html_h1_header() {
        let result = markdown_to_simple_html("# Top Level Header");
        assert!(result.contains("<h2>Top Level Header</h2>"));
    }

    #[test]
    fn markdown_to_html_h3_header() {
        let result = markdown_to_simple_html("### Sub Header");
        assert!(result.contains("<h4>Sub Header</h4>"));
    }

    #[test]
    fn markdown_to_html_empty_string() {
        let result = markdown_to_simple_html("");
        assert_eq!(result, "<p></p>");
    }

    #[test]
    fn markdown_to_html_plain_text_wrapped_in_p() {
        let result = markdown_to_simple_html("Just plain text");
        assert!(result.starts_with("<p>"));
        assert!(result.ends_with("</p>"));
        assert!(result.contains("Just plain text"));
    }

    // -- Markdown table conversion --

    #[test]
    fn markdown_table_basic() {
        let table = "| Name | Value |\n|------|-------|\n| Revenue | $100K |\n| Users | 500 |";
        let result = markdown_to_simple_html(table);
        assert!(result.contains("<table"));
        assert!(result.contains("<th"));
        assert!(result.contains("Name"));
        assert!(result.contains("Value"));
        assert!(result.contains("<td"));
        assert!(result.contains("$100K"));
        assert!(result.contains("500"));
    }

    #[test]
    fn markdown_table_with_surrounding_text() {
        let input = "Here are the results:\n\n| Metric | Value |\n|--------|-------|\n| Sales | $50K |\n\nAs shown above.";
        let result = markdown_to_simple_html(input);
        assert!(result.contains("<table"));
        assert!(result.contains("Sales"));
        assert!(result.contains("Here are the results"));
        assert!(result.contains("As shown above"));
    }

    #[test]
    fn markdown_table_alternating_row_colors() {
        let table = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |";
        let result = markdown_table_to_html(table);
        assert!(result.contains("#ffffff")); // Even rows
        assert!(result.contains("#f9fafb")); // Odd rows
    }

    #[test]
    fn markdown_table_header_styling() {
        let table = "| Col1 | Col2 |\n|------|------|\n| a | b |";
        let result = markdown_table_to_html(table);
        assert!(result.contains("background-color: #f8fafc")); // Header bg
        assert!(result.contains("border-bottom: 2px solid #e2e8f0")); // Header border
        assert!(result.contains("font-weight: 600")); // Header font
    }

    #[test]
    fn markdown_table_trailing_whitespace() {
        // LLMs sometimes add trailing whitespace after |
        let table = "| Name | Value |  \n|------|-------|  \n| A | B |  \n";
        let result = markdown_to_simple_html(table);
        assert!(result.contains("<table"));
        assert!(result.contains("Name"));
    }

    #[test]
    fn markdown_table_not_a_table() {
        let not_table = "Just regular text with | pipe chars";
        let result = markdown_to_simple_html(not_table);
        assert!(!result.contains("<table"));
    }

    #[test]
    fn markdown_table_no_br_tags_inside() {
        // Regression: markdown_to_simple_html was converting newlines inside
        // table HTML to <br> tags, creating huge white space in emails.
        let input = "Results:\n\n| Metric | Value |\n|--------|-------|\n| Sales | $50K |\n| Users | 100 |\n\nEnd.";
        let result = markdown_to_simple_html(input);
        // Extract the table HTML
        let table_start = result.find("<table").expect("should contain table");
        let table_end = result.find("</table>").expect("should contain /table") + 8;
        let table_html = &result[table_start..table_end];
        assert!(
            !table_html.contains("<br>"),
            "Table HTML should not contain <br> tags, got: {table_html}"
        );
    }

    // -- Numbered lists --

    #[test]
    fn markdown_numbered_list() {
        let result = markdown_to_simple_html("1. First item\n2. Second item\n3. Third item");
        assert!(result.contains("<ol>"));
        assert!(result.contains("<li>First item</li>"));
        assert!(result.contains("<li>Second item</li>"));
        assert!(result.contains("<li>Third item</li>"));
    }

    // -- Horizontal rules --

    #[test]
    fn markdown_horizontal_rule() {
        let result = markdown_to_simple_html("Before\n\n---\n\nAfter");
        assert!(result.contains("<hr"));
        assert!(result.contains("border-top: 1px solid #e5e7eb"));
    }

    // -- Additional email template edge cases --

    #[test]
    fn email_accent_color_alert_is_amber() {
        let (_, html) = build_watch_alert_email(
            "user@example.com",
            "Watch",
            "Alert",
            "Message",
            1,
            "https://app.kyomi.ai",
            None,
            WatchMode::Alert,
        );
        assert!(html.contains("#d97706")); // Amber accent border
        assert!(html.contains("#fffbeb")); // Warm amber background
    }

    #[test]
    fn email_accent_color_report_is_green() {
        let (_, html) = build_watch_alert_email(
            "user@example.com",
            "Watch",
            "Report",
            "Message",
            1,
            "https://app.kyomi.ai",
            None,
            WatchMode::Report,
        );
        assert!(html.contains("#059669")); // Green accent border
        assert!(html.contains("#ecfdf5")); // Light green background
    }

    #[test]
    fn email_design_system_compliance() {
        let (_, html) = build_watch_alert_email(
            "user@example.com",
            "Watch",
            "Alert",
            "Message",
            1,
            "https://app.kyomi.ai",
            None,
            WatchMode::Alert,
        );
        // Button uses primary color (#d97706), not dark (#111827)
        assert!(html.contains("background-color: #d97706"));
        // Header has border-bottom
        assert!(html.contains("border-bottom: 1px solid #e5e7eb"));
        // Footer has privacy and terms links
        assert!(html.contains("kyomi.ai/privacy"));
        assert!(html.contains("kyomi.ai/terms"));
    }

    #[test]
    fn email_contains_view_url_with_execution_id() {
        let (_, html) = build_watch_alert_email(
            "user@example.com",
            "Watch",
            "Alert",
            "Message",
            777,
            "https://app.kyomi.ai",
            None,
            WatchMode::Alert,
        );
        assert!(html.contains("https://app.kyomi.ai/watches/alerts?alert=777"));
    }

    // -- process_message_for_email --

    /// Build a dummy `QueryContext` for tests where chart data resolution is
    /// never actually triggered (e.g., empty renderer URL → all charts become
    /// placeholders).  The PgPool is created lazily and never connects.
    fn dummy_query_ctx() -> QueryContext {
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fake:fake@localhost/fake")
            .expect("connect_lazy should not fail");
        QueryContext {
            db: kyomi_core::DbPool::Postgres(pg),
            user_id: "test-user".into(),
            workspace_id: "test-workspace".into(),
            encryption_key: Arc::new([0u8; 32]),
            config: Arc::new(kyomi_core::Config::test_config()),
            connect_registry: None,
        }
    }

    #[tokio::test]
    async fn process_message_no_charts() {
        let message = "Revenue is down **15%** this week.";
        let ctx = dummy_query_ctx();
        // Empty renderer URL — no charts to render.
        let (html, images) = process_message_for_email(message, "", &ctx).await;
        assert!(images.is_empty());
        assert!(html.contains("<strong>15%</strong>"));
        assert!(html.contains("Revenue is down"));
    }

    #[tokio::test]
    async fn process_message_chartml_becomes_placeholder_without_renderer() {
        let message = "Revenue is down.\n```chartml\ntype: chart\nversion: 1\n```\nSee chart above.";
        let ctx = dummy_query_ctx();
        // Empty renderer URL — charts replaced with placeholders.
        let (html, images) = process_message_for_email(message, "", &ctx).await;
        assert!(images.is_empty());
        assert!(!html.contains("chartml"));
        assert!(html.contains("[Chart available - view in Kyomi]"));
        assert!(html.contains("Revenue is down"));
        assert!(html.contains("See chart above"));
    }

    #[tokio::test]
    async fn process_message_yaml_chart_becomes_placeholder() {
        let message = "Results:\n```yaml\ntype: chart\nvisualize:\n  type: bar\n```\nEnd.";
        let ctx = dummy_query_ctx();
        let (html, images) = process_message_for_email(message, "", &ctx).await;
        assert!(images.is_empty());
        assert!(!html.contains("yaml"));
        assert!(html.contains("[Chart available - view in Kyomi]"));
    }

    #[tokio::test]
    async fn process_message_preserves_normal_text() {
        let message = "No charts here, just text.";
        let ctx = dummy_query_ctx();
        let (html, images) = process_message_for_email(message, "", &ctx).await;
        assert!(images.is_empty());
        assert!(html.contains("No charts here, just text."));
    }

    #[test]
    fn email_subject_emoji_for_alert() {
        let (subject, _) = build_watch_alert_email(
            "user@example.com",
            "Watch",
            "Title",
            "msg",
            1,
            "https://app.kyomi.ai",
            None,
            WatchMode::Alert,
        );
        assert!(subject.starts_with("🔔"));
    }

    #[test]
    fn email_subject_emoji_for_report() {
        let (subject, _) = build_watch_alert_email(
            "user@example.com",
            "Watch",
            "Title",
            "msg",
            1,
            "https://app.kyomi.ai",
            None,
            WatchMode::Report,
        );
        assert!(subject.starts_with("📊"));
    }

    #[test]
    fn email_has_kyomi_branding() {
        let (_, html) = build_watch_alert_email(
            "user@example.com",
            "Watch",
            "Alert",
            "Message",
            1,
            "https://app.kyomi.ai",
            None,
            WatchMode::Alert,
        );
        assert!(html.contains("cid:kyomi_logo"));
        assert!(html.contains("alt=\"Kyomi\""));
        assert!(html.contains("https://kyomi.ai"));
        assert!(html.contains("class=\"header\""));
    }

    #[test]
    fn email_case_insensitive_configurer_match() {
        let (_, html) = build_watch_alert_email(
            "User@Example.com",
            "Watch",
            "Alert",
            "Message",
            1,
            "https://app.kyomi.ai",
            Some("user@example.com"),
            WatchMode::Alert,
        );
        // Case-insensitive: should show "You configured" since emails match
        assert!(html.contains("You configured"));
    }

    // -- EmailService partial configuration tests --

    #[test]
    fn email_service_missing_host_not_configured() {
        let service = EmailService {
            host: None,
            port: 587,
            user: Some("user".into()),
            password: Some("pass".into()),
            from_email: "noreply@kyomi.ai".into(),
            from_name: "Kyomi".into(),
        };
        assert!(!service.is_configured());
    }

    #[test]
    fn email_service_missing_user_not_configured() {
        let service = EmailService {
            host: Some("smtp.example.com".into()),
            port: 587,
            user: None,
            password: Some("pass".into()),
            from_email: "noreply@kyomi.ai".into(),
            from_name: "Kyomi".into(),
        };
        assert!(!service.is_configured());
    }

    #[test]
    fn email_service_missing_password_not_configured() {
        let service = EmailService {
            host: Some("smtp.example.com".into()),
            port: 587,
            user: Some("user".into()),
            password: None,
            from_email: "noreply@kyomi.ai".into(),
            from_name: "Kyomi".into(),
        };
        assert!(!service.is_configured());
    }

    // -- Preview truncation edge cases --

    #[test]
    fn preview_empty_string() {
        let preview = truncate_preview("", 200);
        assert_eq!(preview, "");
    }

    #[test]
    fn preview_one_char_max() {
        let preview = truncate_preview("Hello", 1);
        assert_eq!(preview, "H...");
    }

    // -- Embedded logo --

    #[test]
    fn logo_bytes_embedded_and_valid_png() {
        // PNG files start with the 8-byte signature: 137 80 78 71 13 10 26 10
        assert!(LOGO_BYTES.len() > 8, "Logo should not be empty");
        assert_eq!(&LOGO_BYTES[..4], b"\x89PNG", "Logo should be a valid PNG");
    }

    #[test]
    fn logo_cid_constant() {
        assert_eq!(LOGO_CID, "kyomi_logo");
    }

    #[test]
    fn email_chart_limits_match_python() {
        assert_eq!(MAX_EMAIL_CHARTS, 3);
    }
}
