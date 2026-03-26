// SPDX-License-Identifier: AGPL-3.0-or-later

//! Python-compatible d3-format number formatting.
//!
//! Covers the subset of d3-format specifiers used in ChartML metric cards:
//! - Fixed-point: `.0f`, `.1f`, `.2f` (with optional comma grouping)
//! - Percentage: `.0%`, `.1%`
//! - SI prefix: `~s` (1.2K, 3.4M, 1.2B)
//! - Currency prefix: `$`
//! - Comma grouping: `,`
//!
//! Ports Python's `d3_format.py`.

use std::sync::LazyLock;

/// SI suffixes for `~s` format.
const SI_PREFIXES: &[(f64, &str)] = &[
    (1e15, "P"),
    (1e12, "T"),
    (1e9, "B"),
    (1e6, "M"),
    (1e3, "K"),
];

/// Regex to parse d3-format specifiers.
///
/// Captures: `[$][,][.precision][f|%|s|~s]`
static FORMAT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^(?P<currency>\$)?(?P<comma>,)?(?:\.(?P<precision>\d+))?(?P<type>[f%s~]|~s)?$",
    )
    .expect("valid regex")
});

/// Format a numeric value using a d3-format specifier string.
///
/// Returns `"N/A"` for `None`, NaN, or infinity.
/// Returns `str(value)` for empty/unrecognized format.
pub fn format_d3(value: Option<&serde_json::Value>, fmt: Option<&str>) -> String {
    let val = match value {
        Some(v) => v,
        None => return "N/A".into(),
    };

    // JSON null → N/A (matches Python's None handling)
    if val.is_null() {
        return "N/A".into();
    }

    // Try to extract a numeric value
    let numeric = if let Some(n) = val.as_f64() {
        n
    } else if let Some(s) = val.as_str() {
        match s.parse::<f64>() {
            Ok(n) => n,
            Err(_) => return s.to_string(),
        }
    } else if let Some(i) = val.as_i64() {
        i as f64
    } else {
        return val.to_string();
    };

    if numeric.is_nan() || numeric.is_infinite() {
        return "N/A".into();
    }

    let fmt = match fmt {
        Some(f) if !f.is_empty() => f,
        _ => return format_number_plain(numeric),
    };

    let m = match FORMAT_RE.captures(fmt) {
        Some(caps) => caps,
        None => return format_number_plain(numeric),
    };

    let currency = m.name("currency").map(|_| "$").unwrap_or("");
    let use_comma = m.name("comma").is_some();
    let precision: Option<usize> = m
        .name("precision")
        .and_then(|p| p.as_str().parse().ok());
    let fmt_type = m.name("type").map(|t| t.as_str()).unwrap_or("");

    // SI prefix format (~s or s)
    if fmt_type == "~s" || fmt_type == "s" {
        return format!("{currency}{}", format_si(numeric, precision));
    }

    // Percentage format
    if fmt_type == "%" {
        let pct_value = numeric * 100.0;
        let prec = precision.unwrap_or(0);
        let mut formatted = format!("{pct_value:.prec$}", prec = prec);
        if use_comma {
            formatted = add_commas(&formatted);
        }
        return format!("{currency}{formatted}%");
    }

    // Fixed-point format (or default when type is empty / "f")
    let prec = precision.unwrap_or(0);
    let mut formatted = format!("{numeric:.prec$}", prec = prec);
    if use_comma {
        formatted = add_commas(&formatted);
    }
    format!("{currency}{formatted}")
}

/// Format a number with SI prefix (K, M, B, T, P).
///
/// Strips trailing zeros per d3-format `~s` convention.
fn format_si(value: f64, precision: Option<usize>) -> String {
    let prec = precision.unwrap_or(1);
    let abs_value = value.abs();
    let sign = if value < 0.0 { "-" } else { "" };

    for &(threshold, suffix) in SI_PREFIXES {
        if abs_value >= threshold {
            let scaled = abs_value / threshold;
            let formatted = format!("{scaled:.prec$}", prec = prec);
            let formatted = strip_trailing_zeros(&formatted);
            return format!("{sign}{formatted}{suffix}");
        }
    }

    // Below 1000 — no suffix
    let formatted = format!("{abs_value:.prec$}", prec = prec);
    let formatted = strip_trailing_zeros(&formatted);
    format!("{sign}{formatted}")
}

/// Strip trailing zeros (and trailing dot) from a formatted number.
fn strip_trailing_zeros(s: &str) -> String {
    if s.contains('.') {
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        trimmed.to_string()
    } else {
        s.to_string()
    }
}

/// Add comma grouping to the integer part of a formatted number.
fn add_commas(formatted: &str) -> String {
    let (integer_part, decimal_part) = if let Some(dot_pos) = formatted.find('.') {
        (&formatted[..dot_pos], Some(&formatted[dot_pos..]))
    } else {
        (formatted, None)
    };

    // Handle negative numbers
    let (sign, digits) = if let Some(stripped) = integer_part.strip_prefix('-') {
        ("-", stripped)
    } else {
        ("", integer_part)
    };

    // Insert commas every 3 digits from the right
    let mut result = String::new();
    for (i, ch) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    let grouped: String = result.chars().rev().collect();

    match decimal_part {
        Some(dec) => format!("{sign}{grouped}{dec}"),
        None => format!("{sign}{grouped}"),
    }
}

/// Plain number formatting (no format specifier).
fn format_number_plain(n: f64) -> String {
    // If it's a whole number, format without decimal
    if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fmt(val: f64, spec: &str) -> String {
        format_d3(Some(&json!(val)), Some(spec))
    }

    fn fmt_val(val: serde_json::Value, spec: &str) -> String {
        format_d3(Some(&val), Some(spec))
    }

    // -- Fixed-point --

    #[test]
    fn fixed_point_no_decimals() {
        assert_eq!(fmt(42.7, ".0f"), "43");
    }

    #[test]
    fn fixed_point_one_decimal() {
        assert_eq!(fmt(42.75, ".1f"), "42.8");
    }

    #[test]
    fn fixed_point_two_decimals() {
        assert_eq!(fmt(42.0, ".2f"), "42.00");
    }

    // -- Comma grouping --

    #[test]
    fn comma_fixed() {
        assert_eq!(fmt(1234567.89, ",.2f"), "1,234,567.89");
    }

    #[test]
    fn comma_no_decimals() {
        assert_eq!(fmt(1000.0, ",.0f"), "1,000");
    }

    // -- Currency --

    #[test]
    fn currency_comma_fixed() {
        assert_eq!(fmt(1234567.89, "$,.2f"), "$1,234,567.89");
    }

    #[test]
    fn currency_no_decimals() {
        assert_eq!(fmt(42000.0, "$,.0f"), "$42,000");
    }

    // -- Percentage --

    #[test]
    fn percentage_no_decimals() {
        assert_eq!(fmt(0.156, ".0%"), "16%");
    }

    #[test]
    fn percentage_one_decimal() {
        assert_eq!(fmt(0.156, ".1%"), "15.6%");
    }

    // -- SI prefix --

    #[test]
    fn si_thousands() {
        assert_eq!(fmt(1200.0, "~s"), "1.2K");
    }

    #[test]
    fn si_millions() {
        assert_eq!(fmt(3400000.0, "~s"), "3.4M");
    }

    #[test]
    fn si_billions() {
        assert_eq!(fmt(1200000000.0, "~s"), "1.2B");
    }

    #[test]
    fn si_below_thousand() {
        assert_eq!(fmt(42.0, "~s"), "42");
    }

    #[test]
    fn si_with_currency() {
        assert_eq!(fmt(1200000.0, "$~s"), "$1.2M");
    }

    #[test]
    fn si_negative() {
        assert_eq!(fmt(-3400000.0, "~s"), "-3.4M");
    }

    #[test]
    fn si_strips_trailing_zeros() {
        assert_eq!(fmt(1000.0, "~s"), "1K");
        assert_eq!(fmt(2000000.0, "~s"), "2M");
    }

    // -- Edge cases --

    #[test]
    fn none_returns_na() {
        assert_eq!(format_d3(None, Some(".0f")), "N/A");
    }

    #[test]
    fn nan_returns_na() {
        // NaN/Infinity serialize to JSON null, so they become None → "N/A"
        assert_eq!(format_d3(Some(&json!(null)), Some(".0f")), "N/A");
    }

    #[test]
    fn infinity_returns_na() {
        assert_eq!(format_d3(Some(&json!(null)), Some(".0f")), "N/A");
    }

    #[test]
    fn nan_string_returns_na() {
        // "NaN" parses as f64::NAN in Rust, so it also returns N/A
        let val = serde_json::Value::String("NaN".into());
        assert_eq!(format_d3(Some(&val), Some(".0f")), "N/A");
    }

    #[test]
    fn no_format_returns_plain() {
        assert_eq!(format_d3(Some(&json!(42.0)), None), "42");
        assert_eq!(format_d3(Some(&json!(42.5)), None), "42.5");
    }

    #[test]
    fn empty_format_returns_plain() {
        assert_eq!(format_d3(Some(&json!(42.0)), Some("")), "42");
    }

    #[test]
    fn string_value_parsed() {
        assert_eq!(fmt_val(json!("1234.5"), ",.1f"), "1,234.5");
    }

    #[test]
    fn non_numeric_string_returned_as_is() {
        assert_eq!(fmt_val(json!("hello"), ".0f"), "hello");
    }

    // -- add_commas --

    #[test]
    fn commas_small_number() {
        assert_eq!(add_commas("42"), "42");
    }

    #[test]
    fn commas_thousands() {
        assert_eq!(add_commas("1234"), "1,234");
    }

    #[test]
    fn commas_millions() {
        assert_eq!(add_commas("1234567"), "1,234,567");
    }

    #[test]
    fn commas_with_decimal() {
        assert_eq!(add_commas("1234567.89"), "1,234,567.89");
    }

    #[test]
    fn commas_negative() {
        assert_eq!(add_commas("-1234567"), "-1,234,567");
    }
}
