// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useCallback, useRef } from 'react';
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
} from 'lucide-react';
import { trackEvent } from '../../utils/analytics';
import { Spinner } from '../ui/spinner';
import { Button } from '../ui/button';
import { Badge } from '../ui/badge';
import { Alert, AlertDescription } from '../ui/alert';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import Modal from '../Modal';
import ConfirmDialog from '../ConfirmDialog';
import { toast } from '../../lib/toast';
import CatalogSection from './CatalogSection';
import { DatasourceIcon } from '../ui/DatasourceIcon';
import ConnectionFormRenderer from './ConnectionFormRenderer';
import { getConnectionSchema } from './connectionFormSchemas';

/**
 * DatasourceModal - Standalone modal for creating/editing datasources
 *
 * This component handles all modal-related state and logic for datasource management.
 * It supports both create mode (new datasource) and edit mode (existing datasource).
 *
 * Usage:
 *
 * // Create mode
 * <DatasourceModal
 *   isOpen={showModal}
 *   onClose={() => setShowModal(false)}
 *   onSaved={(datasource) => handleDatasourceSaved(datasource)}
 *   title="Add Datasource"
 *   apiClient={apiClient}
 *   canAdmin={true}
 * />
 *
 * // Edit mode
 * <DatasourceModal
 *   isOpen={showModal}
 *   onClose={() => setShowModal(false)}
 *   onSaved={(datasource) => handleDatasourceSaved(datasource)}
 *   datasource={selectedDatasource}
 *   apiClient={apiClient}
 *   canAdmin={isAdmin || isOwner}
 * />
 *
 * Props:
 * - isOpen: boolean - Whether the modal is visible
 * - onClose: () => void - Callback when modal is closed
 * - apiClient: AxiosInstance - API client for backend calls
 * - datasource?: object | null - Datasource to edit (null = create mode)
 * - canAdmin: boolean - Whether user can edit connection settings
 * - onSaved?: (datasource) => void - Callback after successful save
 * - onDeleted?: (datasource) => void - Callback after delete
 * - title?: string - Override default title
 * - showCatalogTab?: boolean - Show catalog tab (default true for admin)
 * - showDeleteButton?: boolean - Show delete button (default true for admin in edit mode)
 */
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
  user = null, // User object for subscription tier checking
}) {
  // ==========================================================================
  // STATE
  // ==========================================================================

  // Form state
  const [formData, setFormData] = useState({
    name: '',
    slug: '',
    datasource_type: 'bigquery',
    connection_config: {},
  });
  const [credentialsForm, setCredentialsForm] = useState({});
  const [settingsData, setSettingsData] = useState(null);
  const [settingsLoading, setSettingsLoading] = useState(false);

  // Tab state
  const [activeTab, setActiveTab] = useState('connection');

  // Operation state
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState(null);

  // OAuth state (BigQuery)
  const [oauthStatus, setOauthStatus] = useState({
    hasOauth: false,
    oauthEmail: null,
    hasBigqueryScopes: false,
    needsBigqueryConnect: true,
  });
  const [oauthConnecting, setOauthConnecting] = useState(false);
  const [showDisconnectConfirm, setShowDisconnectConfirm] = useState(false);
  const [disconnecting, setDisconnecting] = useState(false);
  const [googleProjects, setGoogleProjects] = useState([]);

  // Snowflake auth method and OAuth state
  const [snowflakeAuthMethod, setSnowflakeAuthMethod] = useState('password');
  const [snowflakeOAuthStatus, setSnowflakeOAuthStatus] = useState({
    connected: false,
    email: null,
    connecting: false,
    disconnecting: false,
  });

  // BigQuery auth mode state (kyomi_oauth, enterprise_oauth, service_account)
  const [bigqueryAuthMode, setBigqueryAuthMode] = useState('kyomi_oauth');
  const [bigqueryEnterpriseOAuthStatus, setBigqueryEnterpriseOAuthStatus] = useState({
    connected: false,
    email: null,
    connecting: false,
    disconnecting: false,
  });
  const [serviceAccountFile, setServiceAccountFile] = useState(null);
  const [serviceAccountJson, setServiceAccountJson] = useState('');
  const [serviceAccountEmail, setServiceAccountEmail] = useState('');
  const serviceAccountInputRef = useRef(null);

  // Catalog discovery (create mode)
  const [catalogDiscovery, setCatalogDiscovery] = useState({
    loading: false,
    items: [],
    itemType: 'items',
    error: null,
  });
  const [selectedCatalogItems, setSelectedCatalogItems] = useState([]);

  // Resource discovery state (for universal datasource setup flow)
  const [discoveredResources, setDiscoveredResources] = useState({});
  const [discoveryStatus, setDiscoveryStatus] = useState('idle'); // 'idle', 'loading', 'success', 'error'
  const [discoveryError, setDiscoveryError] = useState(null);

  // Delete confirmation (edit mode)
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);

  // Datasource types (fetched from backend)
  const [datasourceTypes, setDatasourceTypes] = useState({});

  // ==========================================================================
  // DERIVED STATE
  // ==========================================================================

  const isCreateMode = datasource === null;

  // Title logic
  const modalTitle = titleOverride
    ? titleOverride
    : isCreateMode
      ? 'Add Datasource'
      : `${datasource?.name || 'Datasource'} Settings`;

  // ==========================================================================
  // EFFECTS
  // ==========================================================================

  // Fetch datasource types on mount
  useEffect(() => {
    if (apiClient) {
      fetchDatasourceTypes();
    }
  }, [apiClient]);

  // Initialize modal state when opened or datasource changes
  useEffect(() => {
    if (!isOpen) return;

    if (isCreateMode) {
      // Create mode: reset to empty form
      resetFormState();
    } else if (datasource) {
      // Edit mode: immediately set auth mode from connection_config to avoid flash of wrong UI
      if (datasource.datasource_type === 'snowflake') {
        const authMode = datasource.connection_config?.auth_mode;
        if (authMode) {
          setSnowflakeAuthMethod(authMode);
        }
      } else if (datasource.datasource_type === 'bigquery') {
        const authMode = datasource.connection_config?.auth_mode;
        if (authMode) {
          setBigqueryAuthMode(authMode);
        }
      }
      // Then load full settings from API
      loadDatasourceSettings(datasource);
    }
  }, [isOpen, datasource?.id]);

  // Listen for OAuth popup completion (BigQuery and Snowflake)
  useEffect(() => {
    const handleOAuthMessage = async (event) => {
      // Verify origin
      if (event.origin !== window.location.origin) return;

      // BigQuery OAuth messages
      if (event.data?.type === 'GOOGLE_OAUTH_SUCCESS') {
        trackEvent('ds_oauth_success', { props: { datasource_type: 'bigquery', provider: 'google' } });
        setOauthConnecting(false);
        toast.success('BigQuery connected successfully');

        // Refresh OAuth status
        setOauthStatus({
          hasOauth: true,
          oauthEmail: event.data.data?.email || null,
          hasBigqueryScopes: true,
          needsBigqueryConnect: false,
        });

        // Set testResult so user can proceed to catalog tab in create mode
        setTestResult({ success: true, message: 'Connected to Google' });
        setDiscoveryStatus('success');

        // Fetch available projects for credentials dropdowns
        fetchGoogleProjects();

        // If on catalog tab, auto-discover projects for indexing
        if (activeTab === 'catalog') {
          discoverCatalog();
        }
      } else if (event.data?.type === 'GOOGLE_OAUTH_ERROR') {
        trackEvent('ds_oauth_error', { props: { datasource_type: 'bigquery', provider: 'google', error: event.data.error || 'Unknown error' } });
        setOauthConnecting(false);
        toast.error(event.data.error || 'Failed to connect BigQuery');
      }

      // Snowflake OAuth messages
      if (event.data?.type === 'SNOWFLAKE_OAUTH_SUCCESS') {
        trackEvent('ds_oauth_success', { props: { datasource_type: 'snowflake', provider: 'snowflake' } });
        setSnowflakeOAuthStatus({
          connected: true,
          email: event.data.data?.provider_email || null,
          connecting: false,
        });
        // Set auth method to OAuth
        setSnowflakeAuthMethod('oauth');
        toast.success('Snowflake OAuth connected successfully');
        // Auto-discover resources after OAuth connects (like BigQuery)
        testAndDiscover();
      } else if (event.data?.type === 'SNOWFLAKE_OAUTH_ERROR') {
        trackEvent('ds_oauth_error', { props: { datasource_type: 'snowflake', provider: 'snowflake', error: event.data.error || 'Unknown error' } });
        setSnowflakeOAuthStatus((prev) => ({
          ...prev,
          connecting: false,
        }));
        toast.error(event.data.error || 'Failed to connect Snowflake via OAuth');
      }

      // BigQuery Enterprise OAuth messages
      if (event.data?.type === 'BIGQUERY_ENTERPRISE_OAUTH_SUCCESS') {
        trackEvent('ds_oauth_success', { props: { datasource_type: 'bigquery', provider: 'bigquery_enterprise' } });
        setBigqueryEnterpriseOAuthStatus({
          connected: true,
          email: event.data.data?.email || event.data.data?.provider_email || null,
          connecting: false,
          disconnecting: false,
        });
        toast.success('BigQuery Enterprise OAuth connected successfully');

        // Set testResult so user can proceed to catalog tab in create mode
        setTestResult({ success: true, message: 'Connected to Google' });
        setDiscoveryStatus('success');

        // Fetch available projects
        fetchGoogleProjects();
      } else if (event.data?.type === 'BIGQUERY_ENTERPRISE_OAUTH_ERROR') {
        trackEvent('ds_oauth_error', { props: { datasource_type: 'bigquery', provider: 'bigquery_enterprise', error: event.data.error || 'Unknown error' } });
        setBigqueryEnterpriseOAuthStatus((prev) => ({
          ...prev,
          connecting: false,
          disconnecting: false,
        }));
        toast.error(event.data.error || 'Failed to connect BigQuery via Enterprise OAuth');
      }
    };

    window.addEventListener('message', handleOAuthMessage);
    return () => window.removeEventListener('message', handleOAuthMessage);
  }, [activeTab]);

  // ==========================================================================
  // API FUNCTIONS
  // ==========================================================================

  const fetchDatasourceTypes = async () => {
    if (!apiClient) return;
    try {
      const response = await apiClient.get('/api/v1/datasources/types');
      const types = response.data?.types || [];
      // Convert array to object keyed by type_id
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

  const fetchGoogleProjects = async () => {
    try {
      const response = await apiClient.get('/api/v1/auth/google-oauth/projects');
      setGoogleProjects(response.data?.projects || []);
    } catch (error) {
      setGoogleProjects([]);
    }
  };

  const loadDatasourceSettings = async (ds) => {
    setSettingsLoading(true);
    // Initialize form data from datasource
    setFormData({
      name: ds.name,
      slug: ds.slug || '',
      datasource_type: ds.datasource_type,
      connection_config: ds.connection_config || {},
    });

    setActiveTab('connection');
    setTestResult(null);

    // Load settings from API
    try {
      const response = await apiClient.get(`/api/v1/datasources/${ds.id}/settings`);
      setSettingsData(response.data);

      // Merge connection_config from settings response
      if (response.data.connection_config) {
        setFormData((prev) => ({
          ...prev,
          connection_config: {
            ...prev.connection_config,
            ...response.data.connection_config,
            // Preserve sensitive fields that might be sanitized
            shared_password:
              response.data.connection_config.shared_password ||
              prev.connection_config.shared_password ||
              '',
          },
        }));
      }

      const userSettings = response.data.user_settings || {};

      if (ds.datasource_type === 'bigquery') {
        // Set BigQuery auth mode from connection_config
        const authMode = response.data.connection_config?.auth_mode || 'kyomi_oauth';
        setBigqueryAuthMode(authMode);

        setCredentialsForm({
          billing_project: userSettings.billing_project || '',
          default_project: userSettings.default_project || '',
          query_size_limit_gb: userSettings.query_size_limit_gb || 10,
        });

        if (authMode === 'kyomi_oauth') {
          // Kyomi OAuth - use global OAuth status
          setOauthStatus({
            hasOauth: response.data.has_oauth || false,
            oauthEmail: response.data.oauth_email || null,
            hasBigqueryScopes: response.data.has_bigquery_scopes || false,
            needsBigqueryConnect: response.data.needs_bigquery_connect ?? true,
          });
          // Only fetch projects if user has BigQuery scopes
          if (response.data.has_bigquery_scopes) {
            fetchGoogleProjects();
          }
        } else if (authMode === 'enterprise_oauth') {
          // Enterprise OAuth - check per-datasource OAuth status
          // Backend returns has_oauth/oauth_email for enterprise_oauth mode too
          setBigqueryEnterpriseOAuthStatus({
            connected: response.data.has_oauth || false,
            email: response.data.oauth_email || null,
            connecting: false,
            disconnecting: false,
          });
          if (response.data.has_oauth) {
            fetchGoogleProjects();
          }
        } else if (authMode === 'service_account') {
          // Service Account - show service account email if configured
          setServiceAccountEmail(response.data.service_account_email || '');
        }
      } else if (ds.datasource_type === 'snowflake') {
        // Set Snowflake auth mode from connection_config (like BigQuery)
        // Fall back to detection if auth_mode not set
        const hasOAuth = response.data.has_oauth || false;
        const hasPrivateKey = userSettings.private_key && userSettings.private_key.length > 0;
        const savedAuthMode = response.data.connection_config?.auth_mode;

        let authMethod = savedAuthMode;
        if (!authMethod) {
          // Detect from credentials if not explicitly set
          if (hasOAuth) {
            authMethod = 'oauth';
          } else if (hasPrivateKey) {
            authMethod = 'keypair';
          } else {
            authMethod = 'password';
          }
        }
        setSnowflakeAuthMethod(authMethod);

        if (authMethod === 'oauth' && hasOAuth) {
          setSnowflakeOAuthStatus({
            connected: true,
            email: response.data.oauth_email || null,
            connecting: false,
          });
          // Auto-discover resources when loading existing OAuth datasource (like BigQuery)
          // Pass the loaded connection_config directly (React state may be stale)
          const loadedConfig = {
            ...ds.connection_config,
            ...response.data.connection_config,
          };
          setTimeout(() => testAndDiscover(loadedConfig), 100);
        }

        setCredentialsForm({
          username: userSettings.username || '',
          password: '',
          private_key: '',
          private_key_passphrase: '',
        });
      } else {
        setCredentialsForm({
          username: userSettings.username || '',
          password: '',
        });
      }
      setSettingsLoading(false);
    } catch (error) {
      setSettingsData(null);
      setCredentialsForm({});
      setSettingsLoading(false);
    }
  };

  // ==========================================================================
  // FORM HELPERS
  // ==========================================================================

  const resetFormState = () => {
    // Track datasource configuration started
    trackEvent('ds_config_started', { props: { datasource_type: 'bigquery' } });

    setFormData({
      name: '',
      slug: '',
      datasource_type: 'bigquery',
      connection_config: {},
    });
    setCredentialsForm({});
    setSettingsData(null);
    setSettingsLoading(false);
    setActiveTab('connection');
    setTestResult(null);
    setCatalogDiscovery({ loading: false, items: [], itemType: 'items', error: null });
    setSelectedCatalogItems([]);
    setSlugManuallyEdited(false);
    setSnowflakeAuthMethod('password');
    setSnowflakeOAuthStatus({
      connected: false,
      email: null,
      connecting: false,
      disconnecting: false,
    });
    setOauthStatus({
      hasOauth: false,
      oauthEmail: null,
      hasBigqueryScopes: false,
      needsBigqueryConnect: true,
    });
    setGoogleProjects([]);
    // Reset BigQuery auth mode state
    setBigqueryAuthMode('kyomi_oauth');
    setBigqueryEnterpriseOAuthStatus({
      connected: false,
      email: null,
      connecting: false,
      disconnecting: false,
    });
    setServiceAccountFile(null);
    setServiceAccountJson('');
    setServiceAccountEmail('');
    // Reset discovery state
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

  // Track if user has manually edited slug
  const [slugManuallyEdited, setSlugManuallyEdited] = useState(false);

  const handleNameChange = (newName) => {
    // Auto-generate slug as user types (if they haven't manually edited it)
    if (!slugManuallyEdited) {
      const generatedSlug = generateSlug(newName);
      setFormData((prev) => ({ ...prev, name: newName, slug: generatedSlug }));
    } else {
      setFormData((prev) => ({ ...prev, name: newName }));
    }
  };

  const handleSlugChange = (newSlug) => {
    // User is manually editing slug
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
    // Track datasource type selection
    trackEvent('ds_type_selected', { props: { datasource_type: newType } });

    setFormData((prev) => ({
      ...prev,
      datasource_type: newType,
      connection_config: {},
    }));
    // Reset discovery state when type changes
    setDiscoveredResources({});
    setDiscoveryStatus('idle');
    setDiscoveryError(null);
    setTestResult(null);
    setCredentialsForm({});
    setSelectedCatalogItems([]);
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

  /**
   * Generic OAuth connect function - works with any registered OAuth provider.
   * Opens a popup to start the OAuth flow and listens for success/error messages.
   *
   * @param {string} provider - OAuth provider name (e.g., 'google', 'snowflake')
   * @param {Function} onSuccess - Callback when OAuth succeeds
   * @param {Function} onError - Callback when OAuth fails
   */
  const startOAuthConnect = (provider, onSuccess, onError) => {
    trackEvent('ds_oauth_started', { props: { datasource_type: formData.datasource_type, provider } });

    // Use new generic OAuth endpoint pattern
    const url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/${provider}/connect`;
    const width = 500;
    const height = 600;
    const left = window.screenX + (window.outerWidth - width) / 2;
    const top = window.screenY + (window.outerHeight - height) / 2;

    const popup = window.open(
      url,
      `${provider}-oauth`,
      `width=${width},height=${height},left=${left},top=${top},popup=1`
    );

    if (!popup || popup.closed) {
      trackEvent('ds_oauth_error', { props: { datasource_type: formData.datasource_type, provider, error: 'Popup blocked' } });
      onError?.('Popup was blocked. Please allow popups for this site.');
      return;
    }

    // Listen for messages from popup
    const handleMessage = (event) => {
      if (event.origin !== window.location.origin) return;

      const upperProvider = provider.toUpperCase();
      if (event.data.type === `${upperProvider}_OAUTH_SUCCESS`) {
        onSuccess?.(event.data);
        window.removeEventListener('message', handleMessage);
      } else if (event.data.type === `${upperProvider}_OAUTH_ERROR`) {
        onError?.(event.data.error);
        window.removeEventListener('message', handleMessage);
      }
    };

    window.addEventListener('message', handleMessage);

    // Monitor popup for manual close
    const checkPopup = setInterval(() => {
      if (popup.closed) {
        clearInterval(checkPopup);
        window.removeEventListener('message', handleMessage);
      }
    }, 500);
  };

  const handleConnectBigQuery = () => {
    setOauthConnecting(true);
    // Use legacy Google-specific endpoint for backward compatibility
    const url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/google-oauth/connect`;
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

    // Monitor popup
    const checkPopup = setInterval(() => {
      if (popup.closed) {
        clearInterval(checkPopup);
        setOauthConnecting((prev) => {
          if (prev) return false;
          return prev;
        });
      }
    }, 500);
  };

  const handleDisconnectGoogle = async () => {
    setDisconnecting(true);
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
      setCredentialsForm((prev) => ({
        ...prev,
        billing_project: '',
        default_project: '',
      }));
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to disconnect Google account');
    } finally {
      setDisconnecting(false);
      setShowDisconnectConfirm(false);
    }
  };

  /**
   * Connect to Snowflake via OAuth.
   * Requires OAuth credentials (oauth_client_id, oauth_client_secret) to be configured
   * in the datasource's connection_config.
   */
  const handleConnectSnowflake = () => {
    setSnowflakeOAuthStatus((prev) => ({ ...prev, connecting: true }));

    // Include datasource_slug so backend knows which datasource's OAuth credentials to use
    const datasourceSlug = datasource?.slug || formData.slug;
    const url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/snowflake/connect?datasource_slug=${encodeURIComponent(datasourceSlug)}`;

    const width = 500;
    const height = 600;
    const left = window.screenX + (window.outerWidth - width) / 2;
    const top = window.screenY + (window.outerHeight - height) / 2;

    const popup = window.open(
      url,
      'snowflake-oauth',
      `width=${width},height=${height},left=${left},top=${top},popup=1`
    );

    if (!popup || popup.closed) {
      setSnowflakeOAuthStatus((prev) => ({ ...prev, connecting: false }));
      toast.error('Popup was blocked. Please allow popups for this site.');
      return;
    }

    // Monitor popup for manual close
    const checkPopup = setInterval(() => {
      if (popup.closed) {
        clearInterval(checkPopup);
        setSnowflakeOAuthStatus((prev) => {
          if (prev.connecting) return { ...prev, connecting: false };
          return prev;
        });
      }
    }, 500);
  };

  /**
   * Check if Snowflake OAuth is configured for this datasource.
   * Returns true if oauth_client_id and oauth_client_secret are set.
   */
  const isSnowflakeOAuthConfigured = () => {
    const config = getConnectionConfig();
    return !!(config.oauth_client_id && config.oauth_client_secret);
  };

  /**
   * Connect to BigQuery via Enterprise OAuth.
   * Uses per-datasource OAuth credentials configured by admin.
   */
  const handleConnectBigQueryEnterprise = () => {
    setBigqueryEnterpriseOAuthStatus((prev) => ({ ...prev, connecting: true }));

    // Include datasource_slug so backend knows which datasource's OAuth credentials to use
    const datasourceSlug = datasource?.slug || formData.slug;
    const url = `${import.meta.env.VITE_API_BASE_URL || ''}/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=${encodeURIComponent(datasourceSlug)}`;

    const width = 500;
    const height = 600;
    const left = window.screenX + (window.outerWidth - width) / 2;
    const top = window.screenY + (window.outerHeight - height) / 2;

    const popup = window.open(
      url,
      'bigquery-enterprise-oauth',
      `width=${width},height=${height},left=${left},top=${top},popup=1`
    );

    if (!popup || popup.closed) {
      setBigqueryEnterpriseOAuthStatus((prev) => ({ ...prev, connecting: false }));
      toast.error('Popup was blocked. Please allow popups for this site.');
      return;
    }

    // Monitor popup for manual close
    const checkPopup = setInterval(() => {
      if (popup.closed) {
        clearInterval(checkPopup);
        setBigqueryEnterpriseOAuthStatus((prev) => {
          if (prev.connecting) return { ...prev, connecting: false };
          return prev;
        });
      }
    }, 500);
  };

  /**
   * Check if BigQuery Enterprise OAuth is configured for this datasource.
   * Returns true if oauth_client_id and oauth_client_secret are set in connection_config.
   */
  const isBigQueryEnterpriseOAuthConfigured = () => {
    const config = getConnectionConfig();
    return !!(config.oauth_client_id && config.oauth_client_secret);
  };

  /**
   * Disconnect BigQuery Enterprise OAuth credentials.
   * Deletes the user's OAuth credentials stored in UserDatasourceCredential.
   */
  const handleDisconnectBigQueryEnterprise = async () => {
    const datasourceSlug = datasource?.slug || formData.slug;
    if (!datasourceSlug) {
      toast.error('Cannot disconnect: datasource not saved yet');
      return;
    }

    setBigqueryEnterpriseOAuthStatus((prev) => ({ ...prev, disconnecting: true }));

    try {
      await apiClient.delete(`/api/v1/datasources/${datasourceSlug}/credentials`);
      setBigqueryEnterpriseOAuthStatus({
        connected: false,
        email: null,
        connecting: false,
        disconnecting: false,
      });
      setGoogleProjects([]);
      setCredentialsForm((prev) => ({
        ...prev,
        billing_project: '',
        default_project: '',
      }));
      toast.success('BigQuery Enterprise OAuth disconnected');
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to disconnect');
      setBigqueryEnterpriseOAuthStatus((prev) => ({ ...prev, disconnecting: false }));
    }
  };

  /**
   * Disconnect Snowflake OAuth credentials.
   * Deletes the user's OAuth credentials stored in UserDatasourceCredential.
   */
  const handleDisconnectSnowflake = async () => {
    const datasourceSlug = datasource?.slug || formData.slug;
    if (!datasourceSlug) {
      toast.error('Cannot disconnect: datasource not saved yet');
      return;
    }

    setSnowflakeOAuthStatus((prev) => ({ ...prev, disconnecting: true }));

    try {
      await apiClient.delete(`/api/v1/datasources/${datasourceSlug}/credentials`);
      setSnowflakeOAuthStatus({
        connected: false,
        email: null,
        connecting: false,
        disconnecting: false,
      });
      // Clear discovered resources since credentials are gone
      setDiscoveredResources({});
      setDiscoveryStatus('idle');
      setTestResult(null);
      toast.success('Snowflake OAuth disconnected');
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to disconnect');
      setSnowflakeOAuthStatus((prev) => ({ ...prev, disconnecting: false }));
    }
  };

  /**
   * Handle service account JSON file upload.
   */
  const handleServiceAccountFileUpload = (event) => {
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

        // Validate it looks like a service account JSON
        if (!parsed.client_email || !parsed.private_key) {
          toast.error('Invalid service account file. Missing required fields.');
          return;
        }

        setServiceAccountFile(file);
        setServiceAccountJson(jsonContent);
        setServiceAccountEmail(parsed.client_email);

        // Store in connection_config
        handleConnectionConfigChange('service_account_json', jsonContent);

        toast.success('Service account file loaded');
      } catch (err) {
        toast.error('Invalid JSON file');
      }
    };
    reader.readAsText(file);
  };

  /**
   * Handle service account JSON paste/edit.
   */
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
      // Don't update if invalid JSON - user is still typing
      setServiceAccountEmail('');
    }
  };

  /**
   * Handle BigQuery auth mode change.
   * Clears irrelevant fields when switching modes.
   */
  const handleBigQueryAuthModeChange = (newMode) => {
    setBigqueryAuthMode(newMode);

    // Store auth_mode in connection_config
    handleConnectionConfigChange('auth_mode', newMode);

    // Clear irrelevant fields when switching modes
    if (newMode === 'kyomi_oauth') {
      // Clear enterprise OAuth credentials
      handleConnectionConfigChange('oauth_client_id', '');
      handleConnectionConfigChange('oauth_client_secret', '');
      handleConnectionConfigChange('service_account_json', '');
      setBigqueryEnterpriseOAuthStatus({ connected: false, email: null, connecting: false, disconnecting: false });
      setServiceAccountJson('');
      setServiceAccountEmail('');
      setServiceAccountFile(null);
    } else if (newMode === 'enterprise_oauth') {
      // Clear service account
      handleConnectionConfigChange('service_account_json', '');
      setServiceAccountJson('');
      setServiceAccountEmail('');
      setServiceAccountFile(null);
    } else if (newMode === 'service_account') {
      // Clear enterprise OAuth credentials
      handleConnectionConfigChange('oauth_client_id', '');
      handleConnectionConfigChange('oauth_client_secret', '');
      setBigqueryEnterpriseOAuthStatus({ connected: false, email: null, connecting: false, disconnecting: false });
    }
  };

  // ==========================================================================
  // SAVE FUNCTIONS
  // ==========================================================================

  const handleSave = async () => {
    const dsType = formData.datasource_type;
    trackEvent('ds_save_started', { props: { datasource_type: dsType, mode: isCreateMode ? 'create' : 'edit' } });

    setSaving(true);

    try {
      if (isCreateMode) {
        // CREATE MODE
        const catalogConfigKey =
          {
            bigquery: 'catalog_projects',
            postgres: 'catalog_schemas',
            clickhouse: 'catalog_databases',
            snowflake: 'catalog_databases',
            databricks: 'catalog_databases',
            redshift: 'catalog_schemas',
            mysql: 'catalog_databases',
          }[formData.datasource_type] || 'catalog_items';

        const connectionConfigWithCatalog = {
          ...formData.connection_config,
          [catalogConfigKey]: selectedCatalogItems,
        };

        const createPayload = {
          name: formData.name,
          datasource_type: formData.datasource_type,
          connection_config: connectionConfigWithCatalog,
        };

        if (formData.slug) {
          createPayload.slug = formData.slug;
        }

        const response = await apiClient.post('/api/v1/datasources', createPayload);
        const newDatasource = response.data;

        // Save credentials if not using shared credentials
        const isShared = formData.connection_config?.shared_credentials;
        if (!isShared && Object.keys(credentialsForm).length > 0) {
          await apiClient.post(`/api/v1/datasources/${newDatasource.id}/credentials`, {
            credentials: credentialsForm,
          });
        }

        trackEvent('ds_save_success', { props: { datasource_type: dsType, mode: 'create' } });
        toast.success('Datasource created');
        onClose();
        onSaved?.(newDatasource);
      } else {
        // EDIT MODE
        if (canAdmin && activeTab === 'connection') {
          await apiClient.put(`/api/v1/datasources/${datasource.id}`, {
            name: formData.name,
            slug: formData.slug,
            connection_config: formData.connection_config,
          });
        }

        // Save credentials if not shared and user has credential fields to save
        const isShared =
          formData.connection_config?.shared_credentials || settingsData?.shared_credentials;
        if (!isShared && activeTab === 'connection' && Object.keys(credentialsForm).length > 0) {
          await apiClient.post(`/api/v1/datasources/${datasource.id}/credentials`, {
            credentials: credentialsForm,
          });
        }

        trackEvent('ds_save_success', { props: { datasource_type: dsType, mode: 'edit' } });
        toast.success('Settings saved');
        onSaved?.(datasource);
      }
    } catch (error) {
      const errorMsg = error.response?.data?.detail || 'Failed to save';
      trackEvent('ds_save_error', { props: { datasource_type: dsType, error: errorMsg } });
      toast.error(errorMsg);
    } finally {
      setSaving(false);
    }
  };

  const handleSaveAndClose = async () => {
    setSaving(true);

    try {
      // Save connection changes
      if (canAdmin) {
        await apiClient.put(`/api/v1/datasources/${datasource.id}`, {
          name: formData.name,
          slug: formData.slug,
          connection_config: formData.connection_config,
        });
      }

      // Save credentials if not shared and user entered new password
      const isShared =
        formData.connection_config?.shared_credentials || settingsData?.shared_credentials;
      if (!isShared && credentialsForm.password?.length > 0) {
        await apiClient.post(`/api/v1/datasources/${datasource.id}/credentials`, {
          credentials: credentialsForm,
        });
      }

      onClose();
      onSaved?.(datasource);
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to save');
    } finally {
      setSaving(false);
    }
  };

  // ==========================================================================
  // DELETE FUNCTION
  // ==========================================================================

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

  // ==========================================================================
  // TEST & DISCOVER FUNCTIONS
  // ==========================================================================

  const testConnection = async () => {
    const dsType = formData.datasource_type;
    trackEvent('ds_test_started', { props: { datasource_type: dsType } });

    setTesting(true);
    setTestResult(null);
    try {
      let response;

      if (isCreateMode) {
        response = await apiClient.post('/api/v1/datasources/test-connection', {
          datasource_type: formData.datasource_type,
          connection_config: formData.connection_config,
          credentials: credentialsForm,
        });
      } else {
        const payload = {};
        if (credentialsForm.password?.length > 0) {
          payload.credentials = credentialsForm;
        }
        response = await apiClient.post(`/api/v1/datasources/${datasource.id}/test`, payload);
      }

      setTestResult(response.data);
      if (response.data.success) {
        trackEvent('ds_test_success', { props: { datasource_type: dsType } });
        toast.success(response.data.message);
      } else {
        trackEvent('ds_test_error', { props: { datasource_type: dsType, error: response.data.message } });
        toast.error(response.data.message);
      }
    } catch (error) {
      const message = error.response?.data?.detail || 'Connection test failed';
      trackEvent('ds_test_error', { props: { datasource_type: dsType, error: message } });
      setTestResult({ success: false, message });
      toast.error(message);
    } finally {
      setTesting(false);
    }
  };

  /**
   * Test connection AND discover available resources.
   * This is the new universal datasource setup flow.
   * Calls /api/v1/datasources/discover which validates connection and returns
   * all discoverable resources (databases, schemas, warehouses, projects, catalogs).
   */
  const testAndDiscover = async (overrideConfig = null) => {
    // Use override config if provided (for auto-discover after settings load)
    // Otherwise use datasource prop for type (stable), formData for config (has unmasked values)
    const effectiveDatasourceType = datasource?.datasource_type || formData.datasource_type;
    const effectiveConnectionConfig = overrideConfig || formData.connection_config;

    trackEvent('ds_discover_started', { props: { datasource_type: effectiveDatasourceType } });

    setTesting(true);
    setTestResult(null);
    setDiscoveryStatus('loading');
    setDiscoveryError(null);
    setDiscoveredResources({});

    try {
      // Call the discover endpoint which validates connection AND returns resources
      // Include datasource_slug for OAuth-based auth (backend needs it to look up tokens)
      const response = await apiClient.post('/api/v1/datasources/discover', {
        datasource_type: effectiveDatasourceType,
        connection_config: effectiveConnectionConfig,
        credentials: credentialsForm,
        datasource_slug: datasource?.slug || formData.slug,
      });

      const { success, resources, message } = response.data;

      if (success) {
        trackEvent('ds_discover_success', { props: { datasource_type: effectiveDatasourceType } });
        setTestResult({ success: true, message: 'Connected successfully' });
        setDiscoveredResources(resources || {});
        setDiscoveryStatus('success');
        toast.success(message || 'Connection successful and resources discovered');
      } else {
        trackEvent('ds_discover_error', { props: { datasource_type: effectiveDatasourceType, error: message } });
        setTestResult({ success: false, message: message || 'Discovery failed' });
        setDiscoveryStatus('error');
        setDiscoveryError(message || 'Failed to discover resources');
        toast.error(message || 'Discovery failed');
      }
    } catch (error) {
      const message = error.response?.data?.detail || error.response?.data?.message || 'Connection test failed';
      trackEvent('ds_discover_error', { props: { datasource_type: effectiveDatasourceType, error: message } });
      setTestResult({ success: false, message });
      setDiscoveryStatus('error');
      setDiscoveryError(message);
      toast.error(message);

      // If OAuth token refresh failed, update OAuth status to reflect disconnected state
      if (message.includes('OAuth token') && message.includes('failed')) {
        const dsType = datasource?.datasource_type || formData.datasource_type;
        if (dsType === 'snowflake') {
          setSnowflakeOAuthStatus({ connected: false, email: null, connecting: false, disconnecting: false });
        } else if (dsType === 'bigquery') {
          setBigqueryEnterpriseOAuthStatus((prev) => ({ ...prev, connected: false, email: null }));
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
  // CATALOG CONFIG CHANGE (for CatalogSection in edit mode)
  // ==========================================================================

  const handleCatalogConfigChange = useCallback(
    async (configKey, value) => {
      if (!datasource) {
        return;
      }
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
  // RENDER: CONNECTION SETTINGS
  // ==========================================================================

  const renderConnectionSettings = () => {
    const type = datasource?.datasource_type || formData.datasource_type;
    const config = getConnectionConfig();
    const readOnly = !canAdmin;

    return (
      <ConnectionFormRenderer
        datasourceType={type}
        config={readOnly ? config : formData.connection_config}
        onChange={handleConnectionConfigChange}
        readOnly={readOnly}
        showRequired={canAdmin}
        discoveredResources={discoveredResources}
        discoveryStatus={discoveryStatus}
        isCreateMode={isCreateMode}
      />
    );
  };

  // ==========================================================================
  // RENDER: AUTH MODE SELECTOR (BigQuery / Snowflake)
  // ==========================================================================

  const renderAuthModeSelector = () => {
    const type = datasource?.datasource_type || formData.datasource_type;

    // BigQuery auth mode selector
    if (type === 'bigquery') {
      return (
        <div className="space-y-2 pb-4 border-b border-border">
          <label className="block text-sm font-medium">Authentication Mode</label>
          {canAdmin ? (
            <>
              <Select
                value={bigqueryAuthMode}
                onValueChange={handleBigQueryAuthModeChange}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="kyomi_oauth">Kyomi OAuth (Recommended)</SelectItem>
                  <SelectItem value="enterprise_oauth">Enterprise OAuth</SelectItem>
                  <SelectItem value="service_account">Service Account</SelectItem>
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                {bigqueryAuthMode === 'kyomi_oauth' && 'Users authenticate with their Google accounts via Kyomi.'}
                {bigqueryAuthMode === 'enterprise_oauth' && 'Users authenticate with your organization\'s OAuth app.'}
                {bigqueryAuthMode === 'service_account' && 'All users share a service account for automated access.'}
              </p>
            </>
          ) : (
            <p className="text-sm text-muted-foreground">
              {bigqueryAuthMode === 'kyomi_oauth' ? 'Kyomi OAuth' :
               bigqueryAuthMode === 'enterprise_oauth' ? 'Enterprise OAuth' :
               'Service Account'}
            </p>
          )}
        </div>
      );
    }

    // Snowflake auth mode selector
    if (type === 'snowflake') {
      return (
        <div className="space-y-2 pb-4 border-b border-border">
          <label className="block text-sm font-medium">Authentication Mode</label>
          {canAdmin ? (
            <>
              <Select
                value={snowflakeAuthMethod}
                onValueChange={(value) => {
                  setSnowflakeAuthMethod(value);
                  handleConnectionConfigChange('auth_mode', value);
                  if (value !== 'oauth' && snowflakeOAuthStatus.connected) {
                    setSnowflakeOAuthStatus({ connected: false, email: null, connecting: false, disconnecting: false });
                  }
                  // Clear credentials when switching methods
                  if (value === 'oauth') {
                    setCredentialsForm((prev) => ({
                      ...prev,
                      password: '',
                      private_key: '',
                      private_key_passphrase: '',
                    }));
                  } else if (value === 'password') {
                    setCredentialsForm((prev) => ({
                      ...prev,
                      private_key: '',
                      private_key_passphrase: '',
                    }));
                  } else if (value === 'keypair') {
                    setCredentialsForm((prev) => ({ ...prev, password: '' }));
                  }
                }}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="password">Password</SelectItem>
                  <SelectItem value="oauth">OAuth</SelectItem>
                  <SelectItem value="keypair">Key-Pair</SelectItem>
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                {snowflakeAuthMethod === 'oauth' && 'Users authenticate with their Snowflake accounts via OAuth.'}
                {snowflakeAuthMethod === 'password' && 'Users authenticate with username and password.'}
                {snowflakeAuthMethod === 'keypair' && 'Users authenticate using RSA key-pair.'}
              </p>
            </>
          ) : (
            <p className="text-sm text-muted-foreground">
              {snowflakeAuthMethod === 'oauth' ? 'OAuth' :
               snowflakeAuthMethod === 'password' ? 'Password' :
               'Key-Pair'}
            </p>
          )}
        </div>
      );
    }

    return null;
  };

  // ==========================================================================
  // RENDER: CREDENTIALS SECTION
  // ==========================================================================

  const renderCredentialsSection = () => {
    const type = formData.datasource_type;
    const sharedCreds = isUsingSharedCredentials();

    // BigQuery - supports multiple auth modes
    if (type === 'bigquery') {
      const enterpriseOAuthConfigured = isBigQueryEnterpriseOAuthConfigured();

      return (
        <div className="space-y-4 border-t border-border pt-4 mt-4">
          <h4 className="text-sm font-medium">BigQuery Credentials</h4>

          {/* ============== KYOMI OAUTH MODE ============== */}
          {bigqueryAuthMode === 'kyomi_oauth' && (
            <div className="space-y-4">
              {/* Show current Google account status */}
              {oauthStatus.hasOauth && (
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
                    disabled={disconnecting}
                  >
                    {disconnecting ? (
                      <>
                        <Spinner size="sm" />
                        Disconnecting...
                      </>
                    ) : (
                      'Disconnect'
                    )}
                  </Button>
                </div>
              )}

              {/* Show Connect BigQuery button if needed */}
              {oauthStatus.needsBigqueryConnect ? (
                <div className="space-y-3">
                  <Button variant="outline" onClick={handleConnectBigQuery} disabled={oauthConnecting}>
                    {oauthConnecting ? (
                      <>
                        <Spinner size="sm" />
                        Connecting...
                      </>
                    ) : (
                      <>
                        <DatasourceIcon type="bigquery" className="h-4 w-4" />
                        Connect BigQuery
                      </>
                    )}
                  </Button>
                  <p className="text-xs text-muted-foreground">
                    Sign in with Google to access your BigQuery projects.
                  </p>
                </div>
              ) : (
                /* Project dropdowns when connected */
                <div className="grid grid-cols-2 gap-4">
                  <div>
                    <label className="block text-sm font-medium mb-1">Billing Project</label>
                    <Select
                      value={credentialsForm.billing_project || ''}
                      onValueChange={(value) =>
                        setCredentialsForm((prev) => ({ ...prev, billing_project: value }))
                      }
                    >
                      <SelectTrigger>
                        <SelectValue placeholder="Select..." />
                      </SelectTrigger>
                      <SelectContent>
                        {googleProjects.map((p) => (
                          <SelectItem key={p.project_id} value={p.project_id}>
                            {p.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <div>
                    <label className="block text-sm font-medium mb-1">Default Project</label>
                    <Select
                      value={credentialsForm.default_project || ''}
                      onValueChange={(value) =>
                        setCredentialsForm((prev) => ({ ...prev, default_project: value }))
                      }
                    >
                      <SelectTrigger>
                        <SelectValue placeholder="Select..." />
                      </SelectTrigger>
                      <SelectContent>
                        {googleProjects.map((p) => (
                          <SelectItem key={p.project_id} value={p.project_id}>
                            {p.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                </div>
              )}
            </div>
          )}

          {/* ============== ENTERPRISE OAUTH MODE ============== */}
          {bigqueryAuthMode === 'enterprise_oauth' && (
            <div className="space-y-4">
              {/* Admin OAuth Configuration Section */}
              {canAdmin && (
                <div className="space-y-3 pb-4 border-b border-border">
                  <h4 className="text-sm font-medium">OAuth Configuration</h4>
                  <p className="text-xs text-muted-foreground">
                    Configure your organization's Google Cloud OAuth app for BigQuery access.
                  </p>

                  {/* Redirect URL for Google Cloud setup */}
                  <div className="p-3 bg-muted/30 rounded-lg space-y-1">
                    <label className="block text-xs font-medium text-muted-foreground">
                      Redirect URL (use this when creating your Google Cloud OAuth client)
                    </label>
                    <div className="flex items-center gap-2">
                      <code className="flex-1 px-2 py-1 bg-background border border-input rounded text-xs font-mono break-all">
                        {window.location.origin}/auth/oauth/bigquery-enterprise/callback
                      </code>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        onClick={() => {
                          navigator.clipboard.writeText(`${window.location.origin}/auth/oauth/bigquery-enterprise/callback`);
                          toast.success('Redirect URL copied');
                        }}
                      >
                        Copy
                      </Button>
                    </div>
                  </div>

                  <div className="grid grid-cols-2 gap-4">
                    <div>
                      <label className="block text-sm font-medium mb-1">OAuth Client ID</label>
                      <input
                        type="text"
                        value={formData.connection_config.oauth_client_id || ''}
                        onChange={(e) => handleConnectionConfigChange('oauth_client_id', e.target.value)}
                        placeholder="From Google Cloud Console"
                        className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                      />
                    </div>
                    <div>
                      <label className="block text-sm font-medium mb-1">OAuth Client Secret</label>
                      <input
                        type="password"
                        value={formData.connection_config.oauth_client_secret || ''}
                        onChange={(e) => handleConnectionConfigChange('oauth_client_secret', e.target.value)}
                        placeholder="OAuth client secret"
                        className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                      />
                    </div>
                  </div>
                </div>
              )}

              {/* User OAuth Connection */}
              <div className="space-y-3">
                <h4 className="text-sm font-medium">Your Connection</h4>

                {/* Connected Status */}
                {bigqueryEnterpriseOAuthStatus.connected ? (
                  <div className="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
                    <div className="flex items-center gap-2">
                      <Check className="h-4 w-4 text-success-foreground" />
                      <span className="text-sm text-foreground">
                        {bigqueryEnterpriseOAuthStatus.email
                          ? `Connected: ${bigqueryEnterpriseOAuthStatus.email}`
                          : 'Connected to BigQuery'}
                      </span>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={handleDisconnectBigQueryEnterprise}
                      disabled={bigqueryEnterpriseOAuthStatus.disconnecting}
                    >
                      {bigqueryEnterpriseOAuthStatus.disconnecting ? (
                        <>
                          <Spinner size="sm" className="mr-2" />
                          Disconnecting...
                        </>
                      ) : (
                        'Disconnect'
                      )}
                    </Button>
                  </div>
                ) : (
                  <>
                    {enterpriseOAuthConfigured ? (
                      <div className="space-y-2">
                        <Button
                          variant="outline"
                          onClick={handleConnectBigQueryEnterprise}
                          disabled={bigqueryEnterpriseOAuthStatus.connecting || (!datasource?.slug && !formData.slug)}
                        >
                          {bigqueryEnterpriseOAuthStatus.connecting ? (
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
                          Sign in with your organization's Google account.
                        </p>
                      </div>
                    ) : (
                      <Alert variant="warning">
                        <AlertCircle className="h-4 w-4" />
                        <AlertDescription>
                          OAuth credentials not configured. Ask your admin to configure OAuth Client ID and Secret.
                        </AlertDescription>
                      </Alert>
                    )}
                  </>
                )}

                {/* Project dropdowns when connected */}
                {bigqueryEnterpriseOAuthStatus.connected && (
                  <div className="grid grid-cols-2 gap-4 mt-4">
                    <div>
                      <label className="block text-sm font-medium mb-1">Billing Project</label>
                      <Select
                        value={credentialsForm.billing_project || ''}
                        onValueChange={(value) =>
                          setCredentialsForm((prev) => ({ ...prev, billing_project: value }))
                        }
                      >
                        <SelectTrigger>
                          <SelectValue placeholder="Select..." />
                        </SelectTrigger>
                        <SelectContent>
                          {googleProjects.map((p) => (
                            <SelectItem key={p.project_id} value={p.project_id}>
                              {p.name}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                    <div>
                      <label className="block text-sm font-medium mb-1">Default Project</label>
                      <Select
                        value={credentialsForm.default_project || ''}
                        onValueChange={(value) =>
                          setCredentialsForm((prev) => ({ ...prev, default_project: value }))
                        }
                      >
                        <SelectTrigger>
                          <SelectValue placeholder="Select..." />
                        </SelectTrigger>
                        <SelectContent>
                          {googleProjects.map((p) => (
                            <SelectItem key={p.project_id} value={p.project_id}>
                              {p.name}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                  </div>
                )}
              </div>
            </div>
          )}

          {/* ============== SERVICE ACCOUNT MODE ============== */}
          {bigqueryAuthMode === 'service_account' && (
            <div className="space-y-4">
              {canAdmin ? (
                <>
                  <p className="text-xs text-muted-foreground">
                    Upload or paste your Google Cloud service account credentials JSON file.
                    This will be used for all users accessing this datasource.
                  </p>

                  {/* Show current service account if configured */}
                  {serviceAccountEmail && (
                    <div className="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
                      <div className="flex items-center gap-2">
                        <Check className="h-4 w-4 text-success-foreground" />
                        <span className="text-sm text-foreground">
                          Service Account: {serviceAccountEmail}
                        </span>
                      </div>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => {
                          setServiceAccountFile(null);
                          setServiceAccountJson('');
                          setServiceAccountEmail('');
                          handleConnectionConfigChange('service_account_json', '');
                        }}
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
                          ref={serviceAccountInputRef}
                          accept=".json"
                          onChange={handleServiceAccountFileUpload}
                          className="hidden"
                        />
                        <Button
                          variant="outline"
                          onClick={() => serviceAccountInputRef.current?.click()}
                        >
                          <Upload className="h-4 w-4 mr-2" />
                          Upload credentials.json
                        </Button>
                        {serviceAccountFile && (
                          <span className="ml-2 text-sm text-muted-foreground">
                            {serviceAccountFile.name}
                          </span>
                        )}
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
                </>
              ) : (
                /* Non-admin view */
                <div className="flex items-center gap-2 p-3 bg-muted/50 rounded-lg">
                  <Lock className="h-4 w-4 text-muted-foreground" />
                  <span className="text-sm text-muted-foreground">
                    {serviceAccountEmail
                      ? `Using service account: ${serviceAccountEmail}`
                      : 'Using service account configured by admin'}
                  </span>
                </div>
              )}

              {/* Validate & Discover button + Project selection for service account */}
              {serviceAccountEmail && (
                <div className="space-y-4">
                  {/* Validate & Discover button */}
                  <div className="flex items-center gap-3">
                    <Button
                      variant="outline"
                      onClick={() => {
                        // Call testAndDiscover to validate service account and discover projects
                        testAndDiscover();
                      }}
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
                        className={`flex items-center gap-2 text-sm ${testResult.success ? 'text-success-foreground' : 'text-error-foreground'}`}
                      >
                        {testResult.success ? <Check className="h-4 w-4" /> : <X className="h-4 w-4" />}
                        <span>{testResult.success ? 'Valid' : 'Failed'}</span>
                      </div>
                    )}
                  </div>

                  {/* Project dropdowns (when discovered) or text inputs (fallback) */}
                  <div className="grid grid-cols-2 gap-4">
                    <div>
                      <label className="block text-sm font-medium mb-1">Billing Project</label>
                      {discoveredResources.projects?.length > 0 ? (
                        <Select
                          value={credentialsForm.billing_project || ''}
                          onValueChange={(value) =>
                            setCredentialsForm((prev) => ({ ...prev, billing_project: value }))
                          }
                        >
                          <SelectTrigger>
                            <SelectValue placeholder="Select billing project..." />
                          </SelectTrigger>
                          <SelectContent>
                            {discoveredResources.projects.map((p) => (
                              <SelectItem key={p} value={p}>
                                {p}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      ) : (
                        <input
                          type="text"
                          value={credentialsForm.billing_project || ''}
                          onChange={(e) => setCredentialsForm(prev => ({ ...prev, billing_project: e.target.value }))}
                          placeholder="my-gcp-project"
                          className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                        />
                      )}
                    </div>
                    <div>
                      <label className="block text-sm font-medium mb-1">Default Project</label>
                      {discoveredResources.projects?.length > 0 ? (
                        <Select
                          value={credentialsForm.default_project || ''}
                          onValueChange={(value) =>
                            setCredentialsForm((prev) => ({ ...prev, default_project: value }))
                          }
                        >
                          <SelectTrigger>
                            <SelectValue placeholder="Select default project..." />
                          </SelectTrigger>
                          <SelectContent>
                            {discoveredResources.projects.map((p) => (
                              <SelectItem key={p} value={p}>
                                {p}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      ) : (
                        <input
                          type="text"
                          value={credentialsForm.default_project || ''}
                          onChange={(e) => setCredentialsForm(prev => ({ ...prev, default_project: e.target.value }))}
                          placeholder="my-gcp-project"
                          className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                        />
                      )}
                      <p className="text-xs text-muted-foreground mt-1">
                        {discoveredResources.projects?.length > 0
                          ? 'Select from discovered projects'
                          : 'Click "Validate & Discover" to load available projects'}
                      </p>
                    </div>
                  </div>
                </div>
              )}
            </div>
          )}

        </div>
      );
    }

    // Snowflake: OAuth, password, or key-pair (structured like BigQuery)
    if (type === 'snowflake') {
      const oauthConfigured = isSnowflakeOAuthConfigured();

      return (
        <div className="space-y-4 border-t border-border pt-4 mt-4">
          <h4 className="text-sm font-medium">Snowflake Credentials</h4>

          {/* ============== OAUTH MODE ============== */}
          {snowflakeAuthMethod === 'oauth' && (
            <div className="space-y-4">
              {/* Admin OAuth Configuration Section */}
              {canAdmin && (
                <div className="space-y-3 pb-4 border-b border-border">
                  <h4 className="text-sm font-medium">OAuth Configuration</h4>
                  <p className="text-xs text-muted-foreground">
                    Configure your Snowflake OAuth security integration for user authentication.
                  </p>

                  {/* Redirect URL for Snowflake setup */}
                  <div className="p-3 bg-muted/30 rounded-lg space-y-1">
                    <label className="block text-xs font-medium text-muted-foreground">
                      Redirect URL (use this when creating your Snowflake OAuth integration)
                    </label>
                    <div className="flex items-center gap-2">
                      <code className="flex-1 px-2 py-1 bg-background border border-input rounded text-xs font-mono break-all">
                        {window.location.origin}/auth/oauth/snowflake/callback
                      </code>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        onClick={() => {
                          navigator.clipboard.writeText(`${window.location.origin}/auth/oauth/snowflake/callback`);
                          toast.success('Redirect URL copied');
                        }}
                      >
                        Copy
                      </Button>
                    </div>
                  </div>

                  <div className="grid grid-cols-2 gap-4">
                    <div>
                      <label className="block text-sm font-medium mb-1">OAuth Client ID</label>
                      <input
                        type="text"
                        value={formData.connection_config.oauth_client_id || ''}
                        onChange={(e) => handleConnectionConfigChange('oauth_client_id', e.target.value)}
                        placeholder="From Snowflake OAuth integration"
                        className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                      />
                    </div>
                    <div>
                      <label className="block text-sm font-medium mb-1">OAuth Client Secret</label>
                      <input
                        type="password"
                        value={formData.connection_config.oauth_client_secret || ''}
                        onChange={(e) => handleConnectionConfigChange('oauth_client_secret', e.target.value)}
                        placeholder="OAuth client secret"
                        className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                      />
                    </div>
                  </div>
                </div>
              )}

              {/* User OAuth Connection */}
              <div className="space-y-3">
                <h4 className="text-sm font-medium">Your Connection</h4>

                {/* Connected Status */}
                {snowflakeOAuthStatus.connected ? (
                  <div className="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
                    <div className="flex items-center gap-2">
                      <Check className="h-4 w-4 text-success-foreground" />
                      <span className="text-sm text-foreground">
                        {snowflakeOAuthStatus.email
                          ? `Connected: ${snowflakeOAuthStatus.email}`
                          : 'Connected to Snowflake'}
                      </span>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={handleDisconnectSnowflake}
                      disabled={snowflakeOAuthStatus.disconnecting}
                    >
                      {snowflakeOAuthStatus.disconnecting ? (
                        <>
                          <Spinner size="sm" className="mr-2" />
                          Disconnecting...
                        </>
                      ) : (
                        'Disconnect'
                      )}
                    </Button>
                  </div>
                ) : (
                  <>
                    {oauthConfigured ? (
                      <div className="space-y-2">
                        <Button
                          variant="outline"
                          onClick={handleConnectSnowflake}
                          disabled={snowflakeOAuthStatus.connecting || (!datasource?.slug && !formData.slug)}
                        >
                          {snowflakeOAuthStatus.connecting ? (
                            <>
                              <Spinner size="sm" className="mr-2" />
                              Connecting...
                            </>
                          ) : (
                            <>
                              <DatasourceIcon type="snowflake" className="h-4 w-4 mr-2" />
                              Connect Snowflake
                            </>
                          )}
                        </Button>
                        <p className="text-xs text-muted-foreground">
                          Sign in with your Snowflake account using OAuth.
                        </p>
                      </div>
                    ) : (
                      <Alert variant="warning">
                        <AlertCircle className="h-4 w-4" />
                        <AlertDescription>
                          OAuth credentials not configured. Ask your admin to configure OAuth Client ID and Secret.
                        </AlertDescription>
                      </Alert>
                    )}
                  </>
                )}
              </div>
            </div>
          )}

          {/* ============== PASSWORD MODE ============== */}
          {snowflakeAuthMethod === 'password' && (
            <div className="space-y-4">
              {/* Shared credentials toggle - admin only */}
              {canAdmin && (
                <label className="flex items-center gap-3 p-3 bg-muted/30 rounded-lg cursor-pointer">
                  <input
                    type="checkbox"
                    checked={formData.connection_config.shared_credentials || false}
                    onChange={(e) =>
                      setFormData((prev) => ({
                        ...prev,
                        connection_config: {
                          ...prev.connection_config,
                          shared_credentials: e.target.checked,
                          ...(e.target.checked ? {} : { shared_username: '', shared_password: '' }),
                        },
                      }))
                    }
                    className="h-4 w-4 rounded border-input"
                  />
                  <div>
                    <p className="text-sm font-medium">All users share these credentials</p>
                    <p className="text-xs text-muted-foreground">
                      Use a service account instead of individual user credentials
                    </p>
                  </div>
                </label>
              )}

              {/* Credential inputs based on shared vs individual */}
              {sharedCreds && !canAdmin ? (
                /* Non-admin viewing shared credentials */
                <div className="flex items-center gap-2 p-3 bg-muted/50 rounded-lg">
                  <Lock className="h-4 w-4 text-muted-foreground" />
                  <span className="text-sm text-muted-foreground">
                    Using shared credentials configured by admin
                  </span>
                </div>
              ) : canAdmin && formData.connection_config.shared_credentials ? (
                /* Admin configuring shared credentials */
                <div className="space-y-3">
                  <div>
                    <label className="block text-sm font-medium mb-1">
                      Shared Username <span className="text-error-foreground">*</span>
                    </label>
                    <input
                      type="text"
                      value={formData.connection_config.shared_username || ''}
                      onChange={(e) =>
                        setFormData((prev) => ({
                          ...prev,
                          connection_config: { ...prev.connection_config, shared_username: e.target.value },
                        }))
                      }
                      placeholder="svc_account"
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                    />
                  </div>
                  <div>
                    <label className="block text-sm font-medium mb-1">
                      Shared Password <span className="text-error-foreground">*</span>
                    </label>
                    <input
                      type="password"
                      value={formData.connection_config.shared_password || ''}
                      onChange={(e) =>
                        setFormData((prev) => ({
                          ...prev,
                          connection_config: { ...prev.connection_config, shared_password: e.target.value },
                        }))
                      }
                      placeholder="••••••••"
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                    />
                    <p className="text-xs text-muted-foreground mt-1">Credentials are encrypted at rest</p>
                  </div>
                </div>
              ) : (
                /* Individual user credentials */
                <div className="space-y-3">
                  <div>
                    <label className="block text-sm font-medium mb-1">
                      Username <span className="text-error-foreground">*</span>
                    </label>
                    <input
                      type="text"
                      value={credentialsForm.username || ''}
                      onChange={(e) =>
                        setCredentialsForm((prev) => ({ ...prev, username: e.target.value }))
                      }
                      placeholder="Snowflake username"
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                    />
                  </div>
                  <div>
                    <label className="block text-sm font-medium mb-1">
                      Password <span className="text-error-foreground">*</span>
                    </label>
                    <input
                      type="password"
                      value={credentialsForm.password || ''}
                      onChange={(e) =>
                        setCredentialsForm((prev) => ({ ...prev, password: e.target.value }))
                      }
                      placeholder="••••••••"
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                    />
                    <p className="text-xs text-muted-foreground mt-1">Credentials are encrypted at rest</p>
                  </div>
                </div>
              )}
            </div>
          )}

          {/* ============== KEY-PAIR MODE ============== */}
          {snowflakeAuthMethod === 'keypair' && (
            <div className="space-y-4">
              {/* Shared credentials toggle - admin only */}
              {canAdmin && (
                <label className="flex items-center gap-3 p-3 bg-muted/30 rounded-lg cursor-pointer">
                  <input
                    type="checkbox"
                    checked={formData.connection_config.shared_credentials || false}
                    onChange={(e) =>
                      setFormData((prev) => ({
                        ...prev,
                        connection_config: {
                          ...prev.connection_config,
                          shared_credentials: e.target.checked,
                          ...(e.target.checked ? {} : { shared_username: '', shared_private_key: '' }),
                        },
                      }))
                    }
                    className="h-4 w-4 rounded border-input"
                  />
                  <div>
                    <p className="text-sm font-medium">All users share these credentials</p>
                    <p className="text-xs text-muted-foreground">
                      Use a service account instead of individual user credentials
                    </p>
                  </div>
                </label>
              )}

              {/* Credential inputs based on shared vs individual */}
              {sharedCreds && !canAdmin ? (
                /* Non-admin viewing shared credentials */
                <div className="flex items-center gap-2 p-3 bg-muted/50 rounded-lg">
                  <Lock className="h-4 w-4 text-muted-foreground" />
                  <span className="text-sm text-muted-foreground">
                    Using shared credentials configured by admin
                  </span>
                </div>
              ) : canAdmin && formData.connection_config.shared_credentials ? (
                /* Admin configuring shared credentials */
                <div className="space-y-3">
                  <div>
                    <label className="block text-sm font-medium mb-1">
                      Shared Username <span className="text-error-foreground">*</span>
                    </label>
                    <input
                      type="text"
                      value={formData.connection_config.shared_username || ''}
                      onChange={(e) =>
                        setFormData((prev) => ({
                          ...prev,
                          connection_config: { ...prev.connection_config, shared_username: e.target.value },
                        }))
                      }
                      placeholder="svc_account"
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                    />
                  </div>
                  <div>
                    <label className="block text-sm font-medium mb-1">
                      Shared Private Key (PEM format) <span className="text-error-foreground">*</span>
                    </label>
                    <textarea
                      value={formData.connection_config.shared_private_key || ''}
                      onChange={(e) =>
                        setFormData((prev) => ({
                          ...prev,
                          connection_config: { ...prev.connection_config, shared_private_key: e.target.value },
                        }))
                      }
                      placeholder="-----BEGIN PRIVATE KEY-----&#10;..."
                      rows={6}
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono"
                    />
                  </div>
                  <div>
                    <label className="block text-sm font-medium mb-1">
                      Private Key Passphrase (optional)
                    </label>
                    <input
                      type="password"
                      value={formData.connection_config.shared_private_key_passphrase || ''}
                      onChange={(e) =>
                        setFormData((prev) => ({
                          ...prev,
                          connection_config: { ...prev.connection_config, shared_private_key_passphrase: e.target.value },
                        }))
                      }
                      placeholder="••••••••"
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                    />
                  </div>
                </div>
              ) : (
                /* Individual user credentials */
                <div className="space-y-3">
                  <div>
                    <label className="block text-sm font-medium mb-1">
                      Username <span className="text-error-foreground">*</span>
                    </label>
                    <input
                      type="text"
                      value={credentialsForm.username || ''}
                      onChange={(e) =>
                        setCredentialsForm((prev) => ({ ...prev, username: e.target.value }))
                      }
                      placeholder="Snowflake username"
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                    />
                  </div>
                  <div>
                    <label className="block text-sm font-medium mb-1">
                      Private Key (PEM format) <span className="text-error-foreground">*</span>
                    </label>
                    <textarea
                      value={credentialsForm.private_key || ''}
                      onChange={(e) =>
                        setCredentialsForm((prev) => ({ ...prev, private_key: e.target.value }))
                      }
                      placeholder="-----BEGIN PRIVATE KEY-----&#10;MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcw...&#10;-----END PRIVATE KEY-----"
                      rows={6}
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono"
                    />
                    <p className="text-xs text-muted-foreground mt-1">
                      Paste your private key in PEM format
                    </p>
                  </div>
                  <div>
                    <label className="block text-sm font-medium mb-1">
                      Private Key Passphrase (optional)
                    </label>
                    <input
                      type="password"
                      value={credentialsForm.private_key_passphrase || ''}
                      onChange={(e) =>
                        setCredentialsForm((prev) => ({
                          ...prev,
                          private_key_passphrase: e.target.value,
                        }))
                      }
                      placeholder="••••••••"
                      className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                    />
                    <p className="text-xs text-muted-foreground mt-1">
                      Only required if your private key is encrypted
                    </p>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      );
    }

    // Other types (postgres, clickhouse, mysql, etc.)
    return (
      <div className="space-y-4 border-t border-border pt-4 mt-4">
        <div className="flex items-center justify-between">
          <h4 className="text-sm font-medium">Credentials</h4>
        </div>

        {/* Shared credentials toggle - admin only */}
        {canAdmin && (
          <label className="flex items-center gap-3 p-3 bg-muted/30 rounded-lg cursor-pointer">
            <input
              type="checkbox"
              checked={formData.connection_config.shared_credentials || false}
              onChange={(e) =>
                setFormData((prev) => ({
                  ...prev,
                  connection_config: {
                    ...prev.connection_config,
                    shared_credentials: e.target.checked,
                    ...(e.target.checked ? {} : { shared_username: '', shared_password: '' }),
                  },
                }))
              }
              className="h-4 w-4 rounded border-input"
            />
            <div>
              <p className="text-sm font-medium">All users share these credentials</p>
              <p className="text-xs text-muted-foreground">
                Use a service account instead of individual user credentials
              </p>
            </div>
          </label>
        )}

        {/* Show appropriate credential inputs */}
        {sharedCreds && !canAdmin ? (
          <div className="flex items-center gap-2 p-3 bg-muted/50 rounded-lg">
            <Lock className="h-4 w-4 text-muted-foreground" />
            <span className="text-sm text-muted-foreground">
              Using shared credentials configured by admin
            </span>
          </div>
        ) : canAdmin && formData.connection_config.shared_credentials ? (
          <div className="space-y-3">
            <div>
              <label className="block text-sm font-medium mb-1">
                Shared Username <span className="text-error-foreground">*</span>
              </label>
              <input
                type="text"
                value={formData.connection_config.shared_username || ''}
                onChange={(e) =>
                  setFormData((prev) => ({
                    ...prev,
                    connection_config: { ...prev.connection_config, shared_username: e.target.value },
                  }))
                }
                placeholder="svc_account"
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
              />
            </div>
            <div>
              <label className="block text-sm font-medium mb-1">
                Shared Password <span className="text-error-foreground">*</span>
              </label>
              <input
                type="password"
                value={formData.connection_config.shared_password || ''}
                onChange={(e) =>
                  setFormData((prev) => ({
                    ...prev,
                    connection_config: { ...prev.connection_config, shared_password: e.target.value },
                  }))
                }
                placeholder="••••••••"
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
              />
              <p className="text-xs text-muted-foreground mt-1">Credentials are encrypted at rest</p>
            </div>
          </div>
        ) : (
          <div className="space-y-3">
            <div>
              <label className="block text-sm font-medium mb-1">
                Username <span className="text-error-foreground">*</span>
              </label>
              <input
                type="text"
                value={credentialsForm.username || ''}
                onChange={(e) =>
                  setCredentialsForm((prev) => ({ ...prev, username: e.target.value }))
                }
                placeholder="Database username"
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
              />
            </div>
            <div>
              <label className="block text-sm font-medium mb-1">
                Password <span className="text-error-foreground">*</span>
              </label>
              <input
                type="password"
                value={credentialsForm.password || ''}
                onChange={(e) =>
                  setCredentialsForm((prev) => ({ ...prev, password: e.target.value }))
                }
                placeholder="••••••••"
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
              />
              <p className="text-xs text-muted-foreground mt-1">Credentials are encrypted at rest</p>
            </div>
          </div>
        )}
      </div>
    );
  };

  // ==========================================================================
  // RENDER: CONNECTION TAB
  // ==========================================================================

  const renderConnectionTab = () => {
    // Show loading state while settings are being fetched (edit mode only)
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
        {/* Name and Slug (admin only) */}
        {canAdmin && (
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium mb-1">
                Name {isCreateMode && <span className="text-error-foreground">*</span>}
              </label>
              <input
                type="text"
                value={formData.name}
                onChange={(e) => handleNameChange(e.target.value)}
                placeholder={`Production ${datasourceTypes[formData.datasource_type]?.label || 'Database'}`}
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
              />
            </div>
            <div>
              <label className="block text-sm font-medium mb-1">Slug</label>
              <input
                type="text"
                value={formData.slug}
                onChange={(e) => handleSlugChange(e.target.value)}
                placeholder={`production-${formData.datasource_type}`}
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono"
              />
              <p className="text-xs text-muted-foreground mt-1">
                {isCreateMode
                  ? 'Auto-generated from name if left empty'
                  : 'Used in ChartML specs and API calls'}
              </p>
            </div>
          </div>
        )}

        {/* Auth Mode Selector (BigQuery/Snowflake) - shown at the top */}
        {renderAuthModeSelector()}

        {/* Connection Settings */}
        <div>
          <h4 className="text-sm font-medium mb-3">
            {canAdmin ? 'Connection Settings' : 'Connection'}
          </h4>
          {renderConnectionSettings()}
        </div>

        {/* Credentials section */}
        {renderCredentialsSection()}

        {/* Test Connection / Test & Discover - hide for OAuth modes (OAuth flow already validates connection) */}
        {!(
          (formData.datasource_type === 'snowflake' && snowflakeAuthMethod === 'oauth') ||
          formData.datasource_type === 'bigquery'
        ) && (
          <div className="border-t border-border pt-4 mt-4">
            <div className="flex items-center gap-3">
              <Button
                variant="outline"
                onClick={testAndDiscover}
                disabled={testing}
              >
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
            {/* Show discovery error if any */}
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
        )}

        {/* Discovery Fields Section - shown after credentials */}
        {/* In create mode: dropdowns after successful Test & Discover */}
        {/* In edit mode: text inputs with saved values */}
        {(isCreateMode ? discoveryStatus === 'success' : true) && renderDiscoveryFields()}
      </div>
    );
  };

  /**
   * Render discovery fields as dropdowns after successful discovery.
   * These appear AFTER the Test & Discover button, making the flow logical:
   * 1. Fill connection fields
   * 2. Fill credentials
   * 3. Click Test & Discover
   * 4. Select from discovered resources (this section)
   */
  const renderDiscoveryFields = () => {
    const schema = getConnectionSchema(formData.datasource_type);
    const discoveryFields = schema?.discoveryFields || [];

    if (discoveryFields.length === 0) {
      return null;
    }

    // Group fields into rows for 2-column grid
    const groupFieldsIntoRows = (fields) => {
      const rows = [];
      let currentRow = [];
      let currentColumn = 0;

      for (const field of fields) {
        const gridColumn = field.gridColumn || 1;

        if (gridColumn === 'full') {
          if (currentRow.length > 0) {
            rows.push(currentRow);
          }
          rows.push([{ ...field, span: 2 }]);
          currentRow = [];
          currentColumn = 0;
        } else if (gridColumn === 1) {
          if (currentColumn !== 0) {
            rows.push(currentRow);
            currentRow = [];
          }
          currentRow.push(field);
          currentColumn = 1;
        } else {
          if (currentColumn === 0) {
            currentRow.push(null);
          }
          currentRow.push(field);
          rows.push(currentRow);
          currentRow = [];
          currentColumn = 0;
        }
      }

      if (currentRow.length > 0) {
        rows.push(currentRow);
      }

      return rows;
    };

    const rows = groupFieldsIntoRows(discoveryFields);

    // Show dropdowns if discovery succeeded (in both create and edit modes)
    // Otherwise in edit mode, show text inputs with saved values
    const showAsDropdowns = discoveryStatus === 'success';

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
        <div className="space-y-4">
          {rows.map((row, rowIndex) => (
            <div key={rowIndex} className="grid grid-cols-2 gap-4">
              {row.map((field, colIndex) => {
                if (!field) {
                  return <div key={`empty-${colIndex}`} />;
                }

                const currentValue = formData.connection_config[field.name] || '';

                // Show text inputs if discovery hasn't succeeded yet (edit mode initial state)
                if (!showAsDropdowns) {
                  return (
                    <div
                      key={field.name}
                      className={field.span === 2 ? 'col-span-2' : ''}
                    >
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

                // Create mode: render as dropdown if DISCOVERY type, text input if TEXT type
                const isDiscoveryField = !!field.discoveryKey;
                const options = isDiscoveryField ? (discoveredResources[field.discoveryKey] || []) : [];

                // TEXT type fields should render as text inputs even in create mode
                if (!isDiscoveryField) {
                  return (
                    <div
                      key={field.name}
                      className={field.span === 2 ? 'col-span-2' : ''}
                    >
                      <label className="block text-sm font-medium mb-1">
                        {field.label}
                        {!field.optional && <span className="text-error-foreground"> *</span>}
                      </label>
                      <input
                        type="text"
                        value={currentValue}
                        onChange={(e) => handleConnectionConfigChange(field.name, e.target.value)}
                        placeholder={field.placeholder || ''}
                        className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
                      />
                      {field.helpText && (
                        <p className="text-xs text-muted-foreground mt-1">{field.helpText}</p>
                      )}
                    </div>
                  );
                }

                // DISCOVERY type fields render as dropdowns
                return (
                  <div
                    key={field.name}
                    className={field.span === 2 ? 'col-span-2' : ''}
                  >
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
          ))}
        </div>
      </div>
    );
  };

  // ==========================================================================
  // RENDER: CATALOG TAB (CREATE MODE)
  // ==========================================================================

  const renderCatalogTabCreateMode = () => {
    const itemTypeLabel =
      formData.datasource_type === 'bigquery'
        ? 'Projects'
        : formData.datasource_type === 'postgres'
          ? 'Schemas'
          : 'Databases';

    return (
      <div className="space-y-4">
        <div>
          <h4 className="text-sm font-medium mb-1">{itemTypeLabel} to Index</h4>
          <p className="text-sm text-muted-foreground mb-4">
            Select which{' '}
            {formData.datasource_type === 'bigquery'
              ? 'projects'
              : formData.datasource_type === 'postgres'
                ? 'schemas'
                : 'databases'}{' '}
            to include in the data catalog for AI discovery.
          </p>
        </div>

        {/* Loading state */}
        {catalogDiscovery.loading && (
          <div className="flex items-center gap-2 py-8 justify-center">
            <Spinner size="md" className="text-muted-foreground" />
            <span className="text-sm text-muted-foreground">
              Discovering available {catalogDiscovery.itemType}...
            </span>
          </div>
        )}

        {/* Error state */}
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

        {/* Items list */}
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
              {/* Action buttons */}
              <div className="flex items-center gap-2">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() =>
                    setSelectedCatalogItems(catalogDiscovery.items.map((i) => i.name))
                  }
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

              {/* Checkbox list */}
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

              {/* Help text */}
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
            <Button onClick={handleSave} disabled={saving}>
              {saving ? <Spinner size="sm" className="mr-2" /> : null}
              Create
            </Button>
          )}
        </>
      );
    }

    // Edit mode footer
    return (
      <>
        {/* Delete button - admin only in edit mode */}
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
        <Button onClick={handleSaveAndClose} disabled={saving}>
          {saving ? <Spinner size="sm" className="mr-2" /> : null}
          Save
        </Button>
      </>
    );
  };

  // ==========================================================================
  // RENDER: MAIN MODAL
  // ==========================================================================

  return (
    <>
      <Modal
        show={isOpen}
        onClose={onClose}
        title={modalTitle}
        size="lg"
        footer={renderFooter()}
      >
        {canAdmin && showCatalogTab ? (
          // Admin with catalog tab - show full tabs UI
          <div className="space-y-4">
            {/* Tabs: Connection and Catalog */}
            <div className="flex border-b border-border">
              {['connection', 'catalog'].map((tab) => (
                <button
                  key={tab}
                  onClick={() => setActiveTab(tab)}
                  disabled={isCreateMode && tab === 'catalog' && !testResult?.success}
                  className={`px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
                    activeTab === tab
                      ? 'border-primary text-primary'
                      : 'border-transparent text-muted-foreground hover:text-foreground'
                  } ${isCreateMode && tab === 'catalog' && !testResult?.success ? 'opacity-50 cursor-not-allowed' : ''}`}
                >
                  {tab.charAt(0).toUpperCase() + tab.slice(1)}
                </button>
              ))}
            </div>

            {/* Content area */}
            <div className="pt-2 min-h-[400px]">
              {activeTab === 'connection' && (
                <div className="space-y-4">
                  {/* Type selector - only in create mode */}
                  {isCreateMode && (
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
                          {Object.entries(datasourceTypes).map(([key, { label }]) => (
                            <SelectItem key={key} value={key}>
                              <div className="flex items-center gap-2">
                                <DatasourceIcon type={key} className="h-4 w-4" opacity={0.8} />
                                <span>{label}</span>
                              </div>
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                  )}

                  {/* Type badge in edit mode */}
                  {!isCreateMode && (
                    <div className="flex items-center gap-2">
                      <DatasourceIcon type={formData.datasource_type} className="h-5 w-5" />
                      <Badge variant="outline">
                        {datasourceTypes[formData.datasource_type]?.label}
                      </Badge>
                    </div>
                  )}

                  {/* Connection tab content */}
                  {renderConnectionTab()}
                </div>
              )}

              {/* Catalog tab */}
              {activeTab === 'catalog' &&
                (isCreateMode ? (
                  renderCatalogTabCreateMode()
                ) : (
                  datasource && (
                    <CatalogSection
                      datasource={{ ...datasource, connection_config: formData.connection_config }}
                      apiClient={apiClient}
                      isAdmin={canAdmin}
                      onConfigChange={handleCatalogConfigChange}
                    />
                  )
                ))}
            </div>
          </div>
        ) : canAdmin ? (
          // Admin but no catalog tab - show connection without tabs
          <div className="min-h-[400px]">
            <div className="space-y-4">
              {/* Type selector - only in create mode */}
              {isCreateMode && (
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
                      {Object.entries(datasourceTypes).map(([key, { label }]) => (
                        <SelectItem key={key} value={key}>
                          <div className="flex items-center gap-2">
                            <DatasourceIcon type={key} className="h-4 w-4" opacity={0.8} />
                            <span>{label}</span>
                          </div>
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              )}

              {/* Type badge in edit mode */}
              {!isCreateMode && (
                <div className="flex items-center gap-2">
                  <DatasourceIcon type={formData.datasource_type} className="h-5 w-5" />
                  <Badge variant="outline">
                    {datasourceTypes[formData.datasource_type]?.label}
                  </Badge>
                </div>
              )}

              {/* Connection tab content */}
              {renderConnectionTab()}
            </div>
          </div>
        ) : (
          // Non-admin: just show connection tab (no tabs UI)
          <div className="min-h-[300px]">{renderConnectionTab()}</div>
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

      {/* Disconnect Google Account Confirmation */}
      <ConfirmDialog
        isOpen={showDisconnectConfirm}
        onConfirm={handleDisconnectGoogle}
        onCancel={() => setShowDisconnectConfirm(false)}
        title="Disconnect Google Account?"
        message="This will remove your BigQuery access. You'll need to reconnect your Google account to query BigQuery again."
        confirmText="Disconnect"
        variant="destructive"
      />
    </>
  );
}
