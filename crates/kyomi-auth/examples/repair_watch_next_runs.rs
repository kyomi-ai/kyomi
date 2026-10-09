// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-890 maintenance: recalculate persisted fire times after standard weekday
//! conversion ships. Old numeric weekdays 1..7 fired a day early; 0 was rejected.
//! Pause every scheduler and watch writer, back up the DB, and choose one UTC
//! cutoff. Retry with the SAME cutoff for idempotence. Resume after success.
//! Public cron strings, enabled flags and all other watch data remain unchanged.
//! Enabled watches get their next occurrence strictly after cutoff; disabled
//! watches get NULL. Invalid enabled schedules roll back the entire repair.
//! This opt-in example connects without running migrations and is never called
//! by server startup. It supports both PostgreSQL and SQLite.

use chrono::{DateTime, Utc};
use kyomi_core::DbPool;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    const USAGE: &str = "Usage: DATABASE_URL=<target> cargo run --locked -p kyomi-auth --example repair_watch_next_runs -- --after <fixed-RFC3339-UTC-cutoff>\nPause all schedulers/writers and back up first. Reuse the cutoff for retries. No migrations run.";
    if args.len() == 2 && args[1] == "--help" {
        println!("{USAGE}");
        return Ok(());
    }
    if args.len() != 3 || args[1] != "--after" {
        return Err(USAGE.into());
    }
    let cutoff: DateTime<Utc> = args[2].parse()?;
    let url = std::env::var("DATABASE_URL")?;
    let db = if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        DbPool::Postgres(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await?,
        )
    } else if url.starts_with("sqlite:") {
        DbPool::Sqlite(
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await?,
        )
    } else {
        return Err("DATABASE_URL must use postgres://, postgresql:// or sqlite:".into());
    };
    let changed = kyomi_auth::watch_service::recalculate_watch_next_runs(&db, cutoff).await?;
    println!("Recalculated {changed} watch fire times at fixed cutoff {cutoff}");
    Ok(())
}
