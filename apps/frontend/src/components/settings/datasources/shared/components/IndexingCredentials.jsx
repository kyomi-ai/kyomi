// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/components/IndexingCredentials.jsx
import { useState } from 'react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Upload, Check, HelpCircle, Info } from 'lucide-react';
import { Spinner } from '@/components/ui/spinner';
import { AuthModeSelector } from './AuthModeSelector';

/**
 * Indexing-compatible auth modes per datasource type.
 * These exclude OAuth modes since background jobs cannot refresh tokens.
 *
 * IMPORTANT: Each datasource type must be listed explicitly.
 * Do NOT use a "default" fallback - this prevents wrong auth modes for new datasources.
 */
const INDEXING_AUTH_MODES = {
  bigquery: [
    { value: 'service_account', label: 'Service Account', description: 'Use a Google Cloud service account JSON key' },
  ],
  synapse: [
    { value: 'sql', label: 'SQL Authentication', description: 'Username and password for SQL Server' },
    { value: 'service_principal', label: 'Service Principal', description: 'Azure AD app with client credentials' },
  ],
  snowflake: [
    { value: 'password', label: 'Password', description: 'Username and password authentication' },
  ],
  databricks: [
    { value: 'token', label: 'Personal Access Token', description: 'Databricks PAT for API access' },
  ],
  postgres: [
    { value: 'password', label: 'Password', description: 'Username and password authentication' },
  ],
  clickhouse: [
    { value: 'password', label: 'Password', description: 'Username and password authentication' },
  ],
  mysql: [
    { value: 'password', label: 'Password', description: 'Username and password authentication' },
  ],
  sqlserver: [
    { value: 'password', label: 'Password', description: 'Username and password authentication' },
  ],
  redshift: [
    { value: 'password', label: 'Password', description: 'Username and password authentication' },
  ],
};

/**
 * IndexingCredentials - Configure dedicated credentials for catalog indexing
 *
 * By default, catalog indexing uses the workspace owner's credentials.
 * This component allows admins to configure dedicated service account or
 * password credentials for indexing, which is useful when:
 * - Owner uses OAuth (tokens expire, background jobs cannot refresh)
 * - A dedicated read-only account is preferred for security
 * - Indexing needs different permissions than user queries
 */
export function IndexingCredentials({
  datasourceType,
  connectionConfig,
  onConnectionConfigChange,
  disabled = false,
}) {
  const indexingCreds = connectionConfig.indexing_credentials || null;
  const [useCustom, setUseCustom] = useState(!!indexingCreds);

  const handleToggle = (enabled) => {
    setUseCustom(enabled);
    if (!enabled) {
      onConnectionConfigChange('indexing_credentials', null);
    }
  };

  return (
    <div className="space-y-4 border-t border-border pt-4 mt-4">
      <div className="flex items-center justify-between">
        <div>
          <h4 className="text-sm font-medium text-foreground">Catalog Indexing Credentials</h4>
          <p className="text-xs text-muted-foreground mt-0.5">
            By default, the workspace owner's credentials are used for catalog indexing.
          </p>
        </div>
        <a
          href="https://kyomi.ai/docs/datasources/indexing-credentials"
          target="_blank"
          rel="noopener noreferrer"
          className="flex items-center gap-1 text-xs text-muted-foreground hover:text-primary transition-colors"
        >
          <HelpCircle className="h-3.5 w-3.5" />
          Learn more
        </a>
      </div>

      <label className="flex items-center gap-3 cursor-pointer">
        <input
          type="checkbox"
          checked={useCustom}
          onChange={(e) => handleToggle(e.target.checked)}
          disabled={disabled}
          className="w-4 h-4 text-primary border-border rounded focus:ring-primary"
        />
        <span className="text-sm text-foreground">Use dedicated indexing credentials</span>
      </label>

      {useCustom && (
        <div className="pl-7">
          <Alert variant="info" className="mb-4">
            <Info className="h-4 w-4" />
            <AlertDescription>
              OAuth credentials cannot be used for indexing (tokens expire and background jobs cannot
              refresh them). Use a service account or password-based credentials.
            </AlertDescription>
          </Alert>

          <IndexingCredentialsForm
            datasourceType={datasourceType}
            value={indexingCreds}
            onChange={(creds) => onConnectionConfigChange('indexing_credentials', creds)}
            disabled={disabled}
          />
        </div>
      )}
    </div>
  );
}

/**
 * IndexingCredentialsForm - Provider-specific form for indexing credentials
 * Shows auth mode selector if multiple modes are available.
 */
function IndexingCredentialsForm({ datasourceType, value, onChange, disabled }) {
  const authModes = INDEXING_AUTH_MODES[datasourceType];

  // Handle unknown datasource types gracefully
  if (!authModes) {
    return (
      <Alert variant="warning">
        <AlertDescription>
          Indexing credentials configuration is not available for {datasourceType} datasources.
        </AlertDescription>
      </Alert>
    );
  }

  const currentType = value?.type || authModes[0]?.value;

  const handleAuthModeChange = (newType) => {
    // Clear credentials when switching auth modes
    onChange({ type: newType });
  };

  const handleCredentialsChange = (credentials) => {
    if (credentials === null) {
      onChange(null);
    } else {
      onChange({ type: currentType, ...credentials });
    }
  };

  return (
    <div className="space-y-4">
      {/* Only show auth mode selector if multiple options */}
      {authModes.length > 1 && (
        <AuthModeSelector
          modes={authModes}
          value={currentType}
          onChange={handleAuthModeChange}
          canAdmin={true}
          disabled={disabled}
        />
      )}

      {/* Render appropriate form based on auth mode */}
      {currentType === 'service_account' && (
        <ServiceAccountForm value={value} onChange={handleCredentialsChange} disabled={disabled} />
      )}
      {currentType === 'service_principal' && (
        <ServicePrincipalForm value={value} onChange={handleCredentialsChange} disabled={disabled} />
      )}
      {(currentType === 'password' || currentType === 'sql') && (
        <PasswordForm value={value} onChange={handleCredentialsChange} disabled={disabled} />
      )}
      {currentType === 'token' && (
        <TokenForm value={value} onChange={handleCredentialsChange} disabled={disabled} />
      )}
    </div>
  );
}

/**
 * ServiceAccountForm - Google Cloud service account JSON upload
 */
function ServiceAccountForm({ value, onChange, disabled }) {
  const [jsonText, setJsonText] = useState('');
  const [uploading, setUploading] = useState(false);
  const serviceAccountEmail = value?.service_account_json
    ? parseServiceAccountEmail(value.service_account_json)
    : null;

  const handleFileUpload = (e) => {
    const file = e.target.files?.[0];
    if (!file) return;

    setUploading(true);
    const reader = new FileReader();

    reader.onload = (event) => {
      try {
        const json = JSON.parse(event.target.result);
        if (json.type === 'service_account' && json.client_email) {
          onChange({
            service_account_json: event.target.result,
          });
          setJsonText('');
          toast.success('Service account loaded successfully');
        } else {
          toast.error('Invalid service account JSON. Please upload a valid Google Cloud service account key file.');
        }
      } catch (err) {
        toast.error('Invalid JSON file. Please upload a valid service account JSON.');
      } finally {
        setUploading(false);
        // Clear the file input so the same file can be uploaded again
        e.target.value = null;
      }
    };

    reader.onerror = () => {
      toast.error('Failed to read file. Please try again.');
      setUploading(false);
      e.target.value = null;
    };

    reader.readAsText(file);
  };

  const handleJsonPaste = (text) => {
    setJsonText(text);
    if (!text.trim()) {
      return;
    }
    try {
      const json = JSON.parse(text);
      if (json.type === 'service_account' && json.client_email) {
        onChange({
          service_account_json: text,
        });
        toast.success('Service account loaded successfully');
      }
    } catch (err) {
      // Let user continue typing - only apply when valid JSON
    }
  };

  const handleClear = () => {
    onChange(null);
    setJsonText('');
  };

  if (serviceAccountEmail) {
    return (
      <div className="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
        <div className="flex items-center gap-2">
          <Check className="h-4 w-4 text-success-foreground" />
          <span className="text-sm text-foreground">Service Account: {serviceAccountEmail}</span>
        </div>
        <Button variant="outline" size="sm" onClick={handleClear} disabled={disabled}>
          Remove
        </Button>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div>
        <input
          type="file"
          accept=".json"
          onChange={handleFileUpload}
          className="hidden"
          id="indexing-sa-upload"
          disabled={disabled}
        />
        <Button
          variant="outline"
          onClick={() => document.getElementById('indexing-sa-upload')?.click()}
          disabled={disabled || uploading}
        >
          {uploading ? (
            <>
              <Spinner size="sm" className="mr-2" />
              Loading...
            </>
          ) : (
            <>
              <Upload className="h-4 w-4 mr-2" />
              Upload service account JSON
            </>
          )}
        </Button>
      </div>

      <div className="flex items-center gap-2">
        <div className="flex-1 h-px bg-border" />
        <span className="text-xs text-muted-foreground">or paste JSON</span>
        <div className="flex-1 h-px bg-border" />
      </div>

      <textarea
        value={jsonText}
        onChange={(e) => handleJsonPaste(e.target.value)}
        placeholder='{"type": "service_account", "client_email": "...", ...}'
        rows={4}
        disabled={disabled}
        className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono"
      />
    </div>
  );
}

/**
 * ServicePrincipalForm - Azure AD service principal credentials
 */
function ServicePrincipalForm({ value, onChange, disabled }) {
  const [tenantId, setTenantId] = useState(value?.tenant_id || '');
  const [clientId, setClientId] = useState(value?.client_id || '');
  const [clientSecret, setClientSecret] = useState(value?.client_secret || '');

  const handleChange = (field, fieldValue) => {
    const newTenantId = field === 'tenant_id' ? fieldValue : tenantId;
    const newClientId = field === 'client_id' ? fieldValue : clientId;
    const newClientSecret = field === 'client_secret' ? fieldValue : clientSecret;

    if (field === 'tenant_id') setTenantId(fieldValue);
    if (field === 'client_id') setClientId(fieldValue);
    if (field === 'client_secret') setClientSecret(fieldValue);

    if (newTenantId && newClientId && newClientSecret) {
      onChange({
        tenant_id: newTenantId,
        client_id: newClientId,
        client_secret: newClientSecret,
      });
    } else {
      onChange(null);
    }
  };

  return (
    <div className="space-y-3">
      <div>
        <label className="block text-sm font-medium text-foreground mb-1">Tenant ID</label>
        <input
          type="text"
          value={tenantId}
          onChange={(e) => handleChange('tenant_id', e.target.value)}
          placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
          disabled={disabled}
          className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
        />
      </div>
      <div>
        <label className="block text-sm font-medium text-foreground mb-1">Client ID</label>
        <input
          type="text"
          value={clientId}
          onChange={(e) => handleChange('client_id', e.target.value)}
          placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
          disabled={disabled}
          className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
        />
      </div>
      <div>
        <label className="block text-sm font-medium text-foreground mb-1">Client Secret</label>
        <input
          type="password"
          value={clientSecret}
          onChange={(e) => handleChange('client_secret', e.target.value)}
          placeholder="Enter client secret"
          disabled={disabled}
          className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
        />
      </div>
      <p className="text-xs text-muted-foreground">
        Azure AD service principal credentials for catalog indexing.
      </p>
    </div>
  );
}

/**
 * PasswordForm - Generic username/password credentials
 */
function PasswordForm({ value, onChange, disabled }) {
  const [username, setUsername] = useState(value?.username || '');
  const [password, setPassword] = useState(value?.password || '');

  const handleChange = (field, fieldValue) => {
    const newUsername = field === 'username' ? fieldValue : username;
    const newPassword = field === 'password' ? fieldValue : password;

    if (field === 'username') setUsername(fieldValue);
    if (field === 'password') setPassword(fieldValue);

    // Always update parent with current state
    if (newUsername && newPassword) {
      onChange({
        username: newUsername,
        password: newPassword,
      });
    } else {
      // Clear credentials if either field is empty
      onChange(null);
    }
  };

  return (
    <div className="space-y-3">
      <div>
        <label className="block text-sm font-medium text-foreground mb-1">Username</label>
        <input
          type="text"
          value={username}
          onChange={(e) => handleChange('username', e.target.value)}
          placeholder="e.g., readonly_indexer"
          disabled={disabled}
          className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
        />
      </div>
      <div>
        <label className="block text-sm font-medium text-foreground mb-1">Password</label>
        <input
          type="password"
          value={password}
          onChange={(e) => handleChange('password', e.target.value)}
          placeholder="Enter password"
          disabled={disabled}
          className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
        />
      </div>
      <p className="text-xs text-muted-foreground">
        These credentials will be used for catalog indexing only, not for user queries.
      </p>
    </div>
  );
}

/**
 * TokenForm - Personal Access Token (for Databricks)
 */
function TokenForm({ value, onChange, disabled }) {
  const [token, setToken] = useState(value?.access_token || '');

  const handleChange = (newToken) => {
    setToken(newToken);
    if (newToken) {
      onChange({ access_token: newToken });
    } else {
      onChange(null);
    }
  };

  return (
    <div className="space-y-3">
      <div>
        <label className="block text-sm font-medium text-foreground mb-1">Personal Access Token</label>
        <input
          type="password"
          value={token}
          onChange={(e) => handleChange(e.target.value)}
          placeholder="dapi..."
          disabled={disabled}
          className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
        />
      </div>
      <p className="text-xs text-muted-foreground">
        Databricks Personal Access Token for catalog indexing.
      </p>
    </div>
  );
}

/**
 * Parse service account email from JSON string
 */
function parseServiceAccountEmail(jsonString) {
  try {
    const json = JSON.parse(jsonString);
    return json.client_email || null;
  } catch {
    return null;
  }
}
