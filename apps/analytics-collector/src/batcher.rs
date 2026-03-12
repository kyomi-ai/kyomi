//! In-process event batcher — decouples HTTP response latency from ClickHouse inserts.
//!
//! The HTTP handler pushes `BatchEntry` values (database name + enriched row) into a
//! bounded mpsc channel and responds immediately. A background task drains the channel
//! and flushes batches to ClickHouse, reducing per-event overhead from one HTTP
//! round-trip to amortised cost across up to `max_batch` rows.
//!
//! ## Backpressure
//!
//! When the channel is full the handler returns 503 to the client. k.js does not
//! retry, so the event is lost — acceptable for analytics under extreme load.
//!
//! ## Flush triggers
//!
//! A batch is flushed when either:
//! 1. It reaches `max_batch` rows, OR
//! 2. `flush_interval` has elapsed since the last flush (don't hold events too long)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use clickhouse::Client;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{error, info, warn};

use crate::models::{BatchEntry, EventRow};

/// Configurable knobs — all readable from environment variables at startup.
struct BatcherConfig {
    /// Max events buffered in the channel before backpressure kicks in.
    channel_capacity: usize,
    /// Max rows per ClickHouse INSERT.
    max_batch: usize,
    /// Time-based flush interval.
    flush_interval: Duration,
}

impl BatcherConfig {
    fn from_env() -> Self {
        let channel_capacity = parse_env("BATCHER_CHANNEL_CAPACITY", 50_000);
        let max_batch = parse_env("BATCHER_MAX_BATCH", 1_000);
        let flush_interval_ms = parse_env("BATCHER_FLUSH_INTERVAL_MS", 200);
        Self {
            channel_capacity,
            max_batch,
            flush_interval: Duration::from_millis(flush_interval_ms as u64),
        }
    }
}

fn parse_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Handle for submitting events into the batch pipeline.
#[derive(Clone)]
pub struct EventBatcher {
    tx: mpsc::Sender<BatchEntry>,
}

impl EventBatcher {
    /// Create a new batcher and spawn its background flush loop.
    ///
    /// The returned handle is cheap to clone (Arc'd channel sender).
    pub fn new(client: Arc<Client>) -> Self {
        let config = BatcherConfig::from_env();

        info!(
            channel_capacity = config.channel_capacity,
            max_batch = config.max_batch,
            flush_interval_ms = config.flush_interval.as_millis() as u64,
            "Starting event batcher"
        );

        let (tx, rx) = mpsc::channel(config.channel_capacity);

        tokio::spawn(flush_loop(rx, client, config.max_batch, config.flush_interval));

        Self { tx }
    }

    /// Submit an event for batched insertion.
    ///
    /// Returns `Ok(())` if the event was accepted, `Err(())` if the channel is full.
    pub fn submit(&self, entry: BatchEntry) -> Result<(), ()> {
        self.tx.try_send(entry).map_err(|_| {
            warn!("Event batcher channel full — dropping event (backpressure)");
        })
    }
}

/// Background loop that drains the channel and flushes batches to ClickHouse.
///
/// Uses `tokio::select!` (unbiased) so the timer branch gets a fair chance to
/// fire even under sustained load. After receiving one event via `recv()`, we
/// greedily drain everything available with `try_recv()` to maximise batch size.
async fn flush_loop(
    mut rx: mpsc::Receiver<BatchEntry>,
    client: Arc<Client>,
    max_batch: usize,
    flush_interval: Duration,
) {
    let mut buffer: Vec<BatchEntry> = Vec::with_capacity(max_batch);
    let mut ticker = interval(flush_interval);
    // The first tick fires immediately — consume it so we start with a clean slate.
    ticker.tick().await;

    loop {
        tokio::select! {
            maybe_row = rx.recv() => {
                match maybe_row {
                    Some(row) => {
                        buffer.push(row);
                        // Greedily drain everything currently in the channel
                        while buffer.len() < max_batch {
                            match rx.try_recv() {
                                Ok(r) => buffer.push(r),
                                Err(_) => break,
                            }
                        }
                        if buffer.len() >= max_batch {
                            flush(&client, &mut buffer).await;
                            ticker.reset();
                        }
                    }
                    None => {
                        // Channel closed (shutdown) — flush remaining and exit
                        if !buffer.is_empty() {
                            flush(&client, &mut buffer).await;
                        }
                        info!("Event batcher shutting down");
                        return;
                    }
                }
            }

            _ = ticker.tick() => {
                if !buffer.is_empty() {
                    flush(&client, &mut buffer).await;
                }
            }
        }
    }
}

/// Flush a batch of events to ClickHouse, grouped by per-site database.
async fn flush(client: &Client, buffer: &mut Vec<BatchEntry>) {
    let count = buffer.len();
    let start = std::time::Instant::now();

    // Group entries by database so we do one INSERT per database per flush
    let mut groups: HashMap<String, Vec<EventRow>> = HashMap::new();
    for entry in buffer.drain(..) {
        groups.entry(entry.database).or_default().push(entry.row);
    }

    let mut had_error = false;
    for (database, rows) in groups {
        let table = format!("{database}.events");
        if let Err(e) = crate::clickhouse::insert_batch(client, &table, rows.into_iter()).await {
            error!(error = %e, database = %database, "Failed to flush event batch to ClickHouse");
            // Events are lost — acceptable for analytics.
            had_error = true;
        }
    }

    let elapsed = start.elapsed();
    if had_error {
        // At least one database insert failed — errors logged above.
        // Use warn! so operators can distinguish partial failures from clean flushes.
        warn!(batch_size = count, elapsed_ms = elapsed.as_millis() as u64, "Flushed event batch (with errors)");
    } else {
        info!(batch_size = count, elapsed_ms = elapsed.as_millis() as u64, "Flushed event batch");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a minimal BatchEntry for testing.
    fn test_entry(event_name: &str) -> BatchEntry {
        BatchEntry {
            database: "site_test_ws_abc123".into(),
            row: EventRow {
                visitor_id: "v1".into(),
                session_id: "s1".into(),
                user_id: String::new(),
                timestamp: 1000,
                event_name: event_name.into(),
                hostname: "example.com".into(),
                pathname: "/".into(),
                referrer: String::new(),
                referrer_source: String::new(),
                utm_source: String::new(),
                utm_medium: String::new(),
                utm_campaign: String::new(),
                utm_term: String::new(),
                utm_content: String::new(),
                country_code: String::new(),
                region: String::new(),
                city: String::new(),
                browser: String::new(),
                browser_version: String::new(),
                os: String::new(),
                os_version: String::new(),
                device_type: String::new(),
                screen_width: 0,
                screen_height: 0,
                properties: "{}".into(),
            },
        }
    }

    #[test]
    fn parse_env_returns_default_when_unset() {
        assert_eq!(parse_env("BATCHER_TEST_NONEXISTENT_12345", 42), 42);
    }

    #[test]
    fn submit_accepts_events_when_channel_has_capacity() {
        let (tx, _rx) = mpsc::channel(4);
        let batcher = EventBatcher { tx };

        assert!(batcher.submit(test_entry("e1")).is_ok());
        assert!(batcher.submit(test_entry("e2")).is_ok());
        assert!(batcher.submit(test_entry("e3")).is_ok());
        assert!(batcher.submit(test_entry("e4")).is_ok());
    }

    #[test]
    fn submit_returns_err_when_channel_full() {
        let (tx, _rx) = mpsc::channel(2);
        let batcher = EventBatcher { tx };

        assert!(batcher.submit(test_entry("e1")).is_ok());
        assert!(batcher.submit(test_entry("e2")).is_ok());
        assert!(batcher.submit(test_entry("e3")).is_err()); // backpressure
    }

    #[test]
    fn submit_fails_when_receiver_dropped() {
        let (tx, rx) = mpsc::channel(10);
        drop(rx);
        let batcher = EventBatcher { tx };

        assert!(batcher.submit(test_entry("e1")).is_err());
    }

    #[tokio::test]
    async fn channel_closes_when_sender_dropped() {
        let (tx, rx) = mpsc::channel::<BatchEntry>(100);

        tx.send(test_entry("e1")).await.unwrap();
        tx.send(test_entry("e2")).await.unwrap();
        drop(tx);

        // Receiver sees channel closed — flush_loop would drain remaining and exit
        assert!(rx.is_closed());
    }

    #[tokio::test]
    async fn greedy_drain_collects_all_available() {
        // Simulate the greedy try_recv drain logic from flush_loop
        let (tx, mut rx) = mpsc::channel::<BatchEntry>(100);
        let max_batch = 1000;

        // Send 5 events
        for i in 0..5 {
            tx.send(test_entry(&format!("e{i}"))).await.unwrap();
        }

        // Receive one via recv(), then drain rest via try_recv()
        let mut buffer = Vec::new();
        if let Some(entry) = rx.recv().await {
            buffer.push(entry);
            while buffer.len() < max_batch {
                match rx.try_recv() {
                    Ok(r) => buffer.push(r),
                    Err(_) => break,
                }
            }
        }

        assert_eq!(buffer.len(), 5);
        assert_eq!(buffer[0].row.event_name, "e0");
        assert_eq!(buffer[4].row.event_name, "e4");
    }

    #[tokio::test]
    async fn greedy_drain_stops_at_max_batch() {
        let (tx, mut rx) = mpsc::channel::<BatchEntry>(100);
        let max_batch = 3;

        // Send 5 events but max_batch is 3
        for i in 0..5 {
            tx.send(test_entry(&format!("e{i}"))).await.unwrap();
        }

        let mut buffer = Vec::new();
        if let Some(entry) = rx.recv().await {
            buffer.push(entry);
            while buffer.len() < max_batch {
                match rx.try_recv() {
                    Ok(r) => buffer.push(r),
                    Err(_) => break,
                }
            }
        }

        // Should stop at max_batch, leaving 2 in channel
        assert_eq!(buffer.len(), 3);
        assert_eq!(rx.try_recv().unwrap().row.event_name, "e3");
        assert_eq!(rx.try_recv().unwrap().row.event_name, "e4");
    }
}
