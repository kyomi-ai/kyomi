// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * KyomiChart - Thin wrapper around ChartML with Kyomi-specific UI features
 *
 * Adds:
 * - Edit button (pencil icon)
 * - Refresh button (for invalidating cache)
 * - Error boundaries
 * - Loading states
 *
 * Delegates all chart rendering to ChartML core.
 */

import React, { useMemo, useRef, useEffect, useState } from 'react';
import { toast } from 'sonner';
import { ChartML } from '@chartml/core';
import { createKyomiChartML } from '../lib/chartml/createKyomiChartML';
import { useCapabilities } from '../context/CapabilitiesContext';
import { usePalettePreference } from '../hooks/usePalettePreference';
import ChartHeaderBar from './ChartHeaderBar';
import {
  getChartErrorTitle,
  isDatasourceAccessError,
  isBigQueryPermissionError,
  ERROR_HELP_PATHS
} from '../utils/chartErrorHelpers';

/**
 * IMPORTANT: DO NOT import js-yaml or parse YAML in this component!
 *
 * ChartML core handles ALL YAML parsing. This wrapper should never need to parse specs.
 * If you find yourself wanting to parse YAML to extract values like title, STOP and ask:
 * "Should ChartML core be providing this via its API instead?"
 *
 * The wrapper's job is ONLY:
 * - Add container styling (padding, border, background)
 * - Add UI controls (edit/refresh buttons)
 * - Handle resize events
 *
 * Everything else is ChartML core's responsibility.
 */

/**
 * KyomiChart Component
 *
 * @param {Object} props
 * @param {string|Object} props.spec - ChartML specification (YAML string or object)
 * @param {ChartML} [props.chartmlInstance] - Optional pre-configured ChartML instance (for shared source registry)
 * @param {Array} [props.sourceComponents] - Optional array of source components to register
 * @param {Function} [props.onEdit] - Optional callback when edit button is clicked
 * @param {Function} [props.onRefresh] - Optional callback when refresh button is clicked
 * @param {string} [props.className] - Optional CSS class for container
 */
export function KyomiChart({ spec, chartmlInstance, sourceComponents = [], onEdit, onRefresh, className = '' }) {
  const { capabilities } = useCapabilities();
  const userPalette = usePalettePreference();  // Shared hook for palette preference
  const outerContainerRef = useRef(null);  // Watch this for resize
  const chartContainerRef = useRef(null);  // Render chart here
  const chartmlRef = useRef(null);
  const chartInstanceRef = useRef(null);  // Store Chart instance for refresh()
  const [lastUpdated, setLastUpdated] = useState(null);  // null until we have actual metadata
  const [, forceUpdate] = useState(0); // For triggering re-renders to update relative time
  const [expectedHeight, setExpectedHeight] = useState(null);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [renderError, setRenderError] = useState(null); // Store render errors to display to user

  // Use provided ChartML instance OR create Kyomi-configured instance (once)
  if (!chartmlRef.current) {
    if (chartmlInstance) {
      // Use the provided instance (e.g., from markdown-react plugin with registered sources)
      chartmlRef.current = chartmlInstance;
    } else {
      // Create standalone instance
      chartmlRef.current = createKyomiChartML({ capabilities });
    }
  }

  // Register source components if provided (for previewing charts with external data sources)
  useEffect(() => {
    if (chartmlRef.current && sourceComponents && sourceComponents.length > 0) {
      for (const source of sourceComponents) {
        try {
          chartmlRef.current.registerComponent(source);
        } catch (error) {
        }
      }
    }
  }, [sourceComponents]);

  // Calculate expected height BEFORE rendering to prevent layout shift
  // Uses ChartML core API which handles: explicit height → plugin defaults → 400px fallback
  useEffect(() => {
    if (spec && chartmlRef.current) {
      try {
        const { height } = chartmlRef.current.getExpectedDimensions(spec);
        setExpectedHeight(height);
      } catch (error) {
        setExpectedHeight(400);
      }
    }
  }, [spec]);

  // Update relative time display every minute
  useEffect(() => {
    if (!lastUpdated) return;

    const interval = setInterval(() => {
      forceUpdate(n => n + 1); // Trigger re-render to update relative time
    }, 60000); // Every 60 seconds

    return () => clearInterval(interval);
  }, [lastUpdated]);

  // Handle refresh button click - use Chart instance API
  const handleRefresh = async () => {
    if (!chartInstanceRef.current || isRefreshing) return;

    try {

      // Use Chart instance API to refresh
      // The Chart instance will coordinate with registry and call setRefreshStateCallback
      // which handles setting isRefreshing state and updating timestamp
      await chartInstanceRef.current.refresh();

      // Call parent's onRefresh callback if provided
      if (onRefresh) {
        onRefresh();
      }
    } catch (error) {
      toast.error(error.message || 'Failed to refresh chart');
    }
  };

  // Listen for dashboard-level refresh all event
  useEffect(() => {
    const handleDashboardRefreshAll = () => {
      handleRefresh();
    };

    window.addEventListener('dashboard-refresh-all', handleDashboardRefreshAll);
    return () => {
      window.removeEventListener('dashboard-refresh-all', handleDashboardRefreshAll);
    };
  }, [isRefreshing]);

  // Update ChartML instance when palette changes
  useEffect(() => {
    if (chartmlRef.current?.setDefaultPalette) {
      chartmlRef.current.setDefaultPalette(userPalette);
    }
  }, [userPalette]);

  // Render chart using ChartML core directly
  useEffect(() => {

    let isInitialRender = true;
    let ignoreResizeUntil = 0;  // Timestamp to ignore resizes until
    let currentChartId = 0;  // Track which chart render is current

    const renderChart = async () => {
      if (chartContainerRef.current && spec) {
        try {
          // Clear error at START of render to prevent stale errors from old instances
          setRenderError(null);

          // Increment chart ID to track this render
          const thisChartId = ++currentChartId;

          // ChartML core handles YAML parsing, loading indicator, and creates responsive SVG
          // render() returns Chart instance with refresh() and getMetadata()
          // Pass onError callback to catch async data fetching errors
          const chartInstance = await chartmlRef.current.render(spec, chartContainerRef.current, {
            onError: (error) => {
              // Only set error if this is still the current chart render
              if (thisChartId === currentChartId) {
                setRenderError(error);
              } else {
              }
            }
          });

          // Store Chart instance for refresh functionality
          chartInstanceRef.current = chartInstance;

          // Set up refresh state callback for coordinated refreshes
          // This allows registry to update UI when OTHER charts refresh shared sources
          chartInstance.setRefreshStateCallback((refreshing) => {
            setIsRefreshing(refreshing);

            // When refresh completes, sync timestamp from metadata
            if (!refreshing) {
              // Use the ref to get the current chart instance, not the closure
              const metadata = chartInstanceRef.current?.getMetadata();
              if (metadata?.last_updated) {
                setLastUpdated(metadata.last_updated);
              }
            }
          });

          // Update last_updated timestamp from Chart instance metadata
          const metadata = chartInstance.getMetadata();
          if (metadata?.last_updated) {
            setLastUpdated(metadata.last_updated);
          }

          // Error already cleared at start of render

          // Note: expectedHeight is set BEFORE rendering via getExpectedDimensions() API

          // Mark initial render complete after chart finishes rendering
          if (isInitialRender) {
            isInitialRender = false;
            // Ignore resize events for 1 second after initial render to allow D3 animations to complete
            ignoreResizeUntil = Date.now() + 1000;
          } else {
          }
        } catch (error) {
          // Store error to display to user
          setRenderError(error);
        }
      }
    };

    // Initial render
    renderChart();

    // Watch outer container for size changes and re-render chart
    let resizeTimeout;
    const resizeObserver = new ResizeObserver((entries) => {
      const now = Date.now();

      // Skip resize events until initial render completes
      if (isInitialRender) {
        return;
      }

      // Skip resize events for 1 second after initial render to allow D3 animations to complete
      if (now < ignoreResizeUntil) {
        return;
      }

      // Debounce re-renders to avoid excessive calls
      clearTimeout(resizeTimeout);
      resizeTimeout = setTimeout(() => {
        renderChart();
      }, 250);
    });

    if (outerContainerRef.current) {
      resizeObserver.observe(outerContainerRef.current);
    }

    // Cleanup on unmount
    return () => {
      resizeObserver.disconnect();
      clearTimeout(resizeTimeout);
      if (chartContainerRef.current) {
        chartContainerRef.current.innerHTML = '';
      }
    };
  }, [spec, userPalette]);  // Re-render when spec or palette changes

  return (
    <div ref={outerContainerRef} className={className}>
      {/* Shared header bar component */}
      <ChartHeaderBar
        lastUpdated={lastUpdated}
        isRefreshing={isRefreshing}
        onRefresh={handleRefresh}
        onEdit={onEdit}
      />

      {/* Chart card - no top border, connects to header */}
      <div className="p-4 border border-t-0 border-border rounded-b-lg bg-card shadow-sm">
        {/* Show error if chart failed to render */}
        {renderError && (
          <div className="p-6 bg-error/10 border border-error rounded-lg mb-4">
            <div className="flex items-start gap-3">
              <svg className="w-5 h-5 text-error-foreground flex-shrink-0 mt-0.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
              </svg>
              <div className="flex-1">
                <h3 className="text-sm font-semibold text-error-foreground mb-1">
                  {getChartErrorTitle(renderError.message)}
                </h3>
                <p className="text-sm text-error-foreground/90">
                  {renderError.message || 'Failed to render chart'}
                </p>
                {/* Contextual help for datasource accessibility errors */}
                {isDatasourceAccessError(renderError.message) && (
                  <div className="mt-3 p-3 bg-background/50 rounded border border-error/20">
                    <p className="text-xs text-error-foreground/80">
                      <strong>How to fix:</strong> Go to <strong>{ERROR_HELP_PATHS.DATASOURCES}</strong> to configure or enable this datasource.
                    </p>
                  </div>
                )}
                {/* Contextual help for BigQuery permission errors */}
                {isBigQueryPermissionError(renderError.message) && (
                  <div className="mt-3 p-3 bg-background/50 rounded border border-error/20">
                    <p className="text-xs text-error-foreground/80">
                      <strong>How to fix:</strong> Go to <strong>{ERROR_HELP_PATHS.PROFILE}</strong> to configure your BigQuery projects with the required permissions.
                    </p>
                  </div>
                )}
              </div>
            </div>
          </div>
        )}

        {/* Chart container wrapper - prevents layout shift */}
        {/* ALWAYS rendered so chartContainerRef is always valid, even during errors */}
        <div
          className="w-full relative"
          style={{ minHeight: expectedHeight ? `${expectedHeight}px` : undefined }}
        >
          {/* Chart container - ChartML renders here (with built-in loading indicator) */}
          {/* Must have min-height to match parent so loading indicator centers properly */}
          <div
            ref={chartContainerRef}
            className="w-full relative"
            style={{ minHeight: expectedHeight ? `${expectedHeight}px` : undefined }}
          />
        </div>
      </div>
    </div>
  );
}

export default KyomiChart;
