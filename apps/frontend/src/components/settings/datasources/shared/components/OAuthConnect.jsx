// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/components/OAuthConnect.jsx
import { Button } from '@/components/ui/button';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Check, AlertCircle, AlertTriangle } from 'lucide-react';
import { Spinner } from '@/components/ui/spinner';
import { DatasourceIcon } from '@/components/ui/DatasourceIcon';

// Map datasource type to display name for OAuth buttons
const DATASOURCE_LABELS = {
  bigquery: 'BigQuery',
  synapse: 'Azure Synapse',
  snowflake: 'Snowflake',
  databricks: 'Databricks',
};

/**
 * OAuthConnect - Unified OAuth connect/disconnect UI
 *
 * Displays different states:
 * - Not configured: Warning alert asking admin to configure OAuth
 * - Connected: Shows email and disconnect button
 * - Expired: Shows warning message and reconnect button
 * - Not connected: Shows connect button with datasource icon
 *
 * @param {string} datasourceType - Datasource type for icon ('bigquery', 'synapse', 'snowflake', etc.)
 * @param {string} providerLabel - Display name for the OAuth provider (e.g., "Google", "Microsoft")
 * @param {Object} status - Connection status { connected, email, connecting, disconnecting }
 * @param {function} onConnect - Handler for connect action
 * @param {function} onDisconnect - Handler for disconnect action
 * @param {boolean} disabled - Whether actions are disabled
 * @param {boolean} configValid - Whether OAuth credentials are configured
 * @param {string} helpText - Optional help text to display
 * @param {string} credentialStatus - Credential status from backend: 'valid', 'expired', 'missing', 'shared'
 */
export function OAuthConnect({
  datasourceType,
  providerLabel,
  status,
  onConnect,
  onDisconnect,
  disabled = false,
  configValid = true,
  helpText,
  credentialStatus,
}) {
  const datasourceLabel = DATASOURCE_LABELS[datasourceType] || providerLabel;
  // OAuth not configured - show warning for non-admins
  if (!configValid) {
    return (
      <Alert variant="warning">
        <AlertCircle className="h-4 w-4" />
        <AlertDescription>
          OAuth credentials not configured. Ask your admin to configure OAuth Client ID and Secret.
        </AlertDescription>
      </Alert>
    );
  }

  // Connected state - show email and disconnect button
  if (status.connected) {
    return (
      <div className="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
        <div className="flex items-center gap-2">
          <Check className="h-4 w-4 text-success-foreground" />
          <span className="text-sm text-foreground">
            {status.email ? `Connected: ${status.email}` : `Connected to ${providerLabel}`}
          </span>
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={onDisconnect}
          disabled={status.disconnecting}
        >
          {status.disconnecting ? (
            <>
              <Spinner size="sm" className="mr-2" />
              Disconnecting...
            </>
          ) : (
            'Disconnect'
          )}
        </Button>
      </div>
    );
  }

  // Expired state - show warning and reconnect button
  if (credentialStatus === 'expired') {
    return (
      <div className="space-y-3">
        <Alert variant="warning">
          <AlertTriangle className="h-4 w-4" />
          <AlertDescription>
            Your {datasourceLabel} connection has expired. Please reconnect to continue querying.
          </AlertDescription>
        </Alert>
        <Button
          variant="outline"
          onClick={onConnect}
          disabled={disabled || status.connecting}
        >
          {status.connecting ? (
            <>
              <Spinner size="sm" className="mr-2" />
              Reconnecting...
            </>
          ) : (
            <>
              <DatasourceIcon type={datasourceType} className="h-4 w-4 mr-2" opacity={1} />
              Reconnect {datasourceLabel}
            </>
          )}
        </Button>
      </div>
    );
  }

  // Not connected - show connect button
  return (
    <div className="space-y-2">
      <Button
        variant="outline"
        onClick={onConnect}
        disabled={disabled || status.connecting}
      >
        {status.connecting ? (
          <>
            <Spinner size="sm" className="mr-2" />
            Connecting...
          </>
        ) : (
          <>
            <DatasourceIcon type={datasourceType} className="h-4 w-4 mr-2" opacity={1} />
            Connect {datasourceLabel}
          </>
        )}
      </Button>
      {helpText && (
        <p className="text-xs text-muted-foreground">{helpText}</p>
      )}
    </div>
  );
}
