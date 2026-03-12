// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useRef, useCallback } from 'react';
import { useNavigate, useParams, useLocation } from 'react-router-dom';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useAuth } from '../context/AuthContext';
import { useSystemConfig } from '../context/SystemConfigContext';
import { toast } from '../lib/toast';
import { DashboardProvider, useDashboard } from '../context/DashboardContext';
import { MarkdownRenderer } from '../components/MarkdownRenderer';
import { parseMarkdownChartML } from '../lib/markdownChartMLParser';
import * as yaml from 'js-yaml';
import { ArrowPathIcon, ArrowDownTrayIcon, ClockIcon, EllipsisVerticalIcon } from '@heroicons/react/24/outline';
import { Tooltip, TooltipTrigger, TooltipContent } from '../components/ui/tooltip';
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
} from '../components/ui/dropdown-menu';
import InlineEditableTitle from '../components/InlineEditableTitle';
import SaveDashboardModal from '../components/SaveDashboardModal';
import ChartInfoModal from '../components/ChartInfoModal';
import DashboardHistoryPanel from '../components/DashboardHistoryPanel';

/**
 * useDownloadPDF — PDF export logic extracted so both the desktop button
 * and the mobile overflow menu can share the same handler / loading state.
 */
function useDownloadPDF({ dashboardId, dashboardTitle, parameterValues, apiClient }) {
  const [isExporting, setIsExporting] = useState(false);

  const handleDownloadPDF = async () => {
    if (isExporting) return;
    setIsExporting(true);
    try {
      const params = {};
      if (parameterValues && Object.keys(parameterValues).length > 0) {
        params.parameters = JSON.stringify(parameterValues);
      }
      const response = await apiClient.get(
        `/api/v1/dashboards/${dashboardId}/export/pdf`,
        { params, responseType: 'blob' }
      );

      const blob = new Blob([response.data], { type: 'application/pdf' });
      const url = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = url;
      const contentDisposition = response.headers['content-disposition'];
      let filename = 'Dashboard.pdf';
      if (contentDisposition) {
        const match = contentDisposition.match(/filename="?(.+?)"?$/);
        if (match) filename = match[1];
      } else if (dashboardTitle) {
        filename = `${dashboardTitle.replace(/[^\w\s-]/g, '').replace(/\s+/g, '_')}.pdf`;
      }
      link.download = filename;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      URL.revokeObjectURL(url);
    } catch (err) {
      let message = 'Failed to export PDF';
      if (err.response?.status === 403) {
        message = 'PDF export requires a paid plan';
      } else if (err.response?.data) {
        try {
          const text = err.response.data instanceof Blob
            ? await err.response.data.text()
            : JSON.stringify(err.response.data);
          const parsed = JSON.parse(text);
          if (parsed.detail) message = `PDF export failed: ${parsed.detail}`;
        } catch { /* ignore parse errors */ }
      }
      toast.error(message);
      console.error('PDF export error:', err);
    } finally {
      setIsExporting(false);
    }
  };

  return { isExporting, handleDownloadPDF };
}

/**
 * DashboardViewerContent - Inner component that uses DashboardContext
 */
const DashboardViewerContent = () => {
  const navigate = useNavigate();
  const location = useLocation();
  const { dashboardId } = useParams();
  const { user, apiClient, refreshUser } = useAuth();
  const { features } = useSystemConfig();
  const { parameterValues, updateParameters } = useDashboard();
  const queryClient = useQueryClient();

  // Check if user is workspace admin
  const isAdmin = user?.workspace_roles?.includes('workspace_admin') || false;

  const [dashboard, setDashboard] = useState(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState(null);
  const [paramsInitialized, setParamsInitialized] = useState(false);
  const hasInitializedParams = useRef(false); // Track if we've initialized params for this dashboard

  // Modal states for chart actions
  const [dashboardModal, setDashboardModal] = useState({ isOpen: false, messageContent: '' });
  const [chartInfoModal, setChartInfoModal] = useState({ isOpen: false, spec: null });
  const [historyPanelOpen, setHistoryPanelOpen] = useState(false);
  const [previewingVersion, setPreviewingVersion] = useState(null);

  // PDF export (shared between desktop button and mobile overflow menu)
  const { isExporting, handleDownloadPDF } = useDownloadPDF({
    dashboardId,
    dashboardTitle: dashboard?.title,
    parameterValues,
    apiClient,
  });

  // Query for default dashboard (admin feature)
  const { data: defaultDashboardData } = useQuery({
    queryKey: ['default-dashboard'],
    queryFn: async () => {
      const response = await apiClient.get('/api/v1/workspaces/default-dashboard');
      return response.data;
    },
    enabled: isAdmin, // Only fetch for admins
    staleTime: 30000,
    retry: 1, // Only retry once for non-critical feature
    onError: (error) => {
      // Silently log - don't show toast for non-critical feature
      console.warn('Failed to fetch default dashboard:', error);
    },
  });

  const isDefaultDashboard = defaultDashboardData?.default_dashboard_id === dashboardId;

  const isUserDefaultDashboard = user?.extra_metadata?.default_dashboard_id === dashboardId;

  const setUserDefaultMutation = useMutation({
    mutationFn: async (newDefaultId) => {
      await apiClient.patch('/api/v1/users/me/preferences', {
        default_dashboard_id: newDefaultId,
      });
    },
    onSuccess: (_data, newDefaultId) => {
      refreshUser();
      queryClient.invalidateQueries(['default-dashboard']);
      toast.success(newDefaultId === null ? 'Personal default cleared' : 'Set as your default dashboard');
    },
    onError: (error) => {
      toast.error(error.response?.data?.detail || 'Failed to update default dashboard');
    },
  });

  // Mutation to set/clear default dashboard
  const setDefaultMutation = useMutation({
    mutationFn: async (newDefaultId) => {
      await apiClient.patch('/api/v1/workspaces/settings', {
        default_dashboard_id: newDefaultId
      });
    },
    onSuccess: () => {
      queryClient.invalidateQueries(['default-dashboard']);
      toast.success(isDefaultDashboard ? 'Default dashboard cleared' : 'Set as default dashboard');
    },
    onError: (error) => {
      toast.error(error.response?.data?.detail || 'Failed to update default dashboard');
    }
  });

  useEffect(() => {
    loadDashboard();
    // Reset initialization flag when dashboard ID changes
    hasInitializedParams.current = false;
  }, [dashboardId]);

  // Initialize parameter values ONCE when dashboard content first loads
  useEffect(() => {
    if (!dashboard?.content) return;
    if (hasInitializedParams.current) return; // Already initialized, don't reset

    // Parse all components from markdown to get chart-level parameters
    const { parameters, charts } = parseMarkdownChartML(dashboard.content);

    // Build initial parameter values from defaults
    // URL params are now handled by ChartML URLSyncParamsWrapper
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
    while ((match = codeBlockRegex.exec(dashboard.content)) !== null) {
      try {
        const content = match[1].trim();
        const parsed = JSON.parse(JSON.stringify(yaml.load(content))); // Use yaml from imports
        const components = Array.isArray(parsed) ? parsed : [parsed];

        // Check if this block contains any charts
        const blockCharts = components.filter(c => !c.type || c.type === 'chart');

        if (blockCharts.length > 0) {
          // This block contains charts - process their params
          blockCharts.forEach((chart, arrayIndex) => {
            const chartInFlatArray = charts[flatChartIndex];

            if (chartInFlatArray?.params && chartInFlatArray.params.length > 0) {
              // Use chartIndex (increments per block) not chartBlockIndex
              const chartScope = `chart_${chartIndex}_${arrayIndex}`;

              chartInFlatArray.params.forEach(param => {
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
    setParamsInitialized(true);
    hasInitializedParams.current = true; // Mark as initialized
  }, [dashboard, updateParameters]); // Only re-run when dashboard content changes, not when URL updates from user filter interactions

  const loadDashboard = async () => {
    setIsLoading(true);
    setError(null);
    try {
      const response = await apiClient.get(`/api/v1/dashboards/${dashboardId}`);
      setDashboard(response.data);
    } catch (err) {
      setError('Failed to load dashboard');
    } finally {
      setIsLoading(false);
    }
  };

  const handleEdit = () => {
    navigate(`/dashboard/${dashboardId}/edit`);
  };

  const handleBack = () => {
    navigate('/dashboards');
  };

  const handleRefreshAll = () => {
    window.dispatchEvent(new CustomEvent('dashboard-refresh-all'));
  };

  const handleToggleDefault = () => {
    // If currently default, clear it; otherwise set this dashboard as default
    const newDefaultId = isDefaultDashboard ? null : dashboardId;
    setDefaultMutation.mutate(newDefaultId);
  };

  const handleToggleUserDefault = () => {
    const newDefaultId = isUserDefaultDashboard ? null : dashboardId;
    setUserDefaultMutation.mutate(newDefaultId);
  };

  const handleTitleSave = async (newTitle) => {
    if (!newTitle.trim()) return;

    try {
      await apiClient.patch(`/api/v1/dashboards/${dashboardId}`, {
        title: newTitle.trim()
      });
      setDashboard({ ...dashboard, title: newTitle.trim() });

      // Invalidate dashboards query cache to update the list
      queryClient.invalidateQueries(['dashboards']);
    } catch (err) {
      setError('Failed to update dashboard title');
    }
  };

  // Handler for "Add to Dashboard" button on charts (enables copying charts between dashboards)
  const handleSaveChartToDashboard = useCallback((chartMarkdown) => {
    setDashboardModal({
      isOpen: true,
      messageContent: chartMarkdown,
    });
  }, []);

  // Handler for "Chart Info" button on charts
  const handleShowChartInfo = useCallback((spec) => {
    setChartInfoModal({ isOpen: true, spec });
  }, []);

  // Handler for "Ask about this chart" button - navigates to chat with chart context
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

  // Handler for version restore - reload dashboard
  const handleVersionRestored = useCallback(async () => {
    await loadDashboard();
    setPreviewingVersion(null);
    setHistoryPanelOpen(false);
  }, []);

  // Handler for version preview - show version content
  const handlePreviewVersion = useCallback((versionData) => {
    setPreviewingVersion(versionData);
  }, []);

  // Handler for SaveDashboardModal save action
  const handleSaveDashboard = async (mode, titleOrDashboardId, content) => {
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

  // Track when parameterValues state actually changes (now from context)
  useEffect(() => {
    const stateChangeTimestamp = Date.now();
  }, [parameterValues]);

  // URL updates now handled by ChartML URLSyncParamsWrapper
  // (removed old parameter system that wrote chart_0_0 params to URL)

  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center bg-muted">
        <img src="/kyomi_animated_logo.svg" alt="Loading" className="w-12 h-12" />
      </div>
    );
  }

  if (error || !dashboard) {
    return (
      <div className="flex h-full items-center justify-center bg-muted">
          <div className="text-center">
            <h2 className="text-2xl font-bold text-foreground mb-4">Dashboard Not Found</h2>
            <p className="text-muted-foreground mb-6">{error || 'The dashboard you are looking for does not exist.'}</p>
            <button
              onClick={handleBack}
              className="px-6 py-3 text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors"
            >
              Back to Dashboards
            </button>
          </div>
      </div>
    );
  }

  return (
      <div className="flex flex-col h-full bg-muted overflow-hidden" style={{flexDirection: 'column'}}>
        {/* Header */}
        <div className="h-16 bg-card border-b border-border px-4 md:px-6 flex-shrink-0 flex items-center justify-between">
          <div className="flex items-center gap-4 flex-1 min-w-0">
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={handleBack}
                    className="p-2 text-muted-foreground hover:text-foreground hover:bg-accent rounded-lg transition-colors flex-shrink-0"
                    aria-label="Back to dashboards"
                  >
                    <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
                    </svg>
                  </button>
                </TooltipTrigger>
                <TooltipContent>Back to dashboards</TooltipContent>
              </Tooltip>

              {/* Editable Title */}
              <InlineEditableTitle
                value={dashboard.title}
                onSave={handleTitleSave}
                placeholder="Untitled Dashboard"
                className="min-w-0"
              />
            </div>

          <div className="flex items-center gap-1 xl:gap-2 flex-shrink-0">
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={handleRefreshAll}
                  className="flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium text-foreground bg-card border border-border hover:bg-accent rounded-lg transition-colors"
                  aria-label="Refresh all charts"
                >
                  <ArrowPathIcon className="w-4 h-4 flex-shrink-0" />
                  <span className="hidden xl:inline whitespace-nowrap">Refresh All</span>
                </button>
              </TooltipTrigger>
              <TooltipContent>Refresh all charts</TooltipContent>
            </Tooltip>
            {/* Download PDF — desktop only */}
            {features.pdf_export && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={handleDownloadPDF}
                    disabled={isExporting}
                    className={`hidden md:flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium text-foreground bg-card border border-border hover:bg-accent rounded-lg transition-colors ${isExporting ? 'opacity-50 cursor-not-allowed' : ''}`}
                    aria-label="Download PDF"
                  >
                    <ArrowDownTrayIcon className={`w-4 h-4 flex-shrink-0 ${isExporting ? 'animate-pulse' : ''}`} />
                    <span className="hidden xl:inline whitespace-nowrap">{isExporting ? 'Exporting...' : 'Download PDF'}</span>
                  </button>
                </TooltipTrigger>
                <TooltipContent>Download dashboard as PDF</TooltipContent>
              </Tooltip>
            )}
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => setHistoryPanelOpen(!historyPanelOpen)}
                  className={`hidden md:flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
                    historyPanelOpen
                      ? 'bg-primary/10 text-primary border border-primary/20'
                      : 'text-foreground bg-card border border-border hover:bg-accent'
                  }`}
                  aria-label="Toggle version history"
                >
                  <ClockIcon className="w-4 h-4 flex-shrink-0" />
                  <span className="hidden xl:inline whitespace-nowrap">History</span>
                </button>
              </TooltipTrigger>
              <TooltipContent>{historyPanelOpen ? 'Close version history' : 'View version history'}</TooltipContent>
            </Tooltip>
            {/* Set as My Default button - all users */}
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={handleToggleUserDefault}
                  disabled={setUserDefaultMutation.isPending}
                  className={`hidden md:flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
                    isUserDefaultDashboard
                      ? 'bg-primary/10 text-primary border border-primary/20'
                      : 'text-foreground bg-card border border-border hover:bg-accent'
                  } ${setUserDefaultMutation.isPending ? 'opacity-50 cursor-not-allowed' : ''}`}
                  aria-label={isUserDefaultDashboard ? 'Remove as my default' : 'Set as my default'}
                >
                  <svg
                    className="w-4 h-4 flex-shrink-0"
                    fill={isUserDefaultDashboard ? 'currentColor' : 'none'}
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                  >
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                  </svg>
                  <span className="hidden xl:inline whitespace-nowrap">
                    {isUserDefaultDashboard ? 'My Default' : 'Set as My Default'}
                  </span>
                </button>
              </TooltipTrigger>
              <TooltipContent>{isUserDefaultDashboard ? 'Remove as your default dashboard' : 'Set as your default dashboard'}</TooltipContent>
            </Tooltip>
            {/* Set as Workspace Default button - admin only */}
            {isAdmin && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={handleToggleDefault}
                    disabled={setDefaultMutation.isPending}
                    className={`hidden md:flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
                      isDefaultDashboard
                        ? 'bg-primary/10 text-primary border border-primary/20'
                        : 'text-foreground bg-card border border-border hover:bg-accent'
                    } ${setDefaultMutation.isPending ? 'opacity-50 cursor-not-allowed' : ''}`}
                    aria-label={isDefaultDashboard ? 'Remove as workspace default' : 'Set as workspace default'}
                  >
                    <svg
                      className="w-4 h-4 flex-shrink-0"
                      fill={isDefaultDashboard ? 'currentColor' : 'none'}
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                    >
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6" />
                    </svg>
                    <span className="hidden xl:inline whitespace-nowrap">
                      {isDefaultDashboard ? 'Workspace Default' : 'Set Workspace Default'}
                    </span>
                  </button>
                </TooltipTrigger>
                <TooltipContent>
                  {isDefaultDashboard
                    ? 'Click to remove as workspace default'
                    : 'Set this dashboard as the workspace homepage'}
                </TooltipContent>
              </Tooltip>
            )}
            {/* Mobile overflow menu — secondary actions collapsed into "..." */}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  className="flex md:hidden items-center justify-center p-2 text-foreground bg-card border border-border hover:bg-accent rounded-lg transition-colors"
                  aria-label="More actions"
                >
                  <EllipsisVerticalIcon className="w-4 h-4" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                {features.pdf_export && (
                  <DropdownMenuItem onClick={handleDownloadPDF} disabled={isExporting}>
                    <ArrowDownTrayIcon className="w-4 h-4 mr-2" />
                    {isExporting ? 'Exporting...' : 'Download PDF'}
                  </DropdownMenuItem>
                )}
                <DropdownMenuItem onClick={() => setHistoryPanelOpen(!historyPanelOpen)}>
                  <ClockIcon className="w-4 h-4 mr-2" />
                  {historyPanelOpen ? 'Close History' : 'Version History'}
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onClick={handleToggleUserDefault}
                  disabled={setUserDefaultMutation.isPending}
                >
                  <svg className="w-4 h-4 mr-2" fill={isUserDefaultDashboard ? 'currentColor' : 'none'} stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                  </svg>
                  {isUserDefaultDashboard ? 'Remove My Default' : 'Set as My Default'}
                </DropdownMenuItem>
                {isAdmin && (
                  <>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem
                      onClick={handleToggleDefault}
                      disabled={setDefaultMutation.isPending}
                    >
                      <svg className="w-4 h-4 mr-2" fill={isDefaultDashboard ? 'currentColor' : 'none'} stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6" />
                      </svg>
                      {isDefaultDashboard ? 'Remove Workspace Default' : 'Set Workspace Default'}
                    </DropdownMenuItem>
                  </>
                )}
              </DropdownMenuContent>
            </DropdownMenu>

            {/* Edit Dashboard — always visible (primary action) */}
            <button
              onClick={handleEdit}
              className="flex items-center gap-2 px-2 xl:px-4 py-2 text-sm font-medium text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors"
            >
              <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
              </svg>
              <span className="hidden xl:inline whitespace-nowrap">Edit Dashboard</span>
            </button>
          </div>
        </div>

        {/* Content area with optional History panel */}
        <div className="flex-1 overflow-hidden flex">
          {/* Main content */}
          <div className="flex-1 overflow-y-auto p-4 md:p-6 bg-muted">
            <div className="bg-card rounded-lg border border-border shadow-sm min-h-full">
              {/* Preview banner */}
              {previewingVersion && (
                <div className="px-4 py-2 bg-warning border-b border-warning-border flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-warning-foreground">
                      Previewing Version {previewingVersion.version_number}
                    </span>
                    <span className="text-xs text-warning-foreground">
                      ({previewingVersion.change_summary || 'No summary'})
                    </span>
                  </div>
                  <span className="text-xs text-warning-foreground">Read-only</span>
                </div>
              )}

              <div className="p-4 md:p-6">
                {(previewingVersion?.content || dashboard.content)?.trim() ? (
                  paramsInitialized || previewingVersion ? (
                    <MarkdownRenderer
                      messageId={`dashboard-${dashboardId}${previewingVersion ? `-v${previewingVersion.version_number}` : ''}`}
                      sessionId="dashboard-view"
                      isStreaming={false}
                      showAddToDashboard={false}
                      dashboardConfig={{
                        id: dashboard.id || dashboardId,
                        datasets: dashboard.datasets || {}
                      }}
                      isChatBubble={false}
                      onSaveChartToDashboard={handleSaveChartToDashboard}
                      onShowChartInfo={handleShowChartInfo}
                      onAskAboutChart={handleAskAboutChart}
                    >
                      {previewingVersion ? previewingVersion.content : dashboard.content}
                    </MarkdownRenderer>
                  ) : (
                    <div className="flex h-64 items-center justify-center">
                      <img src="/kyomi_animated_logo.svg" alt="Loading" className="w-12 h-12" />
                    </div>
                  )
                ) : (
                  <div className="w-full text-center py-16">
                    <svg className="w-24 h-24 mx-auto text-muted-foreground mb-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
                    </svg>
                    <h3 className="text-xl font-semibold text-foreground mb-2">This dashboard is empty</h3>
                    <p className="text-muted-foreground mb-6">
                      Click "Edit Dashboard" to add content and charts
                    </p>
                    <button
                      onClick={handleEdit}
                      className="inline-flex items-center gap-2 px-6 py-3 text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors"
                    >
                      <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                      </svg>
                      Edit Dashboard
                    </button>
                  </div>
                )}
              </div>
            </div>
          </div>

          {/* History panel (slides alongside content like Copilot) */}
          <DashboardHistoryPanel
            isOpen={historyPanelOpen}
            onClose={() => setHistoryPanelOpen(false)}
            dashboardId={dashboardId}
            onPreviewVersion={handlePreviewVersion}
            onRestoreVersion={handleVersionRestored}
          />
        </div>

        {/* Footer with metadata */}
        <div className="bg-card border-t border-border px-4 md:px-6 py-3 flex-shrink-0">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <div className="flex items-center gap-4">
              <div className="flex items-center gap-1">
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
                </svg>
                Created {new Date(dashboard.created_at).toLocaleDateString()}
              </div>
              <div className="flex items-center gap-1">
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                </svg>
                Last updated {new Date(dashboard.updated_at).toLocaleDateString()}
              </div>
            </div>
          </div>
        </div>

        {/* Modals for chart actions */}
        <SaveDashboardModal
          isOpen={dashboardModal.isOpen}
          onClose={() => setDashboardModal({ isOpen: false, messageContent: '' })}
          onSave={handleSaveDashboard}
          messageContent={dashboardModal.messageContent}
          apiClient={apiClient}
        />
        <ChartInfoModal
          isOpen={chartInfoModal.isOpen}
          onClose={() => setChartInfoModal({ isOpen: false, spec: null })}
          spec={chartInfoModal.spec}
        />
      </div>
  );
};

/**
 * DashboardViewer - Wrapper that provides DashboardContext
 */
const DashboardViewer = () => {
  return (
    <DashboardProvider>
      <DashboardViewerContent />
    </DashboardProvider>
  );
};

export default DashboardViewer;
