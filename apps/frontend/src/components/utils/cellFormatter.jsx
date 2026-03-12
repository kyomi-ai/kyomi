// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Cell Formatter Utilities
 *
 * Handles formatting of complex data types (JSON, arrays, structs) for display in tables
 */

/**
 * Format a cell value for display
 *
 * @param {*} cell - The cell value to format
 * @param {string} colType - Optional column type hint (DATE, DATETIME, TIMESTAMP, JSON, STRUCT, ARRAY, etc.)
 * @returns {Object} - { displayValue: React node or string, fullValue: string for tooltip, isComplex: boolean }
 */
export function formatCellValue(cell, colType = undefined) {
  // Handle null values
  if (cell === null || cell === undefined) {
    return {
      displayValue: <span className="text-muted-foreground italic">null</span>,
      fullValue: 'null',
      isComplex: false
    };
  }

  // Handle Date objects - format based on granular type from backend
  if (cell instanceof Date) {
    let displayValue;

    // Granular types from backend (lowercase)
    if (colType === 'date') {
      // DATE: Display only date part (YYYY-MM-DD)
      displayValue = cell.toISOString().split('T')[0];
    } else if (colType === 'time') {
      // TIME: Display only time part (HH:MM:SS)
      const timeStr = cell.toISOString().split('T')[1];
      displayValue = timeStr ? timeStr.split('.')[0] : cell.toISOString();
    } else if (colType === 'timestamp') {
      // TIMESTAMP (no timezone): Display as YYYY-MM-DD HH:MM:SS
      displayValue = cell.toISOString().replace('T', ' ').replace(/\.\d{3}Z$/, '');
    } else if (colType === 'timestamptz') {
      // TIMESTAMP WITH TIMEZONE: Display with timezone (YYYY-MM-DD HH:MM:SS+00:00)
      displayValue = cell.toISOString().replace('T', ' ').replace('Z', '+00:00');
    } else if (colType === 'datetime') {
      // Legacy datetime type: Display as YYYY-MM-DD HH:MM:SS
      displayValue = cell.toISOString().replace('T', ' ').replace(/\.\d{3}Z$/, '');
    } else {
      // Unknown date type or no type: Use full ISO string
      displayValue = cell.toISOString();
    }

    return {
      displayValue,
      fullValue: displayValue,
      isComplex: false
    };
  }

  // Handle objects and arrays (JSON, STRUCT, ARRAY)
  if (typeof cell === 'object') {
    try {
      // Pretty print JSON with 2-space indentation
      const jsonString = JSON.stringify(cell, null, 2);

      // For display, use compact single-line format for short values
      const compactString = JSON.stringify(cell);

      // If compact version is short enough (< 100 chars), use it directly
      // Otherwise, truncate and add ellipsis
      const MAX_DISPLAY_LENGTH = 100;
      let displayValue;

      if (compactString.length <= MAX_DISPLAY_LENGTH) {
        displayValue = compactString;
      } else {
        // Truncate and add ellipsis
        displayValue = compactString.substring(0, MAX_DISPLAY_LENGTH) + '...';
      }

      return {
        displayValue: <span className="font-mono text-info-foreground text-xs">{displayValue}</span>,
        fullValue: jsonString, // Pretty-printed for tooltip
        isComplex: true
      };
    } catch (error) {
      // If JSON.stringify fails, fall back to String()
      const strValue = String(cell);
      return {
        displayValue: strValue,
        fullValue: strValue,
        isComplex: false
      };
    }
  }

  // Handle boolean values
  if (typeof cell === 'boolean') {
    return {
      displayValue: <span className="font-mono text-primary">{String(cell)}</span>,
      fullValue: String(cell),
      isComplex: false
    };
  }

  // Handle numbers
  if (typeof cell === 'number') {
    const strValue = String(cell);
    return {
      displayValue: strValue,
      fullValue: strValue,
      isComplex: false
    };
  }

  // Handle strings (default case)
  const strValue = String(cell);

  // Check if string is very long
  const MAX_STRING_LENGTH = 200;
  if (strValue.length > MAX_STRING_LENGTH) {
    return {
      displayValue: strValue.substring(0, MAX_STRING_LENGTH) + '...',
      fullValue: strValue,
      isComplex: true // Mark as complex so tooltip shows full value
    };
  }

  return {
    displayValue: strValue,
    fullValue: strValue,
    isComplex: false
  };
}
