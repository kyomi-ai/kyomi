// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Forecast Stage
 *
 * Grabs first table from source map, runs QuackStats forecast(),
 * UNION ALLs historical + forecast rows, materializes result.
 *
 * This is a standalone stage module. It does NOT import other stages, the pipeline
 * runner, or the caching layer. All source table access goes through the sourceMap
 * parameter, not context.
 *
 * @module forecastStage
 */

import { hash } from '../hash.js';

/**
 * Build the 3-step forecast SQL sequence.
 *
 * Pure function — no side effects, no DuckDB access.
 * Returns an array of SQL statements and the final output table ID.
 *
 * @param {string} inputTableId - The DuckDB table to forecast from
 * @param {Object} forecastConfig - { timestamp, value, horizon, confidence_level, model, group_by }
 * @returns {{ statements: string[], outputTableId: string }}
 */
export function buildForecastSQL(inputTableId, forecastConfig) {
  const {
    timestamp,
    value,
    horizon = 3,
    confidence_level = 0.95,
    model = 'auto',
    group_by = [],
  } = forecastConfig;

  const h = hash(inputTableId + JSON.stringify(forecastConfig));
  const srcTable = `__stage_fcast_src_${h}`;
  const predTable = `__stage_fcast_pred_${h}`;
  const outputTableId = `__stage_fcast_${h}`;

  // Columns selected from input: timestamp, value, and any group_by columns
  const selectCols = [timestamp, value, ...group_by].join(', ');

  // Step 1: Materialize input into forecast source table
  const step1 = `CREATE OR REPLACE TABLE "${srcTable}" AS SELECT ${selectCols} FROM "${inputTableId}"`;

  // Step 2: Run forecast()
  const groupByClause = group_by.length > 0
    ? `, group_by = [${group_by.map(c => `'${c}'`).join(', ')}]`
    : '';

  const step2 =
    `CREATE OR REPLACE TABLE "${predTable}" AS SELECT * FROM forecast(` +
    `'${srcTable}', ` +
    `timestamp = '${timestamp}', ` +
    `value = '${value}', ` +
    `horizon = ${horizon}, ` +
    `confidence_level = ${confidence_level}, ` +
    `model = '${model}'` +
    `${groupByClause})`;

  // Step 3: UNION ALL historical + forecast rows
  const groupByCols = group_by.length > 0 ? group_by.join(', ') + ', ' : '';

  const historicalSelect =
    `SELECT ${timestamp}, ${value}, ${groupByCols}` +
    `NULL as forecast, NULL as lower_bound, NULL as upper_bound, FALSE as is_forecast ` +
    `FROM "${srcTable}"`;

  const forecastSelect =
    `SELECT forecast_timestamp as ${timestamp}, NULL as ${value}, ${groupByCols}` +
    `forecast, lower_bound, upper_bound, TRUE as is_forecast ` +
    `FROM "${predTable}"`;

  const orderBy = `ORDER BY ${groupByCols}${timestamp}`;

  const step3 =
    `CREATE OR REPLACE TABLE "${outputTableId}" AS ` +
    `${historicalSelect} UNION ALL ${forecastSelect} ${orderBy}`;

  return {
    statements: [step1, step2, step3],
    outputTableId,
  };
}

/**
 * Forecast stage.
 * Grabs first table from source map, runs QuackStats forecast(),
 * UNION ALLs historical + forecast rows, materializes result.
 *
 * @param {Object} sourceMap - { name: tableId } map (expects single entry)
 * @param {Object} forecastConfig - { timestamp, value, horizon, confidence_level, model, group_by }
 * @param {Object} context - { runSQL, execute }
 * @returns {Promise<Object>} output sourceMap: { _result: outputTableId }
 */
export async function forecastStage(sourceMap, forecastConfig, context) {
  // Guard: forecast stage only handles single-table input
  const entries = Object.keys(sourceMap);
  if (entries.length > 1) {
    throw new Error(
      `forecast stage operates on a single table but received ${entries.length} tables (${entries.join(', ')}). ` +
      `Use the sql stage to join multiple sources first.`
    );
  }

  const inputTableId = Object.values(sourceMap)[0];
  const { statements, outputTableId } = buildForecastSQL(inputTableId, forecastConfig);

  for (const sql of statements) {
    await context.execute(sql);
  }

  return { _result: outputTableId };
}
