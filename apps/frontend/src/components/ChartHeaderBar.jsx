// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ChartHeaderBar - React wrapper around <chart-header-bar> web component
 *
 * Renders the shared web component and bridges React props to attributes/events.
 * Drag handle and width dropdown remain React-only (Tiptap / editor specific).
 *
 * Responsive behavior using CSS container queries (inside Shadow DOM):
 * - Wide containers (≥480px): Shows "Last refreshed X minutes ago"
 * - Narrow containers (<480px): Shows clock icon with compact time (e.g., "3m")
 */

import React, { useRef, useEffect, useState } from 'react';
import '@kyomi/chart-header';
import { Bars2Icon, ViewColumnsIcon } from '@heroicons/react/24/outline';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';

/**
 * @param {Object} props
 * @param {number|null} props.lastUpdated - Timestamp in milliseconds
 * @param {boolean} props.isRefreshing - Whether refresh is in progress
 * @param {Function} props.onRefresh - Refresh button click handler
 * @param {Function} [props.onEdit] - Edit button click handler
 * @param {Function} [props.onDelete] - Delete button click handler
 * @param {Function} [props.onSaveToDashboard] - Save to dashboard handler
 * @param {Function} [props.onInfo] - Info button click handler
 * @param {Function} [props.onAskAbout] - "Ask about this chart" handler
 * @param {string} [props.chartType] - Current chart type (bar, line, area, scatter, pie, doughnut, table, metric)
 * @param {string} [props.chartOrientation] - "horizontal" for horizontal bar, omit otherwise
 * @param {string} [props.chartMode] - "stacked" | "grouped" | "normalized", omit for default
 * @param {Function} [props.onTypeChange] - Chart type change callback (receives { type })
 * @param {Function} [props.onOrientationChange] - Orientation change callback (receives { orientation })
 * @param {Function} [props.onModeChange] - Mode change callback (receives { mode })
 * @param {boolean} [props.draggable] - Show drag handle (Tiptap editor)
 * @param {number} [props.colSpan] - Current column span
 * @param {Function} [props.onWidthChange] - Width change callback
 */
export function ChartHeaderBar({
  lastUpdated,
  isRefreshing,
  onRefresh,
  onEdit,
  onDelete,
  onSaveToDashboard,
  onInfo,
  onAskAbout,
  chartType,
  chartOrientation,
  chartMode,
  onTypeChange,
  onOrientationChange,
  onModeChange,
  draggable,
  colSpan,
  onWidthChange,
}) {
  const ref = useRef(null);
  const [widthMenuOpen, setWidthMenuOpen] = useState(false);
  const widthMenuRef = useRef(null);

  // Bridge custom events to React callbacks
  useEffect(() => {
    const el = ref.current;
    if (!el) return;

    const handlers = {
      'header-refresh': onRefresh,
      'header-edit': onEdit,
      'header-delete': onDelete,
      'header-save-to-dashboard': onSaveToDashboard,
      'header-info': onInfo,
      'header-ask-about': onAskAbout,
    };

    const listeners = [];
    for (const [event, handler] of Object.entries(handlers)) {
      if (handler) {
        const listener = () => handler();
        el.addEventListener(event, listener);
        listeners.push([event, listener]);
      }
    }

    // Type change event carries detail payload ({ type })
    if (onTypeChange) {
      const typeListener = (e) => onTypeChange(e.detail);
      el.addEventListener('header-type-change', typeListener);
      listeners.push(['header-type-change', typeListener]);
    }

    // Orientation change event ({ orientation })
    if (onOrientationChange) {
      const orientationListener = (e) => onOrientationChange(e.detail);
      el.addEventListener('header-orientation-change', orientationListener);
      listeners.push(['header-orientation-change', orientationListener]);
    }

    // Mode change event ({ mode })
    if (onModeChange) {
      const modeListener = (e) => onModeChange(e.detail);
      el.addEventListener('header-mode-change', modeListener);
      listeners.push(['header-mode-change', modeListener]);
    }

    return () => {
      for (const [event, listener] of listeners) {
        el.removeEventListener(event, listener);
      }
    };
  }, [onRefresh, onEdit, onDelete, onSaveToDashboard, onInfo, onAskAbout, onTypeChange, onOrientationChange, onModeChange]);

  // Close width menu when clicking outside
  useEffect(() => {
    if (!widthMenuOpen) return;
    const handleClickOutside = (e) => {
      if (widthMenuRef.current && !widthMenuRef.current.contains(e.target)) {
        setWidthMenuOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [widthMenuOpen]);

  // Build boolean attributes
  const boolAttrs = {};
  if (isRefreshing) boolAttrs.refreshing = '';
  if (onRefresh) boolAttrs['show-refresh'] = '';
  if (onEdit) boolAttrs['show-edit'] = '';
  if (onDelete) boolAttrs['show-delete'] = '';
  if (onSaveToDashboard) boolAttrs['show-save-to-dashboard'] = '';
  if (onInfo) boolAttrs['show-info'] = '';
  if (onAskAbout) boolAttrs['show-ask-about'] = '';
  if (onTypeChange) boolAttrs['show-type-selector'] = '';

  const widthOptions = [
    { span: 3, label: '¼ width' },
    { span: 4, label: '⅓ width' },
    { span: 6, label: '½ width' },
    { span: 8, label: '⅔ width' },
    { span: 9, label: '¾ width' },
    { span: 12, label: 'Full width' },
  ];

  return (
    <chart-header-bar
      ref={ref}
      last-updated={lastUpdated != null ? String(lastUpdated) : undefined}
      chart-type={chartType || undefined}
      chart-orientation={chartOrientation || undefined}
      chart-mode={chartMode || undefined}
      {...boolAttrs}
    >
      {/* Drag handle injected into the "before" slot (rendered inside the bar) */}
      {draggable && (
        <div slot="before">
          <Tooltip>
            <TooltipTrigger asChild>
              <div
                data-drag-handle
                draggable="true"
                contentEditable={false}
                className="p-1 text-muted-foreground hover:text-foreground cursor-grab active:cursor-grabbing rounded-md hover:bg-accent/50 transition-colors"
                aria-label="Drag to reorder"
              >
                <Bars2Icon className="h-4 w-4" />
              </div>
            </TooltipTrigger>
            <TooltipContent>Drag to reorder</TooltipContent>
          </Tooltip>
        </div>
      )}

      {/* Width dropdown injected into the "actions-before" slot */}
      {onWidthChange && (
        <div slot="actions-before" ref={widthMenuRef} className="relative">
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => setWidthMenuOpen(!widthMenuOpen)}
                className="p-1 text-muted-foreground hover:text-foreground hover:bg-accent/50 rounded-md transition-colors"
                aria-label="Change width"
              >
                <ViewColumnsIcon className="h-4 w-4" />
              </button>
            </TooltipTrigger>
            <TooltipContent>Change width</TooltipContent>
          </Tooltip>
          {widthMenuOpen && (
            <div className="absolute right-0 top-full mt-1 bg-popover border border-border rounded-md shadow-lg z-50 py-1 min-w-[100px]">
              {widthOptions.map(({ span, label }) => (
                <button
                  key={span}
                  onClick={() => {
                    onWidthChange(span);
                    setWidthMenuOpen(false);
                  }}
                  className={`w-full px-3 py-1.5 text-left text-sm transition-colors ${
                    colSpan === span
                      ? 'bg-accent text-accent-foreground'
                      : 'text-foreground hover:bg-accent/50'
                  }`}
                >
                  {label}
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </chart-header-bar>
  );
}

export default ChartHeaderBar;
