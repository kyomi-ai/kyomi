// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { useAuth } from '../context/AuthContext';
import { Card } from '../components/ui/card';
import { Button } from '../components/ui/button';
import { Badge } from '../components/ui/badge';
import { DatasourceIcon } from '../components/ui/DatasourceIcon';
import { DatasourceModal } from '../components/settings/datasources';
import { Spinner } from '../components/ui/spinner';
import { Key, Clock, Database, ExternalLink } from 'lucide-react';
import { toast } from '../lib/toast';
import { trackEvent } from '../utils/analytics';

/**
 * DatasourceOnboarding - Unified onboarding for all auth methods
 *
 * Routes users to the appropriate onboarding experience based on their role
 * and the workspace state:
 *
 * 1. Admin with no datasources → Create datasource modal
 * 2. Invited user with existing datasources → Credential setup UI
 * 3. User with all datasources ready → Redirect to /chat
 * 4. Non-admin with no datasources → Waiting message
 */
export default function DatasourceOnboarding() {
  const navigate = useNavigate();
  const { user, apiClient } = useAuth();

  // Loading and checking state
  const [isCheckingDatasources, setIsCheckingDatasources] = useState(true);

  // Modal state for create datasource flow
  const [showModal, setShowModal] = useState(false);

  // Credential setup state for invited users
  const [needsCredentials, setNeedsCredentials] = useState(false);
  const [credentialStatus, setCredentialStatus] = useState(null);
  const [oauthConnecting, setOauthConnecting] = useState(null);

  // Non-admin waiting state (no datasources in workspace)
  const [isWaiting, setIsWaiting] = useState(false);

  // Choice card state (admin with no datasources)
  const [showChoice, setShowChoice] = useState(false);
  const [sampleAvailable, setSampleAvailable] = useState(false);
  const [creatingSample, setCreatingSample] = useState(false);

  // Check if user is workspace admin/owner
  const isAdmin = user?.workspace_roles?.includes('workspace_admin');
  const isOwner = user?.is_owner || false;
  const canAdmin = isAdmin || isOwner;

  // Listen for OAuth popup completion
  useEffect(() => {
    const handleOAuthMessage = async (event) => {
      // Verify origin
      if (event.origin !== window.location.origin) return;

      // BigQuery OAuth messages (kyomi_oauth via global Google OAuth)
      if (event.data?.type === 'GOOGLE_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('BigQuery connected successfully');
        await recheckCredentialStatus();
      } else if (event.data?.type === 'GOOGLE_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect BigQuery');
      }

      // BigQuery Enterprise OAuth messages
      if (event.data?.type === 'BIGQUERY_ENTERPRISE_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('BigQuery connected successfully');
        await recheckCredentialStatus();
      } else if (event.data?.type === 'BIGQUERY_ENTERPRISE_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect BigQuery');
      }

      // Snowflake OAuth messages
      if (event.data?.type === 'SNOWFLAKE_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('Snowflake connected successfully');
        await recheckCredentialStatus();
      } else if (event.data?.type === 'SNOWFLAKE_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect Snowflake');
      }

      // Microsoft Enterprise OAuth messages (for Azure Synapse enterprise OAuth)
      if (event.data?.type === 'MICROSOFT_ENTERPRISE_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('Azure Synapse connected successfully');
        await recheckCredentialStatus();
      } else if (event.data?.type === 'MICROSOFT_ENTERPRISE_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect Azure Synapse');
      }

      // Databricks OAuth messages
      if (event.data?.type === 'DATABRICKS_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('Databricks connected successfully');
        await recheckCredentialStatus();
      } else if (event.data?.type === 'DATABRICKS_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect Databricks');
      }
    };

    window.addEventListener('message', handleOAuthMessage);
    return () => window.removeEventListener('message', handleOAuthMessage);
  }, [apiClient]);

  // Re-check credential status after OAuth connection
  const recheckCredentialStatus = async () => {
    if (!apiClient) return;
    try {
      const response = await apiClient.get('/api/v1/datasources/credential-status');
      const { summary } = response.data;

      if (summary.needs_credentials === 0) {
        // All credentials ready - proceed to chat
        navigate('/chat', { replace: true });
      } else {
        // Update the credential status display
        setCredentialStatus(response.data);
      }
    } catch (error) {
      // Add user-facing error handling so they know to refresh if needed
      toast.error('Connected successfully, but failed to update status. Please refresh the page.');
    }
  };

  // Check workspace state and route to appropriate experience
  useEffect(() => {
    const checkWorkspaceState = async () => {
      try {
        // First check if datasources exist in the workspace
        const datasourcesResponse = await apiClient.get('/api/v1/datasources');
        const datasources = datasourcesResponse.data || [];

        if (datasources.length > 0) {
          // Workspace has datasources - check if user needs to provide credentials
          const credentialResponse = await apiClient.get('/api/v1/datasources/credential-status');
          const { summary } = credentialResponse.data;

          if (summary.needs_credentials > 0) {
            // User needs to provide credentials for some datasources
            setCredentialStatus(credentialResponse.data);
            setNeedsCredentials(true);
            setIsCheckingDatasources(false);
            return;
          }

          // All credentials ready - redirect to chat
          navigate('/chat', { replace: true });
          return;
        }

        // No datasources in workspace
        if (canAdmin) {
          // Check if sample datasource is available
          try {
            const sampleRes = await apiClient.get('/api/v1/datasources/sample/available');
            if (sampleRes.data.configured && !sampleRes.data.already_added) {
              setSampleAvailable(true);
            }
          } catch {
            // Sample not available - that's fine
          }
          // Admin with no datasources - show choice card
          setShowChoice(true);
          setIsCheckingDatasources(false);
        } else {
          // Non-admin with no datasources - show waiting message
          setIsWaiting(true);
          setIsCheckingDatasources(false);
        }
      } catch (error) {
        // On error, default to showing choice card for admins, waiting for others
        if (canAdmin) {
          setShowChoice(true);
        } else {
          setIsWaiting(true);
        }
        setIsCheckingDatasources(false);
      }
    };

    if (apiClient) {
      checkWorkspaceState();
    }
  }, [apiClient, navigate, canAdmin]);

  // Handle OAuth connect for datasources that need OAuth authentication
  const handleOAuthConnect = (datasource) => {
    const config = datasource.connection_config || {};
    const authMode = config.auth_mode;
    let url;


    if (datasource.datasource_type === 'bigquery') {
      // Handle service_account auth mode - should never reach OAuth flow
      if (authMode === 'service_account') {
        toast.error('Service account authentication does not use OAuth. Please contact your admin.');
        return;
      }

      if (authMode === 'enterprise_oauth') {
        // Enterprise OAuth - use per-datasource credentials
        url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
      } else if (authMode === 'kyomi_oauth' || !authMode) {
        // Kyomi OAuth (default) - use global Google OAuth
        url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/google-oauth/connect`;
      } else {
        toast.error(`Unknown authentication mode: ${authMode}`);
        return;
      }
    } else if (datasource.datasource_type === 'snowflake') {
      url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/snowflake/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
    } else if (datasource.datasource_type === 'synapse') {
      // Azure Synapse enterprise OAuth (customer's Azure AD app)
      url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/microsoft-enterprise/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
    } else if (datasource.datasource_type === 'databricks') {
      url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/databricks/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
    } else {
      toast.error('OAuth not supported for this datasource type');
      return;
    }

    setOauthConnecting(datasource.id);

    const width = 500;
    const height = 600;
    const left = window.screenX + (window.outerWidth - width) / 2;
    const top = window.screenY + (window.outerHeight - height) / 2;

    const popup = window.open(
      url,
      'oauth-connect',
      `width=${width},height=${height},left=${left},top=${top},popup=1`
    );

    if (!popup || popup.closed) {
      setOauthConnecting(null);
      toast.error('Popup was blocked. Please allow popups for this site.');
      return;
    }

    // Monitor popup for manual close
    const checkPopup = setInterval(() => {
      if (popup.closed) {
        clearInterval(checkPopup);
        setOauthConnecting((prev) => {
          if (prev === datasource.id) return null;
          return prev;
        });
      }
    }, 500);
  };

  // Handle password credential setup - redirect to datasource settings
  const handlePasswordCredentialSetup = (datasource) => {
    // Navigate to datasource settings page with datasource pre-selected for credential entry
    navigate('/settings/datasources', { state: { openDatasource: datasource.slug } });
  };

  // Handle skip for credential setup
  const handleSkipCredentials = () => {
    navigate('/chat');
  };

  // Handle "Explore with Sample Data" from choice card
  const handleExploreSample = async () => {
    trackEvent('onboarding_choice', { props: { choice: 'sample_data' } });
    setCreatingSample(true);
    try {
      await apiClient.post('/api/v1/datasources/sample');
      toast.success('Sample datasource added — start exploring!');
      navigate('/chat', { replace: true });
    } catch (error) {
      const detail = error.response?.data?.detail;
      toast.error(detail || 'Failed to add sample datasource');
      setCreatingSample(false);
    }
  };

  // Handle "Connect Your Own Database" from choice card
  const handleConnectOwn = () => {
    trackEvent('onboarding_choice', { props: { choice: 'connect_own' } });
    setShowChoice(false);
    setShowModal(true);
  };

  // Handle modal saved (datasource created)
  const handleSaved = () => {
    navigate('/chat');
  };

  // Handle skip for create datasource
  const handleSkip = () => {
    trackEvent('onboarding_choice', { props: { choice: 'skip' } });
    navigate('/chat');
  };

  // Handle modal close
  const handleModalClose = () => {
    setShowModal(false);
  };

  // Get button configuration for a datasource
  const getConnectButton = (datasource) => {
    const isConnecting = oauthConnecting === datasource.id;
    const { auth_method, oauth_provider, credential_status } = datasource;

    if (auth_method === 'oauth') {
      const providerLabel = oauth_provider === 'google' ? 'Google' :
                           oauth_provider === 'snowflake' ? 'Snowflake' :
                           oauth_provider === 'microsoft' ? 'Microsoft' :
                           oauth_provider === 'databricks' ? 'Databricks' :
                           oauth_provider || 'OAuth';

      if (credential_status === 'expired') {
        return {
          text: isConnecting ? 'Reconnecting...' : `Reconnect ${providerLabel}`,
          icon: isConnecting ? Spinner : Clock,
          handler: () => handleOAuthConnect(datasource),
          disabled: isConnecting,
          variant: 'outline',
        };
      }

      return {
        text: isConnecting ? 'Connecting...' : `Connect with ${providerLabel}`,
        useDatasourceIcon: !isConnecting,
        icon: isConnecting ? Spinner : null,
        handler: () => handleOAuthConnect(datasource),
        disabled: isConnecting,
        variant: 'default',
      };
    }

    if (auth_method === 'password') {
      return {
        text: 'Enter Credentials',
        icon: Key,
        handler: () => handlePasswordCredentialSetup(datasource),
        disabled: false,
        variant: 'default',
      };
    }

    return null;
  };

  // Show loading while checking workspace state
  if (isCheckingDatasources) {
    return (
      <div className="min-h-screen bg-background flex items-center justify-center">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    );
  }

  // Show waiting message for non-admins with no datasources
  if (isWaiting) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="max-w-lg w-full p-8">
          <div className="text-center mb-6">
            <h1 className="text-3xl font-bold mb-2">Waiting for Setup</h1>
            <p className="text-muted-foreground">
              Your workspace administrator needs to configure datasources before you can start.
            </p>
          </div>
          <p className="text-sm text-muted-foreground text-center mb-6">
            Please contact your workspace admin to set up the database connections.
            Once they have configured the datasources, you will be able to connect
            your credentials and start using Kyomi.
          </p>
          <Button variant="ghost" onClick={() => navigate('/chat')} className="w-full">
            Go to Chat anyway
          </Button>
        </Card>
      </div>
    );
  }

  // Show credential setup UI for invited users with existing datasources
  if (needsCredentials && credentialStatus) {
    const datasourcesNeedingSetup = credentialStatus.datasources.filter(
      ds => ds.credential_status === 'missing' || ds.credential_status === 'expired'
    );

    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="max-w-2xl w-full p-8">
          <div className="text-center mb-6">
            <h1 className="text-3xl font-bold mb-2">Set Up Your Credentials</h1>
            <p className="text-muted-foreground">
              Your workspace has {credentialStatus.summary.total} datasource{credentialStatus.summary.total !== 1 ? 's' : ''} configured.
              Please provide your credentials to access them.
            </p>
          </div>

          <div className="space-y-3 mb-6">
            {datasourcesNeedingSetup.map(ds => {
              const buttonConfig = getConnectButton(ds);

              return (
                <div key={ds.id} className="flex items-center justify-between p-4 border border-border rounded-xl bg-card">
                  <div className="flex items-center gap-3">
                    <DatasourceIcon type={ds.datasource_type} className="w-8 h-8" opacity={1} />
                    <div>
                      <div className="flex items-center gap-2">
                        <span className="font-medium">{ds.name}</span>
                        {ds.credential_status === 'expired' && (
                          <Badge variant="warning" className="text-xs">Expired</Badge>
                        )}
                      </div>
                      <div className="text-sm text-muted-foreground capitalize">{ds.datasource_type}</div>
                    </div>
                  </div>
                  {buttonConfig && (
                    <Button
                      variant={buttonConfig.variant}
                      onClick={buttonConfig.handler}
                      disabled={buttonConfig.disabled}
                    >
                      {buttonConfig.useDatasourceIcon ? (
                        <DatasourceIcon
                          type={ds.datasource_type}
                          className="h-4 w-4 mr-2"
                          opacity={1}
                        />
                      ) : buttonConfig.icon ? (
                        <buttonConfig.icon
                          className="h-4 w-4 mr-2"
                        />
                      ) : null}
                      {buttonConfig.text}
                    </Button>
                  )}
                </div>
              );
            })}
          </div>

          <Button variant="ghost" onClick={handleSkipCredentials} className="w-full">
            Skip for now
          </Button>
        </Card>
      </div>
    );
  }

  // Show binary choice card (admin with no datasources)
  if (showChoice) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="max-w-xl w-full p-8">
          <div className="text-center mb-8">
            <h1 className="text-3xl font-bold mb-2">Welcome to Kyomi!</h1>
            <p className="text-muted-foreground">
              Choose how you'd like to get started
            </p>
          </div>

          <div className="space-y-4">
            {/* Option 1: Explore with sample data */}
            {sampleAvailable && (
              <div className="border border-border rounded-xl p-5">
                <div className="flex items-start gap-4">
                  <Database className="h-6 w-6 mt-0.5 text-muted-foreground flex-shrink-0" />
                  <div className="flex-1">
                    <h3 className="font-semibold mb-1">Explore with Sample Data</h3>
                    <p className="text-sm text-muted-foreground mb-3">
                      Dive in with our Acme Analytics demo dataset — no setup required
                    </p>
                    <Button
                      onClick={handleExploreSample}
                      disabled={creatingSample}
                      className="w-full"
                    >
                      {creatingSample ? 'Setting up...' : 'Start Exploring'}
                    </Button>
                  </div>
                </div>
              </div>
            )}

            {/* Option 2: Connect own database */}
            <div className="border border-border rounded-xl p-5">
              <div className="flex items-start gap-4">
                <Database className="h-6 w-6 mt-0.5 text-muted-foreground flex-shrink-0" />
                <div className="flex-1">
                  <h3 className="font-semibold mb-1">Connect Your Own Database</h3>
                  <p className="text-sm text-muted-foreground mb-3">
                    Connect your data warehouse to ask questions about your real data
                  </p>
                  <Button
                    onClick={handleConnectOwn}
                    variant={sampleAvailable ? 'outline' : 'default'}
                    className="w-full"
                  >
                    Connect Datasource
                  </Button>
                </div>
              </div>
            </div>
          </div>

          <p className="text-xs text-center text-muted-foreground mt-6">
            You can always change this later in Settings
          </p>
        </Card>
      </div>
    );
  }

  // If modal was closed without creating a datasource, show choice card again
  if (!showModal) {
    // Go back to the choice card
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="max-w-lg w-full p-8">
          <div className="text-center mb-6">
            <h1 className="text-3xl font-bold mb-2">Welcome to Kyomi!</h1>
            <p className="text-muted-foreground">
              Connect a datasource to start asking questions about your data.
            </p>
          </div>

          <div className="space-y-4">
            <Button
              onClick={() => setShowModal(true)}
              className="w-full"
              size="lg"
            >
              Connect Datasource
            </Button>
            <Button
              onClick={handleSkip}
              variant="ghost"
              className="w-full"
            >
              Skip for now
            </Button>
            <p className="text-xs text-center text-muted-foreground">
              You can always add datasources later in Settings.
            </p>
          </div>
        </Card>
      </div>
    );
  }

  // Render DatasourceModal for create mode (admin with no datasources)
  return (
    <div className="min-h-screen bg-background flex items-center justify-center">
      <DatasourceModal
        isOpen={showModal}
        onClose={handleModalClose}
        apiClient={apiClient}
        canAdmin={canAdmin}
        title="Connect Your First Datasource"
        onSaved={handleSaved}
        user={user}
      />
    </div>
  );
}
