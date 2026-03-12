// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/components/OAuthConfig.jsx
import { Button } from '@/components/ui/button';
import { FormField } from './FormField';
import { oauthClientIdField, oauthClientSecretField } from '../schemas';
import { toast } from 'sonner';

/**
 * OAuthConfig - OAuth configuration (client ID/secret) for admins
 *
 * Displays:
 * - Redirect URL with copy button
 * - OAuth Client ID field
 * - OAuth Client Secret field
 *
 * @param {string} provider - Provider name for display (e.g., 'Snowflake', 'BigQuery')
 * @param {string} callbackPath - OAuth callback path (e.g., '/auth/oauth/snowflake/callback')
 * @param {Object} values - Current values { oauth_client_id, oauth_client_secret }
 * @param {function} onChange - Handler for value changes (fieldName, value) => void
 * @param {boolean} disabled - Whether fields are disabled
 * @param {boolean} optional - Whether OAuth config is optional (default app used if not provided)
 * @param {string} optionalText - Text to display when optional=true
 */
export function OAuthConfig({
  provider,
  callbackPath,
  values,
  onChange,
  disabled = false,
  optional = false,
  optionalText,
}) {
  const redirectUrl = `${window.location.origin}${callbackPath}`;

  const handleCopyRedirectUrl = async () => {
    try {
      await navigator.clipboard.writeText(redirectUrl);
      toast.success('Redirect URL copied');
    } catch (error) {
      toast.error('Failed to copy to clipboard');
    }
  };

  return (
    <div className="space-y-3 pb-4 border-b border-border">
      <h4 className="text-sm font-medium text-foreground">
        OAuth Configuration {optional && <span className="text-muted-foreground font-normal">(Optional)</span>}
      </h4>
      <p className="text-xs text-muted-foreground">
        {optionalText || `Configure your ${provider} OAuth integration for user authentication.`}
      </p>

      {/* Redirect URL */}
      <div className="p-3 bg-muted/30 rounded-lg space-y-1">
        <label className="block text-xs font-medium text-muted-foreground">
          Redirect URL (use this when creating your OAuth integration)
        </label>
        <div className="flex items-center gap-2">
          <code className="flex-1 px-2 py-1 bg-background border border-input rounded text-xs font-mono break-all text-foreground">
            {redirectUrl}
          </code>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={handleCopyRedirectUrl}
          >
            Copy
          </Button>
        </div>
      </div>

      {/* OAuth credentials */}
      <div className="grid grid-cols-2 gap-4">
        <FormField
          field={oauthClientIdField({ placeholder: `From ${provider} OAuth integration` })}
          value={values.oauth_client_id}
          onChange={(v) => onChange('oauth_client_id', v)}
          disabled={disabled}
        />
        <FormField
          field={oauthClientSecretField()}
          value={values.oauth_client_secret}
          onChange={(v) => onChange('oauth_client_secret', v)}
          disabled={disabled}
        />
      </div>
    </div>
  );
}
