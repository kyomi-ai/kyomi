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
use kyomi_auth::email_service::EmailService;
use kyomi_core::platform::PlatformRegistry;
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
                            "{}/inbox?alert={}",
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
                let success = send_watch_alert_emails(
                        &email_service,
                        &email_list,
                        &AlertEmailParams {
                            watch_name: &watch.name,
                            alert_title,
                            message,
                            execution_id,
                            frontend_url: &config.frontend_url,
                            creator_email: creator_email.as_deref(),
                            mode,
                        },
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
            url: format!("/inbox?alert={execution_id}"),
            icon: "/kyomi_icon_192.png".to_string(),
        };

        let http_client = kyomi_datasource_server::http_client()
            .expect("building HTTP client with user_agent should not fail");
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

struct AlertEmailParams<'a> {
    watch_name: &'a str,
    alert_title: &'a str,
    message: &'a str,
    execution_id: i32,
    frontend_url: &'a str,
    creator_email: Option<&'a str>,
    mode: WatchMode,
}

/// Send watch alert/report emails to all recipients.
///
/// Renders ChartML blocks to inline images (via chartml-rs) and attaches
/// the Kyomi logo as a CID image. Returns `true` if at least one email was
/// sent successfully.
async fn send_watch_alert_emails(
    email_service: &EmailService,
    emails: &[&str],
    params: &AlertEmailParams<'_>,
    query_ctx: &QueryContext,
) -> bool {
    if emails.is_empty() {
        return false;
    }

    // Process the message once (render charts, convert markdown → HTML).
    let (message_html, chart_images) =
        process_message_for_email(params.message, query_ctx).await;

    let mut any_success = false;

    for &email in emails {
        let (subject, html_body) = build_watch_alert_email(
            email,
            params.watch_name,
            params.alert_title,
            &message_html,
            params.execution_id,
            params.frontend_url,
            params.creator_email,
            params.mode,
        );

        if email_service
            .send_email(email, &subject, &html_body, None, None, &chart_images)
            .await
        {
            any_success = true;
        }
    }

    any_success
}

// ---------------------------------------------------------------------------
// Email message processing — ChartML → rendered CID images
// ---------------------------------------------------------------------------

/// Maximum number of charts to render for a single email.
const MAX_EMAIL_CHARTS: usize = 3;

/// Email chart render dimensions (CSS px). The `<img>` displays at this
/// size; the bitmap is rendered at [`EMAIL_CHART_DENSITY`] for sharpness.
const EMAIL_CHART_WIDTH: u32 = 600;
const EMAIL_CHART_HEIGHT: u32 = 350;

/// Render density in DPI. 144 = 2x the 72 DPI CSS-pixel baseline, so the
/// PNG has double the pixels of its display size and stays crisp on
/// high-DPI (retina) screens.
const EMAIL_CHART_DENSITY: u32 = 144;

/// Chart canvas fill for each color scheme. These MUST match the email
/// template's `.content-card` background colors (light default and the
/// `@media (prefers-color-scheme: dark)` override in
/// [`build_watch_alert_email`]) — the chart PNG sits on that card and the
/// canvas should disappear into it.
const EMAIL_CARD_LIGHT_RGB: [u8; 3] = [0xFF, 0xFF, 0xFF]; // .content-card #ffffff
const EMAIL_CARD_DARK_RGB: [u8; 3] = [0x24, 0x20, 0x1E]; // .content-card dark #24201E

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
    query_ctx: &QueryContext,
) -> (String, Vec<(String, Vec<u8>)>) {
    use crate::chartml_factory;
    use crate::tools::chart_data_resolver;
    use crate::tools::chart_palettes;

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
        // Get user's palette preference for chart rendering. The email path
        // renders both a light and a dark variant, so we materialize both
        // color lists from a single DB lookup here.
        let (light_palette, dark_palette) =
            chart_palettes::get_user_palette_light_dark(&query_ctx.db, &query_ctx.user_id).await;

        // Process blocks in reverse order so byte offsets stay valid after replacements.
        for (idx, (range, yaml_content)) in blocks.iter().enumerate().rev() {
            if idx >= MAX_EMAIL_CHARTS {
                // Replace with placeholder
                processed.replace_range(
                    range.clone(),
                    "[Chart available - view in Kyomi]",
                );
                continue;
            }

            // Parse YAML → serde_json::Value to resolve data.
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

            // Convert resolved spec to YAML for chartml-rs
            let resolved_yaml = match serde_yaml::to_string(&resolved_spec) {
                Ok(y) => y,
                Err(e) => {
                    warn!(error = %e, chart_idx = idx, "Failed to serialize spec to YAML");
                    processed.replace_range(range.clone(), "[Chart available - view in Kyomi]");
                    continue;
                }
            };

            // Render the chart to PNG via chartml-rs (Rust-native, no HTTP).
            // Two variants: light (shown by default) and dark (swapped in by
            // the template's prefers-color-scheme media query). Each canvas
            // matches the content-card surface it sits on.
            match chartml_factory::render_chart_to_png_on_surface(
                &resolved_yaml,
                EMAIL_CHART_WIDTH,
                EMAIL_CHART_HEIGHT,
                EMAIL_CHART_DENSITY,
                Some(&light_palette),
                false,
                EMAIL_CARD_LIGHT_RGB,
            ).await {
                Ok(light_png) => {
                    let uid = &uuid::Uuid::new_v4().to_string()[..8];
                    let cid_light = format!("chart_{idx}_light_{uid}");

                    // The dark variant is an enhancement — if it fails, the
                    // email still goes out with the light chart only.
                    let cid_dark = match chartml_factory::render_chart_to_png_on_surface(
                        &resolved_yaml,
                        EMAIL_CHART_WIDTH,
                        EMAIL_CHART_HEIGHT,
                        EMAIL_CHART_DENSITY,
                        Some(&dark_palette),
                        true,
                        EMAIL_CARD_DARK_RGB,
                    ).await {
                        Ok(dark_png) => {
                            let cid = format!("chart_{idx}_dark_{uid}");
                            images.push((cid.clone(), dark_png));
                            Some(cid)
                        }
                        Err(e) => {
                            warn!(error = %e, chart_idx = idx, "Failed to render dark chart variant for email — sending light only");
                            None
                        }
                    };

                    let img_html =
                        build_chart_img_html(&cid_light, cid_dark.as_deref(), chart_title);
                    processed.replace_range(range.clone(), &img_html);
                    images.push((cid_light, light_png));
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

/// Build the inline HTML for a rendered email chart.
///
/// Emits a light `<img>` (shown by default) and, when a dark render is
/// available, a hidden dark `<img>` that the template's
/// `@media (prefers-color-scheme: dark)` block swaps in (`.chart-light` /
/// `.chart-dark` rules in [`build_watch_alert_email`]). Clients without
/// media-query support only ever show the light chart — matching the rest
/// of the email, which also stays light there.
///
/// The dark variant is wrapped in `<!--[if !mso]><!-- -->` so classic
/// Outlook (Word engine), which ignores `display: none` on images, never
/// parses it. `mso-hide: all` plus `max-height: 0; overflow: hidden` cover
/// the remaining clients that ignore `display: none` alone.
///
/// The whole block stays on ONE line — [`markdown_to_simple_html`] runs
/// after injection and converts newlines to `<br>`/`</p><p>`.
///
/// `width="600"` pins the display size: the PNG is rendered at 2x density
/// ([`EMAIL_CHART_DENSITY`]) and downsampling keeps it crisp on high-DPI
/// screens.
fn build_chart_img_html(cid_light: &str, cid_dark: Option<&str>, title: &str) -> String {
    let title = escape_html(title);
    let w = EMAIL_CHART_WIDTH;
    let img_style =
        format!("width: 100%; max-width: {w}px; height: auto; border-radius: 8px;");

    let light_img = format!(
        r#"<div class="chart-light"><img src="cid:{cid_light}" alt="{title}" width="{w}" style="{img_style} box-shadow: 0 2px 8px rgba(0,0,0,0.1);"></div>"#
    );

    let dark_img = match cid_dark {
        Some(cid) => format!(
            r#"<!--[if !mso]><!--><div class="chart-dark" style="display: none; max-height: 0; overflow: hidden; mso-hide: all;"><img src="cid:{cid}" alt="{title}" width="{w}" style="{img_style}"></div><!--<![endif]-->"#
        ),
        None => String::new(),
    };

    format!(
        r#"<div style="margin: 20px 0; text-align: center;">{light_img}{dark_img}<p class="attribution" style="font-size: 12px; margin-top: 8px;">{title}</p></div>"#
    )
}

/// Minimal HTML escape for text interpolated into the email body and
/// attribute values (chart titles come from user-authored ChartML specs).
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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

    let view_url = format!("{frontend_url}/inbox?alert={execution_id}");

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
        .attribution {{ color: #9C9790; }}
        .content-card {{ background: #ffffff; }}
        @media (prefers-color-scheme: dark) {{
            body {{ background-color: #12100F !important; color: #F5F3EF !important; }}
            h1, h2, h3, h4 {{ color: #F5F3EF !important; }}
            p {{ color: #A8A29E !important; }}
            strong {{ color: #F5F3EF !important; }}
            .content-card {{ background: #24201E !important; }}
            .header {{ border-bottom-color: #2E2925 !important; }}
            .footer {{ border-top-color: #2E2925 !important; color: #78716C !important; }}
            .footer a {{ color: #A8A29E !important; }}
            th {{ color: #F5F3EF !important; background-color: #24201E !important; }}
            td {{ background-color: #1A1715 !important; color: #A8A29E !important; }}
            tr {{ border-color: #2E2925 !important; }}
            thead tr {{ background-color: #24201E !important; }}
            tbody tr {{ background-color: #1A1715 !important; }}
            .attribution {{ color: #78716C !important; }}
            .badge {{ background: #2E2925 !important; color: #F5F3EF !important; }}
            /* Swap chart variants: images can't restyle themselves, so a
               dark-canvas render replaces the light one. Built in
               build_chart_img_html; canvas colors in EMAIL_CARD_*_RGB. */
            .chart-light {{ display: none !important; }}
            .chart-dark {{ display: block !important; max-height: none !important; overflow: visible !important; }}
        }}
    </style>
</head>
<body style="background-color: #FAFAF8;">
    <div class="header">
        <a href="https://kyomi.ai" style="text-decoration: none;">
            <img src="cid:kyomi_logo" alt="Kyomi" class="logo-img" style="height: 48px; width: auto;">
        </a>
    </div>

    <div class="content">
        <!-- Header with badge -->
        <div style="margin-bottom: 24px;">
            <span class="badge" style="display: inline-block; background: {badge_bg}; color: {accent_color}; padding: 4px 12px; border-radius: 16px; font-size: 12px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.5px;">
                {emoji} {type_label}
            </span>
        </div>

        <!-- Title -->
        <h1 style="font-size: 24px; font-weight: 700; margin: 0 0 8px 0; line-height: 1.3;">
            {title_text}
        </h1>

        <!-- Watch name -->
        <p style="font-size: 14px; margin: 0 0 24px 0;">
            From: <strong>{watch_name}</strong>
        </p>

        <!-- Content card -->
        <div class="content-card" style="border: 1px solid #E8E5DE; border-radius: 12px; padding: 24px; margin: 0 0 24px 0;">
            {message_html}
        </div>

        <!-- CTA Button -->
        <div style="text-align: center; margin: 32px 0;">
            <a href="{view_url}" style="display: inline-block; background: {accent_color}; color: #ffffff; padding: 14px 32px; text-decoration: none; border-radius: 8px; font-weight: 600; font-size: 15px;">
                View Full {type_label} in Kyomi
            </a>
        </div>

        <!-- Attribution -->
        <p class="attribution" style="font-size: 13px; text-align: center; margin: 24px 0 0 0;">
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
        r#"<table style="border-collapse: collapse; width: 100%; margin: 16px 0; font-size: 14px;"><thead><tr style="background-color: #F5F3EF; border-bottom: 2px solid #E8E5DE;">"#,
    );

    for header in &headers {
        html.push_str(&format!(
            "<th style=\"padding: 12px 16px; text-align: left; font-weight: 600;\">{header}</th>"
        ));
    }

    html.push_str("</tr></thead><tbody>");

    for (i, row) in rows.iter().enumerate() {
        let bg = if i % 2 == 0 { "#FFFFFF" } else { "#F5F3EF" };
        html.push_str(&format!(
            "<tr style=\"background-color: {bg}; border-bottom: 1px solid #E8E5DE;\">"
        ));
        for cell in row {
            html.push_str(&format!(
                "<td style=\"padding: 10px 16px;\">{cell}</td>"
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
            r#"<hr style="border: none; border-top: 1px solid #E8E5DE; margin: 24px 0;">"#,
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
        assert!(result.contains("#FFFFFF")); // Even rows
        assert!(result.contains("#F5F3EF")); // Odd rows
        // color is now in the stylesheet dark-mode rule, not inline — no color: assertion
        assert!(result.contains("<td style=\"padding: 10px 16px;\"")); // Layout-only inline style
    }

    #[test]
    fn markdown_table_header_styling() {
        let table = "| Col1 | Col2 |\n|------|------|\n| a | b |";
        let result = markdown_table_to_html(table);
        assert!(result.contains("background-color: #F5F3EF")); // Header bg
        assert!(result.contains("border-bottom: 2px solid #E8E5DE")); // Header border
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
        assert!(result.contains("border-top: 1px solid #E8E5DE"));
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
        // Header has border-bottom using warm token
        assert!(html.contains("border-bottom: 1px solid #E8E5DE"));
        // Footer has privacy and terms links
        assert!(html.contains("kyomi.ai/privacy"));
        assert!(html.contains("kyomi.ai/terms"));
    }

    #[test]
    fn email_dark_mode_media_query_present() {
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
        assert!(html.contains("@media (prefers-color-scheme: dark)"));
        assert!(html.contains("background-color: #12100F !important"));
        assert!(html.contains("color: #F5F3EF !important"));
    }

    #[test]
    fn email_dark_mode_chart_swap_rules_present() {
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
        // The chart variant swap rules live inside the dark media query so
        // charts follow the same client support matrix as the rest of the
        // email's dark styling.
        let dark_block = html
            .split("@media (prefers-color-scheme: dark)")
            .nth(1)
            .expect("dark media query present");
        assert!(dark_block.contains(".chart-light { display: none !important; }"));
        assert!(dark_block.contains(".chart-dark { display: block !important;"));
    }

    #[test]
    fn chart_img_html_dual_variant() {
        let html = build_chart_img_html("chart_0_light_abc123", Some("chart_0_dark_abc123"), "Revenue");
        // Light variant visible by default, dark variant hidden until the
        // media query swaps them.
        assert!(html.contains(r#"<div class="chart-light"><img src="cid:chart_0_light_abc123""#));
        assert!(html.contains(r#"<div class="chart-dark" style="display: none; max-height: 0; overflow: hidden; mso-hide: all;"#));
        assert!(html.contains("cid:chart_0_dark_abc123"));
        // Classic Outlook (Word engine) ignores display:none on images — the
        // conditional comment hides the dark variant from it entirely.
        assert!(html.contains("<!--[if !mso]><!-->"));
        assert!(html.contains("<!--<![endif]-->"));
        // 2x-density bitmap pinned to its CSS display size.
        assert!(html.contains(r#"width="600""#));
        assert!(html.contains("max-width: 600px"));
        // markdown_to_simple_html converts newlines to <br> — the block must
        // stay on one line to survive that pass.
        assert!(!html.contains('\n'));
        assert!(html.contains(">Revenue</p>"));
    }

    #[test]
    fn chart_img_html_light_only_fallback() {
        let html = build_chart_img_html("chart_1_light_def456", None, "Orders");
        assert!(html.contains("cid:chart_1_light_def456"));
        assert!(!html.contains("chart-dark"));
        assert!(!html.contains("[if !mso]"));
    }

    #[test]
    fn chart_img_html_escapes_title() {
        let html = build_chart_img_html(
            "chart_0_light_aaa",
            None,
            r#"Q1 "Revenue" <by> Region & Channel"#,
        );
        assert!(html.contains("Q1 &quot;Revenue&quot; &lt;by&gt; Region &amp; Channel"));
        assert!(!html.contains(r#"alt="Q1 "Revenue""#));
    }

    #[test]
    fn email_color_scheme_meta_tag_includes_dark() {
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
        assert!(html.contains(r#"content="light dark""#));
        assert!(html.contains("color-scheme: light dark"));
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
        assert!(html.contains("https://app.kyomi.ai/inbox?alert=777"));
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
        let (html, images) = process_message_for_email(message, &ctx).await;
        assert!(images.is_empty());
        assert!(html.contains("<strong>15%</strong>"));
        assert!(html.contains("Revenue is down"));
    }

    #[tokio::test]
    async fn process_message_chartml_becomes_placeholder_without_renderer() {
        let message = "Revenue is down.\n```chartml\ntype: chart\nversion: 1\n```\nSee chart above.";
        let ctx = dummy_query_ctx();
        let (html, images) = process_message_for_email(message, &ctx).await;
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
        let (html, images) = process_message_for_email(message, &ctx).await;
        assert!(images.is_empty());
        assert!(!html.contains("yaml"));
        assert!(html.contains("[Chart available - view in Kyomi]"));
    }

    #[tokio::test]
    async fn process_message_preserves_normal_text() {
        let message = "No charts here, just text.";
        let ctx = dummy_query_ctx();
        let (html, images) = process_message_for_email(message, &ctx).await;
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
        assert!(html.contains("cid:kyomi_logo\""));
        assert!(html.contains("class=\"logo-img\""));
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

    #[test]
    fn email_chart_limits_match_python() {
        assert_eq!(MAX_EMAIL_CHARTS, 3);
    }
}

