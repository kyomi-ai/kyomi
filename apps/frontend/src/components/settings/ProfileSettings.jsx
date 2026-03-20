// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useCallback } from 'react';
import { CheckCircle, Check, ExternalLink, Unplug, AlertTriangle, MessageSquare, Plug, Copy, Sun, Moon, Monitor, Bell, BellOff, Trash2, Smartphone, Bot } from 'lucide-react';
import usePushNotifications from '../../hooks/usePushNotifications';
import { useSystemConfig } from '../../context/SystemConfigContext';
import { useTheme } from '../../context/ThemeContext';
import { Spinner } from '../ui/spinner';
import { CHART_PALETTES } from '../../config/chartPalettes';
import { Button } from '../ui/button';
import { Badge } from '../ui/badge';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '../ui/card';
import { Tooltip, TooltipTrigger, TooltipContent } from '../ui/tooltip';
import { Alert, AlertDescription } from '../ui/alert';
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '../ui/select';
import { Label } from '../ui/label';
import ConfirmDialog from '../ConfirmDialog';
import useConfirm from '../../hooks/useConfirm';
import { toast } from '../../lib/toast';
import AIProviderSettings from './AIProviderSettings';

export default function ProfileSettings({ user, apiClient, refreshUser, capabilities }) {
  const { theme, setTheme } = useTheme();
  const { features, selfHosted, isPersonalMode } = useSystemConfig();
  const { isOpen, dialogProps, confirm } = useConfirm();
  // Save status tracking (for auto-save feedback)
  const [saveStatus, setSaveStatus] = useState({
    profileName: 'idle', // 'idle' | 'saving' | 'saved' | 'error'
    queryRetention: 'idle',
    chartPalette: 'idle',
    landingPage: 'idle',
    defaultDashboard: 'idle',
  });

  // Profile state
  const [profileName, setProfileName] = useState('');

  // Invitations state
  const [invitations, setInvitations] = useState([]);
  const [invitationsLoading, setInvitationsLoading] = useState(false);

  // User preferences state
  const [queryHistoryRetentionDays, setQueryHistoryRetentionDays] = useState(30);
  const [userPalette, setUserPalette] = useState(null);

  // Landing page & default dashboard preferences
  const [landingPage, setLandingPage] = useState('chat');
  const [userDefaultDashboard, setUserDefaultDashboard] = useState(null);
  const [dashboards, setDashboards] = useState([]);
  const [loadingDashboards, setLoadingDashboards] = useState(false);

  // Slack connection state
  const [slackStatus, setSlackStatus] = useState(null);
  const [slackLoading, setSlackLoading] = useState(false);
  const [slackError, setSlackError] = useState(null);
  const [slackDisconnecting, setSlackDisconnecting] = useState(false);
  const [slackNotAvailable, setSlackNotAvailable] = useState(false);

  // Slack channels state
  const [slackChannels, setSlackChannels] = useState([]);
  const [loadingChannels, setLoadingChannels] = useState(false);
  const [defaultWatchChannel, setDefaultWatchChannel] = useState(null);
  const [loadingDefaultChannel, setLoadingDefaultChannel] = useState(false);
  const [savingDefaultChannel, setSavingDefaultChannel] = useState(false);

  // Push notification state
  const pushNotifications = usePushNotifications();

  // Load push subscriptions when component mounts
  useEffect(() => {
    if (pushNotifications.supported && !pushNotifications.loading) {
      pushNotifications.refreshSubscriptions();
    }
  }, [pushNotifications.supported, pushNotifications.loading]);

  // Initialize profile name from user data
  useEffect(() => {
    if (user?.name) {
      setProfileName(user.name);
    }
  }, [user]);

  // Load user preferences
  useEffect(() => {
    const fetchUserPreferences = async () => {
      try {
        if (user?.extra_metadata?.query_history_retention_days) {
          setQueryHistoryRetentionDays(user.extra_metadata.query_history_retention_days);
        } else {
          const defaultRetention = capabilities?.query_history_retention_days || 7;
          setQueryHistoryRetentionDays(defaultRetention === 0 ? 365 : defaultRetention);
        }
        if (user?.extra_metadata?.landing_page) {
          setLandingPage(user.extra_metadata.landing_page);
        }
        if (user?.extra_metadata?.default_dashboard_id) {
          setUserDefaultDashboard(user.extra_metadata.default_dashboard_id);
        }
      } catch (error) {
      }
    };

    fetchUserPreferences();
  }, [user, capabilities]);

  // Load chart preferences
  useEffect(() => {
    const fetchChartPreferences = async () => {
      try {
        const userResponse = await apiClient.get('/api/v1/users/me/chartml-config');
        if (userResponse.data && userResponse.data.config && userResponse.data.config.style) {
          setUserPalette(userResponse.data.config.style);
        } else {
          setUserPalette('balanced');
        }
      } catch (error) {
        setUserPalette('balanced');
      }
    };

    if (apiClient) {
      fetchChartPreferences();
    }
  }, [apiClient]);

  // Load dashboards for default dashboard selector
  useEffect(() => {
    const fetchDashboards = async () => {
      if (!apiClient) return;
      try {
        setLoadingDashboards(true);
        const response = await apiClient.get('/api/v1/dashboards');
        setDashboards(response.data.dashboards || response.data || []);
      } catch (error) {
        console.warn('Failed to fetch dashboards:', error);
      } finally {
        setLoadingDashboards(false);
      }
    };
    fetchDashboards();
  }, [apiClient]);

  // Fetch user's pending invitations
  useEffect(() => {
    const fetchUserPendingInvitations = async () => {
      if (!apiClient) return;

      try {
        setInvitationsLoading(true);
        const response = await apiClient.get('/api/v1/workspaces/invitations/pending');
        setInvitations(response.data);
      } catch (error) {
      } finally {
        setInvitationsLoading(false);
      }
    };

    fetchUserPendingInvitations();
  }, [apiClient]);

  // Load Slack connection status
  useEffect(() => {
    const loadSlackStatus = async () => {
      if (!apiClient) return;

      try {
        setSlackLoading(true);
        setSlackError(null);
        const response = await apiClient.get('/api/v1/slack/status');
        setSlackStatus(response.data);
      } catch (error) {
        // Don't show error if Slack is just not configured or not available on this plan
        if (error.response?.status === 403) {
          setSlackNotAvailable(true);
        } else if (error.response?.status !== 404 && error.response?.status !== 500) {
          setSlackError('Failed to load Slack connection status');
        }
      } finally {
        setSlackLoading(false);
      }
    };

    loadSlackStatus();

    // Check for callback success/error in URL params
    const params = new URLSearchParams(window.location.search);
    const slackParam = params.get('slack');

    if (slackParam === 'connected') {
      toast.success('Your Slack account has been connected!');
      // Clear the URL param without reload
      const newUrl = window.location.pathname + window.location.search.replace(/[?&]slack=connected/, '').replace(/^&/, '?');
      window.history.replaceState({}, '', newUrl || window.location.pathname);
    } else if (slackParam === 'no_installation') {
      setSlackError('The Kyomi Slack app is not installed in your Slack workspace. Ask your workspace admin to install it first.');
      const newUrl = window.location.pathname + window.location.search.replace(/[?&]slack=no_installation/, '').replace(/^&/, '?');
      window.history.replaceState({}, '', newUrl || window.location.pathname);
    } else if (slackParam === 'user_error') {
      setSlackError('Failed to connect Slack account. Please try again.');
      const newUrl = window.location.pathname + window.location.search.replace(/[?&]slack=user_error/, '').replace(/^&/, '?');
      window.history.replaceState({}, '', newUrl || window.location.pathname);
    }
  }, [apiClient]);

  // Handle "Connect with Slack" button click
  const handleSlackConnect = async () => {
    try {
      setSlackLoading(true);
      setSlackError(null);
      const response = await apiClient.get('/api/v1/slack/user/connect');
      // Redirect to Slack OAuth
      window.location.href = response.data.authorization_url;
    } catch (error) {
      setSlackError(error.response?.data?.detail || 'Failed to start Slack connection');
      setSlackLoading(false);
    }
  };

  // Handle Slack disconnect
  const handleSlackDisconnect = async () => {
    const confirmed = await confirm({
      title: 'Disconnect Slack?',
      message: 'Your watches will no longer be able to post alerts to Slack.',
      confirmText: 'Disconnect',
      variant: 'destructive'
    });

    if (!confirmed) return;

    try {
      setSlackDisconnecting(true);
      setSlackError(null);
      await apiClient.post('/api/v1/slack/user/disconnect');
      setSlackStatus(prev => ({ ...prev, user_connected: false, slack_username: null }));
      setSlackChannels([]);
      setDefaultWatchChannel(null);
      toast.success('Slack account disconnected.');
    } catch (error) {
      setSlackError(error.response?.data?.detail || 'Failed to disconnect Slack');
    } finally {
      setSlackDisconnecting(false);
    }
  };

  // Load available Slack channels
  const loadSlackChannels = async () => {
    if (!apiClient) return;

    try {
      setLoadingChannels(true);
      const response = await apiClient.get('/api/v1/slack/channels');
      setSlackChannels(response.data.channels || []);
    } catch (error) {
      setSlackError(error.response?.data?.detail || 'Failed to load Slack channels');
      setSlackChannels([]);
    } finally {
      setLoadingChannels(false);
    }
  };

  // Load current default watch channel
  const loadDefaultWatchChannel = async () => {
    if (!apiClient) return;

    try {
      setLoadingDefaultChannel(true);
      const response = await apiClient.get('/api/v1/slack/default-watch-channel');
      setDefaultWatchChannel(response.data);
    } catch (error) {
      setDefaultWatchChannel({ channel_id: null, channel_name: null });
    } finally {
      setLoadingDefaultChannel(false);
    }
  };

  // Load channels and default channel when user connects to Slack
  useEffect(() => {
    if (slackStatus?.user_connected) {
      loadSlackChannels();
      loadDefaultWatchChannel();
    }
  }, [slackStatus?.user_connected]);

  // Save default watch channel
  const handleSetDefaultWatchChannel = async (channelId, channelName) => {
    if (!apiClient) return;

    try {
      setSavingDefaultChannel(true);
      const response = await apiClient.post('/api/v1/slack/default-watch-channel', {
        channel_id: channelId,
        channel_name: channelName
      });
      setDefaultWatchChannel(response.data);
      if (channelId && channelName) {
        toast.success(`Default watch channel set to #${channelName}`);
      } else {
        toast.success('Default watch channel cleared');
      }
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to set default watch channel');
    } finally {
      setSavingDefaultChannel(false);
    }
  };

  // Auto-save: Profile name (on blur)
  const autoSaveProfileName = useCallback(async () => {
    if (!profileName || profileName === user?.name) return; // Skip if unchanged

    setSaveStatus(prev => ({ ...prev, profileName: 'saving' }));
    try {
      await apiClient.patch('/api/v1/users/me', { name: profileName });
      await refreshUser();
      setSaveStatus(prev => ({ ...prev, profileName: 'saved' }));

      // Clear "saved" status after 3 seconds
      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, profileName: 'idle' }));
      }, 3000);
    } catch (error) {
      setSaveStatus(prev => ({ ...prev, profileName: 'error' }));
      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, profileName: 'idle' }));
      }, 5000);
    }
  }, [profileName, user?.name, apiClient, refreshUser]);

  // Auto-save: Query retention (on change)
  const autoSaveQueryRetention = useCallback(async (days) => {
    setSaveStatus(prev => ({ ...prev, queryRetention: 'saving' }));
    try {
      await apiClient.patch('/api/v1/users/me/preferences', {
        query_history_retention_days: days
      });
      setSaveStatus(prev => ({ ...prev, queryRetention: 'saved' }));

      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, queryRetention: 'idle' }));
      }, 3000);
    } catch (error) {
      setSaveStatus(prev => ({ ...prev, queryRetention: 'error' }));
      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, queryRetention: 'idle' }));
      }, 5000);
    }
  }, [apiClient]);

  // Auto-save: Chart palette (on selection)
  const autoSaveChartPalette = useCallback(async (palette) => {
    setSaveStatus(prev => ({ ...prev, chartPalette: 'saving' }));
    try {
      const config = {
        type: 'config',
        version: 1,
        style: palette
      };
      await apiClient.put('/api/v1/users/me/chartml-config', { config });
      setSaveStatus(prev => ({ ...prev, chartPalette: 'saved' }));

      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, chartPalette: 'idle' }));
      }, 3000);
    } catch (error) {
      setSaveStatus(prev => ({ ...prev, chartPalette: 'error' }));
      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, chartPalette: 'idle' }));
      }, 5000);
    }
  }, [apiClient]);

  // Auto-save: Landing page (on selection)
  const autoSaveLandingPage = useCallback(async (page) => {
    setSaveStatus(prev => ({ ...prev, landingPage: 'saving' }));
    try {
      await apiClient.patch('/api/v1/users/me/preferences', { landing_page: page });
      await refreshUser();
      setSaveStatus(prev => ({ ...prev, landingPage: 'saved' }));
      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, landingPage: 'idle' }));
      }, 3000);
    } catch (error) {
      setSaveStatus(prev => ({ ...prev, landingPage: 'error' }));
      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, landingPage: 'idle' }));
      }, 5000);
    }
  }, [apiClient, refreshUser]);

  // Auto-save: Default dashboard (on selection)
  const autoSaveDefaultDashboard = useCallback(async (dashboardId) => {
    setSaveStatus(prev => ({ ...prev, defaultDashboard: 'saving' }));
    try {
      await apiClient.patch('/api/v1/users/me/preferences', {
        default_dashboard_id: dashboardId || null,
      });
      await refreshUser();
      setSaveStatus(prev => ({ ...prev, defaultDashboard: 'saved' }));
      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, defaultDashboard: 'idle' }));
      }, 3000);
    } catch (error) {
      setSaveStatus(prev => ({ ...prev, defaultDashboard: 'error' }));
      setTimeout(() => {
        setSaveStatus(prev => ({ ...prev, defaultDashboard: 'idle' }));
      }, 5000);
    }
  }, [apiClient, refreshUser]);

  // Debounced auto-save for profile name (3 seconds after user stops typing)
  useEffect(() => {
    if (!profileName || profileName === user?.name) return;

    const timer = setTimeout(() => {
      autoSaveProfileName();
    }, 3000);

    return () => clearTimeout(timer);
  }, [profileName, user?.name, autoSaveProfileName]);

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

  const fetchUserPendingInvitations = async () => {
    if (!apiClient) return;

    try {
      setInvitationsLoading(true);
      const response = await apiClient.get('/api/v1/workspaces/invitations/pending');
      setInvitations(response.data);
    } catch (error) {
    } finally {
      setInvitationsLoading(false);
    }
  };

  return (
    <div className="p-6">
      <h2 className="text-xl font-semibold text-foreground mb-6">Profile Settings</h2>

      <div className="space-y-6">
        {/* Profile Information Section — hidden in personal mode */}
        {!isPersonalMode && (
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <CardTitle>Profile Information</CardTitle>
              <SaveStatusIndicator status={saveStatus.profileName} />
            </div>
          </CardHeader>
          <CardContent className="space-y-4">
            <div>
              <label className="block text-sm font-medium text-foreground mb-2">
                Name
              </label>
              <input
                type="text"
                value={profileName}
                onChange={(e) => setProfileName(e.target.value)}
                onBlur={autoSaveProfileName}
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-foreground focus:outline-none focus:ring-2 focus:ring-ring"
                placeholder="Your name"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-foreground mb-2">
                Email
              </label>
              <input
                type="email"
                value={user?.email || ''}
                disabled
                className="w-full px-3 py-2 border border-input rounded-md bg-muted text-muted-foreground cursor-not-allowed"
              />
              <p className="text-xs text-muted-foreground mt-1">Email cannot be changed</p>
            </div>
          </CardContent>
        </Card>
        )}

        {/* Appearance Section */}
        <Card>
          <CardHeader>
            <CardTitle>Appearance</CardTitle>
            <CardDescription>Choose how Kyomi looks to you.</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex flex-wrap gap-3">
              {[
                { value: 'light', label: 'Light', icon: Sun },
                { value: 'dark', label: 'Dark', icon: Moon },
                { value: 'system', label: 'System', icon: Monitor },
              ].map(({ value, label, icon: Icon }) => (
                <button
                  key={value}
                  onClick={() => {
                    setTheme(value);
                    apiClient.patch('/api/v1/users/me/preferences', { theme: value }).catch(() => {});
                  }}
                  className={`flex items-center gap-2 px-4 py-2 rounded-lg border-2 text-sm font-medium transition-all ${
                    theme === value
                      ? 'border-primary bg-primary/10 text-primary'
                      : 'border-border text-muted-foreground hover:border-border/80 hover:text-foreground'
                  }`}
                >
                  <Icon className="h-4 w-4" />
                  {label}
                </button>
              ))}
            </div>
          </CardContent>
        </Card>

        {/* Preferences Section */}
        <Card>
          <CardHeader>
            <CardTitle>Preferences</CardTitle>
            <CardDescription>Customize your Kyomi experience.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Landing Page */}
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <Label>Landing Page</Label>
                <SaveStatusIndicator status={saveStatus.landingPage} />
              </div>
              <Select
                value={landingPage}
                onValueChange={(value) => {
                  setLandingPage(value);
                  autoSaveLandingPage(value);
                }}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="chat">Chat</SelectItem>
                  <SelectItem value="dashboards">Dashboards</SelectItem>
                  <SelectItem value="watches">Watches</SelectItem>
                  <SelectItem value="sql_editor">SQL Editor</SelectItem>
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                Choose which page opens when you launch Kyomi.
              </p>
            </div>

            {/* My Default Dashboard */}
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <Label>My Default Dashboard</Label>
                <SaveStatusIndicator status={saveStatus.defaultDashboard} />
              </div>
              <Select
                value={userDefaultDashboard || 'workspace'}
                onValueChange={(value) => {
                  const newValue = value === 'workspace' ? null : value;
                  setUserDefaultDashboard(newValue);
                  autoSaveDefaultDashboard(newValue);
                }}
                disabled={loadingDashboards}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="workspace">Use workspace default</SelectItem>
                  {dashboards.map((d) => (
                    <SelectItem key={d.dashboard_id} value={d.dashboard_id}>
                      {d.title}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                Overrides the workspace default when you click Dashboards or land on dashboards.
              </p>
            </div>
          </CardContent>
        </Card>

        {/* Slack Connection Section - hidden if not available or in personal mode */}
        {!isPersonalMode && !slackNotAvailable && features.slack_integration && <Card>
          <CardHeader>
            <CardTitle>Slack Connection</CardTitle>
            <CardDescription>
              Link your Slack account to receive watch alerts in Slack channels.
            </CardDescription>
          </CardHeader>
          <CardContent>
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
            ) : !slackStatus?.installed ? (
              // Workspace doesn't have Slack installed
              <div className="flex items-start gap-3 p-4 bg-muted/50 rounded-lg">
                <AlertTriangle className="h-5 w-5 text-warning-foreground mt-0.5" />
                <div>
                  <p className="text-sm text-foreground font-medium">Slack not installed</p>
                  <p className="text-sm text-muted-foreground mt-1">
                    Ask your workspace admin to install the Kyomi Slack app in Workspace Settings.
                  </p>
                </div>
              </div>
            ) : slackStatus?.user_connected ? (
              // User is connected
              <div className="space-y-4">
                <div className="flex items-center gap-2">
                  <div className="h-2 w-2 rounded-full bg-success" />
                  <span className="text-sm text-foreground">
                    Connected as <strong>@{slackStatus.slack_username || 'unknown'}</strong> in <strong>{slackStatus.team_name}</strong>
                  </span>
                </div>
                <p className="text-xs text-muted-foreground">
                  Your watches can now post alerts to Slack channels.
                </p>

                {/* Default Watch Channel Selector */}
                <div className="space-y-2">
                  <Label className="flex items-center gap-2">
                    <MessageSquare className="h-4 w-4" />
                    Default Watch Channel
                  </Label>

                  {loadingChannels || loadingDefaultChannel ? (
                    <div className="flex items-center gap-2 text-sm text-muted-foreground">
                      <Spinner size="sm" />
                      <span>Loading channels...</span>
                    </div>
                  ) : slackChannels.length === 0 ? (
                    <div className="flex items-start gap-2 p-3 bg-muted/50 rounded-lg text-sm text-muted-foreground">
                      <AlertTriangle className="h-4 w-4 text-warning-foreground mt-0.5 shrink-0" />
                      <span>
                        Invite the Kyomi app to a Slack channel first. Then refresh this page to see available channels.
                      </span>
                    </div>
                  ) : (
                    <>
                      <Select
                        value={defaultWatchChannel?.channel_id || 'none'}
                        onValueChange={(value) => {
                          if (value === 'none') {
                            handleSetDefaultWatchChannel(null, null);
                          } else {
                            const channel = slackChannels.find(c => c.id === value);
                            if (channel) {
                              handleSetDefaultWatchChannel(channel.id, channel.name);
                            }
                          }
                        }}
                        disabled={savingDefaultChannel}
                      >
                        <SelectTrigger>
                          <SelectValue placeholder="Select a channel" />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="none">No default channel</SelectItem>
                          {slackChannels.map((channel) => (
                            <SelectItem key={channel.id} value={channel.id}>
                              #{channel.name} {channel.is_private ? '(private)' : ''}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <p className="text-xs text-muted-foreground">
                        New watches will post alerts to this channel by default. You can override this for individual watches. If you don't see a channel, add the Kyomi app to it in Slack and refresh this page.
                      </p>
                    </>
                  )}
                </div>

                <Button
                  variant="outline"
                  size="sm"
                  onClick={handleSlackDisconnect}
                  disabled={slackDisconnecting}
                >
                  {slackDisconnecting ? (
                    <>
                      <Spinner size="sm" className="mr-2" />
                      Disconnecting...
                    </>
                  ) : (
                    <>
                      <Unplug className="h-4 w-4 mr-2" />
                      Disconnect
                    </>
                  )}
                </Button>
              </div>
            ) : (
              // Slack installed but user not connected
              <div className="space-y-4">
                <p className="text-sm text-muted-foreground">
                  Connect your Slack account to send watch alerts to <strong>{slackStatus.team_name}</strong>.
                </p>
                <div className="space-y-2">
                  <Button
                    onClick={handleSlackConnect}
                    disabled={slackLoading}
                  >
                    <ExternalLink className="h-4 w-4 mr-2" />
                    Connect with Slack
                  </Button>
                  <p className="text-xs text-muted-foreground">
                    Or type <code className="px-1 py-0.5 bg-muted rounded text-xs">/kyomi connect</code> in Slack
                  </p>
                </div>
              </div>
            )}
          </CardContent>
        </Card>}

        {/* Push Notifications Section - hidden in personal mode */}
        {!isPersonalMode && <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Bell className="h-5 w-5" />
              Push Notifications
            </CardTitle>
            <CardDescription>
              Receive watch alerts even when Kyomi is not open in your browser.
            </CardDescription>
          </CardHeader>
          <CardContent>
            {!pushNotifications.supported ? (
              <Alert>
                <AlertDescription>
                  {window.location.protocol !== 'https:' && window.location.hostname !== 'localhost' ? (
                    <>Push notifications require a secure connection (HTTPS). Access Kyomi via HTTPS or localhost to enable browser alerts.</>
                  ) : (
                    <>Push notifications are not supported in this browser.</>
                  )}
                  {/* iOS guidance */}
                  {/iPad|iPhone/.test(navigator.userAgent) && !window.matchMedia('(display-mode: standalone)').matches && (
                    <span className="block mt-1">
                      On iOS, push notifications require installing Kyomi as an app: tap the Share button, then &quot;Add to Home Screen&quot;.
                    </span>
                  )}
                </AlertDescription>
              </Alert>
            ) : pushNotifications.permission === 'denied' ? (
              <Alert variant="warning">
                <AlertDescription>
                  Notification permission was denied. To enable push notifications, allow notifications for this site in your browser settings.
                </AlertDescription>
              </Alert>
            ) : (
              <div className="space-y-4">
                {pushNotifications.error && (
                  <Alert variant="error">
                    <AlertDescription>{pushNotifications.error}</AlertDescription>
                  </Alert>
                )}

                {/* Enable/Disable toggle for this device */}
                <div className="flex items-center justify-between">
                  <div>
                    <p className="text-sm font-medium text-foreground">
                      {pushNotifications.isSubscribed ? 'Enabled on this device' : 'Enable on this device'}
                    </p>
                    <p className="text-xs text-muted-foreground">
                      {pushNotifications.isSubscribed
                        ? 'You will receive push notifications for watch alerts.'
                        : 'Get notified when your watches detect something.'}
                    </p>
                  </div>
                  <Button
                    variant={pushNotifications.isSubscribed ? 'outline' : 'default'}
                    size="sm"
                    onClick={pushNotifications.isSubscribed ? pushNotifications.unsubscribe : pushNotifications.subscribe}
                    disabled={pushNotifications.loading}
                  >
                    {pushNotifications.loading ? (
                      <Spinner size="sm" />
                    ) : pushNotifications.isSubscribed ? (
                      <>
                        <BellOff className="h-4 w-4 mr-2" />
                        Disable
                      </>
                    ) : (
                      <>
                        <Bell className="h-4 w-4 mr-2" />
                        Enable
                      </>
                    )}
                  </Button>
                </div>

                {/* Device list */}
                {pushNotifications.subscriptions.length > 0 && (
                  <div className="space-y-2 pt-3 border-t border-border">
                    <p className="text-xs font-medium text-muted-foreground uppercase tracking-wide">
                      Registered Devices
                    </p>
                    {pushNotifications.subscriptions.map((sub) => (
                      <div
                        key={sub.id}
                        className="flex items-center justify-between p-3 bg-muted/50 rounded-lg"
                      >
                        <div className="flex items-center gap-3">
                          <Smartphone className="h-4 w-4 text-muted-foreground" />
                          <div>
                            <p className="text-sm text-foreground">
                              {sub.device_label || 'Unknown device'}
                            </p>
                            <p className="text-xs text-muted-foreground">
                              Added {new Date(sub.created_at).toLocaleDateString()}
                              {sub.last_used_at && (
                                <> &middot; Last used {new Date(sub.last_used_at).toLocaleDateString()}</>
                              )}
                            </p>
                          </div>
                        </div>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => pushNotifications.deleteSubscription(sub.id)}
                          title="Remove this device"
                        >
                          <Trash2 className="h-4 w-4 text-muted-foreground" />
                        </Button>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}
          </CardContent>
        </Card>}

        {/* MCP Connection Section */}
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Plug className="h-5 w-5" />
              MCP Connection
            </CardTitle>
            <CardDescription>
              Connect Kyomi to any MCP-compatible client for AI-powered data analysis.
            </CardDescription>
          </CardHeader>
          <CardContent>
            {(() => {
                const mcpPort = window.location.port || '3000';
                const mcpUrl = isPersonalMode
                  ? `http://localhost:${mcpPort}/mcp`
                  : (window.location.hostname === 'localhost' || window.location.hostname === '127.0.0.1')
                    ? `http://${window.location.hostname}:8002/mcp`
                    : `${window.location.origin}/mcp`;
                const claudeDesktopConfig = JSON.stringify({
                  mcpServers: {
                    kyomi: {
                      url: mcpUrl
                    }
                  }
                }, null, 2);
                const claudeCodeCommand = `claude mcp add --transport http kyomi http://localhost:${mcpPort}/mcp`;
                return (
                  <div className="space-y-6">
                    {/* MCP Server URL */}
                    <div className="space-y-3">
                      <h4 className="font-medium text-foreground">Server URL</h4>
                      {!isPersonalMode && (
                        <p className="text-sm text-muted-foreground">
                          Use this URL to connect from any MCP client. You'll be prompted to authorize via your browser.
                        </p>
                      )}
                      {isPersonalMode && (
                        <p className="text-sm text-muted-foreground">
                          Use this URL to connect from any MCP client.
                        </p>
                      )}
                      <div className="relative">
                        <pre className="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                          {mcpUrl}
                        </pre>
                        <Button
                          size="sm"
                          variant="ghost"
                          className="absolute top-2 right-2"
                          onClick={() => {
                            navigator.clipboard.writeText(mcpUrl);
                            toast.success('Copied to clipboard!');
                          }}
                        >
                          <Copy className="h-4 w-4" />
                        </Button>
                      </div>
                    </div>

                    {/* Claude Code - only in personal mode */}
                    {isPersonalMode && (
                      <div className="space-y-3 pt-4 border-t border-border">
                        <h4 className="font-medium text-foreground">Claude Code</h4>
                        <p className="text-sm text-muted-foreground">
                          Run this command in your terminal to connect Claude Code.
                        </p>
                        <div className="relative">
                          <pre className="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                            {claudeCodeCommand}
                          </pre>
                          <Button
                            size="sm"
                            variant="ghost"
                            className="absolute top-2 right-2"
                            onClick={() => {
                              navigator.clipboard.writeText(claudeCodeCommand);
                              toast.success('Copied to clipboard!');
                            }}
                          >
                            <Copy className="h-4 w-4" />
                          </Button>
                        </div>
                      </div>
                    )}

                    {/* Claude Desktop - only in personal mode */}
                    {isPersonalMode && (
                      <div className="space-y-3 pt-4 border-t border-border">
                        <h4 className="font-medium text-foreground">Claude Desktop</h4>
                        <p className="text-sm text-muted-foreground">
                          Add this to your Claude Desktop configuration file.
                        </p>
                        <div className="relative">
                          <pre className="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                            {claudeDesktopConfig}
                          </pre>
                          <Button
                            size="sm"
                            variant="ghost"
                            className="absolute top-2 right-2"
                            onClick={() => {
                              navigator.clipboard.writeText(claudeDesktopConfig);
                              toast.success('Copied to clipboard!');
                            }}
                          >
                            <Copy className="h-4 w-4" />
                          </Button>
                        </div>
                      </div>
                    )}

                    {/* Cursor One-Click */}
                    <div className="space-y-3 pt-4 border-t border-border">
                      <h4 className="font-medium text-foreground">Cursor</h4>
                      <p className="text-sm text-muted-foreground">
                        One-click install for Cursor users.
                      </p>
                      <Button
                        variant="outline"
                        onClick={() => {
                          const config = { type: "http", url: mcpUrl };
                          const encodedConfig = btoa(JSON.stringify(config));
                          window.open(`cursor://anysphere.cursor-deeplink/mcp/install?name=kyomi&config=${encodedConfig}`, '_blank');
                        }}
                      >
                        <ExternalLink className="h-4 w-4 mr-2" />
                        Connect with Cursor
                      </Button>
                    </div>
                  </div>
                );
              })()
            }
          </CardContent>
        </Card>

        {/* AI Provider Section - Only in personal mode */}
        {isPersonalMode && (
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <Bot className="h-5 w-5" />
                AI Provider
              </CardTitle>
              <CardDescription>
                Configure an LLM provider for built-in chat and automated watches.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <AIProviderSettings />
            </CardContent>
          </Card>
        )}

        {/* Pending Invitations Section - Only show when there are actual invitations, hidden in personal mode */}
        {!isPersonalMode && invitations.length > 0 && (
          <Card>
            <CardHeader>
              <CardTitle>Pending Workspace Invitations</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="border border-border rounded-lg overflow-hidden">
                <table className="min-w-full divide-y divide-border">
                  <thead className="bg-muted">
                    <tr>
                      <th className="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Workspace
                      </th>
                      <th className="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Invited By
                      </th>
                      <th className="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Role
                      </th>
                      <th className="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Invited
                      </th>
                      <th className="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Expires
                      </th>
                      <th className="px-6 py-3 text-right text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Actions
                      </th>
                    </tr>
                  </thead>
                  <tbody className="bg-card divide-y divide-border">
                    {invitations.map((invitation) => (
                      <tr key={invitation.invitation_id}>
                        <td className="px-6 py-4 whitespace-nowrap">
                          <div className="text-sm font-medium text-foreground">
                            {invitation.workspace_name || 'Unnamed Workspace'}
                          </div>
                          <div className="text-xs text-muted-foreground">
                            {invitation.workspace_id}
                          </div>
                        </td>
                        <td className="px-6 py-4 whitespace-nowrap text-sm text-foreground">
                          {invitation.invited_by_name || 'Unknown'}
                        </td>
                        <td className="px-6 py-4 whitespace-nowrap text-sm">
                          <Badge variant={invitation.role === 'admin' ? 'secondary' : 'default'}>
                            {invitation.role}
                          </Badge>
                        </td>
                        <td className="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                          {new Date(invitation.created_at).toLocaleDateString()}
                        </td>
                        <td className="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                          {new Date(invitation.expires_at).toLocaleDateString()}
                        </td>
                        <td className="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={async () => {
                              try {
                                await apiClient.post(`/api/v1/workspaces/invitations/${invitation.invitation_id}/accept`);
                                await refreshUser();
                                await fetchUserPendingInvitations();
                                toast.success('Successfully joined workspace! You can now switch to it using the workspace switcher.');
                              } catch (error) {
                                toast.error(`Failed to accept invitation: ${error.response?.data?.detail || error.message}`);
                              }
                            }}
                            className="mr-2"
                          >
                            Accept
                          </Button>
                          <Button
                            variant="destructive"
                            size="sm"
                            onClick={async () => {
                              const confirmed = await confirm({
                                title: 'Decline Invitation?',
                                message: 'Are you sure you want to decline this invitation?',
                                confirmText: 'Decline',
                                variant: 'destructive'
                              });

                              if (confirmed) {
                                try {
                                  await apiClient.post(`/api/v1/workspaces/invitations/${invitation.invitation_id}/decline`);
                                  await fetchUserPendingInvitations();
                                } catch (error) {
                                  toast.error(`Failed to decline invitation: ${error.response?.data?.detail || error.message}`);
                                }
                              }
                            }}
                          >
                            Decline
                          </Button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </CardContent>
          </Card>
        )}

        {/* Chart Preferences Section */}
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <div>
                <CardTitle>Default Chart Palette</CardTitle>
                <CardDescription>Choose the default color palette for your charts. This overrides workspace defaults.</CardDescription>
              </div>
              <SaveStatusIndicator status={saveStatus.chartPalette} />
            </div>
          </CardHeader>
          <CardContent>
            <div className="space-y-3">
              {[
                { id: 'balanced', name: 'Balanced', colors: CHART_PALETTES.balanced },
                { id: 'vibrant', name: 'Vibrant', colors: CHART_PALETTES.vibrant },
                { id: 'accessible', name: 'Accessible', colors: CHART_PALETTES.accessible }
              ].map((palette) => (
                <button
                  key={palette.id}
                  onClick={() => {
                    setUserPalette(palette.id);
                    autoSaveChartPalette(palette.id);
                  }}
                  disabled={userPalette === null}
                  className={`w-full text-left p-4 rounded-lg border-2 transition-all ${
                    userPalette === palette.id
                      ? 'border-primary bg-primary/10'
                      : 'border-border hover:border-border/80'
                  } ${userPalette === null ? 'opacity-50 cursor-wait' : ''}`}
                >
                  <div className="flex items-start justify-between mb-3">
                    <div className="font-medium text-foreground">{palette.name}</div>
                    {userPalette === palette.id && (
                      <CheckCircle className="text-primary" size={20} />
                    )}
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {palette.colors.map((color, idx) => (
                      <Tooltip key={idx}>
                        <TooltipTrigger asChild>
                          <div
                            className="w-8 h-8 rounded border border-border cursor-help"
                            style={{ backgroundColor: color }}
                          />
                        </TooltipTrigger>
                        <TooltipContent>{color}</TooltipContent>
                      </Tooltip>
                    ))}
                  </div>
                </button>
              ))}
            </div>
          </CardContent>
        </Card>

        {/* SQL Query History Section */}
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <div>
                <CardTitle>SQL Query History</CardTitle>
                <CardDescription>Starred queries are never deleted automatically. Unstarred queries will be removed after the selected period.</CardDescription>
              </div>
              <SaveStatusIndicator status={saveStatus.queryRetention} />
            </div>
          </CardHeader>
          <CardContent>
            <div>
              <label className="block text-sm font-medium text-foreground mb-2">
                Auto-delete unstarred queries after
              </label>
              <select
                value={queryHistoryRetentionDays}
                onChange={(e) => {
                  const days = Number(e.target.value);
                  setQueryHistoryRetentionDays(days);
                  autoSaveQueryRetention(days);
                }}
                className="px-3 py-2 border border-input rounded-lg text-sm bg-background text-foreground focus:outline-none focus:ring-2 focus:ring-ring"
              >
                {capabilities?.query_history_retention_days >= 1 && <option value={1}>1 day</option>}
                {capabilities?.query_history_retention_days >= 7 && <option value={7}>7 days</option>}
                {capabilities?.query_history_retention_days >= 30 && <option value={30}>30 days</option>}
                {capabilities?.query_history_retention_days >= 90 && <option value={90}>90 days (3 months)</option>}
                {(capabilities?.query_history_retention_days >= 365 || capabilities?.query_history_retention_days === 0) && <option value={365}>365 days (1 year)</option>}
              </select>
              {capabilities?.query_history_retention_days > 0 && capabilities?.query_history_retention_days < 365 && (
                <p className="text-xs text-muted-foreground mt-2">
                  Your {capabilities?.subscription_tier || 'current'} plan allows up to {capabilities?.query_history_retention_days} days. Upgrade for longer retention.
                </p>
              )}
            </div>
          </CardContent>
        </Card>

        {/* NOTE: Google Account reconnection is handled in Datasource Settings (BigQuery).
            Sign-in uses our own JWT tokens - Google tokens are ONLY needed for BigQuery access. */}
      </div>

      {/* Confirm Dialog */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
    </div>
  );
}
