# ChartML Renderer Service (POC)

Node.js service for rendering ChartML charts to PNG images without a browser.

## Architecture

```
Python Backend
    ↓
    | HTTP POST /render
    | - ChartML spec (YAML)
    | - Inline data
    ↓
Node.js Service (JSDOM + node-canvas)
    ↓
    1. Parse ChartML YAML
    2. Create JSDOM environment
    3. Mock SVG measurement APIs (getBBox, getComputedTextLength)
    4. Render with D3/ChartML
    5. Extract SVG string
    6. Convert SVG → PNG (sharp)
    ↓
    | Returns base64 PNG
    ↓
Python Backend → Slack
```

## Key Design Decisions

### Why JSDOM instead of a browser?

- **No Chromium dependency** (~500MB vs ~20MB)
- **Faster** (50ms vs 1-2s browser launch)
- **Lower memory** (30MB vs 300MB)
- **Simpler** (no page lifecycle, timeouts, or screenshot APIs)

### Why node-canvas?

ChartML uses `getBBox()` and `getComputedTextLength()` to measure text for layout.
JSDOM has no layout engine, so we mock these APIs using Canvas text measurement.

### Why inline data?

ChartML supports fetching data from HTTP sources. In Kyomi's architecture:
- Python backend runs queries with user credentials
- Backend passes inline data to renderer (no authentication needed)
- Renderer is a pure rendering service (no data access)

## Installation

```bash
npm install
```

## Running the POC

### Terminal 1: Start the service
```bash
npm start
```

Service starts on `http://localhost:3030`

### Terminal 2: Run tests
```bash
npm test
```

This will:
1. Check health endpoint
2. Render a test chart
3. Save PNG to `test-output.png`

## API

### `GET /health`

Health check endpoint.

**Response:**
```json
{
  "status": "ok",
  "service": "chart-renderer",
  "version": "0.1.0"
}
```

### `POST /render`

Render ChartML specification to PNG.

**Request:**
```json
{
  "chartMLSpec": "visualize: bar\ndata:\n  - x: A\n    y: 10",
  "width": 800,
  "height": 600
}
```

**Response:**
```json
{
  "image": "iVBORw0KGgo...",
  "format": "png",
  "width": 800,
  "height": 600
}
```

## POC Success Criteria

- [x] Express server setup
- [x] JSDOM environment creation
- [x] SVG measurement API mocking (getBBox, getComputedTextLength)
- [x] Basic D3 rendering works
- [x] SVG → PNG conversion works
- [ ] Integrate real ChartML library
- [ ] Render bar chart with inline data
- [ ] Render pie chart with inline data
- [ ] Verify text doesn't overlap (measurements accurate enough)

## Next Steps (If POC Succeeds)

1. **ChartML Integration**: Load @chartml/core and chart plugins
2. **Error Handling**: Timeouts, malformed specs, rendering failures
3. **Docker**: Containerize service
4. **Python Integration**: Add HTTP client to backend
5. **Testing**: Unit tests, integration tests
6. **Production**: Deploy alongside backend

## Known Limitations

- Font rendering might differ slightly from browser
- Complex CSS styles might not work (JSDOM has limited CSS support)
- Measurements are approximate (good enough for layout, not pixel-perfect)

## Dependencies

- `express`: HTTP server
- `jsdom`: DOM environment for D3
- `canvas`: Text measurement for SVG layout
- `sharp`: SVG → PNG conversion
- `js-yaml`: ChartML spec parsing
- `d3`: SVG rendering (same version as ChartML)
