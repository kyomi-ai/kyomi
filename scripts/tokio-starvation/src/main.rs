// SPDX-License-Identifier: AGPL-3.0-or-later

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use axum::{Router, routing::get};
use tokio::runtime::Builder;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn probe(address: SocketAddr, timeout: Duration) -> Result<Duration> {
    let start = Instant::now();
    let mut stream = TcpStream::connect_timeout(&address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    if !response.starts_with("HTTP/1.1 200 OK\r\n") || !response.ends_with("ok") {
        return Err(format!("unexpected health response: {response:?}").into());
    }
    Ok(start.elapsed())
}

fn scenario(
    workers: usize,
    blockers: usize,
    duration: Duration,
    busy: bool,
    timer: bool,
) -> Result<()> {
    let runtime = Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()?;
    let address = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/health", get(|| async { "ok" })),
            )
            .await
        });
        Ok::<_, std::io::Error>(address)
    })?;
    let timeout = duration + Duration::from_secs(10);
    let baseline = probe(address, timeout)?;
    let (entered_tx, entered_rx) = mpsc::channel();
    let mut handles = Vec::new();
    let wake_at = tokio::time::Instant::now() + Duration::from_millis(100);
    for _ in 0..blockers {
        let entered_tx = entered_tx.clone();
        handles.push(runtime.spawn(async move {
            // Timer wakeups exercise work originating in the runtime driver,
            // after idle workers have had an opportunity to park.
            if timer {
                tokio::time::sleep_until(wake_at).await;
            }
            entered_tx
                .send(Instant::now())
                .expect("measurement receiver alive");
            if busy {
                let start = Instant::now();
                while start.elapsed() < duration {
                    std::hint::spin_loop();
                }
            } else {
                std::thread::sleep(duration);
            }
        }));
    }
    drop(entered_tx);
    // The observer is outside Tokio: starvation cannot delay the probe's launch,
    // readiness accounting, or wall clock. At most one blocker per worker.
    let mut first_entry = None;
    for _ in 0..blockers {
        let entry = entered_rx.recv_timeout(timeout)?;
        first_entry.get_or_insert(entry);
    }
    let entry_spread = first_entry.map_or(Duration::ZERO, |entry| entry.elapsed());
    let during = probe(address, timeout)?;
    runtime.block_on(async {
        for handle in handles {
            handle.await.expect("blocker did not panic");
        }
    });
    let after = probe(address, timeout)?;
    println!(
        "{workers},{blockers},{},{},{:.3},{:.3},{:.3},{:.3},{:.3}",
        if busy { "busy" } else { "sleep" },
        if timer { "timer" } else { "immediate" },
        duration.as_secs_f64(),
        baseline.as_secs_f64() * 1000.0,
        entry_spread.as_secs_f64() * 1000.0,
        during.as_secs_f64() * 1000.0,
        after.as_secs_f64() * 1000.0
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() > 4 {
        return Err(
            "usage: tokio-starvation-repro [seconds=30] [sleep|busy] [workers=1,2,4,8] [immediate|timer]".into(),
        );
    }
    let seconds: u64 = args.first().map_or(Ok(30), |value| value.parse())?;
    if seconds == 0 {
        return Err("seconds must be positive".into());
    }
    let busy = match args.get(1).map(String::as_str).unwrap_or("sleep") {
        "sleep" => false,
        "busy" => true,
        _ => return Err("mode must be sleep or busy".into()),
    };
    let timer = match args.get(3).map(String::as_str).unwrap_or("immediate") {
        "immediate" => false,
        "timer" => true,
        _ => return Err("launch must be immediate or timer".into()),
    };
    let workers: Vec<usize> = args
        .get(2)
        .map(String::as_str)
        .unwrap_or("1,2,4,8")
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    if workers.contains(&0) {
        return Err("worker counts must be positive".into());
    }
    let default_runtime = Builder::new_multi_thread().enable_all().build()?;
    eprintln!(
        "available_parallelism={} default_runtime_workers={} TOKIO_WORKER_THREADS={:?}",
        std::thread::available_parallelism()?,
        default_runtime.metrics().num_workers(),
        std::env::var_os("TOKIO_WORKER_THREADS")
    );
    drop(default_runtime);
    println!("workers,blockers,mode,launch,seconds,baseline_ms,entry_spread_ms,during_ms,after_ms");
    let mut failures = 0;
    for workers in workers {
        let mut counts = vec![0, 1, workers.saturating_sub(1), workers];
        counts.sort_unstable();
        counts.dedup();
        for blockers in counts {
            if let Err(error) =
                scenario(workers, blockers, Duration::from_secs(seconds), busy, timer)
            {
                eprintln!("FAILED workers={workers} blockers={blockers}: {error}");
                failures += 1;
            }
        }
    }
    if failures > 0 {
        return Err(format!("{failures} scenario(s) failed").into());
    }
    Ok(())
}
