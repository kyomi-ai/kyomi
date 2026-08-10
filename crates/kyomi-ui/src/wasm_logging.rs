// SPDX-License-Identifier: AGPL-3.0-or-later

//! Browser-console `tracing` subscriber for the WASM client (KYO-303).
//!
//! `tracing` events are a no-op unless a subscriber is installed
//! (`tracing::subscriber::set_global_default`). The server installs one —
//! `apps/server/src/main.rs` calls `tracing_subscriber::fmt()` — which is why
//! server-side `tracing::warn!`/`tracing::error!` calls, and the SSR render
//! pass, produce output. The WASM entry point (`main.rs`) never did, so every
//! `tracing::warn!` that runs post-hydration in the browser (IndexedDB
//! failures in `cache/db.rs`, sync errors in `cache/sync_engine.rs`, the
//! fail-closed permission warnings in `utils/permissions.rs`, ...) was
//! silently discarded. [`install`] fixes that for the WASM target only —
//! the SSR path and the server's own subscriber are untouched.
//!
//! Default level is [`Level::WARN`]. Raise it at runtime, without a rebuild,
//! via `localStorage.setItem("kyomi:log_level", "debug")` (see
//! [`LOCAL_STORAGE_LOG_LEVEL_KEY`]) and reload the page — a query param was
//! considered and rejected because it's lost on the first client-side
//! navigation, which is most of them.

// Only referenced by `parse_log_level`/`ConsoleLayer` below, both of which
// are themselves `cfg`-gated to `test`-or-`wasm32` — gated the same way so
// a plain host build (neither `ssr` nor `hydrate`, i.e. what `cargo check
// --workspace` builds this crate with) doesn't trip `unused_imports`.
#[cfg(any(test, target_arch = "wasm32"))]
use tracing::Level;

/// `localStorage` key read once at startup to override the default WARN
/// level. Accepted values (case-insensitive): `trace`, `debug`, `info`,
/// `warn`, `error`. Anything else — unset, unrecognised, wrong case is
/// handled, but e.g. typos — falls back to [`Level::WARN`].
pub const LOCAL_STORAGE_LOG_LEVEL_KEY: &str = "kyomi:log_level";

/// Parse a raw `localStorage` value into a [`Level`], defaulting to
/// [`Level::WARN`] for anything absent or unrecognised.
///
/// Split out from the WASM-only install path below so it can be unit tested
/// on the host target (`cargo test -p kyomi-ui --features ssr`) — the rest
/// of this module depends on `tracing-subscriber`, which is only a
/// dependency on `wasm32` (see `crates/kyomi-ui/Cargo.toml`), and on
/// `web_sys::window()`, which returns `None` off-target.
///
/// `cfg`-gated to `test`-or-`wasm32` (rather than left unconditional) so a
/// plain host `cargo check`/`clippy` build — which compiles this crate with
/// neither `ssr` nor `hydrate`, so nothing else in the crate calls this
/// function — doesn't trip `dead_code`.
#[cfg(any(test, target_arch = "wasm32"))]
fn parse_log_level(raw: Option<&str>) -> Level {
    match raw.map(str::to_ascii_lowercase).as_deref() {
        Some("trace") => Level::TRACE,
        Some("debug") => Level::DEBUG,
        Some("info") => Level::INFO,
        Some("warn") => Level::WARN,
        Some("error") => Level::ERROR,
        _ => Level::WARN,
    }
}

/// Read `localStorage["kyomi:log_level"]`, if available.
///
/// Returns `None` if there's no `window`, no `localStorage` (e.g. private
/// browsing in some browsers), or the key isn't set — all of which fall
/// back to the default WARN level via [`parse_log_level`].
#[cfg(target_arch = "wasm32")]
fn read_local_storage_level() -> Option<String> {
    let storage = web_sys::window()?.local_storage().ok()??;
    storage.get_item(LOCAL_STORAGE_LOG_LEVEL_KEY).ok()?
}

/// A `tracing_subscriber::Layer` that forwards events to the browser
/// console at a level-appropriate method: `console.error` for ERROR,
/// `console.warn` for WARN, `console.info` for INFO, `console.debug` for
/// DEBUG and TRACE. (`console.trace` is deliberately not used for the
/// TRACE level — in real browsers it prints a JS call stack instead of the
/// logged message, which is not what a `tracing::trace!` call means here.)
///
/// No span bookkeeping: `kyomi-ui` has no `tracing::span!`/`#[instrument]`
/// call sites today (checked at KYO-303 implementation time), only events,
/// so this layer only implements `on_event`. The `Registry` it's installed
/// into still provides span storage — this layer just never reads it.
#[cfg(target_arch = "wasm32")]
struct ConsoleLayer {
    max_level: Level,
}

#[cfg(target_arch = "wasm32")]
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ConsoleLayer {
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        *metadata.level() <= self.max_level
    }

    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        use std::fmt::Write as _;

        // Same field-rendering shape as `kyomi-test-tracing`'s `CaptureLayer`
        // (crates/kyomi-test-tracing/src/lib.rs): the `message` field first,
        // unquoted, followed by any structured `key=value` pairs.
        struct FieldVisitor(String);
        impl tracing::field::Visit for FieldVisitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    let _ = write!(self.0, "{value:?}");
                } else {
                    let _ = write!(self.0, " {}={value:?}", field.name());
                }
            }
        }

        let mut visitor = FieldVisitor(String::new());
        event.record(&mut visitor);

        let level = *event.metadata().level();
        let line = format!("{}: {}", event.metadata().target(), visitor.0);
        let js_line = wasm_bindgen::JsValue::from_str(&line);

        match level {
            Level::ERROR => web_sys::console::error_1(&js_line),
            Level::WARN => web_sys::console::warn_1(&js_line),
            Level::INFO => web_sys::console::info_1(&js_line),
            Level::DEBUG | Level::TRACE => web_sys::console::debug_1(&js_line),
        }
    }
}

/// Install a `tracing` subscriber that forwards events to the browser
/// console. Call once at WASM startup, before any `tracing` event is
/// expected to reach the console — see `main.rs`.
///
/// Reads [`LOCAL_STORAGE_LOG_LEVEL_KEY`] to decide the max level; falls back
/// to [`Level::WARN`] if it's absent or unrecognised (see
/// [`parse_log_level`]).
///
/// Safe to call if a global subscriber somehow already exists:
/// `tracing::subscriber::set_global_default` can only succeed once per
/// process by design (that's the documented contract of the global it
/// guards, not a failure mode), so a second call is handled, not
/// `.expect()`-ed into a hard crash of the entry point.
#[cfg(target_arch = "wasm32")]
pub fn install() {
    use tracing_subscriber::layer::SubscriberExt as _;

    let max_level = parse_log_level(read_local_storage_level().as_deref());
    let subscriber = tracing_subscriber::Registry::default().with(ConsoleLayer { max_level });

    if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
        // Reported straight to the console rather than via `tracing` itself
        // — if this branch is reached, that's exactly because a global
        // subscriber already exists, so going through `tracing::warn!` here
        // would depend on the very thing that just failed to install.
        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
            "kyomi_ui::wasm_logging::install: a global tracing subscriber was already installed: {e}"
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_recognized_level_case_insensitively() {
        assert_eq!(parse_log_level(Some("trace")), Level::TRACE);
        assert_eq!(parse_log_level(Some("TRACE")), Level::TRACE);
        assert_eq!(parse_log_level(Some("debug")), Level::DEBUG);
        assert_eq!(parse_log_level(Some("Debug")), Level::DEBUG);
        assert_eq!(parse_log_level(Some("info")), Level::INFO);
        assert_eq!(parse_log_level(Some("INFO")), Level::INFO);
        assert_eq!(parse_log_level(Some("warn")), Level::WARN);
        assert_eq!(parse_log_level(Some("WaRn")), Level::WARN);
        assert_eq!(parse_log_level(Some("error")), Level::ERROR);
        assert_eq!(parse_log_level(Some("ERROR")), Level::ERROR);
    }

    #[test]
    fn falls_back_to_warn_for_an_unrecognized_value() {
        assert_eq!(parse_log_level(Some("verbose")), Level::WARN);
        assert_eq!(parse_log_level(Some("")), Level::WARN);
        assert_eq!(parse_log_level(Some("warning")), Level::WARN); // close, but not a real level
    }

    #[test]
    fn falls_back_to_warn_when_absent() {
        assert_eq!(parse_log_level(None), Level::WARN);
    }
}
