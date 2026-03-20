// SPDX-License-Identifier: AGPL-3.0-or-later

//! HTTP client for the chart-renderer microservice (Node.js).
//!
//! The chart-renderer is a separate Node.js service that:
//! - `POST /render` — renders ChartML to PNG (base64-encoded)
//! - `POST /html-to-pdf` — converts HTML to PDF via WeasyPrint
//! - `POST /transform` — runs the DuckDB transform pipeline on data
//! - `GET /health` — health check
//!
//! Ports Python's `chart_renderer_service.py`.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::{json, Value};
use tracing;

/// Timeout for chart renderer HTTP calls (seconds).
const CHART_RENDERER_TIMEOUT_SECS: u64 = 30;

/// Timeout for HTML-to-PDF calls (seconds). WeasyPrint is slower than chart rendering.
const PDF_TIMEOUT_SECS: u64 = 90;

/// Timeout for health check calls (seconds).
const HEALTH_CHECK_TIMEOUT_SECS: u64 = 5;

/// Chart renderer HTTP client.
pub struct ChartRendererClient {
    base_url: String,
    client: reqwest::Client,
}

impl ChartRendererClient {
    /// Create a new client from the chart_renderer_url config.
    pub fn new(base_url: &str) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .user_agent("Kyomi/1.0")
            .timeout(std::time::Duration::from_secs(CHART_RENDERER_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    /// Render a ChartML spec to PNG bytes.
    ///
    /// Sends the resolved spec (with inline data) to the renderer.
    /// `density` controls SVG rasterization DPI (default 72, use 144 for PDF).
    /// Returns raw PNG bytes.
    pub async fn render_chart(
        &self,
        spec: &Value,
        width: u32,
        height: u32,
        default_palette: Option<&[String]>,
        density: Option<u32>,
    ) -> Result<Vec<u8>, String> {
        let mut payload = json!({
            "chartMLSpec": spec,
            "width": width,
            "height": height,
            "density": density.unwrap_or(72),
        });

        if let Some(palette) = default_palette {
            payload["defaultPalette"] = json!(palette);
        }

        let response = self
            .client
            .post(format!("{}/render", self.base_url))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("Chart renderer HTTP error: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "(no body)".into());
            return Err(format!(
                "Chart renderer returned {status}: {body}"
            ));
        }

        let result: Value = response
            .json()
            .await
            .map_err(|e| format!("Chart renderer invalid JSON response: {e}"))?;

        let image_b64 = result
            .get("image")
            .and_then(|v| v.as_str())
            .ok_or("Chart renderer response missing 'image' field")?;

        BASE64
            .decode(image_b64)
            .map_err(|e| format!("Failed to decode base64 image: {e}"))
    }

    /// Convert an HTML document to PDF via WeasyPrint.
    ///
    /// Sends the complete HTML to the chart-renderer's `/html-to-pdf` endpoint.
    /// Returns raw PDF bytes.
    pub async fn html_to_pdf(&self, html: &str) -> Result<Vec<u8>, String> {
        let payload = json!({ "html": html });

        // Use a longer timeout — WeasyPrint is slower than chart rendering.
        let pdf_client = reqwest::Client::builder()
            .user_agent("Kyomi/1.0")
            .timeout(std::time::Duration::from_secs(PDF_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("Failed to build PDF HTTP client: {e}"))?;

        let response = pdf_client
            .post(format!("{}/html-to-pdf", self.base_url))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("HTML-to-PDF HTTP error: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "(no body)".into());
            return Err(format!(
                "HTML-to-PDF returned {status}: {body}"
            ));
        }

        let result: Value = response
            .json()
            .await
            .map_err(|e| format!("HTML-to-PDF invalid JSON response: {e}"))?;

        let pdf_b64 = result
            .get("pdf")
            .and_then(|v| v.as_str())
            .ok_or("HTML-to-PDF response missing 'pdf' field")?;

        BASE64
            .decode(pdf_b64)
            .map_err(|e| format!("Failed to decode base64 PDF: {e}"))
    }

    /// Run the DuckDB transform pipeline on named source data.
    ///
    /// The chart-renderer runs the pipeline (sql → aggregate → forecast)
    /// and returns resolved inline data.
    pub async fn transform_data(
        &self,
        data: &Value,
        transform: &Value,
    ) -> Result<Value, String> {
        let payload = json!({
            "data": data,
            "transform": transform,
        });

        let response = self
            .client
            .post(format!("{}/transform", self.base_url))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("Chart renderer transform HTTP error: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "(no body)".into());
            return Err(format!(
                "Chart renderer transform returned {status}: {body}"
            ));
        }

        let result: Value = response
            .json()
            .await
            .map_err(|e| format!("Chart renderer transform invalid JSON: {e}"))?;

        result
            .get("data")
            .cloned()
            .ok_or_else(|| "Chart renderer transform response missing 'data' field".into())
    }

    /// Check if the renderer service is available.
    pub async fn health_check(&self) -> bool {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| self.client.clone());

        match client
            .get(format!("{}/health", self.base_url))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                tracing::warn!(error = %e, "Chart renderer health check failed");
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_trims_trailing_slash() {
        let client = ChartRendererClient::new("http://localhost:3030/").unwrap();
        assert_eq!(client.base_url, "http://localhost:3030");
    }

    #[test]
    fn client_preserves_url_without_slash() {
        let client = ChartRendererClient::new("http://chart-renderer:3030").unwrap();
        assert_eq!(client.base_url, "http://chart-renderer:3030");
    }
}
