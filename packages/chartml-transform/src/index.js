// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * @kyomi/chartml-transform
 *
 * Shared DuckDB transform pipeline for ChartML.
 * Used by both the frontend (WASM DuckDB) and chart-renderer (Node native DuckDB).
 *
 * Each runtime provides its own thin DuckDB adapter via the `context` parameter
 * ({ execute, runSQL }). This package contains only pure transform logic — no
 * DuckDB imports, no runtime-specific code.
 */

export { hash } from './hash.js';
export { RESERVED_DATA_KEYS, isNamedSources, replacePlaceholders } from './helpers.js';
export { buildAggregateSQL, requiresAggregation } from './transformSQLBuilder.js';
export { sqlStage } from './stages/sqlStage.js';
export { aggregateStage } from './stages/aggregateStage.js';
export { forecastStage, buildForecastSQL } from './stages/forecastStage.js';
export { runTransformPipeline } from './pipeline.js';
