// SPDX-License-Identifier: AGPL-3.0-or-later

//! Standard numeric cron weekdays, shared by watch execution and descriptions.

/// Evaluate a weekday field in standard cron numbering (0/7 Sunday, 1 Monday).
/// Lists, inclusive ranges and steps are evaluated before Sunday aliases are
/// deduplicated. Wildcards span 0..=6; a stepped number spans that number..=7.
/// Named weekdays remain unsupported, matching the watch API's numeric format.
pub fn evaluate_weekdays(field: &str) -> Result<Vec<u32>, &'static str> {
    fn number(value: &str) -> Result<u32, &'static str> {
        if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) {
            return Err("day-of-week must use numbers from 0 to 7");
        }
        value
            .parse::<u32>()
            .ok()
            .filter(|&n| n <= 7)
            .ok_or("day-of-week must be between 0 and 7 (0 or 7 = Sunday)")
    }
    let mut selected = [false; 7];
    for item in field.split(',') {
        let mut step_parts = item.split('/');
        let base = step_parts.next().unwrap_or_default();
        let step = step_parts.next();
        if step_parts.next().is_some() {
            return Err("malformed day-of-week step");
        }
        let stride = match step {
            Some(s) if !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()) => s
                .parse::<usize>()
                .ok()
                .filter(|&s| s > 0)
                .ok_or("day-of-week step must be positive")?,
            Some(_) => return Err("day-of-week step must be positive"),
            None => 1,
        };
        let (start, end) = if base == "*" {
            (0, 6)
        } else if let Some((start, end)) = base.split_once('-') {
            (number(start)?, number(end)?)
        } else {
            let start = number(base)?;
            (start, if step.is_some() { 7 } else { start })
        };
        if start > end {
            return Err("day-of-week range must be ascending");
        }
        for day in (start..=end).step_by(stride) {
            selected[(day % 7) as usize] = true;
        }
    }
    Ok(selected
        .iter()
        .enumerate()
        .filter_map(|(day, &yes)| yes.then_some(day as u32))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluated_sets_preserve_standard_semantics() {
        for (field, days) in [
            ("0,1,7", vec![0, 1]),
            ("1-5", vec![1, 2, 3, 4, 5]),
            ("5-7", vec![0, 5, 6]),
            ("*", vec![0, 1, 2, 3, 4, 5, 6]),
            ("*/2", vec![0, 2, 4, 6]),
            ("1-7/2", vec![0, 1, 3, 5]),
            ("1/2", vec![0, 1, 3, 5]),
            ("0-7", vec![0, 1, 2, 3, 4, 5, 6]),
            ("0,7,0-7/7", vec![0]),
        ] {
            assert_eq!(evaluate_weekdays(field).unwrap(), days, "{field}");
        }
    }

    #[test]
    fn rejects_malformed_and_out_of_range_fields() {
        for field in [
            "", "8", "-1", "1-8", "7-1", "*/0", "1//2", "1/", "1,", "1--2", "MON", "**",
        ] {
            assert!(evaluate_weekdays(field).is_err(), "{field}");
        }
    }
}
