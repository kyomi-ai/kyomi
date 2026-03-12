// SPDX-License-Identifier: AGPL-3.0-or-later
import React from 'react';
import { StatusBar, StatusBarText } from '@/components/ui/status-bar';
import { Button } from '@/components/ui/button';
import { XMarkIcon } from '@heroicons/react/24/outline';

/**
 * UnifiedStatusBar - A standardized status bar component that encapsulates
 * common patterns across all status bar variants.
 *
 * ## Dismiss Pattern Guidelines
 *
 * Status bars handle dismissal differently based on their requirements:
 *
 * 1. **Self-managed dismiss with useStatusBarDismiss hook** (e.g., AIUsageStatusBar)
 *    - Use when: Dismiss is time-based (e.g., "don't show for 24 hours")
 *    - The component imports useStatusBarDismiss and manages its own localStorage
 *    - Parent doesn't need to track dismiss state
 *
 * 2. **Parent-managed dismiss via props** (e.g., CatalogStatusBar for credential status)
 *    - Use when: Parent needs to reset dismiss state based on external events
 *    - Example: OAuth dismiss should reset when token actually expires
 *    - Parent uses useStatusBarDismiss and passes onDismiss callback
 *
 * 3. **No dismiss** (e.g., CatalogStatusBar, InvitationStatusBar)
 *    - Use when: Status bar represents an active process or required action
 *    - User must wait for completion or take explicit action (accept/decline)
 *
 * This component provides:
 * - Consistent layout (icon + message + actions + dismiss)
 * - Variant-based styling (warning, error, info, success, default)
 * - Optional dismiss button with callback
 * - Flexible action rendering
 *
 * @param {string} variant - 'default' | 'warning' | 'error' | 'success' | 'info'
 * @param {React.ReactNode} icon - Icon to display on the left
 * @param {string|React.ReactNode} message - Message content
 * @param {React.ReactNode} actions - Action buttons/links to display on the right
 * @param {Function} onDismiss - Optional dismiss callback
 * @param {string} dismissLabel - Accessibility label for dismiss button (default: 'Dismiss')
 * @param {boolean} show - Whether to show the status bar (default: true)
 * @param {string} className - Additional CSS classes
 */
const UnifiedStatusBar = ({
  variant = 'default',
  icon,
  message,
  actions,
  onDismiss,
  dismissLabel = 'Dismiss',
  show = true,
  className,
}) => {
  if (!show) {
    return null;
  }

  return (
    <div className="w-full flex-shrink-0">
      <StatusBar variant={variant} className={`max-w-none ${className || ''}`}>
        <div className="max-w-7xl mx-auto flex items-center justify-between gap-4 w-full">
          {/* Icon and Message */}
          <div className="flex items-center gap-3 flex-1 min-w-0">
            {icon && (
              <span className="flex-shrink-0">
                {icon}
              </span>
            )}
            <StatusBarText variant={variant}>
              {message}
            </StatusBarText>
          </div>

          {/* Action Buttons */}
          <div className="flex items-center gap-3 flex-shrink-0">
            {actions}
            {onDismiss && (
              <Button
                onClick={onDismiss}
                variant="ghost"
                size="sm"
                aria-label={dismissLabel}
                title={dismissLabel}
              >
                <XMarkIcon className="w-5 h-5" />
              </Button>
            )}
          </div>
        </div>
      </StatusBar>
    </div>
  );
};

export { UnifiedStatusBar };
export default UnifiedStatusBar;
