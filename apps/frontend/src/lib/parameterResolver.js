// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Parameter Variable Resolution
 *
 * Resolves $params.* references in ChartML specs with actual parameter values
 * Supports both global dashboard params and scoped chart-level params
 */

/**
 * Resolve parameter variable references in a ChartML specification
 *
 * @param {Object} chartSpec - ChartML specification (may contain $params.* references)
 * @param {Object} paramValues - Current parameter values { paramId: value, ... }
 * @param {string|number} [chartScope] - Optional chart scope (e.g., "chart_0") for chart-level params
 * @returns {Object} Resolved ChartML specification
 */
export function resolveParamReferences(chartSpec, paramValues = {}, chartScope = null) {
  if (!chartSpec) return chartSpec;

  // Convert to JSON string for easy replacement
  let specString = JSON.stringify(chartSpec);

  // Find all $params.* references
  const paramReferenceRegex = /"\$params\.([a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)*)"/g;

  // Replace each reference with its value
  specString = specString.replace(paramReferenceRegex, (match, path) => {
    let value;

    // Resolution order:
    // 1. Try scoped param as direct key (chart_0_0.selected_regions)
    // 2. Try scoped param as nested path (for date_range.start style params)
    // 3. Fall back to global param
    if (chartScope) {
      const scopedKey = `${chartScope}.${path}`;

      // First try direct key lookup (for scoped params like chart_0_0.selected_regions)
      value = paramValues[scopedKey];

      // If not found, try nested path lookup (for params like date_range.start)
      if (value === undefined) {
        value = getNestedValue(paramValues, scopedKey);
      }
    }

    if (value === undefined) {
      // Try direct key lookup first
      value = paramValues[path];

      // Then try nested path lookup
      if (value === undefined) {
        value = getNestedValue(paramValues, path);
      }
    }

    if (value === undefined) {
      return match; // Keep original reference if not found
    }

    // Return JSON-serialized value
    return JSON.stringify(value);
  });

  return JSON.parse(specString);
}

/**
 * Legacy function name for backward compatibility
 * @deprecated Use resolveParamReferences instead
 */
export function resolveFilterReferences(chartSpec, parameterValues = {}) {
  return resolveParamReferences(chartSpec, parameterValues);
}

/**
 * Get nested value from object using dot notation
 *
 * @param {Object} obj - Object to query
 * @param {string} path - Dot-separated path (e.g., "date_range.start")
 * @returns {*} Value at path or undefined
 */
function getNestedValue(obj, path) {
  return path.split('.').reduce((current, key) => {
    return current?.[key];
  }, obj);
}

/**
 * Initialize parameter values from URL query params or defaults
 *
 * Supports both new compressed format (?p=base64) and scoped params (chart_0.paramId)
 *
 * @param {Array} paramDefinitions - Parameter definitions from params component
 * @param {string} searchString - URL search string (e.g., "?p=eyJmb28iOiJiYXIifQ")
 * @param {string} [scope] - Optional scope prefix for chart-level params (e.g., "chart_0")
 * @returns {Object} Parameter values { paramId: value, ... } or scoped { "chart_0.paramId": value }
 */
export function initializeParamsFromURL(paramDefinitions, searchString = '', scope = null) {
  // Parse compressed params from URL
  const urlParamValues = parseParamsFromURL(searchString);
  const values = {};

  paramDefinitions.forEach(param => {
    const scopedKey = scope ? `${scope}.${param.id}` : param.id;

    // Check if value exists in URL (scoped or global)
    let value = urlParamValues[scopedKey];
    if (value === undefined && scope) {
      // Fall back to global param if scoped not found
      value = urlParamValues[param.id];
    }

    if (value !== undefined) {
      values[scopedKey] = value;
    } else {
      // Use default value
      values[scopedKey] = param.default;
    }
  });

  return values;
}

/**
 * Legacy function name for backward compatibility
 * @deprecated Use initializeParamsFromURL instead
 */
export function initializeFiltersFromURL(parameterDefinitions, searchString = '') {
  return initializeParamsFromURL(parameterDefinitions, searchString);
}

/**
 * Convert parameter values to compressed URL query string
 *
 * Uses base64url encoding to keep URLs short and shareable
 *
 * @param {Object} paramValues - Parameter values { paramId: value, ... } or scoped { "chart_0.paramId": value }
 * @returns {string} URL query string (without leading ?)
 */
export function paramsToQueryString(paramValues) {
  if (!paramValues || Object.keys(paramValues).length === 0) {
    return '';
  }

  try {
    // Compress the parameter state using base64url encoding
    const jsonString = JSON.stringify(paramValues);
    const base64 = btoa(jsonString);
    // Make it URL-safe by replacing +/= characters
    const base64url = base64
      .replace(/\+/g, '-')
      .replace(/\//g, '_')
      .replace(/=/g, '');

    return `p=${base64url}`;
  } catch (error) {
    // Fallback to empty string
    return '';
  }
}

/**
 * Legacy function name for backward compatibility
 * @deprecated Use paramsToQueryString instead
 */
export function parametersToQueryString(parameterValues) {
  return paramsToQueryString(parameterValues);
}

/**
 * Parse compressed parameter state from URL query string
 *
 * @param {string} searchString - URL search string (e.g., "?p=eyJmb28iOiJiYXIifQ")
 * @returns {Object} Parameter values { paramId: value, ... } or scoped { "chart_0.paramId": value }
 */
export function parseParamsFromURL(searchString) {
  if (!searchString) return {};

  try {
    const urlParams = new URLSearchParams(searchString);
    const encodedParams = urlParams.get('p');

    if (!encodedParams) return {};

    // Decode base64url
    let base64 = encodedParams
      .replace(/-/g, '+')
      .replace(/_/g, '/');

    // Add padding if needed
    while (base64.length % 4) {
      base64 += '=';
    }

    const jsonString = atob(base64);
    return JSON.parse(jsonString);
  } catch (error) {
    return {};
  }
}
