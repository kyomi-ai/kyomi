// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ChartML Schema Service
 *
 * Fetches the ChartML v2 JSON Schema from the backend API.
 * The backend is the SINGLE SOURCE OF TRUTH for the schema.
 */

import apiClient from '../api/apiClient';

let schemaCache = null;
let schemaPromise = null;

/**
 * Fetches the ChartML schema from the backend API.
 * Uses caching to avoid repeated fetches.
 *
 * @returns {Promise<Object>} The ChartML JSON Schema
 */
export async function getChartmlSchema() {
  // Return cached schema if available
  if (schemaCache) {
    return schemaCache;
  }

  // If fetch is already in progress, return the same promise
  if (schemaPromise) {
    return schemaPromise;
  }

  // Fetch schema from backend
  schemaPromise = apiClient.get('/api/v1/chartml/schema')
    .then(response => {
      schemaCache = response.data;
      schemaPromise = null;
      return schemaCache;
    })
    .catch(error => {
      schemaPromise = null;
      throw error;
    });

  return schemaPromise;
}

/**
 * Chart type requirements helper
 * Defines which fields are required for each chart type
 */
export const chartTypeRequirements = {
  bar: { requires: ["rows", "columns"] },
  line: { requires: ["rows", "columns"] },
  area: { requires: ["rows", "columns"] },
  scatter: { requires: ["rows", "columns"] },
  pie: { requires: ["rows", "columns"] },
  doughnut: { requires: ["rows", "columns"] },
  table: { requires: [] },
  metric: { requires: ["value"] }
};
