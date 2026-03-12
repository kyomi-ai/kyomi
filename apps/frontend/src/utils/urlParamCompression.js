// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * URL Parameter Compression Utilities
 *
 * Uses native browser Compression API (gzip) to compress dashboard parameters
 * into a single URL query parameter for shareable dashboard links.
 *
 * Zero bundle size - uses built-in browser APIs.
 * Supported: Chrome 80+, Firefox 113+, Safari 16.4+, Edge 80+
 */

/**
 * Compress parameters object to URL-safe base64 string
 *
 * @param {Object} params - Parameters object (e.g., { dashboard_filters: { region: [...] } })
 * @returns {Promise<string>} Compressed URL-safe base64 string
 *
 * @example
 * const params = { dashboard_filters: { region: ['US', 'EU'] } };
 * const compressed = await compressParamsToURL(params);
 * // Returns: "H4sIAAAAAAAAE6tWyk..."
 */
export async function compressParamsToURL(params) {
  try {
    const json = JSON.stringify(params);

    // Convert to blob and compress with gzip
    const blob = new Blob([json]);
    const stream = blob.stream().pipeThrough(
      new CompressionStream('gzip')
    );
    const compressed = await new Response(stream).arrayBuffer();

    // Convert to base64
    const base64 = btoa(String.fromCharCode(...new Uint8Array(compressed)));

    // Make URL-safe (replace +/= with -_~ for URL encoding)
    return base64
      .replace(/\+/g, '-')
      .replace(/\//g, '_')
      .replace(/=/g, '~');
  } catch (error) {
    throw new Error('Failed to compress parameters for URL');
  }
}

/**
 * Decompress URL-safe base64 string back to parameters object
 *
 * @param {string} compressed - Compressed URL-safe base64 string
 * @returns {Promise<Object>} Decompressed parameters object
 *
 * @example
 * const params = await decompressParamsFromURL("H4sIAAAAAAAAE6tWyk...");
 * // Returns: { dashboard_filters: { region: ['US', 'EU'] } }
 */
export async function decompressParamsFromURL(compressed) {
  try {
    // Convert from URL-safe base64
    const base64 = compressed
      .replace(/-/g, '+')
      .replace(/_/g, '/')
      .replace(/~/g, '=');

    // Decode base64 to binary
    const binary = atob(base64);
    const bytes = Uint8Array.from(binary, c => c.charCodeAt(0));

    // Decompress with gzip
    const stream = new Blob([bytes]).stream().pipeThrough(
      new DecompressionStream('gzip')
    );
    const decompressed = await new Response(stream).text();

    return JSON.parse(decompressed);
  } catch (error) {
    return {}; // Return empty object on error (invalid/corrupted URL)
  }
}

/**
 * Get all dashboard-level parameters from ChartML registry
 *
 * Only includes params blocks with names starting with "dashboard" or "dashboard_"
 * to exclude chart-level or other non-shareable params.
 *
 * @param {Object} chartmlInstance - ChartML instance
 * @returns {Object} All dashboard params organized by scope
 *
 * @example
 * const params = getAllDashboardParams(chartmlInstance);
 * // Returns: { dashboard_filters: { region: ['US'], status: 'active' } }
 */
export function getAllDashboardParams(chartmlInstance) {
  const allParams = {};

  if (!chartmlInstance.registry?.params) {
    return allParams;
  }

  // Iterate through registered params blocks
  // Only include dashboard-level params (scope starts with "dashboard")
  for (const [scopeName, paramsBlock] of chartmlInstance.registry.params) {
    // Filter: only include scopes starting with "dashboard"
    if (scopeName.toLowerCase().startsWith('dashboard')) {
      allParams[scopeName] = paramsBlock.values || {};
    }
  }

  return allParams;
}

/**
 * Set dashboard parameters in ChartML registry from params object
 *
 * @param {Object} chartmlInstance - ChartML instance
 * @param {Object} params - Parameters object organized by scope
 *
 * @example
 * setAllDashboardParams(chartmlInstance, {
 *   dashboard_filters: { region: ['US', 'EU'], status: 'active' }
 * });
 */
export function setAllDashboardParams(chartmlInstance, params) {
  if (!chartmlInstance.registry) {
    return;
  }

  for (const [scopeName, scopeParams] of Object.entries(params)) {
    // Only set if this params block is registered
    if (chartmlInstance.registry.params?.has(scopeName)) {
      for (const [paramId, value] of Object.entries(scopeParams)) {
        chartmlInstance.registry.setParamValue(scopeName, paramId, value);
      }
    }
  }
}

/**
 * Update URL with current dashboard parameters
 *
 * @param {Object} chartmlInstance - ChartML instance
 * @returns {Promise<void>}
 */
export async function updateURLWithParams(chartmlInstance) {
  const params = getAllDashboardParams(chartmlInstance);

  // Don't update URL if no params
  if (Object.keys(params).length === 0) {
    return;
  }

  try {
    const compressed = await compressParamsToURL(params);
    const url = new URL(window.location);

    // Compare with current URL to prevent unnecessary updates
    const currentCompressed = url.searchParams.get('p');
    if (currentCompressed === compressed) {
      return; // No change needed
    }

    url.searchParams.set('p', compressed);
    window.history.replaceState({}, '', url);
  } catch (error) {
  }
}

/**
 * Initialize dashboard parameters from URL on page load
 *
 * @param {Object} chartmlInstance - ChartML instance
 * @returns {Promise<boolean>} True if params were loaded from URL
 */
export async function initializeParamsFromURL(chartmlInstance) {
  const urlParams = new URLSearchParams(window.location.search);
  const compressed = urlParams.get('p');

  if (!compressed) {
    return false; // No params in URL
  }

  try {
    const params = await decompressParamsFromURL(compressed);
    setAllDashboardParams(chartmlInstance, params);
    return true;
  } catch (error) {
    return false;
  }
}
