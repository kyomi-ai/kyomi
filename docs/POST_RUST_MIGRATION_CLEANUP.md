# Post Rust Migration Cleanup

After migrating chart rendering from Node.js to chartml-rs and PDF generation from WeasyPrint to Typst, the following cleanup tasks remain. All are safe to do — the old code paths are no longer called.

## 1. Remove Dead HTML Functions from `pdf_export.rs`

These functions are now unused (replaced by Typst equivalents):

- `render_metric_html()`
- `render_table_html()`
- `columns_from_first_row()`
- `replace_chartml_with_images()`
- `markdown_to_html()`
- `inline_formatting()`
- `build_pdf_html()`
- `escape_html()`

Also remove the associated tests that test the HTML output.

**File**: `crates/kyomi-agent/src/pdf_export.rs`

## 2. Remove `chart_renderer_url` Config Threading

The `chart_renderer_url` parameter is threaded through many function signatures but no longer used. Remove the `_chart_renderer_url` parameter from:

- `generate_dashboard_pdf()` in `pdf_export.rs`
- `process_message_for_email()` in `alert.rs`
- `process_and_build_slack_blocks()` in `enterprise/kyomi-slack/src/message_processor.rs`
- All callers that pass `config.chart_renderer_url` through these functions

Then remove the config field itself:
- `chart_renderer_url` from `Config` struct in `crates/kyomi-core/src/config.rs`
- `CHART_RENDERER_URL` env var parsing
- `chart_renderer_configured()` method
- Health check for chart renderer in `apps/server/src/health.rs` or `system_config.rs`

**Files**: `kyomi-core/src/config.rs`, `kyomi-agent/src/alert.rs`, `kyomi-agent/src/pdf_export.rs`, `enterprise/kyomi-slack/src/message_processor.rs`, `enterprise/kyomi-slack/src/alert.rs`, `enterprise/kyomi-slack/src/routes.rs`

## 3. Delete `ChartRendererClient` HTTP Client

The HTTP client that called the Node.js service is no longer used by any code path.

- Delete `crates/kyomi-agent/src/tools/chart_renderer.rs`
- Remove `pub mod chart_renderer;` from `crates/kyomi-agent/src/tools/mod.rs`
- Remove `reqwest` from kyomi-agent deps if no other module uses it

## 4. Delete Node.js Chart Renderer Service

The entire service can be removed:

- Delete `apps/chart-renderer/` directory (server.js, Dockerfile, package.json, etc.)
- Remove `chart-renderer` service from `docker-compose.dev.yml`
- Remove `k8s/chart-renderer.yaml` (if exists)
- Remove chart-renderer from any CI/CD workflows
- Remove the GHCR image: `ghcr.io/kyomi-ai/kyomi-chart-renderer`

## 5. Update Deployment

- Remove chart-renderer k8s Deployment + Service (2 replicas currently)
- Remove `CHART_RENDERER_URL` from backend deployment env vars
- Update Helm chart if chart-renderer is referenced

## 6. Update Documentation

- Update any architecture diagrams that show the chart-renderer service
- Update `apps/chart-renderer/DESIGN.md` references elsewhere
- Update self-hosting docs if they mention the chart-renderer container

## Priority

Items 1-3 are low-risk code cleanup. Item 4 is infrastructure. Items 5-6 are deployment/docs. All can be done in a single PR.
