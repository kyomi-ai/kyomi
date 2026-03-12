// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Transform Pipeline Runner
 *
 * Orchestrates registered stages in order, passing the output sourceMap of each
 * stage as input to the next. Returns the final table ID and a list of intermediate
 * tables — the caller is responsible for post-processing (caching, reading rows)
 * and cleanup.
 *
 * @module pipeline
 */

import { sqlStage } from './stages/sqlStage.js';
import { aggregateStage } from './stages/aggregateStage.js';
import { forecastStage } from './stages/forecastStage.js';

// ---------------------------------------------------------------------------
// Stage registry — defines pipeline order
// ---------------------------------------------------------------------------
const STAGES = [
  { key: 'sql', handler: sqlStage },
  { key: 'aggregate', handler: aggregateStage },
  { key: 'forecast', handler: forecastStage },
];

/**
 * Run the transform pipeline: iterate registered stages in order.
 *
 * Returns `{ finalTableId, intermediateTables }` so each runtime can do its own
 * post-processing:
 *   - Frontend: creates __transform_{hash} cache table, cleans intermediates
 *   - Chart-renderer: reads rows from final table, cleans intermediates
 *
 * @param {Object} sourceMap - { name: tableId } map of loaded source tables
 * @param {Object} transform - The spec.transform object (e.g., { sql: "...", aggregate: {...} })
 * @param {Object} context - { runSQL, execute } wrappers for DuckDB
 * @returns {Promise<{ finalTableId: string, intermediateTables: string[] }>}
 */
export async function runTransformPipeline(sourceMap, transform, context) {
  // Pre-flight: reject multi-source + aggregate-only before starting the pipeline.
  // The aggregate stage also guards this (defense in depth), but catching it here
  // gives a clean error before any stage work begins.
  const sourceCount = Object.keys(sourceMap).length;
  if (sourceCount > 1 && transform.aggregate && !transform.sql) {
    throw new Error(
      `Cannot use aggregate stage with ${sourceCount} data sources without a sql stage to join them first.`
    );
  }

  // Run registered stages in sequence — output of each is input of next
  let currentMap = { ...sourceMap };
  const intermediateTables = [];

  for (const { key, handler } of STAGES) {
    if (transform[key]) {
      currentMap = await handler(currentMap, transform[key], context);
      // Collect intermediate table IDs for cleanup
      intermediateTables.push(...Object.values(currentMap));
    }
  }

  // Grab the single table from the last stage's output
  const finalTableId = Object.values(currentMap)[0];

  return { finalTableId, intermediateTables };
}
