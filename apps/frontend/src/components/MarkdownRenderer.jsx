// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useMemo, useRef, useState, useEffect, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { toast } from 'sonner';
import ms from 'ms';
import ReactMarkdown from 'react-markdown';
import apiClient from '../api/apiClient';
import remarkGfm from 'remark-gfm';
import remarkRemoveComments from 'remark-remove-comments';
import rehypeRaw from 'rehype-raw';
import { ArrowTopRightOnSquareIcon, ClipboardDocumentIcon, CheckIcon } from '@heroicons/react/24/outline';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneLight, oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { serializeChart, convertVisualizeForTypeChange } from '../utils/chartParser';
import { Alert, AlertTitle, AlertDescription } from '@/components/ui/alert';
import { ChartMLCodeBlock, ChartMLChart } from '@chartml/markdown-react';
import { createKyomiChartML } from '../lib/chartml/createKyomiChartML';
import { createTrialChartML } from '../lib/chartml/createTrialChartML';
import { useCapabilities } from '../context/CapabilitiesContext';
import { usePalettePreference } from '../hooks/usePalettePreference';
import { useTheme } from '../context/ThemeContext';
import ChartHeaderBar from './ChartHeaderBar';
import { initializeParamsFromURL, updateURLWithParams } from '../utils/urlParamCompression';
import {
  getChartErrorTitle,
  isDatasourceAccessError,
  isBigQueryPermissionError,
  ERROR_HELP_PATHS
} from '../utils/chartErrorHelpers';
import WatchPreviewCard from './watches/WatchPreviewCard';

// Code block with syntax highlighting, line numbers, and copy button
function CodeBlock({ language, children, compact = false }) {
  const [copied, setCopied] = useState(false);
  const { resolvedTheme } = useTheme();

  const handleCopy = async () => {
    await navigator.clipboard.writeText(String(children));
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="relative group">
      <button
        onClick={handleCopy}
        className="absolute top-2 right-2 p-2 rounded bg-accent hover:bg-accent/80 opacity-0 group-hover:opacity-100 transition-opacity z-10"
        title={copied ? 'Copied!' : 'Copy code'}
      >
        {copied ? (
          <CheckIcon className="h-4 w-4 text-success-foreground" />
        ) : (
          <ClipboardDocumentIcon className="h-4 w-4 text-muted-foreground" />
        )}
      </button>
      <SyntaxHighlighter
        language={language || 'text'}
        style={resolvedTheme === 'dark' ? oneDark : oneLight}
        showLineNumbers={false}
        customStyle={{
          margin: '16px 0',
          borderRadius: '6px',
          border: 'none',
          fontSize: compact ? '0.75rem' : '0.929rem', /* 12px compact / 13px regular */
          backgroundColor: 'var(--color-muted)',
          padding: '16px',
        }}
        codeTagProps={{
          style: {
            fontFamily: 'var(--font-mono)',
            backgroundColor: 'transparent',
          }
        }}
      >
        {String(children).replace(/\n$/, '')}
      </SyntaxHighlighter>
    </div>
  );
}

function MarkdownRendererComponent({
  children: markdown,
  className = '',
  messageId,
  sessionId,
  isStreaming = false,
  onMessageUpdate,
  onEditChart = null,        // Callback for chart edit (opens ChartBuilderModal)
  onDeleteChart = null,      // Callback for chart delete (chartBlockIndex, chartArrayIndex)
  onSaveChartToDashboard = null,  // Callback for saving individual chart to dashboard (chartYaml)
  onShowChartInfo = null,    // Callback for showing chart info modal (spec)
  onAskAboutChart = null,    // Callback for "ask about this chart" - opens chat with chart context
  onWatchApproved = null,    // Callback when watch preview is approved (watchData, cardId)
  acceptedCardIds = null,    // Set of card IDs that have been accepted
  compact = false,           // Compact mode for thinking tracker (smaller code blocks)
  isTrialMode = false        // Trial mode uses different ChartML with trial query endpoint
}) {
  const navigate = useNavigate();
  const { capabilities } = useCapabilities();
  const userPalette = usePalettePreference();  // Shared hook for palette preference

  // Create a shared ChartML instance per message for source registry
  // Use trial-specific ChartML in trial mode (uses /api/v1/trial/query endpoint)
  const chartmlInstance = useMemo(() => {
    const instance = isTrialMode
      ? createTrialChartML({ defaultPalette: userPalette })
      : createKyomiChartML({ capabilities });
    instance.setDefaultPalette(userPalette);
    return instance;
  }, [messageId, capabilities, userPalette, isTrialMode]);

  // Initialize dashboard parameters from URL on mount
  useEffect(() => {
    initializeParamsFromURL(chartmlInstance).catch(() => {
      // Silently handle initialization errors
    });
  }, [chartmlInstance]);

  // Sync URL when dashboard parameters change
  useEffect(() => {
    // Poll registry for changes and sync to URL
    const interval = setInterval(async () => {
      try {
        await updateURLWithParams(chartmlInstance);
      } catch (error) {
        // Silently handle URL update errors
      }
    }, 500); // Check every 500ms

    return () => clearInterval(interval);
  }, [chartmlInstance]);

  // ChartWithChrome - Wrapper component that adds refresh button and "last refreshed" text
  // This gets passed to ChartMLCodeBlock as the chartWrapper option
  // Receives chartBlockIndex and chartArrayIndex from ChartML markdown-react
  function ChartWithChrome({ spec: originalSpec, chartmlInstance, className, chartBlockIndex, chartArrayIndex }) {
    const chartInstanceRef = useRef(null);
    const [lastUpdated, setLastUpdated] = useState(null);  // null until we have actual metadata
    const [isRefreshing, setIsRefreshing] = useState(false);
    const [renderError, setRenderError] = useState(null);  // Store render errors to display to user
    const [, forceUpdate] = useState(0);
    const [typeOverride, setTypeOverride] = useState(null);
    const [orientationOverride, setOrientationOverride] = useState(undefined); // undefined = no override
    const [modeOverride, setModeOverride] = useState(undefined); // undefined = no override

    // Reset overrides when the underlying spec changes (e.g. new message)
    useEffect(() => {
      setTypeOverride(null);
      setOrientationOverride(undefined);
      setModeOverride(undefined);
    }, [originalSpec]);

    // Handle type change from header (orientation/mode overrides preserved;
    // the useMemo cleanup removes incompatible properties from the derived spec)
    const handleTypeOverride = ({ type }) => {
      setTypeOverride(type);
    };

    // Handle orientation chip toggle (viewer-only, non-persisting)
    const handleOrientationOverride = ({ orientation }) => {
      setOrientationOverride(orientation);
    };

    // Handle mode chip toggle (viewer-only, non-persisting)
    const handleModeOverride = ({ mode }) => {
      setModeOverride(mode);
    };

    // Derive effective spec with type/orientation/mode overrides applied
    const spec = useMemo(() => {
      if (!originalSpec?.visualize) return originalSpec;
      const hasTypeOverride = typeOverride !== null;
      const hasOrientationOverride = orientationOverride !== undefined;
      const hasModeOverride = modeOverride !== undefined;
      if (!hasTypeOverride && !hasOrientationOverride && !hasModeOverride) return originalSpec;

      const effectiveType = hasTypeOverride ? typeOverride : originalSpec.visualize.type;
      const viz = { ...originalSpec.visualize, type: effectiveType };

      // Apply orientation override (null means delete, undefined means no override)
      if (hasOrientationOverride) {
        if (orientationOverride) {
          viz.orientation = orientationOverride;
        } else {
          delete viz.orientation;
        }
      }

      // Apply mode override (null means delete, undefined means no override)
      if (hasModeOverride) {
        if (modeOverride) {
          viz.mode = modeOverride;
        } else {
          delete viz.mode;
        }
      }

      // Clean up incompatible properties based on effective type
      if (effectiveType !== 'bar') {
        delete viz.orientation;
      }
      if (effectiveType !== 'bar' && effectiveType !== 'area') {
        delete viz.mode;
      }

      // Convert visualize structure when crossing type categories (chart/table/metric)
      if (hasTypeOverride) {
        convertVisualizeForTypeChange(viz, originalSpec.visualize.type, effectiveType);
      }

      // Strip per-row mark overrides so they inherit the new visualize.type
      // Rows can be strings ("revenue") or objects ({ field: "revenue", mark: "line" })
      if (hasTypeOverride && Array.isArray(viz.rows)) {
        viz.rows = viz.rows.map((row) =>
          typeof row === 'object' && row !== null ? (({ mark, ...rest }) => rest)(row) : row
        );
      }
      return { ...originalSpec, visualize: viz };
    }, [originalSpec, typeOverride, orientationOverride, modeOverride]);

    // Generate a unique ID for this chart instance (stable across re-renders)
    const chartId = useRef(`chart-${messageId}-${chartBlockIndex}-${chartArrayIndex}`).current;

    // Update relative time display every minute
    useEffect(() => {
      const interval = setInterval(() => {
        forceUpdate(n => n + 1);
      }, 60000);
      return () => clearInterval(interval);
    }, []);

    // Stable callback to receive Chart instance from ChartMLChart
    const handleChartRender = useRef((chartInstance) => {
      chartInstanceRef.current = chartInstance;

      // Set up refresh state callback for animating refresh button and syncing timestamp
      chartInstance.setRefreshStateCallback((refreshing) => {
        setIsRefreshing(refreshing);

        // When refresh completes, sync timestamp from Chart metadata
        if (!refreshing && chartInstanceRef.current) {
          const metadata = chartInstanceRef.current.getMetadata();
          if (metadata?.last_updated) {
            setLastUpdated(metadata.last_updated);
          }
        }
      });

      // Get initial metadata
      const metadata = chartInstance.getMetadata();
      if (metadata?.last_updated) {
        setLastUpdated(metadata.last_updated);
      }
    }).current;

    // Handle refresh button click
    const handleRefresh = async () => {
      if (chartInstanceRef.current && !isRefreshing) {
        try {
          await chartInstanceRef.current.refresh();

          // Update timestamp from metadata
          const metadata = chartInstanceRef.current.getMetadata();
          if (metadata?.last_updated) {
            setLastUpdated(metadata.last_updated);
          }
        } catch (error) {
          toast.error(error.message || 'Failed to refresh chart');
        }
      }
    };

    // Handle edit button click
    const handleEdit = onEditChart ? () => {
      // Pass spec and chart indices to the callback
      onEditChart(spec, chartBlockIndex, chartArrayIndex);
    } : null;

    // Handle delete button click
    const handleDelete = onDeleteChart ? () => {
      onDeleteChart(chartBlockIndex, chartArrayIndex);
    } : null;

    // Handle save to dashboard button click
    const handleSaveToDashboard = onSaveChartToDashboard ? () => {
      // Serialize the spec back to YAML and wrap in code fence
      const chartYaml = serializeChart(spec);
      const chartMarkdown = '```chartml\n' + chartYaml + '\n```';
      onSaveChartToDashboard(chartMarkdown);
    } : null;

    // Handle info button click - calls parent callback with spec
    const handleInfo = onShowChartInfo ? () => {
      onShowChartInfo(spec);
    } : null;

    // Handle "ask about this chart" button click - navigates to chat with chart context
    const handleAskAbout = onAskAboutChart ? () => {
      // Serialize the spec back to YAML and wrap in code fence
      const chartYaml = serializeChart(spec);
      const chartMarkdown = '```chartml\n' + chartYaml + '\n```';
      onAskAboutChart(chartMarkdown, spec);
    } : null;

    // Listen for dashboard-level refresh all event
    useEffect(() => {
      const handleDashboardRefreshAll = () => {
        handleRefresh();
      };

      window.addEventListener('dashboard-refresh-all', handleDashboardRefreshAll);
      return () => {
        window.removeEventListener('dashboard-refresh-all', handleDashboardRefreshAll);
      };
    }, []);

    // Auto-refresh based on TTL from spec
    // Requires: 1) explicit opt-in via cache.autoRefresh: true
    //           2) datasource allows auto-refresh (admin setting)
    useEffect(() => {
      // Must explicitly opt-in to auto-refresh in spec
      if (spec?.data?.cache?.autoRefresh !== true) return;

      const ttlString = spec?.data?.cache?.ttl;
      if (!ttlString) return;

      const ttlMs = ms(ttlString);
      if (typeof ttlMs !== 'number' || isNaN(ttlMs)) return;

      // Check if datasource allows auto-refresh (admin can disable for pay-per-query sources)
      const datasourceSlug = spec?.data?.datasource;
      let intervalId = null;

      const setupAutoRefresh = async () => {
        // If there's a datasource, check if it allows auto-refresh
        if (datasourceSlug) {
          try {
            const response = await apiClient.get(`/api/v1/datasources/${datasourceSlug}`);
            if (response.data?.auto_refresh_allowed === false) {
              return;
            }
          } catch (error) {
            // If we can't fetch datasource info, allow auto-refresh (fail open for inline data)
          }
        }

        // Set up interval to refresh at the TTL rate
        const intervalMs = Math.max(ttlMs, 10000); // Minimum 10 seconds to avoid hammering

        intervalId = setInterval(() => {
          if (chartInstanceRef.current && !isRefreshing) {
            handleRefresh();
          }
        }, intervalMs);
      };

      setupAutoRefresh();

      return () => {
        if (intervalId) {
          clearInterval(intervalId);
        }
      };
    }, [spec?.data?.cache?.ttl, spec?.data?.cache?.autoRefresh, spec?.data?.datasource, chartId]);

    return (
      <div className="my-2">
        {/* Shared header bar component */}
        <ChartHeaderBar
          lastUpdated={lastUpdated}
          isRefreshing={isRefreshing}
          onRefresh={isTrialMode ? null : handleRefresh}
          onEdit={handleEdit}
          onDelete={handleDelete}
          onSaveToDashboard={handleSaveToDashboard}
          onInfo={handleInfo}
          onAskAbout={handleAskAbout}
          chartType={spec?.visualize?.type}
          chartOrientation={spec?.visualize?.orientation}
          chartMode={spec?.visualize?.mode}
          onTypeChange={handleTypeOverride}
          onOrientationChange={handleOrientationOverride}
          onModeChange={handleModeOverride}
        />

        {/* Chart card - no top border, connects to header */}
        <div className="p-4 border border-t-0 border-border rounded-b-lg bg-card shadow-sm">
          {/* Show error if chart failed to render */}
          {renderError ? (
            <div className="p-6 bg-error/10 border border-error rounded-lg">
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
          ) : (
            /* Chart render area - ChartMLChart does the actual rendering */
            <ChartMLChart
              key={spec?.visualize?.type || 'chart'}
              spec={spec}
              chartmlInstance={chartmlInstance}
              className={className}
              onChartRender={handleChartRender}
              onError={(error) => {
                // Handle errors from ChartML via proper callback interface
                setRenderError(error);
              }}
              context={{ chartId }}  // Pass chartId through context (available for future use)
            />
          )}
        </div>
      </div>
    );
  }

  // Custom code renderer for non-ChartML blocks
  const customCodeRenderer = React.useCallback((codeProps) => {
    const { lang, children, className } = codeProps;

    // Code blocks have className like 'language-sql', lang set, or contain newlines
    // Inline code has none of these
    const content = String(children);
    const isCodeBlock = className || lang || content.includes('\n');

    // For inline code (single backticks), use simple styling
    if (!isCodeBlock) {
      return <code className="inline-code">{children}</code>;
    }

    // Handle structured watch-response blocks - render message + optional preview card
    // Check both lang and className since markdown parsers handle language identifiers differently
    const isWatchResponse = lang === 'json:watch-response' ||
                            className?.includes('watch-response') ||
                            (lang === 'json' && content.includes('"watch"') && content.includes('"message"'));

    if (isWatchResponse && onWatchApproved) {
      try {
        const data = JSON.parse(content);
        // Verify it has the expected structure
        if ('message' in data && 'watch' in data) {
          const { message, watch } = data;
          // Generate stable card ID from messageId + watch content
          const cardId = `${messageId}-${watch?.name}-${watch?.schedule}`;
          const isAccepted = acceptedCardIds?.has(cardId);
          // Break out of pre/code styling with explicit overrides
          return (
            <div className="watch-response not-prose" style={{ fontFamily: 'var(--font-sans, ui-sans-serif, system-ui, sans-serif)', whiteSpace: 'normal' }}>
              {message && (
                <ReactMarkdown remarkPlugins={[remarkGfm, remarkRemoveComments]} rehypePlugins={[rehypeRaw]} components={components}>
                  {message}
                </ReactMarkdown>
              )}
              {watch && (
                <WatchPreviewCard
                  preview={watch}
                  onApprove={(watchData) => onWatchApproved(watchData, cardId)}
                  created={isAccepted}
                />
              )}
            </div>
          );
        }
      } catch (e) {
        // If JSON parsing fails, fall through to regular code block
      }
    }

    // For code blocks (triple backticks), use SyntaxHighlighter with line numbers
    return <CodeBlock language={lang || className?.replace('language-', '') || 'text'} compact={compact}>{children}</CodeBlock>;
  }, [onWatchApproved, acceptedCardIds, messageId, compact]);

  // Use ChartMLCodeBlock with our chrome wrapper and custom code renderer
  // ChartML's DefaultParamsRenderer handles all param rendering
  // URL sync happens via polling in useEffect above
  // IMPORTANT: Include markdown as dependency so chartBlockIndex counter resets when content changes
  const { code, pre } = React.useMemo(
    () => ChartMLCodeBlock({
      chartmlInstance,
      chartWrapper: ChartWithChrome,
      codeRenderer: customCodeRenderer
      // No paramsWrapper - use ChartML's default renderer
    }),
    [chartmlInstance, markdown, customCodeRenderer]
  );

  // Memoize components object to prevent recreation on every render
  const components = React.useMemo(() => ({
    code,
    pre,

    // Table overflow wrapper (keeps responsive scroll; inner styling via kyomi-markdown CSS)
    table({ children }) {
      return (
        <div className="overflow-x-auto my-4">
          <table>{children}</table>
        </div>
      );
    },

    // Enhanced images with better styling
    img({ src, alt, title }) {
      return (
        <div className="my-4">
          <img
            src={src}
            alt={alt}
            title={title}
            className="max-w-full h-auto rounded-lg shadow-sm border border-border hover:shadow-md transition-shadow cursor-pointer"
            loading="lazy"
          />
          {alt && (
            <p className="text-sm text-muted-foreground mt-2 text-center italic">
              {alt}
            </p>
          )}
        </div>
      );
    },

    // Enhanced links with external link indicators
    // Internal links use React Router navigation to avoid full page reloads
    a({ href, children }) {
      const isExternal = href && (href.startsWith('http') || href.startsWith('https'));
      const isInternal = href && href.startsWith('/');

      const handleClick = (e) => {
        if (isInternal) {
          e.preventDefault();
          navigate(href);
        }
      };

      return (
        <a
          href={href}
          onClick={handleClick}
          target={isExternal ? '_blank' : undefined}
          rel={isExternal ? 'noopener noreferrer' : undefined}
          className="text-primary hover:text-primary/80 underline decoration-primary/30 hover:decoration-primary/50 transition-colors inline-flex items-center gap-1"
        >
          {children}
          {isExternal && (
            <ArrowTopRightOnSquareIcon className="h-3 w-3 ml-1" />
          )}
        </a>
      );
    },

    // Headings, paragraphs, lists, blockquotes, and hr are styled via .kyomi-markdown CSS
  }), [code, pre, navigate]);

  // Don't strip comments manually - remarkRemoveComments plugin handles this
  let cleanMarkdown = markdown || '';

  // Filter out incomplete chartml code blocks to prevent YAML parsing errors during streaming
  // Simple text filtering: find the last ```chartml and check if it has a closing ```
  const lastChartmlStart = cleanMarkdown.lastIndexOf('```chartml');
  if (lastChartmlStart !== -1) {
    // Check if there's a closing ``` after the last ```chartml
    const afterChartml = cleanMarkdown.substring(lastChartmlStart + 10); // Skip past '```chartml'
    const closingFence = afterChartml.indexOf('```');

    if (closingFence === -1) {
      // No closing fence found - this is an incomplete block
      // Remove everything from the last ```chartml onwards
      cleanMarkdown = cleanMarkdown.substring(0, lastChartmlStart);
    }
  }

  return (
    <div className={`kyomi-markdown markdown-content ${className}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkRemoveComments]}
        rehypePlugins={[rehypeRaw]}
        components={components}
      >
        {cleanMarkdown}
      </ReactMarkdown>
    </div>
  );
}

// Memoize component to prevent re-renders when props haven't changed
export const MarkdownRenderer = React.memo(MarkdownRendererComponent);
