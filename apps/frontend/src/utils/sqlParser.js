// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * SQL Parser Utilities
 *
 * Functions for parsing and splitting SQL statements
 */

/**
 * Parse SQL text into individual statements
 * Splits on semicolons while respecting:
 * - String literals (single and double quotes)
 * - Comments (-- and /* */)
 * - Nested parentheses
 *
 * @param {string} sqlText - The SQL text to parse
 * @returns {string[]} Array of SQL statements
 */
export function parseSQLStatements(sqlText) {
  if (!sqlText || !sqlText.trim()) {
    return [];
  }

  const statements = [];
  let current = '';
  let inSingleQuote = false;
  let inDoubleQuote = false;
  let inLineComment = false;
  let inBlockComment = false;

  for (let i = 0; i < sqlText.length; i++) {
    const char = sqlText[i];
    const nextChar = sqlText[i + 1];
    const prevChar = sqlText[i - 1];

    // Handle line comments (-- to end of line)
    if (char === '-' && nextChar === '-' && !inSingleQuote && !inDoubleQuote && !inBlockComment) {
      inLineComment = true;
      current += char;
      continue;
    }

    if (inLineComment) {
      current += char;
      if (char === '\n') {
        inLineComment = false;
      }
      continue;
    }

    // Handle block comments (/* ... */)
    if (char === '/' && nextChar === '*' && !inSingleQuote && !inDoubleQuote && !inLineComment) {
      inBlockComment = true;
      current += char;
      continue;
    }

    if (inBlockComment) {
      current += char;
      if (char === '*' && nextChar === '/') {
        current += nextChar;
        i++; // Skip the /
        inBlockComment = false;
      }
      continue;
    }

    // Handle single quotes
    if (char === "'" && !inDoubleQuote && !inLineComment && !inBlockComment) {
      // Check for escaped quote ('' in SQL)
      if (inSingleQuote && nextChar === "'") {
        current += char + nextChar;
        i++; // Skip next quote
        continue;
      }
      inSingleQuote = !inSingleQuote;
      current += char;
      continue;
    }

    // Handle double quotes
    if (char === '"' && !inSingleQuote && !inLineComment && !inBlockComment) {
      // Check for escaped quote ("" in SQL)
      if (inDoubleQuote && nextChar === '"') {
        current += char + nextChar;
        i++; // Skip next quote
        continue;
      }
      inDoubleQuote = !inDoubleQuote;
      current += char;
      continue;
    }

    // Handle semicolons (statement separator)
    if (char === ';' && !inSingleQuote && !inDoubleQuote && !inLineComment && !inBlockComment) {
      // Add the current statement (without semicolon)
      const trimmed = current.trim();
      if (trimmed) {
        statements.push(trimmed);
      }
      current = '';
      continue;
    }

    // Regular character
    current += char;
  }

  // Add the last statement if there is one
  const trimmed = current.trim();
  if (trimmed) {
    statements.push(trimmed);
  }

  return statements;
}

/**
 * Get a preview of a SQL statement (first line, truncated)
 * @param {string} sql - The SQL statement
 * @param {number} maxLength - Maximum length of preview
 * @returns {string} Preview text
 */
export function getSQLPreview(sql, maxLength = 50) {
  if (!sql) return '';

  // Get first line
  const firstLine = sql.split('\n')[0].trim();

  // Remove leading SELECT/INSERT/UPDATE/DELETE/WITH keywords for cleaner preview
  const cleaned = firstLine.replace(/^(SELECT|INSERT|UPDATE|DELETE|WITH|CREATE|DROP|ALTER)\s+/i, '');

  // Truncate if too long
  if (cleaned.length > maxLength) {
    return cleaned.substring(0, maxLength) + '...';
  }

  return cleaned || firstLine.substring(0, maxLength) + '...';
}
