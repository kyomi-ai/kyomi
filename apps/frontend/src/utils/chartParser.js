// SPDX-License-Identifier: AGPL-3.0-or-later
import yaml from 'js-yaml';

/**
 * Chart Configuration Versioning
 *
 * Chart configs include a `version` field to handle breaking changes:
 * - Version 1 (current): Initial chart config format
 * - Future versions: When making breaking changes, increment version
 *   and add migration logic in ChartRenderer to convert old configs
 *
 * Example migration pattern:
 *   if (chartVersion === 1) {
 *     // Migrate v1 -> v2
 *     chartData = { ...chartData, version: 2, newField: defaultValue };
 *   }
 */

/**
 * Serialize chart data to YAML or JSON format
 * @param {Object|Array} chartData - The chart data object or array to serialize
 * @param {string} format - 'yaml' or 'json'
 * @returns {string} Serialized chart configuration
 */
export function serializeChart(chartData, format = 'yaml') {
  if (!chartData || (typeof chartData !== 'object' && !Array.isArray(chartData))) {
    throw new Error('Chart data must be a valid object or array');
  }

  if (format === 'yaml') {
    return yaml.dump(chartData, {
      indent: 2,
      lineWidth: 80,
      noRefs: true,
      sortKeys: false,
      noCompatMode: true
    });
  } else if (format === 'json') {
    return JSON.stringify(chartData, null, 2);
  } else {
    throw new Error(`Unknown format: ${format}. Must be 'yaml' or 'json'`);
  }
}

/**
 * Chart type categories — each uses a different visualize structure:
 * - chart: columns (x-axis) + rows (y-axis values)
 * - table: columns as array of {field, label, width, format}
 * - metric: value, label, format, compareWith, invertTrend
 */
function getTypeCategory(type) {
  if (type === 'table') return 'table';
  if (type === 'metric') return 'metric';
  return 'chart';
}

/**
 * Convert visualize properties when switching between incompatible chart type categories.
 * Mutates the `viz` object in place.
 *
 * @param {Object} viz - The visualize block to convert (mutated in place)
 * @param {string} fromType - Previous chart type
 * @param {string} toType - New chart type
 */
export function convertVisualizeForTypeChange(viz, fromType, toType) {
  const fromCat = getTypeCategory(fromType);
  const toCat = getTypeCategory(toType);
  if (fromCat === toCat) return; // Same category, no conversion needed

  // --- Leaving table ---
  if (fromCat === 'table') {
    const tableCols = Array.isArray(viz.columns) ? viz.columns : [];
    if (toCat === 'chart') {
      // Table → chart: first column as x-axis, rest as y-axis rows
      const first = tableCols[0];
      const rest = tableCols.slice(1);
      viz.columns = first?.field || first;
      viz.rows = rest.map(c => {
        if (typeof c === 'string') return c;
        const { width, ...kept } = c;
        return Object.keys(kept).length === 1 && kept.field ? kept.field : kept;
      });
    } else if (toCat === 'metric') {
      // Table → metric: use first numeric-looking column as value
      const valueCol = tableCols.length > 1 ? tableCols[1] : tableCols[0];
      viz.value = valueCol?.field || valueCol;
      if (valueCol?.label) viz.label = valueCol.label;
      if (valueCol?.format) viz.format = valueCol.format;
      delete viz.columns;
      delete viz.rows;
    }
  }

  // --- Leaving metric ---
  else if (fromCat === 'metric') {
    const metricValue = viz.value;
    const metricLabel = viz.label;
    const metricFormat = viz.format;
    // Clean metric-specific props
    delete viz.value;
    delete viz.label;
    delete viz.format;
    delete viz.compareWith;
    delete viz.invertTrend;

    if (toCat === 'chart') {
      // Metric → chart: value field becomes the sole row, no columns (will need user config)
      delete viz.columns;
      const row = {};
      if (metricValue) row.field = metricValue;
      if (metricLabel) row.label = metricLabel;
      if (metricFormat) row.format = metricFormat;
      viz.rows = row.field ? [Object.keys(row).length === 1 ? row.field : row] : [];
    } else if (toCat === 'table') {
      // Metric → table: value becomes a column definition
      const cols = [];
      if (metricValue) {
        const col = { field: metricValue };
        if (metricLabel) col.label = metricLabel;
        if (metricFormat) col.format = metricFormat;
        cols.push(col);
      }
      viz.columns = cols;
      delete viz.rows;
    }
  }

  // --- Leaving chart ---
  else if (fromCat === 'chart') {
    const srcCols = Array.isArray(viz.columns) ? viz.columns : viz.columns ? [viz.columns] : [];
    const srcRows = Array.isArray(viz.rows) ? viz.rows : viz.rows ? [viz.rows] : [];

    if (toCat === 'table') {
      // Chart → table: merge columns + rows into flat column definitions
      const cols = [];
      const addField = (f) => {
        if (typeof f === 'string') cols.push({ field: f });
        else if (f && typeof f === 'object') cols.push({ ...f });
      };
      srcCols.forEach(addField);
      srcRows.forEach(addField);
      viz.columns = cols;
      delete viz.rows;
    } else if (toCat === 'metric') {
      // Chart → metric: first row field becomes value
      const firstRow = srcRows[0];
      viz.value = typeof firstRow === 'string' ? firstRow : firstRow?.field;
      if (typeof firstRow === 'object' && firstRow?.label) viz.label = firstRow.label;
      if (typeof firstRow === 'object' && firstRow?.format) viz.format = firstRow.format;
      delete viz.columns;
      delete viz.rows;
    }
  }
}

