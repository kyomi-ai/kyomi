// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * SQL Transform Stage
 *
 * Executes SQL with {sourceName} placeholder replacement against DuckDB tables.
 * Materializes the result as a new table and returns a single-entry source map.
 *
 * This is a standalone stage module. It does NOT import other stages, the pipeline
 * runner, or the caching layer. All source table access goes through the sourceMap
 * parameter, not context.
 *
 * @module sqlStage
 */

import { hash } from '../hash.js';
import { replacePlaceholders } from '../helpers.js';

/**
 * SQL transform stage.
 *
 * Receives a source map, executes SQL with {name} placeholder replacement,
 * materializes the result as a permanent DuckDB table.
 *
 * For a single SQL string: materializes it directly.
 * For an array of SQL strings: executes all but the last as setup statements
 * (via context.execute), then materializes the last statement.
 *
 * @param {Object} sourceMap - { name: tableId } map of available tables
 * @param {string|string[]} sqlConfig - SQL string or array of SQL strings
 * @param {Object} context - { runSQL, execute }
 * @returns {Promise<Object>} output sourceMap (single entry: { _result: outputTableId })
 */
export async function sqlStage(sourceMap, sqlConfig, context) {
  const statements = Array.isArray(sqlConfig) ? sqlConfig : [sqlConfig];

  if (statements.length === 0) {
    throw new Error('sqlStage: sql config must contain at least one SQL statement');
  }

  // Replace placeholders in all statements
  const resolved = statements.map(stmt => replacePlaceholders(stmt, sourceMap));

  // Execute setup statements (all but the last)
  for (let i = 0; i < resolved.length - 1; i++) {
    await context.execute(resolved[i]);
  }

  // Materialize the final statement as a new table
  const finalSQL = resolved[resolved.length - 1];
  const configKey = JSON.stringify(sqlConfig);
  const h = hash(configKey);
  const outputTableId = `__stage_sql_${h}`;

  await context.execute(`CREATE OR REPLACE TABLE "${outputTableId}" AS ${finalSQL}`);

  return { _result: outputTableId };
}
