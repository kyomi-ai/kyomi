// SPDX-License-Identifier: AGPL-3.0-or-later

//! Watch service — CRUD, scheduling, execution tracking, search, and alert lifecycle.
//!
//! Ports Python's `watch_service.py` into a shared service layer used by both
//! agent tools (Phase 9B-4) and REST endpoints (Phase 12).
//!
//! Key design decisions:
//! - Free-function pattern (`&DbPool` first arg) matching `dashboard_service.rs`
//! - 5-field cron validation with the `cron` crate (converts to 7-field internally)
//! - Cloud plan — uniform watch cap for all tiers (see `WATCH_LIMIT`),
//!   matching the "all capabilities, all tiers" policy in `capability.rs`.
//! - Rate limiting: max 5 manual runs per hour
//! - Soft-delete for alerts (deleted_at / deleted_by)

use chrono::{DateTime, Utc};
use cron::Schedule;
use kyomi_core::sql_compat;
use kyomi_core::{DbPool, Result};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::chat_service;
use crate::sync_log_service;
use kyomi_types::sync::{SyncActionType, entity_types};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Maximum watches per workspace, uniform across all subscription tiers.
///
/// The rest of the capability model (see `kyomi_core::capability`) was
/// flattened to "Cloud plan — all capabilities, unlimited for all tiers"
/// when manual seat management was removed. Watches intentionally keep a
/// finite cap because each watch schedules async LLM + datasource work —
/// unlimited scheduled jobs on a trial is a foot-gun. Bump this constant
/// if the operational guardrails change.
const WATCH_LIMIT: i64 = 50;

/// Maximum manual runs per hour (rate limit).
const MAX_MANUAL_RUNS_PER_HOUR: i64 = 5;

/// Human-readable descriptions for common cron patterns.
const CRON_DISPLAY: &[(&str, &str)] = &[
    ("0 * * * *", "Every hour"),
    ("0 0 * * *", "Daily at midnight UTC"),
    ("0 9 * * *", "Daily at 9:00 AM UTC"),
    ("0 9 * * 1-5", "Weekdays at 9:00 AM UTC"),
    ("0 0 * * 0", "Weekly on Sunday at midnight UTC"),
    ("0 9 * * 1", "Weekly on Monday at 9:00 AM UTC"),
    ("0 0 1 * *", "Monthly on the 1st at midnight UTC"),
    ("0 9 1 * *", "Monthly on the 1st at 9:00 AM UTC"),
    ("*/5 * * * *", "Every 5 minutes"),
    ("*/15 * * * *", "Every 15 minutes"),
    ("*/30 * * * *", "Every 30 minutes"),
    ("* * * * *", "Every minute"),
];

/// Weekday names indexed by cron day-of-week (0 = Sunday).
const WEEKDAY_NAMES: &[&str] = &["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const WEEKDAY_FULL_NAMES: &[&str] = &[
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

// ─── Helper row types for scalar queries ──────────────────────────────────

/// Helper for fetching a single string column.
#[derive(sqlx::FromRow)]
struct StringRow {
    value: String,
}


// ─── Update struct ──────────────────────────────────────────────────────────

/// Partial update payload for `update_watch`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WatchUpdate {
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub schedule: Option<String>,
    pub mode: Option<String>,
    pub enabled: Option<bool>,
    pub alert_emails: Option<String>,
    pub alert_emails_enabled: Option<bool>,
    pub queries: Option<serde_json::Value>,
    pub datasource_hints: Option<serde_json::Value>,
}

// ─── ID generation ──────────────────────────────────────────────────────────

/// Generate a watch ID in the `"watch-{uuid}"` format.
pub fn generate_watch_id() -> String {
    format!("watch-{}", uuid::Uuid::new_v4())
}

// ─── Validation helpers (pure functions, unit-testable) ─────────────────────

/// Validate watch name length (3–255 characters).
pub fn validate_watch_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.len() < 3 {
        return Err(kyomi_core::Error::BadRequest(
            "Watch name must be at least 3 characters".into(),
        ));
    }
    if trimmed.len() > 255 {
        return Err(kyomi_core::Error::BadRequest(
            "Watch name must be at most 255 characters".into(),
        ));
    }
    Ok(())
}

/// Validate watch mode is either `"alert"` or `"report"`.
pub fn validate_watch_mode(mode: &str) -> Result<()> {
    match mode {
        "alert" | "report" => Ok(()),
        _ => Err(kyomi_core::Error::BadRequest(
            "Watch mode must be 'alert' or 'report'".into(),
        )),
    }
}

/// Validate prompt has at least 10 characters.
pub fn validate_prompt_length(prompt: &str) -> Result<()> {
    if prompt.trim().len() < 10 {
        return Err(kyomi_core::Error::BadRequest(
            "Watch prompt must be at least 10 characters".into(),
        ));
    }
    Ok(())
}

// ─── Schedule parsing ───────────────────────────────────────────────────────

/// Convert a 5-field cron expression to the 7-field format required by the `cron` crate.
///
/// The `cron` crate expects: `sec min hour dom month dow year`
/// Standard cron is: `min hour dom month dow`
///
/// We prepend `"0"` (run at second 0) and append `"*"` (any year).
fn to_seven_field(five_field: &str) -> String {
    format!("0 {} *", five_field.trim())
}

/// Validate and parse a 5-field cron expression.
///
/// Returns the cleaned cron string on success.
pub fn parse_schedule(schedule: &str) -> Result<String> {
    let stripped = schedule.trim();

    // Must have exactly 5 fields
    let fields: Vec<&str> = stripped.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(kyomi_core::Error::BadRequest(
            "Invalid cron expression. Use standard cron format (5 fields): \
             'minute hour day-of-month month day-of-week'. \
             Example: '0 9 * * *' (daily at 9am UTC), '0 15 * * 1-5' (weekdays at 3pm UTC)."
                .into(),
        ));
    }

    // Only allow valid cron characters
    for ch in stripped.chars() {
        if !ch.is_ascii_digit()
            && ch != '*'
            && ch != '-'
            && ch != ','
            && ch != '/'
            && !ch.is_ascii_whitespace()
        {
            return Err(kyomi_core::Error::BadRequest(
                "Invalid cron expression. Use standard cron format (5 fields): \
                 'minute hour day-of-month month day-of-week'. \
                 Example: '0 9 * * *' (daily at 9am UTC), '0 15 * * 1-5' (weekdays at 3pm UTC)."
                    .into(),
            ));
        }
    }

    // Parse with the cron crate to validate field ranges
    let seven_field = to_seven_field(stripped);
    let schedule_parsed = Schedule::from_str(&seven_field).map_err(|e| {
        kyomi_core::Error::BadRequest(format!(
            "Invalid cron expression: {e}. Use standard cron format (5 fields): \
             'minute hour day-of-month month day-of-week'. \
             Example: '0 9 * * *' (daily at 9am UTC)."
        ))
    })?;

    // Validate minimum interval (must be >= 60 seconds between runs)
    let now = Utc::now();
    let mut upcoming = schedule_parsed.after(&now);
    if let (Some(first), Some(second)) = (upcoming.next(), upcoming.next()) {
        let interval = second.signed_duration_since(first);
        if interval.num_seconds() < 60 {
            return Err(kyomi_core::Error::BadRequest(
                "Watch schedules cannot run more frequently than once per minute.".into(),
            ));
        }
    }

    // Reconstruct the canonical 5-field form
    Ok(fields.join(" "))
}

// ─── Cron description ───────────────────────────────────────────────────────

/// Format an hour and minute into a human-readable time string.
fn format_time(hour: &str, minute: &str) -> String {
    if hour == "*" {
        if minute == "*" {
            return String::new();
        }
        if let Some(interval) = minute.strip_prefix("*/") {
            return format!("every {interval} minutes");
        }
        if minute == "0" {
            return "at the start of each hour".into();
        }
        return format!("at minute {minute}");
    }

    let hour_int: u32 = match hour.parse() {
        Ok(h) => h,
        Err(_) => return format!("at {hour}:{minute}"),
    };
    let minute_int: u32 = minute.parse().unwrap_or(0);

    let period = if hour_int < 12 { "AM" } else { "PM" };
    let display_hour = match hour_int % 12 {
        0 => 12,
        h => h,
    };

    if minute_int == 0 {
        format!("at {display_hour}:00 {period} UTC")
    } else {
        format!("at {display_hour}:{minute_int:02} {period} UTC")
    }
}

/// Format a day-of-week cron field into a human-readable string.
fn format_days_of_week(dow: &str) -> String {
    if dow == "*" {
        return String::new();
    }

    // Range like "1-5" (Mon-Fri)
    if dow.contains('-') && !dow.contains(',') {
        let parts: Vec<&str> = dow.split('-').collect();
        if parts.len() == 2
            && let (Ok(start), Ok(end)) =
                (parts[0].parse::<usize>(), parts[1].parse::<usize>())
        {
            if (start == 1 && end == 5) || (start == 0 && end == 4) {
                return "Weekdays".into();
            }
            if start < WEEKDAY_NAMES.len() && end < WEEKDAY_NAMES.len() {
                return format!("{}-{}", WEEKDAY_NAMES[start], WEEKDAY_NAMES[end]);
            }
        }
        return dow.into();
    }

    // List like "1,3,5" (Mon, Wed, Fri)
    if dow.contains(',') {
        let names: Vec<&str> = dow
            .split(',')
            .filter_map(|d| {
                d.trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| WEEKDAY_NAMES.get(i).copied())
            })
            .collect();
        if names.len() >= 2 {
            let (init, last) = names.split_at(names.len() - 1);
            return format!("{} and {}", init.join(", "), last[0]);
        }
        if names.len() == 1 {
            return names[0].into();
        }
        return dow.into();
    }

    // Single day
    if let Ok(idx) = dow.parse::<usize>()
        && let Some(name) = WEEKDAY_FULL_NAMES.get(idx)
    {
        return (*name).into();
    }
    dow.into()
}

/// Format a day-of-month cron field into a human-readable string.
fn format_day_of_month(dom: &str) -> String {
    if dom == "*" {
        return String::new();
    }

    if let Ok(day) = dom.parse::<u32>() {
        let suffix = ordinal_suffix(day);
        format!("on the {day}{suffix}")
    } else {
        format!("on day {dom}")
    }
}

/// Get ordinal suffix for a day number.
fn ordinal_suffix(n: u32) -> &'static str {
    if (10..=20).contains(&(n % 100)) {
        "th"
    } else {
        match n % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    }
}

/// Generate a human-readable description of a 5-field cron expression.
///
/// Checks a static lookup for common patterns first, then builds a dynamic
/// description from the individual fields.
pub fn describe_cron(cron_expr: &str) -> String {
    // Static lookup for common patterns
    for &(pattern, description) in CRON_DISPLAY {
        if cron_expr == pattern {
            return description.into();
        }
    }

    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    if parts.len() != 5 {
        return cron_expr.into();
    }

    let (minute, hour, day_of_month, _month, day_of_week) =
        (parts[0], parts[1], parts[2], parts[3], parts[4]);

    let time_str = format_time(hour, minute);
    let dow_str = format_days_of_week(day_of_week);
    let dom_str = format_day_of_month(day_of_month);

    // Interval pattern (every N minutes)
    if let Some(interval) = minute.strip_prefix("*/") {
        return format!("Every {interval} minutes");
    }

    // Hourly
    if hour == "*" && minute == "0" {
        if !dow_str.is_empty() {
            return format!("Every hour on {dow_str}");
        }
        return "Every hour".into();
    }

    // Weekly pattern (specific days, any date)
    if day_of_week != "*" && day_of_month == "*" {
        if !dow_str.is_empty() {
            return format!("{dow_str} {time_str}");
        }
        return format!("Weekly {time_str}");
    }

    // Monthly pattern (specific date, any day of week)
    if day_of_month != "*" && day_of_week == "*" {
        return format!("Monthly {dom_str} {time_str}");
    }

    // Daily pattern (any date, any day of week)
    if day_of_month == "*" && day_of_week == "*" {
        return format!("Daily {time_str}");
    }

    // Complex pattern — assemble what we have
    let mut desc_parts = Vec::new();
    if !dow_str.is_empty() {
        desc_parts.push(dow_str);
    }
    if !dom_str.is_empty() {
        desc_parts.push(dom_str);
    }
    if !time_str.is_empty() {
        desc_parts.push(time_str);
    }

    if desc_parts.is_empty() {
        cron_expr.into()
    } else {
        desc_parts.join(" ")
    }
}

// ─── Next-run calculation ───────────────────────────────────────────────────

/// Calculate the next fire time for a 5-field cron expression from now.
pub fn calculate_next_run(cron_expr: &str) -> Result<DateTime<Utc>> {
    let seven_field = to_seven_field(cron_expr);
    let schedule = Schedule::from_str(&seven_field).map_err(|e| {
        kyomi_core::Error::BadRequest(format!("Invalid cron expression: {e}"))
    })?;

    schedule.upcoming(Utc).next().ok_or_else(|| {
        kyomi_core::Error::BadRequest("Cron expression has no upcoming fire times".into())
    })
}

// ─── Tier limit helper ──────────────────────────────────────────────────────

/// Get the watch limit for a subscription tier.
///
/// Cloud plan — uniform cap for all tiers. The tier parameter is kept
/// for call-site compatibility with the legacy per-tier API.
fn watch_limit_for_tier(tier: kyomi_core::SubscriptionTier) -> i64 {
    let _ = tier;
    WATCH_LIMIT
}

// ─── Create watch ───────────────────────────────────────────────────────────

/// Create a new watch.
///
/// Validates name, prompt, mode, schedule. Checks tier-based limits and
/// duplicate names within the workspace. Calculates `next_run_at` from
/// the cron schedule. INSERTs and returns the new watch.
#[allow(clippy::too_many_arguments)]
pub async fn create_watch(
    db: &DbPool,
    workspace_id: &str,
    created_by: &str,
    name: &str,
    prompt: &str,
    schedule: &str,
    mode: &str,
    queries: Option<&serde_json::Value>,
    datasource_hints: Option<&serde_json::Value>,
    alert_emails: Option<&str>,
    alert_emails_enabled: bool,
) -> Result<kyomi_core::models::Watch> {
    // Validate inputs
    validate_watch_name(name)?;
    validate_prompt_length(prompt)?;
    validate_watch_mode(mode)?;
    let cron_schedule = parse_schedule(schedule)?;

    let is_pg = db.is_postgres();

    // Check tier-based limit
    let tier_sql = "SELECT subscription_tier AS value FROM workspaces WHERE workspace_id = $1";
    let tier_row: Option<StringRow> =
        kyomi_core::db_fetch_optional!(db, StringRow, tier_sql, workspace_id)
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to get workspace tier: {e}"))
            })?;
    let tier_str = tier_row
        .ok_or_else(|| kyomi_core::Error::NotFound("Workspace not found".into()))?
        .value;
    let tier: kyomi_core::SubscriptionTier = tier_str.parse().map_err(|e: String| {
        kyomi_core::Error::Internal(format!("invalid subscription tier: {e}"))
    })?;

    let limit = watch_limit_for_tier(tier);

    let count: i64 = kyomi_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM watches WHERE workspace_id = $1",
        workspace_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to count watches: {e}"))
    })?;

    if count >= limit {
        return Err(kyomi_core::Error::Forbidden(format!(
            "Watch limit reached ({limit}). Please upgrade your plan to create more watches."
        )));
    }

    // Check duplicate name (case-insensitive)
    let dup_sql =
        "SELECT watch_id AS value FROM watches WHERE workspace_id = $1 AND LOWER(name) = LOWER($2) LIMIT 1";
    let duplicate: Option<StringRow> =
        kyomi_core::db_fetch_optional!(db, StringRow, dup_sql, workspace_id, name.trim())
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to check duplicate name: {e}"))
            })?;

    if duplicate.is_some() {
        return Err(kyomi_core::Error::Conflict(format!(
            "A watch with the name '{name}' already exists in this workspace"
        )));
    }

    // Calculate next run
    let next_run_at = calculate_next_run(&cron_schedule)?;
    let watch_id = generate_watch_id();
    let now = Utc::now();

    // Serialize JSON values for binding
    let datasource_hints_str = datasource_hints
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to serialize datasource_hints: {e}")))?;
    let queries_str = queries
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to serialize queries: {e}")))?;

    let sql = format!(
        r#"
        INSERT INTO watches (
            watch_id, workspace_id, created_by, name, prompt, schedule, mode,
            datasource_hints, queries, alert_emails,
            alert_emails_enabled, enabled, next_run_at, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9::jsonb, $10, $11, {enabled}, $12, $13, $13)
        RETURNING watch_id, workspace_id, created_by, name, prompt, schedule,
                  mode, datasource_hints, queries, alert_emails,
                  alert_emails_enabled, enabled, last_run_at, last_run_status,
                  next_run_at, created_at, updated_at
        "#,
        enabled = sql_compat::bool_true(is_pg),
    );

    let watch = kyomi_core::db_fetch_one!(
        db,
        kyomi_core::models::Watch,
        &sql,
        &watch_id,
        workspace_id,
        created_by,
        name.trim(),
        prompt.trim(),
        &cron_schedule,
        mode,
        datasource_hints_str,
        queries_str,
        alert_emails,
        alert_emails_enabled,
        next_run_at,
        now
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to create watch: {e}")))?;

    tracing::info!(watch_id = %watch.watch_id, name = %watch.name, "Created watch");

    // Sync log — best-effort: log a warning and continue on failure.
    {
        let snapshot = serde_json::to_value(&watch).ok();
        if let Err(e) = sync_log_service::write_sync_entry(
            db,
            sync_log_service::SyncEntryParams {
                entity_type: entity_types::WATCH,
                entity_id: &watch.watch_id,
                workspace_id: &watch.workspace_id,
                action: SyncActionType::Insert,
                data: snapshot,
                owner_user_id: Some(&watch.created_by),
                is_workspace_visible: false,
            },
        )
        .await
        {
            tracing::warn!(error = %e, watch_id = %watch.watch_id, "Failed to write sync log entry");
        }
    }

    Ok(watch)
}

// ─── Get watch ──────────────────────────────────────────────────────────────

/// Fetch a watch by ID, scoped to both workspace and owner.
///
/// Watches have no sharing model — they are strictly private to their
/// creator (KYO-177/KYO-179) — so this filters on `created_by` in addition
/// to `workspace_id`. A workspace member who is not the owner gets `None`,
/// identical to a watch that doesn't exist, so callers must never leak
/// existence via a different error type (see `Error::Forbidden` note on
/// callers below).
pub async fn get_watch(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> Result<Option<kyomi_core::models::Watch>> {
    let sql = r#"
        SELECT watch_id, workspace_id, created_by, name, prompt, schedule,
               mode, datasource_hints, queries, alert_emails,
               alert_emails_enabled, enabled, last_run_at, last_run_status,
               next_run_at, created_at, updated_at
        FROM watches
        WHERE watch_id = $1 AND workspace_id = $2 AND created_by = $3
    "#;

    let watch = kyomi_core::db_fetch_optional!(
        db,
        kyomi_core::models::Watch,
        sql,
        watch_id,
        workspace_id,
        user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get watch: {e}")))?;

    Ok(watch)
}

// ─── List watches ───────────────────────────────────────────────────────────

/// List a user's own watches within a workspace (newest first).
///
/// Watches are strictly private to their creator — there is no sharing
/// model. Filters to `created_by = user_id` so a caller never receives
/// another member's watches.
pub async fn list_watches(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> Result<Vec<kyomi_core::models::Watch>> {
    let sql = r#"
        SELECT watch_id, workspace_id, created_by, name, prompt, schedule,
               mode, datasource_hints, queries, alert_emails,
               alert_emails_enabled, enabled, last_run_at, last_run_status,
               next_run_at, created_at, updated_at
        FROM watches
        WHERE workspace_id = $1 AND created_by = $2
        ORDER BY created_at DESC
    "#;

    let watches =
        kyomi_core::db_fetch_all!(db, kyomi_core::models::Watch, sql, workspace_id, user_id)
            .map_err(|e| kyomi_core::Error::Internal(format!("failed to list watches: {e}")))?;

    Ok(watches)
}

// ─── Sync helpers ─────────────────────────────────────────────────────────────

/// List a user's own watches within a workspace, returning the full Watch
/// records as JSON values for the sync bootstrap protocol.
///
/// Watches and their alert history are strictly private to their creator —
/// there is no sharing model. Filters to `created_by = user_id` so a
/// bootstrapping client never receives another member's watches.
pub async fn list_watches_for_sync(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> Result<Vec<serde_json::Value>> {
    let sql = r#"
        SELECT watch_id, workspace_id, created_by, name, prompt, schedule,
               mode, datasource_hints, queries, alert_emails,
               alert_emails_enabled, enabled, last_run_at, last_run_status,
               next_run_at, created_at, updated_at
        FROM watches
        WHERE workspace_id = $1 AND created_by = $2
        ORDER BY created_at DESC
    "#;

    let watches =
        kyomi_core::db_fetch_all!(db, kyomi_core::models::Watch, sql, workspace_id, user_id)
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to list watches for sync: {e}"))
            })?;

    let values = watches
        .into_iter()
        .map(|w| serde_json::to_value(&w).unwrap_or_default())
        .collect();

    Ok(values)
}

// ─── List enabled watches ───────────────────────────────────────────────────

/// List all enabled watches across all workspaces (for the scheduler).
pub async fn list_enabled_watches(db: &DbPool) -> Result<Vec<kyomi_core::models::Watch>> {
    let is_pg = db.is_postgres();
    let sql = format!(
        r#"
        SELECT watch_id, workspace_id, created_by, name, prompt, schedule,
               mode, datasource_hints, queries, alert_emails,
               alert_emails_enabled, enabled, last_run_at, last_run_status,
               next_run_at, created_at, updated_at
        FROM watches
        WHERE enabled = {enabled}
        "#,
        enabled = sql_compat::bool_true(is_pg),
    );

    let watches = kyomi_core::db_fetch_all!(db, kyomi_core::models::Watch, &sql)
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to list enabled watches: {e}")))?;

    Ok(watches)
}

// ─── Update watch ───────────────────────────────────────────────────────────

/// Update a watch with partial fields.
///
/// Uses dynamic UPDATE with `param_idx` tracking (same pattern as
/// `chat_service.rs:670–716`). Recalculates `next_run_at` when the
/// schedule changes, and sets/clears it on enable/disable.
///
/// Owner-scoped: the initial fetch and the `UPDATE` itself both filter on
/// `created_by = user_id`. The `UPDATE`-level guard is defence-in-depth —
/// it closes the TOCTOU window between the fetch and the write, so even a
/// racing ownership change (or a bug in the fetch gate) can't let a
/// non-owner's write land.
pub async fn update_watch(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
    user_id: &str,
    updates: &WatchUpdate,
) -> Result<kyomi_core::models::Watch> {
    // Fetch current watch for schedule-dependent logic
    let current = get_watch(db, watch_id, workspace_id, user_id).await?;
    let current = current.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Watch {watch_id} not found"))
    })?;

    // Validate any provided fields
    if let Some(ref name) = updates.name {
        validate_watch_name(name)?;
    }
    if let Some(ref prompt) = updates.prompt {
        validate_prompt_length(prompt)?;
    }
    if let Some(ref mode) = updates.mode {
        validate_watch_mode(mode)?;
    }

    // Validate and parse schedule if changed
    let parsed_schedule = updates
        .schedule
        .as_deref()
        .map(parse_schedule)
        .transpose()?;

    // Compute next_run_at based on schedule and/or enabled changes
    let next_run_at: Option<Option<DateTime<Utc>>> =
        if let Some(ref sched) = parsed_schedule {
            // Schedule changed — always recalculate
            Some(Some(calculate_next_run(sched)?))
        } else if let Some(enabled) = updates.enabled {
        if enabled {
            // Re-enabling: compute from current schedule
            Some(Some(calculate_next_run(&current.schedule)?))
        } else {
            // Disabling: clear next_run_at
            Some(None)
        }
    } else {
        None // No change to next_run_at
    };

    // Build dynamic UPDATE
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx = 4u32; // $1 = watch_id, $2 = workspace_id, $3 = user_id (created_by)

    if updates.name.is_some() {
        set_parts.push(format!("name = ${param_idx}"));
        param_idx += 1;
    }
    if updates.prompt.is_some() {
        set_parts.push(format!("prompt = ${param_idx}"));
        param_idx += 1;
    }
    if parsed_schedule.is_some() {
        set_parts.push(format!("schedule = ${param_idx}"));
        param_idx += 1;
    }
    if updates.mode.is_some() {
        set_parts.push(format!("mode = ${param_idx}"));
        param_idx += 1;
    }
    if updates.enabled.is_some() {
        set_parts.push(format!("enabled = ${param_idx}"));
        param_idx += 1;
    }
    if updates.alert_emails.is_some() {
        set_parts.push(format!("alert_emails = ${param_idx}"));
        param_idx += 1;
    }
    if updates.alert_emails_enabled.is_some() {
        set_parts.push(format!("alert_emails_enabled = ${param_idx}"));
        param_idx += 1;
    }
    if updates.queries.is_some() {
        set_parts.push(format!("queries = ${param_idx}::jsonb"));
        param_idx += 1;
    }
    if updates.datasource_hints.is_some() {
        set_parts.push(format!("datasource_hints = ${param_idx}::jsonb"));
        param_idx += 1;
    }
    if next_run_at.is_some() {
        set_parts.push(format!("next_run_at = ${param_idx}"));
        param_idx += 1;
    }

    // Always update updated_at
    set_parts.push(format!("updated_at = ${param_idx}"));

    if set_parts.len() == 1 {
        // Only updated_at — no real changes, just return current
        return Ok(current);
    }

    let sql = format!(
        r#"UPDATE watches SET {} WHERE watch_id = $1 AND workspace_id = $2 AND created_by = $3
           RETURNING watch_id, workspace_id, created_by, name, prompt, schedule, mode,
                     datasource_hints, queries, alert_emails,
                     alert_emails_enabled, enabled, last_run_at, last_run_status,
                     next_run_at, created_at, updated_at"#,
        set_parts.join(", ")
    );

    let now = Utc::now();

    // Dynamic SQL with variable bind chain — need manual match dispatch
    macro_rules! bind_update_params {
        ($query:expr) => {{
            let mut q = $query;
            if let Some(ref name) = updates.name {
                q = q.bind(name.trim());
            }
            if let Some(ref prompt) = updates.prompt {
                q = q.bind(prompt.trim());
            }
            if let Some(ref sched) = parsed_schedule {
                q = q.bind(sched.as_str());
            }
            if let Some(ref mode) = updates.mode {
                q = q.bind(mode.as_str());
            }
            if let Some(enabled) = updates.enabled {
                q = q.bind(enabled);
            }
            if let Some(ref emails) = updates.alert_emails {
                let value: Option<&str> = if emails.is_empty() { None } else { Some(emails.as_str()) };
                q = q.bind(value);
            }
            if let Some(emails_enabled) = updates.alert_emails_enabled {
                q = q.bind(emails_enabled);
            }
            if let Some(ref queries_val) = updates.queries {
                let s = serde_json::to_string(queries_val).unwrap_or_default();
                q = q.bind(s);
            }
            if let Some(ref hints) = updates.datasource_hints {
                let s = serde_json::to_string(hints).unwrap_or_default();
                q = q.bind(s);
            }
            if let Some(ref nra) = next_run_at {
                q = q.bind(*nra);
            }
            q = q.bind(now);
            q
        }};
    }

    let watch = kyomi_core::db_with_pool!(db, |p| {
        let q = sqlx::query_as::<_, kyomi_core::models::Watch>(&sql)
            .bind(watch_id)
            .bind(workspace_id)
            .bind(user_id);
        bind_update_params!(q).fetch_one(p).await
    })
    .map_err(|e| match e {
        // fetch_one on a zero-row result means the created_by guard hit —
        // i.e. the caller isn't the owner. The earlier get_watch() call
        // already returned NotFound for a non-owner in the common case;
        // this only fires if ownership changed between the fetch and the
        // write (the TOCTOU window this guard exists to close).
        sqlx::Error::RowNotFound => {
            kyomi_core::Error::NotFound(format!("Watch {watch_id} not found"))
        }
        e => kyomi_core::Error::Internal(format!("failed to update watch: {e}")),
    })?;

    tracing::info!(watch_id = %watch_id, "Updated watch");

    // Sync log — best-effort: log a warning and continue on failure.
    {
        let snapshot = serde_json::to_value(&watch).ok();
        if let Err(e) = sync_log_service::write_sync_entry(
            db,
            sync_log_service::SyncEntryParams {
                entity_type: entity_types::WATCH,
                entity_id: &watch.watch_id,
                workspace_id: &watch.workspace_id,
                action: SyncActionType::Update,
                data: snapshot,
                owner_user_id: Some(&watch.created_by),
                is_workspace_visible: false,
            },
        )
        .await
        {
            tracing::warn!(error = %e, watch_id = %watch_id, "Failed to write sync log entry");
        }
    }

    Ok(watch)
}

// ─── Delete watch ───────────────────────────────────────────────────────────

/// Delete a watch by ID, scoped to both workspace and owner.
///
/// Returns the deleted watch's `created_by` (the owner) so callers can route
/// the private live-sync broadcast correctly. Watches have no sharing model —
/// only the creator may ever delete them — so the `DELETE` filters on
/// `created_by = user_id` directly (no separate ownership check needed: a
/// non-owner's delete simply matches zero rows and falls through to the
/// same `NotFound` a nonexistent watch_id would produce), and no separate
/// SELECT is needed to recover the owner: `RETURNING created_by` gets it
/// from the same statement.
pub async fn delete_watch(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> Result<String> {
    let sql = r#"
        DELETE FROM watches WHERE watch_id = $1 AND workspace_id = $2 AND created_by = $3
        RETURNING created_by AS value
    "#;

    let deleted: Option<StringRow> =
        kyomi_core::db_fetch_optional!(db, StringRow, sql, watch_id, workspace_id, user_id)
            .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete watch: {e}")))?;

    let created_by = deleted
        .ok_or_else(|| kyomi_core::Error::NotFound(format!("Watch {watch_id} not found")))?
        .value;

    tracing::info!(watch_id = %watch_id, "Deleted watch");

    // Sync log — best-effort: log a warning and continue on failure.
    if let Err(e) = sync_log_service::write_sync_entry(
        db,
        sync_log_service::SyncEntryParams {
            entity_type: entity_types::WATCH,
            entity_id: watch_id,
            workspace_id,
            action: SyncActionType::Delete,
            data: None,
            owner_user_id: Some(&created_by),
            is_workspace_visible: false,
        },
    )
    .await
    {
        tracing::warn!(error = %e, watch_id = %watch_id, "Failed to write sync log entry");
    }

    Ok(created_by)
}

// ─── Toggle watch ───────────────────────────────────────────────────────────

/// Enable or disable a watch.
pub async fn toggle_watch(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
    user_id: &str,
    enabled: bool,
) -> Result<kyomi_core::models::Watch> {
    update_watch(
        db,
        watch_id,
        workspace_id,
        user_id,
        &WatchUpdate {
            enabled: Some(enabled),
            ..Default::default()
        },
    )
    .await
}

// ─── Executions ─────────────────────────────────────────────────────────────

/// Get execution history for a watch (newest first, limited).
pub async fn get_executions(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
    user_id: &str,
    limit: u32,
) -> Result<Vec<kyomi_core::models::WatchExecution>> {
    // Verify watch belongs to workspace AND is owned by the caller — this
    // is the sole authorization gate for the unscoped `watch_id`-only query
    // below (see the comment on that query for why it doesn't need its own
    // `created_by` filter).
    let watch = get_watch(db, watch_id, workspace_id, user_id).await?;
    if watch.is_none() {
        return Ok(Vec::new());
    }

    let limit_i64 = limit as i64;
    // Deliberately keyed on `watch_id` alone (no `workspace_id`/`created_by`
    // filter here): the `get_watch` call above has already established that
    // `watch_id` belongs to `workspace_id` AND is owned by `user_id`. This
    // guard must not be removed — without it, this query would return
    // execution history for any watch_id regardless of caller.
    let sql = r#"
        SELECT id, watch_id, watch_name, mode, workspace_id, session_id,
               started_at, completed_at, status, agent_response, error_message,
               input_tokens, output_tokens, cost_estimate, execution_trace,
               alert_triggered, notification_id, read_at, deleted_at, deleted_by, created_by
        FROM watch_executions
        WHERE watch_id = $1
        ORDER BY started_at DESC
        LIMIT $2
    "#;

    let executions = kyomi_core::db_fetch_all!(
        db,
        kyomi_core::models::WatchExecution,
        sql,
        watch_id,
        limit_i64
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get executions: {e}")))?;

    Ok(executions)
}

/// Get a specific execution by ID with watch verification.
pub async fn get_execution_by_id(
    db: &DbPool,
    watch_id: &str,
    execution_id: i32,
    workspace_id: &str,
    user_id: &str,
) -> Result<Option<kyomi_core::models::WatchExecution>> {
    // Verify watch belongs to workspace AND is owned by the caller — this
    // is the sole authorization gate for the unscoped `watch_id`-only query
    // below (see the comment on that query for why it doesn't need its own
    // `created_by` filter).
    let watch = get_watch(db, watch_id, workspace_id, user_id).await?;
    if watch.is_none() {
        return Ok(None);
    }

    // Deliberately keyed on `id`/`watch_id` alone (no `workspace_id`/
    // `created_by` filter here): the `get_watch` call above has already
    // established that `watch_id` belongs to `workspace_id` AND is owned by
    // `user_id`. This guard must not be removed — without it, this query
    // would return an execution for any watch_id regardless of caller.
    let sql = r#"
        SELECT id, watch_id, watch_name, mode, workspace_id, session_id,
               started_at, completed_at, status, agent_response, error_message,
               input_tokens, output_tokens, cost_estimate, execution_trace,
               alert_triggered, notification_id, read_at, deleted_at, deleted_by, created_by
        FROM watch_executions
        WHERE id = $1 AND watch_id = $2
    "#;

    let execution = kyomi_core::db_fetch_optional!(
        db,
        kyomi_core::models::WatchExecution,
        sql,
        execution_id,
        watch_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get execution: {e}")))?;

    Ok(execution)
}

/// Get a specific execution by ID only (works even if watch is deleted).
///
/// Owner-scoped directly on `watch_executions.created_by` (KYO-179 review
/// gap). This function exists specifically so callers can look up an
/// execution/alert *without* going through `get_watch` first — its whole
/// purpose is to work after the parent watch has been deleted, which is
/// exactly why `watch_executions.created_by` was denormalized onto this
/// table in `20260725000000_add_created_by_to_watch_executions.sql`: once
/// `watch_id` goes `NULL` (`ON DELETE SET NULL`), there is no `watches` row
/// left to join back to for an ownership check. `created_by` is populated
/// for every execution at creation time from the owning watch
/// (`execute_watch` passes `watch.created_by` into `create_execution`), and
/// is backfilled for pre-existing rows by that same migration, so filtering
/// on it directly is both correct and matches the pattern already used by
/// `list_alerts` (`we.workspace_id = $1 AND we.created_by = $2`) and by
/// `get_execution_by_id` (which gates via `get_watch`, itself filtered on
/// `created_by`). `execution_id` is a sequential integer and therefore
/// trivially enumerable — a non-owner must get `None`, never a `Forbidden`
/// that would confirm the ID exists.
pub async fn get_execution_by_id_only(
    db: &DbPool,
    execution_id: i32,
    workspace_id: &str,
    user_id: &str,
) -> Result<Option<kyomi_core::models::WatchExecution>> {
    let sql = r#"
        SELECT id, watch_id, watch_name, mode, workspace_id, session_id,
               started_at, completed_at, status, agent_response, error_message,
               input_tokens, output_tokens, cost_estimate, execution_trace,
               alert_triggered, notification_id, read_at, deleted_at, deleted_by, created_by
        FROM watch_executions
        WHERE id = $1 AND workspace_id = $2 AND created_by = $3
    "#;

    let execution = kyomi_core::db_fetch_optional!(
        db,
        kyomi_core::models::WatchExecution,
        sql,
        execution_id,
        workspace_id,
        user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get execution: {e}")))?;

    Ok(execution)
}

/// Create a new execution record (called when a watch starts running).
pub async fn create_execution(
    db: &DbPool,
    watch_id: &str,
    watch_name: &str,
    workspace_id: &str,
    mode: kyomi_core::WatchMode,
    created_by: &str,
) -> Result<kyomi_core::models::WatchExecution> {
    let is_pg = db.is_postgres();
    let mode_str = mode.as_ref();

    let sql = format!(
        r#"
        INSERT INTO watch_executions (
            watch_id, watch_name, mode, workspace_id, status,
            started_at, alert_triggered, created_by
        )
        VALUES ($1, $2, $3, $4, 'running', {now}, {false_val}, $5)
        RETURNING id, watch_id, watch_name, mode, workspace_id, session_id,
                  started_at, completed_at, status, agent_response, error_message,
                  input_tokens, output_tokens, cost_estimate, execution_trace,
                  alert_triggered, notification_id, read_at, deleted_at, deleted_by, created_by
        "#,
        now = sql_compat::now(is_pg),
        false_val = sql_compat::bool_false(is_pg),
    );

    let execution = kyomi_core::db_fetch_one!(
        db,
        kyomi_core::models::WatchExecution,
        &sql,
        watch_id,
        watch_name,
        mode_str,
        workspace_id,
        created_by
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to create execution: {e}")))?;

    tracing::info!(
        execution_id = execution.id,
        watch_id = %watch_id,
        "Created watch execution"
    );
    Ok(execution)
}

/// Complete an execution record with results.
#[allow(clippy::too_many_arguments)]
pub async fn complete_execution(
    db: &DbPool,
    execution_id: i32,
    status: kyomi_core::WatchExecutionStatus,
    agent_response: Option<&str>,
    error_message: Option<&str>,
    input_tokens: i32,
    output_tokens: i32,
    cost_estimate: Option<f64>,
    alert_triggered: bool,
    notification_id: Option<&str>,
    execution_trace: Option<&serde_json::Value>,
) -> Result<kyomi_core::models::WatchExecution> {
    let is_pg = db.is_postgres();
    let status_str = status.as_ref();

    let trace_str = execution_trace
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to serialize execution_trace: {e}")))?;

    // Postgres needs explicit TEXT→JSONB cast; SQLite stores JSON as TEXT natively
    let trace_cast = if is_pg { "::jsonb" } else { "" };
    let sql = format!(
        r#"
        UPDATE watch_executions
        SET completed_at = {now},
            status = $2,
            agent_response = $3,
            error_message = $4,
            input_tokens = $5,
            output_tokens = $6,
            cost_estimate = $7,
            alert_triggered = $8,
            notification_id = $9,
            execution_trace = $10{trace_cast}
        WHERE id = $1
        RETURNING id, watch_id, watch_name, mode, workspace_id, session_id,
                  started_at, completed_at, status, agent_response, error_message,
                  input_tokens, output_tokens, cost_estimate, execution_trace,
                  alert_triggered, notification_id, read_at, deleted_at, deleted_by, created_by
        "#,
        now = sql_compat::now(is_pg),
    );

    let execution = kyomi_core::db_fetch_one!(
        db,
        kyomi_core::models::WatchExecution,
        &sql,
        execution_id,
        status_str,
        agent_response,
        error_message,
        input_tokens,
        output_tokens,
        cost_estimate,
        alert_triggered,
        notification_id,
        trace_str
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to complete execution: {e}"))
    })?;

    tracing::info!(
        execution_id = execution_id,
        status = %status,
        "Completed watch execution"
    );
    Ok(execution)
}

/// Minimal fallback to mark an execution as failed when `complete_execution` itself errors.
///
/// Only touches `status`, `error_message`, and `completed_at` — avoids columns with
/// potential type mismatches (e.g. `execution_trace` jsonb) so this always succeeds.
pub async fn fail_execution_minimal(
    db: &DbPool,
    execution_id: i32,
    error_message: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let now_fn = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE watch_executions \
         SET status = 'error', \
             error_message = $2, \
             completed_at = {now_fn} \
         WHERE id = $1"
    );
    kyomi_core::db_execute!(db, &sql, execution_id, error_message)
        .map_err(|e| kyomi_core::Error::Internal(format!("minimal execution update failed: {e}")))?;
    Ok(())
}

// ─── Rate limiting ──────────────────────────────────────────────────────────

/// Check if a watch can be manually executed now.
///
/// Returns `(can_run, reason)`. If `can_run` is false, `reason` explains why.
///
/// `get_watch` is now owner-scoped (KYO-179), so this doubles as the
/// authorization gate for manual runs — a non-owner gets "Watch not found",
/// same as a nonexistent watch_id.
pub async fn can_run_watch_now(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> Result<(bool, String)> {
    let is_pg = db.is_postgres();

    // Verify watch exists and is owned by the caller
    let watch = get_watch(db, watch_id, workspace_id, user_id).await?;
    if watch.is_none() {
        return Ok((false, "Watch not found".into()));
    }

    // Check for running executions. If any have been running longer than
    // 5 minutes, mark them as failed (they likely crashed or timed out)
    // so the watch isn't permanently stuck.
    let stale_cutoff = Utc::now() - chrono::Duration::minutes(5);
    let stale_count: i64 = kyomi_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM watch_executions WHERE watch_id = $1 AND status = 'running' AND started_at < $2",
        watch_id,
        stale_cutoff
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to check stale executions: {e}"))
    })?;

    if stale_count > 0 {
        let expire_sql = format!(
            r#"UPDATE watch_executions
               SET status = 'error', completed_at = {now},
                   error_message = 'Execution timed out after 5 minutes'
               WHERE watch_id = $1 AND status = 'running' AND started_at < $2"#,
            now = sql_compat::now(is_pg),
        );

        kyomi_core::db_execute!(db, &expire_sql, watch_id, stale_cutoff)
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to expire stale executions: {e}"))
            })?;

        tracing::warn!(
            watch_id = %watch_id,
            stale_count = stale_count,
            "Expired stale watch executions stuck in 'running' state"
        );
    }

    // After expiring stale ones, check if any are genuinely still running
    let running_count: i64 = kyomi_core::db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM watch_executions WHERE watch_id = $1 AND status = 'running' LIMIT 1",
        watch_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to check running executions: {e}"))
    })?;

    if running_count > 0 {
        return Ok((false, "Watch is already running".into()));
    }

    // Rate limit: max 5 manual runs per hour (skip in dev mode)
    let is_dev = std::env::var("FRONTEND_URL")
        .unwrap_or_default()
        .contains("localhost");

    if !is_dev {
        let one_hour_ago = Utc::now() - chrono::Duration::hours(1);
        let recent_runs: i64 = kyomi_core::db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM watch_executions WHERE watch_id = $1 AND started_at >= $2",
            watch_id,
            one_hour_ago
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to count recent runs: {e}"))
        })?;

        if recent_runs >= MAX_MANUAL_RUNS_PER_HOUR {
            return Ok((
                false,
                "Rate limit exceeded. Maximum 5 runs per hour.".into(),
            ));
        }
    }

    Ok((true, "OK".into()))
}

// ─── Run status update ──────────────────────────────────────────────────────

/// Update watch after an execution (called by the scheduler).
///
/// When `next_run_at` is `None`, the existing `next_run_at` value is
/// preserved (the scheduler's CAS update already set the correct next
/// run time before spawning the execution).
pub async fn update_watch_run_status(
    db: &DbPool,
    watch_id: &str,
    status: &str,
    next_run_at: Option<DateTime<Utc>>,
) -> Result<()> {
    let is_pg = db.is_postgres();

    match next_run_at {
        Some(nra) => {
            let sql = format!(
                r#"
                UPDATE watches
                SET last_run_at = {now},
                    last_run_status = $2,
                    next_run_at = $3
                WHERE watch_id = $1
                "#,
                now = sql_compat::now(is_pg),
            );
            kyomi_core::db_execute!(db, &sql, watch_id, status, nra)
        }
        None => {
            // Don't touch next_run_at — scheduler's CAS already set it
            let sql = format!(
                r#"
                UPDATE watches
                SET last_run_at = {now},
                    last_run_status = $2
                WHERE watch_id = $1
                "#,
                now = sql_compat::now(is_pg),
            );
            kyomi_core::db_execute!(db, &sql, watch_id, status)
        }
    }
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to update watch run status: {e}"))
    })?;

    Ok(())
}

// ─── Search watches ─────────────────────────────────────────────────────────

/// Search a user's own watches within a workspace by name and prompt.
///
/// Watches are strictly private to their creator — there is no sharing
/// model. Filters to `created_by = user_id` so a caller never receives
/// another member's watches. If `query` is `None` or empty, returns all of
/// the caller's watches sorted by `created_at DESC`. Otherwise, performs
/// ILIKE search on `name` and `prompt`.
pub async fn search_watches(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    query: Option<&str>,
    limit: i64,
) -> Result<Vec<kyomi_core::models::Watch>> {
    let is_pg = db.is_postgres();
    let has_query = query.is_some_and(|q| !q.trim().is_empty());

    let sql = if has_query {
        format!(
            r#"
            SELECT watch_id, workspace_id, created_by, name, prompt, schedule, mode,
                   datasource_hints, queries, alert_emails,
                   alert_emails_enabled, enabled, last_run_at, last_run_status,
                   next_run_at, created_at, updated_at
            FROM watches
            WHERE workspace_id = $1
              AND created_by = $3
              AND ({name_like} OR {prompt_like})
            ORDER BY created_at DESC
            LIMIT $2
            "#,
            name_like = sql_compat::ilike(is_pg, "name", "'%' || $4 || '%'"),
            prompt_like = sql_compat::ilike(is_pg, "prompt", "'%' || $4 || '%'"),
        )
    } else {
        r#"
        SELECT watch_id, workspace_id, created_by, name, prompt, schedule, mode,
               datasource_hints, queries, alert_emails,
               alert_emails_enabled, enabled, last_run_at, last_run_status,
               next_run_at, created_at, updated_at
        FROM watches
        WHERE workspace_id = $1
          AND created_by = $3
        ORDER BY created_at DESC
        LIMIT $2
        "#
        .to_string()
    };

    // Dynamic SQL — bind chain varies based on has_query. Placeholder order
    // is fixed across both branches: $1=workspace_id, $2=limit,
    // $3=user_id, and (has_query only) $4=query.
    let watches = kyomi_core::db_with_pool!(db, |p| {
        let mut q = sqlx::query_as::<_, kyomi_core::models::Watch>(&sql)
            .bind(workspace_id)
            .bind(limit)
            .bind(user_id);
        if let (true, Some(query_str)) = (has_query, query) {
            q = q.bind(query_str.trim());
        }
        q.fetch_all(p).await
    })
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to search watches: {e}"))
    })?;

    Ok(watches)
}

// ─── Unread alerts count ────────────────────────────────────────────────────

/// Count unread, non-deleted alerts for a workspace (for sidebar badge).
///
/// Filters directly on `watch_executions.created_by` — a denormalized
/// snapshot of the parent watch's owner (see `watch_name`/`mode`/
/// `workspace_id` for the same pattern). `watch_id` is `ON DELETE SET NULL`,
/// so a join back to `watches` would silently drop alerts once the parent
/// watch is deleted. Watches are strictly private, no admin/owner bypass.
pub async fn get_unread_alerts_count(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> Result<i64> {
    let is_pg = db.is_postgres();
    let sql = format!(
        r#"
        SELECT COUNT(*)
        FROM watch_executions we
        WHERE we.workspace_id = $1
          AND we.created_by = $2
          AND we.alert_triggered = {true_val}
          AND we.read_at IS NULL
          AND we.deleted_at IS NULL
        "#,
        true_val = sql_compat::bool_true(is_pg),
    );

    let count: i64 = kyomi_core::db_fetch_scalar!(db, i64, &sql, workspace_id, user_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to count unread alerts: {e}"))
        })?;

    Ok(count)
}

// ─── Alerts history ────────────────────────────────────────────────────────

/// Get alerts history (paginated, with total count).
///
/// Filters to alert_triggered=true executions owned by `user_id` via the
/// denormalized `watch_executions.created_by` snapshot column — not a join
/// to `watches`, since `watch_id` is `ON DELETE SET NULL` and a join would
/// silently drop alerts once the parent watch is deleted. Alerts are
/// strictly private to whoever owns the watch. Optionally filters by
/// watch_id. Returns `(executions, total_count)` for pagination.
pub async fn get_alerts_history(
    db: &DbPool,
    workspace_id: &str,
    watch_id: Option<&str>,
    limit: i64,
    offset: i64,
    include_deleted: bool,
    user_id: &str,
) -> Result<(Vec<kyomi_core::models::WatchExecution>, i64)> {
    let is_pg = db.is_postgres();
    let true_val = sql_compat::bool_true(is_pg);

    let deleted_filter = if include_deleted {
        ""
    } else {
        "AND we.deleted_at IS NULL"
    };

    // COUNT query — $1 (workspace_id), $2 (user_id), optionally $3 (watch_id)
    let count_watch_filter = if watch_id.is_some() {
        "AND we.watch_id = $3"
    } else {
        ""
    };

    let count_sql = format!(
        r#"
        SELECT COUNT(*)
        FROM watch_executions we
        WHERE we.workspace_id = $1
          AND we.created_by = $2
          AND we.alert_triggered = {true_val}
          {deleted_filter}
          {count_watch_filter}
        "#
    );

    // Dynamic SQL — bind chain varies based on watch_id
    let total_count: i64 = kyomi_core::db_with_pool!(db, |p| {
        if let Some(wid) = watch_id {
            sqlx::query_scalar(&count_sql)
                .bind(workspace_id)
                .bind(user_id)
                .bind(wid)
                .fetch_one(p)
                .await
        } else {
            sqlx::query_scalar(&count_sql)
                .bind(workspace_id)
                .bind(user_id)
                .fetch_one(p)
                .await
        }
    })
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to count alerts: {e}")))?;

    // SELECT query — $1 (workspace_id), $2 (user_id), $3 (limit), $4 (offset),
    // optionally $5 (watch_id)
    let select_watch_filter = if watch_id.is_some() {
        "AND we.watch_id = $5"
    } else {
        ""
    };

    let select_sql = format!(
        r#"
        SELECT we.id, we.watch_id, we.watch_name, we.mode, we.workspace_id, we.session_id,
               we.started_at, we.completed_at, we.status, we.agent_response, we.error_message,
               we.input_tokens, we.output_tokens, we.cost_estimate, we.execution_trace,
               we.alert_triggered, we.notification_id, we.read_at, we.deleted_at, we.deleted_by,
               we.created_by
        FROM watch_executions we
        WHERE we.workspace_id = $1
          AND we.created_by = $2
          AND we.alert_triggered = {true_val}
          {deleted_filter}
          {select_watch_filter}
        ORDER BY we.started_at DESC
        LIMIT $3 OFFSET $4
        "#
    );

    // Dynamic SQL — bind chain varies based on watch_id
    let executions = kyomi_core::db_with_pool!(db, |p| {
        if let Some(wid) = watch_id {
            sqlx::query_as::<_, kyomi_core::models::WatchExecution>(&select_sql)
                .bind(workspace_id)
                .bind(user_id)
                .bind(limit)
                .bind(offset)
                .bind(wid)
                .fetch_all(p)
                .await
        } else {
            sqlx::query_as::<_, kyomi_core::models::WatchExecution>(&select_sql)
                .bind(workspace_id)
                .bind(user_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(p)
                .await
        }
    })
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch alerts: {e}")))?;

    Ok((executions, total_count))
}

// ─── Alert lifecycle ────────────────────────────────────────────────────────

/// Mark an alert as read (set `read_at` to now).
///
/// Owner-scoped on `watch_executions.created_by` (KYO-204) — the same
/// denormalized-column pattern used by `get_alerts_history` /
/// `get_execution_by_id_only`. A non-owner's call zero-rows-affects
/// silently rather than erroring: this matches the pre-existing
/// already-read no-op behaviour and is the enumeration-safe outcome (an
/// error here would double as an oracle for which `execution_id`s exist).
pub async fn mark_alert_read(
    db: &DbPool,
    execution_id: i32,
    workspace_id: &str,
    user_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let sql = format!(
        r#"
        UPDATE watch_executions
        SET read_at = {now}
        WHERE id = $1
          AND workspace_id = $2
          AND created_by = $3
          AND alert_triggered = {true_val}
          AND read_at IS NULL
        "#,
        now = sql_compat::now(is_pg),
        true_val = sql_compat::bool_true(is_pg),
    );

    let result = kyomi_core::db_execute!(db, &sql, execution_id, workspace_id, user_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to mark alert read: {e}"))
        })?;

    if result.rows_affected() > 0 {
        tracing::info!(execution_id, "Alert marked as read");
    }
    Ok(())
}

/// Mark an alert as unread (clear `read_at`).
///
/// Owner-scoped on `watch_executions.created_by` (KYO-204) — see
/// `mark_alert_read` doc comment for the zero-rows-affected rationale.
pub async fn mark_alert_unread(
    db: &DbPool,
    execution_id: i32,
    workspace_id: &str,
    user_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let sql = format!(
        r#"
        UPDATE watch_executions
        SET read_at = NULL
        WHERE id = $1
          AND workspace_id = $2
          AND created_by = $3
          AND alert_triggered = {true_val}
          AND read_at IS NOT NULL
        "#,
        true_val = sql_compat::bool_true(is_pg),
    );

    let result = kyomi_core::db_execute!(db, &sql, execution_id, workspace_id, user_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to mark alert unread: {e}"))
        })?;

    if result.rows_affected() > 0 {
        tracing::info!(execution_id, "Alert marked as unread");
    }
    Ok(())
}

/// Soft-delete an alert (set `deleted_at` and `deleted_by`).
///
/// Owner-scoped on `watch_executions.created_by` (KYO-204). Previously
/// `user_id` was bound only for the `deleted_by` audit column and did not
/// gate the `WHERE` clause, so any workspace member could soft-delete
/// another member's alert by guessing its (sequential) `execution_id`.
/// Zero rows affected still returns `NotFound`, matching the pre-existing
/// contract — a non-owner now lands there naturally, which is exactly
/// right (never `Forbidden`, which would confirm the row exists).
pub async fn delete_alert(
    db: &DbPool,
    execution_id: i32,
    workspace_id: &str,
    user_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let sql = format!(
        r#"
        UPDATE watch_executions
        SET deleted_at = {now}, deleted_by = $3
        WHERE id = $1
          AND workspace_id = $2
          AND created_by = $3
          AND alert_triggered = {true_val}
        "#,
        now = sql_compat::now(is_pg),
        true_val = sql_compat::bool_true(is_pg),
    );

    let result = kyomi_core::db_execute!(db, &sql, execution_id, workspace_id, user_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to delete alert: {e}"))
        })?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::NotFound(format!(
            "Alert execution {execution_id} not found"
        )));
    }

    tracing::info!(execution_id, user_id = %user_id, "Alert deleted");
    Ok(())
}

/// Restore a soft-deleted alert (clear `deleted_at` and `deleted_by`).
///
/// Owner-scoped on `watch_executions.created_by` (KYO-204) — see
/// `delete_alert` doc comment for the `NotFound`-on-zero-rows rationale.
pub async fn restore_alert(
    db: &DbPool,
    execution_id: i32,
    workspace_id: &str,
    user_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let sql = format!(
        r#"
        UPDATE watch_executions
        SET deleted_at = NULL, deleted_by = NULL
        WHERE id = $1
          AND workspace_id = $2
          AND created_by = $3
          AND alert_triggered = {true_val}
        "#,
        true_val = sql_compat::bool_true(is_pg),
    );

    let result = kyomi_core::db_execute!(db, &sql, execution_id, workspace_id, user_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to restore alert: {e}"))
        })?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::NotFound(format!(
            "Alert execution {execution_id} not found"
        )));
    }

    tracing::info!(execution_id, "Alert restored");
    Ok(())
}

// ─── Bulk operations ────────────────────────────────────────────────────────

/// Soft-delete multiple alerts at once. Returns the number of rows affected.
///
/// Owner-scoped on `watch_executions.created_by` (KYO-204). Previously
/// `user_id` was bound only for the `deleted_by` audit column and did not
/// gate the `WHERE` clause, so any workspace member could soft-delete
/// another member's alerts by guessing their (sequential) `execution_id`s
/// — this was in fact a more convenient attack surface than the
/// single-alert `delete_alert`, since a whole ID range could be swept in
/// one call.
///
/// The returned count reflects only rows the caller actually owns —
/// non-owned ids in the batch are silently filtered out by the `WHERE`
/// clause, not treated as an error. This is deliberate: a mixed batch is
/// the normal shape for a UI "select all" action (a user's own alerts
/// mixed with, say, a stale client-side selection), not an attack, and
/// failing the whole batch on a single non-owned id would be a worse user
/// experience for no security benefit. Callers must not treat
/// `count < execution_ids.len()` as an error condition.
pub async fn bulk_delete_alerts(
    db: &DbPool,
    execution_ids: &[i32],
    workspace_id: &str,
    user_id: &str,
) -> Result<u64> {
    if execution_ids.is_empty() {
        return Ok(0);
    }

    let is_pg = db.is_postgres();

    // Build a parameterized IN clause
    let placeholders: Vec<String> = (0..execution_ids.len())
        .map(|i| format!("${}", i + 3)) // $1 = workspace_id, $2 = user_id
        .collect();

    let sql = format!(
        r#"
        UPDATE watch_executions
        SET deleted_at = {now}, deleted_by = $2
        WHERE id IN ({ids})
          AND workspace_id = $1
          AND created_by = $2
          AND alert_triggered = {true_val}
          AND deleted_at IS NULL
        "#,
        now = sql_compat::now(is_pg),
        ids = placeholders.join(", "),
        true_val = sql_compat::bool_true(is_pg),
    );

    // Dynamic SQL with variable-length bind chain — identical for both backends.
    let count = kyomi_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql).bind(workspace_id).bind(user_id);
        for id in execution_ids {
            query = query.bind(id);
        }
        query.execute(p).await.map(|r| r.rows_affected())
    })
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to bulk delete alerts: {e}"))
    })?;

    tracing::info!(count, "Bulk deleted alerts");
    Ok(count)
}

/// Mark multiple alerts as read at once. Returns the number of rows affected.
///
/// Owner-scoped on `watch_executions.created_by` (KYO-204) — see
/// `bulk_delete_alerts` doc comment for the mixed-batch-count rationale:
/// non-owned ids are silently filtered out, not treated as an error, so
/// callers must not treat `count < execution_ids.len()` as an error
/// condition.
pub async fn bulk_mark_alerts_read(
    db: &DbPool,
    execution_ids: &[i32],
    workspace_id: &str,
    user_id: &str,
) -> Result<u64> {
    if execution_ids.is_empty() {
        return Ok(0);
    }

    let is_pg = db.is_postgres();

    let placeholders: Vec<String> = (0..execution_ids.len())
        .map(|i| format!("${}", i + 3)) // $1 = workspace_id, $2 = user_id
        .collect();

    let sql = format!(
        r#"
        UPDATE watch_executions
        SET read_at = {now}
        WHERE id IN ({ids})
          AND workspace_id = $1
          AND created_by = $2
          AND alert_triggered = {true_val}
          AND read_at IS NULL
        "#,
        now = sql_compat::now(is_pg),
        ids = placeholders.join(", "),
        true_val = sql_compat::bool_true(is_pg),
    );

    // Dynamic SQL with variable-length bind chain — identical for both backends.
    let count = kyomi_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql).bind(workspace_id).bind(user_id);
        for id in execution_ids {
            query = query.bind(id);
        }
        query.execute(p).await.map(|r| r.rows_affected())
    })
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to bulk mark alerts read: {e}"))
    })?;

    tracing::info!(count, "Bulk marked alerts as read");
    Ok(count)
}

/// Mark multiple alerts as unread at once. Returns the number of rows affected.
///
/// Owner-scoped on `watch_executions.created_by` (KYO-204) — see
/// `bulk_delete_alerts` doc comment for the mixed-batch-count rationale:
/// non-owned ids are silently filtered out, not treated as an error, so
/// callers must not treat `count < execution_ids.len()` as an error
/// condition.
pub async fn bulk_mark_alerts_unread(
    db: &DbPool,
    execution_ids: &[i32],
    workspace_id: &str,
    user_id: &str,
) -> Result<u64> {
    if execution_ids.is_empty() {
        return Ok(0);
    }

    let is_pg = db.is_postgres();

    let placeholders: Vec<String> = (0..execution_ids.len())
        .map(|i| format!("${}", i + 3)) // $1 = workspace_id, $2 = user_id
        .collect();

    let sql = format!(
        r#"
        UPDATE watch_executions
        SET read_at = NULL
        WHERE id IN ({ids})
          AND workspace_id = $1
          AND created_by = $2
          AND alert_triggered = {true_val}
          AND read_at IS NOT NULL
        "#,
        ids = placeholders.join(", "),
        true_val = sql_compat::bool_true(is_pg),
    );

    // Dynamic SQL with variable-length bind chain — identical for both backends.
    let count = kyomi_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql).bind(workspace_id).bind(user_id);
        for id in execution_ids {
            query = query.bind(id);
        }
        query.execute(p).await.map(|r| r.rows_affected())
    })
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to bulk mark alerts unread: {e}"))
    })?;

    tracing::info!(count, "Bulk marked alerts as unread");
    Ok(count)
}

// ─── Alert → Chat orchestration ───────────────────────────────────────────────

/// Create a new chat session seeded with the context and results from a watch
/// alert execution.
///
/// Extracts watch metadata, thinking events, and agent state from the execution
/// record, creates a new `"chat"` session, stores the user/assistant message
/// pair, and persists the restored agent state so the user can continue the
/// conversation where the watch left off.
///
/// Returns the new `session_id`.
pub async fn create_chat_session_from_alert(
    db: &DbPool,
    encryption_key: &[u8; 32],
    user_id: &str,
    workspace_id: &str,
    execution_id: i32,
) -> Result<String> {
    // 1. Get the execution (works even if the watch has been deleted).
    // Owner-gated on watch_executions.created_by (KYO-179) — this is the
    // sole authorization check for the whole function. Everything below
    // (watch_name, alert_title, watch_prompt, agent_response, and the
    // session_id used to pull thinking_events in step 4) is derived from
    // this `execution` row, so gating it here closes the leak for the rest
    // of the function. A non-owner's execution_id gets NotFound, never
    // Forbidden — execution_id is a sequential integer and trivially
    // enumerable, so a Forbidden would confirm the row exists.
    let execution = get_execution_by_id_only(db, execution_id, workspace_id, user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Alert not found".into()))?;

    if !execution.alert_triggered {
        return Err(kyomi_core::Error::BadRequest(
            "This execution did not trigger an alert".into(),
        ));
    }

    // 2. Extract watch context from the execution trace.
    let watch_name = execution
        .watch_name
        .as_deref()
        .unwrap_or("Deleted Watch");

    let mut watch_prompt: Option<String> = None;
    let mut alert_title: Option<String> = None;

    if let Some(obj) = execution
        .execution_trace
        .as_ref()
        .and_then(|t| t.as_object())
    {
        watch_prompt = obj
            .get("watch_prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        alert_title = obj
            .get("alert_title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    // 3. Fallback: fetch from the live watch when the trace lacks a prompt.
    if let (None, Some(wid)) = (&watch_prompt, &execution.watch_id) {
        let watch = get_watch(db, wid, workspace_id, user_id).await?;
        watch_prompt = watch.map(|w| w.prompt);
    }

    let watch_prompt = watch_prompt.unwrap_or_else(|| "(Watch has been deleted)".to_string());

    // 4. Load thinking events from the execution's session messages.
    //
    // chat_service::get_session_messages() filters only on `session_id` —
    // it has no user/owner scoping of its own. That is safe here ONLY
    // because `session_id` is read from `execution`, and `execution` was
    // already fetched through the owner-gated `get_execution_by_id_only`
    // above: a non-owner never reaches this line (the function returns
    // NotFound in step 1), so `session_id` can never belong to another
    // user's execution. If this session_id is ever sourced from anywhere
    // other than the gated `execution` row, it must get its own ownership
    // check — do not assume get_session_messages is safe on its own.
    let mut thinking_events: Option<serde_json::Value> = None;

    if let Some(ref session_id) = execution.session_id {
        match chat_service::get_session_messages(db, encryption_key, session_id, 1000).await {
            Ok(messages) => {
                for msg in &messages {
                    if msg.message_type == "assistant" && !msg.thinking_events.is_empty() {
                        thinking_events =
                            Some(serde_json::Value::Array(msg.thinking_events.clone()));
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load thinking events from session: {e}");
            }
        }
    }

    // 5. Load agent_state from the execution trace.
    let mut agent_state: Option<serde_json::Value> = None;
    if let Some(obj) = execution
        .execution_trace
        .as_ref()
        .and_then(|t| t.as_object())
    {
        agent_state = obj.get("agent_state").cloned();

        // Fallback for old executions that stored events in execution_trace.
        if thinking_events.is_none()
            && let Some(events) = obj.get("events").filter(|e| e.is_array())
        {
            thinking_events = Some(events.clone());
        }
    }

    // 6. Create the new chat session.
    let session_id = uuid::Uuid::new_v4().to_string();
    let title = format!(
        "Alert: {}",
        alert_title.as_deref().unwrap_or(watch_name)
    );

    chat_service::create_session_with_id(
        db,
        user_id,
        workspace_id,
        &session_id,
        Some(&title),
        "chat",
        None,
    )
    .await?;

    // 7. Add the "user" message representing the monitored prompt.
    let user_message = format!("Monitor: {watch_name}\n\n{watch_prompt}");
    chat_service::add_message(
        db,
        encryption_key,
        &session_id,
        "user",
        &user_message,
        None,
        None,
        None,
        Some(user_id),
        None,
        None,
        None,
    )
    .await?;

    // 8. Add the "assistant" message with the alert response and thinking events.
    let metadata = thinking_events
        .as_ref()
        .map(|events| serde_json::json!({ "thinking_events": events }));

    chat_service::add_message(
        db,
        encryption_key,
        &session_id,
        "assistant",
        execution.agent_response.as_deref().unwrap_or(""),
        metadata.as_ref(),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await?;

    // 9. Persist agent state so the user can continue where the watch left off.
    if let Some(mut state_val) = agent_state {
        // Remove the watch-specific system prompt from the carried-over agent state.
        if let Some(messages) = state_val.get_mut("messages").and_then(|m| m.as_array_mut()) {
            let is_system = messages
                .first()
                .and_then(|f| f.get("role"))
                .and_then(|r| r.as_str())
                == Some("system");

            if is_system {
                messages.remove(0);

                // Adjust compaction index after system prompt removal.
                if let Some(idx_val) = state_val
                    .get("messages_since_compaction_index")
                    .and_then(|v| v.as_i64())
                    .filter(|&v| v > 0)
                {
                    state_val["messages_since_compaction_index"] =
                        serde_json::json!(std::cmp::max(0, idx_val - 1));
                }
            }
        }

        state_val["timestamp"] = serde_json::json!(Utc::now().to_rfc3339());

        let config = serde_json::json!({ "agent_state": state_val });
        if let Err(e) = chat_service::update_session(db, &session_id, None, None, Some(&config))
            .await
        {
            tracing::error!(
                "Failed to save agent_state for session {session_id}: {e}"
            );
        }
    } else {
        // Fallback for old executions without agent_state.
        let fallback_state = serde_json::json!({
            "version": "2.0",
            "timestamp": Utc::now().to_rfc3339(),
            "messages": [
                {"role": "user", "content": user_message},
                {"role": "assistant", "content": execution.agent_response.as_deref().unwrap_or("")},
            ],
            "global_iteration": 1,
            "compacted_summary": null,
            "messages_since_compaction_index": 0,
            "last_input_tokens": 0,
            "config": {
                "max_iterations": 25,
                "temperature": 0.1,
            }
        });

        let config = serde_json::json!({ "agent_state": fallback_state });
        if let Err(e) = chat_service::update_session(db, &session_id, None, None, Some(&config))
            .await
        {
            tracing::error!(
                "Failed to save fallback agent_state for session {session_id}: {e}"
            );
        }
    }

    Ok(session_id)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── generate_watch_id ───────────────────────────────────────────────

    #[test]
    fn test_generate_watch_id() {
        let id = generate_watch_id();
        assert!(id.starts_with("watch-"), "should start with 'watch-': {id}");
        // "watch-" (6) + UUID (36) = 42
        assert_eq!(id.len(), 42, "watch ID should be 42 chars: {id}");

        // Each call produces a unique ID
        let id2 = generate_watch_id();
        assert_ne!(id, id2, "IDs should be unique");
    }

    // ── parse_schedule ──────────────────────────────────────────────────

    #[test]
    fn test_parse_schedule_valid() {
        assert_eq!(parse_schedule("0 9 * * *").unwrap(), "0 9 * * *");
        assert_eq!(parse_schedule("*/5 * * * *").unwrap(), "*/5 * * * *");
        assert_eq!(parse_schedule("0 0 1 * *").unwrap(), "0 0 1 * *");
        assert_eq!(parse_schedule("30 14 * * 1-5").unwrap(), "30 14 * * 1-5");
    }

    #[test]
    fn test_parse_schedule_trims_whitespace() {
        assert_eq!(parse_schedule("  0 9 * * *  ").unwrap(), "0 9 * * *");
    }

    #[test]
    fn test_parse_schedule_invalid_too_few_fields() {
        let result = parse_schedule("0 9 *");
        assert!(result.is_err(), "3 fields should fail");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("5 fields"), "error should mention 5 fields: {err}");
    }

    #[test]
    fn test_parse_schedule_invalid_too_many_fields() {
        let result = parse_schedule("0 9 * * * *");
        assert!(result.is_err(), "6 fields should fail");
    }

    #[test]
    fn test_parse_schedule_invalid_values() {
        // Minute 60 is out of range
        let result = parse_schedule("60 * * * *");
        assert!(result.is_err(), "minute 60 should fail");

        // Hour 25 is out of range
        let result = parse_schedule("0 25 * * *");
        assert!(result.is_err(), "hour 25 should fail");
    }

    #[test]
    fn test_parse_schedule_invalid_characters() {
        let result = parse_schedule("0 9 * * MON");
        assert!(result.is_err(), "alpha characters should fail");
    }

    // ── describe_cron ───────────────────────────────────────────────────

    #[test]
    fn test_describe_cron_common_patterns() {
        assert_eq!(describe_cron("0 9 * * *"), "Daily at 9:00 AM UTC");
        assert_eq!(describe_cron("0 * * * *"), "Every hour");
        assert_eq!(describe_cron("0 9 * * 1"), "Weekly on Monday at 9:00 AM UTC");
        assert_eq!(describe_cron("0 0 1 * *"), "Monthly on the 1st at midnight UTC");
        assert_eq!(describe_cron("*/5 * * * *"), "Every 5 minutes");
        assert_eq!(describe_cron("*/15 * * * *"), "Every 15 minutes");
    }

    #[test]
    fn test_describe_cron_complex() {
        assert_eq!(
            describe_cron("30 14 * * 1-5"),
            "Weekdays at 2:30 PM UTC"
        );
        assert_eq!(
            describe_cron("0 22 * * *"),
            "Daily at 10:00 PM UTC"
        );
        assert_eq!(
            describe_cron("0 0 15 * *"),
            "Monthly on the 15th at 12:00 AM UTC"
        );
    }

    // ── validate_watch_name ─────────────────────────────────────────────

    #[test]
    fn test_validate_watch_name_too_short() {
        assert!(validate_watch_name("ab").is_err());
        assert!(validate_watch_name("").is_err());
    }

    #[test]
    fn test_validate_watch_name_too_long() {
        let long_name = "x".repeat(256);
        assert!(validate_watch_name(&long_name).is_err());
    }

    #[test]
    fn test_validate_watch_name_valid() {
        assert!(validate_watch_name("abc").is_ok());
        assert!(validate_watch_name("Daily Sales Monitor").is_ok());
        assert!(validate_watch_name(&"x".repeat(255)).is_ok());
    }

    // ── validate_watch_mode ─────────────────────────────────────────────

    #[test]
    fn test_validate_watch_mode_valid() {
        assert!(validate_watch_mode("alert").is_ok());
        assert!(validate_watch_mode("report").is_ok());
    }

    #[test]
    fn test_validate_watch_mode_invalid() {
        assert!(validate_watch_mode("monitor").is_err());
        assert!(validate_watch_mode("").is_err());
        assert!(validate_watch_mode("Alert").is_err()); // case-sensitive
    }

    // ── validate_prompt_length ──────────────────────────────────────────

    #[test]
    fn test_validate_prompt_too_short() {
        assert!(validate_prompt_length("short").is_err());
        assert!(validate_prompt_length("123456789").is_err()); // 9 chars
    }

    #[test]
    fn test_validate_prompt_valid() {
        assert!(validate_prompt_length("1234567890").is_ok()); // exactly 10
        assert!(validate_prompt_length("Check if revenue drops more than 10%").is_ok());
    }

    // ── calculate_next_run ──────────────────────────────────────────────

    #[test]
    fn test_calculate_next_run() {
        let next = calculate_next_run("0 9 * * *").unwrap();
        assert!(next > Utc::now(), "next run should be in the future");
    }

    #[test]
    fn test_calculate_next_run_invalid() {
        let result = calculate_next_run("invalid");
        assert!(result.is_err());
    }
}

// ─── Contract tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod contract_tests {
    use super::*;

    // ── Cron edge cases ─────────────────────────────────────────────────

    #[test]
    fn parse_every_minute_schedule() {
        assert_eq!(parse_schedule("* * * * *").unwrap(), "* * * * *");
    }

    #[test]
    fn parse_yearly_schedule() {
        // 0 0 1 1 * = midnight, January 1st
        assert_eq!(parse_schedule("0 0 1 1 *").unwrap(), "0 0 1 1 *");
    }

    #[test]
    fn parse_specific_day_of_week() {
        // Every Sunday at 9am (cron library uses 1=Mon..7=Sun)
        assert_eq!(parse_schedule("0 9 * * 7").unwrap(), "0 9 * * 7");
        // Every Friday at 5pm
        assert_eq!(parse_schedule("0 17 * * 5").unwrap(), "0 17 * * 5");
    }

    #[test]
    fn parse_schedule_with_comma_list() {
        // At minute 0 and 30 of every hour
        assert_eq!(parse_schedule("0,30 * * * *").unwrap(), "0,30 * * * *");
    }

    #[test]
    fn parse_schedule_with_step_in_hour() {
        // Every 2 hours
        assert_eq!(parse_schedule("0 */2 * * *").unwrap(), "0 */2 * * *");
    }

    // ── describe_cron additional patterns ────────────────────────────────

    #[test]
    fn describe_cron_every_minute() {
        assert_eq!(describe_cron("* * * * *"), "Every minute");
    }

    #[test]
    fn describe_cron_every_30_minutes() {
        assert_eq!(describe_cron("*/30 * * * *"), "Every 30 minutes");
    }

    #[test]
    fn describe_cron_midnight() {
        assert_eq!(describe_cron("0 0 * * *"), "Daily at midnight UTC");
    }

    #[test]
    fn describe_cron_weekdays_at_3pm() {
        assert_eq!(
            describe_cron("0 15 * * 1-5"),
            "Weekdays at 3:00 PM UTC"
        );
    }

    #[test]
    fn describe_cron_sunday_midnight() {
        assert_eq!(
            describe_cron("0 0 * * 0"),
            "Weekly on Sunday at midnight UTC"
        );
    }

    #[test]
    fn describe_cron_specific_day_of_month() {
        assert_eq!(
            describe_cron("0 12 25 * *"),
            "Monthly on the 25th at 12:00 PM UTC"
        );
    }

    #[test]
    fn describe_cron_returns_raw_for_invalid() {
        assert_eq!(describe_cron("not a cron"), "not a cron");
    }

    // ── Combined validation ─────────────────────────────────────────────

    #[test]
    fn all_validations_pass_for_valid_watch() {
        assert!(validate_watch_name("Daily Sales Monitor").is_ok());
        assert!(validate_prompt_length("Check if revenue drops more than 10%").is_ok());
        assert!(validate_watch_mode("alert").is_ok());
        assert!(parse_schedule("0 9 * * *").is_ok());
    }

    #[test]
    fn all_validations_fail_for_completely_invalid_watch() {
        assert!(validate_watch_name("").is_err());
        assert!(validate_prompt_length("short").is_err());
        assert!(validate_watch_mode("invalid").is_err());
        assert!(parse_schedule("bad cron").is_err());
    }

    #[test]
    fn validate_name_boundary_values() {
        // Exactly at min boundary
        assert!(validate_watch_name("abc").is_ok());
        // Below min boundary
        assert!(validate_watch_name("ab").is_err());
        // Exactly at max boundary
        assert!(validate_watch_name(&"x".repeat(255)).is_ok());
        // Above max boundary
        assert!(validate_watch_name(&"x".repeat(256)).is_err());
    }

    #[test]
    fn validate_prompt_boundary_values() {
        // Exactly at min boundary (10 chars)
        assert!(validate_prompt_length("1234567890").is_ok());
        // Below min boundary (9 chars)
        assert!(validate_prompt_length("123456789").is_err());
    }

    #[test]
    fn validate_name_with_only_spaces_fails() {
        // "   " (3 spaces) trimmed is empty, < 3 chars
        assert!(validate_watch_name("   ").is_err());
    }

    #[test]
    fn validate_prompt_with_only_spaces_fails() {
        // 20 spaces trimmed is empty, < 10 chars
        assert!(validate_prompt_length("                    ").is_err());
    }

    // ── Watch ID generation ─────────────────────────────────────────────

    #[test]
    fn watch_id_format_is_consistent() {
        for _ in 0..10 {
            let id = generate_watch_id();
            assert!(id.starts_with("watch-"), "ID should start with 'watch-': {id}");
            assert_eq!(id.len(), 42, "watch ID should be 42 chars: {id}");
        }
    }

    #[test]
    fn watch_ids_are_unique() {
        // UUID v4 collision probability is ~1 in 2^122, so 100 iterations is safe.
        let ids: std::collections::HashSet<String> =
            (0..100).map(|_| generate_watch_id()).collect();
        assert_eq!(ids.len(), 100, "Generated 100 IDs but some were duplicates");
    }

    // ── Next run calculation ────────────────────────────────────────────

    #[test]
    fn next_run_is_always_in_the_future() {
        // The cron library returns the next fire time strictly after the current
        // second, so `next > now` is always true even at exact schedule boundaries.
        let schedules = [
            "0 9 * * *",
            "*/5 * * * *",
            "0 0 1 * *",
            "0 0 * * 7",
            "* * * * *",
        ];
        let now = Utc::now();
        for schedule in &schedules {
            let next = calculate_next_run(schedule).unwrap();
            assert!(
                next > now,
                "Next run for '{schedule}' should be in the future"
            );
        }
    }

    // ── Ordinal suffix ──────────────────────────────────────────────────

    #[test]
    fn ordinal_suffix_correctness() {
        assert_eq!(ordinal_suffix(1), "st");
        assert_eq!(ordinal_suffix(2), "nd");
        assert_eq!(ordinal_suffix(3), "rd");
        assert_eq!(ordinal_suffix(4), "th");
        assert_eq!(ordinal_suffix(11), "th");
        assert_eq!(ordinal_suffix(12), "th");
        assert_eq!(ordinal_suffix(13), "th");
        assert_eq!(ordinal_suffix(21), "st");
        assert_eq!(ordinal_suffix(22), "nd");
        assert_eq!(ordinal_suffix(23), "rd");
        assert_eq!(ordinal_suffix(31), "st");
    }

    // ── Tier limits ─────────────────────────────────────────────────────

    #[test]
    fn tier_limits_are_uniform() {
        use kyomi_core::SubscriptionTier::*;
        // Cloud plan — every tier returns the same cap.
        assert_eq!(watch_limit_for_tier(Free), WATCH_LIMIT);
        assert_eq!(watch_limit_for_tier(Starter), WATCH_LIMIT);
        assert_eq!(watch_limit_for_tier(Basic), WATCH_LIMIT);
        assert_eq!(watch_limit_for_tier(Pro), WATCH_LIMIT);
        assert_eq!(watch_limit_for_tier(Team), WATCH_LIMIT);
        assert_eq!(watch_limit_for_tier(Enterprise), WATCH_LIMIT);
        assert_eq!(watch_limit_for_tier(Cloud), WATCH_LIMIT);
    }

    // ── format_time edge cases ──────────────────────────────────────────

    #[test]
    fn format_time_noon() {
        assert_eq!(format_time("12", "0"), "at 12:00 PM UTC");
    }

    #[test]
    fn format_time_midnight() {
        assert_eq!(format_time("0", "0"), "at 12:00 AM UTC");
    }

    #[test]
    fn format_time_with_minutes() {
        assert_eq!(format_time("14", "30"), "at 2:30 PM UTC");
    }

    #[test]
    fn format_time_wildcard_hour() {
        assert_eq!(format_time("*", "0"), "at the start of each hour");
    }

    #[test]
    fn format_time_wildcard_both() {
        assert_eq!(format_time("*", "*"), "");
    }

    // ── format_days_of_week edge cases ──────────────────────────────────

    #[test]
    fn format_days_of_week_wildcard() {
        assert_eq!(format_days_of_week("*"), "");
    }

    #[test]
    fn format_days_of_week_single_day() {
        assert_eq!(format_days_of_week("0"), "Sunday");
        assert_eq!(format_days_of_week("1"), "Monday");
        assert_eq!(format_days_of_week("6"), "Saturday");
    }

    #[test]
    fn format_days_of_week_weekdays() {
        assert_eq!(format_days_of_week("1-5"), "Weekdays");
    }

    #[test]
    fn format_days_of_week_list() {
        assert_eq!(format_days_of_week("1,3,5"), "Mon, Wed and Fri");
    }
}

// ─── Watch privacy tests (KYO-177) ───────────────────────────────────────────
//
// Watches and their alert history have no sharing model — they must be
// strictly private to their creator. These are real sqlite-backed
// integration tests (not mocks) covering: sync bootstrap filtering, sync
// delta filtering, alerts-query filtering, and live-broadcast routing.

#[cfg(test)]
mod privacy_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> DbPool {
        let _ = kyomi_core::constants::load_with_fallback();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        DbPool::Sqlite(pool)
    }

    /// Seed two users ("user-a", "user-b") in one workspace ("ws-1", owned by
    /// user-a). Returns the pool with fixtures in place.
    async fn seed_workspace_with_two_users(pool: &DbPool) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };

        sqlx::query("INSERT INTO users (user_id, email) VALUES ('user-a', 'a@test.local')")
            .execute(sq)
            .await
            .expect("insert user-a");
        sqlx::query("INSERT INTO users (user_id, email) VALUES ('user-b', 'b@test.local')")
            .execute(sq)
            .await
            .expect("insert user-b");

        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
             VALUES ('ws-1', 'Shared Workspace', 'user-a')",
        )
        .execute(sq)
        .await
        .expect("insert workspace");

        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
             VALUES ('ws-1', 'user-a', 'workspace_admin', 1)",
        )
        .execute(sq)
        .await
        .expect("insert workspace_users user-a");
        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
             VALUES ('ws-1', 'user-b', 'user', 1)",
        )
        .execute(sq)
        .await
        .expect("insert workspace_users user-b");
    }

    async fn create_test_watch(pool: &DbPool, created_by: &str, name: &str) -> kyomi_core::models::Watch {
        create_watch(
            pool,
            "ws-1",
            created_by,
            name,
            "Check if revenue drops more than 10 percent",
            "0 9 * * *",
            "alert",
            None,
            None,
            None,
            false,
        )
        .await
        .expect("create watch")
    }

    /// Insert a triggered, unread watch_execution row directly (no service fn
    /// exists for this — `create_execution`/`complete_execution` don't set
    /// `alert_triggered`, so tests build the row by hand like the rest of the
    /// alerts-lifecycle tests in this crate do). Returns the new execution's
    /// `id`, needed by the alert-lifecycle (mark read/unread, delete/restore,
    /// bulk) tests to address the row.
    async fn insert_triggered_alert(pool: &DbPool, watch_id: &str, created_by: &str) -> i32 {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        let row: (i32,) = sqlx::query_as(
            "INSERT INTO watch_executions \
             (watch_id, workspace_id, status, alert_triggered, started_at, completed_at, created_by) \
             VALUES ($1, 'ws-1', 'success', 1, datetime('now'), datetime('now'), $2) \
             RETURNING id",
        )
        .bind(watch_id)
        .bind(created_by)
        .fetch_one(sq)
        .await
        .expect("insert triggered alert");
        row.0
    }

    // ── list_watches_for_sync ────────────────────────────────────────────

    #[tokio::test]
    async fn list_watches_for_sync_excludes_other_users_watches() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Watch").await;
        let wb = create_test_watch(&pool, "user-b", "B's Watch").await;

        let bs_watches = list_watches_for_sync(&pool, "ws-1", "user-b")
            .await
            .expect("list_watches_for_sync for user-b");

        assert_eq!(
            bs_watches.len(),
            1,
            "user-b should see exactly their own watch, not user-a's"
        );
        let seen_id = bs_watches[0]
            .get("watch_id")
            .and_then(|v| v.as_str())
            .expect("watch_id field");
        assert_eq!(seen_id, wb.watch_id, "should be user-b's watch");
        assert_ne!(seen_id, wa.watch_id, "must not leak user-a's watch");
    }

    // ── list_watches / search_watches (KYO-178) ──────────────────────────
    //
    // `list_watches` and `search_watches` previously filtered on
    // `workspace_id` alone, so any workspace member could read every other
    // member's private watches. These tests lock in `created_by = user_id`
    // scoping on both functions.

    #[tokio::test]
    async fn list_watches_excludes_other_users_watches() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Watch").await;
        let wb = create_test_watch(&pool, "user-b", "B's Watch").await;

        let bs_watches = list_watches(&pool, "ws-1", "user-b")
            .await
            .expect("list_watches for user-b");

        assert_eq!(
            bs_watches.len(),
            1,
            "user-b should see exactly their own watch, not user-a's"
        );
        assert_eq!(bs_watches[0].watch_id, wb.watch_id, "should be user-b's watch");
        assert_ne!(bs_watches[0].watch_id, wa.watch_id, "must not leak user-a's watch");
    }

    #[tokio::test]
    async fn search_watches_excludes_other_users_watches_with_query() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        // Both users have a watch whose name matches the same search term
        // (watch names are unique per workspace, so the names differ but
        // both contain "Revenue"), exercising the `has_query` branch (ILIKE
        // on name/prompt) — this is what proves the renumbered
        // $3=user_id / $4=query bind chain is correct, since a bind-order
        // mistake would either error or silently leak user-a's row into
        // user-b's results.
        let wa = create_test_watch(&pool, "user-a", "Revenue Alert Watch A").await;
        let wb = create_test_watch(&pool, "user-b", "Revenue Alert Watch B").await;

        let results = search_watches(&pool, "ws-1", "user-b", Some("revenue"), 50)
            .await
            .expect("search_watches with query for user-b");

        assert_eq!(
            results.len(),
            1,
            "user-b should only match their own watch, not user-a's identically-named one"
        );
        assert_eq!(results[0].watch_id, wb.watch_id, "should be user-b's watch");
        assert_ne!(results[0].watch_id, wa.watch_id, "must not leak user-a's watch");
    }

    #[tokio::test]
    async fn search_watches_excludes_other_users_watches_without_query() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        // No query term — exercises the no-query branch.
        let wa = create_test_watch(&pool, "user-a", "A's Watch").await;
        let wb = create_test_watch(&pool, "user-b", "B's Watch").await;

        let results = search_watches(&pool, "ws-1", "user-b", None, 50)
            .await
            .expect("search_watches without query for user-b");

        assert_eq!(
            results.len(),
            1,
            "user-b should only see their own watch when listing without a query"
        );
        assert_eq!(results[0].watch_id, wb.watch_id, "should be user-b's watch");
        assert_ne!(results[0].watch_id, wa.watch_id, "must not leak user-a's watch");
    }

    // ── sync_log delta filtering ─────────────────────────────────────────

    #[tokio::test]
    async fn sync_delta_excludes_other_users_watch_creation() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        // B's cursor starts at the current watermark (0 — nothing synced yet).
        let cursor = sync_log_service::get_latest_sync_id(&pool, "ws-1")
            .await
            .expect("get_latest_sync_id");

        // A creates a watch — writes a private sync_log entry owned by A.
        let wa = create_test_watch(&pool, "user-a", "A's New Watch").await;

        let entries_for_b = sync_log_service::get_entries_since(&pool, "ws-1", cursor, "user-b", 100)
            .await
            .expect("get_entries_since for user-b");
        assert!(
            entries_for_b
                .iter()
                .all(|e| e.entity_id != wa.watch_id),
            "user-b's delta must not include user-a's private watch creation"
        );

        // B creates their own watch — B's delta must include it.
        let wb = create_test_watch(&pool, "user-b", "B's New Watch").await;
        let entries_for_b_after = sync_log_service::get_entries_since(&pool, "ws-1", cursor, "user-b", 100)
            .await
            .expect("get_entries_since for user-b after own create");
        assert!(
            entries_for_b_after
                .iter()
                .any(|e| e.entity_id == wb.watch_id),
            "user-b's delta must include their own watch creation"
        );
        assert!(
            entries_for_b_after
                .iter()
                .all(|e| e.entity_id != wa.watch_id),
            "user-b's delta still must not include user-a's watch after B creates their own"
        );
    }

    // ── Alerts queries ───────────────────────────────────────────────────

    #[tokio::test]
    async fn alerts_queries_exclude_other_users_alerts() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Alert Watch").await;
        let wb = create_test_watch(&pool, "user-b", "B's Alert Watch").await;
        insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;
        insert_triggered_alert(&pool, &wb.watch_id, "user-b").await;

        let count_for_b = get_unread_alerts_count(&pool, "ws-1", "user-b")
            .await
            .expect("get_unread_alerts_count for user-b");
        assert_eq!(
            count_for_b, 1,
            "user-b should only see their own watch's alert, not user-a's"
        );

        let (executions, total) =
            get_alerts_history(&pool, "ws-1", None, 50, 0, false, "user-b")
                .await
                .expect("get_alerts_history for user-b");
        assert_eq!(total, 1, "total should reflect only user-b's alert");
        assert_eq!(executions.len(), 1);
        assert_eq!(
            executions[0].watch_id.as_deref(),
            Some(wb.watch_id.as_str()),
            "the returned alert must belong to user-b's watch"
        );
    }

    /// Regression test for the watch_id-join bug: `watch_executions.watch_id`
    /// is `ON DELETE SET NULL`, so once the parent watch is deleted, a join
    /// back to `watches` for ownership silently drops the alert — even for
    /// the watch's own creator. Ownership must be filtered via the
    /// denormalized `watch_executions.created_by` column instead, which
    /// survives watch deletion the same way `watch_name`/`workspace_id` do.
    #[tokio::test]
    async fn alerts_remain_visible_to_owner_after_watch_deleted() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Watch To Delete").await;
        insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;

        // Sanity check: alert is visible before deletion.
        let count_before = get_unread_alerts_count(&pool, "ws-1", "user-a")
            .await
            .expect("get_unread_alerts_count before delete");
        assert_eq!(count_before, 1, "alert should be visible before watch deletion");

        // Delete the parent watch — this sets watch_executions.watch_id to NULL
        // via the ON DELETE SET NULL foreign key.
        delete_watch(&pool, &wa.watch_id, "ws-1", "user-a")
            .await
            .expect("delete_watch");

        let count_after = get_unread_alerts_count(&pool, "ws-1", "user-a")
            .await
            .expect("get_unread_alerts_count after delete");
        assert_eq!(
            count_after, 1,
            "alert must still be visible to its creator after the parent watch is deleted"
        );

        let (executions, total) =
            get_alerts_history(&pool, "ws-1", None, 50, 0, false, "user-a")
                .await
                .expect("get_alerts_history after delete");
        assert_eq!(
            total, 1,
            "alerts history must still include the alert after watch deletion"
        );
        assert_eq!(executions.len(), 1);
        assert_eq!(
            executions[0].watch_id, None,
            "watch_id should now be NULL (ON DELETE SET NULL)"
        );
        assert_eq!(
            executions[0].created_by.as_deref(),
            Some("user-a"),
            "created_by snapshot must survive watch deletion"
        );
    }

    // ── Live broadcast routing ───────────────────────────────────────────

    #[tokio::test]
    async fn broadcast_watch_sync_routes_to_owner_only() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Broadcast Watch").await;

        let manager = crate::websocket::WebSocketManager::new(None, pool.clone());
        let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
        let (_conn_b, mut rx_b) = manager.connect("user-b").expect("connect user-b");

        // Drain the immediate heartbeat each connect() sends.
        let heartbeat_a = rx_a.try_recv().expect("heartbeat for user-a");
        assert!(heartbeat_a.contains("heartbeat"));
        let heartbeat_b = rx_b.try_recv().expect("heartbeat for user-b");
        assert!(heartbeat_b.contains("heartbeat"));

        crate::websocket::helpers::broadcast_watch_sync(
            &pool,
            &manager,
            &wa.watch_id,
            "ws-1",
            SyncActionType::Insert,
            "user-a",
        )
        .await;

        let msg_a = rx_a
            .try_recv()
            .expect("owner (user-a) should receive the sync_action broadcast");
        assert!(msg_a.contains("sync_action"), "message should be a SyncAction: {msg_a}");
        assert!(msg_a.contains(&wa.watch_id), "message should reference the watch: {msg_a}");

        let result_b = rx_b.try_recv();
        assert!(
            result_b.is_err(),
            "non-owner (user-b) must NOT receive the watch broadcast, got: {result_b:?}"
        );
    }

    // ── IDOR guards on single-record watch operations (KYO-179) ─────────
    //
    // Watches are strictly private to their creator. These tests cover
    // get_watch / update_watch / delete_watch / toggle_watch /
    // get_executions being scoped on ownership, not just workspace_id, and
    // confirm owner behaviour is completely unaffected.

    #[tokio::test]
    async fn get_watch_returns_none_for_non_owner() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Private Watch").await;

        let result = get_watch(&pool, &wa.watch_id, "ws-1", "user-b")
            .await
            .expect("get_watch should not error for a non-owner");
        assert!(
            result.is_none(),
            "user-b must not be able to fetch user-a's watch by ID"
        );
    }

    #[tokio::test]
    async fn update_watch_rejects_non_owner_and_leaves_watch_unchanged() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Watch").await;

        let attempted_update = WatchUpdate {
            name: Some("Hijacked Name".to_string()),
            prompt: Some("Hijacked prompt with enough characters".to_string()),
            ..Default::default()
        };

        let result = update_watch(&pool, &wa.watch_id, "ws-1", "user-b", &attempted_update).await;
        assert!(
            matches!(result, Err(kyomi_core::Error::NotFound(_))),
            "user-b's update of user-a's watch must fail with NotFound, got: {result:?}"
        );

        // Re-fetch as the owner — every field must be unchanged and the
        // watch must still exist.
        let refetched = get_watch(&pool, &wa.watch_id, "ws-1", "user-a")
            .await
            .expect("get_watch as owner")
            .expect("watch must still exist");
        assert_eq!(refetched.name, wa.name, "name must be unchanged");
        assert_eq!(refetched.prompt, wa.prompt, "prompt must be unchanged");
    }

    #[tokio::test]
    async fn delete_watch_rejects_non_owner_and_leaves_watch_intact() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Watch To Keep").await;

        let result = delete_watch(&pool, &wa.watch_id, "ws-1", "user-b").await;
        assert!(
            matches!(result, Err(kyomi_core::Error::NotFound(_))),
            "user-b's delete of user-a's watch must fail with NotFound, got: {result:?}"
        );

        let still_exists = get_watch(&pool, &wa.watch_id, "ws-1", "user-a")
            .await
            .expect("get_watch as owner")
            .is_some();
        assert!(
            still_exists,
            "watch must still exist after a non-owner's delete attempt"
        );
    }

    #[tokio::test]
    async fn toggle_watch_rejects_non_owner_and_leaves_enabled_unchanged() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Toggle Watch").await;
        assert!(wa.enabled, "sanity check: watches are created enabled");

        let result = toggle_watch(&pool, &wa.watch_id, "ws-1", "user-b", false).await;
        assert!(
            matches!(result, Err(kyomi_core::Error::NotFound(_))),
            "user-b's toggle of user-a's watch must fail with NotFound, got: {result:?}"
        );

        let refetched = get_watch(&pool, &wa.watch_id, "ws-1", "user-a")
            .await
            .expect("get_watch as owner")
            .expect("watch must still exist");
        assert!(
            refetched.enabled,
            "enabled state must be unchanged by a non-owner's toggle attempt"
        );
    }

    #[tokio::test]
    async fn get_executions_returns_empty_for_non_owner() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Execution Watch").await;
        insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;

        let executions = get_executions(&pool, &wa.watch_id, "ws-1", "user-b", 20)
            .await
            .expect("get_executions should not error for a non-owner");
        assert!(
            executions.is_empty(),
            "user-b must not see execution history for user-a's watch"
        );
    }

    #[tokio::test]
    async fn owner_can_still_get_update_toggle_and_delete_their_own_watch() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Own Watch").await;

        // get_watch as owner succeeds.
        let fetched = get_watch(&pool, &wa.watch_id, "ws-1", "user-a")
            .await
            .expect("get_watch as owner")
            .expect("owner must be able to fetch their own watch");
        assert_eq!(fetched.watch_id, wa.watch_id);

        // update_watch as owner actually changes the field.
        let update = WatchUpdate {
            name: Some("A's Renamed Watch".to_string()),
            ..Default::default()
        };
        let updated = update_watch(&pool, &wa.watch_id, "ws-1", "user-a", &update)
            .await
            .expect("owner's update_watch must succeed");
        assert_eq!(updated.name, "A's Renamed Watch", "owner's update must take effect");

        // toggle_watch as owner succeeds and flips enabled.
        let toggled = toggle_watch(&pool, &wa.watch_id, "ws-1", "user-a", false)
            .await
            .expect("owner's toggle_watch must succeed");
        assert!(!toggled.enabled, "owner's toggle must take effect");

        // delete_watch as owner succeeds.
        let deleted_owner = delete_watch(&pool, &wa.watch_id, "ws-1", "user-a")
            .await
            .expect("owner's delete_watch must succeed");
        assert_eq!(deleted_owner, "user-a");

        let gone = get_watch(&pool, &wa.watch_id, "ws-1", "user-a")
            .await
            .expect("get_watch after delete");
        assert!(gone.is_none(), "watch must be gone after owner deletes it");
    }

    // ── create_chat_session_from_alert (KYO-179 review gap) ─────────────
    //
    // `get_execution_by_id_only` is the primary, un-gated-until-now data
    // source for `continue_alert_in_chat` — the fallback `get_watch` call
    // was fixed in the first pass, but this path (reachable with nothing
    // but a guessed sequential `execution_id`) was not. These tests cover
    // the fix directly, including that no durable side effect (a chat
    // session owned by the attacker) is created on rejection.

    /// Insert a triggered alert execution carrying identifiable content in
    /// `agent_response` and `execution_trace` (watch_prompt/alert_title), so
    /// leak-detection assertions have something concrete to check for.
    /// Returns the new execution's `id`.
    async fn insert_triggered_alert_with_content(
        pool: &DbPool,
        watch_id: &str,
        created_by: &str,
        agent_response: &str,
        watch_prompt: &str,
        alert_title: &str,
    ) -> i32 {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        let execution_trace = serde_json::json!({
            "watch_prompt": watch_prompt,
            "alert_title": alert_title,
        });
        let row: (i32,) = sqlx::query_as(
            "INSERT INTO watch_executions \
             (watch_id, workspace_id, status, alert_triggered, started_at, completed_at, \
              created_by, agent_response, execution_trace) \
             VALUES ($1, 'ws-1', 'success', 1, datetime('now'), datetime('now'), $2, $3, $4) \
             RETURNING id",
        )
        .bind(watch_id)
        .bind(created_by)
        .bind(agent_response)
        .bind(execution_trace.to_string())
        .fetch_one(sq)
        .await
        .expect("insert triggered alert with content");
        row.0
    }

    #[tokio::test]
    async fn create_chat_session_from_alert_rejects_non_owner_and_leaks_nothing() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Secret Revenue Watch").await;
        let execution_id = insert_triggered_alert_with_content(
            &pool,
            &wa.watch_id,
            "user-a",
            "SECRET: APAC revenue dropped 42 percent",
            "Check if revenue drops more than 10 percent in APAC",
            "Revenue Alert",
        )
        .await;

        let encryption_key = &[0u8; 32];

        // user-b guesses user-a's execution_id (a sequential integer).
        let result =
            create_chat_session_from_alert(&pool, encryption_key, "user-b", "ws-1", execution_id)
                .await;
        assert!(
            matches!(result, Err(kyomi_core::Error::NotFound(_))),
            "user-b must not be able to continue user-a's alert in chat, got: {result:?}"
        );

        let sq = match &pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };

        // No chat session must exist for user-b as a side effect of the
        // rejected attempt — the leak this test guards against is durable
        // persistence into an attacker-owned session, not just the return
        // value.
        let session_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM chat_sessions WHERE user_id = 'user-b'")
                .fetch_one(sq)
                .await
                .expect("count chat_sessions for user-b");
        assert_eq!(
            session_count.0, 0,
            "a rejected alert-to-chat attempt must not create any chat session for the attacker"
        );

        // No message content must have been persisted anywhere user-b's
        // sessions could read it back from.
        let leaked_messages: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM chat_messages cm \
             JOIN chat_sessions cs ON cm.session_id = cs.session_id \
             WHERE cs.user_id = 'user-b'",
        )
        .fetch_one(sq)
        .await
        .expect("count chat_messages for user-b's sessions");
        assert_eq!(
            leaked_messages.0, 0,
            "no message content must have been persisted into any session owned by user-b"
        );
    }

    #[tokio::test]
    async fn create_chat_session_from_alert_succeeds_for_owner() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Own Alert Watch").await;
        let execution_id = insert_triggered_alert_with_content(
            &pool,
            &wa.watch_id,
            "user-a",
            "Revenue is stable this week",
            "Check if revenue drops more than 10 percent",
            "Revenue Alert",
        )
        .await;

        let encryption_key = &[0u8; 32];

        let session_id =
            create_chat_session_from_alert(&pool, encryption_key, "user-a", "ws-1", execution_id)
                .await
                .expect("owner's continue-alert-in-chat must succeed");

        let session = chat_service::get_session(&pool, &session_id)
            .await
            .expect("get_session")
            .expect("session must exist");
        assert_eq!(session.user_id, "user-a", "session must be owned by the caller");
        assert!(
            session.title.as_deref().unwrap_or_default().contains("Revenue Alert"),
            "session title must reflect the alert, got: {:?}",
            session.title
        );

        let messages = chat_service::get_session_messages(&pool, encryption_key, &session_id, 10)
            .await
            .expect("get_session_messages");
        assert!(
            messages
                .iter()
                .any(|m| m.content.contains("Revenue is stable this week")),
            "owner's session must contain the alert's agent_response"
        );
    }

    // ── Alert mutation IDOR guards (KYO-204) ─────────────────────────────
    //
    // `mark_alert_read`, `mark_alert_unread`, `delete_alert`, `restore_alert`,
    // `bulk_delete_alerts`, `bulk_mark_alerts_read`, and
    // `bulk_mark_alerts_unread` all previously scoped their `WHERE` clause on
    // `workspace_id` alone (any `user_id` bind was for an audit column only,
    // e.g. `deleted_by`), so any workspace member could mutate another
    // member's alerts by guessing a sequential `execution_id`. All seven now
    // filter on `watch_executions.created_by`, matching the read-path
    // precedent set by `get_alerts_history` / `get_execution_by_id_only`
    // (KYO-179). `bulk_delete_alerts` was not in the original ticket's gap
    // table — it was found to have the identical defect while auditing
    // `delete_alert`'s callers, and is covered here alongside the other six.
    //
    // Every non-owner test asserts both the call's own contract (error, or
    // `Ok` with zero/partial rows affected — per that function's existing
    // zero-rows behaviour) AND that the target row is genuinely unchanged,
    // re-read via the already-privacy-scoped `get_execution_by_id_only` as
    // the owner. A fix that mutates the row and then merely reports failure
    // would still pass a return-value-only assertion.

    #[tokio::test]
    async fn mark_alert_read_rejects_non_owner_and_leaves_alert_unchanged() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Alert Watch").await;
        let execution_id = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;

        mark_alert_read(&pool, execution_id, "ws-1", "user-b")
            .await
            .expect("mark_alert_read must not error for a non-owner (silent no-op)");

        let refetched = get_execution_by_id_only(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only as owner")
            .expect("execution must still exist");
        assert!(
            refetched.read_at.is_none(),
            "user-b's mark_alert_read must not have marked user-a's alert as read"
        );
    }

    #[tokio::test]
    async fn mark_alert_unread_rejects_non_owner_and_leaves_alert_unchanged() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Alert Watch").await;
        let execution_id = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;

        // Owner marks it read first so there's a read_at value to protect.
        mark_alert_read(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("owner's mark_alert_read must succeed");
        let read_at_before = get_execution_by_id_only(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only as owner")
            .expect("execution must exist")
            .read_at
            .expect("read_at must be set after owner marks read");

        mark_alert_unread(&pool, execution_id, "ws-1", "user-b")
            .await
            .expect("mark_alert_unread must not error for a non-owner (silent no-op)");

        let refetched = get_execution_by_id_only(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only as owner")
            .expect("execution must still exist");
        assert_eq!(
            refetched.read_at,
            Some(read_at_before),
            "user-b's mark_alert_unread must not have cleared user-a's read_at"
        );
    }

    #[tokio::test]
    async fn delete_alert_rejects_non_owner_and_leaves_alert_unchanged() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Alert Watch").await;
        let execution_id = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;

        let result = delete_alert(&pool, execution_id, "ws-1", "user-b").await;
        assert!(
            matches!(result, Err(kyomi_core::Error::NotFound(_))),
            "user-b's delete of user-a's alert must fail with NotFound, got: {result:?}"
        );

        let refetched = get_execution_by_id_only(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only as owner")
            .expect("execution must still exist");
        assert!(
            refetched.deleted_at.is_none(),
            "alert must not be soft-deleted by a non-owner"
        );
        assert!(
            refetched.deleted_by.is_none(),
            "deleted_by must not be set by a non-owner"
        );
    }

    #[tokio::test]
    async fn restore_alert_rejects_non_owner_and_leaves_alert_unchanged() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Alert Watch").await;
        let execution_id = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;

        // Owner deletes it first so there's a deleted state to protect.
        delete_alert(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("owner's delete_alert must succeed");
        let deleted_state = get_execution_by_id_only(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only as owner")
            .expect("execution must exist");
        let deleted_at_before = deleted_state
            .deleted_at
            .expect("deleted_at must be set after owner deletes");
        let deleted_by_before = deleted_state.deleted_by.clone();
        assert_eq!(deleted_by_before.as_deref(), Some("user-a"));

        let result = restore_alert(&pool, execution_id, "ws-1", "user-b").await;
        assert!(
            matches!(result, Err(kyomi_core::Error::NotFound(_))),
            "user-b's restore of user-a's alert must fail with NotFound, got: {result:?}"
        );

        let refetched = get_execution_by_id_only(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only as owner")
            .expect("execution must still exist");
        assert_eq!(
            refetched.deleted_at,
            Some(deleted_at_before),
            "deleted_at must be unchanged by a non-owner's restore attempt"
        );
        assert_eq!(
            refetched.deleted_by, deleted_by_before,
            "deleted_by must be unchanged by a non-owner's restore attempt"
        );
    }

    #[tokio::test]
    async fn bulk_delete_alerts_rejects_non_owner_and_leaves_alert_unchanged() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Alert Watch").await;
        let execution_id = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;

        let count = bulk_delete_alerts(&pool, &[execution_id], "ws-1", "user-b")
            .await
            .expect("bulk_delete_alerts must not error for a non-owner batch");
        assert_eq!(
            count, 0,
            "bulk_delete_alerts must not affect any rows for a non-owned id"
        );

        let refetched = get_execution_by_id_only(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only as owner")
            .expect("execution must still exist");
        assert!(
            refetched.deleted_at.is_none(),
            "alert must not be soft-deleted by a non-owner's bulk delete"
        );
    }

    #[tokio::test]
    async fn bulk_mark_alerts_read_rejects_non_owner_and_leaves_alert_unchanged() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Alert Watch").await;
        let execution_id = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;

        let count = bulk_mark_alerts_read(&pool, &[execution_id], "ws-1", "user-b")
            .await
            .expect("bulk_mark_alerts_read must not error for a non-owner batch");
        assert_eq!(
            count, 0,
            "bulk_mark_alerts_read must not affect any rows for a non-owned id"
        );

        let refetched = get_execution_by_id_only(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only as owner")
            .expect("execution must still exist");
        assert!(
            refetched.read_at.is_none(),
            "alert must not be marked read by a non-owner's bulk mark-read"
        );
    }

    #[tokio::test]
    async fn bulk_mark_alerts_unread_rejects_non_owner_and_leaves_alert_unchanged() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Alert Watch").await;
        let execution_id = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;

        mark_alert_read(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("owner's mark_alert_read must succeed");
        let read_at_before = get_execution_by_id_only(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only as owner")
            .expect("execution must exist")
            .read_at
            .expect("read_at must be set");

        let count = bulk_mark_alerts_unread(&pool, &[execution_id], "ws-1", "user-b")
            .await
            .expect("bulk_mark_alerts_unread must not error for a non-owner batch");
        assert_eq!(
            count, 0,
            "bulk_mark_alerts_unread must not affect any rows for a non-owned id"
        );

        let refetched = get_execution_by_id_only(&pool, execution_id, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only as owner")
            .expect("execution must still exist");
        assert_eq!(
            refetched.read_at,
            Some(read_at_before),
            "read_at must be unchanged by a non-owner's bulk mark-unread"
        );
    }

    #[tokio::test]
    async fn owner_can_still_mark_delete_restore_and_bulk_mutate_their_own_alerts() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Lifecycle Watch").await;

        // mark_alert_read / mark_alert_unread as owner.
        let exec1 = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;
        mark_alert_read(&pool, exec1, "ws-1", "user-a")
            .await
            .expect("owner's mark_alert_read must succeed");
        let after_read = get_execution_by_id_only(&pool, exec1, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only")
            .expect("execution must exist");
        assert!(
            after_read.read_at.is_some(),
            "owner's mark_alert_read must take effect"
        );

        mark_alert_unread(&pool, exec1, "ws-1", "user-a")
            .await
            .expect("owner's mark_alert_unread must succeed");
        let after_unread = get_execution_by_id_only(&pool, exec1, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only")
            .expect("execution must exist");
        assert!(
            after_unread.read_at.is_none(),
            "owner's mark_alert_unread must take effect"
        );

        // delete_alert / restore_alert as owner.
        let exec2 = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;
        delete_alert(&pool, exec2, "ws-1", "user-a")
            .await
            .expect("owner's delete_alert must succeed");
        let after_delete = get_execution_by_id_only(&pool, exec2, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only")
            .expect("execution must exist");
        assert!(
            after_delete.deleted_at.is_some(),
            "owner's delete_alert must take effect"
        );
        assert_eq!(after_delete.deleted_by.as_deref(), Some("user-a"));

        restore_alert(&pool, exec2, "ws-1", "user-a")
            .await
            .expect("owner's restore_alert must succeed");
        let after_restore = get_execution_by_id_only(&pool, exec2, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only")
            .expect("execution must exist");
        assert!(
            after_restore.deleted_at.is_none(),
            "owner's restore_alert must take effect"
        );
        assert!(after_restore.deleted_by.is_none());

        // Bulk ops as owner.
        let exec3 = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;
        let exec4 = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;

        let read_count = bulk_mark_alerts_read(&pool, &[exec3, exec4], "ws-1", "user-a")
            .await
            .expect("owner's bulk_mark_alerts_read must succeed");
        assert_eq!(
            read_count, 2,
            "owner's bulk_mark_alerts_read must affect both rows"
        );

        let unread_count = bulk_mark_alerts_unread(&pool, &[exec3, exec4], "ws-1", "user-a")
            .await
            .expect("owner's bulk_mark_alerts_unread must succeed");
        assert_eq!(
            unread_count, 2,
            "owner's bulk_mark_alerts_unread must affect both rows"
        );

        let delete_count = bulk_delete_alerts(&pool, &[exec3, exec4], "ws-1", "user-a")
            .await
            .expect("owner's bulk_delete_alerts must succeed");
        assert_eq!(
            delete_count, 2,
            "owner's bulk_delete_alerts must affect both rows"
        );
    }

    #[tokio::test]
    async fn bulk_delete_alerts_mixed_batch_affects_only_callers_own_alert() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Watch").await;
        let wb = create_test_watch(&pool, "user-b", "B's Watch").await;
        let exec_a = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;
        let exec_b = insert_triggered_alert(&pool, &wb.watch_id, "user-b").await;

        let count = bulk_delete_alerts(&pool, &[exec_a, exec_b], "ws-1", "user-b")
            .await
            .expect("bulk_delete_alerts for a mixed batch must not error");
        assert_eq!(
            count, 1,
            "mixed batch must only affect the caller's own alert"
        );

        let a_state = get_execution_by_id_only(&pool, exec_a, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only for A")
            .expect("A's execution must still exist");
        assert!(
            a_state.deleted_at.is_none(),
            "A's alert must be untouched by B's mixed-batch delete"
        );

        let b_state = get_execution_by_id_only(&pool, exec_b, "ws-1", "user-b")
            .await
            .expect("get_execution_by_id_only for B")
            .expect("B's execution must still exist");
        assert!(
            b_state.deleted_at.is_some(),
            "B's own alert must have been deleted"
        );
    }

    #[tokio::test]
    async fn bulk_mark_alerts_read_mixed_batch_affects_only_callers_own_alert() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Watch").await;
        let wb = create_test_watch(&pool, "user-b", "B's Watch").await;
        let exec_a = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;
        let exec_b = insert_triggered_alert(&pool, &wb.watch_id, "user-b").await;

        let count = bulk_mark_alerts_read(&pool, &[exec_a, exec_b], "ws-1", "user-b")
            .await
            .expect("bulk_mark_alerts_read for a mixed batch must not error");
        assert_eq!(
            count, 1,
            "mixed batch must only affect the caller's own alert"
        );

        let a_state = get_execution_by_id_only(&pool, exec_a, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only for A")
            .expect("A's execution must still exist");
        assert!(
            a_state.read_at.is_none(),
            "A's alert must be untouched by B's mixed-batch mark-read"
        );

        let b_state = get_execution_by_id_only(&pool, exec_b, "ws-1", "user-b")
            .await
            .expect("get_execution_by_id_only for B")
            .expect("B's execution must still exist");
        assert!(
            b_state.read_at.is_some(),
            "B's own alert must have been marked read"
        );
    }

    #[tokio::test]
    async fn bulk_mark_alerts_unread_mixed_batch_affects_only_callers_own_alert() {
        let pool = test_pool().await;
        seed_workspace_with_two_users(&pool).await;

        let wa = create_test_watch(&pool, "user-a", "A's Watch").await;
        let wb = create_test_watch(&pool, "user-b", "B's Watch").await;
        let exec_a = insert_triggered_alert(&pool, &wa.watch_id, "user-a").await;
        let exec_b = insert_triggered_alert(&pool, &wb.watch_id, "user-b").await;

        // Both start read so there's an unread transition to protect/apply.
        mark_alert_read(&pool, exec_a, "ws-1", "user-a")
            .await
            .expect("owner's mark_alert_read for A must succeed");
        mark_alert_read(&pool, exec_b, "ws-1", "user-b")
            .await
            .expect("owner's mark_alert_read for B must succeed");

        let count = bulk_mark_alerts_unread(&pool, &[exec_a, exec_b], "ws-1", "user-b")
            .await
            .expect("bulk_mark_alerts_unread for a mixed batch must not error");
        assert_eq!(
            count, 1,
            "mixed batch must only affect the caller's own alert"
        );

        let a_state = get_execution_by_id_only(&pool, exec_a, "ws-1", "user-a")
            .await
            .expect("get_execution_by_id_only for A")
            .expect("A's execution must still exist");
        assert!(
            a_state.read_at.is_some(),
            "A's alert must still be read — untouched by B's mixed-batch mark-unread"
        );

        let b_state = get_execution_by_id_only(&pool, exec_b, "ws-1", "user-b")
            .await
            .expect("get_execution_by_id_only for B")
            .expect("B's execution must still exist");
        assert!(
            b_state.read_at.is_none(),
            "B's own alert must have been marked unread"
        );
    }
}
