// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Parse and clean up SQL error messages to extract useful information.
 *
 * Handles various BigQuery error formats:
 * - "400 POST" style errors
 * - "Syntax error:" style errors
 * - Generic errors (truncated if too long)
 *
 * @param {string} error - Raw error message from BigQuery
 * @returns {string} Cleaned and formatted error message
 */
export const formatSQLError = (error) => {
  const parts = error.split(':');

  // Handle "400 POST" style errors from BigQuery
  if (parts.length >= 3 && error.includes('400 POST')) {
    const errorMessagePart = parts.slice(2).join(':');
    const messageMatch = errorMessagePart.match(/\s*(.+?)(?:\s+at\s+\[|Location:|Job ID:|$)/i);
    if (messageMatch) {
      let message = messageMatch[1].trim();
      const locationMatch = error.match(/at\s+\[(\d+:\d+)\]/);
      if (locationMatch) {
        message += ` (line ${locationMatch[1].replace(':', ', column ')})`;
      }
      return message;
    }
  }

  // Handle "Syntax error:" style errors
  const syntaxMatch = error.match(/Syntax error:\s*(.+?)(?:\s+at\s+\[|Location:|$)/i);
  if (syntaxMatch) {
    let message = syntaxMatch[1].trim();
    const locationMatch = error.match(/at\s+\[(\d+:\d+)\]/);
    if (locationMatch) {
      message += ` (line ${locationMatch[1].replace(':', ', column ')})`;
    }
    return message;
  }

  // Truncate very long errors
  const displayError = error.length > 200 ? error.substring(0, 200) + '...' : error;
  return displayError;
};

export default formatSQLError;
