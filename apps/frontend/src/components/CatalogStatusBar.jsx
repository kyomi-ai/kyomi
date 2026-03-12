// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useCallback } from 'react';
import { ArrowPathIcon, ExclamationTriangleIcon, Cog6ToothIcon } from '@heroicons/react/24/outline';
import { UnifiedStatusBar } from '@/components/ui/unified-status-bar';
import { Button } from '@/components/ui/button';
import { GoogleIcon } from '@/components/ui/icons';
import { useStatusBarDismiss } from '@/hooks/useStatusBarDismiss';
import { useNavigate } from 'react-router-dom';
import { useAuth } from '../context/AuthContext';
import { useWebSocket } from '../context/WebSocketContext';
import { toast } from '../lib/toast';

/**
 * CatalogStatusBar - Unified status bar for catalog and datasource credential status
 *
 * This component handles two types of status:
 *
 * 1. CATALOG INDEXING: Shows progress when catalog is being refreshed
 *    - Props: status, progress, lastRefresh
 *
 * 2. CREDENTIAL STATUS: Shows warnings when datasources need attention
 *    - Fetches from /api/v1/datasources/credential-status
 *    - Only shows for datasources where user_enabled === true
 *    - Priority order:
 *      a) Expired OAuth (critical - was working, now broken)
 *      b) Missing OAuth credentials (BigQuery kyomi_oauth, Snowflake, etc.)
 *      c) Missing password credentials (PostgreSQL, ClickHouse, etc.)
 *
 * The component prioritizes catalog indexing display over credential status
 * (if catalog is running/failed, that takes precedence).
 */
const CatalogStatusBar = ({
  // Catalog indexing props (passed from parent)
  status: catalogStatus = 'idle', // 'idle' | 'running' | 'failed'
  progress = null, // { total_projects, processed, tables_indexed, datasource_type, datasource_name, error }
  lastRefresh = null, // ISO timestamp
}) => {
  const navigate = useNavigate();
  const { user, apiClient } = useAuth();
  const { subscribe } = useWebSocket();

  // Credential status state
  const [credentialIssues, setCredentialIssues] = useState([]);
  const [reconnectingIds, setReconnectingIds] = useState(new Set());
  const [credentialLoading, setCredentialLoading] = useState(true);

  // Dismiss hooks - separate for catalog and credential warnings
  const {
    isDismissed: catalogDismissed,
    handleDismiss: handleCatalogDismiss
  } = useStatusBarDismiss('catalog_status_dismissed', { expiryHours: 24 });

  const {
    isDismissed: credentialDismissed,
    handleDismiss: handleCredentialDismiss,
    resetDismiss: resetCredentialDismiss
  } = useStatusBarDismiss('credential_status_dismissed', { expiryHours: 0 });

  // Fetch credential status
  const fetchCredentialStatus = useCallback(async () => {
    if (!apiClient) return;

    try {
      const response = await apiClient.get('/api/v1/datasources/credential-status');
      const datasources = response.data?.datasources || [];

      // Filter to datasources that need attention AND are user-enabled
      // Priority 1: Expired OAuth (critical)
      const expiredOAuth = datasources.filter(ds =>
        ds.credential_status === 'expired' &&
        ds.auth_method === 'oauth' &&
        ds.user_enabled
      );

      // Priority 2: Missing OAuth credentials
      const missingOAuth = datasources.filter(ds =>
        ds.credential_status === 'missing' &&
        ds.auth_method === 'oauth' &&
        ds.user_enabled
      );

      // Priority 3: Missing password credentials
      const missingPassword = datasources.filter(ds =>
        ds.credential_status === 'missing' &&
        ds.auth_method === 'password' &&
        ds.user_enabled
      );

      // Determine which issues to show
      // Show ALL issues that need attention, with priority set on each issue
      // (priority is used for status bar variant and action buttons)
      let issues = [];

      if (expiredOAuth.length > 0) {
        // Expired takes priority - show expired first, then missing
        issues = [
          ...expiredOAuth.map(ds => ({ ...ds, priority: 'expired' })),
          ...missingOAuth.map(ds => ({ ...ds, priority: 'missing_oauth' })),
          ...missingPassword.map(ds => ({ ...ds, priority: 'missing_password' })),
        ];
      } else if (missingOAuth.length > 0 || missingPassword.length > 0) {
        // No expired - show all missing (OAuth and password combined)
        issues = [
          ...missingOAuth.map(ds => ({ ...ds, priority: 'missing_oauth' })),
          ...missingPassword.map(ds => ({ ...ds, priority: 'missing_password' })),
        ];
      }

      // If we have new expired issues, reset dismissed state (critical)
      // Check if we went from no expired issues to having expired issues
      const hadExpiredIssues = credentialIssues.some(issue => issue.priority === 'expired');
      if (expiredOAuth.length > 0 && !hadExpiredIssues) {
        resetCredentialDismiss();
      }

      setCredentialIssues(issues);  // Issues already have priority property set
    } catch (error) {
    } finally {
      setCredentialLoading(false);
    }
  }, [apiClient, credentialIssues.length, resetCredentialDismiss]);

  // Fetch credential status on mount and periodically
  useEffect(() => {
    if (!user) return;

    fetchCredentialStatus();

    // Fallback polling at 5 minutes (reduced from 60 seconds since we have WebSocket push)
    const intervalId = setInterval(fetchCredentialStatus, 5 * 60 * 1000);

    // Expose function for external triggers (e.g., after OAuth success)
    window.checkDatasourceCredentialStatus = fetchCredentialStatus;

    return () => {
      clearInterval(intervalId);
      delete window.checkDatasourceCredentialStatus;
    };
  }, [user, fetchCredentialStatus]);

  // Subscribe to WebSocket credential status updates for real-time push
  useEffect(() => {
    if (!subscribe) return;

    const unsubscribe = subscribe('credential_status_changed', () => {
      // Credential status changed - refresh the status
      fetchCredentialStatus();
    });

    return unsubscribe;
  }, [subscribe, fetchCredentialStatus]);

  // Listen for OAuth popup completion messages
  useEffect(() => {
    const handleOAuthMessage = async (event) => {
      if (event.origin !== window.location.origin) return;

      // Handle various OAuth success messages
      if (event.data?.type === 'GOOGLE_OAUTH_SUCCESS' ||
          event.data?.type === 'BIGQUERY_ENTERPRISE_OAUTH_SUCCESS' ||
          event.data?.type === 'SNOWFLAKE_OAUTH_SUCCESS' ||
          event.data?.type === 'MICROSOFT_OAUTH_SUCCESS') {
        setReconnectingIds(new Set());
        await fetchCredentialStatus();
      }
    };

    window.addEventListener('message', handleOAuthMessage);
    return () => window.removeEventListener('message', handleOAuthMessage);
  }, [fetchCredentialStatus]);

  // Handle OAuth reconnect for a specific datasource
  const handleReconnect = useCallback((datasource) => {
    const config = datasource.connection_config || {};
    const authMode = config.auth_mode;

    let url;

    if (datasource.datasource_type === 'bigquery') {
      if (authMode === 'enterprise_oauth') {
        url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
      } else {
        // Default to kyomi_oauth (global Google OAuth)
        url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/google-oauth/connect`;
      }
    } else if (datasource.datasource_type === 'snowflake') {
      url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/snowflake/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
    } else if (datasource.datasource_type === 'synapse') {
      url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/microsoft/connect?datasource_slug=${encodeURIComponent(datasource.slug)}`;
    } else {
      // Unsupported OAuth - navigate to datasource settings
      navigate('/settings/datasources');
      return;
    }

    setReconnectingIds(prev => new Set([...prev, datasource.id]));

    const width = 500;
    const height = 600;
    const left = window.screenX + (window.outerWidth - width) / 2;
    const top = window.screenY + (window.outerHeight - height) / 2;

    const popup = window.open(
      url,
      'oauth-reconnect',
      `width=${width},height=${height},left=${left},top=${top},popup=1`
    );

    if (!popup || popup.closed) {
      setReconnectingIds(prev => {
        const next = new Set(prev);
        next.delete(datasource.id);
        return next;
      });
      toast.error(
        'Popup blocked by browser. You can allow popups for this site, or reconnect from Settings.',
        {
          duration: 8000,
          action: {
            label: 'Go to Settings',
            onClick: () => navigate('/settings/datasources')
          }
        }
      );
      return;
    }

    // Monitor popup for manual close
    const checkPopup = setInterval(() => {
      if (popup.closed) {
        clearInterval(checkPopup);
        setReconnectingIds(prev => {
          const next = new Set(prev);
          next.delete(datasource.id);
          return next;
        });
      }
    }, 500);
  }, [navigate]);

  // Navigate to datasource settings page
  const handleGoToSettings = useCallback(() => {
    navigate('/settings/datasources');
  }, [navigate]);

  // Get OAuth provider label
  const getOAuthProviderLabel = (datasource) => {
    if (datasource.oauth_provider === 'google') return 'Google';
    if (datasource.oauth_provider === 'snowflake') return 'Snowflake';
    if (datasource.oauth_provider === 'microsoft') return 'Microsoft';
    // Infer from datasource type
    if (datasource.datasource_type === 'bigquery') return 'Google';
    if (datasource.datasource_type === 'snowflake') return 'Snowflake';
    if (datasource.datasource_type === 'synapse') return 'Microsoft';
    return 'OAuth';
  };

  // Get type display name (for messaging)
  const getTypeDisplayName = (dsType) => {
    const typeNames = {
      bigquery: 'BigQuery',
      postgres: 'PostgreSQL',
      clickhouse: 'ClickHouse',
      snowflake: 'Snowflake',
      mysql: 'MySQL',
      redshift: 'Redshift',
      databricks: 'Databricks',
      sqlserver: 'SQL Server',
      synapse: 'Azure Synapse',
    };
    return typeNames[dsType] || dsType;
  };

  // ============================================
  // CATALOG INDEXING DISPLAY
  // ============================================

  if (catalogStatus === 'running') {
    const getCatalogMessage = () => {
      const datasourceName = progress?.datasource_name ||
        (progress?.datasource_type ? getTypeDisplayName(progress.datasource_type) : 'catalog');

      if (progress) {
        const { total_projects, processed, tables_indexed } = progress;
        if (total_projects && processed !== undefined) {
          const percentage = Math.round((processed / total_projects) * 100);
          return `Indexing ${datasourceName}... ${processed}/${total_projects} projects (${tables_indexed || 0} tables indexed) - ${percentage}%`;
        }
        if (tables_indexed !== undefined) {
          return `Indexing ${datasourceName}... ${tables_indexed} tables indexed`;
        }
      }
      return `Indexing ${datasourceName}...`;
    };

    return (
      <UnifiedStatusBar
        variant="info"
        icon={<ArrowPathIcon className="w-5 h-5 animate-spin" />}
        message={getCatalogMessage()}
      />
    );
  }

  // Check if this catalog failure is caused by expired OAuth token
  // If so, suppress the generic catalog error and show the credential status instead
  // (which has a more actionable message with Reconnect button)
  const failedDatasourceSlug = progress?.datasource_slug;
  const isCatalogFailureDueToExpiredOAuth = failedDatasourceSlug &&
    credentialIssues.some(issue =>
      issue.slug === failedDatasourceSlug && issue.priority === 'expired'
    );

  if (catalogStatus === 'failed' && !catalogDismissed && !isCatalogFailureDueToExpiredOAuth) {
    const getCatalogFailedMessage = () => {
      const datasourceName = progress?.datasource_name ||
        (progress?.datasource_type ? getTypeDisplayName(progress.datasource_type) : 'catalog');
      const errorDetail = progress?.error;
      const prefix = datasourceName === 'catalog' ? 'Catalog' : `${datasourceName} catalog`;
      if (errorDetail) {
        return `${prefix} refresh failed: ${errorDetail}`;
      }
      return `${prefix} refresh failed. Some tables may not be searchable.`;
    };

    const handleFixClick = () => {
      const slug = progress?.datasource_slug;
      if (slug) {
        navigate(`/settings/datasources?open=${encodeURIComponent(slug)}`);
      } else {
        navigate('/settings/datasources');
      }
    };

    const actions = (
      <div className="flex items-center gap-3">
        {lastRefresh && (
          <span className="text-sm text-muted-foreground">
            Last successful refresh: {new Date(lastRefresh).toLocaleString()}
          </span>
        )}
        <Button
          variant="outline"
          size="sm"
          onClick={handleFixClick}
          className="h-7 px-2 text-xs"
        >
          <Cog6ToothIcon className="w-3.5 h-3.5 mr-1" />
          Open Settings
        </Button>
      </div>
    );

    return (
      <UnifiedStatusBar
        variant="error"
        message={getCatalogFailedMessage()}
        actions={actions}
        onDismiss={handleCatalogDismiss}
        dismissLabel="Dismiss"
      />
    );
  }

  // ============================================
  // CREDENTIAL STATUS DISPLAY
  // ============================================

  // Don't show credential status if:
  // - Loading
  // - No user
  // - No issues
  // - Dismissed (for non-expired issues)
  if (credentialLoading || !user || credentialIssues.length === 0) {
    return null;
  }

  // Check if any issues are expired (for variant styling)
  const firstIssue = credentialIssues[0];
  const hasExpired = credentialIssues.some(issue => issue.priority === 'expired');

  // Allow dismiss for all credential issues (user can dismiss, will reappear on next check)
  if (credentialDismissed) {
    return null;
  }

  // Build message based on credential issues
  const getCredentialMessage = () => {
    if (credentialIssues.length === 1) {
      const ds = credentialIssues[0];
      const typeName = getTypeDisplayName(ds.datasource_type);

      if (ds.priority === 'expired') {
        return `Your ${typeName} connection "${ds.name}" has expired. Please reconnect to continue querying.`;
      } else if (ds.priority === 'missing_oauth') {
        const providerName = getOAuthProviderLabel(ds);
        return `Connect your ${providerName} account to enable "${ds.name}".`;
      } else {
        // missing_password
        return `Your ${typeName} connection "${ds.name}" needs credentials.`;
      }
    }

    // Multiple datasources - check for mixed types
    const expiredCount = credentialIssues.filter(ds => ds.priority === 'expired').length;
    const missingCount = credentialIssues.length - expiredCount;

    if (expiredCount > 0 && missingCount === 0) {
      // All expired
      const names = credentialIssues.map(ds => ds.name).join(', ');
      return `${expiredCount} datasource connection${expiredCount > 1 ? 's have' : ' has'} expired (${names}). Please reconnect to continue querying.`;
    } else if (expiredCount > 0 && missingCount > 0) {
      // Mix of expired and missing
      return `${expiredCount} expired and ${missingCount} datasource${missingCount > 1 ? 's need' : ' needs'} credentials. Set up in Settings.`;
    }

    // All missing (no expired)
    return `${credentialIssues.length} datasource${credentialIssues.length > 1 ? 's need' : ' needs'} credentials. Set up in Settings.`;
  };

  // Build action buttons for credential issues
  const getCredentialActions = () => {
    if (credentialIssues.length === 1) {
      const ds = credentialIssues[0];

      // OAuth datasources get a connect/reconnect button
      if (ds.auth_method === 'oauth') {
        const isReconnecting = reconnectingIds.has(ds.id);
        const datasourceLabel = getTypeDisplayName(ds.datasource_type);

        return (
          <Button
            onClick={() => handleReconnect(ds)}
            disabled={isReconnecting}
            variant="outline"
            size="sm"
            className="gap-2"
          >
            {isReconnecting ? 'Reconnecting...' :
              ds.priority === 'expired' ? `Reconnect ${datasourceLabel}` : `Connect ${datasourceLabel}`}
          </Button>
        );
      }

      // Password datasources get a "Go to Settings" button
      return (
        <Button
          onClick={handleGoToSettings}
          variant="outline"
          size="sm"
          className="gap-2"
        >
          <Cog6ToothIcon className="w-4 h-4" />
          Set Up Credentials
        </Button>
      );
    }

    // Multiple datasources - show button to go to settings
    return (
      <Button
        onClick={handleGoToSettings}
        variant="outline"
        size="sm"
        className="gap-2"
      >
        <Cog6ToothIcon className="w-4 h-4" />
        {hasExpired ? 'Reconnect in Settings' : 'Set Up in Settings'}
      </Button>
    );
  };

  // Determine variant based on priority
  const getVariant = () => {
    if (hasExpired) return 'error';
    return 'warning';
  };

  return (
    <UnifiedStatusBar
      variant={getVariant()}
      icon={<ExclamationTriangleIcon className="w-5 h-5" />}
      message={getCredentialMessage()}
      actions={getCredentialActions()}
      onDismiss={handleCredentialDismiss}
      dismissLabel="Dismiss"
    />
  );
};

export default CatalogStatusBar;
