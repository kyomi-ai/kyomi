// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useRef, useCallback } from 'react';
import { useNavigate, useParams, useLocation } from 'react-router-dom';
import { useQueryClient } from '@tanstack/react-query';
import { useAuth } from '../context/AuthContext';
import { useWebSocket } from '../context/WebSocketContext';
import { DashboardProvider, useDashboard } from '../context/DashboardContext';
import { TiptapDashboardEditor } from '../components/tiptap/TiptapDashboardEditor';
import { MarkdownRenderer } from '../components/MarkdownRenderer';
import ChartBuilderModal from '../components/ChartBuilderModal';
import MonacoMarkdownEditor from '../components/MonacoMarkdownEditor';
import DashboardCopilotSidebar from '../components/DashboardCopilotSidebar';
import { parseMarkdownChartML } from '../lib/markdownChartMLParser';
import { validateParameterIds } from '../lib/parameterValidator';
import { loader } from '@monaco-editor/react';
import { useTheme } from '../context/ThemeContext';
import { Tooltip, TooltipTrigger, TooltipContent } from '../components/ui/tooltip';
import { ChatBubbleLeftRightIcon, ClockIcon } from '@heroicons/react/24/outline';
import yaml from 'js-yaml';
import DashboardHistoryPanel from '../components/DashboardHistoryPanel';
import ConfirmDialog from '../components/ConfirmDialog';
import useConfirm from '../hooks/useConfirm';
import InlineEditableTitle from '../components/InlineEditableTitle';
import { useProductTour } from '../components/ProductTour';
import SaveDashboardModal from '../components/SaveDashboardModal';
import ChartInfoModal from '../components/ChartInfoModal';
import InsertDashboardLinkModal from '../components/InsertDashboardLinkModal';

// Constant placeholder text (outside component to prevent recreation on render)
const EDITOR_PLACEHOLDER = `# Welcome to Dashboard Editor

Create a markdown-based dashboard with embedded charts using ChartML.

## How to Add Charts

Use a chart code block with ChartML (YAML or JSON):

\`\`\`chart
- version: 1
  type: vertical-bar
  title: Sales by Region
  sql: SELECT region, SUM(revenue) as total FROM sales GROUP BY region
  x: region
  series:
    - y: total
\`\`\`

## Tips
- Write regular markdown for text, headings, tables, and lists
- Click the "Add Chart" button to use the visual chart builder
- Toggle "Preview" to see your dashboard rendered with live charts
- Press "Save" to store your dashboard

Need help? Click the documentation link above for the complete ChartML guide.`;

/**
 * DashboardEditorContent - Inner component with dashboard editing logic
 */
const DashboardEditorContent = () => {
  const navigate = useNavigate();
  const { dashboardId } = useParams();
  const { apiClient } = useAuth();
  const { updateParameters } = useDashboard();
  const { isOpen, dialogProps, confirm } = useConfirm();
  const queryClient = useQueryClient();
  const { subscribe } = useWebSocket();
  const { showTour } = useProductTour();
  const { resolvedTheme } = useTheme();

  const [title, setTitle] = useState('Untitled Dashboard');
  const [content, setContent] = useState('');
  const [debouncedContent, setDebouncedContent] = useState(''); // Debounced content for preview
  const [originalContent, setOriginalContent] = useState({ title: 'Untitled Dashboard', content: '' });
  const [hasUnsavedChanges, setHasUnsavedChanges] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [_error, setError] = useState(null);
  // Editor mode: 'visual' for Tiptap WYSIWYG, 'source' for Monaco markdown
  const [editorMode, setEditorMode] = useState('visual');
  const [cursorPosition, setCursorPosition] = useState({ line: 1, column: 1 });
  const debounceTimerRef = useRef(null);
  const validationTimerRef = useRef(null); // Separate timer for parameter validation
  const editorViewRef = useRef(null); // Monaco editor instance
  const [editorReady, setEditorReady] = useState(false); // Track when editor is created
  const tiptapEditorRef = useRef(null);

  // Chart builder state
  const [showChartBuilder, setShowChartBuilder] = useState(false);
  const [editingChartML, setEditingChartML] = useState(null);
  const [editingChartIndex, setEditingChartIndex] = useState(null);
  const [editingChartArrayIndex, setEditingChartArrayIndex] = useState(null);
  const [editingNodeId, setEditingNodeId] = useState(null); // Tiptap node ID for visual mode
  const [isAddingNewChart, setIsAddingNewChart] = useState(false);

  // Copilot sidebar state
  const [copilotOpen, setCopilotOpen] = useState(false);

  // History panel state
  const [historyPanelOpen, setHistoryPanelOpen] = useState(false);
  const [previewingVersion, setPreviewingVersion] = useState(null);

  // Modal states for chart actions (Add to Dashboard, Chart Info)
  const [saveToDashboardModal, setSaveToDashboardModal] = useState({ isOpen: false, messageContent: '' });
  const [chartInfoModal, setChartInfoModal] = useState({ isOpen: false, spec: null });
  const [insertDashboardLinkModal, setInsertDashboardLinkModal] = useState(false);

  // Handle dashboard update from copilot
  const handleCopilotDashboardUpdate = useCallback((newContent) => {
    setContent(newContent);
    setDebouncedContent(newContent);
  }, []);

  // Handle version restore - reload dashboard and close panel
  const handleVersionRestored = useCallback(async () => {
    await loadDashboard();
    setPreviewingVersion(null);
    setHistoryPanelOpen(false);
  }, []);

  // Handle version preview - show version content in editor (read-only preview)
  const handlePreviewVersion = useCallback((versionData) => {
    setPreviewingVersion(versionData);
  }, []);

  // Ref to track if we should allow navigation
  const allowNavigationRef = useRef(false);
  const location = useLocation();

  // Block browser back/forward navigation when there are unsaved changes
  useEffect(() => {
    const handlePopState = async () => {
      if (hasUnsavedChanges && !allowNavigationRef.current) {
        // Immediately push state back to block navigation
        window.history.pushState(null, '', window.location.pathname);

        // Show styled confirm dialog
        const shouldLeave = await confirm({
          title: 'Unsaved Changes',
          message: 'You have unsaved changes. Are you sure you want to leave this page? Your changes will be lost.',
          confirmText: 'Leave Page',
          variant: 'destructive'
        });

        if (shouldLeave) {
          allowNavigationRef.current = true;
          window.history.back();
        }
      }
    };

    if (hasUnsavedChanges) {
      // Push a dummy state so popstate fires when back is pressed
      window.history.pushState(null, '', window.location.pathname);
      window.addEventListener('popstate', handlePopState);
    }

    return () => {
      window.removeEventListener('popstate', handlePopState);
    };
  }, [hasUnsavedChanges, location, confirm]);

  // Handle content change from editor
  const handleContentChange = useCallback((value) => {
    setContent(value);
  }, []);

  // Handle chart edit - open ChartBuilderModal with existing ChartML
  const handleEditChart = React.useCallback((chartML, chartIndex, chartArrayIndex, nodeId) => {
    setEditingChartML(chartML);
    setEditingChartIndex(chartIndex);
    setEditingChartArrayIndex(chartArrayIndex);
    setEditingNodeId(nodeId); // Store node ID for Tiptap mode
    setShowChartBuilder(true);
  }, []);

  // Handle "Add to Dashboard" button - opens modal to copy chart to another dashboard
  const handleSaveChartToDashboard = useCallback((chartMarkdown) => {
    setSaveToDashboardModal({
      isOpen: true,
      messageContent: chartMarkdown,
    });
  }, []);

  // Handle "Chart Info" button - shows chart specification details
  const handleShowChartInfo = useCallback((spec) => {
    setChartInfoModal({ isOpen: true, spec });
  }, []);

  // Handle dashboard link insertion from modal
  const handleInsertDashboardLink = useCallback((dashboard) => {
    if (tiptapEditorRef.current) {
      tiptapEditorRef.current.insertLinkAtSavedPosition(
        dashboard.title || 'Untitled Dashboard',
        dashboard.dashboard_id
      );
    }
  }, []);

  // Handle "Ask about this chart" button - navigates to chat with chart context
  const handleAskAboutChart = useCallback((chartMarkdown, spec) => {
    // Navigate to chat with chart context in router state
    navigate('/chat', {
      state: {
        exploreChart: true,
        chartMarkdown,
        chartTitle: spec?.style?.title || spec?.title || 'Chart'
      }
    });
  }, [navigate]);

  // Handle save from SaveDashboardModal
  const handleSaveToDashboardConfirm = async (mode, titleOrDashboardId, content) => {
    try {
      if (mode === 'new') {
        // Create new dashboard with the chart content
        const newDashboard = await apiClient.post('/api/v1/dashboards', {
          title: titleOrDashboardId,
          content: content
        });
        navigate(`/dashboard/${newDashboard.data.dashboard_id}`);
      } else {
        // Add to existing dashboard
        const existingDashboard = await apiClient.get(`/api/v1/dashboards/${titleOrDashboardId}`);
        const updatedContent = existingDashboard.data.content + '\n\n---\n\n' + content;
        await apiClient.patch(`/api/v1/dashboards/${titleOrDashboardId}`, { content: updatedContent });
        navigate(`/dashboard/${titleOrDashboardId}`);
      }
    } catch (error) {
      throw error; // Let the modal handle the error
    }
  };

  // Handle chart save from ChartBuilderModal - update markdown or insert new
  const handleChartBuilderSave = (updatedChartML) => {
    // Handle new chart insertion via Tiptap editor (visual mode)
    if (isAddingNewChart && editorMode === 'visual' && tiptapEditorRef.current) {
      // Serialize the chart to YAML
      const chartYaml = yaml.dump(updatedChartML, { indent: 2, lineWidth: -1 });

      // Insert at saved cursor position
      tiptapEditorRef.current.insertChartAtSavedPosition(chartYaml);

      // Close modal and reset state
      setShowChartBuilder(false);
      setIsAddingNewChart(false);
      setEditingChartML(null);
      setEditingChartIndex(null);
      setEditingChartArrayIndex(null);
      setEditingNodeId(null);
      return;
    }

    // Handle chart update in Tiptap editor (visual mode) - update node by ID
    if (editorMode === 'visual' && editingNodeId && tiptapEditorRef.current) {
      const editor = tiptapEditorRef.current.getEditor();

      // Safety check - editor might not be initialized yet
      if (!editor) {
        return;
      }

      // Find the node by ID and update its content attribute
      editor.state.doc.descendants((node, pos) => {
        if (node.type.name === 'chartMLBlock' && node.attrs.id === editingNodeId) {
          // Serialize updated ChartML to YAML
          const updatedYaml = yaml.dump(updatedChartML, { indent: 2, lineWidth: -1 });

          // Update the node's content attribute
          editor.chain()
            .setNodeSelection(pos)
            .updateAttributes('chartMLBlock', { content: updatedYaml })
            .run();

          // Close modal and reset state
          setShowChartBuilder(false);
          setEditingChartML(null);
          setEditingChartIndex(null);
          setEditingChartArrayIndex(null);
          setEditingNodeId(null);
          return false; // Stop traversal
        }
      });
      return;
    }

    if (editingChartIndex === null) {
      return;
    }

    // Find and replace the Nth ChartML block in markdown
    // Must match ALL ChartML block types (source, params, config, chart, chartml)
    // to align with chartBlockIndex from @chartml/markdown-react
    const chartRegex = /```(?:source|params|config|chart|chartml)\s*\n([\s\S]*?)\n```/g;
    let count = 0;
    let match;
    const matches = [];

    // Find all chart blocks and their positions
    while ((match = chartRegex.exec(content)) !== null) {
      matches.push({
        index: count,
        startPos: match.index,
        endPos: match.index + match[0].length,
        fullMatch: match[0],
        content: match[1]
      });
      count++;
    }

    // Find the ChartML block to update
    const chartToUpdate = matches.find(m => m.index === editingChartIndex);
    if (!chartToUpdate) {
      return;
    }

    // Parse the existing chart block
    const existingChartData = yaml.load(chartToUpdate.content);

    // Determine what to serialize
    let dataToSerialize;
    if (Array.isArray(existingChartData)) {
      // Chart block contains array - update specific chart within array
      if (editingChartArrayIndex === null) {
        return;
      }

      const updatedArray = [...existingChartData];
      updatedArray[editingChartArrayIndex] = updatedChartML;
      dataToSerialize = updatedArray;
    } else {
      // Chart block contains single chart - replace entire chart
      dataToSerialize = updatedChartML;
    }

    // Serialize to YAML
    const serialized = yaml.dump(dataToSerialize, { indent: 2, lineWidth: -1 });
    const updatedBlock = '```chartml\n' + serialized + '\n```';
    const updatedContent = content.substring(0, chartToUpdate.startPos) +
                           updatedBlock +
                           content.substring(chartToUpdate.endPos);

    // Update both content and debouncedContent immediately
    setContent(updatedContent);
    setDebouncedContent(updatedContent);

    // Close modal and reset state
    setShowChartBuilder(false);
    setIsAddingNewChart(false);
    setEditingChartML(null);
    setEditingChartIndex(null);
    setEditingChartArrayIndex(null);
  };

  // Debounce content updates for parameter initialization (3 second delay)
  useEffect(() => {
    // Clear any existing timer
    if (debounceTimerRef.current) {
      clearTimeout(debounceTimerRef.current);
    }

    // Set new timer to update debounced content after 3 seconds
    debounceTimerRef.current = setTimeout(() => {
      setDebouncedContent(content);
    }, 3000);

    // Cleanup on unmount or when content changes
    return () => {
      if (debounceTimerRef.current) {
        clearTimeout(debounceTimerRef.current);
      }
    };
  }, [content]);

  // Validate parameters with 3-second debounce (matches preview debounce)
  useEffect(() => {
    // Clear any existing validation timer
    if (validationTimerRef.current) {
      clearTimeout(validationTimerRef.current);
    }

    // Set new timer to validate after 3 seconds
    validationTimerRef.current = setTimeout(async () => {
      if (!editorViewRef.current || !editorReady) return;

      const editor = editorViewRef.current;
      const model = editor.getModel();
      if (!model) return;

      // Run validation
      const markers = validateParameterIds(content);

      // Get Monaco instance and set markers
      try {
        const monaco = await loader.init();
        monaco.editor.setModelMarkers(model, 'parameterValidator', markers);
      } catch (error) {
      }
    }, 3000);

    // Cleanup on unmount or when content changes
    return () => {
      if (validationTimerRef.current) {
        clearTimeout(validationTimerRef.current);
      }
    };
  }, [content, editorReady]);

  // Initialize parameter values when debounced content changes
  useEffect(() => {
    if (!debouncedContent) return;

    // Parse all components from markdown to get chart-level parameters
    const { parameters, charts } = parseMarkdownChartML(debouncedContent);

    // Build initial parameter values from defaults
    const initialValues = {};

    // Process dashboard-level parameters (global parameters)
    if (parameters && parameters.length > 0) {
      const allParamDefinitions = parameters.flat();
      allParamDefinitions.forEach(param => {
        if (initialValues[param.id] === undefined) {
          initialValues[param.id] = param.default;
        }
      });
    }

    // Process chart-level params (scoped to each chart)
    // IMPORTANT: MarkdownRenderer increments chartIndex for EVERY chartml block,
    // then ChartGridv2 builds scope as chart_${chartIndex}_${arrayIndex}
    // We must replicate this exact logic here
    const codeBlockRegex = /```chartml\s*\n([\s\S]*?)```/g;
    let chartIndex = 0;  // Increments for EVERY chartml block (matches MarkdownRenderer)
    let flatChartIndex = 0;  // Tracks position in charts array

    let match;
    while ((match = codeBlockRegex.exec(debouncedContent)) !== null) {
      try {
        const content = match[1].trim();
        const parsed = JSON.parse(JSON.stringify(yaml.load(content)));
        const components = Array.isArray(parsed) ? parsed : [parsed];

        // Check if this block contains any charts
        const blockCharts = components.filter(c => !c.type || c.type === 'chart');

        if (blockCharts.length > 0) {
          // This block contains charts - process their params
          blockCharts.forEach((chart, arrayIndex) => {
            const chartInFlatArray = charts[flatChartIndex];

            if (chartInFlatArray?.parameters && chartInFlatArray.parameters.length > 0) {
              // Use chartIndex (increments per block) not chartBlockIndex
              const chartScope = `chart_${chartIndex}_${arrayIndex}`;

              chartInFlatArray.parameters.forEach(param => {
                const scopedKey = `${chartScope}.${param.id}`;
                if (initialValues[scopedKey] === undefined) {
                  initialValues[scopedKey] = param.default;
                }
              });
            }

            flatChartIndex++;
          });

          // Increment chartIndex after processing ALL charts in this block
          chartIndex++;
        }
      } catch (error) {
        // Skip blocks that fail to parse
      }
    }

    updateParameters(initialValues);
  }, [debouncedContent, updateParameters]);

  // Track unsaved changes
  useEffect(() => {
    const changed = title !== originalContent.title || content !== originalContent.content;
    // Only update state if the value actually changes
    setHasUnsavedChanges(prev => {
      if (prev === changed) return prev; // No change, avoid re-render
      return changed;
    });
  }, [title, content, originalContent]);

  // Handle editor mount
  const handleEditorMount = useCallback(() => {
    setEditorReady(true);
  }, []);

  // Warn before navigating away with unsaved changes
  useEffect(() => {
    const handleBeforeUnload = (e) => {
      if (hasUnsavedChanges) {
        e.preventDefault();
        e.returnValue = '';
      }
    };

    window.addEventListener('beforeunload', handleBeforeUnload);
    return () => window.removeEventListener('beforeunload', handleBeforeUnload);
  }, [hasUnsavedChanges]);

  // Load dashboard if editing existing one
  useEffect(() => {
    if (dashboardId && dashboardId !== 'new') {
      loadDashboard();
    }
  }, [dashboardId]);

  // Subscribe to WebSocket for async dashboard summary updates
  useEffect(() => {
    if (!dashboardId || dashboardId === 'new') return;

    const unsubscribe = subscribe('dashboard_update', (message) => {
      if (message.data?.context_type !== 'dashboard_summary') return;
      if (message.data?.dashboard_id !== dashboardId) return;

      const newContent = message.data.content;
      if (!newContent) return;

      // Only update if user hasn't made new edits since the summary was triggered
      if (!hasUnsavedChanges) {
        setContent(newContent);
        setDebouncedContent(newContent);
        setOriginalContent(prev => ({ ...prev, content: newContent }));
      }

      // Always invalidate list cache so cards show the summary
      queryClient.invalidateQueries(['dashboards']);
    });

    return unsubscribe;
  }, [dashboardId, subscribe, hasUnsavedChanges, queryClient]);

  const loadDashboard = async () => {
    setIsLoading(true);
    setError(null);
    try {
      const response = await apiClient.get(`/api/v1/dashboards/${dashboardId}`);
      const loadedTitle = response.data.title;
      const loadedContent = response.data.content;
      setTitle(loadedTitle);
      setContent(loadedContent);
      setDebouncedContent(loadedContent); // Initialize debounced content
      setOriginalContent({ title: loadedTitle, content: loadedContent });
    } catch (err) {
      setError('Failed to load dashboard');
    } finally {
      setIsLoading(false);
    }
  };

  const [saveSuccess, setSaveSuccess] = useState(false);

  const handleSave = async () => {
    setIsSaving(true);
    setError(null);
    try {
      let savedDashboardId = dashboardId;

      if (dashboardId === 'new') {
        // Create new dashboard
        const response = await apiClient.post('/api/v1/dashboards', {
          title,
          content
        });
        savedDashboardId = response.data.dashboard_id;
        // Update URL to reflect saved dashboard ID (without full navigation)
        navigate(`/dashboard/edit/${savedDashboardId}`, { replace: true });
      } else {
        // Update existing dashboard
        await apiClient.patch(`/api/v1/dashboards/${dashboardId}`, {
          title,
          content
        });
      }

      // Update original content to reflect saved state
      setOriginalContent({ title, content });
      setHasUnsavedChanges(false);

      // Invalidate dashboards query cache so list refreshes
      queryClient.invalidateQueries(['dashboards']);

      // Show success feedback briefly
      setSaveSuccess(true);
      setTimeout(() => setSaveSuccess(false), 2000);
    } catch (err) {
      setError('Failed to save dashboard');
    } finally {
      setIsSaving(false);
    }
  };

  const handleClose = async () => {
    if (hasUnsavedChanges) {
      const confirmed = await confirm({
        title: 'Unsaved Changes',
        message: 'You have unsaved changes. Are you sure you want to leave?',
        confirmText: 'Leave',
        variant: 'destructive'
      });

      if (confirmed) {
        // Navigate to dashboard viewer if we have a saved dashboard, otherwise to list
        if (dashboardId && dashboardId !== 'new') {
          navigate(`/dashboard/${dashboardId}`);
        } else {
          navigate('/dashboards');
        }
      }
    } else {
      // Navigate to dashboard viewer if we have a saved dashboard, otherwise to list
      if (dashboardId && dashboardId !== 'new') {
        navigate(`/dashboard/${dashboardId}`);
      } else {
        navigate('/dashboards');
      }
    }
  };

  // Show chart edit tour when in visual mode with content
  useEffect(() => {
    if (editorMode !== 'visual' || !content) return;
    // Delay to allow charts to render
    const timer = setTimeout(() => {
      const editButton = document.querySelector('button[aria-label="Edit chart"]');
      if (editButton) {
        showTour('dashboardChartEdit');
      }
    }, 1000);
    return () => clearTimeout(timer);
  }, [content, editorMode, showTour]);

  // Show dashboard editor tour on first load
  useEffect(() => {
    showTour('dashboardEditor');
  }, [showTour]);

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full bg-background">
        <img src="/kyomi_animated_logo.svg" alt="Loading" className="w-12 h-12" />
      </div>
    );
  }

  // Mode toggle component for toolbar right slot
  const modeToggle = (
    <div className="flex items-center bg-accent rounded-md p-0.5">
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            onClick={() => setEditorMode('source')}
            className={`px-1.5 sm:px-2 py-1 text-xs font-medium rounded transition-colors flex items-center gap-1 ${
              editorMode === 'source'
                ? 'bg-card text-foreground shadow-sm'
                : 'text-muted-foreground hover:text-foreground'
            }`}
            aria-label="Source editor"
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M10 20l4-16m4 4l4 4-4 4M6 16l-4-4 4-4" />
            </svg>
            <span className="hidden sm:inline">Source</span>
          </button>
        </TooltipTrigger>
        <TooltipContent>Markdown source</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            onClick={() => setEditorMode('visual')}
            className={`px-1.5 sm:px-2 py-1 text-xs font-medium rounded transition-colors flex items-center gap-1 ${
              editorMode === 'visual'
                ? 'bg-card text-foreground shadow-sm'
                : 'text-muted-foreground hover:text-foreground'
            }`}
            aria-label="Visual editor"
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
            </svg>
            <span className="hidden sm:inline">Visual</span>
          </button>
        </TooltipTrigger>
        <TooltipContent>Visual editor (WYSIWYG)</TooltipContent>
      </Tooltip>
    </div>
  );

  return (
      <div className="h-full flex flex-col overflow-hidden">
        {/* Header Box */}
        <div className="h-16 bg-card border-b border-border px-6 flex-shrink-0 flex items-center justify-between">
            <div className="flex items-center flex-1">
              <InlineEditableTitle
                value={title}
                onSave={setTitle}
                placeholder="Untitled Dashboard"
              />
            </div>

            <div className="flex items-center space-x-3">
            {/* History toggle button - only show for existing dashboards */}
            {dashboardId && dashboardId !== 'new' && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={() => {
                      if (historyPanelOpen) {
                        setHistoryPanelOpen(false);
                      } else {
                        setCopilotOpen(false); // Close Copilot when opening History
                        setHistoryPanelOpen(true);
                      }
                    }}
                    className={`flex items-center gap-2 px-2 md:px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
                      historyPanelOpen
                        ? 'bg-primary/10 text-primary'
                        : 'bg-accent text-foreground hover:bg-accent'
                    }`}
                    aria-label="Toggle version history"
                  >
                    <ClockIcon className="w-4 h-4 flex-shrink-0" />
                    <span className="hidden sm:inline">History</span>
                  </button>
                </TooltipTrigger>
                <TooltipContent>View version history</TooltipContent>
              </Tooltip>
            )}

            {/* Copilot toggle button */}
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => {
                    if (copilotOpen) {
                      setCopilotOpen(false);
                    } else {
                      setHistoryPanelOpen(false); // Close History when opening Copilot
                      setPreviewingVersion(null); // Exit preview mode
                      setCopilotOpen(true);
                    }
                  }}
                  className={`flex items-center gap-2 px-2 md:px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
                    copilotOpen
                      ? 'bg-primary/10 text-primary'
                      : 'bg-accent text-foreground hover:bg-accent'
                  }`}
                  aria-label="Toggle Copilot"
                >
                  <ChatBubbleLeftRightIcon className="w-4 h-4 flex-shrink-0" />
                  <span className="hidden sm:inline">Copilot</span>
                </button>
              </TooltipTrigger>
              <TooltipContent>AI assistant for dashboard editing</TooltipContent>
            </Tooltip>

            <button
              onClick={handleClose}
              className="flex items-center gap-2 px-2 md:px-4 py-2 text-sm font-medium bg-accent text-foreground hover:bg-accent rounded-lg transition-colors"
            >
              <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
              <span className="hidden sm:inline">Close</span>
            </button>

            <button
              onClick={handleSave}
              disabled={isSaving}
              className={`flex items-center gap-2 px-2 md:px-4 py-2 text-sm font-medium text-white rounded-lg transition-colors disabled:opacity-50 ${
                saveSuccess ? 'bg-success-foreground' : 'bg-primary hover:bg-primary/90'
              }`}
            >
              {saveSuccess ? (
                <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                </svg>
              ) : (
                <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 7H5a2 2 0 00-2 2v9a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-3m-1 4l-3 3m0 0l-3-3m3 3V4" />
                </svg>
              )}
              <span className="hidden sm:inline">{isSaving ? 'Saving...' : saveSuccess ? 'Saved!' : 'Save'}</span>
            </button>
          </div>
        </div>

        {/* Content Area with optional sidebars */}
        <div className="flex-1 overflow-hidden flex">
          {/* Main editor area */}
          <div className="flex-1 overflow-hidden bg-muted p-4 md:p-6">
            <div className="bg-card rounded-lg border border-border shadow-sm h-full flex flex-col overflow-hidden">
              {editorMode === 'visual' ? (
                /* Visual mode: Tiptap editor (hidden when previewing) + MarkdownRenderer preview */
                <>
                  {/* Preview mode: show yellow toolbar + MarkdownRenderer */}
                  {previewingVersion && (
                    <div className="flex flex-col h-full">
                      <div className="flex items-center gap-2 p-3 border-b border-warning-border bg-warning flex-shrink-0">
                        <span className="text-sm font-medium text-warning-foreground">
                          Previewing Version {previewingVersion.version_number}
                        </span>
                        <span className="text-xs text-warning-foreground">Read-only</span>
                        <div className="flex-1" />
                        {modeToggle}
                      </div>
                      <div className="flex-1 overflow-auto p-4 md:p-6">
                        <MarkdownRenderer>{previewingVersion.content}</MarkdownRenderer>
                      </div>
                    </div>
                  )}
                  {/* Edit mode: show TiptapDashboardEditor (hidden when previewing) */}
                  <div className={previewingVersion ? 'hidden' : 'contents'}>
                    <TiptapDashboardEditor
                      ref={tiptapEditorRef}
                      content={content}
                      onChange={(newContent) => {
                        setContent(newContent);
                        setDebouncedContent(newContent);
                      }}
                      onEditChart={handleEditChart}
                      onInsertChart={() => {
                        // Open chart builder for new chart (no existing chartML)
                        setIsAddingNewChart(true);
                        setEditingChartML(null);
                        setEditingChartIndex(null);
                        setEditingChartArrayIndex(null);
                        setShowChartBuilder(true);
                      }}
                      onInsertDashboardLink={() => setInsertDashboardLinkModal(true)}
                      onSaveChartToDashboard={handleSaveChartToDashboard}
                      onShowChartInfo={handleShowChartInfo}
                      onAskAboutChart={handleAskAboutChart}
                      placeholder="Start typing your dashboard content, or click 'Add Chart' to insert a chart..."
                      rightSlot={modeToggle}
                    />
                  </div>
                </>
              ) : (
                /* Source mode: Monaco markdown editor with matching toolbar */
                <>
                  {/* Toolbar - yellow when previewing, normal otherwise */}
                  <div className={`flex items-center gap-2 p-3 border-b flex-shrink-0 ${
                    previewingVersion
                      ? 'border-warning-border bg-warning'
                      : 'border-border bg-muted/50'
                  }`}>
                    {previewingVersion ? (
                      <>
                        <span className="text-sm font-medium text-warning-foreground">
                          Previewing Version {previewingVersion.version_number}
                        </span>
                        <span className="text-xs text-warning-foreground">Read-only</span>
                      </>
                    ) : (
                      <span className="text-xs text-muted-foreground">Markdown</span>
                    )}
                    <div className="flex-1" />
                    {modeToggle}
                  </div>
                  <div className="flex-1 min-h-0 overflow-hidden">
                    <MonacoMarkdownEditor
                      value={previewingVersion ? previewingVersion.content : content}
                      onChange={previewingVersion ? undefined : handleContentChange}
                      onCursorChange={setCursorPosition}
                      onMount={handleEditorMount}
                      placeholder={EDITOR_PLACEHOLDER}
                      fontSize={13}
                      editorRef={editorViewRef}
                      disabled={!!previewingVersion}
                      editorTheme={resolvedTheme}
                    />
                  </div>
                  {/* Cursor Position Status Bar */}
                  <div className="px-3 py-1.5 bg-muted border-t border-border text-xs text-muted-foreground flex justify-end flex-shrink-0">
                    Ln {String(cursorPosition.line)}, Col {String(cursorPosition.column)}
                  </div>
                </>
              )}
            </div>
          </div>

          {/* History panel (same pattern as Copilot) */}
          {dashboardId && dashboardId !== 'new' && (
            <DashboardHistoryPanel
              isOpen={historyPanelOpen}
              onClose={() => setHistoryPanelOpen(false)}
              dashboardId={dashboardId}
              onPreviewVersion={handlePreviewVersion}
              onRestoreVersion={handleVersionRestored}
            />
          )}

          {/* Copilot sidebar */}
          <DashboardCopilotSidebar
            isOpen={copilotOpen}
            onClose={() => setCopilotOpen(false)}
            dashboardContent={previewingVersion ? previewingVersion.content : content}
            onDashboardUpdate={handleCopilotDashboardUpdate}
          />
        </div>

        {/* Chart Builder Modal */}
        {showChartBuilder && (
          <ChartBuilderModal
            chartML={editingChartML}
            dashboardContent={content}
            onSave={handleChartBuilderSave}
            onClose={() => {
              setShowChartBuilder(false);
              setEditingChartML(null);
              setEditingChartIndex(null);
              setEditingChartArrayIndex(null);
            }}
          />
        )}

      {/* Confirm Dialog */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />

      {/* Chart action modals */}
      <SaveDashboardModal
        isOpen={saveToDashboardModal.isOpen}
        onClose={() => setSaveToDashboardModal({ isOpen: false, messageContent: '' })}
        onSave={handleSaveToDashboardConfirm}
        messageContent={saveToDashboardModal.messageContent}
        apiClient={apiClient}
      />
      <ChartInfoModal
        isOpen={chartInfoModal.isOpen}
        onClose={() => setChartInfoModal({ isOpen: false, spec: null })}
        spec={chartInfoModal.spec}
      />
      <InsertDashboardLinkModal
        isOpen={insertDashboardLinkModal}
        onClose={() => setInsertDashboardLinkModal(false)}
        onSelect={handleInsertDashboardLink}
        apiClient={apiClient}
      />
      </div>
  );
};

/**
 * DashboardEditor - Wrapper that provides DashboardContext
 */
const DashboardEditor = () => {
  return (
    <DashboardProvider>
      <DashboardEditorContent />
    </DashboardProvider>
  );
};

export default DashboardEditor;
