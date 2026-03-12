// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useAuth } from '../context/AuthContext';
import { useCapabilities } from '../context/CapabilitiesContext';
import { useWebSocket } from '../context/WebSocketContext';
import { Card, CardHeader, CardTitle, CardContent, CardDescription } from '../components/ui/card';
import { Button } from '../components/ui/button';
import { Badge } from '../components/ui/badge';
import { StatusBadge } from '../components/ui/status-badge';
import { Switch } from '../components/ui/switch';
import { Tooltip, TooltipTrigger, TooltipContent } from '../components/ui/tooltip';
import { Alert, AlertDescription } from '../components/ui/alert';
import { toast } from '../lib/toast';
import Modal from '../components/Modal';
import ConfirmDialog from '../components/ConfirmDialog';
import useConfirm from '../hooks/useConfirm';
import WatchModal from '../components/watches/WatchModal';
import WatchAgentSidebar from '../components/watches/WatchAgentSidebar';
import AlertsHistory from '../components/watches/AlertsHistory';
import ExecutionLogViewer from '../components/watches/ExecutionLogViewer';
import { ChartBarIcon } from '@heroicons/react/24/outline';
import {
  Eye,
  Plus,
  Settings,
  Trash2,
  Play,
  Clock,
  Bell,
  AlertCircle,
  CheckCircle,
  XCircle,
  ArrowUpRight,
  FileText,
  Sparkles,
  Loader2,
} from 'lucide-react';
import { Spinner, SpinnerFullPage, SpinnerPage } from '../components/ui/spinner';
import { describeCron } from '../utils/cronUtils';

/**
 * WatchesPage - Kyomi Watch management
 *
 * Proactive data monitoring with AI-powered alerts.
 * Premium feature: Pro and Team plans only.
 */
export default function WatchesPage() {
  const navigate = useNavigate();
  const { view } = useParams();
  const [searchParams] = useSearchParams();
  const { apiClient } = useAuth();
  const queryClient = useQueryClient();
  const capabilities = useCapabilities();
  const { subscribe } = useWebSocket();
  const { isOpen, dialogProps, confirm } = useConfirm();

  // Modal state - only used for editing now
  const [showWatchModal, setShowWatchModal] = useState(false);
  const [editingWatch, setEditingWatch] = useState(null);

  // Track which alert should be expanded (from URL param)
  const [expandedAlertId, setExpandedAlertId] = useState(null);

  // Execution log modal state
  const [showExecutionLog, setShowExecutionLog] = useState(false);
  const [viewingWatchId, setViewingWatchId] = useState(null);
  const [selectedExecutionId, setSelectedExecutionId] = useState(null);

  // AI Sidebar state
  const [showAgentSidebar, setShowAgentSidebar] = useState(false);
  const [agentEditingWatch, setAgentEditingWatch] = useState(null);

  // View from URL - 'config' or 'alerts' (default to alerts)
  const activeView = view === 'config' ? 'watches' : 'alerts';

  // Handle alert parameter from URL (e.g., ?alert=42)
  useEffect(() => {
    const alertId = searchParams.get('alert');
    if (alertId) {
      setExpandedAlertId(parseInt(alertId, 10));
    }
  }, [searchParams]);

  // Check if user has Kyomi Watch capability and AI is available
  const capabilitiesLoading = capabilities?.loading ?? true;
  const hasWatchCapability = capabilities?.kyomiWatchEnabled ?? false;
  const aiEnabled = capabilities?.aiEnabled ?? false;
  const creditsExhausted = capabilities?.creditsExhausted ?? false;

  // Fetch watches (all hooks must be called unconditionally)
  const {
    data: watches,
    isLoading: watchesLoading,
    error: watchesError,
  } = useQuery({
    queryKey: ['watches'],
    queryFn: async () => {
      const response = await apiClient.get('/api/v1/watches');
      return response.data;
    },
    enabled: hasWatchCapability && !capabilitiesLoading,
  });

  // Fetch execution list for viewing (without trace data for efficiency)
  const {
    data: executions,
    isLoading: executionsLoading,
  } = useQuery({
    queryKey: ['watch-executions', viewingWatchId],
    queryFn: async () => {
      const response = await apiClient.get(`/api/v1/watches/${viewingWatchId}/executions`);
      return response.data;
    },
    enabled: !!viewingWatchId && showExecutionLog && !capabilitiesLoading,
  });

  // Determine which execution to fetch (selected or most recent)
  const executionIdToFetch = selectedExecutionId || executions?.[0]?.id;

  // Fetch selected execution with full trace
  const {
    data: selectedExecution,
    isLoading: executionLoading,
  } = useQuery({
    queryKey: ['watch-execution', viewingWatchId, executionIdToFetch],
    queryFn: async () => {
      const response = await apiClient.get(`/api/v1/watches/${viewingWatchId}/executions/${executionIdToFetch}`);
      return response.data;
    },
    enabled: !!viewingWatchId && showExecutionLog && !!executionIdToFetch && !capabilitiesLoading,
  });

  // Refetch watches when navigating to this page
  useEffect(() => {
    if (!hasWatchCapability || capabilitiesLoading) {
      return;
    }

    // Refetch when page mounts or route changes (e.g., navigating to /watches/config)
    queryClient.invalidateQueries(['watches']);
  }, [view, queryClient, hasWatchCapability, capabilitiesLoading]);

  // Subscribe to watch state updates via WebSocket
  useEffect(() => {
    if (!hasWatchCapability || capabilitiesLoading) {
      return;
    }

    const unsubscribe = subscribe('watch_state_update', (message) => {
      // Watch execution state changed - invalidate queries to refetch
      queryClient.invalidateQueries(['watches']);

      // Also invalidate executions if viewing a log for this watch
      if (viewingWatchId && message.data?.watch_id === viewingWatchId) {
        queryClient.invalidateQueries(['watch-executions', viewingWatchId]);
      }
    });

    return unsubscribe;
  }, [subscribe, queryClient, hasWatchCapability, capabilitiesLoading, viewingWatchId]);

  // Toggle watch mutation
  const toggleMutation = useMutation({
    mutationFn: async (watchId) => {
      const response = await apiClient.post(`/api/v1/watches/${watchId}/toggle`);
      return response.data;
    },
    onSuccess: () => {
      queryClient.invalidateQueries(['watches']);
    },
    onError: (error) => {
      toast.error(error.response?.data?.detail || 'Failed to toggle watch');
    },
  });

  // Run watch now mutation
  const runNowMutation = useMutation({
    mutationFn: async (watchId) => {
      const response = await apiClient.post(`/api/v1/watches/${watchId}/run`);
      return response.data;
    },
    onSuccess: () => {
      toast.success('Watch execution started');
      queryClient.invalidateQueries(['watches']);
    },
    onError: (error) => {
      toast.error(error.response?.data?.detail || 'Failed to run watch');
    },
  });

  // Delete watch mutation
  const deleteMutation = useMutation({
    mutationFn: async (watchId) => {
      await apiClient.delete(`/api/v1/watches/${watchId}`);
      return watchId;
    },
    onSuccess: () => {
      toast.success('Watch deleted');
      queryClient.invalidateQueries(['watches']);
    },
    onError: (error) => {
      toast.error(error.response?.data?.detail || 'Failed to delete watch');
    },
  });

  // Show loader while capabilities are loading (must be after all hooks)
  if (capabilitiesLoading) {
    return <SpinnerFullPage />;
  }

  const handleDeleteWatch = async (watch) => {
    const confirmed = await confirm({
      title: 'Delete Watch',
      message: `Are you sure you want to delete "${watch.name}"? This action cannot be undone.`,
      confirmText: 'Delete',
      variant: 'destructive',
    });
    if (confirmed) {
      deleteMutation.mutate(watch.watch_id);
    }
  };

  const handleEditWatch = (watch) => {
    setEditingWatch(watch);
    setShowWatchModal(true);
  };

  const handleCreateWatch = () => {
    // Open AI sidebar for creating a watch
    setAgentEditingWatch(null);
    setShowAgentSidebar(true);
  };

  const handleEditWithAI = (watch) => {
    // Open AI sidebar for editing a watch
    setAgentEditingWatch(watch);
    setShowAgentSidebar(true);
  };

  const handleAgentSidebarClose = () => {
    setShowAgentSidebar(false);
    setAgentEditingWatch(null);
  };

  const handleWatchCreatedByAgent = () => {
    // Watch was created/updated by the agent
    queryClient.invalidateQueries(['watches']);
    toast.success(agentEditingWatch ? 'Watch updated' : 'Watch created');
  };

  const handleWatchModalClose = () => {
    setShowWatchModal(false);
    setEditingWatch(null);
  };

  const handleWatchSaved = () => {
    queryClient.invalidateQueries(['watches']);
    setShowWatchModal(false);
    setEditingWatch(null);
  };

  const handleViewExecutionLog = (watch) => {
    setViewingWatchId(watch.watch_id);
    setShowExecutionLog(true);
  };

  const handleExecutionLogClose = () => {
    setShowExecutionLog(false);
    setViewingWatchId(null);
    setSelectedExecutionId(null);
  };

  const handleSelectExecution = (executionId) => {
    setSelectedExecutionId(executionId);
  };

  // Status badge component using design system's StatusBadge
  const WatchStatusBadge = ({ status }) => {
    const statusConfig = {
      success: { variant: 'success', icon: CheckCircle, label: 'Success' },
      no_alert: { variant: 'default', icon: CheckCircle, label: 'No Alert' },
      error: { variant: 'error', icon: XCircle, label: 'Error' },
      running: { variant: 'info', icon: Loader2, label: 'Running' },
    };
    const config = statusConfig[status] || statusConfig.no_alert;
    const Icon = config.icon;

    return (
      <StatusBadge variant={config.variant} className="gap-1">
        <Icon className={`h-3 w-3 ${status === 'running' ? 'animate-spin' : ''}`} />
        {config.label}
      </StatusBadge>
    );
  };

  // Format date for display
  const formatDate = (dateStr) => {
    if (!dateStr) return 'Never';
    const date = new Date(dateStr);
    return date.toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  };

  // Upgrade prompt for non-Pro users
  if (!hasWatchCapability) {
    return (
      <div className="h-full flex flex-col bg-background">
        <div className="flex items-center justify-between px-6 py-4 border-b border-border">
          <div className="flex items-center gap-3">
            <Eye className="h-6 w-6 text-primary" />
            <h1 className="text-xl font-semibold text-foreground">Kyomi Watch</h1>
          </div>
        </div>

        <div className="flex-1 flex items-center justify-center p-6">
          <Card className="max-w-lg">
            <CardHeader className="text-center">
              <div className="mx-auto mb-4 h-16 w-16 rounded-full bg-primary/10 flex items-center justify-center">
                <Eye className="h-8 w-8 text-primary" />
              </div>
              <CardTitle className="text-2xl">Proactive Data Monitoring</CardTitle>
              <CardDescription className="text-base">
                Let Kyomi watch your data and alert you when something noteworthy happens.
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <ul className="space-y-3 text-sm text-muted-foreground">
                <li className="flex items-start gap-2">
                  <CheckCircle className="h-5 w-5 text-success-foreground mt-0.5 shrink-0" />
                  <span>Monitor data with plain English instructions</span>
                </li>
                <li className="flex items-start gap-2">
                  <CheckCircle className="h-5 w-5 text-success-foreground mt-0.5 shrink-0" />
                  <span>Get alerts when metrics change or anomalies occur</span>
                </li>
                <li className="flex items-start gap-2">
                  <CheckCircle className="h-5 w-5 text-success-foreground mt-0.5 shrink-0" />
                  <span>Schedule checks hourly, daily, or custom intervals</span>
                </li>
              </ul>
              <div className="pt-4">
                <Button className="w-full" onClick={() => window.location.href = '/settings/billing'}>
                  Upgrade to Pro
                  <ArrowUpRight className="ml-2 h-4 w-4" />
                </Button>
                <p className="text-xs text-center text-muted-foreground mt-2">
                  Kyomi Watch is available on Pro and Team plans
                </p>
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    );
  }

  return (
    <div className="h-full flex bg-muted">
      {/* Main content area */}
      <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
      {/* Header */}
      <div className="h-16 bg-card border-b border-border px-6 flex-shrink-0 flex items-center justify-between">
        <div className="flex items-center gap-3">
          <Eye className="h-6 w-6 text-primary hidden sm:block" />
          <h1 className="text-lg sm:text-xl font-semibold text-foreground">Kyomi Watch</h1>
        </div>
        <div className="flex items-center gap-2">
          {/* View toggle - Alerts first since it's the inbox */}
          <div className="flex items-center rounded-lg bg-muted p-1">
            <button
              onClick={() => navigate('/watches/alerts')}
              className={`flex items-center gap-1.5 px-2 sm:px-3 py-1.5 text-sm rounded-md transition-colors ${
                activeView === 'alerts'
                  ? 'bg-background text-foreground shadow-sm'
                  : 'text-muted-foreground hover:text-foreground'
              }`}
            >
              <Bell className="h-4 w-4" />
              <span className="hidden sm:inline">Alerts</span>
            </button>
            <button
              onClick={() => navigate('/watches/config')}
              className={`flex items-center gap-1.5 px-2 sm:px-3 py-1.5 text-sm rounded-md transition-colors ${
                activeView === 'watches'
                  ? 'bg-background text-foreground shadow-sm'
                  : 'text-muted-foreground hover:text-foreground'
              }`}
            >
              <Eye className="h-4 w-4" />
              <span className="hidden sm:inline">Watches</span>
            </button>
          </div>
          <Tooltip>
            <TooltipTrigger asChild>
              <span>
                <Button onClick={handleCreateWatch} disabled={!aiEnabled}>
                  <Plus className="h-4 w-4" />
                  <span className="hidden sm:inline">Create Watch</span>
                </Button>
              </span>
            </TooltipTrigger>
            {!aiEnabled && (
              <TooltipContent>
                {creditsExhausted
                  ? 'AI budget exhausted for this billing period'
                  : 'AI features are not available'}
              </TooltipContent>
            )}
          </Tooltip>
        </div>
      </div>

      {/* Budget exhausted warning */}
      {creditsExhausted && (
        <div className="px-6 pt-4">
          <Alert variant="warning">
            <AlertCircle className="h-4 w-4" />
            <AlertDescription>
              Your AI budget is exhausted for this billing period. Existing watches will not run until your budget resets.
              <Button variant="link" className="h-auto p-0 ml-1" onClick={() => navigate('/settings/billing')}>
                Upgrade your plan
              </Button>
            </AlertDescription>
          </Alert>
        </div>
      )}

      {/* Content */}
      <div className="flex-1 overflow-auto p-3 sm:p-6 @container">
        {activeView === 'watches' ? (
          // Watches list view
          <>
            {watchesLoading || watches === undefined ? (
              <SpinnerPage />
            ) : watchesError ? (
              <Alert variant="destructive">
                <AlertCircle className="h-4 w-4" />
                <AlertDescription>
                  Failed to load watches: {watchesError.message}
                </AlertDescription>
              </Alert>
            ) : watches?.length === 0 ? (
              <div className="flex flex-col items-center justify-center py-12 text-center">
                <div className="h-16 w-16 rounded-full bg-muted flex items-center justify-center mb-4">
                  <Eye className="h-8 w-8 text-muted-foreground" />
                </div>
                <h3 className="text-lg font-medium text-foreground mb-2">No watches yet</h3>
                <p className="text-muted-foreground mb-4 max-w-md">
                  {creditsExhausted
                    ? 'Your AI budget is exhausted. Wait for it to reset or upgrade your plan to create watches.'
                    : 'Create your first watch to start monitoring your data proactively.'}
                </p>
                <Button onClick={handleCreateWatch} disabled={!aiEnabled}>
                  <Plus className="h-4 w-4 mr-2" />
                  Create Watch
                </Button>
              </div>
            ) : (
              <div className="grid gap-4 @2xl:grid-cols-2 @4xl:grid-cols-3">
                {watches.map((watch) => (
                  <Card key={watch.watch_id} className="relative">
                    <CardHeader className="pb-2">
                      <div className="flex items-start justify-between">
                        <div className="flex-1 min-w-0">
                          <CardTitle className="text-base truncate flex items-center gap-2">
                            {watch.mode === 'report' ? (
                              <ChartBarIcon className="h-4 w-4 shrink-0 text-muted-foreground" />
                            ) : (
                              <Bell className="h-4 w-4 shrink-0 text-muted-foreground" />
                            )}
                            {watch.name}
                          </CardTitle>
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <p className="text-sm text-muted-foreground mt-1 line-clamp-2 cursor-help">
                                {watch.prompt}
                              </p>
                            </TooltipTrigger>
                            <TooltipContent side="bottom" className="max-w-sm">
                              <p className="text-sm whitespace-pre-wrap">{watch.prompt}</p>
                            </TooltipContent>
                          </Tooltip>
                        </div>
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <div>
                              <Switch
                                checked={watch.enabled}
                                onCheckedChange={() => toggleMutation.mutate(watch.watch_id)}
                                disabled={toggleMutation.isPending}
                              />
                            </div>
                          </TooltipTrigger>
                          <TooltipContent>
                            {watch.enabled ? 'Disable watch' : 'Enable watch'}
                          </TooltipContent>
                        </Tooltip>
                      </div>
                    </CardHeader>
                    <CardContent className="space-y-3">
                      {/* Schedule */}
                      <div className="flex items-center text-sm text-muted-foreground">
                        <Clock className="h-4 w-4 mr-2" />
                        {describeCron(watch.schedule).description}
                      </div>

                      {/* Status & last run */}
                      <div className="flex items-center justify-between">
                        {watch.last_run_status && (
                          <WatchStatusBadge status={watch.last_run_status} />
                        )}
                        <span className="text-xs text-muted-foreground">
                          {watch.last_run_at ? `Last: ${formatDate(watch.last_run_at)}` : 'Not run yet'}
                        </span>
                      </div>

                      {/* Next run */}
                      {watch.enabled && watch.next_run_at && (
                        <div className="text-xs text-muted-foreground">
                          Next run: {formatDate(watch.next_run_at)}
                        </div>
                      )}

                      {/* Actions */}
                      <div className="flex items-center gap-2 pt-2 border-t border-border">
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() => runNowMutation.mutate(watch.watch_id)}
                              disabled={runNowMutation.isPending}
                            >
                              <Play className="h-4 w-4" />
                            </Button>
                          </TooltipTrigger>
                          <TooltipContent>Run now</TooltipContent>
                        </Tooltip>
                        {watch.last_run_at && (
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <Button
                                variant="ghost"
                                size="sm"
                                onClick={() => handleViewExecutionLog(watch)}
                              >
                                <FileText className="h-4 w-4" />
                              </Button>
                            </TooltipTrigger>
                            <TooltipContent>View execution history</TooltipContent>
                          </Tooltip>
                        )}
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <span>
                              <Button
                                variant="ghost"
                                size="sm"
                                onClick={() => handleEditWithAI(watch)}
                                disabled={!aiEnabled}
                              >
                                <Sparkles className="h-4 w-4" />
                              </Button>
                            </span>
                          </TooltipTrigger>
                          <TooltipContent>
                            {!aiEnabled
                              ? (creditsExhausted ? 'AI budget exhausted' : 'AI not available')
                              : 'Edit with AI'}
                          </TooltipContent>
                        </Tooltip>
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() => handleEditWatch(watch)}
                            >
                              <Settings className="h-4 w-4" />
                            </Button>
                          </TooltipTrigger>
                          <TooltipContent>Quick edit</TooltipContent>
                        </Tooltip>
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() => handleDeleteWatch(watch)}
                              className="text-destructive hover:text-destructive"
                            >
                              <Trash2 className="h-4 w-4" />
                            </Button>
                          </TooltipTrigger>
                          <TooltipContent>Delete watch</TooltipContent>
                        </Tooltip>
                      </div>
                    </CardContent>
                  </Card>
                ))}
              </div>
            )}
          </>
        ) : (
          // Alerts history view
          <AlertsHistory expandedAlertId={expandedAlertId} />
        )}
      </div>

      {/* Watch Modal - only used for editing existing watches */}
      {showWatchModal && editingWatch && (
        <WatchModal
          watch={editingWatch}
          onClose={handleWatchModalClose}
          onSaved={handleWatchSaved}
        />
      )}

      {/* Execution Log Modal */}
      {showExecutionLog && (
        <Modal
          show={true}
          onClose={handleExecutionLogClose}
          title={
            <div className="flex items-center gap-2">
              <FileText className="h-5 w-5 text-primary" />
              <span>Execution History</span>
              {viewingWatchId && watches && (
                <span className="text-muted-foreground font-normal">
                  - {watches.find(w => w.watch_id === viewingWatchId)?.name}
                </span>
              )}
            </div>
          }
          size="xl"
        >
          <ExecutionLogViewer
            executions={executions || []}
            selectedExecution={selectedExecution}
            onSelectExecution={handleSelectExecution}
            isLoading={executionsLoading || executionLoading}
            watchPrompt={watches?.find(w => w.watch_id === viewingWatchId)?.prompt}
          />
        </Modal>
      )}

      {/* Confirm Dialog */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
      </div>

      {/* AI Agent Sidebar */}
      <WatchAgentSidebar
        isOpen={showAgentSidebar}
        onClose={handleAgentSidebarClose}
        editingWatch={agentEditingWatch}
        onWatchCreated={handleWatchCreatedByAgent}
      />
    </div>
  );
}
