// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useMemo, useRef, useState, useEffect } from 'react';
import { ChartMLChart } from '@chartml/markdown-react';
import { createKyomiChartML } from '../lib/chartml/createKyomiChartML';
import { useCapabilities } from '../context/CapabilitiesContext';
import { usePalettePreference } from '../hooks/usePalettePreference';
import ChartHeaderBar from './ChartHeaderBar';
import * as yaml from 'js-yaml';

// Map colSpan (1-12) to Tailwind grid classes
function getColSpanClass(colSpan) {
  const classMap = {
    1: 'col-span-12 md:col-span-6 xl:col-span-1',
    2: 'col-span-12 md:col-span-6 xl:col-span-2',
    3: 'col-span-12 md:col-span-6 xl:col-span-3',
    4: 'col-span-12 md:col-span-6 xl:col-span-4',
    5: 'col-span-12 md:col-span-6 xl:col-span-5',
    6: 'col-span-12 md:col-span-6 xl:col-span-6',
    7: 'col-span-12 xl:col-span-7',
    8: 'col-span-12 xl:col-span-8',
    9: 'col-span-12 xl:col-span-9',
    10: 'col-span-12 xl:col-span-10',
    11: 'col-span-12 xl:col-span-11',
    12: 'col-span-12',
  };
  return classMap[colSpan] || 'col-span-12';
}

/**
 * HybridMarkdownEditor - Shows markdown as editable text, but renders chartml blocks as charts
 *
 * This is for dashboard editing where users want to:
 * - Edit text directly (headings, paragraphs, lists as raw markdown)
 * - See charts rendered inline
 * - Click edit on charts to modify them
 */
export function HybridMarkdownEditor({
  content,
  onChange,
  onEditChart,
  messageId = 'hybrid-editor'
}) {
  const { capabilities } = useCapabilities();
  const userPalette = usePalettePreference();

  // Create ChartML instance for rendering charts
  const chartmlInstance = useMemo(() => {
    const instance = createKyomiChartML({ capabilities });
    instance.setDefaultPalette(userPalette);
    return instance;
  }, [messageId, capabilities, userPalette]);

  // Parse content into segments: text blocks and chartml blocks
  const segments = useMemo(() => {
    if (!content) return [];

    const result = [];
    const regex = /```chartml\s*\n([\s\S]*?)```/g;
    let lastIndex = 0;
    let chartIndex = 0;
    let match;

    while ((match = regex.exec(content)) !== null) {
      // Add text before this chartml block
      if (match.index > lastIndex) {
        const textContent = content.substring(lastIndex, match.index);
        if (textContent.trim()) {
          result.push({
            type: 'text',
            content: textContent,
            startOffset: lastIndex,
            endOffset: match.index
          });
        }
      }

      // Add the chartml block
      result.push({
        type: 'chartml',
        content: match[1],
        fullMatch: match[0],
        startOffset: match.index,
        endOffset: match.index + match[0].length,
        chartIndex: chartIndex++
      });

      lastIndex = match.index + match[0].length;
    }

    // Add remaining text after last chartml block
    if (lastIndex < content.length) {
      const textContent = content.substring(lastIndex);
      if (textContent.trim()) {
        result.push({
          type: 'text',
          content: textContent,
          startOffset: lastIndex,
          endOffset: content.length
        });
      }
    }

    return result;
  }, [content]);

  // Handle text segment change
  const handleTextChange = (segment, newText) => {
    const updatedContent =
      content.substring(0, segment.startOffset) +
      newText +
      content.substring(segment.endOffset);
    onChange(updatedContent);
  };

  return (
    <div className="hybrid-editor space-y-4">
      {segments.map((segment, index) => (
        segment.type === 'text' ? (
          <TextSegment
            key={`text-${index}`}
            segment={segment}
            onChange={(newText) => handleTextChange(segment, newText)}
          />
        ) : (
          <ChartSegment
            key={`chart-${segment.chartIndex}`}
            segment={segment}
            chartmlInstance={chartmlInstance}
            onEditChart={onEditChart}
            chartIndex={segment.chartIndex}
          />
        )
      ))}

      {segments.length === 0 && (
        <div className="text-muted-foreground text-sm p-4 text-center">
          Start typing to add content...
        </div>
      )}
    </div>
  );
}

/**
 * TextSegment - Editable text area for markdown content
 */
function TextSegment({ segment, onChange }) {
  const [isEditing, setIsEditing] = useState(false);
  const [editValue, setEditValue] = useState(segment.content);
  const textareaRef = useRef(null);

  // Sync with segment content changes
  useEffect(() => {
    setEditValue(segment.content);
  }, [segment.content]);

  // Auto-resize textarea
  useEffect(() => {
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
      textareaRef.current.style.height = textareaRef.current.scrollHeight + 'px';
    }
  }, [editValue, isEditing]);

  const handleBlur = () => {
    if (editValue !== segment.content) {
      onChange(editValue);
    }
    setIsEditing(false);
  };

  const handleKeyDown = (e) => {
    // Cmd/Ctrl+Enter to save and blur
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      textareaRef.current?.blur();
    }
  };

  return (
    <div
      className="text-segment group relative"
      onClick={() => !isEditing && setIsEditing(true)}
    >
      {isEditing ? (
        <textarea
          ref={textareaRef}
          value={editValue}
          onChange={(e) => setEditValue(e.target.value)}
          onBlur={handleBlur}
          onKeyDown={handleKeyDown}
          autoFocus
          className="w-full p-3 font-mono text-sm bg-card border-2 border-primary rounded-lg focus:outline-none focus:ring-2 focus:ring-primary/30 resize-none min-h-[60px]"
          placeholder="Enter markdown text..."
        />
      ) : (
        <pre className="whitespace-pre-wrap font-mono text-sm p-3 bg-muted rounded-lg border border-transparent hover:border-border hover:bg-card cursor-text transition-colors">
          {segment.content || <span className="text-muted-foreground">Click to edit...</span>}
        </pre>
      )}
    </div>
  );
}

/**
 * ChartSegment - Renders a chartml block as interactive chart(s)
 * Supports both single charts and arrays of charts with layout.colSpan
 */
function ChartSegment({ segment, chartmlInstance, onEditChart, chartIndex }) {
  // Parse the YAML content - may be single chart or array
  const parsed = useMemo(() => {
    try {
      return yaml.load(segment.content);
    } catch (err) {
      return null;
    }
  }, [segment.content]);

  if (!parsed) {
    return (
      <div className="p-4 bg-error border border-error-border rounded-lg text-error-foreground text-sm">
        Invalid ChartML syntax
      </div>
    );
  }

  // Normalize to array for consistent handling
  const charts = Array.isArray(parsed) ? parsed : [parsed];

  // Filter to only chart components (exclude source, params, config types)
  const chartSpecs = charts.filter(c => !c.type || c.type === 'chart');

  if (chartSpecs.length === 0) {
    return null; // No charts to render (might be just sources/params)
  }

  // Render charts in a 12-column grid (same as ChartML markdown-react)
  return (
    <div className="chart-segment my-4">
      <div className="grid grid-cols-12 gap-2">
        {chartSpecs.map((spec, arrayIndex) => {
          // Get colSpan from chart layout (defaults to 12 for full width)
          const colSpan = spec?.layout?.colSpan || 12;
          const colSpanClass = getColSpanClass(colSpan);

          return (
            <div key={arrayIndex} className={colSpanClass}>
              <SingleChart
                spec={spec}
                chartmlInstance={chartmlInstance}
                onEditChart={onEditChart}
                chartIndex={chartIndex}
                arrayIndex={arrayIndex}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}

/**
 * SingleChart - Renders one chart with header bar
 */
function SingleChart({ spec, chartmlInstance, onEditChart, chartIndex, arrayIndex }) {
  const chartInstanceRef = useRef(null);
  const [lastUpdated, setLastUpdated] = useState(null);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [renderError, setRenderError] = useState(null);
  const [, forceUpdate] = useState(0);

  // Update relative time display every minute
  useEffect(() => {
    const interval = setInterval(() => forceUpdate(n => n + 1), 60000);
    return () => clearInterval(interval);
  }, []);

  const handleChartRender = (chartInstance) => {
    chartInstanceRef.current = chartInstance;

    chartInstance.setRefreshStateCallback?.((refreshing) => {
      setIsRefreshing(refreshing);
      if (!refreshing && chartInstanceRef.current) {
        const metadata = chartInstanceRef.current.getMetadata?.();
        if (metadata?.last_updated) {
          setLastUpdated(metadata.last_updated);
        }
      }
    });

    const metadata = chartInstance.getMetadata?.();
    if (metadata?.last_updated) {
      setLastUpdated(metadata.last_updated);
    }
  };

  const handleRefresh = async () => {
    if (chartInstanceRef.current && !isRefreshing) {
      await chartInstanceRef.current.refresh?.();
    }
  };

  const handleEdit = onEditChart ? () => {
    onEditChart(spec, chartIndex, arrayIndex);
  } : null;

  return (
    <div className="chart-segment my-4">
      <ChartHeaderBar
        lastUpdated={lastUpdated}
        isRefreshing={isRefreshing}
        onRefresh={handleRefresh}
        onEdit={handleEdit}
      />

      <div className="p-4 border border-t-0 border-border rounded-b-lg bg-card shadow-sm">
        {renderError ? (
          <div className="p-4 bg-error/10 border border-error rounded-lg">
            <p className="text-sm text-error-foreground">
              {renderError.message || 'Failed to render chart'}
            </p>
          </div>
        ) : (
          <ChartMLChart
            spec={spec}
            chartmlInstance={chartmlInstance}
            onChartRender={handleChartRender}
            onError={(error) => setRenderError(error)}
          />
        )}
      </div>
    </div>
  );
}

export default HybridMarkdownEditor;
