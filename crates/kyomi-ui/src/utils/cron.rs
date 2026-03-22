// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cron utilities for displaying UTC cron expressions in local time.
//!
//! Cron expressions are stored in UTC. This module converts them to
//! human-readable descriptions in the user's local timezone.
//!
//! Ported from `apps/frontend/src/utils/cronUtils.js`.

use crate::types::CronDescription;

/// Result of converting a UTC hour to local time.
#[derive(Clone, Debug)]
pub struct HourConversion {
    /// The converted hour (0-23).
    pub hour: u32,
    /// Day offset from conversion: -1 (previous day), 0 (same day), or +1 (next day).
    pub day_offset: i32,
}

/// Convert a UTC hour to local hour using the browser's timezone offset.
///
/// `tz_offset_minutes` is the value from JavaScript's `new Date().getTimezoneOffset()`,
/// which is positive for zones west of UTC and negative for zones east.
pub fn utc_to_local_hour(utc_hour: u32, tz_offset_minutes: i32) -> HourConversion {
    // JS getTimezoneOffset() returns positive for west, negative for east.
    // To convert UTC -> local: local = utc - offset_hours
    // (e.g., UTC+11 has offset=-660, so local = utc - (-11) = utc + 11)
    let offset_hours = tz_offset_minutes as f64 / 60.0;
    let local = utc_hour as f64 - offset_hours;

    let mut hour = local.floor() as i32;
    let mut day_offset = 0;

    if hour >= 24 {
        hour -= 24;
        day_offset = 1;
    } else if hour < 0 {
        hour += 24;
        day_offset = -1;
    }

    HourConversion {
        hour: hour as u32,
        day_offset,
    }
}

/// Convert a local hour to UTC hour.
///
/// `tz_offset_minutes` is the value from JavaScript's `new Date().getTimezoneOffset()`.
pub fn local_hour_to_utc(local_hour: u32, tz_offset_minutes: i32) -> HourConversion {
    let offset_hours = tz_offset_minutes as f64 / 60.0;
    let utc = local_hour as f64 + offset_hours;

    let mut hour = utc.floor() as i32;
    let mut day_offset = 0;

    if hour >= 24 {
        hour -= 24;
        day_offset = 1;
    } else if hour < 0 {
        hour += 24;
        day_offset = -1;
    }

    HourConversion {
        hour: hour as u32,
        day_offset,
    }
}

/// Day-of-week names indexed by cron value (0 = Sunday).
const WEEKDAYS: &[&str] = &[
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Format a time as "H:MM AM/PM".
fn format_time(hour: u32, minute: u32) -> String {
    let hour_12 = if hour == 0 {
        12
    } else if hour > 12 {
        hour - 12
    } else {
        hour
    };
    let ampm = if hour < 12 { "AM" } else { "PM" };
    format!("{hour_12}:{minute:02} {ampm}")
}

/// Validate that a field contains only valid cron characters.
fn is_valid_field(field: &str) -> bool {
    field
        .chars()
        .all(|c| c.is_ascii_digit() || c == ',' || c == '-' || c == '*' || c == '/')
}

/// Parse weekday numbers from a cron day-of-week field and adjust for timezone.
///
/// Returns day names adjusted by the day offset from UTC-to-local conversion.
fn parse_weekdays(field: &str, utc_hour: u32, tz_offset_minutes: i32) -> Option<Vec<String>> {
    if field == "*" {
        return None;
    }
    let conversion = utc_to_local_hour(utc_hour, tz_offset_minutes);
    let day_offset = conversion.day_offset;

    let mut days = Vec::new();
    for range in field.split(',') {
        if range.contains('-') {
            let parts: Vec<&str> = range.split('-').collect();
            if parts.len() != 2 {
                continue;
            }
            if let (Ok(start), Ok(end)) = (parts[0].parse::<i32>(), parts[1].parse::<i32>()) {
                for i in start..=end {
                    let adjusted = ((i + day_offset) % 7 + 7) % 7;
                    if let Some(name) = WEEKDAYS.get(adjusted as usize) {
                        days.push((*name).to_string());
                    }
                }
            }
        } else if let Ok(day_num) = range.parse::<i32>() {
            let adjusted = ((day_num + day_offset) % 7 + 7) % 7;
            if let Some(name) = WEEKDAYS.get(adjusted as usize) {
                days.push((*name).to_string());
            }
        }
    }

    Some(days)
}

/// Generate a human-readable description of a cron expression.
///
/// Converts UTC times to local for display using the provided timezone offset.
///
/// `tz_offset_minutes` is the value from JavaScript's `new Date().getTimezoneOffset()`.
pub fn describe_cron(cron_expr: &str, tz_offset_minutes: i32) -> CronDescription {
    let cron_expr = cron_expr.trim();

    if cron_expr.is_empty() {
        return CronDescription {
            valid: false,
            description: "Invalid cron expression".into(),
        };
    }

    let parts: Vec<&str> = cron_expr.split_whitespace().collect();
    if parts.len() != 5 {
        return CronDescription {
            valid: false,
            description: "Cron expression must have 5 fields: minute hour day month weekday"
                .into(),
        };
    }

    let (minute, hour, day_of_month, _month, day_of_week) =
        (parts[0], parts[1], parts[2], parts[3], parts[4]);

    // Validate basic structure
    if !parts.iter().all(|f| is_valid_field(f)) {
        return CronDescription {
            valid: false,
            description: "Invalid characters in cron expression".into(),
        };
    }

    let description = describe_cron_inner(
        minute,
        hour,
        day_of_month,
        _month,
        day_of_week,
        tz_offset_minutes,
    );

    CronDescription {
        valid: true,
        description,
    }
}

/// Inner logic for building the cron description.
fn describe_cron_inner(
    minute: &str,
    hour: &str,
    day_of_month: &str,
    _month: &str,
    day_of_week: &str,
    tz_offset_minutes: i32,
) -> String {
    // Every hour at specific minute
    if minute != "*"
        && hour == "*"
        && day_of_month == "*"
        && _month == "*"
        && day_of_week == "*"
    {
        return format!("Every hour at minute {minute}");
    }

    // Step syntax for hours (e.g., */2 = every 2 hours)
    if minute != "*"
        && hour.starts_with("*/")
        && day_of_month == "*"
        && _month == "*"
        && day_of_week == "*"
    {
        if let (Ok(m), Some(step_str)) = (minute.parse::<u32>(), hour.strip_prefix("*/")) {
            if let Ok(step) = step_str.parse::<u32>() {
                return format!("Every {step} hours at :{m:02}");
            }
        }
    }

    // Specific hours (hourly with selected hours, e.g., "0 9,17 * * *")
    if minute != "*"
        && hour != "*"
        && hour.contains(',')
        && day_of_month == "*"
        && _month == "*"
        && day_of_week == "*"
    {
        if let Ok(m) = minute.parse::<u32>() {
            let hours: Vec<u32> = hour
                .split(',')
                .filter_map(|h| h.trim().parse::<u32>().ok())
                .collect();
            let times: Vec<String> = hours
                .iter()
                .map(|&h| {
                    let local = utc_to_local_hour(h, tz_offset_minutes);
                    format_time(local.hour, m)
                })
                .collect();
            if times.len() <= 3 {
                return format!("Daily at {}", times.join(", "));
            } else {
                return format!(
                    "Daily at {} times: {}, {}, ... {}",
                    times.len(),
                    times[0],
                    times[1],
                    times[times.len() - 1]
                );
            }
        }
    }

    // Daily at specific time
    if minute != "*"
        && hour != "*"
        && !hour.contains(',')
        && !hour.starts_with("*/")
        && day_of_month == "*"
        && _month == "*"
        && day_of_week == "*"
    {
        if let (Ok(h), Ok(m)) = (hour.parse::<u32>(), minute.parse::<u32>()) {
            let local = utc_to_local_hour(h, tz_offset_minutes);
            return format!("Daily at {}", format_time(local.hour, m));
        }
    }

    // Weekly on specific days
    if minute != "*"
        && hour != "*"
        && !hour.contains(',')
        && !hour.starts_with("*/")
        && day_of_month == "*"
        && _month == "*"
        && day_of_week != "*"
    {
        if let (Ok(h), Ok(m)) = (hour.parse::<u32>(), minute.parse::<u32>()) {
            let local = utc_to_local_hour(h, tz_offset_minutes);
            let time_str = format_time(local.hour, m);

            if let Some(days) = parse_weekdays(day_of_week, h, tz_offset_minutes) {
                if !days.is_empty() {
                    // Check for weekdays (5 days, no Saturday or Sunday)
                    if days.len() == 5
                        && !days.contains(&"Saturday".to_string())
                        && !days.contains(&"Sunday".to_string())
                    {
                        return format!("Weekdays at {time_str}");
                    }
                    // Check for weekends
                    if days.len() == 2
                        && days.contains(&"Saturday".to_string())
                        && days.contains(&"Sunday".to_string())
                    {
                        return format!("Weekends at {time_str}");
                    }
                    return format!("{} at {time_str}", days.join(", "));
                }
            }
        }
    }

    // Monthly on specific day
    if minute != "*"
        && hour != "*"
        && !hour.contains(',')
        && !hour.starts_with("*/")
        && day_of_month != "*"
        && _month == "*"
        && day_of_week == "*"
    {
        if let (Ok(h), Ok(m), Ok(dom)) = (
            hour.parse::<u32>(),
            minute.parse::<u32>(),
            day_of_month.parse::<i32>(),
        ) {
            let local = utc_to_local_hour(h, tz_offset_minutes);
            let time_str = format_time(local.hour, m);

            // Adjust day for timezone crossing
            let mut day = dom - local.day_offset;
            day = day.clamp(1, 28);

            let suffix = match (day % 10, day % 100) {
                (_, 11..=13) => "th",
                (1, _) => "st",
                (2, _) => "nd",
                (3, _) => "rd",
                _ => "th",
            };
            return format!("Monthly on the {day}{suffix} at {time_str}");
        }
    }

    // Complex expression - show raw breakdown
    let time_desc = if hour == "*" {
        "every hour".to_string()
    } else {
        format!("at hour {hour} UTC")
    };
    let min_desc = if minute == "*" {
        "every minute".to_string()
    } else {
        format!("minute {minute}")
    };
    let day_desc = if day_of_month == "*" {
        "every day".to_string()
    } else {
        format!("day {day_of_month}")
    };
    let month_desc = if _month == "*" {
        String::new()
    } else {
        format!("in month {_month}")
    };
    let week_desc = if day_of_week == "*" {
        String::new()
    } else {
        format!("on weekday {day_of_week}")
    };

    let mut parts = vec![
        format!("Runs {min_desc}"),
        time_desc,
        day_desc,
    ];
    if !month_desc.is_empty() {
        parts.push(month_desc);
    }
    if !week_desc.is_empty() {
        parts.push(week_desc);
    }

    parts.join(", ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utc_to_local_hour_positive_offset() {
        // UTC+11 (e.g., Sydney) — offset = -660
        let result = utc_to_local_hour(9, -660);
        assert_eq!(result.hour, 20);
        assert_eq!(result.day_offset, 0);
    }

    #[test]
    fn test_utc_to_local_hour_day_rollover() {
        // UTC+11, 15:00 UTC -> 02:00 next day local
        let result = utc_to_local_hour(15, -660);
        assert_eq!(result.hour, 2);
        assert_eq!(result.day_offset, 1);
    }

    #[test]
    fn test_utc_to_local_hour_negative_offset() {
        // UTC-8 (e.g., PST) — offset = 480
        let result = utc_to_local_hour(9, 480);
        assert_eq!(result.hour, 1);
        assert_eq!(result.day_offset, 0);
    }

    #[test]
    fn test_utc_to_local_hour_day_rollback() {
        // UTC-8, 3:00 UTC -> 19:00 previous day local
        let result = utc_to_local_hour(3, 480);
        assert_eq!(result.hour, 19);
        assert_eq!(result.day_offset, -1);
    }

    #[test]
    fn test_local_hour_to_utc() {
        // UTC+11: local 20:00 -> UTC 09:00
        let result = local_hour_to_utc(20, -660);
        assert_eq!(result.hour, 9);
        assert_eq!(result.day_offset, 0);
    }

    #[test]
    fn test_describe_cron_daily() {
        // UTC timezone (offset = 0)
        let result = describe_cron("0 9 * * *", 0);
        assert!(result.valid);
        assert_eq!(result.description, "Daily at 9:00 AM");
    }

    #[test]
    fn test_describe_cron_weekdays() {
        // UTC timezone
        let result = describe_cron("0 9 * * 1-5", 0);
        assert!(result.valid);
        assert_eq!(result.description, "Weekdays at 9:00 AM");
    }

    #[test]
    fn test_describe_cron_hourly() {
        let result = describe_cron("30 * * * *", 0);
        assert!(result.valid);
        assert_eq!(result.description, "Every hour at minute 30");
    }

    #[test]
    fn test_describe_cron_invalid_fields() {
        let result = describe_cron("0 9 * *", 0);
        assert!(!result.valid);
    }

    #[test]
    fn test_describe_cron_invalid_chars() {
        let result = describe_cron("0 9 * * MON", 0);
        assert!(!result.valid);
    }

    #[test]
    fn test_describe_cron_monthly() {
        let result = describe_cron("0 9 1 * *", 0);
        assert!(result.valid);
        assert_eq!(result.description, "Monthly on the 1st at 9:00 AM");
    }

    #[test]
    fn test_describe_cron_empty() {
        let result = describe_cron("", 0);
        assert!(!result.valid);
    }
}
