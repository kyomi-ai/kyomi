// SPDX-License-Identifier: AGPL-3.0-or-later
import { useCallback } from 'react';
import { convertVisualizeForTypeChange } from '../utils/chartParser';
import { Input } from './ui/input';
import { Label } from './ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from './ui/select';

/**
 * Chart type definitions — same 8 types as <chart-header-bar> web component.
 * Each entry has a key, label, and a React SVG icon component.
 */
const CHART_TYPE_OPTIONS = [
  { key: 'bar', label: 'Bar', icon: BarChartIcon },
  { key: 'line', label: 'Line', icon: LineChartIcon },
  { key: 'area', label: 'Area', icon: AreaChartIcon },
  { key: 'scatter', label: 'Scatter', icon: ScatterChartIcon },
  { key: 'pie', label: 'Pie', icon: PieChartIcon },
  { key: 'doughnut', label: 'Doughnut', icon: DoughnutChartIcon },
  { key: 'table', label: 'Table', icon: TableIcon },
  { key: 'metric', label: 'Metric', icon: MetricIcon },
];

// ── Chart type icon components (React SVGs matching web component icons) ──

function BarChartIcon({ className }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="1.5" stroke="currentColor" className={className}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z" />
    </svg>
  );
}

function LineChartIcon({ className }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="1.5" stroke="currentColor" className={className}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M2.25 18 9 11.25l4.306 4.306a11.95 11.95 0 0 1 5.814-5.518l2.74-1.22m0 0-5.94-2.281m5.94 2.28-2.28 5.941" />
    </svg>
  );
}

function AreaChartIcon({ className }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="1.5" stroke="currentColor" className={className}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M3 20l4-8 4 4 4-10 4 6v8H3Z" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M3 20l4-8 4 4 4-10 4 6" />
    </svg>
  );
}

function ScatterChartIcon({ className }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="1.5" stroke="currentColor" className={className}>
      <circle cx="5" cy="17" r="1.5" />
      <circle cx="8" cy="10" r="1.5" />
      <circle cx="12" cy="14" r="1.5" />
      <circle cx="14" cy="7" r="1.5" />
      <circle cx="17" cy="12" r="1.5" />
      <circle cx="20" cy="5" r="1.5" />
    </svg>
  );
}

function PieChartIcon({ className }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="1.5" stroke="currentColor" className={className}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M12 3a9 9 0 1 0 9 9h-9V3Z" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M14 2.05A9 9 0 0 1 21.95 10H14V2.05Z" />
    </svg>
  );
}

function DoughnutChartIcon({ className }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="1.5" stroke="currentColor" className={className}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M12 3a9 9 0 1 0 9 9h-9V3Z" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M14 2.05A9 9 0 0 1 21.95 10H14V2.05Z" />
      <circle cx="12" cy="12" r="4" fill="currentColor" fillOpacity="0.1" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}

function TableIcon({ className }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="1.5" stroke="currentColor" className={className}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M3.375 19.5h17.25m-17.25 0a1.125 1.125 0 0 1-1.125-1.125M3.375 19.5h7.5c.621 0 1.125-.504 1.125-1.125m-9.75 0V5.625m0 12.75v-1.5c0-.621.504-1.125 1.125-1.125m18.375 2.625V5.625m0 12.75c0 .621-.504 1.125-1.125 1.125m1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125m0 3.75h-7.5A1.125 1.125 0 0 1 12 18.375m9.75-12.75c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125m19.5 0v1.5c0 .621-.504 1.125-1.125 1.125M2.25 5.625v1.5c0 .621.504 1.125 1.125 1.125m0 0h17.25m-17.25 0h7.5c.621 0 1.125.504 1.125 1.125M3.375 8.25c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125m17.25-3.75h-7.5c-.621 0-1.125.504-1.125 1.125m8.625-1.125c.621 0 1.125.504 1.125 1.125v1.5c0 .621-.504 1.125-1.125 1.125m-17.25 0h7.5m-7.5 0c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125M12 10.875v-1.5m0 1.5c0 .621-.504 1.125-1.125 1.125M12 10.875c0 .621.504 1.125 1.125 1.125m-2.25 0c.621 0 1.125.504 1.125 1.125M10.875 12h-7.5m8.625 0h7.5m-7.5 0c-.621 0-1.125.504-1.125 1.125M20.625 12c.621 0 1.125.504 1.125 1.125v1.5c0 .621-.504 1.125-1.125 1.125m-17.25 0h7.5m-7.5 0c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125M12 13.875v-1.5m0 1.5c0 .621-.504 1.125-1.125 1.125M12 13.875c0 .621.504 1.125 1.125 1.125m-2.25 0c.621 0 1.125.504 1.125 1.125M10.875 15h-7.5" />
    </svg>
  );
}

function MetricIcon({ className }) {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="1.5" stroke="currentColor" className={className}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M5.25 8.25h15m-16.5 7.5h15m-1.8-13.5-3.9 19.5m-2.1-19.5-3.9 19.5" />
    </svg>
  );
}

/**
 * ChartVisualEditor - Visual property editor for ChartML specs.
 *
 * Renders form controls that map to ChartML YAML properties.
 * Used as the "Visual" tab in ChartBuilderModal.
 *
 * @param {Object} props.chartML - Current ChartML spec object
 * @param {Function} props.onChange - Callback with updated spec object
 */
export default function ChartVisualEditor({ chartML, onChange }) {
  if (!chartML?.visualize) {
    return (
      <div className="p-4 text-sm text-muted-foreground">
        No chart configuration available. Configure a data source in the SQL tab first.
      </div>
    );
  }

  const vizType = chartML?.visualize?.type || 'bar';
  const orientation = chartML?.visualize?.orientation || null;
  const mode = chartML?.visualize?.mode || null;
  const title = chartML?.title || '';

  const handleTypeChange = useCallback((newType) => {
    const updated = structuredClone(chartML);
    if (!updated.visualize) updated.visualize = {};
    const previousType = updated.visualize.type;
    updated.visualize.type = newType;

    // Cleanup incompatible properties
    if (newType !== 'bar') delete updated.visualize.orientation;
    if (newType !== 'bar' && newType !== 'area') delete updated.visualize.mode;

    // Convert visualize structure when crossing type categories (chart/table/metric)
    convertVisualizeForTypeChange(updated.visualize, previousType, newType);

    // Strip per-row mark overrides so rows inherit new type
    // Rows can be strings ("revenue") or objects ({ field: "revenue", mark: "line" })
    if (Array.isArray(updated.visualize.rows)) {
      for (const row of updated.visualize.rows) {
        if (typeof row === 'object' && row !== null) delete row.mark;
      }
    }

    onChange(updated);
  }, [chartML, onChange]);

  const handleOrientationToggle = useCallback(() => {
    const updated = structuredClone(chartML);
    if (orientation === 'horizontal') {
      delete updated.visualize.orientation;
    } else {
      updated.visualize.orientation = 'horizontal';
    }
    onChange(updated);
  }, [chartML, orientation, onChange]);

  const handleModeToggle = useCallback(() => {
    const updated = structuredClone(chartML);
    const chartType = updated.visualize.type;

    if (chartType === 'bar') {
      // Toggle between stacked (default) and grouped
      if (mode === 'grouped') {
        delete updated.visualize.mode;
      } else {
        updated.visualize.mode = 'grouped';
      }
    } else if (chartType === 'area') {
      // Toggle between stacked (default) and normalized
      if (mode === 'normalized') {
        delete updated.visualize.mode;
      } else {
        updated.visualize.mode = 'normalized';
      }
    }
    onChange(updated);
  }, [chartML, mode, onChange]);

  const handleTitleChange = useCallback((newTitle) => {
    const updated = structuredClone(chartML);
    updated.title = newTitle;
    onChange(updated);
  }, [chartML, onChange]);

  return (
    <div className="p-4 space-y-6">
      {/* Chart Type */}
      <div className="space-y-2">
        <Label className="text-xs font-medium text-muted-foreground uppercase tracking-wide">
          Chart Type
        </Label>
        <Select value={vizType} onValueChange={handleTypeChange}>
          <SelectTrigger className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {CHART_TYPE_OPTIONS.map((option) => {
              const Icon = option.icon;
              return (
                <SelectItem key={option.key} value={option.key}>
                  <span className="flex items-center gap-2">
                    <Icon className="w-4 h-4" />
                    {option.label}
                  </span>
                </SelectItem>
              );
            })}
          </SelectContent>
        </Select>

        {/* Modifier chips — contextual based on chart type */}
        {(vizType === 'bar' || vizType === 'area') && (
          <div className="flex flex-wrap gap-2 mt-2">
            {vizType === 'bar' && (
              <>
                <button
                  type="button"
                  onClick={handleOrientationToggle}
                  className={`inline-flex items-center px-2.5 py-0.5 text-xs font-medium rounded-full border transition-colors ${
                    orientation === 'horizontal'
                      ? 'bg-primary/10 border-primary/50 text-primary'
                      : 'bg-transparent border-border text-muted-foreground hover:border-foreground hover:text-foreground'
                  }`}
                >
                  Horizontal
                </button>
                <button
                  type="button"
                  onClick={handleModeToggle}
                  className={`inline-flex items-center px-2.5 py-0.5 text-xs font-medium rounded-full border transition-colors ${
                    mode === 'grouped'
                      ? 'bg-primary/10 border-primary/50 text-primary'
                      : 'bg-transparent border-border text-muted-foreground hover:border-foreground hover:text-foreground'
                  }`}
                >
                  Grouped
                </button>
              </>
            )}
            {vizType === 'area' && (
              <button
                type="button"
                onClick={handleModeToggle}
                className={`inline-flex items-center px-2.5 py-0.5 text-xs font-medium rounded-full border transition-colors ${
                  mode === 'normalized'
                    ? 'bg-primary/10 border-primary/50 text-primary'
                    : 'bg-transparent border-border text-muted-foreground hover:border-foreground hover:text-foreground'
                }`}
              >
                Normalized
              </button>
            )}
          </div>
        )}
      </div>

      {/* Title */}
      <div className="space-y-2">
        <Label htmlFor="chart-title" className="text-xs font-medium text-muted-foreground uppercase tracking-wide">
          Title
        </Label>
        <Input
          id="chart-title"
          type="text"
          value={title}
          onChange={(e) => handleTitleChange(e.target.value)}
          placeholder="Chart title"
        />
      </div>
    </div>
  );
}
