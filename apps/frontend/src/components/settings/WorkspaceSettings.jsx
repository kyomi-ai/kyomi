// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useCallback } from 'react';
import { Check, ExternalLink, RefreshCw, Unplug } from 'lucide-react';
import { useCapabilities } from '../../context/CapabilitiesContext';
import { useSystemConfig } from '../../context/SystemConfigContext';
import { Spinner } from '../ui/spinner';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '../ui/card';
import { Button } from '../ui/button';
import { Alert, AlertDescription } from '../ui/alert';
import { Badge } from '../ui/badge';
import ConfirmDialog from '../ConfirmDialog';
import useConfirm from '../../hooks/useConfirm';

export default function WorkspaceSettings({ user, apiClient }) {
  const capabilities = useCapabilities();
  const { features } = useSystemConfig();
  const { isOpen, dialogProps, confirm } = useConfirm();
  // Save status tracking (for auto-save feedback)
  const [saveStatus, setSaveStatus] = useState({
    workspaceName: 'idle', // 'idle' | 'saving' | 'saved' | 'error'
  });

  // Workspace settings state
  const [workspaceName, setWorkspaceName] = useState('');
  const [workspaceSettingsLoading, setWorkspaceSettingsLoading] = useState(false);

  // Knowledge graph state
  const [graphRebuilding, setGraphRebuilding] = useState(false);
  const [graphResult, setGraphResult] = useState(null); // { type: 'success'|'error', message }

  // Slack integration state
  const [slackStatus, setSlackStatus] = useState(null);
  const [slackLoading, setSlackLoading] = useState(false);
  const [slackError, setSlackError] = useState(null);
  const [slackSuccess, setSlackSuccess] = useState(null);
  const [slackUninstalling, setSlackUninstalling] = useState(false);

  // Check for Slack OAuth callback success/error in URL params
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const slackParam = params.get('slack');

    if (slackParam === 'installed') {
      setSlackSuccess('Kyomi has been added to your Slack workspace!');
      // Clear the URL param without reload
      const newUrl = window.location.pathname + window.location.search.replace(/[?&]slack=installed/, '').replace(/^&/, '?');
      window.history.replaceState({}, '', newUrl || window.location.pathname);
      // Clear success after 5 seconds
      setTimeout(() => setSlackSuccess(null), 5000);
    } else if (slackParam === 'error') {
      setSlackError('Failed to install Slack integration. Please try again.');
      const newUrl = window.location.pathname + window.location.search.replace(/[?&]slack=error/, '').replace(/^&/, '?');
      window.history.replaceState({}, '', newUrl || window.location.pathname);
    }
  }, []);

  // Load workspace settings
  useEffect(() => {
    const loadWorkspaceSettings = async () => {
      if (!apiClient || !user) return;

      try {
        setWorkspaceSettingsLoading(true);
        const response = await apiClient.get('/api/v1/workspaces/settings');
        const data = response.data;

        setWorkspaceName(data.name || '');
      } catch (error) {
      } finally {
        setWorkspaceSettingsLoading(false);
      }
    };

    loadWorkspaceSettings();
  }, [apiClient, user]);

  // Load Slack integration status
  useEffect(() => {
    const loadSlackStatus = async () => {
      if (!apiClient || !user) return;

      try {
        setSlackLoading(true);
        setSlackError(null);
        const response = await apiClient.get('/api/v1/slack/status');
        setSlackStatus(response.data);
      } catch (error) {
        // Don't show error if Slack is just not configured
        if (error.response?.status !== 404) {
          setSlackError('Failed to load Slack integration status');
        }
      } finally {
        setSlackLoading(false);
      }
    };

    loadSlackStatus();
  }, [apiClient, user]);

  // Handle "Add to Slack" button click
  const handleSlackInstall = async () => {
    try {
      setSlackLoading(true);
      setSlackError(null);
      const response = await apiClient.get('/api/v1/slack/install');
      // Redirect to Slack OAuth
      window.location.href = response.data.authorization_url;
    } catch (error) {
      setSlackError(error.response?.data?.detail || 'Failed to start Slack installation');
      setSlackLoading(false);
    }
  };

  // Handle Slack uninstall
  const handleSlackUninstall = async () => {
    const confirmed = await confirm({
      title: 'Remove Slack Integration?',
      message: 'Are you sure you want to remove the Slack integration? Watch alerts will no longer be sent to Slack.',
      confirmText: 'Remove',
      variant: 'destructive'
    });
    if (!confirmed) {
      return;
    }

    try {
      setSlackUninstalling(true);
      setSlackError(null);
      setSlackSuccess(null);
      await apiClient.delete(`/api/v1/slack/uninstall?slack_team_id=${slackStatus.team_id}`);
      setSlackStatus({ installed: false });
      setSlackSuccess('Slack integration removed successfully.');
      setTimeout(() => setSlackSuccess(null), 3000);
    } catch (error) {
      setSlackError(error.response?.data?.detail || 'Failed to remove Slack integration');
    } finally {
      setSlackUninstalling(false);
    }
  };

  // Handle knowledge graph rebuild
  const handleGraphRebuild = async () => {
    try {
      setGraphRebuilding(true);
      setGraphResult(null);
      const response = await apiClient.post('/api/v1/workspaces/admin/populate-graph');
      const data = response.data;
      setGraphResult({
        type: 'success',
        message: `Graph rebuilt: ${data.tables} tables, ${data.columns} columns, ${data.learnings} learnings, ${data.metrics} metrics.`,
      });
      setTimeout(() => setGraphResult(null), 8000);
    } catch (error) {
      setGraphResult({
        type: 'error',
        message: error.response?.data?.detail || 'Failed to rebuild knowledge graph',
      });
    } finally {
      setGraphRebuilding(false);
    }
  };

  // Auto-save: Workspace name (on blur)
  const autoSaveWorkspaceName = useCallback(async () => {
    if (!workspaceName) return;

    setSaveStatus(prev => ({ ...prev, workspaceName: 'saving' }));
    try {
      await apiClient.patch('/api/v1/workspaces/settings', {
        name: workspaceName,
      });
      setSaveStatus(prev => ({ ...prev, workspaceName: 'saved' }));

      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, workspaceName: 'idle' }));
      }, 3000);
    } catch (error) {
      setSaveStatus(prev => ({ ...prev, workspaceName: 'error' }));
      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, workspaceName: 'idle' }));
      }, 5000);
    }
  }, [workspaceName, apiClient]);

  // Debounced auto-save for workspace name (3 seconds after user stops typing)
  useEffect(() => {
    if (!workspaceName) return;

    const timer = setTimeout(() => {
      autoSaveWorkspaceName();
    }, 3000);

    return () => clearTimeout(timer);
  }, [workspaceName, autoSaveWorkspaceName]);

  // Save status indicator component
  const SaveStatusIndicator = ({ status }) => {
    if (status === 'saving') {
      return (
        <span className="inline-flex items-center gap-1 text-xs text-muted-foreground">
          <Spinner size="sm" />
          Saving...
        </span>
      );
    }
    if (status === 'saved') {
      return (
        <span className="inline-flex items-center gap-1 text-xs text-success-foreground">
          <Check className="h-3 w-3" />
          Saved
        </span>
      );
    }
    if (status === 'error') {
      return (
        <span className="inline-flex items-center gap-1 text-xs text-error-foreground">
          Failed to save
        </span>
      );
    }
    return null;
  };

  return (
    <div className="p-6">
      <h2 className="text-xl font-semibold text-foreground mb-4">Workspace Settings</h2>
      <p className="text-muted-foreground mb-6">
        Configure workspace-wide preferences (admin only).
      </p>

      {workspaceSettingsLoading ? (
        <Card>
          <CardContent className="pt-6">
            <p className="text-sm text-muted-foreground text-center py-8">
              Loading workspace settings...
            </p>
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-6">
          {/* Workspace Name */}
          <Card>
            <CardHeader>
              <div className="flex items-center justify-between">
                <div>
                  <CardTitle>Workspace Name</CardTitle>
                  <CardDescription>
                    Give your workspace a meaningful name to help identify it.
                  </CardDescription>
                </div>
                <SaveStatusIndicator status={saveStatus.workspaceName} />
              </div>
            </CardHeader>
            <CardContent>
              <input
                type="text"
                value={workspaceName}
                onChange={(e) => setWorkspaceName(e.target.value)}
                onBlur={autoSaveWorkspaceName}
                placeholder="My Workspace"
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-foreground focus:outline-none focus:ring-2 focus:ring-ring"
              />
            </CardContent>
          </Card>

          {/* Knowledge Graph */}
          <Card>
            <CardHeader>
              <CardTitle>Knowledge Graph</CardTitle>
              <CardDescription>
                Rebuild the knowledge graph from your catalog and learnings. This fixes stale or missing graph data.
              </CardDescription>
            </CardHeader>
            <CardContent>
              {graphResult && (
                <Alert variant={graphResult.type === 'success' ? 'success' : 'error'} className="mb-4">
                  <AlertDescription>{graphResult.message}</AlertDescription>
                </Alert>
              )}
              <Button
                variant="outline"
                onClick={handleGraphRebuild}
                disabled={graphRebuilding}
              >
                {graphRebuilding ? (
                  <>
                    <Spinner size="sm" className="mr-2" />
                    Rebuilding...
                  </>
                ) : (
                  <>
                    <RefreshCw className="h-4 w-4 mr-2" />
                    Rebuild Graph
                  </>
                )}
              </Button>
            </CardContent>
          </Card>

          {/* Slack Integration */}
          {features.slack_integration && <Card>
            <CardHeader>
              <div className="flex items-center justify-between">
                <div>
                  <CardTitle>Slack Integration</CardTitle>
                  <CardDescription>
                    Enable watch alerts to post to Slack channels as Kyomi.
                  </CardDescription>
                </div>
                {!capabilities?.slackIntegrationEnabled && (
                  <Badge variant="warning">Team Plan</Badge>
                )}
              </div>
            </CardHeader>
            <CardContent>
              {!capabilities?.slackIntegrationEnabled ? (
                <Alert variant="info">
                  <AlertDescription>
                    Slack integration is available on Team and Enterprise plans.
                    <a href="/settings/billing" className="text-primary font-medium ml-1 hover:underline">
                      Upgrade to enable
                    </a>
                  </AlertDescription>
                </Alert>
              ) : (
                <>
                  {slackSuccess && (
                    <Alert variant="success" className="mb-4">
                      <AlertDescription>{slackSuccess}</AlertDescription>
                    </Alert>
                  )}
                  {slackError && (
                    <Alert variant="error" className="mb-4">
                      <AlertDescription>{slackError}</AlertDescription>
                    </Alert>
                  )}

                  {slackLoading ? (
                    <div className="flex items-center gap-2 text-muted-foreground">
                      <Spinner size="sm" />
                      <span>Loading Slack status...</span>
                    </div>
                  ) : slackStatus?.installed ? (
                    <div className="space-y-4">
                      <div className="flex items-center gap-2">
                        <div className="h-2 w-2 rounded-full bg-success" />
                        <span className="text-sm text-foreground">
                          Connected to <strong>{slackStatus.team_name}</strong>
                        </span>
                      </div>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={handleSlackUninstall}
                        disabled={slackUninstalling}
                      >
                        {slackUninstalling ? (
                          <>
                            <Spinner size="sm" className="mr-2" />
                            Removing...
                          </>
                        ) : (
                          <>
                            <Unplug className="h-4 w-4 mr-2" />
                            Remove Integration
                          </>
                        )}
                      </Button>
                    </div>
                  ) : (
                    <div className="space-y-4">
                      <p className="text-sm text-muted-foreground">
                        Connect Kyomi to your Slack workspace to receive watch alerts in channels.
                      </p>
                      <Button
                        onClick={handleSlackInstall}
                        disabled={slackLoading}
                      >
                        <ExternalLink className="h-4 w-4 mr-2" />
                        Add Kyomi to Slack
                      </Button>
                    </div>
                  )}
                </>
              )}
            </CardContent>
          </Card>}

        </div>
      )}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
    </div>
  );
}
