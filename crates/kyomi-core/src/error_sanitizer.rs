// SPDX-License-Identifier: AGPL-3.0-or-later

//! Error message sanitizer — strips credentials and internal network details
//! from error strings before they are sent to the frontend.
//!
//! Raw driver errors often contain passwords, internal hostnames, and private IP
//! addresses embedded in connection strings or URLs. This module provides a
//! single [`sanitize_error`] function that replaces all such patterns with
//! safe placeholder strings.
//!
//! The original error is never discarded — callers must log it server-side
//! before sanitizing.

use std::sync::OnceLock;

use regex::Regex;

// ---------------------------------------------------------------------------
// Compiled regex accessors (OnceLock per CODING_STANDARDS.md)
// ---------------------------------------------------------------------------

/// Matches full connection URLs for common datasource schemes.
///
/// Captures everything from the scheme up to the first whitespace or
/// closing parenthesis, which is the common delimiter in driver error messages.
fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Match scheme://... up to the first whitespace, closing paren, angle bracket, or quote.
        // Note: raw string literals cannot contain `\"`, so the quote characters are
        // expressed as a character class without backslash-escaping.
        Regex::new(
            "(?i)(https?|postgres(?:ql)?|mysql|rediss?|clickhouse)://[^\\s)>\"']+",
        )
        .expect("url_regex is valid")
    })
}

/// Matches standalone credential-like query parameters outside of URLs
/// (e.g. `password=secret` in a log line that has already had the URL removed,
/// or in non-URL formatted error text).
fn credential_param_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Match `key=value` where value runs until whitespace or common delimiters.
        // Using a regular string (not raw) so `\"` and `\'` can be expressed correctly.
        Regex::new(
            "(?i)(password|passwd|secret|api_key|apikey|api-key|token|authorization|access_key|private_key)=([^\\s&,;)>\"'\\]]+)",
        )
        .expect("credential_param_regex is valid")
    })
}

/// Matches internal Kubernetes service hostnames (`*.svc.cluster.local`)
/// and private IP addresses in the RFC 1918 ranges, with an optional port.
fn internal_host_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?x)
            # Kubernetes internal service hostname
            [\w.-]+\.svc\.cluster\.local(?::\d+)?
            |
            # RFC 1918 private IP addresses with optional port.
            # \b prevents matching a public IP that happens to contain a
            # private-range substring (e.g. 210.0.0.1) — digits are word
            # chars so there is no \b between '2' and '1'.
            \b(?:
                10\.\d{1,3}\.\d{1,3}\.\d{1,3}
                | 172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}
                | 192\.168\.\d{1,3}\.\d{1,3}
            )(?::\d+)?
            ",
        )
        .expect("internal_host_regex is valid")
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Strip sensitive data from a datasource driver error message.
///
/// Applies three passes in order:
/// 1. **URL redaction** — replaces any `scheme://...` connection string with
///    `[connection details redacted]`.
/// 2. **Credential parameter redaction** — replaces `password=<value>` (and
///    similar) with `password=[REDACTED]`.
/// 3. **Internal host redaction** — replaces Kubernetes `.svc.cluster.local`
///    hostnames and RFC 1918 private IP addresses with `[internal host]`.
///
/// The returned string is safe to send to the frontend. The raw error must
/// still be logged server-side before this function is called.
pub fn sanitize_error(message: &str) -> String {
    // Pass 1: redact full URLs
    let step1 = url_regex().replace_all(message, "[connection details redacted]");

    // Pass 2: redact standalone credential parameters
    let step2 = credential_param_regex().replace_all(&step1, "$1=[REDACTED]");

    // Pass 3: redact internal hostnames and private IPs
    let step3 = internal_host_regex().replace_all(&step2, "[internal host]");

    step3.into_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_clickhouse_url_with_password() {
        let input = "Data error: source 'source' fetch failed: Data error: query failed: \
                     query stream failed: ClickHouse HTTP request failed: \
                     error sending request for url \
                     (http://clickhouse.data-pipeline.svc.cluster.local:8123/\
                     ?database=product%5Fanalytics&user=default&default_format=JSONCompact\
                     &password=e1981792d847f6e2c0e254bc760d9750)";
        let output = sanitize_error(input);
        assert!(
            !output.contains("password="),
            "password should be redacted: {output}"
        );
        assert!(
            !output.contains("e1981792d847f6e2c0e254bc760d9750"),
            "password value should be redacted: {output}"
        );
        assert!(
            output.contains("[connection details redacted]"),
            "URL placeholder expected: {output}"
        );
    }

    #[test]
    fn redacts_postgres_connection_string() {
        let input =
            "connection error: postgres://admin:s3cr3tP@ss@db.internal:5432/prod?sslmode=require";
        let output = sanitize_error(input);
        assert!(
            !output.contains("s3cr3tP@ss"),
            "password should not appear: {output}"
        );
        assert!(
            output.contains("[connection details redacted]"),
            "URL placeholder expected: {output}"
        );
    }

    #[test]
    fn redacts_svc_cluster_local_hostname() {
        let input = "failed to connect to clickhouse.data-pipeline.svc.cluster.local:8123";
        let output = sanitize_error(input);
        assert!(
            !output.contains("svc.cluster.local"),
            "hostname should be redacted: {output}"
        );
        assert!(
            output.contains("[internal host]"),
            "internal host placeholder expected: {output}"
        );
    }

    #[test]
    fn redacts_private_ip_10_block() {
        let input = "connection refused: 10.43.22.129:5432";
        let output = sanitize_error(input);
        assert!(
            !output.contains("10.43.22.129"),
            "private IP should be redacted: {output}"
        );
        assert!(
            output.contains("[internal host]"),
            "internal host placeholder expected: {output}"
        );
    }

    #[test]
    fn redacts_private_ip_192_168_block() {
        let input = "TCP connect error: 192.168.1.200:3000";
        let output = sanitize_error(input);
        assert!(
            !output.contains("192.168.1.200"),
            "private IP should be redacted: {output}"
        );
        assert!(
            output.contains("[internal host]"),
            "internal host placeholder expected: {output}"
        );
    }

    #[test]
    fn redacts_standalone_password_param() {
        let input = "auth error: password=super_secret_value, host=db";
        let output = sanitize_error(input);
        assert!(
            !output.contains("super_secret_value"),
            "password value should be redacted: {output}"
        );
        assert!(
            output.contains("password=[REDACTED]"),
            "redacted placeholder expected: {output}"
        );
    }

    #[test]
    fn clean_error_passes_through_unchanged() {
        let input = "query execution failed: column 'foo' does not exist";
        let output = sanitize_error(input);
        assert_eq!(output, input, "clean error should pass through unchanged");
    }

    #[test]
    fn multiple_sensitive_patterns_all_redacted() {
        let input = "failed: postgres://user:hunter2@192.168.1.50:5432/mydb \
                     and also api_key=abc123 secret=xyz789";
        let output = sanitize_error(input);
        assert!(
            !output.contains("hunter2"),
            "password in URL should be redacted: {output}"
        );
        assert!(
            !output.contains("192.168.1.50"),
            "private IP should be redacted: {output}"
        );
        assert!(
            !output.contains("abc123"),
            "api_key value should be redacted: {output}"
        );
        assert!(
            !output.contains("xyz789"),
            "secret value should be redacted: {output}"
        );
    }

    #[test]
    fn empty_string_passes_through() {
        let output = sanitize_error("");
        assert_eq!(output, "", "empty string should pass through unchanged");
    }

    #[test]
    fn does_not_redact_public_ip_with_10_suffix() {
        let output = sanitize_error("connect failed: 210.0.0.1:443");
        assert!(
            output.contains("210.0.0.1"),
            "public IP should not be modified: {output}"
        );
    }

    #[test]
    fn does_not_redact_public_ip_with_172_prefix() {
        let output = sanitize_error("connect failed: 1172.16.0.1:443");
        assert!(
            output.contains("1172.16.0.1"),
            "public IP should not be modified: {output}"
        );
    }
}
