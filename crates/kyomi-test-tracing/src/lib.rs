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
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(CaptureLayer(events.clone()));
    let guard = tracing::subscriber::set_default(subscriber);
    TracingCapture {
        events,
        _guard: guard,
    }
}
