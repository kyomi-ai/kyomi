// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * useSQLDryRun Hook - Debounced SQL dry run validation with ref support
 *
 * Automatically validates SQL queries after user stops typing (1 second debounce).
 * Returns validation state and results for display.
 *
 * CRITICAL: This hook works with an editorRef to avoid parent component re-renders
 * on every keystroke. It reads SQL directly from the ref.
 *
 * Features:
 * - Sets error markers in Monaco editor for visual feedback (using line/column from backend)
 * - Clears markers when query is valid
 * - Uses unified queryService for all datasource types
 * - BigQuery: Returns cost estimates with line/column on errors
 * - Other datasources: Uses EXPLAIN for syntax validation with line on errors
 *
 * Usage:
 *   const { dryRunning, dryRunResult, triggerDryRun } = useSQLDryRun(editorRef, enabled, datasource);
 *   // Call triggerDryRun() when editor content changes
 *
 * @param {React.RefObject} editorRef - Ref to editor with getValue() and setErrorMarkers() methods
 * @param {boolean} enabled - Whether dry run is enabled (default: true)
 * @param {Object} datasource - Datasource info with {slug, type} or just type string for backwards compat
 * @returns {Object} { dryRunning, dryRunResult, triggerDryRun }
 */

import { useState, useRef, useCallback } from 'react';
import { queryService } from '../services/queryService';

export function useSQLDryRun(editorRef, enabled = true, datasource = null) {
  const [dryRunning, setDryRunning] = useState(false);
  const [dryRunResult, setDryRunResult] = useState(null);
  const timeoutRef = useRef(null);

  // Keep datasource in a ref so triggerDryRun always has current value
  const datasourceRef = useRef(datasource);
  datasourceRef.current = datasource;

  const triggerDryRun = useCallback(() => {
    // Clear existing timeout
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current);
    }

    // Get selected SQL or full editor content if no selection
    const sql = editorRef?.current?.getSelectedOrFullText?.() || editorRef?.current?.getValue() || '';

    // Reset if disabled or no SQL
    if (!enabled || !sql || sql.trim().length === 0) {
      setDryRunResult(null);
      if (editorRef?.current?.setErrorMarkers) {
        editorRef.current.setErrorMarkers([]);
      }
      return;
    }

    // Get current datasource from ref (always fresh)
    const currentDatasource = datasourceRef.current;
    const slug = typeof currentDatasource === 'object' ? currentDatasource?.slug : null;
    const type = typeof currentDatasource === 'string' ? currentDatasource : currentDatasource?.type;

    // Need both datasource slug and type for dry run - just skip if not ready
    if (!slug || !type) {
      setDryRunResult(null);
      if (editorRef?.current?.setErrorMarkers) {
        editorRef.current.setErrorMarkers([]);
      }
      return;
    }

    // Debounce: run 1 second after typing stops (matches BigQuery console)
    timeoutRef.current = setTimeout(async () => {
      setDryRunning(true);
      try {
        const result = await queryService.dryRun(sql, { slug, type });

        // Store result for UI display
        setDryRunResult({
          valid: result.valid,
          message: result.message,
        });

        // Set error markers if backend provided line/column
        if (!result.valid && result.line && editorRef?.current?.setErrorMarkers) {
          editorRef.current.setErrorMarkers([{
            line: result.line,
            column: result.column || 1,  // Default to column 1 if not provided
            message: result.message,
          }]);
        } else {
          // Clear error markers on success or no location info
          if (editorRef?.current?.setErrorMarkers) {
            editorRef.current.setErrorMarkers([]);
          }
        }
      } catch (error) {
        // Check each source explicitly - no silent fallbacks
        const errorMessage =
          error.response?.data?.detail ||
          error.response?.data?.message ||
          error.response?.data?.error ||
          error.message ||
          `Unknown error: ${String(error)}`;

        setDryRunResult({
          valid: false,
          message: errorMessage,
        });

        // Clear markers for network/unexpected errors (no location info)
        if (editorRef?.current?.setErrorMarkers) {
          editorRef.current.setErrorMarkers([]);
        }
      } finally {
        setDryRunning(false);
      }
    }, 1000);
  }, [editorRef, enabled]); // datasource is read from ref, not closure

  return { dryRunning, dryRunResult, triggerDryRun };
}
