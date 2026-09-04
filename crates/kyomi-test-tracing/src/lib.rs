// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared tracing-capture helper for tests asserting "a swallowed error is
//! now logged" (KYO-215, KYO-216, KYO-240; extracted per KYO-244).
//!
//! Depends on nothing beyond `tracing`, `tracing-subscriber` and `std` —
//! kept that way deliberately so any crate's `[dev-dependencies]` can pull
//! it in without dragging in `axum`/`sqlx`/`kyomi-server` the way depending
//! on `kyomi-test-harness` would (see KYO-244 for the full history: this is
//! the third copy of a ~35-line helper that first appeared in KYO-215 and
//! KYO-216, and staying duplicated a third time was explicitly the trigger
//! for extracting it).

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use tracing::Level;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

/// One captured tracing event: its level and rendered fields (the
/// `message` field followed by any `key=value` pairs), in emission order.
type EventLog = Arc<Mutex<Vec<(Level, String)>>>;

/// `tracing_subscriber::Layer` that records every event's level and
/// rendered fields into an [`EventLog`], so tests can assert on log output
/// without a real subscriber (fmt/json) formatting it to stdout.
struct CaptureLayer(EventLog);

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
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
        self.0
            .lock()
            .expect("capture lock poisoned")
            .push((*event.metadata().level(), visitor.0));
    }
}

/// A tracing capture in progress.
///
/// While this value is alive it is installed as the default subscriber for
/// the current thread ([`tracing::subscriber::set_default`]), and every
/// `tracing` event emitted on this thread is appended to its log. Dropping
/// it restores whichever default subscriber was active before
/// [`capture_tracing`] was called.
pub struct TracingCapture {
    events: EventLog,
    _guard: tracing::subscriber::DefaultGuard,
}

impl TracingCapture {
    /// All events captured so far, in emission order.
    pub fn events(&self) -> Vec<(Level, String)> {
        self.events.lock().expect("capture lock poisoned").clone()
    }

    /// Every captured event at exactly `level`, in emission order.
    ///
    /// Use this for negative assertions (e.g. "no error log contains the
    /// secret payload") and count assertions (e.g. "exactly one warning"),
    /// where a single boolean from
    /// [`has_message_containing`](Self::has_message_containing) can't
    /// express what's being checked.
    pub fn events_at(&self, level: Level) -> Vec<(Level, String)> {
        self.events().into_iter().filter(|(l, _)| *l == level).collect()
    }

    /// Whether any event at `level` contains `needle` in its rendered
    /// message/fields.
    pub fn has_message_containing(&self, level: Level, needle: &str) -> bool {
        self.events()
            .iter()
            .any(|(l, msg)| *l == level && msg.contains(needle))
    }

    /// Shorthand for [`has_message_containing`](Self::has_message_containing)
    /// at [`Level::ERROR`] — the common case across the call sites this
    /// crate was extracted from.
    pub fn has_error_containing(&self, needle: &str) -> bool {
        self.has_message_containing(Level::ERROR, needle)
    }
}

// ─── Interest-cache race guard (KYO-636, superseding the KYO-616 fix) ──────
//
// `tracing`'s per-callsite `Interest` is resolved exactly ONCE per callsite,
// process-wide: `DefaultCallsite::register()` runs a compare-exchange state
// machine (`UNREGISTERED -> REGISTERING -> REGISTERED`) and whichever thread
// wins folds together every currently *registered* `Dispatch` (i.e. every
// live `Dispatch::new(..)`-backed subscriber — `Dispatch::none()`, the true
// no-subscriber fallback, is never one of them) via `Interest::and`. Two
// facts make the failure mode precise:
//
//   - `Interest::and` yields `Sometimes` on disagreement between multiple
//     registered dispatchers — it never yields `Never`.
//   - `Interest::never()` is produced ONLY when the registered-dispatcher
//     set is empty at the exact instant a callsite is first executed.
//
// `capture_tracing()` installs its subscriber as a per-thread default via
// `tracing::subscriber::set_default`, which does NOT add it to the
// registered-dispatcher set consulted at first-registration time. So if a
// *different*, subscriber-less test's thread reaches a given callsite
// first, that callsite's interest is cached `Never` for the rest of the
// process — permanently, regardless of what any later test installs.
// `cargo test`'s default multi-threaded runner makes this a real race, not
// a theoretical one: measured at 2 failures in 10 runs on KYO-616's suite.
//
// The fix is `tracing::subscriber::set_global_default(AlwaysInterestedNoop)`
// below, run once via `Once`. This does two things at once, both load-
// bearing, but via a SINGLE call rather than two:
//
//   - It becomes the process's permanent fallback dispatcher: any thread
//     with no thread-local override that reaches a callsite for the first
//     time from this point on resolves against it, never against the empty
//     set — that callsite can never again be cached `Never`.
//   - Constructing it goes through `Dispatch::new()`, and `tracing-core`
//     documents (`callsite` module, "Rebuilding Cached Interest") that
//     constructing a `Dispatch` unconditionally re-resolves the `Interest`
//     of every callsite ALREADY registered at that moment, against the
//     full current dispatcher set. That is what repairs a callsite
//     poisoned to `Never` by an earlier, subscriber-less thread — no
//     separate step is needed for it.
//
// `tracing::callsite::rebuild_interest_cache()` performs that exact same
// full-registry rebuild (`Callsites::rebuild_interest`, the same function
// `Dispatch::new()` calls internally) and is invoked immediately after, as
// a second, explicit statement of "re-resolve everything now" that doesn't
// depend on `Dispatch::new()`'s rebuild-on-construct behavior specifically.
// It was verified empirically to be redundant with `set_global_default`
// alone against tracing-core 0.1.36 (this workspace's locked version): the
// integration test in `tests/interest_cache_race_guard.rs` still passes
// with this line commented out. It is kept anyway, deliberately, as a
// second guard against that automatic-rebuild-on-new-`Dispatch` behavior —
// which is documented but is still an implementation detail of a
// dependency we don't control — ever being narrowed in a later
// `tracing-core` release; it costs one `Once`-guarded call, once per
// process. Do not read its empirical redundancy today as "delete this
// line" — the two lines are deliberately not each other's proof.
//
// `set_global_default` can only succeed once per process, so a second call
// (e.g. a binary that installs its own global default before any test
// runs) returns `Err` here — that error is discarded rather than logged or
// panicked on. That is NOT unconditionally "just as effective": a
// pre-existing global default that itself filters callsites (e.g. a
// restrictive `EnvFilter`) can still yield `Interest::never()` for some of
// them, same as an empty set would. It only substitutes for this guard when
// that pre-existing dispatcher is, like `AlwaysInterestedNoop`, unconditionally
// interested in everything.
static INSTALL_GLOBAL_DEFAULT: std::sync::Once = std::sync::Once::new();

/// A `Subscriber` that is unconditionally interested in every callsite and
/// does nothing with what it's given. Never asserted on directly — its only
/// job is to exist, permanently, as a registered dispatcher, so the
/// interest-cache race described above can never again find an empty
/// registered-dispatcher set.
struct AlwaysInterestedNoop;

impl tracing::Subscriber for AlwaysInterestedNoop {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

/// Ensure the permanent always-interested global default is installed, and
/// that every callsite already poisoned `Never` before it existed is
/// repaired. Idempotent and cheap after the first call (`Once`'s fast path
/// is a single atomic load). See the module-level comment above for what
/// each line does and why both stay even though the second is redundant
/// with the first today.
fn ensure_interest_cache_race_guard_installed() {
    INSTALL_GLOBAL_DEFAULT.call_once(|| {
        let _ = tracing::subscriber::set_global_default(AlwaysInterestedNoop);
        tracing::callsite::rebuild_interest_cache();
    });
}

/// Start capturing `tracing` events on the current thread.
///
/// Install this at the top of a test, exercise the code under test, then
/// assert against the returned [`TracingCapture`] — capture stays active
/// (and events keep accumulating) until it's dropped.
///
/// ```
/// use kyomi_test_tracing::capture_tracing;
///
/// let logs = capture_tracing();
/// tracing::error!("simulated failure");
/// assert!(logs.has_error_containing("simulated failure"));
/// ```
pub fn capture_tracing() -> TracingCapture {
    ensure_interest_cache_race_guard_installed();

    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer(events.clone()));
    let guard = tracing::subscriber::set_default(subscriber);
    TracingCapture {
        events,
        _guard: guard,
    }
}
