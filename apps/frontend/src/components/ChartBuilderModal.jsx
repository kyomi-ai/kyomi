// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useRef, useEffect, useCallback, useMemo } from 'react';
import { createPortal } from 'react-dom';
import Editor, { loader } from '@monaco-editor/react';
import * as monaco from 'monaco-editor';
import * as yaml from 'js-yaml';
import KyomiChart from './KyomiChart';
import ChartBuilderCopilotSidebar from './ChartBuilderCopilotSidebar';
import ChartVisualEditor from './ChartVisualEditor';
import SQLEditor from './SQLEditor';
import { queryService } from '../services/queryService';
import { useSQLDryRun } from '../hooks/useSQLDryRun';
import { getChartmlSchema } from '../schemas/schemaService';
import { toast } from '../lib/toast';
import {
  registerChartmlLanguage,
  registerChartmlCompletionProvider,
  validateChartmlDocument
} from '../lib/chartmlLanguage';
import { useCapabilities } from '../context/CapabilitiesContext';
import { useMonacoEditor } from '../hooks/useMonacoEditor';
import { useProductTour } from './ProductTour';
import DatasourceSelector from './DatasourceSelector';
import useDatasources from '../hooks/useDatasources';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';
import { useTheme } from '../context/ThemeContext';

// Configure loader to use npm package instead of CDN
loader.config({ monaco });

/**
 * ChartBuilderModal - Modal for creating/editing charts
 *
 * Two-screen flow:
 * 1. SQL Editor (write query)
 * 2. Chart Config (Visual/AI/YAML tabs + always-visible Preview)
 *
 * ChartML is the single source of truth - no internal conversion formats
 */
const ChartBuilderModal = ({
  chartML: chartMLProp,
  dashboardContent,
  onSave,
  onClose,
  datasourceSlug = null,
  datasourceType = null,
}) => {
  const { bigqueryMode } = useCapabilities();
  const { showTour } = useProductTour();
  const { resolvedTheme } = useTheme();
  const renderCount = useRef(0);
  renderCount.current++;

  if (renderCount.current % 10 === 0) {
  }

  const isNewChart = !chartMLProp || Object.keys(chartMLProp).length === 0;

  // Determine if this is a new chart FROM the SQL editor (has SQL but no config)
  const isNewFromSQLEditor = isNewChart && chartMLProp?.data?.query;

  // Top-level tab state - 'sql' or 'chart'
  // Open on Chart Config if: existing chart OR new chart from SQL editor
  // Open on SQL Editor if: truly new chart (no SQL yet)
  const [activeTab, setActiveTab] = useState(isNewChart && !isNewFromSQLEditor ? 'sql' : 'chart');

  // Chart config sub-tab state - 'visual' (default), 'ai', or 'yaml'
  const [configTab, setConfigTab] = useState('visual');

  // SQL Editor local state - separate from chartML, synced only on tab switch
  const [sqlEditorText, setSqlEditorText] = useState(() => {
    // Initialize with SQL from chartML
    return chartMLProp?.data?.query || '';
  });

  // UI state
  const [showCopyButton, setShowCopyButton] = useState(false);
  const [queryResults, setQueryResults] = useState(null);
  const [sqlSidebarOpen, setSqlSidebarOpen] = useState(true);

  // ChartML state - SINGLE SOURCE OF TRUTH
  const [chartML, setChartML] = useState(() => {
    if (chartMLProp && Object.keys(chartMLProp).length > 0) {
      return chartMLProp;
    }
    // Default ChartML for new charts - use datasource props if provided
    const dataSection = {
      query: '',
      cache: { ttl: '24h' }
    };
    // Use datasource slug if provided (preferred), otherwise provider type
    if (datasourceSlug) {
      dataSection.datasource = datasourceSlug;
    }
    if (datasourceType) {
      dataSection.provider = datasourceType;
    } else if (!datasourceSlug) {
      // Only default to bigquery if no datasource info provided
      dataSection.provider = 'bigquery';
    }

    return {
      type: 'chart',
      version: 1,
      title: 'New Chart',
      data: dataSection,
      visualize: {
        type: 'table'
      }
    };
  });

  // YAML editor state
  const [yamlText, setYamlText] = useState(() => {
    // Initialize YAML from ChartML only once
    return yaml.dump(chartML, { noRefs: true, sortKeys: false });
  });

  const [yamlError, setYamlError] = useState(null);

  // Datasource selection state - synced with chartML.data.datasource (slug)
  // Also supports legacy datasource_id for backwards compatibility
  // Falls back to datasourceSlug prop if no datasource in chartML
  const [selectedDatasource, setSelectedDatasource] = useState(() => {
    return chartMLProp?.data?.datasource || chartMLProp?.data?.datasource_id || datasourceSlug || null;
  });

  // Fetch datasources to look up type when provider is not set in chartML
  const { datasources } = useDatasources();

  // Derive the effective provider for SQL tab enable/disable logic
  // Priority: chartML.data.provider > lookup from datasource slug > prop > 'bigquery'
  const effectiveProvider = useMemo(() => {
    // If provider is explicitly set in chartML, use it
    if (chartML?.data?.provider) {
      return chartML.data.provider;
    }
    // If datasource slug is set, look up its type
    const datasourceSlugToLookup = chartML?.data?.datasource || chartML?.data?.datasource_id || selectedDatasource;
    if (datasourceSlugToLookup && datasources.length > 0) {
      const ds = datasources.find(d => d.slug === datasourceSlugToLookup);
      if (ds?.datasource_type) {
        return ds.datasource_type;
      }
    }
    // Fall back to prop or default
    return datasourceType || 'bigquery';
  }, [chartML?.data?.provider, chartML?.data?.datasource, chartML?.data?.datasource_id, selectedDatasource, datasources, datasourceType]);

  const { handleEditorWillMount, handleEditorDidMount, editorRef, monacoRef } = useMonacoEditor();
  const [baseSchema, setBaseSchema] = useState(null); // Fetched from backend
  const validationTimerRef = useRef(null);

  // Extract source components from dashboard markdown for preview
  const sourceComponents = useMemo(() => {
    if (!dashboardContent) return [];

    const chartRegex = /```(?:chart|chartml)\s*\n([\s\S]*?)\n```/g;
    const sources = [];
    let match;

    while ((match = chartRegex.exec(dashboardContent)) !== null) {
      try {
        const parsed = yaml.load(match[1]);
        const components = Array.isArray(parsed) ? parsed : [parsed];

        // Extract source components
        for (const component of components) {
          if (component.type === 'source') {
            sources.push(component);
          }
        }
      } catch (e) {
        // Skip blocks that fail to parse
      }
    }

    return sources;
  }, [dashboardContent]);

  // Track original YAML for change detection
  const originalYamlRef = useRef(yaml.dump(chartML, { noRefs: true, sortKeys: false }));
  const decorationsCollectionRef = useRef(null);
  const diffEditorRef = useRef(null); // Hidden diff editor for computing changes

  // Track if change is from user typing vs external update (prevent cursor jumping)
  const isTypingRef = useRef(false);

  // Track if update is from copilot (prevent double render from onChange trigger)
  const isCopilotUpdateRef = useRef(false);

  // Track current yamlText for editor mount (so it can access latest value)
  const yamlTextRef = useRef(yamlText);

  // Dry run hook for SQL validation - use sqlEditorText when on SQL tab
  const { dryRunning, dryRunResult } = useSQLDryRun(
    activeTab === 'sql' ? sqlEditorText : (chartML?.data?.query || ''),
    activeTab !== 'sql'
  );

  // Fetch base schema from backend on mount
  useEffect(() => {
    async function fetchSchema() {
      try {
        const schema = await getChartmlSchema();
        setBaseSchema(schema);
      } catch (error) {
      }
    }
    fetchSchema();
  }, []);

  // Cleanup diff editor on unmount
  useEffect(() => {
    return () => {
      if (diffEditorRef.current) {
        try {
          diffEditorRef.current.editor.setModel(null); // Clear model before disposal
          diffEditorRef.current.editor.dispose();
          diffEditorRef.current.container.remove();
        } catch (error) {
        } finally {
          diffEditorRef.current = null;
        }
      }
    };
  }, []);

  // Keep yamlTextRef in sync for editor mount
  useEffect(() => {
    yamlTextRef.current = yamlText;
  }, [yamlText]);

  // Register chartml language when schema is loaded
  useEffect(() => {
    if (!monacoRef.current || !baseSchema || !editorRef.current) return;

    try {
      // Register chartml as a Monaco language (idempotent - skips if already registered globally)
      registerChartmlLanguage(monacoRef.current, baseSchema);

      // Register completion provider (idempotent - skips if already registered globally)
      registerChartmlCompletionProvider(monacoRef.current, baseSchema);

    } catch (error) {
    }
  }, [baseSchema, editorRef.current]);

  // Schema validation with 3-second debounce
  useEffect(() => {
    if (!monacoRef.current || !baseSchema || !editorRef.current) return;

    // Clear previous timer
    if (validationTimerRef.current) {
      clearTimeout(validationTimerRef.current);
    }

    // Debounce validation by 3 seconds
    validationTimerRef.current = setTimeout(async () => {
      const editor = editorRef.current;
      const monaco = monacoRef.current;
      const model = editor.getModel();
      if (!model) return;

      const content = model.getValue();

      // Call async backend validation (with client-side fallback)
      const markers = await validateChartmlDocument(content, baseSchema, monaco);

      // Set markers on the model
      monaco.editor.setModelMarkers(model, 'chartmlSchema', markers);

    }, 3000);

    return () => {
      if (validationTimerRef.current) {
        clearTimeout(validationTimerRef.current);
      }
    };
  }, [yamlText, baseSchema]);

  // Configure Monaco editor when it mounts (extends useMonacoEditor hook)
  const handleEditorDidMountCustom = useCallback((editor, monaco) => {
    // Call base handler from hook
    handleEditorDidMount(editor, monaco);

    // ChartBuilderModal-specific: Set the current yamlText value (in case it changed before editor mounted)
    // Use ref to get the latest value
    editor.setValue(yamlTextRef.current || '');

    // Don't configure monaco-yaml here - it will be configured by the useEffect
    // when dynamicSchema is ready. This avoids race condition where editor mounts
    // before schema loads.

    // Create decorations collection for change indicators
    decorationsCollectionRef.current = editor.createDecorationsCollection();

    // Create a hidden diff editor for computing line changes
    // This uses Monaco's proper diff algorithm instead of naive line comparison
    if (!diffEditorRef.current) {
      const diffContainer = document.createElement('div');
      diffContainer.style.display = 'none'; // Hidden from view
      document.body.appendChild(diffContainer);

      diffEditorRef.current = {
        container: diffContainer,
        editor: monaco.editor.createDiffEditor(diffContainer, {
          readOnly: true,
          renderSideBySide: false
        })
      };
    }
  }, []);

  // Update change decorations when YAML text changes using proper diff algorithm
  useEffect(() => {
    if (!editorRef.current || !monacoRef.current || !diffEditorRef.current) return;

    // Skip if no changes
    if (yamlText === originalYamlRef.current) {
      if (decorationsCollectionRef.current) {
        decorationsCollectionRef.current.clear();
      }
      return;
    }

    const monaco = monacoRef.current;
    const diffEditor = diffEditorRef.current.editor;

    let disposed = false;
    let diffUpdateDisposable = null;

    // Create models for diff computation
    const originalModel = monaco.editor.createModel(originalYamlRef.current, 'yaml');
    const modifiedModel = monaco.editor.createModel(yamlText, 'yaml');

    try {
      // Listen for diff computation completion
      diffUpdateDisposable = diffEditor.onDidUpdateDiff(() => {
        if (disposed) return;

        try {
          // Get the computed line changes using Monaco's diff algorithm
          const lineChanges = diffEditor.getLineChanges();
          const decorations = [];

          if (lineChanges) {
            // Mark all modified lines in the modified document
            lineChanges.forEach(change => {
              // modifiedStartLineNumber and modifiedEndLineNumber indicate changed lines
              for (let lineNumber = change.modifiedStartLineNumber; lineNumber <= change.modifiedEndLineNumber; lineNumber++) {
                decorations.push({
                  range: new monaco.Range(lineNumber, 1, lineNumber, 1),
                  options: {
                    isWholeLine: true,
                    linesDecorationsClassName: 'yaml-modified-line-gutter',
                    className: 'yaml-modified-line-background'
                  }
                });
              }
            });
          }

          // Update decorations collection
          if (decorationsCollectionRef.current && !disposed) {
            decorationsCollectionRef.current.set(decorations);
          }
        } catch (error) {
        }
      });

      // Set models on the hidden diff editor - this triggers async diff computation
      diffEditor.setModel({
        original: originalModel,
        modified: modifiedModel
      });
    } catch (error) {
    }

    // Cleanup on unmount or when yamlText changes
    return () => {
      disposed = true;

      // Dispose event listener
      if (diffUpdateDisposable) {
        diffUpdateDisposable.dispose();
      }

      // Check if diff editor still exists and hasn't been disposed
      if (diffEditorRef.current && diffEditor) {
        try {
          // Reset diff editor model before disposing to release references
          diffEditor.setModel(null);
        } catch (error) {
          // Ignore errors if editor was already disposed
        }
      }

      // Cleanup models after diff computation
      originalModel.dispose();
      modifiedModel.dispose();
    };
  }, [yamlText]);

  // Update editor value manually only for external changes (not from typing) - prevents cursor jumping
  useEffect(() => {
    if (!editorRef.current || !monacoRef.current) return;

    const currentValue = editorRef.current.getValue();

    // Only update if value changed AND we're not currently typing
    if (yamlText !== currentValue && !isTypingRef.current) {
      const position = editorRef.current.getPosition();
      editorRef.current.setValue(yamlText || '');
      if (position) {
        editorRef.current.setPosition(position);
      }
    }
  }, [yamlText]);

  // Debounced YAML parsing
  const parseTimeoutRef = useRef(null);

  const handleYamlChange = useCallback((value) => {
    isTypingRef.current = true;
    setYamlText(value);

    // Reset typing flag after a short delay
    setTimeout(() => {
      isTypingRef.current = false;
    }, 100);

    // Skip parsing if this change came from copilot (already parsed and validated)
    if (isCopilotUpdateRef.current) {
      isCopilotUpdateRef.current = false;
      return;
    }

    // Clear previous timeout
    if (parseTimeoutRef.current) {
      clearTimeout(parseTimeoutRef.current);
    }

    // Debounce parsing by 500ms
    parseTimeoutRef.current = setTimeout(() => {
      try {
        const parsed = yaml.load(value);

        // Validate it has required fields
        if (!parsed || typeof parsed !== 'object') {
          setYamlError('ChartML must be an object');
          return;
        }

        if (!parsed.visualize) {
          setYamlError('ChartML must have a "visualize" section');
          return;
        }

        setChartML(parsed);
        setYamlError(null);
      } catch (e) {
        setYamlError(`YAML syntax error: ${e.message.split('\n')[0]}`);
      }
    }, 500);
  }, []);

  // Handle copilot save - update ChartML
  const handleCopilotSave = useCallback((newChartML) => {
    // Set flag to prevent onChange from triggering duplicate parse
    isCopilotUpdateRef.current = true;

    setChartML(newChartML);
    setYamlText(yaml.dump(newChartML, { noRefs: true, sortKeys: false }));
  }, []);

  // Handle visual editor change - update ChartML and sync to YAML
  const handleVisualEditorChange = useCallback((updatedChartML) => {
    setChartML(updatedChartML);
    const newYaml = yaml.dump(updatedChartML, { noRefs: true, sortKeys: false });
    setYamlText(newYaml);
  }, []);

  // Run SQL query - uses unified queryService for all datasource types
  const handleRunQuery = useCallback(async (query) => {
    if (!query) {
      throw new Error('No query provided');
    }

    if (!selectedDatasource || !effectiveProvider) {
      throw new Error('Please select a datasource before running a query');
    }

    try {
      // Execute query using unified queryService (works with all datasource types)
      const result = await queryService.executeQuery(query, {
        slug: selectedDatasource,
        type: effectiveProvider,
      }, {
        pageSize: 50,
      });

      // Store in local state for potential later use
      setQueryResults(result);

      return result;
    } catch (error) {
      // Re-throw so SQLEditor can handle the error display
      throw error;
    }
  }, [selectedDatasource, effectiveProvider]);

  // Handle SQL text changes - update ONLY local SQL state (no chartML sync)
  const handleSqlTextChange = useCallback((newSqlText) => {
    setSqlEditorText(newSqlText);
  }, []);

  // Handle datasource selection change
  const handleDatasourceChange = useCallback((datasourceSlug, datasourceObj) => {
    setSelectedDatasource(datasourceSlug);

    // Update chartML with datasource (slug) and provider
    setChartML(current => {
      const updated = {
        ...current,
        data: {
          ...current.data,
          datasource: datasourceSlug,
          provider: datasourceObj?.datasource_type || current.data?.provider || 'bigquery'
        }
      };
      // Also update YAML text to stay in sync
      setYamlText(yaml.dump(updated, { noRefs: true, sortKeys: false }));
      return updated;
    });
  }, []);

  // Copy SQL to clipboard
  const handleCopySql = useCallback(() => {
    const sqlText = activeTab === 'sql' ? sqlEditorText : (chartML?.data?.query || '');
    navigator.clipboard.writeText(sqlText);
  }, [chartML?.data?.query, activeTab, sqlEditorText]);

  // Save chart
  const handleSave = useCallback(async () => {
    if (yamlError) {
      toast.error('Please fix YAML errors before saving');
      return;
    }

    // If on SQL tab, copy SQL editor content to chartML before saving
    let finalChartML = chartML;
    if (activeTab === 'sql') {
      finalChartML = {
        ...chartML,
        data: {
          ...chartML.data,
          query: sqlEditorText
        }
      };
    }

    if (onSave) {
      await onSave(finalChartML);
    }

    // Update original YAML reference to current state (clear change indicators)
    originalYamlRef.current = yamlText;

    // Clear decorations
    if (decorationsCollectionRef.current) {
      decorationsCollectionRef.current.clear();
    }

    onClose();
  }, [chartML, yamlError, yamlText, onSave, onClose, activeTab, sqlEditorText]);

  // Show chart copilot tour when user first views chart tab
  useEffect(() => {
    if (activeTab === 'chart') {
      // Delay to ensure copilot input is rendered
      const timer = setTimeout(() => {
        showTour('chartCopilot');
      }, 500);
      return () => clearTimeout(timer);
    }
  }, [activeTab, showTour]); // Trigger when switching to chart tab

  // Config tab labels
  const CONFIG_TABS = [
    { key: 'visual', label: 'Visual' },
    { key: 'ai', label: 'AI' },
    { key: 'yaml', label: 'YAML' },
  ];

  // Modal content
  const modalContent = (
    <>
      {/* CSS for change decorations */}
      <style>{`
        .yaml-modified-line-gutter {
          background: rgba(234, 179, 8, 0.8);
          width: 3px !important;
          margin-left: 3px;
        }
        .yaml-modified-line-background {
          background: rgba(234, 179, 8, 0.1);
        }
      `}</style>

      {/* Modal Overlay (standard design system backdrop) */}
      <div
        className="modal-overlay z-[100]"
        onClick={onClose}
      >
        <div
          className="modal-content w-full h-full max-w-[95vw] max-h-[95vh] flex flex-col"
          onClick={(e) => e.stopPropagation()}
        >
          {/* Header */}
          <div className="border-b border-border px-6 py-4 flex items-center justify-between flex-shrink-0">
            <h2 className="text-lg font-semibold text-foreground">
              Chart Builder: {chartML?.title || 'New Chart'}
            </h2>

            <button
              onClick={onClose}
              className="text-muted-foreground hover:text-foreground transition-colors"
            >
              <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          {/* Main content area */}
          <div className="flex-1 min-h-0 flex flex-col">

          {/* Top-level Tabs (SQL / Chart Config) */}
          <div className="border-b border-border px-6 flex gap-8 flex-shrink-0">
            <button
              onClick={() => {
                // Switching TO SQL tab: sync query and datasource from chartML
                setSqlEditorText(chartML?.data?.query || '');
                setSelectedDatasource(chartML?.data?.datasource || chartML?.data?.datasource_id || null);
                setActiveTab('sql');
              }}
              className={`px-1 py-3 text-sm font-medium border-b-2 transition-colors ${
                activeTab === 'sql'
                  ? 'border-amber-600 text-primary'
                  : 'border-transparent text-muted-foreground hover:text-foreground hover:border-border'
              }`}
            >
              SQL Editor
            </button>
            <button
              onClick={() => {
                // Switching TO Chart tab: sync SQL, datasource back to chartML
                setChartML(current => ({
                  ...current,
                  data: {
                    ...current.data,
                    query: sqlEditorText,
                    datasource: selectedDatasource
                  }
                }));
                // Update YAML to match
                const updated = {
                  ...chartML,
                  data: {
                    ...chartML.data,
                    query: sqlEditorText,
                    datasource: selectedDatasource
                  }
                };
                setYamlText(yaml.dump(updated, { noRefs: true, sortKeys: false }));
                setActiveTab('chart');
              }}
              className={`px-1 py-3 text-sm font-medium border-b-2 transition-colors ${
                activeTab === 'chart'
                  ? 'border-amber-600 text-primary'
                  : 'border-transparent text-muted-foreground hover:text-foreground hover:border-border'
              }`}
            >
              Chart Config
            </button>
          </div>

          {/* Tab Content */}

          {/* SQL EDITOR TAB */}
          {activeTab === 'sql' && (
          <div className="px-6 py-4 flex-1 min-h-0 flex flex-col gap-4 overflow-auto">
            {/* Datasource selector + Catalog toggle */}
            <div className="flex items-center gap-3">
              <DatasourceSelector
                value={selectedDatasource}
                onChange={handleDatasourceChange}
                renderWhenEmpty={false}
              />
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={() => setSqlSidebarOpen(!sqlSidebarOpen)}
                    className={`flex items-center gap-2 px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
                      sqlSidebarOpen
                        ? 'bg-primary/10 text-primary'
                        : 'bg-accent text-foreground hover:bg-accent'
                    }`}
                  >
                    <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4m0 5c0 2.21-3.582 4-8 4s-8-1.79-8-4" />
                    </svg>
                    <span>Catalog</span>
                  </button>
                </TooltipTrigger>
                <TooltipContent>Catalog & History</TooltipContent>
              </Tooltip>
            </div>

            <div
              className="flex-1 min-h-0"
              onMouseEnter={() => setShowCopyButton(true)}
              onMouseLeave={() => setShowCopyButton(false)}
            >
              <SQLEditor
                value={sqlEditorText}
                onChange={handleSqlTextChange}
                showCopyButton={showCopyButton}
                onCopy={handleCopySql}
                dryRunning={dryRunning}
                dryRunResult={dryRunResult}
                onRunQuery={handleRunQuery}
                existingResults={queryResults}
                hideCreateChartButton={true}
                datasourceSlug={selectedDatasource}
                selectedDatasourceType={effectiveProvider}
                mobileSidebarOpen={sqlSidebarOpen}
                onMobileSidebarClose={() => setSqlSidebarOpen(false)}
              />
            </div>
          </div>
          )}

          {/* CHART CONFIG TAB */}
          {activeTab === 'chart' && (
          <div className="flex-1 min-h-0 flex">
            {/* Left: Editing Panel */}
            <div className="w-1/2 border-r border-border flex flex-col min-h-0">
              {/* Config sub-tab bar */}
              <div className="border-b border-border px-4 py-2 bg-muted flex items-center gap-1 flex-shrink-0">
                {CONFIG_TABS.map((tab) => (
                  <button
                    key={tab.key}
                    onClick={() => setConfigTab(tab.key)}
                    className={`px-3 py-1 text-xs font-medium rounded transition-colors ${
                      configTab === tab.key
                        ? 'bg-background text-foreground shadow-sm'
                        : 'text-muted-foreground hover:text-foreground'
                    }`}
                  >
                    {tab.label}
                  </button>
                ))}
              </div>

              {/* Tab content */}
              <div className="flex-1 min-h-0 overflow-auto">
                {configTab === 'visual' && (
                  <ChartVisualEditor
                    chartML={chartML}
                    onChange={handleVisualEditorChange}
                  />
                )}
                {configTab === 'ai' && (
                  <ChartBuilderCopilotSidebar
                    chartML={chartML}
                    onChartUpdate={handleCopilotSave}
                  />
                )}
                {configTab === 'yaml' && (
                  <div className="h-full min-h-0">
                    <Editor
                      key="chartml-editor"
                      height="100%"
                      defaultLanguage="chartml"
                      path="chartml.yaml"
                      defaultValue={yamlText}
                      onChange={handleYamlChange}
                      beforeMount={handleEditorWillMount}
                      onMount={handleEditorDidMountCustom}
                      theme={resolvedTheme === 'dark' ? 'chartml-dark' : 'chartml-theme'}
                      keepCurrentModel={true}
                      options={{
                        minimap: { enabled: false },
                        fontSize: 12,
                        lineNumbers: 'on',
                        scrollBeyondLastLine: false,
                        wordWrap: 'on',
                        wrappingIndent: 'indent',
                        automaticLayout: true,
                        tabSize: 2,
                        insertSpaces: true,
                        formatOnPaste: true,
                        formatOnType: true,
                        quickSuggestions: {
                          other: true,
                          comments: false,
                          strings: true
                        },
                        suggestOnTriggerCharacters: true,
                        acceptSuggestionOnEnter: 'on',
                        tabCompletion: 'on',
                        quickSuggestionsDelay: 100,
                        // Explicitly enable suggest widget
                        suggest: {
                          showWords: false,
                          showSnippets: false,
                          showMethods: false,
                          showFunctions: false,
                          showConstructors: false,
                          showFields: false,
                          showVariables: false,
                          showClasses: false,
                          showStructs: false,
                          showInterfaces: false,
                          showModules: false,
                          showProperties: true,
                          showEvents: false,
                          showOperators: false,
                          showUnits: false,
                          showValues: true,
                          showConstants: false,
                          showEnums: true,
                          showEnumMembers: true,
                          showKeywords: true,
                          showText: false,
                          showColors: false,
                          showFiles: false,
                          showReferences: false,
                          showFolders: false,
                          showTypeParameters: false,
                          showIssues: false,
                          showUsers: false
                        },
                        // Code folding
                        folding: true,
                        foldingStrategy: 'indentation',
                        showFoldingControls: 'always',
                        // Change indicators
                        glyphMargin: false,
                        lineDecorationsWidth: 10
                      }}
                    />
                  </div>
                )}
              </div>
            </div>

            {/* Right: Always-visible Preview */}
            <div className="w-1/2 flex flex-col min-h-0">
              <div className="border-b border-border px-4 py-2 bg-muted flex-shrink-0">
                <h3 className="text-xs font-medium text-foreground">Preview</h3>
              </div>

              <div className="flex-1 min-h-0 overflow-auto p-4">
                {chartML && (
                  <KyomiChart
                    spec={chartML}
                    sourceComponents={sourceComponents}
                  />
                )}
              </div>
            </div>
          </div>
          )}

          {/* Unified Footer - always visible */}
          <div className="border-t border-border px-6 py-3 bg-card flex justify-end gap-3 flex-shrink-0">
            <button
              onClick={onClose}
              className="px-3 py-1.5 text-xs font-medium text-foreground hover:text-foreground hover:bg-accent rounded-md transition-colors"
            >
              Cancel
            </button>
            <button
              onClick={handleSave}
              disabled={!!yamlError}
              className="px-4 py-2 text-xs font-medium text-white bg-primary hover:bg-primary/90 rounded-md transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Save Chart
            </button>
          </div>

          </div>
          {/* End main content area */}

        </div>
      </div>
    </>
  );

  return createPortal(modalContent, document.body);
};

export default ChartBuilderModal;
