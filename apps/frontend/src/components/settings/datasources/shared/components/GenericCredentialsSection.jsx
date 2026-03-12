// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/components/GenericCredentialsSection.jsx
import { useRef } from 'react';
import { Button } from '@/components/ui/button';
import { Check, Upload, Lock, Plug, X, HelpCircle } from 'lucide-react';
import { Spinner } from '@/components/ui/spinner';
import { DatasourceIcon } from '@/components/ui/DatasourceIcon';
import { AuthModeSelector } from './AuthModeSelector';
import { OAuthConnect } from './OAuthConnect';
import { OAuthConfig } from './OAuthConfig';
import { SharedCredentialsToggle } from './SharedCredentialsToggle';
import { CredentialsForm } from './CredentialsForm';
import { ProjectDropdowns } from './ProjectDropdowns';

// Map OAuth providers to user-facing labels for OAuthConfig
const OAUTH_CONFIG_LABELS = {
  'BigQuery-enterprise': 'Google Cloud',
  'microsoft-enterprise': 'Microsoft Azure',
};

// Map OAuth providers to user-facing labels for OAuthConnect
const OAUTH_CONNECT_LABELS = {
  'BigQuery-enterprise': 'Google',
  'microsoft-enterprise': 'Microsoft',
};

/**
 * GenericCredentialsSection — unified, schema-driven credentials section.
 *
 * Replaces per-provider CredentialsSection components (bigquery, snowflake,
 * databricks, synapse). Renders auth mode selector + mode-specific controls
 * based entirely on the provider schema's authModes properties.
 *
 * Used in CREATE mode of DatasourceModal.
 */
export function GenericCredentialsSection({
  schema,
  authMode,
  onAuthModeChange,
  // Global OAuth state (for kyomi_oauth / global OAuth modes)
  globalOAuthStatus,
  onGlobalOAuthConnect,
  onGlobalOAuthDisconnect,
  oauthConnecting = false,
  oauthDisconnecting = false,
  // Provider OAuth state (for enterprise/standard per-datasource OAuth)
  providerOAuthStatus,
  onProviderOAuthConnect,
  onProviderOAuthDisconnect,
  // Credentials form (per-user credential fields)
  credentialsForm,
  onCredentialsChange,
  // Connection config (workspace-level config)
  connectionConfig,
  onConnectionConfigChange,
  // Shared credentials
  sharedCredentials,
  onSharedCredentialsChange,
  // Admin permissions
  canAdmin,
  disabled,
  // Service account state
  serviceAccountEmail,
  serviceAccountJson,
  onServiceAccountUpload,
  onServiceAccountJsonChange,
  onServiceAccountClear,
  // Validation
  testing,
  testResult,
  onTestAndDiscover,
  // Discovery
  discoveredResources,
  googleProjects,
  projectFetchError,
  // Credential status from backend
  credentialStatus,
  // Beta access acknowledgment
  onBetaAcknowledgedChange,
  // Docs URL
  docsUrl,
}) {
  const fileInputRef = useRef(null);
  const currentMode = schema.authModes?.find((m) => m.value === authMode);

  // Check if current mode has discovery-type credential fields (e.g., project dropdowns)
  const hasDiscoveryFields = currentMode?.credentialFields?.some((f) => f.type === 'discovery');
  // Non-discovery credential fields (password, token, etc.)
  const hasStandardCredentialFields = currentMode?.credentialFields?.some((f) => f.type !== 'discovery') && currentMode?.credentialFields?.length > 0;

  // Derive OAuth configuration state
  const oauthConfigured = currentMode?.oauth?.configFields?.every(
    (field) => !!connectionConfig?.[field.name]
  ) ?? true;

  // Derive OAuth provider label and type for handleConnectProviderOAuth
  const oauthProvider = currentMode?.oauth?.provider;
  const oauthConfigLabel = oauthProvider
    ? (OAUTH_CONFIG_LABELS[oauthProvider] || oauthProvider)
    : '';
  const oauthConnectLabel = oauthProvider
    ? (OAUTH_CONNECT_LABELS[oauthProvider] || oauthProvider)
    : '';
  // handleConnectProviderOAuth maps internally via its endpointMap
  const oauthProviderType = oauthProvider?.toLowerCase();

  // Project list source depends on auth mode
  const projectsList = currentMode?.serviceAccount
    ? (discoveredResources?.projects || [])
    : (googleProjects || []);

  // For service account, project values are stored in connection_config (workspace-level)
  // For OAuth modes, they're stored in credentialsForm (per-user)
  const projectFormValues = currentMode?.serviceAccount
    ? { billing_project: connectionConfig?.billing_project, default_project: connectionConfig?.default_project }
    : credentialsForm;
  const projectOnChange = currentMode?.serviceAccount
    ? onConnectionConfigChange
    : onCredentialsChange;

  return (
    <div className="space-y-4 border-t border-border pt-4 mt-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <h4 className="text-sm font-medium text-foreground">{schema.label} Credentials</h4>
        {docsUrl && (
          <a
            href={docsUrl}
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-1 text-xs text-muted-foreground hover:text-primary transition-colors"
          >
            <HelpCircle className="h-3.5 w-3.5" />
            Setup Guide
          </a>
        )}
      </div>

      {/* Auth mode selector (only for providers with multiple auth modes) */}
      {schema.authModes && (
        <AuthModeSelector
          modes={schema.authModes}
          value={authMode}
          onChange={onAuthModeChange}
          canAdmin={canAdmin}
          onBetaAcknowledgedChange={onBetaAcknowledgedChange}
        />
      )}

      {/* ============== GLOBAL OAUTH MODE (e.g., BigQuery kyomi_oauth) ============== */}
      {currentMode?.oauth?.global && (
        <div className="space-y-4">
          {globalOAuthStatus?.hasOauth && (
            <div className="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
              <div className="flex items-center gap-2">
                <Check className="h-4 w-4 text-success-foreground" />
                <span className="text-sm text-foreground">
                  {globalOAuthStatus.oauthEmail
                    ? `Google account: ${globalOAuthStatus.oauthEmail}`
                    : 'Google account connected'}
                </span>
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={onGlobalOAuthDisconnect}
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
          )}

          {globalOAuthStatus?.needsBigqueryConnect ? (
            <div className="space-y-3">
              <Button
                variant="outline"
                onClick={onGlobalOAuthConnect}
                disabled={oauthConnecting}
              >
                {oauthConnecting ? (
                  <>
                    <Spinner size="sm" />
                    Connecting...
                  </>
                ) : (
                  <>
                    <DatasourceIcon type={schema.type} className="h-4 w-4" />
                    Connect {schema.label}
                  </>
                )}
              </Button>
              <p className="text-xs text-muted-foreground">
                Sign in with Google to access your {schema.label} projects.
              </p>
            </div>
          ) : (
            /* Project dropdowns when connected */
            hasDiscoveryFields && (
              <ProjectDropdowns
                credentialsForm={projectFormValues}
                onCredentialsChange={projectOnChange}
                projectsList={projectsList}
                errorMessage={projectFetchError}
              />
            )
          )}
        </div>
      )}

      {/* ============== PER-DATASOURCE OAUTH (enterprise or standard) ============== */}
      {currentMode?.oauth && !currentMode.oauth.global && (
        <div className="space-y-4">
          {/* Admin OAuth Configuration (if mode has configFields) */}
          {canAdmin && currentMode.oauth.configFields && (
            <OAuthConfig
              provider={oauthConfigLabel}
              callbackPath={currentMode.callbackPath}
              values={connectionConfig}
              onChange={onConnectionConfigChange}
            />
          )}

          {/* User OAuth Connection */}
          <div className="space-y-3">
            <h4 className="text-sm font-medium text-foreground">Your Connection</h4>
            <OAuthConnect
              datasourceType={schema.type}
              providerLabel={oauthConnectLabel}
              status={providerOAuthStatus}
              onConnect={() => onProviderOAuthConnect(oauthProviderType)}
              onDisconnect={onProviderOAuthDisconnect}
              configValid={oauthConfigured}
              disabled={disabled}
              helpText={`Sign in with your ${oauthConnectLabel} account.`}
              credentialStatus={credentialStatus}
            />

            {/* Project dropdowns when connected (for modes with discovery credentialFields) */}
            {providerOAuthStatus?.connected && hasDiscoveryFields && (
              <div className="mt-4">
                <ProjectDropdowns
                  credentialsForm={projectFormValues}
                  onCredentialsChange={projectOnChange}
                  projectsList={projectsList}
                  errorMessage={projectFetchError}
                />
              </div>
            )}
          </div>
        </div>
      )}

      {/* ============== SERVICE ACCOUNT MODE ============== */}
      {currentMode?.serviceAccount && (
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
                    onClick={onServiceAccountClear}
                  >
                    Remove
                  </Button>
                </div>
              )}

              {/* File upload and JSON paste */}
              {!serviceAccountEmail && (
                <div className="space-y-3">
                  <div>
                    <input
                      type="file"
                      ref={fileInputRef}
                      accept=".json"
                      onChange={onServiceAccountUpload}
                      className="hidden"
                    />
                    <Button
                      variant="outline"
                      onClick={() => fileInputRef.current?.click()}
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

                  <div>
                    <textarea
                      value={serviceAccountJson}
                      onChange={(e) => onServiceAccountJsonChange(e.target.value)}
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

          {/* Validate & Discover + Project selection (when service account configured) */}
          {serviceAccountEmail && (
            <div className="space-y-4">
              <div className="flex items-center gap-3">
                <Button
                  variant="outline"
                  onClick={() => onTestAndDiscover()}
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

              {hasDiscoveryFields && (
                <ProjectDropdowns
                  credentialsForm={projectFormValues}
                  onCredentialsChange={projectOnChange}
                  projectsList={projectsList}
                  errorMessage={projectFetchError}
                />
              )}
            </div>
          )}
        </div>
      )}

      {/* ============== STANDARD CREDENTIAL MODES (password, token, keypair, etc.) ============== */}
      {currentMode && !currentMode.oauth && !currentMode.serviceAccount && currentMode.supportsSharedCredentials && (
        <SharedCredentialsToggle
          enabled={sharedCredentials}
          onChange={onSharedCredentialsChange}
          canAdmin={canAdmin}
        >
          {hasStandardCredentialFields && (
            <CredentialsForm
              fields={currentMode.credentialFields}
              values={sharedCredentials
                ? Object.fromEntries(currentMode.credentialFields.map((f) => [f.name, connectionConfig?.[`shared_${f.name}`] || '']))
                : credentialsForm}
              onChange={sharedCredentials
                ? (fieldName, value) => onConnectionConfigChange(`shared_${fieldName}`, value)
                : onCredentialsChange}
              disabled={disabled}
            />
          )}
        </SharedCredentialsToggle>
      )}

      {/* Credential fields for modes without shared credentials support */}
      {currentMode && !currentMode.oauth && !currentMode.serviceAccount && !currentMode.supportsSharedCredentials && hasStandardCredentialFields && (
        <CredentialsForm
          fields={currentMode.credentialFields}
          values={credentialsForm}
          onChange={onCredentialsChange}
          disabled={disabled}
        />
      )}

      {/* ============== SIMPLE PROVIDERS (no authModes, e.g., postgres, mysql) ============== */}
      {!currentMode && schema.credentialFields?.length > 0 && (
        schema.supportsSharedCredentials ? (
          <SharedCredentialsToggle
            enabled={sharedCredentials}
            onChange={onSharedCredentialsChange}
            canAdmin={canAdmin}
          >
            <CredentialsForm
              fields={schema.credentialFields}
              values={sharedCredentials
                ? Object.fromEntries(schema.credentialFields.map((f) => [f.name, connectionConfig?.[`shared_${f.name}`] || '']))
                : credentialsForm}
              onChange={sharedCredentials
                ? (fieldName, value) => onConnectionConfigChange(`shared_${fieldName}`, value)
                : onCredentialsChange}
              disabled={disabled}
            />
          </SharedCredentialsToggle>
        ) : (
          <CredentialsForm
            fields={schema.credentialFields}
            values={credentialsForm}
            onChange={onCredentialsChange}
            disabled={disabled}
          />
        )
      )}

    </div>
  );
}
