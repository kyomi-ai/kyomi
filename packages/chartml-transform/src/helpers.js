// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Shared helpers for the transform pipeline.
 *
 * @module helpers
 */

/**
 * Reserved keys that indicate an unnamed (legacy) data source.
 * Any spec.data object whose keys are all in this set is treated as
 * an unnamed source (passthrough), not a named multi-source spec.
 */
export const RESERVED_DATA_KEYS = new Set(['datasource', 'provider', 'query', 'rows', 'url', 'cache']);

/**
 * Detect whether spec.data uses named sources or is an unnamed source.
 *
 * @param {*} dataSpec - The spec.data value
 * @returns {boolean} True if named sources, false if unnamed
 */
export function isNamedSources(dataSpec) {
  if (typeof dataSpec === 'string') return false;
  if (!dataSpec || typeof dataSpec !== 'object') return false;
  if (Array.isArray(dataSpec)) return false;
  return !Object.keys(dataSpec).some(key => RESERVED_DATA_KEYS.has(key));
}

/**
 * Replace {sourceName} placeholders in a SQL string with actual table identifiers.
 *
 * @param {string} sql - SQL string with {name} placeholders
 * @param {Object} sourceMap - { name: tableId } map
 * @returns {string} SQL with placeholders replaced by quoted table identifiers
 */
export function replacePlaceholders(sql, sourceMap) {
  let result = sql;
  for (const [name, tableId] of Object.entries(sourceMap)) {
    result = result.replaceAll(`{${name}}`, `"${tableId}"`);
  }
  return result;
}
