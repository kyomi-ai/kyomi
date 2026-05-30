// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared retry utility with exponential backoff and jitter.
//!
//! Provides two functions:
//! - [`retry_with_backoff`] — for operations returning [`crate::Error`], using
//!   [`crate::Error::is_transient`] to classify errors automatically.
//! - [`retry_with_backoff_classified`] — for operations returning any error type,
//!   with a caller-supplied `is_transient` predicate.
//!
//! Both use exponential backoff with full jitter with a maximum of
//! [`MAX_RETRIES`] retry attempts. Each retry attempt is logged at `warn` level
//! with the attempt number.

use rand::Rng;
use std::time::Duration;
use tracing::warn;

use crate::Result;

/// Maximum number of retry attempts for transient errors (not counting the
/// initial attempt). Total call count = 1 + MAX_RETRIES.
pub const MAX_RETRIES: u32 = 10;

/// Initial wait interval before the first retry (seconds).
const INITIAL_INTERVAL_SECS: f64 = 1.0;

/// Maximum wait interval between retries (caps exponential growth).
const MAX_INTERVAL: Duration = Duration::from_secs(30);

/// Multiplier applied to the interval on each retry (2× = double each time).
const MULTIPLIER: f64 = 2.0;

/// Compute the delay for a given retry index (0-based: 0 = first retry).
///
/// Uses exponential backoff with ±50% jitter:
/// `delay = clamp(initial * multiplier^attempt, max) * jitter_factor`
/// where `jitter_factor` is uniformly random in [0.5, 1.5].
fn backoff_delay(retry_index: u32) -> Duration {
    let base = INITIAL_INTERVAL_SECS * MULTIPLIER.powi(retry_index as i32);
    let capped = base.min(MAX_INTERVAL.as_secs_f64());
    let jitter: f64 = rand::rng().random_range(0.5..=1.5);
    let secs = capped * jitter;
    Duration::from_secs_f64(secs.min(MAX_INTERVAL.as_secs_f64()))
}

/// Retry an async operation with exponential backoff and jitter.
///
/// Uses [`crate::Error::is_transient`] to classify errors automatically.
/// Transient errors ([`crate::Error::TooManyRequests`] and
/// [`crate::Error::ServiceUnavailable`]) are retried up to [`MAX_RETRIES`]
/// times. All other errors cause immediate failure.
///
/// Each retry attempt is logged at `warn` level with the attempt number.
///
/// # Example
///
/// ```ignore
/// use kyomi_core::retry::retry_with_backoff;
///
/// let result = retry_with_backoff(|| async {
///     make_http_call().await
/// }).await;
/// ```
pub async fn retry_with_backoff<F, Fut, T>(operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    retry_with_backoff_classified(operation, |e: &crate::Error| e.is_transient()).await
}

/// Retry an async operation with exponential backoff, using a caller-supplied
/// error classifier.
///
/// This is the lower-level primitive used by [`retry_with_backoff`] and by
/// callers that work with error types other than [`crate::Error`] (e.g. SMTP
/// errors from lettre, or Stripe errors from async-stripe).
///
/// `is_transient` returns `true` for errors that should be retried and `false`
/// for errors that are permanent (authentication failures, invalid input, etc.).
///
/// # Example
///
/// ```ignore
/// use kyomi_core::retry::retry_with_backoff_classified;
///
/// let result = retry_with_backoff_classified(
///     || async { smtp_transport.send(message.clone()).await },
///     |e: &SmtpError| is_smtp_transient(e),
/// ).await;
/// ```
pub async fn retry_with_backoff_classified<F, Fut, T, E, C>(
    operation: F,
    is_transient: C,
) -> std::result::Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
    E: std::fmt::Display,
    C: Fn(&E) -> bool,
{
    let mut attempt: u32 = 0;

    loop {
        attempt += 1;

        match operation().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                if !is_transient(&e) {
                    return Err(e);
                }

                // Retries are counted starting from the second call, so the
                // retry index is `attempt - 1`.
                let retry_index = attempt - 1;

                if retry_index >= MAX_RETRIES {
                    return Err(e);
                }

                let delay = backoff_delay(retry_index);

                warn!(
                    attempt,
                    max_retries = MAX_RETRIES,
                    delay_ms = delay.as_millis(),
                    error = %e,
                    "transient error, retrying with backoff"
                );

                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// Returns `true` if the HTTP status code indicates a transient server-side
/// error that should be retried.
///
/// Retryable status codes:
/// - `429` Too Many Requests (rate limiting)
/// - `502` Bad Gateway (upstream failure)
/// - `503` Service Unavailable (temporary outage)
/// - `504` Gateway Timeout (upstream timeout)
///
/// Status codes like `400`, `401`, `403`, `404` are permanent errors that
/// will produce the same result on every attempt and must not be retried.
pub fn is_transient_http_status(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn success_on_first_attempt_no_retries() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let result: Result<u32> = retry_with_backoff(|| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::SeqCst), 1, "called exactly once");
    }

    #[tokio::test(start_paused = true)]
    async fn transient_failure_then_success() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        // start_paused = true: tokio time is frozen; tokio::time::sleep returns
        // immediately, keeping the test fast while still exercising the retry path.
        let result: Result<&str> = retry_with_backoff(|| {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(crate::Error::TooManyRequests("rate limited".to_string(), 0))
                } else {
                    Ok("ok")
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "first attempt failed, second succeeded"
        );
    }

    #[tokio::test]
    async fn non_retryable_error_fails_immediately() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let result: Result<u32> = retry_with_backoff(|| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(crate::Error::Unauthorized("invalid key".to_string()))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "permanent error must not be retried"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn max_retries_exhausted_returns_last_error() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let result: crate::Result<u32> = retry_with_backoff_classified(
            || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(crate::Error::ServiceUnavailable("down".to_string()))
                }
            },
            |e: &crate::Error| e.is_transient(),
        )
        .await;

        assert!(result.is_err());
        // Initial attempt + MAX_RETRIES retries.
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1 + MAX_RETRIES,
            "should attempt once plus MAX_RETRIES retries"
        );
    }

    #[test]
    fn is_transient_http_status_retryable() {
        assert!(is_transient_http_status(429));
        assert!(is_transient_http_status(502));
        assert!(is_transient_http_status(503));
        assert!(is_transient_http_status(504));
    }

    #[test]
    fn is_transient_http_status_non_retryable() {
        assert!(!is_transient_http_status(400));
        assert!(!is_transient_http_status(401));
        assert!(!is_transient_http_status(403));
        assert!(!is_transient_http_status(404));
        assert!(!is_transient_http_status(200));
        assert!(!is_transient_http_status(500));
        assert!(!is_transient_http_status(501));
    }
}
