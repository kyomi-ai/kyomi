// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useCallback } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useAuth } from '../../context/AuthContext';
import { useSystemConfig } from '../../context/SystemConfigContext';
import Modal from '../Modal';
import { Button } from '../ui/button';
import { Input } from '../ui/input';
import { Label } from '../ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../ui/select';
import { toast } from '../../lib/toast';
import ScheduleSelector from './ScheduleSelector';
import { Eye, MessageSquare, AlertTriangle, Mail, Code, Trash2, Plus } from 'lucide-react';
import { PencilIcon, TrashIcon, BellIcon, ChartBarIcon } from '@heroicons/react/24/outline';
import { Spinner } from '../ui/spinner';
import { Switch } from '../ui/switch';

/**
 * WatchModal - Edit watch modal
 *
 * Note: Watch creation is now done through chat (AI-guided).
 * This modal is only used for editing existing watches.
 *
 * @param {Object} watch - Existing watch to edit
 * @param {Function} onClose - Close callback
 * @param {Function} onSaved - Save success callback
 */
export default function WatchModal({ watch, onClose, onSaved }) {
  const { apiClient, user } = useAuth();
  const { features } = useSystemConfig();
  const queryClient = useQueryClient();

  // Form state - initialized from watch prop
  // Default alert_emails to user's email if not set
  const [formData, setFormData] = useState({
    name: watch?.name ?? '',
    prompt: watch?.prompt ?? '',
    schedule: watch?.schedule ?? '',
    mode: watch?.mode ?? 'alert',  // "alert" or "report"
    slack_channel_id: watch?.slack_channel_id ?? '',
    alert_emails: watch?.alert_emails ?? user?.email ?? '',
    alert_emails_enabled: watch?.alert_emails_enabled ?? false,
    queries: watch?.queries ?? [],
  });

  // UI state - track which query is being edited (null = none)
  const [editingQueryIdx, setEditingQueryIdx] = useState(null);

  // Fetch Slack integration status
  const { data: slackStatus } = useQuery({
    queryKey: ['slack-status'],
    queryFn: async () => {
      const response = await apiClient.get('/api/v1/slack/status');
      return response.data;
    },
    staleTime: 60000, // Cache for 1 minute
  });

  // Fetch available Slack channels if user is connected
  const { data: slackChannels, isLoading: channelsLoading } = useQuery({
    queryKey: ['slack-channels'],
    queryFn: async () => {
      const response = await apiClient.get('/api/v1/slack/channels');
      return response.data.channels;
    },
    enabled: slackStatus?.installed && slackStatus?.user_connected,
    staleTime: 60000, // Cache for 1 minute
  });

  // Fetch available datasources for query editor
  const { data: datasources = [] } = useQuery({
    queryKey: ['datasources'],
    queryFn: async () => {
      const response = await apiClient.get('/api/v1/datasources');
      return response.data;
    },
    staleTime: 60000, // Cache for 1 minute
  });

  // Update mutation
  const updateMutation = useMutation({
    mutationFn: async (data) => {
      const response = await apiClient.patch(`/api/v1/watches/${watch.watch_id}`, data);
      return response.data;
    },
    onSuccess: () => {
      toast.success('Watch updated successfully');
      queryClient.invalidateQueries(['watches']);
      onSaved?.();
    },
    onError: (error) => {
      toast.error(error.response?.data?.detail || 'Failed to update watch');
    },
  });

  const handleSubmit = (e) => {
    e.preventDefault();

    // Validation
    if (!formData.name.trim() || formData.name.trim().length < 3) {
      toast.error('Name must be at least 3 characters');
      return;
    }
    if (!formData.prompt.trim() || formData.prompt.trim().length < 10) {
      toast.error('Monitoring instruction must be at least 10 characters');
      return;
    }

    const data = {
      name: formData.name.trim(),
      prompt: formData.prompt.trim(),
      schedule: formData.schedule,
      mode: formData.mode,
      slack_channel_id: formData.slack_channel_id || null,
      alert_emails: formData.alert_emails.trim() || null,
      alert_emails_enabled: formData.alert_emails_enabled,
      queries: formData.queries.filter(q => q.sql.trim()),
    };

    updateMutation.mutate(data);
  };

  // Stable callback for schedule changes
  const handleScheduleChange = useCallback((newSchedule) => {
    setFormData(prev => ({ ...prev, schedule: newSchedule }));
  }, []);

  const isPending = updateMutation.isPending;

  return (
    <Modal
      show={true}
      onClose={onClose}
      title={
        <div className="flex items-center gap-2">
          <Eye className="h-5 w-5 text-primary" />
          <span>Edit Watch</span>
        </div>
      }
      size="lg"
      footer={
        <>
          <Button type="button" variant="ghost" onClick={onClose} disabled={isPending}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={isPending}>
            {isPending ? (
              <>
                <Spinner className="mr-2" />
                Saving...
              </>
            ) : (
              'Save Changes'
            )}
          </Button>
        </>
      }
    >
      <form onSubmit={handleSubmit} className="space-y-5">
        {/* Name */}
        <div className="space-y-2">
          <Label htmlFor="watch-name">Name</Label>
          <Input
            id="watch-name"
            type="text"
            value={formData.name}
            onChange={(e) => setFormData({ ...formData, name: e.target.value })}
            placeholder="Daily Sales Monitor"
          />
          <p className="text-xs text-muted-foreground">
            A short, descriptive name for this watch
          </p>
        </div>

        {/* Mode Toggle */}
        <div className="space-y-2">
          <Label className="flex items-center gap-2">
            Mode
          </Label>
          <div className="flex flex-col sm:flex-row gap-3 sm:gap-4">
            <button
              type="button"
              onClick={() => setFormData({ ...formData, mode: 'alert' })}
              className={`flex-1 p-3 rounded-lg border text-left transition-colors ${
                formData.mode === 'alert'
                  ? 'border-primary bg-primary/5'
                  : 'border-border hover:border-muted-foreground/50'
              }`}
            >
              <div className="flex items-center gap-2 mb-1">
                <BellIcon className="h-5 w-5" />
                <span className={`font-medium ${formData.mode === 'alert' ? 'text-primary' : ''}`}>
                  Alert
                </span>
              </div>
              <p className="text-xs text-muted-foreground">
                Agent decides when to notify you based on conditions
              </p>
            </button>
            <button
              type="button"
              onClick={() => setFormData({ ...formData, mode: 'report' })}
              className={`flex-1 p-3 rounded-lg border text-left transition-colors ${
                formData.mode === 'report'
                  ? 'border-primary bg-primary/5'
                  : 'border-border hover:border-muted-foreground/50'
              }`}
            >
              <div className="flex items-center gap-2 mb-1">
                <ChartBarIcon className="h-5 w-5" />
                <span className={`font-medium ${formData.mode === 'report' ? 'text-primary' : ''}`}>
                  Report
                </span>
              </div>
              <p className="text-xs text-muted-foreground">
                Always sends a summary on schedule, no conditions
              </p>
            </button>
          </div>
        </div>

        {/* Prompt */}
        <div className="space-y-2">
          <Label htmlFor="watch-prompt">
            {formData.mode === 'report' ? 'Report Instructions' : 'Monitoring Instructions'}
          </Label>
          <textarea
            id="watch-prompt"
            value={formData.prompt}
            onChange={(e) => setFormData({ ...formData, prompt: e.target.value })}
            placeholder={
              formData.mode === 'report'
                ? "Summarize our daily sales revenue. Include key metrics, trends, and any notable observations."
                : "Check our daily sales revenue. Alert me if it drops more than 10% compared to the same day last week, or if there are any unusual patterns."
            }
            rows={6}
            className="flex w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 resize-y min-h-[120px]"
          />
          <p className="text-xs text-muted-foreground">
            {formData.mode === 'report'
              ? "Describe what data to include in the scheduled report. Be specific about metrics and format."
              : "Describe what to monitor and when to alert you. Be specific about thresholds or conditions."}
          </p>
        </div>

        {/* Pre-determined Queries */}
        <div className="space-y-3">
          <Label className="flex items-center gap-2">
            <Code className="h-4 w-4" />
            Reference Queries
          </Label>
          <p className="text-xs text-muted-foreground">
            {formData.mode === 'report'
              ? 'These queries serve as reference for the report generation.'
              : 'These queries serve as reference for the monitoring agent.'}
          </p>

          {formData.queries.length === 0 ? (
            <div className="text-sm text-muted-foreground italic p-3 bg-muted/50 rounded">
              No reference queries configured
            </div>
          ) : (
            <div className="space-y-2">
              {formData.queries.map((query, idx) => (
                editingQueryIdx === idx ? (
                  // Edit mode for this specific query
                  <div key={idx} className="border border-border rounded-lg p-4 space-y-3 bg-muted/20">
                    <div className="flex items-start gap-2">
                      <div className="flex-1 space-y-2">
                        <Label className="text-xs">Query Title</Label>
                        <Input
                          value={query.comment}
                          onChange={(e) => {
                            const newQueries = [...formData.queries];
                            newQueries[idx] = { ...query, comment: e.target.value };
                            setFormData({ ...formData, queries: newQueries });
                          }}
                          placeholder="e.g., Daily Revenue Trend"
                          className="text-sm"
                        />
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          const newQueries = formData.queries.filter((_, i) => i !== idx);
                          setFormData({ ...formData, queries: newQueries });
                          setEditingQueryIdx(null);
                        }}
                        className="mt-6"
                      >
                        <Trash2 className="h-4 w-4 text-destructive" />
                      </Button>
                    </div>

                    <div className="space-y-2">
                      <Label className="text-xs">SQL Query</Label>
                      <textarea
                        value={query.sql}
                        onChange={(e) => {
                          const newQueries = [...formData.queries];
                          newQueries[idx] = { ...query, sql: e.target.value };
                          setFormData({ ...formData, queries: newQueries });
                        }}
                        placeholder="SELECT ..."
                        rows={4}
                        className="w-full font-mono text-xs p-2 border border-input rounded bg-background"
                      />
                    </div>

                    <div className="space-y-2">
                      <Label className="text-xs">Datasource (Optional)</Label>
                      <Select
                        value={query.datasource || "none"}
                        onValueChange={(value) => {
                          const newQueries = [...formData.queries];
                          newQueries[idx] = { ...query, datasource: value === "none" ? null : value };
                          setFormData({ ...formData, queries: newQueries });
                        }}
                      >
                        <SelectTrigger className="h-9">
                          <SelectValue placeholder="Select a datasource" />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="none">None</SelectItem>
                          {datasources.map((ds) => (
                            <SelectItem key={ds.id} value={ds.slug}>
                              {ds.name}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>

                    <div className="flex gap-2 pt-2 border-t border-border">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        onClick={() => setEditingQueryIdx(null)}
                      >
                        Done
                      </Button>
                    </div>
                  </div>
                ) : (
                  // Read-only view: Compact attachment-like block
                  <div key={idx} className="flex items-start gap-3 p-3 rounded-lg border border-border bg-muted/30 hover:bg-muted/50 transition-colors group">
                    <Code className="h-4 w-4 text-muted-foreground mt-0.5 shrink-0" />
                    <div className="flex-1 min-w-0">
                      <p className="font-medium text-sm text-foreground break-words">{query.comment}</p>
                      <p className="text-xs text-muted-foreground font-mono mt-1 truncate">{query.sql}</p>
                      {query.datasource && (
                        <div className="mt-2">
                          <span className="inline-block px-2 py-1 rounded text-xs bg-secondary text-secondary-foreground">
                            {query.datasource}
                          </span>
                        </div>
                      )}
                    </div>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => setEditingQueryIdx(idx)}
                      className="shrink-0 sm:opacity-0 sm:group-hover:opacity-100 transition-opacity"
                    >
                      <PencilIcon className="h-4 w-4" />
                    </Button>
                  </div>
                )
              ))}
            </div>
          )}

          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => {
              setFormData({
                ...formData,
                queries: [...formData.queries, { comment: '', sql: '', datasource: null }]
              });
            }}
          >
            <Plus className="h-4 w-4 mr-1" />
            Add Query
          </Button>
        </div>

        {/* Schedule */}
        <ScheduleSelector
          value={formData.schedule}
          onChange={handleScheduleChange}
        />

        {/* Slack Notifications */}
        {features.watch_slack_alerts && (
        <div className="space-y-2">
          <Label htmlFor="slack-channel" className="flex items-center gap-2">
            <MessageSquare className="h-4 w-4" />
            Slack Notifications
          </Label>

          {!slackStatus?.installed ? (
            // Slack app not installed in workspace
            <div className="flex items-start gap-2 p-3 bg-muted/50 rounded-lg text-sm text-muted-foreground">
              <AlertTriangle className="h-4 w-4 text-warning-foreground mt-0.5 shrink-0" />
              <span>
                Slack is not installed. Ask your workspace admin to connect Slack in Settings.
              </span>
            </div>
          ) : !slackStatus?.user_connected ? (
            // User not connected to Slack
            <div className="flex items-start gap-2 p-3 bg-muted/50 rounded-lg text-sm text-muted-foreground">
              <AlertTriangle className="h-4 w-4 text-warning-foreground mt-0.5 shrink-0" />
              <span>
                Connect your Slack account in Profile Settings to send {formData.mode === 'report' ? 'reports' : 'alerts'} to Slack.
              </span>
            </div>
          ) : channelsLoading ? (
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <Spinner size="sm" />
              <span>Loading channels...</span>
            </div>
          ) : slackChannels && slackChannels.length === 0 ? (
            // No channels - bot needs to be invited
            <div className="flex items-start gap-2 p-3 bg-muted/50 rounded-lg text-sm text-muted-foreground">
              <AlertTriangle className="h-4 w-4 text-warning-foreground mt-0.5 shrink-0" />
              <span>
                Invite the Kyomi app to a Slack channel first. Then refresh this page to see available channels.
              </span>
            </div>
          ) : (
            // Channel selector
            <>
              <Select value={formData.slack_channel_id || "none"} onValueChange={(value) => setFormData({ ...formData, slack_channel_id: value === "none" ? null : value })}>
                <SelectTrigger id="slack-channel">
                  <SelectValue placeholder="Select a channel" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">None (no Slack notifications)</SelectItem>
                  {slackChannels?.map((channel) => (
                    <SelectItem key={channel.id} value={channel.id}>
                      #{channel.name} {channel.is_private ? '(private)' : ''}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                {formData.mode === 'report' ? 'Reports' : 'Alerts'} will be posted to this channel as Kyomi.
              </p>
            </>
          )}
        </div>
        )}

        {/* Email Notifications */}
        {features.watch_email_alerts && (
        <div className="space-y-3">
          <div className="flex items-center justify-between">
            <Label htmlFor="alert-emails-toggle" className="flex items-center gap-2">
              <Mail className="h-4 w-4" />
              Email Notifications
            </Label>
            <div className="flex items-center gap-2">
              <span className="text-xs text-muted-foreground">
                {formData.alert_emails_enabled ? 'Enabled' : 'Disabled'}
              </span>
              <Switch
                id="alert-emails-toggle"
                checked={formData.alert_emails_enabled}
                onCheckedChange={(checked) => setFormData({ ...formData, alert_emails_enabled: checked })}
              />
            </div>
          </div>
          <Input
            id="alert-emails"
            type="text"
            value={formData.alert_emails}
            onChange={(e) => setFormData({ ...formData, alert_emails: e.target.value })}
            placeholder="your@email.com, colleague@email.com"
            disabled={!formData.alert_emails_enabled}
            className={!formData.alert_emails_enabled ? 'opacity-50' : ''}
          />
          <p className="text-xs text-muted-foreground">
            {formData.alert_emails_enabled
              ? `Comma-separated email addresses to receive ${formData.mode === 'report' ? 'reports' : 'alerts'}.`
              : 'Enable email notifications to configure recipients.'}
          </p>
        </div>
        )}

        {/* Help text */}
        <div className="rounded-lg bg-muted/50 p-4 text-sm text-muted-foreground">
          <p className="font-medium text-foreground mb-2">How it works</p>
          {formData.mode === 'report' ? (
            <ul className="space-y-1 text-xs">
              <li>The AI will analyze your data based on your instructions</li>
              <li>A report summary will be sent on every scheduled run</li>
              <li>You can view all reports in the Alerts tab</li>
            </ul>
          ) : (
            <ul className="space-y-1 text-xs">
              <li>The AI will analyze your data based on your instructions</li>
              <li>If something noteworthy is found, you will receive an alert</li>
              <li>You can view all alerts in the Alerts tab</li>
            </ul>
          )}
        </div>
      </form>
    </Modal>
  );
}
