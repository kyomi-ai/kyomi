// SPDX-License-Identifier: AGPL-3.0-or-later

//! Watch service — CRUD, scheduling, execution tracking, search, and alert lifecycle.
//!
//! Ports Python's `watch_service.py` into a shared service layer used by both
//! agent tools (Phase 9B-4) and REST endpoints (Phase 12).
//!
//! Key design decisions:
//! - Free-function pattern (`&DbPool` first arg) matching `dashboard_service.rs`
//! - 5-field cron validation with the `cron` crate (converts to 7-field internally)
//! - Tier-based watch limits (free=0, pro=10, team=50, enterprise=200)
//! - Rate limiting: max 5 manual runs per hour
//! - Soft-delete for alerts (deleted_at / deleted_by)

use chrono::{DateTime, Utc};
use cron::Schedule;
use kyomi_core::sql_compat;
use kyomi_core::{DbPool, Result};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Watch limits per subscription tier.
const WATCH_LIMIT_FREE: i64 = 0;
const WATCH_LIMIT_PRO: i64 = 10;
const WATCH_LIMIT_TEAM: i64 = 50;
const WATCH_LIMIT_ENTERPRISE: i64 = 200;

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
fn watch_limit_for_tier(tier: kyomi_core::SubscriptionTier) -> i64 {
    use kyomi_core::SubscriptionTier::*;
    match tier {
        Pro => WATCH_LIMIT_PRO,
        Team => WATCH_LIMIT_TEAM,
        Enterprise => WATCH_LIMIT_ENTERPRISE,
        _ => WATCH_LIMIT_FREE, // free, starter, basic
    }
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
    Ok(watch)
}

// ─── Get watch ──────────────────────────────────────────────────────────────

/// Fetch a watch by ID within a workspace.
pub async fn get_watch(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
) -> Result<Option<kyomi_core::models::Watch>> {
    let sql = r#"
        SELECT watch_id, workspace_id, created_by, name, prompt, schedule,
               mode, datasource_hints, queries, alert_emails,
               alert_emails_enabled, enabled, last_run_at, last_run_status,
               next_run_at, created_at, updated_at
        FROM watches
        WHERE watch_id = $1 AND workspace_id = $2
    "#;

    let watch = kyomi_core::db_fetch_optional!(
        db,
        kyomi_core::models::Watch,
        sql,
        watch_id,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get watch: {e}")))?;

    Ok(watch)
}

// ─── List watches ───────────────────────────────────────────────────────────

/// List all watches for a workspace (newest first).
pub async fn list_watches(
    db: &DbPool,
    workspace_id: &str,
) -> Result<Vec<kyomi_core::models::Watch>> {
    let sql = r#"
        SELECT watch_id, workspace_id, created_by, name, prompt, schedule,
               mode, datasource_hints, queries, alert_emails,
               alert_emails_enabled, enabled, last_run_at, last_run_status,
               next_run_at, created_at, updated_at
        FROM watches
        WHERE workspace_id = $1
        ORDER BY created_at DESC
    "#;

    let watches = kyomi_core::db_fetch_all!(db, kyomi_core::models::Watch, sql, workspace_id)
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to list watches: {e}")))?;

    Ok(watches)
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
pub async fn update_watch(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
    updates: &WatchUpdate,
) -> Result<kyomi_core::models::Watch> {
    // Fetch current watch for schedule-dependent logic
    let current = get_watch(db, watch_id, workspace_id).await?;
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
    let mut param_idx = 3u32; // $1 = watch_id, $2 = workspace_id

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
        r#"UPDATE watches SET {} WHERE watch_id = $1 AND workspace_id = $2
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

    let watch = match db {
        DbPool::Postgres(pg) => {
            let q = sqlx::query_as::<_, kyomi_core::models::Watch>(&sql)
                .bind(watch_id)
                .bind(workspace_id);
            bind_update_params!(q).fetch_one(pg).await
        }
        DbPool::Sqlite(sq) => {
            let q = sqlx::query_as::<_, kyomi_core::models::Watch>(&sql)
                .bind(watch_id)
                .bind(workspace_id);
            bind_update_params!(q).fetch_one(sq).await
        }
    }
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to update watch: {e}"))
    })?;

    tracing::info!(watch_id = %watch_id, "Updated watch");
    Ok(watch)
}

// ─── Delete watch ───────────────────────────────────────────────────────────

/// Delete a watch by ID within a workspace.
pub async fn delete_watch(db: &DbPool, watch_id: &str, workspace_id: &str) -> Result<()> {
    let result = kyomi_core::db_execute!(
        db,
        "DELETE FROM watches WHERE watch_id = $1 AND workspace_id = $2",
        watch_id,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete watch: {e}")))?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::NotFound(format!(
            "Watch {watch_id} not found"
        )));
    }

    tracing::info!(watch_id = %watch_id, "Deleted watch");
    Ok(())
}

// ─── Toggle watch ───────────────────────────────────────────────────────────

/// Enable or disable a watch.
pub async fn toggle_watch(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
    enabled: bool,
) -> Result<kyomi_core::models::Watch> {
    update_watch(
        db,
        watch_id,
        workspace_id,
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
    limit: u32,
) -> Result<Vec<kyomi_core::models::WatchExecution>> {
    // Verify watch belongs to workspace
    let watch = get_watch(db, watch_id, workspace_id).await?;
    if watch.is_none() {
        return Ok(Vec::new());
    }

    let limit_i64 = limit as i64;
    let sql = r#"
        SELECT id, watch_id, watch_name, mode, workspace_id, session_id,
               started_at, completed_at, status, agent_response, error_message,
               input_tokens, output_tokens, cost_estimate, execution_trace,
               alert_triggered, notification_id, read_at, deleted_at, deleted_by
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
) -> Result<Option<kyomi_core::models::WatchExecution>> {
    // Verify watch belongs to workspace
    let watch = get_watch(db, watch_id, workspace_id).await?;
    if watch.is_none() {
        return Ok(None);
    }

    let sql = r#"
        SELECT id, watch_id, watch_name, mode, workspace_id, session_id,
               started_at, completed_at, status, agent_response, error_message,
               input_tokens, output_tokens, cost_estimate, execution_trace,
               alert_triggered, notification_id, read_at, deleted_at, deleted_by
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
pub async fn get_execution_by_id_only(
    db: &DbPool,
    execution_id: i32,
    workspace_id: &str,
) -> Result<Option<kyomi_core::models::WatchExecution>> {
    let sql = r#"
        SELECT id, watch_id, watch_name, mode, workspace_id, session_id,
               started_at, completed_at, status, agent_response, error_message,
               input_tokens, output_tokens, cost_estimate, execution_trace,
               alert_triggered, notification_id, read_at, deleted_at, deleted_by
        FROM watch_executions
        WHERE id = $1 AND workspace_id = $2
    "#;

    let execution = kyomi_core::db_fetch_optional!(
        db,
        kyomi_core::models::WatchExecution,
        sql,
        execution_id,
        workspace_id
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
) -> Result<kyomi_core::models::WatchExecution> {
    let is_pg = db.is_postgres();
    let mode_str = mode.as_ref();

    let sql = format!(
        r#"
        INSERT INTO watch_executions (
            watch_id, watch_name, mode, workspace_id, status,
            started_at, alert_triggered
        )
        VALUES ($1, $2, $3, $4, 'running', {now}, {false_val})
        RETURNING id, watch_id, watch_name, mode, workspace_id, session_id,
                  started_at, completed_at, status, agent_response, error_message,
                  input_tokens, output_tokens, cost_estimate, execution_trace,
                  alert_triggered, notification_id, read_at, deleted_at, deleted_by
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
        workspace_id
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
                  alert_triggered, notification_id, read_at, deleted_at, deleted_by
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
pub async fn can_run_watch_now(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
) -> Result<(bool, String)> {
    let is_pg = db.is_postgres();

    // Verify watch exists
    let watch = get_watch(db, watch_id, workspace_id).await?;
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

/// Search watches in a workspace by name and prompt.
///
/// If `query` is `None` or empty, returns all watches sorted by `created_at DESC`.
/// Otherwise, performs ILIKE search on `name` and `prompt`.
pub async fn search_watches(
    db: &DbPool,
    workspace_id: &str,
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
              AND ({name_like} OR {prompt_like})
            ORDER BY created_at DESC
            LIMIT $2
            "#,
            name_like = sql_compat::ilike(is_pg, "name", "'%' || $3 || '%'"),
            prompt_like = sql_compat::ilike(is_pg, "prompt", "'%' || $3 || '%'"),
        )
    } else {
        r#"
        SELECT watch_id, workspace_id, created_by, name, prompt, schedule, mode,
               datasource_hints, queries, alert_emails,
               alert_emails_enabled, enabled, last_run_at, last_run_status,
               next_run_at, created_at, updated_at
        FROM watches
        WHERE workspace_id = $1
        ORDER BY created_at DESC
        LIMIT $2
        "#
        .to_string()
    };

    // Dynamic SQL — bind chain varies based on has_query
    let watches = match db {
        DbPool::Postgres(pg) => {
            let mut q = sqlx::query_as::<_, kyomi_core::models::Watch>(&sql)
                .bind(workspace_id)
                .bind(limit);
            if let (true, Some(query_str)) = (has_query, query) {
                q = q.bind(query_str.trim());
            }
            q.fetch_all(pg).await
        }
        DbPool::Sqlite(sq) => {
            let mut q = sqlx::query_as::<_, kyomi_core::models::Watch>(&sql)
                .bind(workspace_id)
                .bind(limit);
            if let (true, Some(query_str)) = (has_query, query) {
                q = q.bind(query_str.trim());
            }
            q.fetch_all(sq).await
        }
    }
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to search watches: {e}"))
    })?;

    Ok(watches)
}

// ─── Unread alerts count ────────────────────────────────────────────────────

/// Count unread, non-deleted alerts for a workspace (for sidebar badge).
pub async fn get_unread_alerts_count(db: &DbPool, workspace_id: &str) -> Result<i64> {
    let is_pg = db.is_postgres();
    let sql = format!(
        r#"
        SELECT COUNT(*)
        FROM watch_executions
        WHERE workspace_id = $1
          AND alert_triggered = {true_val}
          AND read_at IS NULL
          AND deleted_at IS NULL
        "#,
        true_val = sql_compat::bool_true(is_pg),
    );

    let count: i64 = kyomi_core::db_fetch_scalar!(db, i64, &sql, workspace_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to count unread alerts: {e}"))
        })?;

    Ok(count)
}

// ─── Alerts history ────────────────────────────────────────────────────────

/// Get alerts history (paginated, with total count).
///
/// Filters to alert_triggered=true executions. Optionally filters by watch_id.
/// Returns `(executions, total_count)` for pagination.
pub async fn get_alerts_history(
    db: &DbPool,
    workspace_id: &str,
    watch_id: Option<&str>,
    limit: i64,
    offset: i64,
    include_deleted: bool,
) -> Result<(Vec<kyomi_core::models::WatchExecution>, i64)> {
    let is_pg = db.is_postgres();
    let true_val = sql_compat::bool_true(is_pg);

    let deleted_filter = if include_deleted {
        ""
    } else {
        "AND we.deleted_at IS NULL"
    };

    // COUNT query — only $1 (workspace_id) and optionally $2 (watch_id)
    let count_watch_filter = if watch_id.is_some() {
        "AND we.watch_id = $2"
    } else {
        ""
    };

    let count_sql = format!(
        r#"
        SELECT COUNT(*)
        FROM watch_executions we
        WHERE we.workspace_id = $1
          AND we.alert_triggered = {true_val}
          {deleted_filter}
          {count_watch_filter}
        "#
    );

    // Dynamic SQL — bind chain varies based on watch_id
    let total_count: i64 = match db {
        DbPool::Postgres(pg) => {
            if let Some(wid) = watch_id {
                sqlx::query_scalar(&count_sql)
                    .bind(workspace_id)
                    .bind(wid)
                    .fetch_one(pg)
                    .await
            } else {
                sqlx::query_scalar(&count_sql)
                    .bind(workspace_id)
                    .fetch_one(pg)
                    .await
            }
        }
        DbPool::Sqlite(sq) => {
            if let Some(wid) = watch_id {
                sqlx::query_scalar(&count_sql)
                    .bind(workspace_id)
                    .bind(wid)
                    .fetch_one(sq)
                    .await
            } else {
                sqlx::query_scalar(&count_sql)
                    .bind(workspace_id)
                    .fetch_one(sq)
                    .await
            }
        }
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to count alerts: {e}")))?;

    // SELECT query — $1 (workspace_id), $2 (limit), $3 (offset), optionally $4 (watch_id)
    let select_watch_filter = if watch_id.is_some() {
        "AND we.watch_id = $4"
    } else {
        ""
    };

    let select_sql = format!(
        r#"
        SELECT we.id, we.watch_id, we.watch_name, we.mode, we.workspace_id, we.session_id,
               we.started_at, we.completed_at, we.status, we.agent_response, we.error_message,
               we.input_tokens, we.output_tokens, we.cost_estimate, we.execution_trace,
               we.alert_triggered, we.notification_id, we.read_at, we.deleted_at, we.deleted_by
        FROM watch_executions we
        WHERE we.workspace_id = $1
          AND we.alert_triggered = {true_val}
          {deleted_filter}
          {select_watch_filter}
        ORDER BY we.started_at DESC
        LIMIT $2 OFFSET $3
        "#
    );

    // Dynamic SQL — bind chain varies based on watch_id
    let executions = match db {
        DbPool::Postgres(pg) => {
            if let Some(wid) = watch_id {
                sqlx::query_as::<_, kyomi_core::models::WatchExecution>(&select_sql)
                    .bind(workspace_id)
                    .bind(limit)
                    .bind(offset)
                    .bind(wid)
                    .fetch_all(pg)
                    .await
            } else {
                sqlx::query_as::<_, kyomi_core::models::WatchExecution>(&select_sql)
                    .bind(workspace_id)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(pg)
                    .await
            }
        }
        DbPool::Sqlite(sq) => {
            if let Some(wid) = watch_id {
                sqlx::query_as::<_, kyomi_core::models::WatchExecution>(&select_sql)
                    .bind(workspace_id)
                    .bind(limit)
                    .bind(offset)
                    .bind(wid)
                    .fetch_all(sq)
                    .await
            } else {
                sqlx::query_as::<_, kyomi_core::models::WatchExecution>(&select_sql)
                    .bind(workspace_id)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(sq)
                    .await
            }
        }
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch alerts: {e}")))?;

    Ok((executions, total_count))
}

// ─── Alert lifecycle ────────────────────────────────────────────────────────

/// Mark an alert as read (set `read_at` to now).
pub async fn mark_alert_read(
    db: &DbPool,
    execution_id: i32,
    workspace_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let sql = format!(
        r#"
        UPDATE watch_executions
        SET read_at = {now}
        WHERE id = $1
          AND workspace_id = $2
          AND alert_triggered = {true_val}
          AND read_at IS NULL
        "#,
        now = sql_compat::now(is_pg),
        true_val = sql_compat::bool_true(is_pg),
    );

    let result = kyomi_core::db_execute!(db, &sql, execution_id, workspace_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to mark alert read: {e}"))
        })?;

    if result.rows_affected() > 0 {
        tracing::info!(execution_id, "Alert marked as read");
    }
    Ok(())
}

/// Mark an alert as unread (clear `read_at`).
pub async fn mark_alert_unread(
    db: &DbPool,
    execution_id: i32,
    workspace_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let sql = format!(
        r#"
        UPDATE watch_executions
        SET read_at = NULL
        WHERE id = $1
          AND workspace_id = $2
          AND alert_triggered = {true_val}
          AND read_at IS NOT NULL
        "#,
        true_val = sql_compat::bool_true(is_pg),
    );

    let result = kyomi_core::db_execute!(db, &sql, execution_id, workspace_id)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to mark alert unread: {e}"))
        })?;

    if result.rows_affected() > 0 {
        tracing::info!(execution_id, "Alert marked as unread");
    }
    Ok(())
}

/// Soft-delete an alert (set `deleted_at` and `deleted_by`).
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
pub async fn restore_alert(
    db: &DbPool,
    execution_id: i32,
    workspace_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let sql = format!(
        r#"
        UPDATE watch_executions
        SET deleted_at = NULL, deleted_by = NULL
        WHERE id = $1
          AND workspace_id = $2
          AND alert_triggered = {true_val}
        "#,
        true_val = sql_compat::bool_true(is_pg),
    );

    let result = kyomi_core::db_execute!(db, &sql, execution_id, workspace_id)
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
          AND alert_triggered = {true_val}
          AND deleted_at IS NULL
        "#,
        now = sql_compat::now(is_pg),
        ids = placeholders.join(", "),
        true_val = sql_compat::bool_true(is_pg),
    );

    // Dynamic SQL with variable-length bind chain — need manual match dispatch
    let result = match db {
        DbPool::Postgres(pg) => {
            let mut query = sqlx::query(&sql).bind(workspace_id).bind(user_id);
            for id in execution_ids {
                query = query.bind(id);
            }
            query.execute(pg).await.map(kyomi_core::db::DbQueryResult::from_pg)
        }
        DbPool::Sqlite(sq) => {
            let mut query = sqlx::query(&sql).bind(workspace_id).bind(user_id);
            for id in execution_ids {
                query = query.bind(id);
            }
            query.execute(sq).await.map(kyomi_core::db::DbQueryResult::from_sqlite)
        }
    }
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to bulk delete alerts: {e}"))
    })?;

    let count = result.rows_affected();
    tracing::info!(count, "Bulk deleted alerts");
    Ok(count)
}

/// Mark multiple alerts as read at once. Returns the number of rows affected.
pub async fn bulk_mark_alerts_read(
    db: &DbPool,
    execution_ids: &[i32],
    workspace_id: &str,
) -> Result<u64> {
    if execution_ids.is_empty() {
        return Ok(0);
    }

    let is_pg = db.is_postgres();

    let placeholders: Vec<String> = (0..execution_ids.len())
        .map(|i| format!("${}", i + 2)) // $1 = workspace_id
        .collect();

    let sql = format!(
        r#"
        UPDATE watch_executions
        SET read_at = {now}
        WHERE id IN ({ids})
          AND workspace_id = $1
          AND alert_triggered = {true_val}
          AND read_at IS NULL
        "#,
        now = sql_compat::now(is_pg),
        ids = placeholders.join(", "),
        true_val = sql_compat::bool_true(is_pg),
    );

    // Dynamic SQL with variable-length bind chain — need manual match dispatch
    let result = match db {
        DbPool::Postgres(pg) => {
            let mut query = sqlx::query(&sql).bind(workspace_id);
            for id in execution_ids {
                query = query.bind(id);
            }
            query.execute(pg).await.map(kyomi_core::db::DbQueryResult::from_pg)
        }
        DbPool::Sqlite(sq) => {
            let mut query = sqlx::query(&sql).bind(workspace_id);
            for id in execution_ids {
                query = query.bind(id);
            }
            query.execute(sq).await.map(kyomi_core::db::DbQueryResult::from_sqlite)
        }
    }
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to bulk mark alerts read: {e}"))
    })?;

    let count = result.rows_affected();
    tracing::info!(count, "Bulk marked alerts as read");
    Ok(count)
}

/// Mark multiple alerts as unread at once. Returns the number of rows affected.
pub async fn bulk_mark_alerts_unread(
    db: &DbPool,
    execution_ids: &[i32],
    workspace_id: &str,
) -> Result<u64> {
    if execution_ids.is_empty() {
        return Ok(0);
    }

    let is_pg = db.is_postgres();

    let placeholders: Vec<String> = (0..execution_ids.len())
        .map(|i| format!("${}", i + 2)) // $1 = workspace_id
        .collect();

    let sql = format!(
        r#"
        UPDATE watch_executions
        SET read_at = NULL
        WHERE id IN ({ids})
          AND workspace_id = $1
          AND alert_triggered = {true_val}
          AND read_at IS NOT NULL
        "#,
        ids = placeholders.join(", "),
        true_val = sql_compat::bool_true(is_pg),
    );

    // Dynamic SQL with variable-length bind chain — need manual match dispatch
    let result = match db {
        DbPool::Postgres(pg) => {
            let mut query = sqlx::query(&sql).bind(workspace_id);
            for id in execution_ids {
                query = query.bind(id);
            }
            query.execute(pg).await.map(kyomi_core::db::DbQueryResult::from_pg)
        }
        DbPool::Sqlite(sq) => {
            let mut query = sqlx::query(&sql).bind(workspace_id);
            for id in execution_ids {
                query = query.bind(id);
            }
            query.execute(sq).await.map(kyomi_core::db::DbQueryResult::from_sqlite)
        }
    }
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to bulk mark alerts unread: {e}"))
    })?;

    let count = result.rows_affected();
    tracing::info!(count, "Bulk marked alerts as unread");
    Ok(count)
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
    fn tier_limits_are_correct() {
        use kyomi_core::SubscriptionTier::*;
        assert_eq!(watch_limit_for_tier(Free), 0);
        assert_eq!(watch_limit_for_tier(Pro), 10);
        assert_eq!(watch_limit_for_tier(Team), 50);
        assert_eq!(watch_limit_for_tier(Enterprise), 200);
        // Non-watch tiers default to free
        assert_eq!(watch_limit_for_tier(Starter), 0);
        assert_eq!(watch_limit_for_tier(Basic), 0);
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
