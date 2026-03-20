// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useAuth } from '../context/AuthContext';
import { useCapabilities } from '../context/CapabilitiesContext';
import { useSystemConfig } from '../context/SystemConfigContext';
import { useNavigate, useLocation } from 'react-router-dom';
import { CreditCard, Users, Settings, User, Shield, BarChart3, Activity, Server } from 'lucide-react';
import { Spinner } from '../components/ui/spinner';
import { Button } from '../components/ui/button';
import { Badge } from '../components/ui/badge';
import { Alert, AlertTitle, AlertDescription } from '../components/ui/alert';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '../components/ui/card';
import { Tooltip, TooltipTrigger, TooltipContent } from '../components/ui/tooltip';
import BillingPanel from '../components/BillingPanel';
import UsagePanel from '../components/UsagePanel';
import ProfileSettings from '../components/settings/ProfileSettings';
import WorkspaceSettings from '../components/settings/WorkspaceSettings';
import TeamManagement from '../components/settings/TeamManagement';
import DatasourceSettings from '../components/settings/DatasourceSettings';
import AnalyticsSettings from '../components/settings/AnalyticsSettings';
import SessionManagement from '../components/SessionManagement';
import PasskeyManager from '../components/PasskeyManager';
import PasswordManager from '../components/PasswordManager';
import TwoFactorAuth from '../components/TwoFactorAuth';
import { toast } from '../lib/toast';

export default function SettingsContent() {
  const { user, apiClient, refreshUser } = useAuth();
  const { capabilities } = useCapabilities();
  const { isPersonalMode } = useSystemConfig();
  const navigate = useNavigate();
  const location = useLocation();

  // Initialize activeTab from URL path (e.g., /settings/profile -> 'profile')
  const pathSegments = location.pathname.split('/');
  const tabFromUrl = pathSegments[2] || 'profile'; // default to 'profile' if no tab in URL
  const [activeTab, setActiveTab] = useState(tabFromUrl);
  const [workspaceInfo, setWorkspaceInfo] = useState(null);

  // Update activeTab when URL changes (browser back/forward)
  useEffect(() => {
    const pathSegments = location.pathname.split('/');
    const newTab = pathSegments[2] || 'profile';
    if (newTab !== activeTab) {
      setActiveTab(newTab);
    }
  }, [location.pathname]);

  // Function to change tab and update URL
  const handleTabChange = (tabId) => {
    setActiveTab(tabId);
    navigate(`/settings/${tabId}`, { replace: true });
  };
  const [loading, setLoading] = useState(true);
  const [connections, setConnections] = useState([]);

  // LLM settings
  const [selectedModel, setSelectedModel] = useState('claude-haiku-4-5-latest');
  const [availableModels, setAvailableModels] = useState([]);

  // Chart Preferences state (workspace level)
  const [workspacePalette, setWorkspacePalette] = useState(null);
  const [savingWorkspacePrefs, setSavingWorkspacePrefs] = useState(false);

  // Fetch workspace information and Google projects from API
  useEffect(() => {
    const fetchTenantInfo = async () => {
      try {
        const response = await apiClient.get('/api/v1/workspaces/settings');
        if (response.data) {
          setWorkspaceInfo(response.data);
          // Load database connections if exist
          if (response.data.database_connections) {
            setConnections(response.data.database_connections);
          }
          // Load selected model from workspace settings
          if (response.data.default_model) {
            setSelectedModel(response.data.default_model);
          }
        }
      } catch (error) {
      } finally {
        setLoading(false);
      }
    };

    const fetchChartPreferences = async () => {
      try {
        // Fetch workspace config and extract palette
        const workspaceResponse = await apiClient.get('/api/v1/workspaces/chartml-config');
        if (workspaceResponse.data && workspaceResponse.data.config && workspaceResponse.data.config.style) {
          setWorkspacePalette(workspaceResponse.data.config.style);
        } else {
          // No saved preference, use system default
          setWorkspacePalette('autumn_forest');
        }
      } catch (error) {
        // On error, fall back to system defaults
        setWorkspacePalette('autumn_forest');
      }
    };

    const loadAvailableModels = async () => {
      try {
        const response = await apiClient.getAvailableModels();
        if (response.models) {
          // Flatten the models into a single list with provider prefixes
          const allModels = [];

          // Add Claude models
          if (response.models.claude) {
            response.models.claude.forEach(model => {
              allModels.push({
                id: model,
                label: `Claude: ${model.replace('claude-', '').replace('-latest', ' (latest)')}`,
                provider: 'claude'
              });
            });
          }

          // Add Ollama models
          if (response.models.ollama) {
            response.models.ollama.forEach(model => {
              allModels.push({
                id: model,
                label: `Ollama: ${model}`,
                provider: 'ollama'
              });
            });
          }

          setAvailableModels(allModels);
        }
      } catch (error) {
      }
    };

    if (user && apiClient) {
      fetchTenantInfo();
      loadAvailableModels();
      fetchChartPreferences();
    }
  }, [user, apiClient]);

  const saveWorkspaceChartPreferences = async () => {
    setSavingWorkspacePrefs(true);
    try {
      // Generate ChartML config from palette selection
      const config = {
        type: 'config',
        version: 1,
        style: workspacePalette
      };

      await apiClient.put('/api/v1/workspaces/chartml-config', {
        config: config
      });
    } catch (error) {
      toast.error('Failed to save preferences: ' + (error.response?.data?.detail || error.message));
    } finally {
      setSavingWorkspacePrefs(false);
    }
  };

  const saveModelSettings = async () => {
    try {
      await apiClient.post('/api/v1/workspaces/model-settings', {
        default_model: selectedModel
      });
    } catch (error) {
    }
  };

  // Check if user is workspace admin and owner
  const isAdmin = user?.workspace_roles?.includes('workspace_admin');
  const isOwner = user?.is_owner || false;

  // Redirect non-admins/non-owners away from restricted tabs
  useEffect(() => {
    const adminOnlyTabs = ['workspace', 'team', 'analytics'];
    const ownerOnlyTabs = ['billing'];

    if ((adminOnlyTabs.includes(activeTab) && !isAdmin) ||
        (ownerOnlyTabs.includes(activeTab) && !isOwner)) {
      navigate('/settings/profile', { replace: true });
    }
  }, [activeTab, isAdmin, isOwner, navigate]);

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12">
        <div className="text-center">
          <Spinner size="lg" className="text-primary mx-auto mb-4" />
          <p className="text-muted-foreground">Loading settings...</p>
        </div>
      </div>
    );
  }

  // Determine current state and available tabs
  // For demo mode, assume billing is enabled to showcase post-billing features
  const isDemoMode = import.meta.env.MODE === 'development';
  // Self-hosted mode: capabilities returns billing_enabled=false, hide billing UI entirely
  const selfHosted = capabilities?.billing_enabled === false && !isDemoMode;
  const billingEnabled = selfHosted ? false : (workspaceInfo?.billing_enabled || isDemoMode);

  const canSetupOrganization = billingEnabled && !workspaceInfo?.multi_user_enabled;
  const isOrganization = workspaceInfo?.multi_user_enabled;

  // Check if Team tier or higher (team, enterprise) - use user object for immediate availability
  const isTeamTier = user?.subscription_tier && ['team', 'enterprise'].includes(user.subscription_tier);

  const availableTabs = [
    { id: 'profile', name: 'Profile', icon: User, available: true },
    { id: 'security', name: 'Security', icon: Shield, available: !isPersonalMode },
    { id: 'workspace', name: 'Workspace', icon: Settings, available: isAdmin && !isPersonalMode },
    { id: 'datasources', name: 'Data Sources', icon: Server, available: true },
    { id: 'analytics', name: 'Analytics', icon: Activity, available: isAdmin && !selfHosted },
    { id: 'usage', name: 'Usage', icon: BarChart3, available: !selfHosted },
    { id: 'billing', name: 'Billing', icon: CreditCard, available: isOwner && !selfHosted },
    { id: 'team', name: 'Team', icon: Users, available: isTeamTier && isAdmin && !isPersonalMode }
  ];

  const visibleTabs = availableTabs.filter(tab => tab.available);


  return (
    <div className="w-full space-y-8" style={{display: 'block'}}>
      {/* Settings Header */}
      <div className="mb-8">
        <h1 className="text-3xl font-bold text-foreground">Settings</h1>
        <p className="text-muted-foreground mt-2">Manage your workspace configuration and billing settings</p>
      </div>

      {/* Settings Navigation Tabs */}
      <div className="w-full bg-card rounded-xl shadow-sm border border-border mb-6 overflow-hidden">
        <div className="border-b border-border overflow-x-auto scrollbar-thin scrollbar-thumb-muted-foreground/30 scrollbar-track-transparent">
          <div className="flex space-x-4 md:space-x-8 px-4 md:px-6 min-w-max">
            {visibleTabs.map((tab) => {
              const Icon = tab.icon;
              const isActive = activeTab === tab.id;
              return (
                <button
                  key={tab.id}
                  onClick={() => handleTabChange(tab.id)}
                  className={`flex items-center space-x-2 py-4 border-b-2 font-medium text-sm transition-colors whitespace-nowrap flex-shrink-0 ${
                    isActive
                      ? 'border-primary text-primary'
                      : 'border-transparent text-muted-foreground hover:text-foreground hover:border-border'
                  }`}
                >
                  <Icon size={16} />
                  <span>{tab.name}</span>
                </button>
              );
            })}
          </div>
        </div>
      </div>

      {/* Settings Content */}
      <div className="w-full bg-card rounded-xl shadow-sm border border-border">
        {/* Profile Tab */}
        {activeTab === 'profile' && (
          <ProfileSettings
            user={user}
            apiClient={apiClient}
            refreshUser={refreshUser}
            capabilities={capabilities}
          />
        )}

        {/* Security Tab */}
        {activeTab === 'security' && (
          <div className="p-6">
            <h2 className="text-xl font-semibold text-foreground mb-6">Security</h2>
            <div className="space-y-6">
              <PasswordManager
                user={user}
                apiClient={apiClient}
                onPasswordUpdate={refreshUser}
              />
              <TwoFactorAuth
                apiClient={apiClient}
              />
              <PasskeyManager />
              <SessionManagement />
            </div>
          </div>
        )}

        {/* Usage Tab */}
        {activeTab === 'usage' && (
          <div className="p-6">
            <UsagePanel />
          </div>
        )}

        {/* Billing Tab */}
        {activeTab === 'billing' && (
          <>
            {!isOwner ? (
              <div className="p-6">
                <Alert variant="error">
                  <AlertTitle>Access Denied</AlertTitle>
                  <AlertDescription>
                    You must be the workspace owner to access billing settings.
                  </AlertDescription>
                </Alert>
              </div>
            ) : (
              <div className="p-6">
                <BillingPanel />
              </div>
            )}
          </>
        )}

        {/* Team Tab */}
        {activeTab === 'team' && (
          <>
            {!isAdmin || !isTeamTier ? (
              <div className="p-6">
                <Alert variant="error">
                  <AlertTitle>Access Denied</AlertTitle>
                  <AlertDescription>
                    {!isTeamTier
                      ? "Team management is only available on Team and Enterprise plans."
                      : "You must be a workspace administrator to manage team members."
                    }
                  </AlertDescription>
                </Alert>
              </div>
            ) : (
              <TeamManagement
                user={user}
                apiClient={apiClient}
                workspaceInfo={workspaceInfo}
              />
            )}
          </>
        )}

        {/* Workspace Tab */}
        {activeTab === 'workspace' && (
          <>
            {!isAdmin ? (
              <div className="p-6">
                <Alert variant="error">
                  <AlertTitle>Access Denied</AlertTitle>
                  <AlertDescription>
                    You must be a workspace administrator to access workspace settings.
                  </AlertDescription>
                </Alert>
              </div>
            ) : (
              <WorkspaceSettings
                user={user}
                apiClient={apiClient}
              />
            )}
          </>
        )}

        {/* Data Sources Tab */}
        {activeTab === 'datasources' && (
          <DatasourceSettings
            apiClient={apiClient}
            isAdmin={isAdmin}
            isOwner={isOwner}
            user={user}
          />
        )}

        {/* Analytics Tab */}
        {activeTab === 'analytics' && (
          <>
            {!isAdmin ? (
              <div className="p-6">
                <Alert variant="error">
                  <AlertTitle>Access Denied</AlertTitle>
                  <AlertDescription>
                    You must be a workspace administrator to manage analytics settings.
                  </AlertDescription>
                </Alert>
              </div>
            ) : (
              <AnalyticsSettings />
            )}
          </>
        )}

      </div>
    </div>
  );
}