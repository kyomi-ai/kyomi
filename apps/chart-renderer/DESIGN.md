# Chart Renderer Integration Design

## Overview

Integrate the ChartML renderer service into Kyomi to render charts as PNG images for Slack bot responses.

## Current State (What Already Exists)

The Slack bot integration already has:
- `_handle_app_mention()` - Handles @kyomi mentions
- `_run_slack_query()` - Uses the full chat agent with datasource access
- `_post_slack_response()` - Posts text responses to Slack
- Agent can query BigQuery/Postgres/etc via existing tools

**Gap**: The agent returns text/markdown. It doesn't generate ChartML or render charts.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Slack                                    │
│  User: @kyomi show me sessions by country                       │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Kyomi Backend (Python)                        │
│                                                                  │
│  1. Receive Slack @mention event                                │
│  2. Process with LLM agent                                      │
│  3. Agent executes query → gets data rows                       │
│  4. Agent generates ChartML spec with inline data               │
│  5. Backend detects ChartML in response                         │
│  6. Backend calls Chart Renderer service                        │
│  7. Backend uploads PNG to Slack                                │
│  8. Backend posts message with chart attachment                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                 Chart Renderer Service (Node.js)                 │
│                                                                  │
│  POST /render                                                   │
│  { chartMLSpec: {...}, width: 800, height: 400 }                │
│                                                                  │
│  Returns: { image: "base64...", format: "png" }                 │
└─────────────────────────────────────────────────────────────────┘
```

## Key Design Decision: How Does the Agent Generate ChartML?

**Option A: New `render_chart` Tool**
The agent gets a tool it can call to render charts:
```python
@tool
def render_chart(data: list[dict], chart_type: str, x_field: str, y_field: str, title: str) -> dict:
    """Generate a chart from query results."""
    # Builds ChartML spec from parameters
    # Returns {"chartml": spec, "rendered": True}
```
- Pro: Explicit control over when charts are rendered
- Pro: Structured parameters ensure valid specs
- Con: Agent must learn when to use the tool

**Option B: Automatic Detection**
After query execution, backend detects tabular data suitable for charting:
```python
def should_render_chart(query_result: list[dict], user_question: str) -> bool:
    # Heuristics: "show me", "chart", "visualize", aggregation results
```
- Pro: Works without agent changes
- Con: May render charts when not wanted

**Option C: Agent Outputs ChartML Directly (Current Kyomi Web Behavior)**
The agent already generates ChartML in web UI responses. Same behavior for Slack.
- Pro: Consistent with web experience
- Con: Need to verify agent prompts support this in Slack context

**Recommendation: Option C** - The agent already knows how to generate ChartML for the web UI. We just need to detect it in Slack responses and render it.

## Components

### 1. Chart Renderer Service (already built)

- **Location**: `apps/chart-renderer/`
- **Port**: 3030 (configurable via `CHART_RENDERER_PORT`)
- **Endpoints**:
  - `GET /health` - Health check
  - `POST /render` - Render ChartML to PNG

### 2. Backend Integration

#### 2.1 Chart Renderer Client

New module: `apps/backend/src/services/chart_renderer.py`

```python
class ChartRendererClient:
    """Client for the ChartML renderer service."""

    def __init__(self, base_url: str = None):
        self.base_url = base_url or os.getenv("CHART_RENDERER_URL", "http://localhost:3030")

    async def render_chart(
        self,
        chartml_spec: dict,
        width: int = 800,
        height: int = 400
    ) -> bytes:
        """Render ChartML spec to PNG bytes."""
        async with httpx.AsyncClient() as client:
            response = await client.post(
                f"{self.base_url}/render",
                json={
                    "chartMLSpec": chartml_spec,
                    "width": width,
                    "height": height
                },
                timeout=30.0
            )
            response.raise_for_status()
            result = response.json()
            return base64.b64decode(result["image"])

    async def health_check(self) -> bool:
        """Check if renderer service is available."""
        try:
            async with httpx.AsyncClient() as client:
                response = await client.get(f"{self.base_url}/health", timeout=5.0)
                return response.status_code == 200
        except Exception:
            return False
```

#### 2.2 ChartML Detection & Extraction

The LLM agent already generates ChartML specs. We need to detect and extract them from responses.

```python
def extract_chartml_from_response(response_text: str) -> list[dict]:
    """Extract ChartML specs from LLM response text.

    Looks for YAML blocks that start with 'type: chart'
    """
    # Pattern: ```yaml or ``` followed by type: chart
    # Returns list of parsed ChartML specs
```

#### 2.3 Slack File Upload

Extend the Slack integration to upload chart images:

```python
async def upload_chart_to_slack(
    slack_client: AsyncWebClient,
    channel_id: str,
    thread_ts: str,
    png_bytes: bytes,
    title: str = "Chart"
) -> str:
    """Upload PNG chart to Slack and return the file permalink."""

    response = await slack_client.files_upload_v2(
        channel=channel_id,
        thread_ts=thread_ts,
        file=png_bytes,
        filename=f"chart_{int(time.time())}.png",
        title=title
    )
    return response["file"]["permalink"]
```

### 3. Integration Points

#### 3.1 Modify `_post_slack_response()` in `slack_integration.py`

Currently posts text only. Needs to:
1. Detect ChartML in response
2. Render to PNG via chart renderer service
3. Upload PNG to Slack
4. Post message with chart and cleaned text

```python
async def _post_slack_response(
    workspace: Workspace,
    channel_id: str,
    thread_ts: str,
    response: str,
    session_id: str,
    frontend_url: str,
):
    """Post formatted AI response to Slack, rendering any charts."""

    # 1. Extract ChartML specs from response
    chartml_specs = extract_chartml_from_response(response)
    file_ids = []

    if chartml_specs:
        chart_renderer = ChartRendererClient()

        for spec in chartml_specs:
            try:
                # 2. Render chart to PNG
                png_bytes = await chart_renderer.render_chart(spec)

                # 3. Upload to Slack
                file_response = await slack_client.files_upload_v2(
                    channel=channel_id,
                    thread_ts=thread_ts,
                    content=png_bytes,
                    filename=f"chart_{spec.get('title', 'chart').replace(' ', '_')}.png",
                    title=spec.get("title", "Chart"),
                )
                file_ids.append(file_response["file"]["id"])

            except Exception as e:
                logger.error(f"Failed to render/upload chart: {e}")

        # 4. Remove raw ChartML from text (user sees chart image, not YAML)
        response = remove_chartml_blocks(response)

    # 5. Post message (with file attachments if any)
    await slack_client.chat_postMessage(
        channel=channel_id,
        thread_ts=thread_ts,
        text=response,
        # Files are already uploaded to thread, they'll appear
    )
```

#### 3.2 Original Design: Slack Message Handler

Modify `slack_integration.py` to render charts:

```python
async def handle_app_mention(event: dict, client: AsyncWebClient):
    # ... existing code to process with LLM agent ...

    response_text = agent_response.content

    # Check for ChartML in response
    chartml_specs = extract_chartml_from_response(response_text)

    if chartml_specs:
        # Render each chart
        chart_renderer = ChartRendererClient()

        for spec in chartml_specs:
            try:
                png_bytes = await chart_renderer.render_chart(spec)
                await upload_chart_to_slack(
                    client,
                    event["channel"],
                    event["ts"],
                    png_bytes,
                    title=spec.get("title", "Chart")
                )
            except Exception as e:
                logger.error(f"Failed to render chart: {e}")

        # Remove ChartML blocks from text response (show summary only)
        response_text = remove_chartml_blocks(response_text)

    # Post text response
    await client.chat_postMessage(
        channel=event["channel"],
        thread_ts=event["ts"],
        text=response_text
    )
```

## Deployment

### Docker Compose (Development)

```yaml
services:
  backend:
    # ... existing config ...
    environment:
      - CHART_RENDERER_URL=http://chart-renderer:3030
    depends_on:
      - chart-renderer

  chart-renderer:
    build: ./apps/chart-renderer
    ports:
      - "3030:3030"
    environment:
      - CHART_RENDERER_PORT=3030
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3030/health"]
      interval: 30s
      timeout: 10s
      retries: 3
```

### Production

- Deploy chart-renderer as a separate service
- Or bundle with backend using Docker multi-stage build
- Consider scaling: chart rendering is CPU-intensive

## Slack Permissions

Current bot scopes (from `slack_integration.py`):
```
chat:write,channels:read,groups:read,commands,app_mentions:read
```

**Required addition**: `files:write` - needed to upload chart PNGs to Slack.

Update `BOT_SCOPES` in `slack_integration.py`:
```python
BOT_SCOPES = "chat:write,channels:read,groups:read,commands,app_mentions:read,files:write"
```

Note: Existing Slack app installations will need to be re-authorized to gain the new scope.

## Configuration

New environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `CHART_RENDERER_URL` | `http://localhost:3030` | Chart renderer service URL |
| `CHART_RENDERER_TIMEOUT` | `30` | Timeout in seconds for render requests |
| `CHART_DEFAULT_WIDTH` | `800` | Default chart width in pixels |
| `CHART_DEFAULT_HEIGHT` | `400` | Default chart height in pixels |

## Error Handling

| Scenario | Behavior |
|----------|----------|
| Renderer service unavailable | Post text response with message: "📊 Chart could not be rendered" |
| Render timeout (>30s) | Log error, skip chart, post text only |
| Invalid ChartML spec | Log validation error, skip that chart |
| Slack file upload fails | Log error, post text with link to web UI |
| No ChartML in response | Normal text response (no change) |

### Fallback Message Format

When chart rendering fails:
```
Here's what I found:

[Text summary of results]

📊 *Chart could not be rendered.* View the interactive chart at: https://kyomi.ai/chat/{session_id}
```

## Testing

1. **Unit tests**: ChartML extraction, spec validation
2. **Integration tests**: Full flow from Slack event to chart upload
3. **Load tests**: Multiple concurrent chart renders

## Risks

| Risk | Severity | Likelihood | Mitigation |
|------|----------|------------|------------|
| **Agent doesn't generate ChartML in Slack context** | High | Medium | Need to verify agent prompts work for Slack. May need prompt changes or explicit instruction to include charts. |
| **ChartML extraction is fragile** | Medium | Medium | LLM output format varies. Regex/YAML parsing may fail on edge cases. Need robust extraction with fallback. |
| **Slack re-authorization required** | Medium | Certain | Adding `files:write` scope requires all existing installations to re-auth. Need user communication / migration plan. |
| **Chart renderer service down** | Medium | Low | Backend depends on renderer. Need health checks, circuit breaker, graceful fallback to text-only. |
| **Memory pressure under load** | Medium | Low | JSDOM + D3 + sharp uses significant memory per render. Concurrent requests could cause OOM. Consider: request queuing, memory limits, horizontal scaling. |
| **JSDOM SVG mocks incomplete** | Low | Medium | We mocked `getBBox`, `getTotalLength`, etc. Complex charts may use other unmocked APIs and fail. Will discover during testing. |
| **Slack file upload rate limits** | Low | Low | Slack has rate limits. Unlikely to hit with normal usage, but heavy use could trigger throttling. |
| **Render timeout in slow cases** | Low | Low | Complex charts with lots of data points may exceed 30s timeout. Need to test with realistic data sizes. |

### Highest Risk: Agent ChartML Generation

The design assumes the agent generates ChartML, but this needs verification:

1. **Check current behavior**: Does the web UI agent actually output ChartML YAML in responses?
2. **Check Slack context**: Is the agent system prompt the same for Slack as web?
3. **Fallback plan**: If agent doesn't generate ChartML, we need Option A (explicit `render_chart` tool)

**Recommendation**: Before implementing, manually test asking the agent to create a chart in the web UI and verify it outputs ChartML in the response.

## Future Enhancements

1. **Chart caching**: Cache rendered PNGs by spec hash
2. **Interactive charts**: Generate both PNG (for Slack) and interactive URL
3. **Chart templates**: Pre-defined chart styles for common queries
4. **Async rendering**: Queue-based rendering for better reliability

## Implementation Phases

### Phase 1: Basic Integration (MVP)

| Task | File(s) | Description |
|------|---------|-------------|
| 1.1 | `services/chart_renderer.py` | Create `ChartRendererClient` with `render_chart()` and `health_check()` |
| 1.2 | `services/chartml_utils.py` | Create `extract_chartml_from_response()` and `remove_chartml_blocks()` |
| 1.3 | `routers/slack_integration.py` | Modify `_post_slack_response()` to detect/render/upload charts |
| 1.4 | `apps/chart-renderer/Dockerfile` | Create Dockerfile for chart-renderer service |
| 1.5 | `docker-compose.dev.yml` | Add chart-renderer service |
| 1.6 | `.env.example` | Add `CHART_RENDERER_URL` |
| 1.7 | Manual test | Test full flow: Slack @mention → query → chart → PNG in thread |

### Phase 2: Production Ready

| Task | Description |
|------|-------------|
| 2.1 | Add circuit breaker for renderer service calls |
| 2.2 | Add structured logging for chart render events |
| 2.3 | Production docker-compose / deployment config |
| 2.4 | Add Slack bot scope `files:write` if not present |
| 2.5 | Graceful degradation when renderer is down |

### Phase 3: Enhancements

| Task | Description |
|------|-------------|
| 3.1 | Cache rendered PNGs by spec hash (Redis) |
| 3.2 | Support more chart types (pie, metric cards via plugins) |
| 3.3 | Chart size/style customization via Slack command |
