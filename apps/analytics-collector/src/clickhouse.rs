use clickhouse::Client;
use tracing::info;

use crate::models::EventRow;

/// Create the ClickHouse client from environment variables.
pub fn create_client() -> Client {
    let host = std::env::var("ANALYTICS_CLICKHOUSE_HOST").unwrap_or_else(|_| "localhost".into());
    let port = std::env::var("ANALYTICS_CLICKHOUSE_PORT").unwrap_or_else(|_| "8126".into());
    let user = std::env::var("ANALYTICS_CLICKHOUSE_USER").unwrap_or_else(|_| "default".into());
    let password = std::env::var("ANALYTICS_CLICKHOUSE_PASSWORD").unwrap_or_default();
    let secure = std::env::var("ANALYTICS_CLICKHOUSE_SECURE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let scheme = if secure { "https" } else { "http" };
    let url = format!("{scheme}://{host}:{port}");
    info!(url = %url, "Connecting to ClickHouse");

    let mut client = Client::default()
        .with_url(&url)
        .with_user(user);

    if !password.is_empty() {
        client = client.with_password(password);
    }

    client
}

/// Insert a batch of event rows into the given table (e.g. `site_ws_abc123.events`).
pub async fn insert_batch(
    client: &Client,
    table: &str,
    rows: impl Iterator<Item = EventRow>,
) -> Result<(), clickhouse::error::Error> {
    let mut insert = client.insert::<EventRow>(table)?;
    for row in rows {
        insert.write(&row).await?;
    }
    insert.end().await?;
    Ok(())
}

/// Health check — ping ClickHouse with a simple query.
pub async fn health_check(client: &Client) -> Result<(), clickhouse::error::Error> {
    client.query("SELECT 1").execute().await?;
    Ok(())
}
