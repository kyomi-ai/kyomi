// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Date Parser Utility
 *
 * Converts ISO date/timestamp strings to JavaScript Date objects.
 * Used to parse API responses from backend before storing in DuckDB or displaying in charts/tables.
 */

/**
 * Parse datetime columns from ISO strings to Date objects
 *
 * Converts values in datetime columns from ISO 8601 strings to JavaScript Date objects.
 * This enables proper date handling in charts (which expect Date objects) and tables
 * (which format Date objects correctly).
 *
 * Recognizes granular date/time types from backend:
 * - date: Date only (YYYY-MM-DD)
 * - time: Time only (HH:MM:SS)
 * - timestamp: Date + time without timezone
 * - timestamptz: Date + time with timezone
 * - datetime: Legacy type (still supported)
 *
 * @param {Array} columns - Column metadata [{name, type}] or string[]
 * @param {Array[]} rows - Row data [[val1, val2], ...]
 * @returns {Array[]} Rows with datetime values parsed to Date objects
 *
 * @example
 * const columns = [{name: 'created_at', type: 'timestamp'}, {name: 'value', type: 'number'}];
 * const rows = [['2024-01-15T10:30:45Z', 100], ['2024-01-15T11:00:00Z', 200]];
 * const parsed = parseDateColumns(columns, rows);
 * // Result: [[Date('2024-01-15T10:30:45Z'), 100], [Date('2024-01-15T11:00:00Z'), 200]]
 */
export function parseDateColumns(columns, rows) {
  // Date/time types that should be parsed to Date objects
  const DATE_TIME_TYPES = new Set(['date', 'time', 'timestamp', 'timestamptz', 'datetime']);

  // Find indices of datetime columns
  const dateColumnIndices = [];

  columns.forEach((col, idx) => {
    // Handle both object format {name, type} and string format
    const colType = typeof col === 'string' ? null : col.type;

    if (colType && DATE_TIME_TYPES.has(colType)) {
      dateColumnIndices.push(idx);
    }
  });

  // If no datetime columns, return original rows (no parsing needed)
  if (dateColumnIndices.length === 0) {
    return rows;
  }

  // Parse datetime strings to Date objects
  return rows.map(row => {
    const newRow = [...row]; // Shallow copy to avoid mutating original

    dateColumnIndices.forEach(idx => {
      const value = row[idx];

      // Only parse if it's a non-empty string
      if (typeof value === 'string' && value) {
        const date = new Date(value);

        // Only replace if parsing was successful
        if (!isNaN(date.getTime())) {
          newRow[idx] = date;
        }
        // If parsing failed, leave original string value
      }
      // If value is null, undefined, or already a Date, leave as-is
    });

    return newRow;
  });
}
