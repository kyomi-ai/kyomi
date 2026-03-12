// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Chart Error Helpers Utility
 *
 * Centralized error detection and classification for chart rendering errors.
 * Used by KyomiChart and MarkdownRenderer to provide consistent error UI.
 *
 * Error Classification:
 * 1. Datasource Access Errors - User needs to configure/enable datasource
 * 2. BigQuery Permission Errors - User needs to configure BigQuery project permissions
 * 3. General Chart Errors - Other rendering failures
 */

/**
 * Navigation paths for error help text.
 * Centralized to ensure consistency across error messages.
 */
export const ERROR_HELP_PATHS = {
  DATASOURCES: 'Settings \u2192 Datasources',
  PROFILE: 'Settings \u2192 Profile',
};

/**
 * Patterns that indicate datasource accessibility issues.
 * These errors require the user to configure or enable their datasource.
 *
 * Note: 'not found' is intentionally NOT included here because it's too broad.
 * "Table not found" and "Column not found" are SQL errors, not datasource issues.
 * The backend uses "Datasource.*not found" pattern for actual missing datasources.
 */
const DATASOURCE_ACCESS_PATTERNS = [
  'is disabled',
  'requires credentials',
  'have expired',
  'not accessible',
];

/**
 * Patterns that indicate BigQuery-specific permission issues.
 * These errors require the user to configure their BigQuery project permissions.
 */
const BIGQUERY_PERMISSION_PATTERNS = [
  'billing',
  'BigQuery',
  'permission',
  'Access Denied',
  'Google account',
];

/**
 * Check if an error message indicates a datasource accessibility issue.
 *
 * @param {string} message - Error message to check
 * @returns {boolean} True if the error is a datasource access issue
 *
 * @example
 * isDatasourceAccessError("Datasource 'my-db' is disabled. Go to Settings to enable it.") // true
 * isDatasourceAccessError("Table 'users' not found") // false
 * isDatasourceAccessError("Connection refused") // false
 */
export function isDatasourceAccessError(message) {
  if (!message) return false;
  return DATASOURCE_ACCESS_PATTERNS.some(pattern => message.includes(pattern));
}

/**
 * Check if an error message indicates a BigQuery permission issue.
 * Only returns true if:
 * 1. The message matches BigQuery permission patterns
 * 2. The error is NOT already a datasource access error (those take precedence)
 * 3. The message doesn't already contain the Settings path (already has guidance)
 *
 * @param {string} message - Error message to check
 * @returns {boolean} True if the error is a BigQuery permission issue
 *
 * @example
 * isBigQueryPermissionError("BigQuery Error: You don't have permission...") // true
 * isBigQueryPermissionError("Datasource 'bq' is disabled. Go to Settings...") // false (datasource access error)
 * isBigQueryPermissionError("Table 'users' not found") // false
 */
export function isBigQueryPermissionError(message) {
  if (!message) return false;

  // Don't show BigQuery help if already showing datasource help
  if (message.includes(ERROR_HELP_PATHS.DATASOURCES)) return false;

  return BIGQUERY_PERMISSION_PATTERNS.some(pattern => message.includes(pattern));
}

/**
 * Patterns that indicate a datasource-level availability issue (for error title).
 * Broader than access errors - includes 'not available' and 'not found' for general availability.
 *
 * Note: 'not found' here is acceptable because we're only using this for the ERROR TITLE,
 * not for determining which help text to show. A "Table not found" error getting the title
 * "Datasource Not Available" is slightly off but not harmful. The actual help text logic
 * (isDatasourceAccessError) is more precise.
 */
const DATASOURCE_UNAVAILABLE_PATTERNS = [
  'not available',
  'is disabled',
  'requires credentials',
  'have expired',
  'not accessible',
  'not found',
];

/**
 * Get the appropriate error title based on error type.
 *
 * @param {string} message - Error message to analyze
 * @returns {string} Either "Datasource Not Available" or "Chart Error"
 *
 * @example
 * getChartErrorTitle("Datasource 'my-db' is not accessible") // "Datasource Not Available"
 * getChartErrorTitle("Invalid SQL syntax") // "Chart Error"
 */
export function getChartErrorTitle(message) {
  if (!message) return 'Chart Error';

  const isDatasourceIssue = DATASOURCE_UNAVAILABLE_PATTERNS.some(
    pattern => message.includes(pattern)
  );

  return isDatasourceIssue ? 'Datasource Not Available' : 'Chart Error';
}
