// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * DatasourceModal - Schema-driven modal for creating/editing datasources
 *
 * This is a refactored version of the original 3,100+ line DatasourceModal.
 * It uses the provider registry and shared components for a modular architecture.
 *
 * Key features:
 * - Schema-driven field rendering from provider registry
 * - Unified GenericCredentialsSection for all datasource types
 * - OAuth callback handling for multiple providers
 * - Create and edit mode support
 */
import { useState, useEffect, useCallback } from 'react';
import { useSystemConfig } from '../../../context/SystemConfigContext';
import {
  Database,
  Check,
  AlertCircle,
  Lock,
  Plug,
  X,
  RefreshCw,
  Trash2,
  Upload,
  HelpCircle,
} from 'lucide-react';
import { Spinner } from '../../ui/spinner';

// Documentation URLs for each datasource type
const DATASOURCE_DOCS = {
  bigquery: 'https://kyomi.ai/docs/datasources/bigquery',
  snowflake: 'https://kyomi.ai/docs/datasources/snowflake',
  postgres: 'https://kyomi.ai/docs/datasources/postgres',
  mysql: 'https://kyomi.ai/docs/datasources/mysql',
  clickhouse: 'https://kyomi.ai/docs/datasources/clickhouse',
  sqlserver: 'https://kyomi.ai/docs/datasources/sqlserver',
  redshift: 'https://kyomi.ai/docs/datasources/redshift',
  databricks: 'https://kyomi.ai/docs/datasources/databricks',
  synapse: 'https://kyomi.ai/docs/datasources/synapse',
  duckdb: 'https://kyomi.ai/docs/datasources/duckdb',
};
import { Button } from '../../ui/button';
import { Badge } from '../../ui/badge';
import { Alert, AlertDescription } from '../../ui/alert';
import { Switch } from '../../ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../../ui/select';
import Modal from '../../Modal';
import ConfirmDialog from '../../ConfirmDialog';
import { toast } from '../../../lib/toast';
import bigQueryDirectService from '../../../services/BigQueryDirectService';
import CatalogSection from '../CatalogSection';
import { DatasourceIcon } from '../../ui/DatasourceIcon';
import {
  FormField,
  SharedCredentialsToggle,
  CredentialsForm,
  AuthModeSelector,
  OAuthConnect,
  OAuthConfig,
  IndexingCredentials,
  ProjectDropdowns,
  GenericCredentialsSection,
  ConnectSetup,
  ConnectStatus,
} from './shared/components';
import { getProvider, getProviderTypes, getDefaultAuthMode } from './index';

// Get the default datasource type (lazy to avoid circular dependency)
function getDefaultDatasourceType() {
  return getProviderTypes()[0]?.value || 'postgres';
}

export default function DatasourceModal({
  isOpen,
  onClose,
  apiClient,
  datasource = null,
  canAdmin,
  onSaved,
  onDeleted,
  title: titleOverride,
  showCatalogTab = true,
  showDeleteButton = true,
  user = null,
}) {
  const { isPersonalMode } = useSystemConfig();

  // ==========================================================================
  // STATE
  // ==========================================================================

  // Form state
  const [formData, setFormData] = useState({
    name: '',
    slug: '',
    datasource_type: getDefaultDatasourceType(),
    connection_config: {},
    auto_refresh_allowed: true,  // Default: allow dashboard auto-refresh
  });
  const [credentialsForm, setCredentialsForm] = useState({});
  const [settingsData, setSettingsData] = useState(null);
  // Credential presence flags from backend (indicates stored credentials)
  const [credentialFlags, setCredentialFlags] = useState({
    has_password: false,
    has_username: false,
    has_access_token: false,
  });
  // Track which credential fields have been modified by user
  const [dirtyCredentials, setDirtyCredentials] = useState({});
  const [settingsLoading, setSettingsLoading] = useState(false);
  const [slugManuallyEdited, setSlugManuallyEdited] = useState(false);

  // Kyomi Connect state
  const [connectionType, setConnectionType] = useState('direct'); // 'direct' or 'connect'
  const [connectSetup, setConnectSetup] = useState(null); // { token, name } after connect creation
  const [connectDatasource, setConnectDatasource] = useState(null); // Store datasource for onSaved callback

  // Tab and operation state
  // Create mode: connection/catalog (legacy flow)
  // Edit mode admins: workspace, credentials, catalog
  // Edit mode non-admins: credentials only
  const [activeTab, setActiveTab] = useState('connection');
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState(null);

  // Auth mode impact warning dialog state
  const [showAuthModeWarning, setShowAuthModeWarning] = useState(false);
  const [pendingAuthMode, setPendingAuthMode] = useState(null);
  const [affectedUsersCount, setAffectedUsersCount] = useState(0);
  const [affectedUsersLoading, setAffectedUsersLoading] = useState(false);

  // Auth mode (for providers with multiple auth modes like BigQuery, Snowflake)
  const [authMode, setAuthMode] = useState('password');

  // Beta access acknowledgment (for modes that require beta access)
  // Start as false - will be set to true by AuthModeSelector if mode doesn't require beta
  const [betaAcknowledged, setBetaAcknowledged] = useState(false);

  // OAuth state
  const [oauthStatus, setOauthStatus] = useState({
    hasOauth: false,
    oauthEmail: null,
    hasBigqueryScopes: false,
    needsBigqueryConnect: true,
  });
  const [oauthConnecting, setOauthConnecting] = useState(false);
  const [oauthDisconnecting, setOauthDisconnecting] = useState(false);
  const [showDisconnectConfirm, setShowDisconnectConfirm] = useState(false);
  const [googleProjects, setGoogleProjects] = useState([]);

  // Per-datasource OAuth state (for Snowflake, BigQuery Enterprise)
  const [providerOAuthStatus, setProviderOAuthStatus] = useState({
    connected: false,
    email: null,
    connecting: false,
    disconnecting: false,
  });

  // Credential status from backend (valid, expired, missing, shared)
  const [credentialStatus, setCredentialStatus] = useState('missing');

  // Service account state (BigQuery)
  const [serviceAccountEmail, setServiceAccountEmail] = useState('');
  const [serviceAccountJson, setServiceAccountJson] = useState('');

  // Project fetch error (BigQuery - when user has no BigQuery access)
  const [projectFetchError, setProjectFetchError] = useState(null);

  // Discovery state
  const [discoveredResources, setDiscoveredResources] = useState({});
  const [discoveryStatus, setDiscoveryStatus] = useState('idle');
  const [discoveryError, setDiscoveryError] = useState(null);

  // Catalog discovery (create mode)
  const [catalogDiscovery, setCatalogDiscovery] = useState({
    loading: false,
    items: [],
    itemType: 'items',
    error: null,
  });
  const [selectedCatalogItems, setSelectedCatalogItems] = useState([]);

  // Delete confirmation
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);

  // Sample datasource state
  const [sampleAvailable, setSampleAvailable] = useState(false);
  const [sampleAlreadyAdded, setSampleAlreadyAdded] = useState(false);
  const [creatingSample, setCreatingSample] = useState(false);

  // ==========================================================================
  // DERIVED STATE
  // ==========================================================================

  const isCreateMode = datasource === null;
  const provider = getProvider(formData.datasource_type);
  const schema = provider?.schema;

  // Check if current auth mode requires beta access
  const currentAuthModeSchema = schema?.authModes?.find((m) => m.value === authMode);
  const requiresBetaAccess = currentAuthModeSchema?.requiresBeta === true;

  const isSampleDatasource = datasource?.is_sample === true;

  const modalTitle = titleOverride
    ? titleOverride
    : isCreateMode
      ? 'Add Datasource'
      : `${datasource?.name || 'Datasource'} Settings`;

  // ==========================================================================
  // EFFECTS
  // ==========================================================================

  // Initialize modal state when opened or datasource changes
  useEffect(() => {
    if (!isOpen) return;

    if (isCreateMode) {
      resetFormState();
    } else if (datasource) {
      // Set auth mode immediately to avoid UI flash
      const savedAuthMode = datasource.connection_config?.auth_mode;
      if (savedAuthMode) {
        setAuthMode(savedAuthMode);
      } else {
        setAuthMode(getDefaultAuthMode(datasource.datasource_type));
      }
      loadDatasourceSettings(datasource);
    }
  }, [isOpen, datasource?.id]);

  // Check sample datasource availability when modal opens in create mode
  useEffect(() => {
    if (!isOpen || !isCreateMode || !apiClient) return;
    const checkSample = async () => {
      try {
        const response = await apiClient.get('/api/v1/datasources/sample/available');
        setSampleAvailable(response.data.configured);
        setSampleAlreadyAdded(response.data.already_added);
      } catch {
        setSampleAvailable(false);
      }
    };
    checkSample();
  }, [isOpen, isCreateMode, apiClient]);

  // Listen for OAuth popup completion
  useEffect(() => {
    const handleOAuthMessage = async (event) => {
      if (event.origin !== window.location.origin) return;

      // Google/BigQuery OAuth (Kyomi OAuth mode)
      if (event.data?.type === 'GOOGLE_OAUTH_SUCCESS') {
        // Clear cached token since we just got new OAuth credentials
        const dsSlug = datasource?.slug || formData?.slug;
        if (dsSlug) {
          bigQueryDirectService.clearCache(dsSlug);
        }
        setOauthConnecting(false);
        toast.success('BigQuery connected successfully');
        setOauthStatus({
          hasOauth: true,
          oauthEmail: event.data.data?.email || null,
          hasBigqueryScopes: true,
          needsBigqueryConnect: false,
        });
        setTestResult({ success: true, message: 'Connected to Google' });
        setDiscoveryStatus('success');
        fetchGoogleProjects();
        if (activeTab === 'catalog') {
          discoverCatalog();
        }
      } else if (event.data?.type === 'GOOGLE_OAUTH_ERROR') {
        setOauthConnecting(false);
        toast.error(event.data.error || 'Failed to connect BigQuery');
      }

      // Snowflake OAuth
      if (event.data?.type === 'SNOWFLAKE_OAUTH_SUCCESS') {
        setProviderOAuthStatus({
          connected: true,
          email: event.data.data?.provider_email || null,
          connecting: false,
          disconnecting: false,
        });
        setAuthMode('oauth');
        // Persist auth_mode to connection_config so it's saved with the datasource
        handleConnectionConfigChange('auth_mode', 'oauth');
        // Set auth_type so backend knows to use OAuth tokens
        setCredentialsForm({ auth_type: 'oauth' });
        toast.success('Snowflake OAuth connected successfully');
        testAndDiscover(null, { auth_type: 'oauth' });
      } else if (event.data?.type === 'SNOWFLAKE_OAUTH_ERROR') {
        setProviderOAuthStatus((prev) => ({ ...prev, connecting: false }));
        toast.error(event.data.error || 'Failed to connect Snowflake via OAuth');
      }

      // Databricks OAuth
      if (event.data?.type === 'DATABRICKS_OAUTH_SUCCESS') {
        setProviderOAuthStatus({
          connected: true,
          email: event.data.data?.provider_email || event.data.data?.email || null,
          connecting: false,
          disconnecting: false,
        });
        setAuthMode('oauth');
        // Persist auth_mode to connection_config so it's saved with the datasource
        handleConnectionConfigChange('auth_mode', 'oauth');
        // Set auth_type so backend knows to use OAuth tokens
        setCredentialsForm({ auth_type: 'oauth' });
        toast.success('Databricks OAuth connected successfully');
        testAndDiscover(null, { auth_type: 'oauth' });
      } else if (event.data?.type === 'DATABRICKS_OAUTH_ERROR') {
        setProviderOAuthStatus((prev) => ({ ...prev, connecting: false }));
        toast.error(event.data.error || 'Failed to connect Databricks via OAuth');
      }

      // BigQuery Enterprise OAuth
      if (event.data?.type === 'BIGQUERY_ENTERPRISE_OAUTH_SUCCESS') {
        // Clear cached token since we just got new OAuth credentials
        const dsSlug = datasource?.slug || formData?.slug;
        if (dsSlug) {
          bigQueryDirectService.clearCache(dsSlug);
        }
        setProviderOAuthStatus({
          connected: true,
          email: event.data.data?.email || event.data.data?.provider_email || null,
          connecting: false,
          disconnecting: false,
        });
        toast.success('BigQuery Enterprise OAuth connected successfully');
        setTestResult({ success: true, message: 'Connected to Google' });
        setDiscoveryStatus('success');
        // Pass datasource identifier and force auth mode since state might not be updated yet
        fetchGoogleProjects(dsSlug, 'enterprise_oauth');
      } else if (event.data?.type === 'BIGQUERY_ENTERPRISE_OAUTH_ERROR') {
        setProviderOAuthStatus((prev) => ({ ...prev, connecting: false }));
        toast.error(event.data.error || 'Failed to connect BigQuery via Enterprise OAuth');
      }

      // Microsoft Enterprise OAuth (for Azure Synapse - using organization's Azure AD app)
      if (event.data?.type === 'MICROSOFT_ENTERPRISE_OAUTH_SUCCESS') {
        setProviderOAuthStatus({
          connected: true,
          email: event.data.data?.provider_email || event.data.data?.email || null,
          connecting: false,
          disconnecting: false,
        });
        setAuthMode('enterprise_oauth');
        // Persist auth_mode to connection_config so it's saved with the datasource
        handleConnectionConfigChange('auth_mode', 'enterprise_oauth');
        setCredentialsForm({ auth_type: 'oauth' });
        toast.success('Microsoft Enterprise OAuth connected successfully');
        // Pass auth_mode explicitly since React state update is async
        testAndDiscover(
          { ...formData.connection_config, auth_mode: 'enterprise_oauth' },
          { auth_type: 'oauth' },
          { silent: true }
        );
      } else if (event.data?.type === 'MICROSOFT_ENTERPRISE_OAUTH_ERROR') {
        setProviderOAuthStatus((prev) => ({ ...prev, connecting: false }));
        toast.error(event.data.error || 'Failed to connect via Enterprise OAuth');
      }
    };

    window.addEventListener('message', handleOAuthMessage);
    return () => window.removeEventListener('message', handleOAuthMessage);
  }, [activeTab]);

  // ==========================================================================
  // API FUNCTIONS
  // ==========================================================================

  const fetchGoogleProjects = async (datasourceSlugOrId = null, forceAuthMode = null) => {
    // Clear any previous error
    setProjectFetchError(null);

    try {
      // For enterprise OAuth, use the catalog schemas endpoint which supports per-datasource credentials
      // For kyomi_oauth, use the global google-oauth/projects endpoint
      const currentAuthMode = forceAuthMode || formData.connection_config?.auth_mode || authMode;
      const dsIdentifier = datasourceSlugOrId || datasource?.slug || datasource?.id;


      if (currentAuthMode === 'enterprise_oauth' && dsIdentifier) {
        const response = await apiClient.get(`/api/v1/datasources/${dsIdentifier}/schemas`);
        // The schemas endpoint returns { schemas: [...], message: ... }
        if (response.data?.message && response.data?.schemas?.length === 0) {
          // There's an error message - store it for inline display
          setProjectFetchError(response.data.message);
        }
        setGoogleProjects(response.data?.schemas?.map(s => ({ project_id: s, name: s })) || []);
      } else {
        // Kyomi OAuth - use global endpoint
        const response = await apiClient.get('/api/v1/auth/google-oauth/projects');
        // Check for message (e.g., no BigQuery access)
        if (response.data?.message && (!response.data?.projects || response.data?.projects?.length === 0)) {
          setProjectFetchError(response.data.message);
        }
        setGoogleProjects(response.data?.projects || []);
      }
    } catch (error) {
      setProjectFetchError('Failed to fetch BigQuery projects. Please try again.');
      setGoogleProjects([]);
    }
  };

  const loadDatasourceSettings = async (ds) => {
    setSettingsLoading(true);
    setFormData({
      name: ds.name,
      slug: ds.slug || '',
      datasource_type: ds.datasource_type,
      connection_config: ds.connection_config || {},
      auto_refresh_allowed: ds.auto_refresh_allowed !== false,  // Default to true
    });
    // Default to workspace tab for admins, credentials for non-admins
    setActiveTab(canAdmin ? 'workspace' : 'credentials');
    setTestResult(null);
    // Reset dirty tracking when loading a datasource
    setDirtyCredentials({});

    try {
      const response = await apiClient.get(`/api/v1/datasources/${ds.id}/settings`);
      setSettingsData(response.data);

      // Set credential status from backend (valid, expired, missing, shared)
      setCredentialStatus(response.data.credential_status || 'missing');

      // Store credential presence flags from backend
      setCredentialFlags({
        has_password: response.data.has_password || false,
        has_username: response.data.has_username || false,
        has_access_token: response.data.has_access_token || false,
      });

      if (response.data.connection_config) {
        setFormData((prev) => ({
          ...prev,
          connection_config: {
            ...prev.connection_config,
            ...response.data.connection_config,
            shared_password:
              response.data.connection_config.shared_password ||
              prev.connection_config.shared_password ||
              '',
          },
        }));
      }

      const userSettings = response.data.user_settings || {};
      const savedAuthMode = response.data.connection_config?.auth_mode || getDefaultAuthMode(ds.datasource_type);
      setAuthMode(savedAuthMode);

      // Initialize credentials and OAuth status based on datasource type and auth mode
      if (ds.datasource_type === 'bigquery') {
        setCredentialsForm({
          billing_project: userSettings.billing_project || '',
          default_project: userSettings.default_project || '',
          query_size_limit_gb: userSettings.query_size_limit_gb || 10,
        });

        if (savedAuthMode === 'kyomi_oauth') {
          setOauthStatus({
            hasOauth: response.data.has_oauth || false,
            oauthEmail: response.data.oauth_email || null,
            hasBigqueryScopes: response.data.has_bigquery_scopes || false,
            needsBigqueryConnect: response.data.needs_bigquery_connect ?? true,
          });
          if (response.data.has_bigquery_scopes) {
            fetchGoogleProjects();
          }
        } else if (savedAuthMode === 'enterprise_oauth') {
          // Use credential_status to determine if OAuth is actually valid (not expired)
          const credentialStatus = response.data.credential_status || 'missing';
          const isOAuthValid = credentialStatus === 'valid' && response.data.has_oauth;
          setProviderOAuthStatus({
            connected: isOAuthValid,
            email: response.data.oauth_email || null,
            connecting: false,
            disconnecting: false,
          });
          if (isOAuthValid) {
            // Pass slug and auth mode explicitly since state hasn't updated yet
            fetchGoogleProjects(ds.slug, 'enterprise_oauth');
          }
        } else if (savedAuthMode === 'service_account') {
          setServiceAccountEmail(response.data.service_account_email || '');
        }
      } else if (ds.datasource_type === 'snowflake') {
        const hasOAuth = response.data.has_oauth || false;
        // Use credential_status to determine if OAuth is actually valid (not expired)
        const credentialStatus = response.data.credential_status || 'missing';
        const isOAuthValid = credentialStatus === 'valid' && hasOAuth;

        // If backend says we have OAuth credentials, use OAuth mode even if auth_mode wasn't saved
        const effectiveAuthMode = hasOAuth ? 'oauth' : savedAuthMode;
        if (hasOAuth && savedAuthMode !== 'oauth') {
          setAuthMode('oauth');
        }

        // Set credentials FIRST based on auth mode
        if (effectiveAuthMode === 'oauth') {
          // For OAuth mode, set auth_type so backend knows to look up tokens
          setCredentialsForm({ auth_type: 'oauth' });
        } else {
          // For password mode, set empty password fields
          setCredentialsForm({
            username: userSettings.username || '',
            password: '',
          });
        }

        // Only mark as connected if OAuth credentials are valid (not expired)
        setProviderOAuthStatus({
          connected: isOAuthValid,
          email: response.data.oauth_email || null,
          connecting: false,
          disconnecting: false,
        });

        if (isOAuthValid) {
          const loadedConfig = { ...ds.connection_config, ...response.data.connection_config };
          // Pass OAuth credentials explicitly since state may not have updated yet
          setTimeout(() => testAndDiscover(loadedConfig, { auth_type: 'oauth' }, { silent: true }), 100);
        }
      } else if (ds.datasource_type === 'databricks') {
        const hasOAuth = response.data.has_oauth || false;
        // Use credential_status to determine if OAuth is actually valid (not expired)
        const credentialStatus = response.data.credential_status || 'missing';
        const isOAuthValid = credentialStatus === 'valid' && hasOAuth;

        // If backend says we have OAuth credentials, use OAuth mode even if auth_mode wasn't saved
        const effectiveAuthMode = hasOAuth ? 'oauth' : savedAuthMode;
        if (hasOAuth && savedAuthMode !== 'oauth') {
          setAuthMode('oauth');
        }

        // Set credentials based on auth mode
        if (effectiveAuthMode === 'oauth') {
          // For OAuth mode, set auth_type so backend knows to look up tokens
          setCredentialsForm({ auth_type: 'oauth' });
        } else {
          // For token mode (PAT), set empty access_token field
          setCredentialsForm({
            access_token: '',
          });
        }

        // Only mark as connected if OAuth credentials are valid (not expired)
        setProviderOAuthStatus({
          connected: isOAuthValid,
          email: response.data.oauth_email || null,
          connecting: false,
          disconnecting: false,
        });

        if (isOAuthValid) {
          const loadedConfig = { ...ds.connection_config, ...response.data.connection_config };
          // Pass OAuth credentials explicitly since state may not have updated yet
          setTimeout(() => testAndDiscover(loadedConfig, { auth_type: 'oauth' }, { silent: true }), 100);
        }
      } else if (ds.datasource_type === 'synapse') {
        const hasOAuth = response.data.has_oauth || false;
        // Use credential_status to determine if OAuth is actually valid (not expired)
        const credentialStatus = response.data.credential_status || 'missing';
        const isOAuthValid = credentialStatus === 'valid' && hasOAuth;

        // Determine effective auth mode:
        // - If user has OAuth credentials and savedAuthMode is an OAuth mode, use savedAuthMode
        // - If user has OAuth credentials but savedAuthMode isn't an OAuth mode (legacy), default to 'oauth'
        // - Otherwise use savedAuthMode
        const isOAuthMode = savedAuthMode === 'oauth' || savedAuthMode === 'enterprise_oauth';
        let effectiveAuthMode = savedAuthMode;
        if (hasOAuth) {
          // If we have OAuth but mode isn't an OAuth mode, default to 'oauth'
          if (!isOAuthMode) {
            effectiveAuthMode = 'oauth';
            setAuthMode('oauth');
          }
        }

        // Set credentials based on auth mode
        if (effectiveAuthMode === 'oauth' || effectiveAuthMode === 'enterprise_oauth') {
          // For OAuth modes, set auth_type so backend knows to look up tokens
          setCredentialsForm({ auth_type: 'oauth' });
        } else if (effectiveAuthMode === 'service_principal') {
          // Service principal credentials
          setCredentialsForm({
            client_id: userSettings.client_id || '',
            client_secret: '',
          });
        } else {
          // SQL authentication
          setCredentialsForm({
            username: userSettings.username || '',
            password: '',
          });
        }

        // Only mark as connected if OAuth credentials are valid (not expired)
        setProviderOAuthStatus({
          connected: isOAuthValid,
          email: response.data.oauth_email || null,
          connecting: false,
          disconnecting: false,
        });

        if (isOAuthValid) {
          const loadedConfig = { ...ds.connection_config, ...response.data.connection_config };
          // Pass OAuth credentials explicitly since state may not have updated yet
          setTimeout(() => testAndDiscover(loadedConfig, { auth_type: 'oauth' }, { silent: true }), 100);
        }
      } else {
        // Standard password-based datasources
        setCredentialsForm({
          username: userSettings.username || '',
          password: '',
        });
      }
    } catch (error) {
      setSettingsData(null);
      setCredentialsForm({});
    } finally {
      setSettingsLoading(false);
    }
  };

  // ==========================================================================
  // FORM HELPERS
  // ==========================================================================

  const resetFormState = () => {
    setFormData({
      name: '',
      slug: '',
      datasource_type: getDefaultDatasourceType(),
      connection_config: {},
    });
    setCredentialsForm({});
    setSettingsData(null);
    setCredentialFlags({ has_password: false, has_username: false, has_access_token: false });
    setDirtyCredentials({});
    setSettingsLoading(false);
    // Create mode uses connection tab (legacy flow)
    setActiveTab('connection');
    setTestResult(null);
    // Reset auth mode warning state
    setShowAuthModeWarning(false);
    setPendingAuthMode(null);
    setAffectedUsersCount(0);
    setCatalogDiscovery({ loading: false, items: [], itemType: 'items', error: null });
    setSelectedCatalogItems([]);
    setSlugManuallyEdited(false);
    setConnectionType('direct');
    setConnectSetup(null);
    setAuthMode(getDefaultAuthMode(getDefaultDatasourceType()));
    setOauthStatus({
      hasOauth: false,
      oauthEmail: null,
      hasBigqueryScopes: false,
      needsBigqueryConnect: true,
    });
    setProviderOAuthStatus({
      connected: false,
      email: null,
      connecting: false,
      disconnecting: false,
    });
    setGoogleProjects([]);
    setProjectFetchError(null);
    setServiceAccountEmail('');
    setServiceAccountJson('');
    setDiscoveredResources({});
    setDiscoveryStatus('idle');
    setDiscoveryError(null);
  };

  const generateSlug = (name) => {
    return name
      .toLowerCase()
      .replace(/[\s_]+/g, '-')
      .replace(/[^a-z0-9-]/g, '')
      .replace(/-+/g, '-')
      .replace(/^-|-$/g, '');
  };

  const handleNameChange = (newName) => {
    if (!slugManuallyEdited) {
      setFormData((prev) => ({ ...prev, name: newName, slug: generateSlug(newName) }));
    } else {
      setFormData((prev) => ({ ...prev, name: newName }));
    }
  };

  const handleSlugChange = (newSlug) => {
    setSlugManuallyEdited(true);
    setFormData((prev) => ({
      ...prev,
      slug: newSlug.toLowerCase().replace(/[^a-z0-9-]/g, ''),
    }));
  };

  const handleConnectionConfigChange = (fieldName, value) => {
    setFormData((prev) => ({
      ...prev,
      connection_config: { ...prev.connection_config, [fieldName]: value },
    }));
  };

  const handleDatasourceTypeChange = (newType) => {
    setFormData((prev) => ({
      ...prev,
      datasource_type: newType,
      connection_config: {},
    }));
    setAuthMode(getDefaultAuthMode(newType));
    // Reset connection type if new type doesn't support Connect
    const newProvider = getProvider(newType);
    if (!newProvider?.schema?.connectSupported) {
      setConnectionType('direct');
    }
    setDiscoveredResources({});
    setDiscoveryStatus('idle');
    setDiscoveryError(null);
    setTestResult(null);
    setCredentialsForm({});
    setSelectedCatalogItems([]);
  };

  const handleAuthModeChange = async (newMode) => {
    // In edit mode, check if other users would be affected by this change
    if (!isCreateMode && datasource?.slug && authMode !== newMode) {
      setAffectedUsersLoading(true);
      try {
        const response = await apiClient.get(
          `/api/v1/datasources/${datasource.slug}/affected-users?new_auth_mode=${encodeURIComponent(newMode)}`
        );

        if (response.data.affected_count > 0) {
          // Show warning dialog before applying change
          setAffectedUsersCount(response.data.affected_count);
          setPendingAuthMode(newMode);
          setShowAuthModeWarning(true);
          setAffectedUsersLoading(false);
          return; // Don't apply change yet - wait for user confirmation
        }
      } catch (error) {
        // On error, proceed without warning (fail open for UX)
      } finally {
        setAffectedUsersLoading(false);
      }
    }

    // Apply the auth mode change
    applyAuthModeChange(newMode);
  };

  const applyAuthModeChange = (newMode) => {
    setAuthMode(newMode);
    handleConnectionConfigChange('auth_mode', newMode);

    // Clear irrelevant fields when switching modes
    if (formData.datasource_type === 'bigquery') {
      if (newMode === 'kyomi_oauth') {
        handleConnectionConfigChange('oauth_client_id', '');
        handleConnectionConfigChange('oauth_client_secret', '');
        handleConnectionConfigChange('service_account_json', '');
        setProviderOAuthStatus({ connected: false, email: null, connecting: false, disconnecting: false });
        setServiceAccountJson('');
        setServiceAccountEmail('');
      } else if (newMode === 'enterprise_oauth') {
        handleConnectionConfigChange('service_account_json', '');
        setServiceAccountJson('');
        setServiceAccountEmail('');
      } else if (newMode === 'service_account') {
        handleConnectionConfigChange('oauth_client_id', '');
        handleConnectionConfigChange('oauth_client_secret', '');
        setProviderOAuthStatus({ connected: false, email: null, connecting: false, disconnecting: false });
      }
    } else if (formData.datasource_type === 'snowflake') {
      if (newMode !== 'oauth' && providerOAuthStatus.connected) {
        setProviderOAuthStatus({ connected: false, email: null, connecting: false, disconnecting: false });
      }
      if (newMode === 'oauth') {
        setCredentialsForm((prev) => ({ ...prev, password: '', private_key: '', private_key_passphrase: '' }));
      } else if (newMode === 'password') {
        setCredentialsForm((prev) => ({ ...prev, private_key: '', private_key_passphrase: '' }));
      } else if (newMode === 'keypair') {
        setCredentialsForm((prev) => ({ ...prev, password: '' }));
      }
    } else if (formData.datasource_type === 'databricks') {
      if (newMode !== 'oauth' && providerOAuthStatus.connected) {
        setProviderOAuthStatus({ connected: false, email: null, connecting: false, disconnecting: false });
      }
      if (newMode === 'oauth') {
        // Clear PAT when switching to OAuth
        setCredentialsForm((prev) => ({ ...prev, access_token: '' }));
      }
    }
  };

  const handleCredentialsChange = (fieldName, value) => {
    setCredentialsForm((prev) => ({ ...prev, [fieldName]: value }));
    // Mark this field as dirty (user has modified it)
    setDirtyCredentials((prev) => ({ ...prev, [fieldName]: true }));
  };

  const handleSharedCredentialsChange = (enabled) => {
    setFormData((prev) => ({
      ...prev,
      connection_config: {
        ...prev.connection_config,
        shared_credentials: enabled,
        ...(enabled ? {} : { shared_username: '', shared_password: '' }),
      },
    }));
  };

  const getConnectionConfig = () => {
    if (canAdmin) {
      return formData.connection_config || {};
    }
    return settingsData?.connection_config || {};
  };

  const isUsingSharedCredentials = () => {
    const config = getConnectionConfig();
    return config.shared_credentials || false;
  };

  // ==========================================================================
  // OAUTH FUNCTIONS
  // ==========================================================================

  const handleConnectBigQuery = () => {
    setOauthConnecting(true);

    // Determine URL based on auth mode
    const currentAuthMode = formData.connection_config?.auth_mode || authMode;
    let url;

    if (currentAuthMode === 'enterprise_oauth') {
      // Enterprise OAuth - use datasource-specific endpoint
      const dsSlug = datasource?.slug || formData.slug;
      url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=${encodeURIComponent(dsSlug)}`;
    } else {
      // Kyomi OAuth (default) - use global Google OAuth
      url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/google-oauth/connect`;
    }

    const width = 500;
    const height = 600;
    const left = window.screenX + (window.outerWidth - width) / 2;
    const top = window.screenY + (window.outerHeight - height) / 2;

    const popup = window.open(
      url,
      'bigquery-oauth',
      `width=${width},height=${height},left=${left},top=${top},popup=1`
    );

    if (!popup || popup.closed) {
      setOauthConnecting(false);
      toast.error('Popup was blocked. Please allow popups for this site.');
      return;
    }

    const checkPopup = setInterval(() => {
      if (popup.closed) {
        clearInterval(checkPopup);
        setOauthConnecting((prev) => (prev ? false : prev));
      }
    }, 500);
  };

  const handleDisconnectGoogle = async () => {
    setOauthDisconnecting(true);
    try {
      await apiClient.post('/api/v1/auth/google-oauth/disconnect');
      toast.success('Google account disconnected');
      setOauthStatus({
        hasOauth: false,
        oauthEmail: null,
        hasBigqueryScopes: false,
        needsBigqueryConnect: true,
      });
      setGoogleProjects([]);
      setCredentialsForm((prev) => ({ ...prev, billing_project: '', default_project: '' }));
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to disconnect Google account');
    } finally {
      setOauthDisconnecting(false);
      setShowDisconnectConfirm(false);
    }
  };

  const handleConnectProviderOAuth = async (providerType) => {
    setProviderOAuthStatus((prev) => ({ ...prev, connecting: true }));

    // Validate required fields
    if (!formData.name) {
      setProviderOAuthStatus((prev) => ({ ...prev, connecting: false }));
      toast.error('Please enter a datasource name first.');
      return;
    }

    let datasourceId = datasource?.id;
    let datasourceSlug = datasource?.slug || formData.slug;

    // In create mode, we need to create the datasource first
    if (!datasourceId) {
      try {
        const createPayload = {
          name: formData.name,
          datasource_type: formData.datasource_type,
          connection_config: formData.connection_config,
        };
        if (formData.slug) {
          createPayload.slug = formData.slug;
        }

        const response = await apiClient.post('/api/v1/datasources', createPayload);
        const newDatasource = response.data;
        datasourceId = newDatasource.id;
        datasourceSlug = newDatasource.slug;

        // Update form state with the new datasource info
        setFormData((prev) => ({ ...prev, slug: newDatasource.slug }));

        // Notify parent that datasource was created (so list refreshes)
        onSaved?.(newDatasource);

        toast.success('Datasource created. Connecting OAuth...');
      } catch (error) {
        setProviderOAuthStatus((prev) => ({ ...prev, connecting: false }));
        toast.error(error.response?.data?.detail || 'Failed to create datasource. Please try again.');
        return;
      }
    } else if (canAdmin) {
      // In edit mode, admins save the connection config before initiating OAuth
      // This ensures the backend has the latest OAuth credentials
      // Non-admins skip this step - they can only connect their personal credentials
      try {
        await apiClient.put(`/api/v1/datasources/${datasourceId}`, {
          name: formData.name,
          slug: formData.slug,
          connection_config: formData.connection_config,
        });
      } catch (error) {
        setProviderOAuthStatus((prev) => ({ ...prev, connecting: false }));
        toast.error('Failed to save configuration. Please try again.');
        return;
      }
    }

    // Map provider type to OAuth endpoint
    const endpointMap = {
      snowflake: 'snowflake',
      bigquery: 'bigquery-enterprise',
      databricks: 'databricks',
    };
    const endpoint = endpointMap[providerType] || providerType;
    const url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/${endpoint}/connect?datasource_slug=${encodeURIComponent(datasourceSlug)}`;

    const width = 500;
    const height = 600;
    const left = window.screenX + (window.outerWidth - width) / 2;
    const top = window.screenY + (window.outerHeight - height) / 2;

    const popup = window.open(
      url,
      `${providerType}-oauth`,
      `width=${width},height=${height},left=${left},top=${top},popup=1`
    );

    if (!popup || popup.closed) {
      setProviderOAuthStatus((prev) => ({ ...prev, connecting: false }));
      toast.error('Popup was blocked. Please allow popups for this site.');
      return;
    }

    const checkPopup = setInterval(() => {
      if (popup.closed) {
        clearInterval(checkPopup);
        setProviderOAuthStatus((prev) => (prev.connecting ? { ...prev, connecting: false } : prev));
      }
    }, 500);
  };

  const handleDisconnectProviderOAuth = async () => {
    const datasourceSlug = datasource?.slug || formData.slug;
    if (!datasourceSlug) {
      toast.error('Cannot disconnect: datasource not saved yet');
      return;
    }

    setProviderOAuthStatus((prev) => ({ ...prev, disconnecting: true }));

    try {
      await apiClient.delete(`/api/v1/datasources/${datasourceSlug}/credentials`);
      setProviderOAuthStatus({ connected: false, email: null, connecting: false, disconnecting: false });
      setDiscoveredResources({});
      setDiscoveryStatus('idle');
      setTestResult(null);
      if (formData.datasource_type === 'bigquery') {
        setGoogleProjects([]);
        setCredentialsForm((prev) => ({ ...prev, billing_project: '', default_project: '' }));
      }
      toast.success('OAuth disconnected');
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to disconnect');
      setProviderOAuthStatus((prev) => ({ ...prev, disconnecting: false }));
    }
  };

  // ==========================================================================
  // SERVICE ACCOUNT FUNCTIONS (BigQuery)
  // ==========================================================================

  const handleServiceAccountUpload = (event) => {
    const file = event.target.files?.[0];
    if (!file) return;

    if (!file.name.endsWith('.json')) {
      toast.error('Please upload a JSON file');
      return;
    }

    const reader = new FileReader();
    reader.onload = (e) => {
      try {
        const jsonContent = e.target?.result;
        const parsed = JSON.parse(jsonContent);

        if (!parsed.client_email || !parsed.private_key) {
          toast.error('Invalid service account file. Missing required fields.');
          return;
        }

        setServiceAccountJson(jsonContent);
        setServiceAccountEmail(parsed.client_email);
        handleConnectionConfigChange('service_account_json', jsonContent);
        toast.success('Service account file loaded');
      } catch (err) {
        toast.error('Invalid JSON file');
      }
    };
    reader.readAsText(file);
  };

  const handleServiceAccountJsonChange = (jsonText) => {
    setServiceAccountJson(jsonText);

    if (!jsonText.trim()) {
      setServiceAccountEmail('');
      handleConnectionConfigChange('service_account_json', '');
      return;
    }

    try {
      const parsed = JSON.parse(jsonText);
      if (parsed.client_email) {
        setServiceAccountEmail(parsed.client_email);
      }
      handleConnectionConfigChange('service_account_json', jsonText);
    } catch {
      setServiceAccountEmail('');
    }
  };

  const handleServiceAccountClear = () => {
    setServiceAccountJson('');
    setServiceAccountEmail('');
    handleConnectionConfigChange('service_account_json', '');
  };

  // ==========================================================================
  // TEST & DISCOVER FUNCTIONS
  // ==========================================================================

  const testAndDiscover = async (overrideConfig = null, overrideCredentials = null, options = {}) => {
    const { silent = false } = options;
    setTesting(true);
    setTestResult(null);
    setDiscoveryStatus('loading');
    setDiscoveryError(null);
    setDiscoveredResources({});

    const effectiveDatasourceType = datasource?.datasource_type || formData.datasource_type;
    const effectiveConnectionConfig = overrideConfig || formData.connection_config;
    const effectiveCredentials = overrideCredentials || credentialsForm;

    try {
      const response = await apiClient.post('/api/v1/datasources/discover', {
        datasource_type: effectiveDatasourceType,
        connection_config: effectiveConnectionConfig,
        credentials: effectiveCredentials,
        datasource_slug: datasource?.slug || formData.slug,
      });

      const { success, resources, message } = response.data;

      if (success) {
        setTestResult({ success: true, message: 'Connected successfully' });
        setDiscoveredResources(resources || {});
        setDiscoveryStatus('success');
        if (!silent) {
          toast.success(message || 'Connection successful and resources discovered');
        }
      } else {
        setTestResult({ success: false, message: message || 'Discovery failed' });
        setDiscoveryStatus('error');
        setDiscoveryError(message || 'Failed to discover resources');
        if (!silent) {
          toast.error(message || 'Discovery failed');
        }
      }
    } catch (error) {
      const message = error.response?.data?.detail || error.response?.data?.message || 'Connection test failed';
      setTestResult({ success: false, message });
      setDiscoveryStatus('error');
      setDiscoveryError(message);
      if (!silent) {
        toast.error(message);
      }

      if (message.includes('OAuth token') && message.includes('failed')) {
        const dsType = datasource?.datasource_type || formData.datasource_type;
        if (dsType === 'snowflake' || dsType === 'bigquery' || dsType === 'databricks') {
          setProviderOAuthStatus((prev) => ({ ...prev, connected: false, email: null }));
        }
      }
    } finally {
      setTesting(false);
    }
  };

  const discoverCatalog = async () => {
    setCatalogDiscovery({ loading: true, items: [], itemType: 'items', error: null });
    try {
      const response = await apiClient.post('/api/v1/datasources/discover-catalog', {
        datasource_type: formData.datasource_type,
        connection_config: formData.connection_config,
        credentials: credentialsForm,
        datasource_slug: datasource?.slug || formData.slug,  // For OAuth credential lookup
      });

      if (response.data.success) {
        setCatalogDiscovery({
          loading: false,
          items: response.data.items || [],
          itemType: response.data.item_type || 'items',
          error: null,
        });
      } else {
        setCatalogDiscovery({
          loading: false,
          items: [],
          itemType: 'items',
          error: response.data.message || 'Discovery failed',
        });
      }
    } catch (error) {
      setCatalogDiscovery({
        loading: false,
        items: [],
        itemType: 'items',
        error: error.response?.data?.detail || 'Failed to discover catalog items',
      });
    }
  };

  // ==========================================================================
  // SAVE FUNCTIONS
  // ==========================================================================

  const handleSave = async () => {
    setSaving(true);

    try {
      if (isCreateMode) {
        const catalogConfigKey = schema?.catalogConfig?.key || 'catalog_items';
        const connectionConfigWithCatalog = {
          ...formData.connection_config,
          [catalogConfigKey]: selectedCatalogItems,
        };

        const createPayload = {
          name: formData.name,
          datasource_type: formData.datasource_type,
          connection_config: connectionConfigWithCatalog,
          connection_type: connectionType,
        };

        if (formData.slug) {
          createPayload.slug = formData.slug;
        }

        const response = await apiClient.post('/api/v1/datasources', createPayload);
        const newDatasource = response.data;


        // Connect datasources have no user credentials — skip credential POST
        if (connectionType !== 'connect') {
          const isShared = formData.connection_config?.shared_credentials;
          if (!isShared && Object.keys(credentialsForm).length > 0) {
            await apiClient.post(`/api/v1/datasources/${newDatasource.id}/credentials`, {
              credentials: credentialsForm,
            });
          }
        }

        // If Connect datasource was created, show token setup screen
        if (response.data.connect_token) {
          setConnectSetup({
            token: response.data.connect_token,
            name: formData.name,
            type: formData.datasource_type,
          });
          setConnectDatasource(newDatasource); // Store for onSaved callback
          // DO NOT call onSaved yet — we still need to show the ConnectSetup UI
          // onSaved will be called when user clicks "Done" in ConnectSetup
          return; // Don't close the modal — show ConnectSetup instead
        }

        toast.success('Datasource created');
        onClose();
        onSaved?.(newDatasource);
      } else {
        let updatedDatasource = datasource;
        if (canAdmin && (activeTab === 'connection' || activeTab === 'workspace')) {
          const response = await apiClient.put(`/api/v1/datasources/${datasource.id}`, {
            name: formData.name,
            slug: formData.slug,
            connection_config: formData.connection_config,
            auto_refresh_allowed: formData.auto_refresh_allowed,
          });
          updatedDatasource = response.data;
        }

        // Save credentials - only send dirty (modified) fields
        // Backend will merge with stored credentials
        if (activeTab === 'connection' || activeTab === 'credentials') {
          const dirtyFields = Object.keys(dirtyCredentials).filter((k) => dirtyCredentials[k]);
          if (dirtyFields.length > 0) {
            const credentialsToSend = {};
            dirtyFields.forEach((field) => {
              credentialsToSend[field] = credentialsForm[field];
            });
            await apiClient.post(`/api/v1/datasources/${datasource.id}/credentials`, {
              credentials: credentialsToSend,
            });
          }
        }

        // Clear BigQuery token cache when auth settings might have changed
        if (datasource?.datasource_type === 'bigquery' && datasource?.slug) {
          bigQueryDirectService.clearCache(datasource.slug);
        }

        toast.success('Settings saved');
        onSaved?.(updatedDatasource);
      }
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to save');
    } finally {
      setSaving(false);
    }
  };

  const handleSaveAndClose = async () => {
    setSaving(true);

    try {
      let updatedDatasource = datasource;
      if (canAdmin) {
        const response = await apiClient.put(`/api/v1/datasources/${datasource.id}`, {
          name: formData.name,
          slug: formData.slug,
          connection_config: formData.connection_config,
          auto_refresh_allowed: formData.auto_refresh_allowed,
        });
        updatedDatasource = response.data;
      }

      // Save credentials - only send dirty (modified) fields
      // Backend will merge with stored credentials
      const dirtyFields = Object.keys(dirtyCredentials).filter((k) => dirtyCredentials[k]);
      if (dirtyFields.length > 0) {
        const credentialsToSend = {};
        dirtyFields.forEach((field) => {
          credentialsToSend[field] = credentialsForm[field];
        });
        await apiClient.post(`/api/v1/datasources/${datasource.id}/credentials`, {
          credentials: credentialsToSend,
        });
      }

      // Clear BigQuery token cache when auth settings might have changed
      if (datasource?.datasource_type === 'bigquery' && datasource?.slug) {
        bigQueryDirectService.clearCache(datasource.slug);
      }

      onClose();
      onSaved?.(updatedDatasource);
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to save');
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!datasource) return;
    try {
      await apiClient.delete(`/api/v1/datasources/${datasource.id}`);
      toast.success('Datasource deleted');
      setShowDeleteConfirm(false);
      onClose();
      onDeleted?.(datasource);
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to delete');
    }
  };

  const handleCreateSample = async () => {
    setCreatingSample(true);
    try {
      await apiClient.post('/api/v1/datasources/sample');
      toast.success('Sample datasource added');
      onClose();
      onSaved?.();
    } catch (error) {
      const detail = error.response?.data?.detail;
      if (error.response?.status === 409) {
        toast.error(detail || 'Sample datasource already exists');
        setSampleAlreadyAdded(true);
      } else {
        toast.error(detail || 'Failed to add sample datasource');
      }
    } finally {
      setCreatingSample(false);
    }
  };

  const handleCatalogConfigChange = useCallback(
    async (configKey, value) => {
      if (!datasource) return;
      if (!formData.name) {
        toast.error('Missing datasource name');
        return;
      }
      try {
        const updatedConfig = { ...formData.connection_config, [configKey]: value };
        await apiClient.put(`/api/v1/datasources/${datasource.id}`, {
          name: formData.name,
          connection_config: updatedConfig,
        });
        setFormData((prev) => ({ ...prev, connection_config: updatedConfig }));
      } catch (error) {
        toast.error('Failed to update configuration');
      }
    },
    [apiClient, datasource, formData]
  );

  // ==========================================================================
  // RENDER: CONNECTION FIELDS
  // ==========================================================================

  const renderConnectionFields = () => {
    if (!schema?.connectionFields || schema.connectionFields.length === 0) {
      return null;
    }

    const readOnly = !canAdmin;
    const config = canAdmin ? formData.connection_config : (settingsData?.connection_config || {});

    return (
      <div className="grid grid-cols-2 gap-4">
        {schema.connectionFields.map((field) => (
          <FormField
            key={field.name}
            field={field}
            value={config[field.name] ?? field.defaultValue ?? ''}
            onChange={(value) => handleConnectionConfigChange(field.name, value)}
            disabled={readOnly}
          />
        ))}
      </div>
    );
  };

  // ==========================================================================
  // RENDER: SSH TUNNEL SECTION
  // ==========================================================================

  const renderSSHTunnelSection = () => {
    if (!schema?.sshTunnelSupported || !canAdmin) {
      return null;
    }

    const config = formData.connection_config;
    const sshEnabled = config.ssh_enabled || false;

    const handleSSHToggle = (enabled) => {
      if (enabled) {
        handleConnectionConfigChange('ssh_enabled', true);
      } else {
        handleConnectionConfigChange('ssh_enabled', false);
        handleConnectionConfigChange('ssh_host', '');
        handleConnectionConfigChange('ssh_port', 22);
        handleConnectionConfigChange('ssh_username', '');
      }
    };

    return (
      <div className="border-t border-border pt-4 mt-4">
        <label className="flex items-center gap-3 p-3 bg-muted/30 rounded-lg cursor-pointer mb-4">
          <input
            type="checkbox"
            checked={sshEnabled}
            onChange={(e) => handleSSHToggle(e.target.checked)}
            className="h-4 w-4 rounded border-input"
          />
          <div>
            <p className="text-sm font-medium">Connect via SSH Tunnel</p>
            <p className="text-xs text-muted-foreground">
              Use a bastion host to reach the database behind a firewall
            </p>
          </div>
        </label>

        {sshEnabled && (
          <div className="space-y-4 pl-4 border-l-2 border-muted">
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm font-medium mb-1">
                  SSH Host <span className="text-error-foreground">*</span>
                </label>
                <input
                  type="text"
                  value={config.ssh_host || ''}
                  onChange={(e) => handleConnectionConfigChange('ssh_host', e.target.value)}
                  placeholder="bastion.example.com"
                  className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                />
              </div>
              <div>
                <label className="block text-sm font-medium mb-1">SSH Port</label>
                <input
                  type="number"
                  value={config.ssh_port || 22}
                  onChange={(e) => handleConnectionConfigChange('ssh_port', parseInt(e.target.value) || 22)}
                  className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                />
              </div>
              <div className="col-span-2">
                <label className="block text-sm font-medium mb-1">
                  SSH Username <span className="text-error-foreground">*</span>
                </label>
                <input
                  type="text"
                  value={config.ssh_username || ''}
                  onChange={(e) => handleConnectionConfigChange('ssh_username', e.target.value)}
                  placeholder="ssh_user"
                  className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                />
              </div>
            </div>
            <div className="p-3 bg-muted/30 rounded-lg">
              <p className="text-xs text-muted-foreground">
                SSH keypair will be generated when you save. Add the public key to the bastion server's authorized_keys file.
              </p>
            </div>
          </div>
        )}
      </div>
    );
  };

  // ==========================================================================
  // RENDER: DISCOVERY FIELDS
  // ==========================================================================

  const renderDiscoveryFields = () => {
    if (!schema?.discoveryFields || schema.discoveryFields.length === 0) {
      return null;
    }

    const showAsDropdowns = discoveryStatus === 'success';
    const config = formData.connection_config;

    return (
      <div className="border-t border-border pt-4 mt-4">
        <h4 className="text-sm font-medium mb-3">
          {showAsDropdowns ? 'Select Resources' : 'Resource Configuration'}
        </h4>
        {showAsDropdowns && (
          <p className="text-xs text-muted-foreground mb-4">
            Choose from the discovered resources below:
          </p>
        )}
        <div className="grid grid-cols-2 gap-4">
          {schema.discoveryFields.map((field) => {
            const currentValue = config[field.name] || '';
            const isDiscoveryField = !!field.discoveryKey;
            const options = isDiscoveryField ? (discoveredResources[field.discoveryKey] || []) : [];

            if (!showAsDropdowns || !isDiscoveryField) {
              return (
                <div key={field.name} className={field.gridColumn === 'full' ? 'col-span-2' : ''}>
                  <label className="block text-sm font-medium mb-1">
                    {field.label}
                    {!field.optional && canAdmin && <span className="text-error-foreground"> *</span>}
                  </label>
                  {canAdmin ? (
                    <input
                      type="text"
                      value={currentValue}
                      onChange={(e) => handleConnectionConfigChange(field.name, e.target.value)}
                      placeholder={field.placeholder || ''}
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                    />
                  ) : (
                    <p className="text-sm text-muted-foreground py-2">{currentValue || '—'}</p>
                  )}
                  {field.helpText && (
                    <p className="text-xs text-muted-foreground mt-1">{field.helpText}</p>
                  )}
                </div>
              );
            }

            return (
              <div key={field.name} className={field.gridColumn === 'full' ? 'col-span-2' : ''}>
                <label className="block text-sm font-medium mb-1">
                  {field.label}
                  {!field.optional && <span className="text-error-foreground"> *</span>}
                </label>
                <Select
                  value={currentValue}
                  onValueChange={(val) => handleConnectionConfigChange(field.name, val)}
                  disabled={options.length === 0}
                >
                  <SelectTrigger>
                    <SelectValue
                      placeholder={
                        options.length === 0
                          ? `No ${field.discoveryKey} found`
                          : (field.placeholder || `Select ${field.label}...`)
                      }
                    />
                  </SelectTrigger>
                  <SelectContent>
                    {options.map((option) => (
                      <SelectItem key={option} value={option}>
                        {option}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {field.helpText && (
                  <p className="text-xs text-muted-foreground mt-1">{field.helpText}</p>
                )}
              </div>
            );
          })}
        </div>
      </div>
    );
  };

  // ==========================================================================
  // RENDER: CREDENTIALS SECTION
  // ==========================================================================

  const renderCredentialsSection = () => {
    const providerData = getProvider(formData.datasource_type);
    const providerSchema = providerData?.schema || schema;

    // No credential fields and no auth modes — nothing to render
    if (!providerSchema?.authModes && (!providerSchema?.credentialFields || providerSchema.credentialFields.length === 0)) {
      return null;
    }

    return (
      <GenericCredentialsSection
        schema={providerSchema}
        authMode={authMode}
        onAuthModeChange={handleAuthModeChange}
        // Global OAuth (BigQuery kyomi_oauth)
        globalOAuthStatus={oauthStatus}
        onGlobalOAuthConnect={handleConnectBigQuery}
        onGlobalOAuthDisconnect={() => setShowDisconnectConfirm(true)}
        oauthConnecting={oauthConnecting}
        oauthDisconnecting={oauthDisconnecting}
        // Provider OAuth (enterprise/standard)
        providerOAuthStatus={providerOAuthStatus}
        onProviderOAuthConnect={handleConnectProviderOAuth}
        onProviderOAuthDisconnect={handleDisconnectProviderOAuth}
        // Credentials
        credentialsForm={credentialsForm}
        onCredentialsChange={handleCredentialsChange}
        connectionConfig={formData.connection_config}
        onConnectionConfigChange={handleConnectionConfigChange}
        // Shared credentials
        sharedCredentials={isUsingSharedCredentials()}
        onSharedCredentialsChange={handleSharedCredentialsChange}
        // Service account
        serviceAccountEmail={serviceAccountEmail}
        serviceAccountJson={serviceAccountJson}
        onServiceAccountUpload={handleServiceAccountUpload}
        onServiceAccountJsonChange={handleServiceAccountJsonChange}
        onServiceAccountClear={handleServiceAccountClear}
        // Common
        canAdmin={canAdmin}
        disabled={!canAdmin}
        testing={testing}
        testResult={testResult}
        onTestAndDiscover={testAndDiscover}
        credentialStatus={credentialStatus}
        // Discovery
        discoveredResources={discoveredResources}
        googleProjects={googleProjects}
        projectFetchError={projectFetchError}
        onBetaAcknowledgedChange={setBetaAcknowledged}
        // Docs
        docsUrl={DATASOURCE_DOCS[formData.datasource_type]}
      />
    );
  };

  // ==========================================================================
  // RENDER: TEST & DISCOVER BUTTON
  // ==========================================================================

  const renderTestAndDiscoverButton = () => {
    const mode = currentAuthModeSchema;
    // Hide for OAuth modes (OAuth flow already validates connection)
    // Hide for service account modes (they have their own validate button)
    if (mode?.oauth || mode?.serviceAccount) return null;
    // Hide for providers with authModes but no connection fields (e.g., BigQuery kyomi_oauth without configFields)
    if (schema?.authModes && !schema?.connectionFields?.length && !mode?.credentialFields?.length) return null;

    return (
      <div className="border-t border-border pt-4 mt-4">
        <div className="flex items-center gap-3">
          <Button variant="outline" onClick={() => testAndDiscover()} disabled={testing}>
            {testing ? (
              <>
                <Spinner size="sm" className="mr-2" />
                Discovering...
              </>
            ) : (
              <>
                <Plug className="h-4 w-4 mr-2" />
                Test & Discover
              </>
            )}
          </Button>
          {testResult && (
            <div
              className={`flex items-center gap-2 text-sm ${testResult.success ? 'text-success-foreground' : 'text-error-foreground'}`}
            >
              {testResult.success ? <Check className="h-4 w-4" /> : <X className="h-4 w-4" />}
              <span>{testResult.success ? 'Connected' : 'Failed'}</span>
            </div>
          )}
        </div>
        {discoveryError && discoveryStatus === 'error' && (
          <Alert variant="warning" className="mt-3">
            <AlertCircle className="h-4 w-4" />
            <AlertDescription>{discoveryError}</AlertDescription>
          </Alert>
        )}
        <p className="text-xs text-muted-foreground mt-2">
          Validate connection and discover available resources (warehouses, databases, schemas, etc.)
        </p>
      </div>
    );
  };

  // ==========================================================================
  // RENDER: CONNECTION TAB (legacy - for create mode only)
  // ==========================================================================

  const renderConnectionTab = () => {
    // Show loading state while fetching settings in edit mode
    if (!isCreateMode && settingsLoading) {
      return (
        <div className="flex items-center justify-center py-12">
          <Spinner size="md" className="text-muted-foreground" />
          <span className="ml-2 text-sm text-muted-foreground">Loading settings...</span>
        </div>
      );
    }

    return (
      <div className="space-y-4">
        {/* Name and Slug fields */}
        {canAdmin && (
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium mb-1">
                Name <span className="text-error-foreground">*</span>
              </label>
              <input
                type="text"
                value={formData.name}
                onChange={(e) => handleNameChange(e.target.value)}
                placeholder="My Database"
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
              />
            </div>
            <div>
              <label className="block text-sm font-medium mb-1">Slug</label>
              <input
                type="text"
                value={formData.slug}
                onChange={(e) => handleSlugChange(e.target.value)}
                placeholder="my-database"
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono"
              />
              <p className="text-xs text-muted-foreground mt-1">
                {isCreateMode
                  ? 'Auto-generated from name. Used in ChartML specs and API calls'
                  : 'Used in ChartML specs and API calls'}
              </p>
            </div>
          </div>
        )}

        {/* Dashboard Auto-Refresh Setting (edit mode only, admins only) */}
        {canAdmin && !isCreateMode && (
          <div className="flex items-center justify-between py-3 px-4 bg-muted/30 rounded-lg">
            <div>
              <label className="text-sm font-medium">Allow Dashboard Auto-Refresh</label>
              <p className="text-xs text-muted-foreground mt-0.5">
                When enabled, dashboards can automatically refresh charts using this datasource.
                Disable for pay-per-query sources like BigQuery to control costs.
              </p>
            </div>
            <Switch
              checked={formData.auto_refresh_allowed}
              onCheckedChange={(checked) =>
                setFormData((prev) => ({ ...prev, auto_refresh_allowed: checked }))
              }
            />
          </div>
        )}

        {/* Connection Method selector (Connect vs Direct) — create mode only, not in personal mode */}
        {isCreateMode && schema?.connectSupported && !isPersonalMode && (
          <div className="mb-2">
            <label className="text-sm font-medium text-foreground mb-3 block">Connection Method</label>
            <div className="grid grid-cols-2 gap-3">
              <button
                type="button"
                onClick={() => setConnectionType('direct')}
                className={`p-4 rounded-lg border text-left transition-colors ${
                  connectionType === 'direct'
                    ? 'border-primary bg-primary/5'
                    : 'border-border hover:border-muted-foreground/30'
                }`}
              >
                <div className="font-medium text-sm text-foreground">Direct Connection</div>
                <div className="text-xs text-muted-foreground mt-1">
                  Enter credentials here. Kyomi connects directly to your database.
                </div>
              </button>
              <button
                type="button"
                onClick={() => setConnectionType('connect')}
                className={`p-4 rounded-lg border text-left transition-colors ${
                  connectionType === 'connect'
                    ? 'border-primary bg-primary/5'
                    : 'border-border hover:border-muted-foreground/30'
                }`}
              >
                <div className="font-medium text-sm text-foreground">Kyomi Connect</div>
                <div className="text-xs text-muted-foreground mt-1">
                  Deploy an agent in your network. Credentials never leave your infrastructure.
                </div>
              </button>
            </div>
          </div>
        )}

        {connectionType === 'connect' && isCreateMode ? (
          /* Connect mode: show info box instead of connection/credentials fields */
          <div className="rounded-lg border border-border bg-muted/30 p-4">
            <p className="text-sm text-foreground font-medium mb-2">How Kyomi Connect works</p>
            <ol className="text-sm text-muted-foreground space-y-1 list-decimal list-inside">
              <li>Save this datasource to generate a secure token</li>
              <li>Deploy the Kyomi Connect agent in your network</li>
              <li>The agent connects outbound to Kyomi — no inbound access needed</li>
            </ol>
          </div>
        ) : (
          <>
            {/* Connection Settings */}
            {schema?.connectionFields?.length > 0 && (
              <div>
                <h4 className="text-sm font-medium mb-3">
                  {canAdmin ? 'Connection Settings' : 'Connection'}
                </h4>
                {renderConnectionFields()}
                {renderSSHTunnelSection()}
              </div>
            )}

            {/* Credentials section */}
            {renderCredentialsSection()}

            {/* Test & Discover button */}
            {renderTestAndDiscoverButton()}

            {/* Discovery Fields */}
            {(isCreateMode ? discoveryStatus === 'success' : true) && renderDiscoveryFields()}
          </>
        )}
      </div>
    );
  };

  // ==========================================================================
  // RENDER: WORKSPACE SETTINGS TAB (admin only)
  // ==========================================================================

  const renderWorkspaceSettingsTab = () => {
    // Show loading state while fetching settings in edit mode
    if (!isCreateMode && settingsLoading) {
      return (
        <div className="flex items-center justify-center py-12">
          <Spinner size="md" className="text-muted-foreground" />
          <span className="ml-2 text-sm text-muted-foreground">Loading settings...</span>
        </div>
      );
    }

    // Sample datasources have read-only connection config
    if (isSampleDatasource) {
      return (
        <div className="space-y-4">
          <Alert>
            <AlertDescription>
              This is a sample datasource with pre-configured connection settings.
              Connection settings cannot be modified.
            </AlertDescription>
          </Alert>
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium mb-1 text-muted-foreground">Name</label>
              <p className="text-sm">{formData.name}</p>
            </div>
            <div>
              <label className="block text-sm font-medium mb-1 text-muted-foreground">Slug</label>
              <p className="text-sm font-mono">{formData.slug}</p>
            </div>
          </div>
          <div>
            <label className="block text-sm font-medium mb-1 text-muted-foreground">Type</label>
            <div className="flex items-center gap-2">
              <DatasourceIcon type={formData.datasource_type} className="h-4 w-4" />
              <span className="text-sm">{getProvider(formData.datasource_type)?.label || formData.datasource_type}</span>
            </div>
          </div>
        </div>
      );
    }

    const type = formData.datasource_type;
    const providerData = getProvider(type);

    return (
      <div className="space-y-4">
        {/* Name and Slug fields */}
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-sm font-medium mb-1">
              Name <span className="text-error-foreground">*</span>
            </label>
            <input
              type="text"
              value={formData.name}
              onChange={(e) => handleNameChange(e.target.value)}
              placeholder="My Database"
              className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
            />
          </div>
          <div>
            <label className="block text-sm font-medium mb-1">Slug</label>
            <input
              type="text"
              value={formData.slug}
              onChange={(e) => handleSlugChange(e.target.value)}
              placeholder="my-database"
              className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono"
            />
            <p className="text-xs text-muted-foreground mt-1">
              Used in ChartML specs and API calls
            </p>
          </div>
        </div>

        {/* Connection Settings — hidden for Connect datasources */}
        {datasource?.connection_type !== 'connect' && schema?.connectionFields?.length > 0 && (
          <div>
            <h4 className="text-sm font-medium mb-3">Connection Settings</h4>
            {renderConnectionFields()}
            {renderSSHTunnelSection()}
          </div>
        )}

        {/* Auth Mode selector - provider-specific — hidden for Connect datasources */}
        {datasource?.connection_type !== 'connect' && providerData?.schema?.authModes && (
          <div className="border-t border-border pt-4 mt-4">
            <h4 className="text-sm font-medium mb-3">Authentication Method</h4>
            <AuthModeSelector
              modes={providerData.schema.authModes}
              value={authMode}
              onChange={handleAuthModeChange}
              canAdmin={canAdmin}
              disabled={affectedUsersLoading}
              onBetaAcknowledgedChange={setBetaAcknowledged}
            />
          </div>
        )}

        {/* The following sections are irrelevant for Connect datasources */}
        {datasource?.connection_type !== 'connect' && (
          <>
            {/* OAuth Configuration (enterprise OAuth - BigQuery, Snowflake, etc.) */}
            {renderWorkspaceOAuthConfig()}

            {/* Shared Credentials section (password auth modes) */}
            {renderWorkspaceSharedCredentials()}

            {/* Service Account section (BigQuery service_account mode) */}
            {renderWorkspaceServiceAccount()}

            {/* Test & Discover button */}
            {renderTestAndDiscoverButton()}

            {/* Discovery Fields */}
            {renderDiscoveryFields()}
          </>
        )}

        {/* Dashboard Auto-Refresh Setting */}
        {!isCreateMode && (
          <div className="flex items-center justify-between py-3 px-4 bg-muted/30 rounded-lg border-t border-border mt-4">
            <div>
              <label className="text-sm font-medium">Allow Dashboard Auto-Refresh</label>
              <p className="text-xs text-muted-foreground mt-0.5">
                When enabled, dashboards can automatically refresh charts using this datasource.
                Disable for pay-per-query sources like BigQuery to control costs.
              </p>
            </div>
            <Switch
              checked={formData.auto_refresh_allowed}
              onCheckedChange={(checked) =>
                setFormData((prev) => ({ ...prev, auto_refresh_allowed: checked }))
              }
            />
          </div>
        )}

      </div>
    );
  };

  // Helper: Render OAuth config section in Workspace Settings (admin configures OAuth client)
  const renderWorkspaceOAuthConfig = () => {
    const mode = currentAuthModeSchema;
    // Only show OAuth config for modes with admin-configurable OAuth (not global OAuth like kyomi_oauth)
    if (!mode?.oauth?.configFields || mode.oauth?.global) return null;

    // Derive provider label from oauth.provider (e.g., 'bigquery-enterprise' -> 'Google Cloud', 'snowflake' -> 'Snowflake')
    const providerLabels = {
      'bigquery-enterprise': 'Google Cloud',
      'microsoft-enterprise': 'Microsoft Azure',
    };
    const providerLabel = providerLabels[mode.oauth.provider]
      || mode.oauth.provider?.charAt(0).toUpperCase() + mode.oauth.provider?.slice(1)
      || 'OAuth';

    return (
      <div className="border-t border-border pt-4 mt-4">
        <OAuthConfig
          provider={providerLabel}
          callbackPath={mode.callbackPath}
          values={formData.connection_config}
          onChange={handleConnectionConfigChange}
        />
      </div>
    );
  };

  // Helper: Render shared credentials section in Workspace Settings
  const renderWorkspaceSharedCredentials = () => {
    const mode = currentAuthModeSchema;

    // Check if current auth mode supports shared credentials
    // For providers without authModes (simple password-based), check schema-level
    const supportsShared = mode
      ? mode.supportsSharedCredentials
      : schema?.supportsSharedCredentials;
    if (!supportsShared) return null;

    // Don't show for OAuth or service account modes (they have their own flow)
    if (mode?.oauth || mode?.serviceAccount) return null;

    const sharedCreds = formData.connection_config?.shared_credentials || false;

    // Derive shared credential fields from the mode's credentialFields
    const sharedFields = mode?.credentialFields || schema?.credentialFields || [];

    return (
      <div className="border-t border-border pt-4 mt-4">
        <SharedCredentialsToggle
          enabled={sharedCreds}
          onChange={handleSharedCredentialsChange}
          canAdmin={canAdmin}
        >
          {sharedCreds && sharedFields.length > 0 && (
            <div className="space-y-3 mt-3">
              {sharedFields.map((field) => (
                <div key={`shared_${field.name}`}>
                  <label className="block text-sm font-medium mb-1">
                    Shared {field.label} <span className="text-error-foreground">*</span>
                  </label>
                  <input
                    type={field.type === 'password' ? 'password' : 'text'}
                    value={formData.connection_config[`shared_${field.name}`] || ''}
                    onChange={(e) => handleConnectionConfigChange(`shared_${field.name}`, e.target.value)}
                    placeholder={field.type === 'password' ? '••••••••' : field.placeholder || ''}
                    className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                  />
                  {field.type === 'password' && (
                    <p className="text-xs text-muted-foreground mt-1">Credentials are encrypted at rest</p>
                  )}
                </div>
              ))}
            </div>
          )}
        </SharedCredentialsToggle>
      </div>
    );
  };

  // Helper: Render service account section in Workspace Settings
  const renderWorkspaceServiceAccount = () => {
    const mode = currentAuthModeSchema;
    if (!mode?.serviceAccount) return null;

    return (
      <div className="border-t border-border pt-4 mt-4">
        <h4 className="text-sm font-medium mb-3">Service Account Configuration</h4>
        <p className="text-xs text-muted-foreground mb-3">
          Upload or paste your Google Cloud service account credentials JSON file.
          This will be used for all users accessing this datasource.
        </p>

        {/* Show current service account if configured */}
        {serviceAccountEmail && (
          <div className="flex items-center justify-between p-3 bg-muted/50 rounded-lg mb-3">
            <div className="flex items-center gap-2">
              <Check className="h-4 w-4 text-success-foreground" />
              <span className="text-sm text-foreground">
                Service Account: {serviceAccountEmail}
              </span>
            </div>
            <Button
              variant="outline"
              size="sm"
              onClick={handleServiceAccountClear}
            >
              Remove
            </Button>
          </div>
        )}

        {/* File upload and JSON paste */}
        {!serviceAccountEmail && (
          <div className="space-y-3">
            {/* File upload button */}
            <div>
              <input
                type="file"
                accept=".json"
                onChange={handleServiceAccountUpload}
                className="hidden"
                id="service-account-upload"
              />
              <Button
                variant="outline"
                onClick={() => document.getElementById('service-account-upload')?.click()}
              >
                <Upload className="h-4 w-4 mr-2" />
                Upload credentials.json
              </Button>
            </div>

            <div className="flex items-center gap-2">
              <div className="flex-1 h-px bg-border" />
              <span className="text-xs text-muted-foreground">or paste JSON</span>
              <div className="flex-1 h-px bg-border" />
            </div>

            {/* JSON textarea */}
            <div>
              <textarea
                value={serviceAccountJson}
                onChange={(e) => handleServiceAccountJsonChange(e.target.value)}
                placeholder='{"type": "service_account", "client_email": "...", ...}'
                rows={6}
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono"
              />
              <p className="text-xs text-muted-foreground mt-1">
                Paste the contents of your service account JSON file
              </p>
            </div>
          </div>
        )}

        {/* Validate & Discover + Project selection (when service account configured) */}
        {serviceAccountEmail && (
          <div className="space-y-4 mt-4">
            <div className="flex items-center gap-3">
              <Button
                variant="outline"
                onClick={() => testAndDiscover()}
                disabled={testing}
              >
                {testing ? (
                  <>
                    <Spinner size="sm" className="mr-2" />
                    Validating...
                  </>
                ) : (
                  <>
                    <Plug className="h-4 w-4 mr-2" />
                    Validate & Discover Projects
                  </>
                )}
              </Button>
              {testResult && (
                <div
                  className={`flex items-center gap-2 text-sm ${
                    testResult.success ? 'text-success-foreground' : 'text-error-foreground'
                  }`}
                >
                  {testResult.success ? (
                    <Check className="h-4 w-4" />
                  ) : (
                    <X className="h-4 w-4" />
                  )}
                  <span>{testResult.success ? 'Valid' : 'Failed'}</span>
                </div>
              )}
            </div>

            {/* Project dropdowns — workspace-level config stored in connection_config */}
            <ProjectDropdowns
              credentialsForm={{
                billing_project: formData.connection_config?.billing_project,
                default_project: formData.connection_config?.default_project,
              }}
              onCredentialsChange={handleConnectionConfigChange}
              projectsList={discoveredResources.projects || []}
              errorMessage={projectFetchError}
            />
          </div>
        )}
      </div>
    );
  };

  // ==========================================================================
  // RENDER: YOUR CREDENTIALS TAB (all users)
  // ==========================================================================

  const renderYourCredentialsTab = () => {
    // Show loading state while fetching settings in edit mode
    if (!isCreateMode && settingsLoading) {
      return (
        <div className="flex items-center justify-center py-12">
          <Spinner size="md" className="text-muted-foreground" />
          <span className="ml-2 text-sm text-muted-foreground">Loading settings...</span>
        </div>
      );
    }

    const type = formData.datasource_type;
    const sharedCreds = isUsingSharedCredentials();

    return (
      <div className="space-y-4">
        <div className="flex items-center justify-between">
          <h4 className="text-sm font-medium text-foreground">Your Credentials</h4>
          {DATASOURCE_DOCS[type] && (
            <a
              href={DATASOURCE_DOCS[type]}
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center gap-1 text-xs text-muted-foreground hover:text-primary transition-colors"
            >
              <HelpCircle className="h-3.5 w-3.5" />
              Setup Guide
            </a>
          )}
        </div>

        {/* If using shared credentials, show read-only message */}
        {sharedCreds && (
          <div className="flex items-center gap-2 p-4 bg-muted/50 rounded-lg">
            <Lock className="h-4 w-4 text-muted-foreground" />
            <div>
              <p className="text-sm text-foreground">Using shared credentials</p>
              <p className="text-xs text-muted-foreground">
                Your workspace administrator has configured shared credentials for this datasource.
                No personal credentials are required.
              </p>
            </div>
          </div>
        )}

        {/* User credentials - schema-driven for all datasource types */}
        {!sharedCreds && renderUserCredentials()}
      </div>
    );
  };

  // Schema-driven user credentials renderer — replaces per-type helpers
  const renderUserCredentials = () => {
    const mode = currentAuthModeSchema;

    // Simple providers without authModes (postgres, mysql, etc.)
    if (!mode) {
      const credentialFields = schema?.credentialFields || [];
      if (credentialFields.length === 0) return null;
      return (
        <CredentialsForm
          fields={credentialFields}
          values={credentialsForm}
          onChange={handleCredentialsChange}
          credentialFlags={credentialFlags}
          dirtyFields={dirtyCredentials}
        />
      );
    }

    // Service account mode — read-only info for users
    // Project dropdowns are on the workspace tab (they're workspace-level config)
    if (mode.serviceAccount) {
      return (
        <div className="flex items-center gap-2 p-3 bg-muted/50 rounded-lg">
          <Lock className="h-4 w-4 text-muted-foreground" />
          <div>
            <p className="text-sm text-foreground">
              {serviceAccountEmail
                ? `Using service account: ${serviceAccountEmail}`
                : 'Using service account configured by admin'}
            </p>
            <p className="text-xs text-muted-foreground">
              No personal credentials are required for this datasource.
            </p>
          </div>
        </div>
      );
    }

    // Global OAuth (BigQuery kyomi_oauth) — custom connect flow
    if (mode.oauth?.global) {
      const projectsList = oauthStatus.hasBigqueryScopes ? googleProjects : [];
      const hasDiscoveryFields = mode.credentialFields?.some((f) => f.type === 'discovery');
      return (
        <div className="space-y-4">
          <div className="space-y-3">
            {oauthStatus.hasOauth ? (
              <div className="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
                <div className="flex items-center gap-2">
                  <Check className="h-4 w-4 text-success-foreground" />
                  <span className="text-sm text-foreground">
                    {oauthStatus.oauthEmail
                      ? `Google account: ${oauthStatus.oauthEmail}`
                      : 'Google account connected'}
                  </span>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setShowDisconnectConfirm(true)}
                  disabled={oauthDisconnecting}
                >
                  {oauthDisconnecting ? (
                    <>
                      <Spinner size="sm" />
                      Disconnecting...
                    </>
                  ) : (
                    'Disconnect'
                  )}
                </Button>
              </div>
            ) : (
              <div className="space-y-3">
                <Button
                  variant="outline"
                  onClick={handleConnectBigQuery}
                  disabled={oauthConnecting}
                >
                  {oauthConnecting ? (
                    <>
                      <Spinner size="sm" className="mr-2" />
                      Connecting...
                    </>
                  ) : (
                    <>
                      <DatasourceIcon type="bigquery" className="h-4 w-4 mr-2" />
                      Connect BigQuery
                    </>
                  )}
                </Button>
                <p className="text-xs text-muted-foreground">
                  Sign in with Google to access your BigQuery projects.
                </p>
              </div>
            )}
          </div>
          {oauthStatus.hasBigqueryScopes && hasDiscoveryFields && (
            <div className="pt-4 border-t border-border">
              <ProjectDropdowns
                credentialsForm={credentialsForm}
                onCredentialsChange={handleCredentialsChange}
                projectsList={projectsList}
                errorMessage={projectFetchError}
              />
            </div>
          )}
        </div>
      );
    }

    // Per-datasource OAuth (enterprise or standard) — OAuthConnect component
    if (mode.oauth) {
      // Map OAuth providers to user-facing labels
      const oauthProviderLabels = {
        'BigQuery-enterprise': 'Google',
        'microsoft-enterprise': 'Microsoft',
      };
      const providerLabel = oauthProviderLabels[mode.oauth.provider] || mode.oauth.provider;
      const oauthConfigured = mode.oauth.configFields?.every(
        (field) => !!formData.connection_config?.[field.name]
      ) ?? true;
      // handleConnectProviderOAuth maps internally via endpointMap
      const oauthProviderType = mode.oauth.provider.toLowerCase();
      const hasDiscoveryFields = mode.credentialFields?.some((f) => f.type === 'discovery');

      return (
        <div className="space-y-4">
          <OAuthConnect
            datasourceType={formData.datasource_type}
            providerLabel={providerLabel}
            status={providerOAuthStatus}
            onConnect={() => handleConnectProviderOAuth(oauthProviderType)}
            onDisconnect={handleDisconnectProviderOAuth}
            configValid={oauthConfigured}
            helpText={`Sign in with your ${providerLabel} account.`}
            credentialStatus={credentialStatus}
          />
          {providerOAuthStatus.connected && hasDiscoveryFields && (
            <div className="pt-4 border-t border-border">
              <ProjectDropdowns
                credentialsForm={credentialsForm}
                onCredentialsChange={handleCredentialsChange}
                projectsList={googleProjects || []}
                errorMessage={projectFetchError}
              />
            </div>
          )}
        </div>
      );
    }

    // Standard credential fields (password, token, keypair, service principal, etc.)
    if (mode.credentialFields?.length > 0) {
      return (
        <CredentialsForm
          fields={mode.credentialFields}
          values={credentialsForm}
          onChange={handleCredentialsChange}
          credentialFlags={credentialFlags}
          dirtyFields={dirtyCredentials}
        />
      );
    }

    return null;
  };

  // ==========================================================================
  // RENDER: CATALOG TAB (CREATE MODE)
  // ==========================================================================

  const renderCatalogTabCreateMode = () => {
    const itemTypeLabel = schema?.catalogConfig?.label || 'Items to Index';

    return (
      <div className="space-y-4">
        <div>
          <h4 className="text-sm font-medium mb-1">{itemTypeLabel}</h4>
          <p className="text-sm text-muted-foreground mb-4">
            Select which items to include in the data catalog for AI discovery.
          </p>
        </div>

        {catalogDiscovery.loading && (
          <div className="flex items-center gap-2 py-8 justify-center">
            <Spinner size="sm" className="text-muted-foreground" />
            <span className="text-sm text-muted-foreground">
              Discovering available {catalogDiscovery.itemType}...
            </span>
          </div>
        )}

        {catalogDiscovery.error && !catalogDiscovery.loading && (
          <div className="space-y-3">
            <Alert variant="warning">
              <AlertCircle className="h-4 w-4" />
              <AlertDescription>{catalogDiscovery.error}</AlertDescription>
            </Alert>
            <Button variant="outline" size="sm" onClick={discoverCatalog}>
              <RefreshCw className="h-4 w-4 mr-2" />
              Retry
            </Button>
          </div>
        )}

        {!catalogDiscovery.loading &&
          !catalogDiscovery.error &&
          (catalogDiscovery.items.length === 0 ? (
            <div className="text-center py-8">
              <Database className="mx-auto h-12 w-12 text-muted-foreground" />
              <p className="mt-4 text-sm text-muted-foreground">
                No {catalogDiscovery.itemType} found. Make sure your connection and credentials are
                correct.
              </p>
              <Button variant="outline" size="sm" onClick={discoverCatalog} className="mt-4">
                <RefreshCw className="h-4 w-4 mr-2" />
                Retry Discovery
              </Button>
            </div>
          ) : (
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelectedCatalogItems(catalogDiscovery.items.map((i) => i.name))}
                >
                  Select All
                </Button>
                <Button variant="ghost" size="sm" onClick={() => setSelectedCatalogItems([])}>
                  Clear
                </Button>
                <span className="text-xs text-muted-foreground ml-auto">
                  {selectedCatalogItems.length} of {catalogDiscovery.items.length} selected
                </span>
              </div>

              <div className="border border-border rounded-lg divide-y divide-border max-h-60 overflow-y-auto">
                {catalogDiscovery.items.map((item) => {
                  const isSelected = selectedCatalogItems.includes(item.name);
                  return (
                    <label
                      key={item.name}
                      className="flex items-center gap-3 px-3 py-2 cursor-pointer hover:bg-accent/50 transition-colors"
                    >
                      <input
                        type="checkbox"
                        checked={isSelected}
                        onChange={() => {
                          setSelectedCatalogItems((prev) =>
                            isSelected
                              ? prev.filter((n) => n !== item.name)
                              : [...prev, item.name]
                          );
                        }}
                        className="h-4 w-4 rounded border-border"
                      />
                      <span className="text-sm font-mono">{item.name}</span>
                      {item.description && (
                        <span className="text-xs text-muted-foreground">({item.description})</span>
                      )}
                    </label>
                  );
                })}
              </div>

              <p className="text-xs text-muted-foreground">
                {selectedCatalogItems.length === 0
                  ? 'Leave empty to index all available items.'
                  : 'Selected items will be indexed for AI discovery after creation.'}
              </p>
            </div>
          ))}
      </div>
    );
  };

  // ==========================================================================
  // RENDER: MODAL FOOTER
  // ==========================================================================

  const renderFooter = () => {
    if (isCreateMode) {
      // Connect mode: single-step create (no catalog step needed)
      if (connectionType === 'connect') {
        return (
          <>
            <Button variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button onClick={handleSave} disabled={saving || !formData.name}>
              {saving ? <Spinner size="sm" className="mr-2" /> : null}
              Create
            </Button>
          </>
        );
      }

      // Direct mode: legacy connection/catalog two-step flow
      return (
        <>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          {activeTab === 'connection' ? (
            <Button
              onClick={() => {
                setActiveTab('catalog');
                if (formData.datasource_type !== 'bigquery' || oauthStatus.hasOauth) {
                  discoverCatalog();
                }
              }}
              disabled={!testResult?.success || !formData.name}
            >
              Next
            </Button>
          ) : (
            <Button onClick={handleSave} disabled={saving || (requiresBetaAccess && !betaAcknowledged)}>
              {saving ? <Spinner size="sm" className="mr-2" /> : null}
              Create
            </Button>
          )}
        </>
      );
    }

    // Edit mode
    return (
      <>
        {showDeleteButton && canAdmin && (
          <Button
            variant="destructive"
            onClick={() => setShowDeleteConfirm(true)}
            className="mr-auto"
          >
            <Trash2 className="h-4 w-4 mr-2" />
            Delete
          </Button>
        )}
        <Button onClick={handleSaveAndClose} disabled={saving || (requiresBetaAccess && !betaAcknowledged)}>
          {saving ? <Spinner size="sm" className="mr-2" /> : null}
          Save
        </Button>
      </>
    );
  };

  // ==========================================================================
  // RENDER: MAIN MODAL
  // ==========================================================================

  const providerTypes = getProviderTypes();

  // Tab configuration for edit mode
  const adminTabs = showCatalogTab ? ['workspace', 'credentials', 'catalog'] : ['workspace', 'credentials'];
  const userTabs = ['credentials'];
  const editModeTabs = canAdmin ? adminTabs : userTabs;

  const tabLabels = {
    workspace: 'Workspace Settings',
    credentials: datasource?.connection_type === 'connect' ? 'Connect' : 'Your Credentials',
    catalog: 'Catalog',
    connection: 'Connection', // Legacy for create mode
  };

  return (
    <>
      <Modal
        show={isOpen}
        onClose={onClose}
        title={connectSetup ? 'Kyomi Connect Setup' : modalTitle}
        size="lg"
        footer={connectSetup ? null : renderFooter()}
      >
        {connectSetup ? (
          // CONNECT SETUP: Show token and instructions after creating a Connect datasource
          <ConnectSetup
            token={connectSetup.token}
            datasourceName={connectSetup.name}
            datasourceType={connectSetup.type}
            onDone={() => {
              setConnectSetup(null);
              setConnectDatasource(null);
              onSaved?.(connectDatasource);
              onClose();
            }}
          />
        ) : isCreateMode ? (
          // CREATE MODE: Use legacy connection/catalog two-step flow
          <div className="space-y-4">
            {canAdmin && connectionType !== 'connect' && (
              <div className="flex border-b border-border">
                {['connection', 'catalog'].map((tab) => (
                  <button
                    key={tab}
                    onClick={() => setActiveTab(tab)}
                    disabled={tab === 'catalog' && !testResult?.success}
                    className={`px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
                      activeTab === tab
                        ? 'border-primary text-primary'
                        : 'border-transparent text-muted-foreground hover:text-foreground'
                    } ${tab === 'catalog' && !testResult?.success ? 'opacity-50 cursor-not-allowed' : ''}`}
                  >
                    {tabLabels[tab]}
                  </button>
                ))}
              </div>
            )}

            <div className="pt-2 min-h-[400px]">
              {activeTab === 'connection' && (
                <div className="space-y-4">
                  {/* Sample datasource quick-add option */}
                  {canAdmin && sampleAvailable && !sampleAlreadyAdded && (
                    <div className="flex items-center justify-between p-3 border border-border rounded-lg bg-muted/30">
                      <div className="flex items-center gap-3">
                        <Database className="h-5 w-5 text-muted-foreground" />
                        <div>
                          <p className="text-sm font-medium">Acme Analytics (Sample)</p>
                          <p className="text-xs text-muted-foreground">Try Kyomi with demo data — no setup required</p>
                        </div>
                      </div>
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={handleCreateSample}
                        disabled={creatingSample}
                      >
                        {creatingSample ? 'Adding...' : 'Add Sample'}
                      </Button>
                    </div>
                  )}

                  {canAdmin && (
                    <div>
                      <label className="block text-sm font-medium mb-1">Type</label>
                      <Select
                        value={formData.datasource_type}
                        onValueChange={handleDatasourceTypeChange}
                      >
                        <SelectTrigger>
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {providerTypes.map(({ value, label }) => (
                            <SelectItem key={value} value={value}>
                              <div className="flex items-center gap-2">
                                <DatasourceIcon type={value} className="h-4 w-4" opacity={0.8} />
                                <span>{label}</span>
                              </div>
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                  )}

                  {renderConnectionTab()}
                </div>
              )}

              {activeTab === 'catalog' && renderCatalogTabCreateMode()}
            </div>
          </div>
        ) : (
          // EDIT MODE: Use new three-tab structure (workspace/credentials/catalog)
          <div className="space-y-4">
            {/* Tab bar - show for admins with multiple tabs, hide for non-admins with single tab */}
            {editModeTabs.length > 1 && (
              <div className="flex border-b border-border">
                {editModeTabs.map((tab) => (
                  <button
                    key={tab}
                    onClick={() => setActiveTab(tab)}
                    className={`px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
                      activeTab === tab
                        ? 'border-primary text-primary'
                        : 'border-transparent text-muted-foreground hover:text-foreground'
                    }`}
                  >
                    {tabLabels[tab]}
                  </button>
                ))}
              </div>
            )}

            {/* Datasource type badge (edit mode) */}
            <div className="flex items-center gap-2">
              <DatasourceIcon type={formData.datasource_type} className="h-5 w-5" />
              <Badge variant="outline">{provider?.label}</Badge>
            </div>

            <div className="min-h-[400px]">
              {activeTab === 'workspace' && canAdmin && renderWorkspaceSettingsTab()}

              {activeTab === 'credentials' && (
                datasource?.connection_type === 'connect'
                  ? <ConnectStatus datasourceId={datasource.id} datasourceType={datasource.datasource_type} datasourceName={datasource.name} />
                  : renderYourCredentialsTab()
              )}

              {activeTab === 'catalog' && canAdmin && datasource && (
                <div className="space-y-0">
                  <CatalogSection
                    datasource={{ ...datasource, connection_config: formData.connection_config }}
                    apiClient={apiClient}
                    isAdmin={canAdmin}
                    onConfigChange={handleCatalogConfigChange}
                  />
                  {!isSampleDatasource && datasource?.connection_type !== 'connect' && (
                    <IndexingCredentials
                      datasourceType={formData.datasource_type}
                      connectionConfig={formData.connection_config}
                      onConnectionConfigChange={handleConnectionConfigChange}
                      disabled={!canAdmin}
                    />
                  )}
                </div>
              )}
            </div>
          </div>
        )}
      </Modal>

      {/* Delete Confirmation */}
      {showDeleteButton && canAdmin && !isCreateMode && (
        <ConfirmDialog
          isOpen={showDeleteConfirm}
          onConfirm={handleDelete}
          onCancel={() => setShowDeleteConfirm(false)}
          title="Delete Datasource?"
          message={`Are you sure you want to delete "${datasource?.name}"? This cannot be undone.`}
          confirmText="Delete"
          variant="destructive"
        />
      )}

      {/* Google Disconnect Confirmation */}
      <ConfirmDialog
        isOpen={showDisconnectConfirm}
        onConfirm={handleDisconnectGoogle}
        onCancel={() => setShowDisconnectConfirm(false)}
        title="Disconnect Google Account?"
        message="This will disconnect your Google account from all BigQuery datasources. You will need to reconnect to use BigQuery."
        confirmText="Disconnect"
        variant="destructive"
      />

      {/* Auth Mode Change Warning */}
      <ConfirmDialog
        isOpen={showAuthModeWarning}
        onConfirm={() => {
          applyAuthModeChange(pendingAuthMode);
          setShowAuthModeWarning(false);
          setPendingAuthMode(null);
        }}
        onCancel={() => {
          setShowAuthModeWarning(false);
          setPendingAuthMode(null);
        }}
        title="Change Authentication Method?"
        message={`${affectedUsersCount} user${affectedUsersCount === 1 ? ' has' : 's have'} existing credentials that will become invalid. They will need to set up new credentials after this change.`}
        confirmText="Change Anyway"
        variant="destructive"
      />
    </>
  );
}
