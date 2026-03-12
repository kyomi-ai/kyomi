// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useSearchParams, useNavigate } from 'react-router-dom';
import {
  Database,
  Plus,
  Trash2,
  Settings,
  AlertTriangle,
  RefreshCw,
  Key,
  BarChart3,
} from 'lucide-react';
import { Spinner } from '../ui/spinner';
import { Button } from '../ui/button';
import { Badge } from '../ui/badge';
import { Card, CardContent } from '../ui/card';
import { Skeleton } from '../ui/skeleton';
import { Switch } from '../ui/switch';
import ConfirmDialog from '../ConfirmDialog';
import { toast } from '../../lib/toast';
import { DatasourceIcon } from '../ui/DatasourceIcon';
import { DatasourceModal } from './datasources';

/**
 * DatasourceSettings - Datasource list management for the Settings page
 *
 * This component handles ONLY the list view of datasources:
 * - Fetching and displaying datasources
 * - Enable/disable toggle
 * - Warning badges for catalog attention
 * - Opening DatasourceModal for create/edit
 *
 * All modal-related functionality is delegated to DatasourceModal.
 */
export default function DatasourceSettings({
  apiClient,
  isAdmin,
  isOwner,
  user = null, // User object for subscription tier checking
}) {
  // ==========================================================================
  // LIST STATE
  // ==========================================================================

  const [loading, setLoading] = useState(true);
  const [datasources, setDatasources] = useState([]);
  const [catalogStatuses, setCatalogStatuses] = useState({});
  const [datasourceTypes, setDatasourceTypes] = useState({});
  const [credentialStatuses, setCredentialStatuses] = useState({});
  const [togglingDatasources, setTogglingDatasources] = useState({});
  const [oauthConnecting, setOauthConnecting] = useState(null); // datasource id currently connecting

  // ==========================================================================
  // MODAL STATE
  // ==========================================================================

  const [showModal, setShowModal] = useState(false);
  const [selectedDatasource, setSelectedDatasource] = useState(null); // null = create, object = edit

  // ==========================================================================
  // DELETE CONFIRMATION STATE (for list delete button)
  // ==========================================================================

  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [datasourceToDelete, setDatasourceToDelete] = useState(null);

  // ==========================================================================
  // DERIVED STATE
  // ==========================================================================

  const canAdmin = isAdmin || isOwner;
  const navigate = useNavigate();

  // URL query params for deep linking (e.g., ?open=snowflake)
  const [searchParams, setSearchParams] = useSearchParams();

  // Check if a datasource is auto-provisioned by analytics
  const isAnalyticsDatasource = (ds) => {
    const credStatus = credentialStatuses[ds.id];
    return !!credStatus?.connection_config?.analytics_site_id;
  };

  // ==========================================================================
  // EFFECTS
  // ==========================================================================

  // Fetch datasource types from backend registry
  useEffect(() => {
    fetchDatasourceTypes();
  }, [apiClient]);

  // Fetch datasources, catalog statuses, and credential statuses
  useEffect(() => {
    fetchDatasources();
  }, [apiClient]);

  // Handle ?open=slug query parameter to auto-open a datasource modal
  useEffect(() => {
    const openSlug = searchParams.get('open');
    if (openSlug && datasources.length > 0 && !showModal) {
      const ds = datasources.find(d => d.slug === openSlug);
      if (ds) {
        setSelectedDatasource(ds);
        setShowModal(true);
        // Clear the query param so it doesn't reopen on navigation
        setSearchParams({}, { replace: true });
      }
    }
  }, [searchParams, datasources, showModal, setSearchParams]);

  // Listen for OAuth popup completion (BigQuery and Snowflake)
  useEffect(() => {
    const handleOAuthMessage = async (event) => {
      // Verify origin
      if (event.origin !== window.location.origin) return;

      // BigQuery OAuth messages (kyomi_oauth via global Google OAuth)
      if (event.data?.type === 'GOOGLE_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('BigQuery connected successfully');
        // Refresh credential status
        await fetchCredentialStatus();
      } else if (event.data?.type === 'GOOGLE_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect BigQuery');
      }

      // BigQuery Enterprise OAuth messages
      if (event.data?.type === 'BIGQUERY_ENTERPRISE_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('BigQuery connected successfully');
        await fetchCredentialStatus();
      } else if (event.data?.type === 'BIGQUERY_ENTERPRISE_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect BigQuery');
      }

      // Snowflake OAuth messages
      if (event.data?.type === 'SNOWFLAKE_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('Snowflake connected successfully');
        await fetchCredentialStatus();
      } else if (event.data?.type === 'SNOWFLAKE_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect Snowflake');
      }

      // Microsoft OAuth messages (for Azure Synapse default OAuth)
      if (event.data?.type === 'MICROSOFT_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('Azure Synapse connected successfully');
        await fetchCredentialStatus();
      } else if (event.data?.type === 'MICROSOFT_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect Azure Synapse');
      }

      // Microsoft Enterprise OAuth messages (for Azure Synapse enterprise OAuth)
      if (event.data?.type === 'MICROSOFT_ENTERPRISE_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('Azure Synapse connected successfully');
        await fetchCredentialStatus();
      } else if (event.data?.type === 'MICROSOFT_ENTERPRISE_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect Azure Synapse');
      }

      // Databricks OAuth messages
      if (event.data?.type === 'DATABRICKS_OAUTH_SUCCESS') {
        setOauthConnecting(null);
        toast.success('Databricks connected successfully');
        await fetchCredentialStatus();
      } else if (event.data?.type === 'DATABRICKS_OAUTH_ERROR') {
        setOauthConnecting(null);
        toast.error(event.data.error || 'Failed to connect Databricks');
      }
    };

    window.addEventListener('message', handleOAuthMessage);
    return () => window.removeEventListener('message', handleOAuthMessage);
  }, [apiClient]);

  // ==========================================================================
  // API FUNCTIONS
  // ==========================================================================

  const fetchDatasourceTypes = async () => {
    if (!apiClient) return;
    try {
      const response = await apiClient.get('/api/v1/datasources/types');
      const types = response.data?.types || [];
      // Convert array to object keyed by type_id for easy lookup
      const typesObj = {};
      types.forEach((type) => {
        typesObj[type.type_id] = {
          label: type.display_name,
          description: type.description,
        };
      });
      setDatasourceTypes(typesObj);
    } catch (error) {
      setDatasourceTypes({});
    }
  };

  const fetchCredentialStatus = async (showToastOnError = false) => {
    if (!apiClient) return;
    try {
      const response = await apiClient.get('/api/v1/datasources/credential-status');
      const statuses = response.data?.datasources || [];
      // Convert array to object keyed by datasource id
      const statusesObj = {};
      statuses.forEach((status) => {
        statusesObj[status.id] = status;
      });
      setCredentialStatuses(statusesObj);
      return statusesObj;
    } catch (error) {
      if (showToastOnError) {
        toast.error('Failed to load credential status');
      }
      return {};
    }
  };

  const fetchDatasources = async (silent = false) => {
    if (!apiClient) return;
    if (!silent) setLoading(true);
    try {
      // Fetch active datasources only (admins delete to remove, not deactivate)
      const response = await apiClient.get('/api/v1/datasources');
      const ds = response.data || [];
      setDatasources(ds);

      // Fetch catalog status and credential status in parallel
      const [catalogResults] = await Promise.all([
        // Fetch catalog status for each datasource (for warning badges)
        Promise.all(
          ds.map(async (d) => {
            try {
              const statusRes = await apiClient.get(
                `/api/v1/datasources/${d.id}/catalog/status`
              );
              return { id: d.id, status: statusRes.data };
            } catch (e) {
              return { id: d.id, status: { table_count: 0, last_indexed: null } };
            }
          })
        ),
        // Fetch credential status for all datasources (show toast on error during initial load)
        fetchCredentialStatus(!silent),
      ]);

      const catalogStatuses = {};
      catalogResults.forEach((r) => {
        catalogStatuses[r.id] = r.status;
      });
      setCatalogStatuses(catalogStatuses);
    } catch (error) {
      if (!silent) toast.error('Failed to load datasources');
    } finally {
      if (!silent) setLoading(false);
    }
  };

  // ==========================================================================
  // LIST ACTIONS
  // ==========================================================================

  // Toggle user-level enabled/disabled state via the toggle endpoint
  const toggleUserDatasource = async (datasource, newEnabledState) => {
    const credStatus = credentialStatuses[datasource.id];
    if (!credStatus) return;

    // Prevent toggling if currently in progress
    if (togglingDatasources[datasource.id]) return;

    // Prevent ENABLING without credentials (disabling is always allowed)
    // Exception: Connect datasources don't have user credentials, they use the Connect token
    const hasCredentials = ['valid', 'shared'].includes(credStatus.credential_status);
    const isConnectDatasource = datasource.connection_type === 'connect';
    if (newEnabledState && !hasCredentials && !isConnectDatasource) {
      toast.error('Connect your credentials first to enable this datasource');
      return;
    }

    // Store previous state for rollback on error
    const previousEnabledState = credStatus.user_enabled;

    // Optimistic update - immediately update UI
    setCredentialStatuses((prev) => ({
      ...prev,
      [datasource.id]: {
        ...prev[datasource.id],
        user_enabled: newEnabledState,
      },
    }));

    setTogglingDatasources((prev) => ({ ...prev, [datasource.id]: true }));
    try {
      await apiClient.post(`/api/v1/datasources/${datasource.id}/toggle`, {
        enabled: newEnabledState,
      });
      toast.success(
        newEnabledState ? 'Datasource enabled' : 'Datasource disabled'
      );
      // Refresh credential status to ensure consistency with backend
      await fetchCredentialStatus();
    } catch (error) {
      // Revert optimistic update on failure
      setCredentialStatuses((prev) => ({
        ...prev,
        [datasource.id]: {
          ...prev[datasource.id],
          user_enabled: previousEnabledState,
        },
      }));
      toast.error(
        error.response?.data?.detail || 'Failed to update datasource'
      );
    } finally {
      setTogglingDatasources((prev) => ({ ...prev, [datasource.id]: false }));
    }
  };

  // Start OAuth connect flow for a datasource
  const handleOAuthConnect = (datasource) => {
    const credStatus = credentialStatuses[datasource.id];
    if (!credStatus) return;

    // Determine OAuth URL based on datasource type and auth method
    // Use credStatus.connection_config since datasource from list endpoint doesn't include it
    let url;
    const config = credStatus.connection_config || datasource.connection_config || {};
    const authMode = config.auth_mode;


    if (datasource.datasource_type === 'bigquery') {
      // Handle service_account auth mode - should never reach OAuth flow
      if (authMode === 'service_account') {
        toast.error('Service account authentication does not use OAuth. Please configure credentials in the settings modal.');
        return;
      }

      if (authMode === 'enterprise_oauth') {
        // Enterprise OAuth - use per-datasource credentials
        url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
      } else if (authMode === 'kyomi_oauth' || !authMode) {
        // Kyomi OAuth (default) - use global Google OAuth
        url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/google-oauth/connect`;
      } else {
        // Unknown auth mode - handle gracefully
        toast.error(`Unknown authentication mode: ${authMode}`);
        return;
      }
    } else if (datasource.datasource_type === 'snowflake') {
      url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/snowflake/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
    } else if (datasource.datasource_type === 'synapse') {
      // Azure Synapse uses Microsoft OAuth
      if (authMode === 'enterprise_oauth') {
        // Enterprise OAuth - use organization's Azure AD app
        url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/microsoft-enterprise/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
      } else {
        // Default OAuth - use Kyomi's multi-tenant Azure app
        url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/microsoft/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
      }
    } else if (datasource.datasource_type === 'databricks') {
      url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/databricks/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
    } else {
      // Unsupported OAuth type
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
        // If still showing as connecting, user closed the popup
        setOauthConnecting((prev) => {
          if (prev === datasource.id) return null;
          return prev;
        });
      }
    }, 500);
  };

  // Delete datasource (from list delete button)
  const handleDeleteFromList = async () => {
    if (!datasourceToDelete) return;
    try {
      await apiClient.delete(`/api/v1/datasources/${datasourceToDelete.id}`);
      toast.success('Datasource deleted');
      setShowDeleteConfirm(false);
      setDatasourceToDelete(null);
      await fetchDatasources();
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to delete');
    }
  };

  // ==========================================================================
  // HELPERS
  // ==========================================================================

  // Check if catalog needs attention (warning badge logic)
  const needsAttention = (datasourceId) => {
    const status = catalogStatuses[datasourceId];
    if (!status) return false;
    if (status.table_count === 0) return true;
    if (!status.last_indexed) return true;
    const lastIndexed = new Date(status.last_indexed);
    const daysSince = (Date.now() - lastIndexed.getTime()) / (1000 * 60 * 60 * 24);
    return daysSince > 7;
  };

  /**
   * Get the credential action button configuration for a datasource.
   * Returns { text, icon, handler, variant } or null if no action needed.
   */
  const getCredentialAction = (datasource) => {
    const credStatus = credentialStatuses[datasource.id];
    if (!credStatus) return null;

    // Kyomi Connect datasources don't have user credentials - they use the Connect token
    if (datasource.connection_type === 'connect') {
      return null;
    }

    const { credential_status, auth_method, oauth_provider } = credStatus;
    const isConnecting = oauthConnecting === datasource.id;

    // Valid or shared credentials - no action button needed
    if (credential_status === 'valid' || credential_status === 'shared') {
      return null;
    }

    // Missing OAuth credentials
    if (credential_status === 'missing' && auth_method === 'oauth') {
      const datasourceLabels = {
        bigquery: 'BigQuery',
        snowflake: 'Snowflake',
        synapse: 'Azure Synapse',
        databricks: 'Databricks',
      };
      const datasourceLabel = datasourceLabels[datasource.datasource_type] || datasource.datasource_type;
      return {
        text: isConnecting ? 'Connecting...' : `Connect ${datasourceLabel}`,
        icon: isConnecting ? Spinner : null,
        useDatasourceIcon: !isConnecting,
        datasourceType: datasource.datasource_type,
        handler: () => handleOAuthConnect(datasource),
        variant: 'default',
        disabled: isConnecting,
        spin: isConnecting,
      };
    }

    // Missing password credentials
    if (credential_status === 'missing' && auth_method === 'password') {
      return {
        text: 'Enter Credentials',
        icon: Key,
        handler: () => openEditModal(datasource),
        variant: 'default',
        disabled: false,
      };
    }

    // Expired OAuth credentials
    if (credential_status === 'expired') {
      const datasourceLabels = {
        bigquery: 'BigQuery',
        snowflake: 'Snowflake',
        synapse: 'Azure Synapse',
        databricks: 'Databricks',
      };
      const datasourceLabel = datasourceLabels[datasource.datasource_type] || datasource.datasource_type;
      return {
        text: isConnecting ? 'Connecting...' : `Reconnect ${datasourceLabel}`,
        icon: isConnecting ? Spinner : RefreshCw,
        handler: () => handleOAuthConnect(datasource),
        variant: 'warning',
        disabled: isConnecting,
        spin: isConnecting,
      };
    }

    return null;
  };

  /**
   * Get the user toggle state for a datasource.
   * Returns { enabled, canEnable, hasCredentials, isToggling } or null if not available.
   */
  const getUserToggleState = (datasource) => {
    const credStatus = credentialStatuses[datasource.id];
    if (!credStatus) return null;

    const hasCredentials = ['valid', 'shared'].includes(credStatus.credential_status);
    // Connect datasources can be enabled even without user credentials (they use the Connect token)
    const isConnectDatasource = datasource.connection_type === 'connect';
    const canEnable = isConnectDatasource || credStatus.can_enable;

    return {
      enabled: credStatus.user_enabled,
      canEnable,
      hasCredentials,
      isToggling: togglingDatasources[datasource.id] || false,
    };
  };

  // ==========================================================================
  // MODAL HANDLERS
  // ==========================================================================

  const openCreateModal = () => {
    setSelectedDatasource(null);
    setShowModal(true);
  };

  const openEditModal = (datasource) => {
    setSelectedDatasource(datasource);
    setShowModal(true);
  };

  const handleModalClose = () => {
    setShowModal(false);
    setSelectedDatasource(null);
  };

  const handleModalSaved = () => {
    setShowModal(false);
    setSelectedDatasource(null);
    fetchDatasources();
  };

  const handleModalDeleted = () => {
    setShowModal(false);
    setSelectedDatasource(null);
    fetchDatasources();
  };

  // ==========================================================================
  // RENDER: LOADING STATE
  // ==========================================================================

  if (loading) {
    return (
      <div className="p-6 space-y-4">
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-24 w-full" />
      </div>
    );
  }

  // ==========================================================================
  // RENDER: MAIN
  // ==========================================================================

  return (
    <div className="p-4 sm:p-6 space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-bold text-foreground">Datasources</h2>
          <p className="text-sm text-muted-foreground">
            Manage database connections
          </p>
        </div>
        {canAdmin && (
          <Button onClick={openCreateModal}>
            <Plus className="h-4 w-4 mr-2" />
            Add Datasource
          </Button>
        )}
      </div>

      {/* Datasources List */}
      <Card>
        <CardContent className="p-0">
          {datasources.length === 0 ? (
            <div className="text-center py-12">
              <Database className="mx-auto h-12 w-12 text-muted-foreground" />
              <p className="mt-4 text-sm text-muted-foreground">
                No datasources configured
              </p>
              {canAdmin && (
                <Button className="mt-4" onClick={openCreateModal}>
                  <Plus className="h-4 w-4 mr-2" />
                  Add Datasource
                </Button>
              )}
            </div>
          ) : (
            <div className="divide-y divide-border">
              {datasources.map((ds) => {
                const credAction = getCredentialAction(ds);
                const toggleState = getUserToggleState(ds);
                const credStatus = credentialStatuses[ds.id];

                return (
                  <div
                    key={ds.id}
                    className="flex flex-col sm:flex-row sm:items-center sm:justify-between p-4 gap-3 hover:bg-muted/50"
                  >
                    <div className="flex items-center gap-3 min-w-0">
                      <DatasourceIcon type={ds.datasource_type} className="h-6 w-6 shrink-0" />
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-1.5 sm:gap-2">
                          <span className="font-medium truncate">{ds.name}</span>
                          <Badge variant="outline">
                            {datasourceTypes[ds.datasource_type]?.label || ds.datasource_type}
                          </Badge>
                          {ds.is_sample && (
                            <Badge variant="secondary" className="text-xs">
                              Sample
                            </Badge>
                          )}
                          {isAnalyticsDatasource(ds) && (
                            <Badge variant="secondary" className="text-xs">
                              <BarChart3 className="h-3 w-3 mr-1" />
                              Analytics
                            </Badge>
                          )}
                          {/* Credential status badge */}
                          {credStatus?.credential_status === 'missing' && (
                            <Badge variant="warning" className="text-xs">
                              Needs Setup
                            </Badge>
                          )}
                          {credStatus?.credential_status === 'expired' && (
                            <Badge variant="warning" className="text-xs">
                              Expired
                            </Badge>
                          )}
                          {/* Catalog warning (only when credentials are valid) */}
                          {credStatus?.can_enable && needsAttention(ds.id) && (
                            <AlertTriangle
                              className="h-4 w-4 text-warning-foreground"
                              title="Catalog needs attention"
                            />
                          )}
                        </div>
                        {ds.slug && (
                          <p className="text-xs text-muted-foreground font-mono truncate">
                            {ds.slug}
                          </p>
                        )}
                      </div>
                    </div>
                    <div className="flex items-center gap-2 sm:gap-3 flex-wrap sm:flex-nowrap">
                      {/* Credential action button (Connect/Reconnect/Enter Credentials) */}
                      {credAction && (
                        <Button
                          variant={credAction.variant === 'warning' ? 'outline' : 'default'}
                          size="sm"
                          onClick={credAction.handler}
                          disabled={credAction.disabled}
                        >
                          {credAction.useDatasourceIcon ? (
                            <DatasourceIcon
                              type={credAction.datasourceType}
                              className="h-4 w-4 sm:mr-1"
                              opacity={1}
                            />
                          ) : credAction.icon ? (
                            <credAction.icon
                              className={`h-4 w-4 sm:mr-1 ${credAction.spin ? 'animate-spin' : ''}`}
                            />
                          ) : null}
                          {credAction.text}
                        </Button>
                      )}

                      {/* User enable/disable toggle (all users, credential-gated) */}
                      {toggleState && (
                        <div className="flex items-center gap-2">
                          <span className="text-xs text-muted-foreground hidden sm:inline">
                            {toggleState.enabled ? 'Enabled' : 'Disabled'}
                          </span>
                          <Switch
                            checked={toggleState.enabled}
                            onCheckedChange={(checked) => toggleUserDatasource(ds, checked)}
                            disabled={!toggleState.canEnable || toggleState.isToggling}
                            className={!toggleState.canEnable ? 'opacity-50 cursor-not-allowed' : ''}
                            title={
                              !toggleState.canEnable
                                ? 'Connect your credentials first to enable this datasource'
                                : toggleState.enabled
                                  ? toggleState.hasCredentials
                                    ? 'Disable datasource for your account'
                                    : 'Disable (credentials required to re-enable)'
                                  : 'Enable datasource for your account'
                            }
                          />
                        </div>
                      )}

                      {isAnalyticsDatasource(ds) ? (
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => navigate('/settings/analytics')}
                          title="Managed by Analytics — go to Analytics settings to edit"
                        >
                          <BarChart3 className="h-4 w-4 sm:mr-1" />
                          <span className="hidden sm:inline">Analytics Settings</span>
                        </Button>
                      ) : (
                        <>
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => openEditModal(ds)}
                          >
                            <Settings className="h-4 w-4 sm:mr-1" />
                            <span className="hidden sm:inline">Settings</span>
                          </Button>
                          {canAdmin && (
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() => {
                                setDatasourceToDelete(ds);
                                setShowDeleteConfirm(true);
                              }}
                            >
                              <Trash2 className="h-4 w-4 text-error-foreground" />
                            </Button>
                          )}
                        </>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </CardContent>
      </Card>

      {/* Datasource Modal (handles both create and edit) */}
      <DatasourceModal
        isOpen={showModal}
        onClose={handleModalClose}
        datasource={selectedDatasource}
        apiClient={apiClient}
        canAdmin={canAdmin}
        onSaved={handleModalSaved}
        onDeleted={handleModalDeleted}
        user={user}
      />

      {/* Delete Confirmation (for list delete button) */}
      <ConfirmDialog
        isOpen={showDeleteConfirm}
        onConfirm={handleDeleteFromList}
        onCancel={() => {
          setShowDeleteConfirm(false);
          setDatasourceToDelete(null);
        }}
        title="Delete Datasource?"
        message={`Are you sure you want to delete "${datasourceToDelete?.name}"? This cannot be undone.`}
        confirmText="Delete"
        variant="destructive"
      />
    </div>
  );
}
