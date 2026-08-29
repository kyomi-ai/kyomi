// SPDX-License-Identifier: AGPL-3.0-or-later

// Pure logic shared between `build.rs` and `src/lib.rs`'s test module.
//
// `build.rs` is never a `cargo test` target — Cargo compiles and runs it
// only as the build-script binary, so a `#[cfg(test)] mod` written
// directly inside `build.rs` is silently never executed by `cargo test`
// (verified empirically for this crate; KYO-510). To get real coverage
// without duplicating the logic, this file holds the pure,
// network-free argument-construction code and is pulled into both
// `build.rs` (via `include!`, so the build script uses it unmodified) and
// `src/lib.rs`'s `#[cfg(test)]` module (via `#[path]`, so `cargo test`
// actually exercises the same source `build.rs` runs).

/// Number of retries `curl` performs after an initial failed attempt
/// (KYO-510). HuggingFace occasionally resets the connection mid-transfer
/// (`curl: (35) Recv failure: Connection reset by peer`) with no code-side
/// cause — see the panic message in `build.rs` for what a reader should
/// conclude when they still see this after retrying.
const CURL_RETRY_COUNT: u32 = 3;

/// Fixed delay between retries, in seconds. `--retry-delay` disables curl's
/// default exponential backoff (1s, 2s, 4s, ... up to 10 minutes) in favor
/// of this fixed wait, which is enough for a transient reset to clear
/// without materially slowing a healthy build.
const CURL_RETRY_DELAY_SECS: u32 = 2;

/// TCP connect timeout, in seconds. Bounds how long a single attempt waits
/// to establish a connection before curl gives up on it (and, with
/// `--retry`, tries again) rather than hanging the build indefinitely.
const CURL_CONNECT_TIMEOUT_SECS: u32 = 10;

/// Per-attempt time budget, in seconds. `man curl` on `-m, --max-time`:
/// "Set the maximum time in seconds that you allow **each transfer** to
/// take. ... If you enable retrying the transfer (--retry) then the
/// maximum time counter is reset each time the transfer is retried."
/// So this bounds *one* attempt, not the download as a whole — a stalled
/// (not merely failed) connection gets cut off and counted as one retry,
/// same as any other failure. 120s is generous for `model.safetensors`
/// (~130MB) on a GitHub-hosted runner's bandwidth; a real attempt that is
/// still crawling at 120s is indistinguishable from a stalled one and
/// should be cut loose rather than allowed to sit. The *cumulative* bound
/// across all attempts is `CURL_RETRY_MAX_TIME_SECS` below — getting this
/// distinction backwards previously understated how long one file's
/// download could actually run (up to `CURL_MAX_TIME_SECS` per attempt,
/// reset on every retry, not once for the whole file; KYO-510 review).
const CURL_MAX_TIME_SECS: u32 = 120;

/// Cumulative time budget, in seconds, for *all* attempts at one file
/// combined — this is the flag actually built for that job. `man curl` on
/// `--retry-max-time`: "The retry timer is reset before the first transfer
/// attempt. Retries are done as usual (see --retry) as long as the timer
/// has not reached this given limit." Without it, `--max-time` alone lets
/// a stalling-but-not-failing connection burn
/// `(1 + CURL_RETRY_COUNT) * CURL_MAX_TIME_SECS` on a single file — with
/// the values above, up to 480s — and up to roughly three times that
/// across all three model files downloaded sequentially, inside a Clippy
/// job whose `timeout-minutes` is 75. 600s (10 minutes) per file keeps the
/// worst case for all three files at ~30 minutes, comfortably inside that
/// budget alongside the rest of the job's compile work, while remaining
/// generous enough for a genuine cold fetch of the largest file on a
/// congested runner. Per the man page's own caveat, this is an
/// *approximate* bound, not a hard one: a request already in flight when
/// the timer expires is allowed to finish rather than being killed
/// mid-transfer, so the real worst case can exceed this by up to one
/// attempt's `--connect-timeout` plus however long that final transfer
/// takes.
const CURL_RETRY_MAX_TIME_SECS: u32 = 600;

/// Build the `curl` argument list for downloading `url` to `dest`.
fn curl_args(url: &str, dest: &str) -> Vec<String> {
    vec![
        "--fail".to_string(),
        "--silent".to_string(),
        "--show-error".to_string(),
        "--location".to_string(),
        // `--retry` alone only covers curl's definition of "transient"
        // (timeouts and a handful of HTTP 4xx/5xx codes) — NOT
        // `curl: (35)` connection-reset errors, which is exactly what
        // broke PR #407's CI run. `--retry-all-errors` is what makes the
        // retry apply to this failure at all.
        "--retry".to_string(),
        CURL_RETRY_COUNT.to_string(),
        "--retry-delay".to_string(),
        CURL_RETRY_DELAY_SECS.to_string(),
        // Kept alongside `--retry-all-errors` even though the latter is a
        // documented superset ("Retry on any error") that already covers
        // ECONNREFUSED. This one names a specific, well-understood retry
        // condition explicitly rather than relying entirely on the
        // "sledgehammer" flag's exact scope — cheap, harmless redundancy,
        // not duplicated logic. See its own test below for the same
        // reasoning applied to coverage.
        "--retry-connrefused".to_string(),
        "--retry-all-errors".to_string(),
        "--connect-timeout".to_string(),
        CURL_CONNECT_TIMEOUT_SECS.to_string(),
        "--max-time".to_string(),
        CURL_MAX_TIME_SECS.to_string(),
        // Bounds the sum of all attempts for this one file — see
        // CURL_RETRY_MAX_TIME_SECS's doc comment for why this, and not
        // --max-time, is what actually keeps a stalling connection from
        // turning a rare flake into a CI hang.
        "--retry-max-time".to_string(),
        CURL_RETRY_MAX_TIME_SECS.to_string(),
        "--output".to_string(),
        dest.to_string(),
        url.to_string(),
    ]
}

#[cfg(test)]
mod curl_args_tests {
    use super::*;

    #[test]
    fn curl_args_includes_all_errors_not_just_retry() {
        // Regression guard for the exact gap this fix closes: plain
        // `--retry` does not cover `curl: (35)` connection-reset errors,
        // so `--retry-all-errors` must be present for the retry to ever
        // engage on the failure that actually happened in CI.
        let args = curl_args("https://example.com/f", "/tmp/f");
        assert!(args.contains(&"--retry-all-errors".to_string()));
        assert!(args.contains(&"--retry".to_string()));
        assert!(args.contains(&CURL_RETRY_COUNT.to_string()));
    }

    #[test]
    fn curl_args_retries_connection_refused() {
        // Deliberately redundant with --retry-all-errors (see the comment
        // at this flag's call site) — kept as an explicit, independently
        // readable guarantee rather than relying solely on the
        // "sledgehammer" flag's documented scope.
        let args = curl_args("https://example.com/f", "/tmp/f");
        assert!(args.contains(&"--retry-connrefused".to_string()));
    }

    #[test]
    fn curl_args_bounds_connect_and_per_attempt_time() {
        let args = curl_args("https://example.com/f", "/tmp/f");
        assert!(args.contains(&"--connect-timeout".to_string()));
        assert!(args.contains(&CURL_CONNECT_TIMEOUT_SECS.to_string()));
        assert!(args.contains(&"--max-time".to_string()));
        assert!(args.contains(&CURL_MAX_TIME_SECS.to_string()));
    }

    #[test]
    fn curl_args_bounds_cumulative_retry_time() {
        // Regression guard for the second curl-semantics gap found in
        // review: `--max-time` is a *per-attempt* bound that `--retry`
        // resets on every retry, so without `--retry-max-time` a stalling
        // connection can burn (1 + CURL_RETRY_COUNT) * CURL_MAX_TIME_SECS
        // on a single file instead of being cut off. This is the flag
        // that actually caps the total — do not "simplify" it away as
        // apparently redundant with --max-time.
        let args = curl_args("https://example.com/f", "/tmp/f");
        assert!(args.contains(&"--retry-max-time".to_string()));
        assert!(args.contains(&CURL_RETRY_MAX_TIME_SECS.to_string()));
    }

    #[test]
    fn curl_args_preserves_url_and_destination() {
        let args = curl_args("https://example.com/model.json", "/out/model.json");
        // `--output <dest>` must be immediately followed by dest, and the
        // URL must be the final, un-mangled argument.
        let output_idx = args
            .iter()
            .position(|a| a == "--output")
            .expect("--output flag missing");
        assert_eq!(args[output_idx + 1], "/out/model.json");
        assert_eq!(args.last().unwrap(), "https://example.com/model.json");
    }

    #[test]
    fn curl_args_keeps_fail_silent_show_error_location() {
        // These four are what turn a 404/500 into a non-zero exit instead
        // of curl writing an HTML error page to `dest` and exiting 0, and
        // what follows HuggingFace's CDN redirects. Losing any of them
        // silently would be a much worse regression than a flaky retry.
        let args = curl_args("https://example.com/f", "/tmp/f");
        for flag in ["--fail", "--silent", "--show-error", "--location"] {
            assert!(args.contains(&flag.to_string()), "missing {flag}");
        }
    }
}
