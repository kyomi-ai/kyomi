use serde::{Deserialize, Serialize};

/// Inbound payload from the JS snippet.
/// Uses short field names to minimize bytes over the wire.
#[derive(Deserialize)]
pub struct EventPayload {
    /// Site ID (legacy dogfooding mode)
    #[serde(default)]
    pub s: String,
    /// Signed analytics key (from data-key attribute)
    #[serde(default)]
    pub key: String,
    /// Event name (defaults to "pageview")
    #[serde(default = "default_pageview")]
    pub n: String,
    /// Page URL
    pub u: String,
    /// Referrer URL
    #[serde(default)]
    pub r: String,
    /// Screen width
    #[serde(default)]
    pub w: u16,
    /// Screen height
    #[serde(default)]
    pub h: u16,
    /// Custom properties (arbitrary JSON: strings, numbers, bools, nested objects)
    #[serde(default)]
    pub p: Option<serde_json::Value>,
    /// User ID (from identify() call)
    #[serde(default)]
    pub uid: String,
    /// Identified flag (from identify() call, 1 = identify has been called)
    #[serde(default)]
    pub i: u8,
}

/// An event row paired with its target ClickHouse database.
/// Used by the batcher to route events to per-site databases.
pub struct BatchEntry {
    pub database: String,
    pub row: EventRow,
}

fn default_pageview() -> String {
    "pageview".to_string()
}

/// Row inserted into a per-site ClickHouse database's `events` table.
/// Field order must match the column order used in the INSERT.
///
/// The `properties` column stores a JSON string (ClickHouse `String` type).
/// This supports arbitrary nested JSON: strings, numbers, bools, arrays, objects.
/// Query with ClickHouse JSON functions: `JSONExtractString(properties, 'key')`,
/// `JSONExtractInt(properties, 'key')`, etc.
#[derive(clickhouse::Row, Serialize)]
pub struct EventRow {
    pub visitor_id: String,
    pub session_id: String,
    pub user_id: String,
    pub timestamp: i64,
    pub event_name: String,
    pub hostname: String,
    pub pathname: String,
    pub referrer: String,
    pub referrer_source: String,
    pub utm_source: String,
    pub utm_medium: String,
    pub utm_campaign: String,
    pub utm_term: String,
    pub utm_content: String,
    pub country_code: String,
    pub region: String,
    pub city: String,
    pub browser: String,
    pub browser_version: String,
    pub os: String,
    pub os_version: String,
    pub device_type: String,
    pub screen_width: u16,
    pub screen_height: u16,
    pub properties: String,
}
