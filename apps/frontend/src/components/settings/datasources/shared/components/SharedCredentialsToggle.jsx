// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/components/SharedCredentialsToggle.jsx
import { Lock } from 'lucide-react';

/**
 * SharedCredentialsToggle - Toggle for shared vs individual credentials
 *
 * Displays different UI based on user permissions:
 * - Non-admin with shared creds: Shows locked message
 * - Admin: Shows toggle checkbox + credential fields as children
 *
 * @param {boolean} enabled - Whether shared credentials are enabled
 * @param {function} onChange - Handler for toggle change
 * @param {boolean} canAdmin - Whether user can administer this datasource
 * @param {React.ReactNode} children - Credential fields to show when NOT using shared
 */
export function SharedCredentialsToggle({
  enabled,
  onChange,
  canAdmin,
  children,
}) {
  // Non-admin viewing shared credentials - show locked message
  if (enabled && !canAdmin) {
    return (
      <div className="flex items-center gap-2 p-3 bg-muted/50 rounded-lg">
        <Lock className="h-4 w-4 text-muted-foreground" />
        <span className="text-sm text-muted-foreground">
          Using shared credentials configured by admin
        </span>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {/* Toggle - admin only */}
      {canAdmin && (
        <label className="flex items-center gap-3 p-3 bg-muted/30 rounded-lg cursor-pointer hover:bg-muted/50 transition-colors">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => onChange(e.target.checked)}
            className="h-4 w-4 rounded border-border text-primary focus:ring-ring"
          />
          <div>
            <p className="text-sm font-medium text-foreground">All users share these credentials</p>
            <p className="text-xs text-muted-foreground">
              Use a service account instead of individual user credentials
            </p>
          </div>
        </label>
      )}

      {/* Credential fields - show for admins (to set shared creds) or when not using shared */}
      {(canAdmin || !enabled) && children}
    </div>
  );
}
