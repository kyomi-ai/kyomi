// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Declarative Aggregate Stage
 *
 * Compiles a ChartML aggregate config (dimensions, measures, filters, sort, limit)
 * into SQL via buildAggregateSQL, executes it against the single input table, and
 * materializes the result as a new DuckDB table.
 *
 * This is a standalone stage module. It does NOT import other stages, the pipeline
 * runner, or the caching layer. All source table access goes through the sourceMap
 * parameter, not context.
 *
 * @module aggregateStage
 */

import { buildAggregateSQL } from '../transformSQLBuilder.js';
import { hash } from '../hash.js';

/**
 * Declarative aggregate stage.
 * Grabs first table from source map, compiles aggregate config to SQL, executes, materializes.
 *
 * @param {Object} sourceMap - { name: tableId } map (expects single entry from previous stage)
 * @param {Object} aggregateConfig - { dimensions, measures, filters, sort, limit }
 * @param {Object} context - { runSQL, execute }
 * @returns {Promise<Object>} output sourceMap (single entry: { _result: outputTableId })
 */
export async function aggregateStage(sourceMap, aggregateConfig, context) {
  // Guard: aggregate stage only handles single-table input
  const entries = Object.keys(sourceMap);
  if (entries.length > 1) {
    throw new Error(
      `aggregate stage operates on a single table but received ${entries.length} tables (${entries.join(', ')}). ` +
      `Use the sql stage to join multiple sources first.`
    );
  }

  const tableId = Object.values(sourceMap)[0];
  const sql = buildAggregateSQL(tableId, aggregateConfig);

  const configKey = JSON.stringify(aggregateConfig);
  const h = hash(configKey);
  const outputTableId = `__stage_agg_${h}`;

  await context.execute(`CREATE OR REPLACE TABLE "${outputTableId}" AS ${sql}`);

  return { _result: outputTableId };
}
