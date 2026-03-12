// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useCallback, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useAuth } from '../../context/AuthContext';
import { MarkdownRenderer } from '../MarkdownRenderer';
import { Badge } from '../ui/badge';
import { Button } from '../ui/button';
import { Label } from '../ui/label';
import { Switch } from '../ui/switch';
import { Checkbox } from '../ui/checkbox';
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '../ui/select';
import { Alert, AlertDescription } from '../ui/alert';
import SaveDashboardModal from '../SaveDashboardModal';
import ChartInfoModal from '../ChartInfoModal';
import ConfirmDialog from '../ConfirmDialog';
import useConfirm from '../../hooks/useConfirm';
import {
  Bell,
  AlertCircle,
  ChevronDown,
  ChevronUp,
  Clock,
  Trash2,
  Undo2,
  MailOpen,
  Mail,
  MoreVertical,
  X,
} from 'lucide-react';
import { ChartBarIcon } from '@heroicons/react/24/outline';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '../ui/dropdown-menu';
import { Spinner } from '../ui/spinner';
import { ChatBubbleLeftRightIcon } from '@heroicons/react/24/outline';

/**
 * AlertsHistory - View all watch alerts
 *
 * Shows execution history for watches that triggered alerts.
 * Filterable by watch and paginated.
 * Gmail-style: checkboxes always visible, toolbar swaps between
 * filters and bulk actions based on selection state.
 *
 * @param {number} expandedAlertId - Optional alert ID to expand (from URL parameter)
 */
export default function AlertsHistory({ expandedAlertId }) {
  const { apiClient } = useAuth();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [selectedWatchId, setSelectedWatchId] = useState('');
  const [expandedAlerts, setExpandedAlerts] = useState(new Set());
  const [showDeleted, setShowDeleted] = useState(false);
  const [page, setPage] = useState(0);
  const limit = 20;

  // Selection state (no explicit "select mode" — selection is implicit)
  const [selectedAlerts, setSelectedAlerts] = useState(new Set());

  // Confirm dialog
  const { isOpen: isConfirmOpen, dialogProps, confirm } = useConfirm();

  // Chart modal state
  const [dashboardModal, setDashboardModal] = useState({ isOpen: false, messageContent: '' });
  const [chartInfoModal, setChartInfoModal] = useState({ isOpen: false, spec: null });

  // Fetch watches for filter dropdown
  const { data: watches } = useQuery({
    queryKey: ['watches'],
    queryFn: async () => {
      const response = await apiClient.get('/api/v1/watches');
      return response.data;
    },
  });

  // Fetch alerts history
  const {
    data: alertsData,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['alerts', selectedWatchId, page, showDeleted],
    queryFn: async () => {
      const params = new URLSearchParams({
        limit: limit.toString(),
        offset: (page * limit).toString(),
        include_deleted: showDeleted.toString(),
      });
      if (selectedWatchId) {
        params.set('watch_id', selectedWatchId);
      }
      const response = await apiClient.get(`/api/v1/watches/alerts?${params}`);
      return response.data;
    },
  });

  // Mark alert as read mutation (called automatically when expanding)
  const markReadMutation = useMutation({
    mutationFn: async (executionId) => {
      const response = await apiClient.post(`/api/v1/watches/alerts/${executionId}/read`);
      return response.data;
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['alerts'] });
      queryClient.invalidateQueries({ queryKey: ['unread-alerts-count'] });
    },
  });

  // Mark alert as unread mutation
  const markUnreadMutation = useMutation({
    mutationFn: async (executionId) => {
      const response = await apiClient.post(`/api/v1/watches/alerts/${executionId}/unread`);
      return response.data;
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['alerts'] });
      queryClient.invalidateQueries({ queryKey: ['unread-alerts-count'] });
    },
  });

  // Delete alert mutation (soft delete)
  const deleteMutation = useMutation({
    mutationFn: async (executionId) => {
      const response = await apiClient.post(`/api/v1/watches/alerts/${executionId}/delete`);
      return response.data;
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['alerts'] });
      queryClient.invalidateQueries({ queryKey: ['unread-alerts-count'] });
    },
  });

  // Restore deleted alert mutation
  const restoreMutation = useMutation({
    mutationFn: async (executionId) => {
      const response = await apiClient.post(`/api/v1/watches/alerts/${executionId}/restore`);
      return response.data;
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['alerts'] });
      queryClient.invalidateQueries({ queryKey: ['unread-alerts-count'] });
    },
  });

  // Continue alert in chat mutation
  const continueChatMutation = useMutation({
    mutationFn: async (executionId) => {
      const response = await apiClient.post(`/api/v1/watches/alerts/${executionId}/continue-chat`);
      return response.data;
    },
    onSuccess: (data) => {
      navigate(`/chat/${data.session_id}`);
    },
  });

  // Bulk action mutations
  const invalidateBulkQueries = () => {
    queryClient.invalidateQueries({ queryKey: ['alerts'] });
    queryClient.invalidateQueries({ queryKey: ['unread-alerts-count'] });
  };

  const bulkDeleteMutation = useMutation({
    mutationFn: async (executionIds) => {
      const response = await apiClient.post('/api/v1/watches/alerts/bulk-delete', {
        execution_ids: executionIds,
      });
      return response.data;
    },
    onSuccess: () => {
      invalidateBulkQueries();
      setSelectedAlerts(new Set());
    },
  });

  const bulkMarkReadMutation = useMutation({
    mutationFn: async (executionIds) => {
      const response = await apiClient.post('/api/v1/watches/alerts/bulk-read', {
        execution_ids: executionIds,
      });
      return response.data;
    },
    onSuccess: () => {
      invalidateBulkQueries();
      setSelectedAlerts(new Set());
    },
  });

  const bulkMarkUnreadMutation = useMutation({
    mutationFn: async (executionIds) => {
      const response = await apiClient.post('/api/v1/watches/alerts/bulk-unread', {
        execution_ids: executionIds,
      });
      return response.data;
    },
    onSuccess: () => {
      invalidateBulkQueries();
      setSelectedAlerts(new Set());
    },
  });

  const isBulkActionPending =
    bulkDeleteMutation.isPending || bulkMarkReadMutation.isPending || bulkMarkUnreadMutation.isPending;

  // Derived state
  const hasSelection = selectedAlerts.size > 0;

  // Clear selection when page or filters change
  useEffect(() => {
    setSelectedAlerts(new Set());
  }, [page, selectedWatchId, showDeleted]);

  // Expand alert if expandedAlertId is provided (e.g., from Slack link)
  useEffect(() => {
    if (expandedAlertId && alertsData?.alerts) {
      setExpandedAlerts(new Set([expandedAlertId]));
      // Auto-mark as read when opening from external link
      const alert = alertsData.alerts.find((a) => a.id === expandedAlertId);
      if (alert && !alert.read_at && !alert.deleted_at) {
        markReadMutation.mutate(expandedAlertId);
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [expandedAlertId, alertsData?.alerts]);

  const toggleExpanded = (alertId) => {
    const newExpanded = new Set(expandedAlerts);
    if (newExpanded.has(alertId)) {
      newExpanded.delete(alertId);
    } else {
      newExpanded.add(alertId);
      // Auto-mark as read when expanding an unread alert
      const alert = alertsData?.alerts?.find((a) => a.id === alertId);
      if (alert && !alert.read_at && !alert.deleted_at) {
        markReadMutation.mutate(alertId);
      }
    }
    setExpandedAlerts(newExpanded);
  };

  // Selection helpers
  const selectableAlerts = (alertsData?.alerts || []).filter((a) => !a.deleted_at);
  const isAllSelected = selectableAlerts.length > 0 && selectedAlerts.size === selectableAlerts.length;
  const isIndeterminate = selectedAlerts.size > 0 && selectedAlerts.size < selectableAlerts.length;

  const toggleAlertSelection = (alertId) => {
    const newSelected = new Set(selectedAlerts);
    if (newSelected.has(alertId)) {
      newSelected.delete(alertId);
    } else {
      newSelected.add(alertId);
    }
    setSelectedAlerts(newSelected);
  };

  const toggleSelectAll = () => {
    if (selectedAlerts.size === selectableAlerts.length) {
      setSelectedAlerts(new Set());
    } else {
      setSelectedAlerts(new Set(selectableAlerts.map((a) => a.id)));
    }
  };

  const handleBulkDelete = async () => {
    if (selectedAlerts.size === 0) return;

    const confirmed = await confirm({
      title: `Delete ${selectedAlerts.size} alert${selectedAlerts.size !== 1 ? 's' : ''}?`,
      message: 'Deleted alerts will be hidden from your alerts list. You can restore them by enabling "Show deleted".',
      confirmText: 'Delete',
      variant: 'destructive',
    });

    if (confirmed) {
      bulkDeleteMutation.mutate([...selectedAlerts]);
    }
  };

  const handleBulkMarkRead = () => {
    if (selectedAlerts.size > 0) {
      bulkMarkReadMutation.mutate([...selectedAlerts]);
    }
  };

  const handleBulkMarkUnread = () => {
    if (selectedAlerts.size > 0) {
      bulkMarkUnreadMutation.mutate([...selectedAlerts]);
    }
  };

  // Handler for saving chart to dashboard
  const handleSaveChartToDashboard = useCallback((chartMarkdown) => {
    setDashboardModal({
      isOpen: true,
      messageContent: chartMarkdown,
    });
  }, []);

  // Handler for showing chart info
  const handleShowChartInfo = useCallback((spec) => {
    setChartInfoModal({ isOpen: true, spec });
  }, []);

  // Handler for "Ask about this chart" - navigate to chat with chart context
  const handleAskAboutChart = useCallback((chartMarkdown, spec) => {
    navigate('/chat', {
      state: {
        exploreChart: true,
        chartMarkdown,
        chartTitle: spec?.style?.title || spec?.title || 'Chart'
      }
    });
  }, [navigate]);

  // Format date for display
  const formatDate = (dateStr) => {
    if (!dateStr) return '';
    const date = new Date(dateStr);
    return date.toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      year: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  };

  // Get watch name - prefer stored name, fall back to lookup
  const getWatchName = (alert) => {
    if (alert.watch_name) {
      return alert.watch_name;
    }
    if (alert.watch_id) {
      const watch = watches?.find((w) => w.watch_id === alert.watch_id);
      if (watch) return watch.name;
    }
    return 'Deleted Watch';
  };

  // Get the selected watch name for display
  const getSelectedWatchLabel = () => {
    if (!selectedWatchId) return 'All watches';
    const watch = watches?.find((w) => w.watch_id === selectedWatchId);
    return watch?.name || 'All watches';
  };

  if (isLoading || alertsData === undefined) {
    return (
      <div className="flex items-center justify-center py-12">
        <Spinner size="lg" className="text-muted-foreground" />
      </div>
    );
  }

  if (error) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="h-4 w-4" />
        <AlertDescription>
          Failed to load alerts: {error.message}
        </AlertDescription>
      </Alert>
    );
  }

  const alerts = alertsData?.alerts || [];
  const total = alertsData?.total || 0;
  const hasMore = (page + 1) * limit < total;
  const hasPrevious = page > 0;

  return (
    <div className="space-y-4">
      {/* Toolbar — swaps between filters and bulk actions based on selection */}
      <div className="flex items-center gap-3 flex-wrap min-h-10">
        {/* Select-all checkbox — pl-4 aligns with per-card checkboxes below */}
        {selectableAlerts.length > 0 && (
          <div className="pl-4">
            <Checkbox
              checked={isAllSelected}
              indeterminate={isIndeterminate}
              onCheckedChange={toggleSelectAll}
            />
          </div>
        )}

        {hasSelection ? (
          /* Selection active: show count + bulk actions + cancel */
          <>
            <span className="text-sm font-medium text-foreground whitespace-nowrap">
              {selectedAlerts.size} selected
            </span>
            <div className="flex items-center gap-1">
              <Button
                variant="ghost"
                size="sm"
                onClick={handleBulkMarkRead}
                disabled={isBulkActionPending}
                title="Mark as read"
              >
                <MailOpen className="h-4 w-4 sm:mr-1.5" />
                <span className="hidden sm:inline">Mark Read</span>
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={handleBulkMarkUnread}
                disabled={isBulkActionPending}
                title="Mark as unread"
              >
                <Mail className="h-4 w-4 sm:mr-1.5" />
                <span className="hidden sm:inline">Mark Unread</span>
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={handleBulkDelete}
                disabled={isBulkActionPending}
                className="text-destructive hover:text-destructive"
                title="Delete selected"
              >
                <Trash2 className="h-4 w-4 sm:mr-1.5" />
                <span className="hidden sm:inline">Delete</span>
              </Button>
            </div>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setSelectedAlerts(new Set())}
              className="ml-auto"
            >
              <X className="h-4 w-4 sm:mr-1.5" />
              <span className="hidden sm:inline">Cancel</span>
            </Button>
          </>
        ) : (
          /* No selection: show filters */
          <>
            <div className="flex items-center gap-2 min-w-0 flex-1 sm:flex-none">
              <Label className="text-muted-foreground hidden sm:inline">Filter:</Label>
              <Select
                value={selectedWatchId}
                onValueChange={(value) => {
                  setSelectedWatchId(value === '_all' ? '' : value);
                  setPage(0);
                }}
              >
                <SelectTrigger className="w-full sm:w-[200px] bg-card">
                  <SelectValue placeholder="All watches">{getSelectedWatchLabel()}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="_all">All watches</SelectItem>
                  {watches?.map((watch) => (
                    <SelectItem key={watch.watch_id} value={watch.watch_id}>
                      {watch.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <Badge variant="secondary" className="hidden sm:inline-flex">
              {total} alert{total !== 1 ? 's' : ''}
            </Badge>
            <div className="flex items-center gap-2 ml-auto">
              <Switch
                id="show-deleted"
                checked={showDeleted}
                onCheckedChange={(checked) => {
                  setShowDeleted(checked);
                  setPage(0);
                }}
              />
              <Label htmlFor="show-deleted" className="text-sm text-muted-foreground cursor-pointer">
                Show deleted
              </Label>
            </div>
          </>
        )}
      </div>

      {/* Alerts list */}
      {alerts.length === 0 ? (
        <div className="flex flex-col items-center justify-center py-12 text-center">
          <div className="h-16 w-16 rounded-full bg-muted flex items-center justify-center mb-4">
            <Bell className="h-8 w-8 text-muted-foreground" />
          </div>
          <h3 className="text-lg font-medium text-foreground mb-2">No alerts yet</h3>
          <p className="text-muted-foreground max-w-md">
            When your watches detect something noteworthy, alerts will appear here.
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          {alerts.map((alert) => {
            const isDeleted = !!alert.deleted_at;
            const isUnread = !alert.read_at && !alert.deleted_at;
            const isSelected = selectedAlerts.has(alert.id);
            return (
              <div
                key={alert.id}
                className={`rounded-lg border overflow-hidden ${
                  isDeleted
                    ? 'opacity-60 border-border bg-muted/30'
                    : isUnread
                      ? 'border-l-4 border-l-primary border-y-border border-r-border bg-primary/10'
                      : 'border-border bg-card'
                } ${isSelected ? 'ring-2 ring-primary/50' : ''}`}
              >
                {/* Alert header */}
                <div className="flex items-center min-w-0">
                  {/* Checkbox — always visible for non-deleted alerts */}
                  {!isDeleted && (
                    <div className="pl-3 sm:pl-4 flex items-center shrink-0">
                      <Checkbox
                        checked={isSelected}
                        onCheckedChange={() => toggleAlertSelection(alert.id)}
                      />
                    </div>
                  )}
                  <button
                    onClick={() => toggleExpanded(alert.id)}
                    className={`flex-1 min-w-0 py-3 pr-2 sm:pr-4 flex items-center justify-between hover:bg-muted/50 transition-colors text-left ${isDeleted ? 'pl-3 sm:pl-4' : 'pl-2'}`}
                  >
                    <div className="flex items-center gap-2 min-w-0 flex-1">
                      {alert.mode === 'report' ? (
                        <ChartBarIcon className={`h-5 w-5 shrink-0 hidden sm:block ${isUnread ? 'text-primary' : 'text-muted-foreground'}`} />
                      ) : (
                        <Bell className={`h-5 w-5 shrink-0 hidden sm:block ${isUnread ? 'text-primary' : 'text-muted-foreground'}`} />
                      )}
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <span className={`text-foreground truncate ${isUnread ? 'font-semibold' : 'font-medium'}`}>
                            {alert.execution_trace?.alert_title || getWatchName(alert)}
                          </span>
                          {isDeleted && (
                            <Badge variant="secondary" className="text-xs shrink-0">
                              Deleted
                            </Badge>
                          )}
                        </div>
                        {alert.execution_trace?.summary && (
                          <p className="text-sm text-muted-foreground truncate mt-0.5">
                            {alert.execution_trace.summary}
                          </p>
                        )}
                        <div className="flex items-center gap-2 text-xs text-muted-foreground mt-0.5 min-w-0">
                          <span className="truncate">{getWatchName(alert)}</span>
                          <span className="shrink-0">•</span>
                          <Clock className="h-3 w-3 shrink-0" />
                          <span className="shrink-0">{formatDate(alert.started_at)}</span>
                        </div>
                      </div>
                    </div>
                    {expandedAlerts.has(alert.id) ? (
                      <ChevronUp className="h-5 w-5 text-muted-foreground shrink-0 ml-1" />
                    ) : (
                      <ChevronDown className="h-5 w-5 text-muted-foreground shrink-0 ml-1" />
                    )}
                  </button>
                  {/* Action buttons */}
                  <div className="pr-2 sm:pr-3 flex items-center shrink-0">
                    {/* Continue in Chat button - hidden on mobile, shown in dropdown instead */}
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => continueChatMutation.mutate(alert.id)}
                      disabled={continueChatMutation.isPending}
                      title="Continue in Chat"
                      className="hidden sm:inline-flex"
                    >
                      {continueChatMutation.isPending ? (
                        <Spinner />
                      ) : (
                        <ChatBubbleLeftRightIcon className="h-4 w-4" />
                      )}
                    </Button>
                    {/* More actions dropdown */}
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="sm">
                          <MoreVertical className="h-4 w-4" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        {/* Continue in Chat - visible in dropdown on mobile */}
                        <DropdownMenuItem
                          onClick={() => continueChatMutation.mutate(alert.id)}
                          disabled={continueChatMutation.isPending}
                          className="sm:hidden"
                        >
                          <ChatBubbleLeftRightIcon className="h-4 w-4 mr-2" />
                          Continue in Chat
                        </DropdownMenuItem>
                        {/* Mark as unread (only for read, non-deleted alerts) */}
                        {!isUnread && !isDeleted && (
                          <DropdownMenuItem
                            onClick={() => markUnreadMutation.mutate(alert.id)}
                            disabled={markUnreadMutation.isPending}
                          >
                            <MailOpen className="h-4 w-4 mr-2" />
                            Mark as unread
                          </DropdownMenuItem>
                        )}
                        {/* Delete/Restore */}
                        {isDeleted ? (
                          <DropdownMenuItem
                            onClick={() => restoreMutation.mutate(alert.id)}
                            disabled={restoreMutation.isPending}
                          >
                            <Undo2 className="h-4 w-4 mr-2" />
                            Restore
                          </DropdownMenuItem>
                        ) : (
                          <DropdownMenuItem
                            onClick={() => deleteMutation.mutate(alert.id)}
                            disabled={deleteMutation.isPending}
                            className="text-destructive focus:text-destructive"
                          >
                            <Trash2 className="h-4 w-4 mr-2" />
                            Delete
                          </DropdownMenuItem>
                        )}
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </div>

                {/* Alert content (expanded) */}
                {expandedAlerts.has(alert.id) && (
                  <div className="px-3 sm:px-4 py-3 border-t border-border bg-muted/30 overflow-x-auto">
                    <MarkdownRenderer
                      className="text-sm"
                      onSaveChartToDashboard={handleSaveChartToDashboard}
                      onShowChartInfo={handleShowChartInfo}
                      onAskAboutChart={handleAskAboutChart}
                    >
                      {alert.agent_response}
                    </MarkdownRenderer>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}

      {/* Pagination */}
      {(hasMore || hasPrevious) && (
        <div className="flex items-center justify-between pt-4 border-t border-border">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setPage(page - 1)}
            disabled={!hasPrevious}
          >
            Previous
          </Button>
          <span className="text-sm text-muted-foreground">
            Page {page + 1} of {Math.ceil(total / limit)}
          </span>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setPage(page + 1)}
            disabled={!hasMore}
          >
            Next
          </Button>
        </div>
      )}

      {/* Save to Dashboard Modal */}
      <SaveDashboardModal
        isOpen={dashboardModal.isOpen}
        onClose={() => setDashboardModal({ isOpen: false, messageContent: '' })}
        messageContent={dashboardModal.messageContent}
        apiClient={apiClient}
      />

      {/* Chart Info Modal */}
      <ChartInfoModal
        isOpen={chartInfoModal.isOpen}
        onClose={() => setChartInfoModal({ isOpen: false, spec: null })}
        spec={chartInfoModal.spec}
      />

      {/* Bulk Delete Confirmation */}
      <ConfirmDialog isOpen={isConfirmOpen} {...dialogProps} />
    </div>
  );
}
