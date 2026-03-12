// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useMemo, useRef, useState, useEffect, useLayoutEffect } from 'react';
import { Node, mergeAttributes } from '@tiptap/core';
import { NodeViewWrapper, ReactNodeViewRenderer } from '@tiptap/react';
import { ChartMLChart } from '@chartml/markdown-react';
import { createKyomiChartML } from '../../lib/chartml/createKyomiChartML';
import { useCapabilities } from '../../context/CapabilitiesContext';
import { usePalettePreference } from '../../hooks/usePalettePreference';
import ChartHeaderBar from '../ChartHeaderBar';
import * as yaml from 'js-yaml';
import { convertVisualizeForTypeChange } from '../../utils/chartParser';


/**
 * SingleChart - Renders one chart with header bar
 */
function SingleChart({ spec, chartmlInstance, onEdit, onTypeChange, onOrientationChange, onModeChange, chartIndex, arrayIndex, colSpan, onWidthChange, onHeightChange, onResize }) {
  const chartInstanceRef = useRef(null);
  const wrapperRef = useRef(null);
  const containerRef = useRef(null);
  const [lastUpdated, setLastUpdated] = useState(null);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [renderError, setRenderError] = useState(null);
  const [, forceUpdate] = useState(0);
  const [resizing, setResizing] = useState(null); // { startX, startY, startHeight, startWidth, gridWidth }

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

  // Width snap points (colSpan values): 1/4, 1/3, 1/2, 2/3, 3/4, full
  const WIDTH_SNAPS = [3, 4, 6, 8, 9, 12];

  // Get colSpan from pixel width relative to grid
  const getColSpanFromWidth = (width, gridWidth) => {
    const ratio = width / gridWidth;
    // Find closest snap point
    let closest = 12;
    let closestDiff = 1;
    for (const snap of WIDTH_SNAPS) {
      const snapRatio = snap / 12;
      const diff = Math.abs(ratio - snapRatio);
      if (diff < closestDiff) {
        closestDiff = diff;
        closest = snap;
      }
    }
    return closest;
  };

  // Resize handlers (height + width)
  useEffect(() => {
    if (!resizing) return;

    const handleMouseMove = (e) => {
      // Height: snap to 10px grid
      const deltaY = e.clientY - resizing.startY;
      const rawHeight = Math.max(100, resizing.startHeight + deltaY);
      const snappedHeight = Math.round(rawHeight / 10) * 10;

      // Width: snap to colSpan breakpoints
      const deltaX = e.clientX - resizing.startX;
      const rawWidth = Math.max(resizing.gridWidth / 4, resizing.startWidth + deltaX);
      const newColSpan = getColSpanFromWidth(rawWidth, resizing.gridWidth);

      if (containerRef.current) {
        containerRef.current.style.height = `${snappedHeight}px`;
      }

      // Update grid-column on .react-renderer for width preview
      const reactRenderer = wrapperRef.current?.closest('.react-renderer');
      if (reactRenderer) {
        reactRenderer.style.gridColumn = `span ${newColSpan}`;
      }
    };

    const handleMouseUp = (e) => {
      // Height: add delta directly to the chart height (not container height).
      const deltaY = e.clientY - resizing.startY;
      const chartHeight = Math.max(100, Math.round((resizing.startChartHeight + deltaY) / 10) * 10);

      // Width
      const deltaX = e.clientX - resizing.startX;
      const rawWidth = Math.max(resizing.gridWidth / 4, resizing.startWidth + deltaX);
      const newColSpan = getColSpanFromWidth(rawWidth, resizing.gridWidth);

      // DON'T clear inline height here — it causes a visual flash because
      // the container snaps to the OLD chart height before the YAML update
      // re-renders the chart at the new height. useLayoutEffect clears it
      // after React has committed the new DOM.
      setResizing(null);

      const widthChanged = newColSpan !== colSpan;
      if (onResize) {
        onResize(widthChanged ? newColSpan : null, chartHeight);
      } else {
        onHeightChange?.(chartHeight);
        if (widthChanged) {
          onWidthChange?.(newColSpan);
        }
      }
    };

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);
    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
    };
  }, [resizing, onHeightChange, onWidthChange, colSpan, spec?.title]);

  // Clear inline container height after the spec's height updates.
  // The YAML update (from onResize) triggers a Tiptap re-render in a separate
  // cycle from setResizing(null). By keying on the spec height, we only clear
  // the inline style AFTER the chart has re-rendered at the new height — so
  // removing it causes no visible flash.
  useLayoutEffect(() => {
    if (!resizing && containerRef.current) {
      containerRef.current.style.height = '';
    }
  }, [spec?.visualize?.style?.height]);

  const handleResizeStart = (e) => {
    e.preventDefault();
    const gridContainer = wrapperRef.current?.closest('.tiptap-content');
    const gridWidth = gridContainer?.offsetWidth || 800;
    const containerEl = containerRef.current;

    // Capture the current chart height from spec (authoritative).
    // If no explicit height, derive from container by subtracting measured padding.
    let startChartHeight = spec?.visualize?.style?.height;
    if (startChartHeight == null && containerEl) {
      const cs = getComputedStyle(containerEl);
      const pad = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom);
      const border = parseFloat(cs.borderTopWidth) + parseFloat(cs.borderBottomWidth);
      startChartHeight = containerEl.offsetHeight - pad - border;
    }
    startChartHeight = startChartHeight || 300;

    setResizing({
      startX: e.clientX,
      startY: e.clientY,
      startHeight: containerEl?.offsetHeight || 300,
      startChartHeight,
      startWidth: containerEl?.offsetWidth || gridWidth * (colSpan / 12),
      gridWidth
    });
  };

  return (
    <div ref={wrapperRef} className="my-2 relative">
      <ChartHeaderBar
        lastUpdated={lastUpdated}
        isRefreshing={isRefreshing}
        onRefresh={handleRefresh}
        onEdit={onEdit ? () => onEdit(spec, chartIndex, arrayIndex) : null}
        chartType={spec?.visualize?.type}
        chartOrientation={spec?.visualize?.orientation}
        chartMode={spec?.visualize?.mode}
        onTypeChange={onTypeChange}
        onOrientationChange={onOrientationChange}
        onModeChange={onModeChange}
        draggable
      />

      <div ref={containerRef} className="p-4 border border-t-0 border-border rounded-b-lg bg-card shadow-sm overflow-hidden">
        {renderError ? (
          <div className="p-4 bg-error/10 border border-error rounded-lg">
            <p className="text-sm text-error-foreground">
              {renderError.message || 'Failed to render chart'}
            </p>
          </div>
        ) : (
          <ChartMLChart
            key={spec?.visualize?.type || 'chart'}
            spec={spec}
            chartmlInstance={chartmlInstance}
            onChartRender={handleChartRender}
            onError={(error) => setRenderError(error)}
          />
        )}
      </div>

      {/* Corner resize handle for height + width */}
      {(onHeightChange || onWidthChange) && (
        <div
          onMouseDown={handleResizeStart}
          className={`absolute bottom-1 right-1 w-4 h-4 cursor-nwse-resize transition-colors ${
            resizing ? 'text-primary' : 'text-border hover:text-muted-foreground'
          }`}
          title="Drag to resize"
        >
          <svg viewBox="0 0 16 16" fill="currentColor" className="w-full h-full">
            <path d="M14 14H12V12H14V14ZM14 10H12V8H14V10ZM10 14H8V12H10V14ZM14 6H12V4H14V6ZM10 10H8V8H10V10ZM6 14H4V12H6V14Z" />
          </svg>
        </div>
      )}
    </div>
  );
}

/**
 * ChartMLNodeView - React component that renders inside the Tiptap editor
 */
function ChartMLNodeView({ node, selected, editor, updateAttributes }) {
  const { capabilities } = useCapabilities();
  const userPalette = usePalettePreference();
  const yamlContent = node.attrs.content || '';
  const wrapperRef = useRef(null);

  // Create ChartML instance for rendering
  const chartmlInstance = useMemo(() => {
    const instance = createKyomiChartML({ capabilities });
    instance.setDefaultPalette(userPalette);
    return instance;
  }, [capabilities, userPalette]);

  // Parse YAML content
  const parsed = useMemo(() => {
    try {
      if (!yamlContent.trim()) return null;
      return yaml.load(yamlContent);
    } catch (err) {
      return null;
    }
  }, [yamlContent]);

  // Handle edit button click - dispatch custom event for parent to handle
  const handleEdit = (spec, chartIndex, arrayIndex) => {
    // Get the node position
    const pos = editor.view.posAtDOM(editor.view.dom.querySelector(`[data-chartml-id="${node.attrs.id}"]`), 0);

    // Dispatch custom event with chart info
    window.dispatchEvent(new CustomEvent('tiptap-edit-chart', {
      detail: {
        spec,
        chartIndex,
        arrayIndex,
        nodeId: node.attrs.id,
        yamlContent,
        pos
      }
    }));
  };

  // Handle "Add to Dashboard" - dispatch event with chart markdown for copying to another dashboard
  const handleSaveToDashboard = () => {
    const chartMarkdown = '```chartml\n' + yamlContent + '\n```';
    window.dispatchEvent(new CustomEvent('tiptap-save-chart-to-dashboard', {
      detail: { chartMarkdown }
    }));
  };

  // Handle "Chart Info" - dispatch event with spec for showing chart details
  const handleShowChartInfo = () => {
    const spec = Array.isArray(parsed) ? parsed[0] : parsed;
    window.dispatchEvent(new CustomEvent('tiptap-show-chart-info', {
      detail: { spec }
    }));
  };

  // Handle "Ask about this chart" - dispatch event to navigate to chat with chart context
  const handleAskAbout = () => {
    const spec = Array.isArray(parsed) ? parsed[0] : parsed;
    const chartMarkdown = '```chartml\n' + yamlContent + '\n```';
    window.dispatchEvent(new CustomEvent('tiptap-ask-about-chart', {
      detail: { chartMarkdown, spec }
    }));
  };

  // Handle chart type change - update visualize.type in the YAML
  const handleTypeChange = ({ type }) => {
    try {
      const currentSpec = yaml.load(yamlContent);
      const specToUpdate = Array.isArray(currentSpec) ? currentSpec[0] : currentSpec;

      if (!specToUpdate.visualize) {
        specToUpdate.visualize = {};
      }
      const previousType = specToUpdate.visualize.type;
      specToUpdate.visualize.type = type;

      // Clean up incompatible properties when switching types
      if (type !== 'bar') {
        delete specToUpdate.visualize.orientation;
      }
      if (type !== 'bar' && type !== 'area') {
        delete specToUpdate.visualize.mode;
      }

      // Convert visualize structure when crossing type categories (chart/table/metric)
      convertVisualizeForTypeChange(specToUpdate.visualize, previousType, type);

      // Strip per-row mark overrides so they inherit the new visualize.type
      // Rows can be strings ("revenue") or objects ({ field: "revenue", mark: "line" })
      if (Array.isArray(specToUpdate.visualize.rows)) {
        for (const row of specToUpdate.visualize.rows) {
          if (typeof row === 'object' && row !== null) delete row.mark;
        }
      }

      const newYaml = yaml.dump(Array.isArray(currentSpec) ? currentSpec : specToUpdate, {
        indent: 2,
        lineWidth: -1,
        noRefs: true,
      });

      updateAttributes({ content: newYaml.trim() });
    } catch (err) {
    }
  };

  // Handle orientation chip toggle
  const handleOrientationChange = ({ orientation }) => {
    try {
      const currentSpec = yaml.load(yamlContent);
      const specToUpdate = Array.isArray(currentSpec) ? currentSpec[0] : currentSpec;

      if (!specToUpdate.visualize) {
        specToUpdate.visualize = {};
      }

      if (orientation) {
        specToUpdate.visualize.orientation = orientation;
      } else {
        delete specToUpdate.visualize.orientation;
      }

      const newYaml = yaml.dump(Array.isArray(currentSpec) ? currentSpec : specToUpdate, {
        indent: 2,
        lineWidth: -1,
        noRefs: true,
      });

      updateAttributes({ content: newYaml.trim() });
    } catch (err) {
    }
  };

  // Handle mode chip toggle
  const handleModeChange = ({ mode }) => {
    try {
      const currentSpec = yaml.load(yamlContent);
      const specToUpdate = Array.isArray(currentSpec) ? currentSpec[0] : currentSpec;

      if (!specToUpdate.visualize) {
        specToUpdate.visualize = {};
      }

      if (mode) {
        specToUpdate.visualize.mode = mode;
      } else {
        delete specToUpdate.visualize.mode;
      }

      const newYaml = yaml.dump(Array.isArray(currentSpec) ? currentSpec : specToUpdate, {
        indent: 2,
        lineWidth: -1,
        noRefs: true,
      });

      updateAttributes({ content: newYaml.trim() });
    } catch (err) {
    }
  };

  // Handle width change - update the layout.colSpan in the YAML
  const handleWidthChange = (newColSpan) => {
    try {
      const currentSpec = yaml.load(yamlContent);
      const specToUpdate = Array.isArray(currentSpec) ? currentSpec[0] : currentSpec;

      // Set or update layout.colSpan
      if (!specToUpdate.layout) {
        specToUpdate.layout = {};
      }
      specToUpdate.layout.colSpan = newColSpan;

      // Serialize back to YAML
      const newYaml = yaml.dump(Array.isArray(currentSpec) ? currentSpec : specToUpdate, {
        indent: 2,
        lineWidth: -1,
        noRefs: true,
      });

      updateAttributes({ content: newYaml.trim() });
    } catch (err) {
    }
  };

  // Handle height change - update visualize.style.height in the YAML
  const handleHeightChange = (newHeight) => {
    try {
      const currentSpec = yaml.load(yamlContent);
      const specToUpdate = Array.isArray(currentSpec) ? currentSpec[0] : currentSpec;

      // Set or update visualize.style.height (per ChartML spec)
      if (!specToUpdate.visualize) {
        specToUpdate.visualize = {};
      }
      if (!specToUpdate.visualize.style) {
        specToUpdate.visualize.style = {};
      }
      specToUpdate.visualize.style.height = newHeight;

      // Serialize back to YAML
      const newYaml = yaml.dump(Array.isArray(currentSpec) ? currentSpec : specToUpdate, {
        indent: 2,
        lineWidth: -1,
        noRefs: true,
      });

      updateAttributes({ content: newYaml.trim() });
    } catch (err) {
    }
  };

  // Handle both width and height change in one update (for corner resize)
  const handleResize = (newColSpan, newHeight) => {
    try {
      const currentSpec = yaml.load(yamlContent);
      const specToUpdate = Array.isArray(currentSpec) ? currentSpec[0] : currentSpec;

      // Update width (layout.colSpan)
      if (newColSpan !== null) {
        if (!specToUpdate.layout) {
          specToUpdate.layout = {};
        }
        specToUpdate.layout.colSpan = newColSpan;
      }

      // Update height (visualize.style.height)
      if (newHeight !== null) {
        if (!specToUpdate.visualize) {
          specToUpdate.visualize = {};
        }
        if (!specToUpdate.visualize.style) {
          specToUpdate.visualize.style = {};
        }
        specToUpdate.visualize.style.height = newHeight;
      }

      // Serialize back to YAML
      const newYaml = yaml.dump(Array.isArray(currentSpec) ? currentSpec : specToUpdate, {
        indent: 2,
        lineWidth: -1,
        noRefs: true,
      });

      updateAttributes({ content: newYaml.trim() });
    } catch (err) {
    }
  };

  if (!parsed) {
    return (
      <NodeViewWrapper className="chartml-node my-4" data-chartml-id={node.attrs.id}>
        <div className={`p-4 bg-error border rounded-lg text-error-foreground text-sm ${selected ? 'ring-2 ring-primary' : 'border-error-border'}`}>
          {yamlContent.trim() ? 'Invalid ChartML syntax' : 'Empty chart block'}
        </div>
      </NodeViewWrapper>
    );
  }

  // Each node now contains a single chart (arrays are split during parsing)
  // Handle both single object and array with one item (for backwards compat)
  const spec = Array.isArray(parsed) ? parsed[0] : parsed;

  // Skip non-chart items (like sources/params only blocks)
  if (spec?.type && spec.type !== 'chart') {
    return (
      <NodeViewWrapper className="chartml-node my-4" data-chartml-id={node.attrs.id}>
        <div className="p-4 bg-muted border border-border rounded-lg text-muted-foreground text-sm">
          No chart to render (might be sources/params only)
        </div>
      </NodeViewWrapper>
    );
  }

  // Get layout properties
  const colSpan = spec?.layout?.colSpan || 12;

  // Set grid-column on the parent .react-renderer wrapper so drag-drop works
  useEffect(() => {
    if (wrapperRef.current) {
      // NodeViewWrapper renders as a div, its parent is the .react-renderer wrapper
      const reactRenderer = wrapperRef.current.parentElement;
      if (reactRenderer && reactRenderer.classList.contains('react-renderer')) {
        reactRenderer.style.gridColumn = `span ${colSpan}`;
      }
    }
  }, [colSpan]);

  return (
    <>
      <NodeViewWrapper
        ref={wrapperRef}
        className={`chartml-node ${selected ? 'ring-2 ring-primary ring-offset-2 rounded-lg' : ''}`}
        data-chartml-id={node.attrs.id}
      >
        <SingleChart
          spec={spec}
          chartmlInstance={chartmlInstance}
          onEdit={handleEdit}
          onTypeChange={handleTypeChange}
          onOrientationChange={handleOrientationChange}
          onModeChange={handleModeChange}
          chartIndex={0}
          arrayIndex={0}
          colSpan={colSpan}
          onWidthChange={handleWidthChange}
          onHeightChange={handleHeightChange}
          onResize={handleResize}
        />
      </NodeViewWrapper>
    </>
  );
}

/**
 * ChartMLNode - Tiptap extension for rendering ChartML blocks
 */
export const ChartMLNode = Node.create({
  name: 'chartMLBlock',
  group: 'block',
  atom: true, // Cannot be split or have cursor inside
  draggable: true,
  selectable: true,

  addAttributes() {
    return {
      content: {
        default: '',
        parseHTML: element => element.getAttribute('data-content') || '',
        renderHTML: attributes => ({
          'data-content': attributes.content,
        }),
      },
      id: {
        default: () => `chart-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
      },
    };
  },

  parseHTML() {
    return [
      {
        tag: 'div[data-type="chartml-block"]',
      },
    ];
  },

  renderHTML({ HTMLAttributes }) {
    // No content hole (0) for atom nodes - they don't have editable content
    return ['div', mergeAttributes(HTMLAttributes, { 'data-type': 'chartml-block' })];
  },

  addNodeView() {
    return ReactNodeViewRenderer(ChartMLNodeView);
  },

  addCommands() {
    return {
      insertChartML: (content = '') => ({ commands }) => {
        return commands.insertContent({
          type: this.name,
          attrs: {
            content,
            id: `chart-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
          },
        });
      },
      updateChartML: (id, content) => ({ tr, state }) => {
        let found = false;
        state.doc.descendants((node, pos) => {
          if (node.type.name === this.name && node.attrs.id === id) {
            tr.setNodeMarkup(pos, undefined, {
              ...node.attrs,
              content,
            });
            found = true;
            return false;
          }
        });
        return found;
      },
    };
  },
});

export default ChartMLNode;
