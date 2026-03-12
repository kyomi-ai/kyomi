# @chartml/core

A declarative markup language for creating beautiful, interactive data visualizations.

ChartML combines the simplicity of YAML with the power of D3.js to make data visualization accessible to everyone. Write charts in clean, readable markup - no JavaScript required.

## Features

- 📝 **Simple & Declarative** - Write charts in clean YAML markup
- 🎨 **Beautiful by Default** - Professional visualizations with sensible defaults
- ⚡ **Interactive** - Built-in tooltips, zooming, panning
- 📊 **Comprehensive** - Bar, line, area, pie, scatter, tables, metrics
- 🔌 **Extensible** - Plugin system for custom data sources
- 📦 **Lightweight** - 17KB gzipped, zero dependencies beyond D3
- 🌐 **Universal** - Works in any JavaScript environment

## Installation

```bash
npm install @chartml/core d3 js-yaml
```

## Quick Start

```javascript
import { renderChart } from '@chartml/core';

const spec = `
data:
  - month: Jan
    revenue: 45000
  - month: Feb
    revenue: 52000
  - month: Mar
    revenue: 61000

visualize:
  type: line
  columns: month
  rows: revenue
  style:
    title: "Monthly Revenue"
    width: 800
    height: 400
`;

await renderChart(spec, document.getElementById('chart'));
```

## Chart Types

ChartML supports all major chart types:

- **Bar Charts** - Vertical, horizontal, stacked, grouped
- **Line Charts** - Single, multi-series, with dots
- **Area Charts** - Stacked, overlapping, with curves
- **Pie/Doughnut** - With percentages and legends
- **Scatter Plots** - With size and color encoding
- **Metric Cards** - KPIs with comparison indicators
- **Tables** - Sortable, paginated data tables

## Data Sources

### Inline Data (Built-in)

```yaml
data:
  - category: A
    value: 100
  - category: B
    value: 200
```

### HTTP Data (Built-in)

```yaml
data: https://api.example.com/data.json
```

### Plugin Data Sources

Extend ChartML with custom data sources:

```javascript
import { ChartML } from '@chartml/core';

const chartml = new ChartML();

// Register BigQuery plugin
chartml.registerDataSource('bigquery', async (spec) => {
  // Execute BigQuery and return rows
  return rows;
});

// Use in ChartML
const spec = `
data:
  type: bigquery
  query: "SELECT * FROM dataset.table"
`;
```

## Styling & Customization

```yaml
visualize:
  type: bar
  columns: month
  rows: revenue
  style:
    title: "Revenue by Month"
    width: 800
    height: 400
    colors: ['#3b82f6', '#8b5cf6', '#ec4899']
    palette: spectrum_pro  # or autumn_forest, horizon_suite
```

## Advanced Features

### Multi-Series Charts

```yaml
visualize:
  type: line
  columns: date
  rows:
    - field: revenue
      label: "Revenue"
    - field: expenses
      label: "Expenses"
```

### Dual Axis

```yaml
visualize:
  type: line
  columns: month
  rows:
    - field: revenue
      axis: left
    - field: growth_rate
      axis: right
      mark: bar
```

### Annotations

```yaml
visualize:
  type: line
  columns: date
  rows: value
  annotations:
    - type: line
      value: 100
      label: "Target"
```

## API Reference

### `renderChart(spec, container)`

Render a ChartML specification into a DOM container.

```javascript
await renderChart(spec, document.getElementById('chart'));
```

### `ChartML` Class

Create a ChartML instance for advanced usage:

```javascript
const chartml = new ChartML({
  palettes: {
    custom: ['#ff0000', '#00ff00', '#0000ff']
  }
});

// Register plugins
chartml.registerDataSource('postgres', handler);
chartml.registerAggregateMiddleware(duckdbMiddleware);

// Render
await chartml.render(spec, container);
```

## Plugin System

### Data Source Plugins

```javascript
chartml.registerDataSource('name', async (spec) => {
  // Fetch data from your source
  // Must return an array of objects
  return [{ col1: 'value', col2: 123 }, ...];
});
```

### Aggregate Middleware

```javascript
chartml.registerAggregateMiddleware(async (data, aggregateSpec) => {
  // Transform data (groupby, aggregate, etc.)
  return transformedData;
});
```

## Examples

See the [ChartML specification](https://chartml.org/spec/) for complete documentation and [42 real-world examples](https://chartml.org/examples/).

## TypeScript

TypeScript declarations are included. For type-safe ChartML:

```typescript
import { ChartML, renderChart } from '@chartml/core';
```

## Browser Support

ChartML works in all modern browsers that support ES modules:

- Chrome/Edge 89+
- Firefox 89+
- Safari 15+

## License

MIT License - see LICENSE file for details

## Links

- **Documentation:** [chartml.org](https://chartml.org)
- **Specification:** [chartml.org/spec](https://chartml.org/spec/)
- **Examples:** [chartml.org/examples](https://chartml.org/examples/)
- **GitHub:** [github.com/kyomi-ai/kyomi](https://github.com/kyomi-ai/kyomi)
- **Issues:** [github.com/kyomi-ai/kyomi/issues](https://github.com/kyomi-ai/kyomi/issues)

## Contributing

Contributions welcome! Please see the [contributing guide](https://github.com/kyomi-ai/kyomi/blob/main/CONTRIBUTING.md).

---

Built with ❤️ by the [Kyomi](https://kyomi.app) team
