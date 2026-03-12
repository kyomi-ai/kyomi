// SPDX-License-Identifier: AGPL-3.0-or-later
import React from 'react';
import { ExclamationTriangleIcon } from '@heroicons/react/24/outline';
import { UnifiedStatusBar } from '@/components/ui/unified-status-bar';
import { useStatusBarDismiss } from '@/hooks/useStatusBarDismiss';
import { Link } from 'react-router-dom';

/**
 * AI Usage Status Bar Component
 *
 * Displays a warning status bar at the bottom when AI budget usage is high:
 * - 80-89%: Warning (yellow)
 * - 90-99%: Critical warning (orange)
 * - 100%+: Blocked (red) - though AI features will be disabled at this point
 *
 * Users can dismiss the warning, which is stored in localStorage with a 24-hour expiry.
 */
const AIUsageStatusBar = ({
  warningLevel = null, // null | 'warning' | 'critical' | 'blocked'
  percentageUsed = 0,
  message = '',
}) => {
  const storageKey = `ai-usage-warning-dismissed-${warningLevel}`;
  const { isDismissed, handleDismiss } = useStatusBarDismiss(storageKey, {
    expiryHours: 24,
  });

  // Don't show if no warning or if dismissed
  if (!warningLevel || warningLevel === 'none' || isDismissed) {
    return null;
  }

  // Map warning level to variant
  const getVariant = () => {
    switch (warningLevel) {
      case 'warning':
        return 'warning';
      case 'critical':
      case 'blocked':
        return 'error';
      default:
        return 'warning';
    }
  };

  const displayMessage = message || `AI budget at ${percentageUsed.toFixed(1)}%`;

  const actions = (
    <Link
      to="/settings/billing"
      className="px-3 py-1 text-sm font-medium bg-background/20 hover:bg-background/30 rounded transition-colors"
    >
      Upgrade Plan
    </Link>
  );

  return (
    <UnifiedStatusBar
      variant={getVariant()}
      icon={<ExclamationTriangleIcon className="w-5 h-5" />}
      message={displayMessage}
      actions={actions}
      onDismiss={() => handleDismiss({
        level: warningLevel,
        percentage: percentageUsed
      })}
      dismissLabel="Dismiss for 24 hours"
    />
  );
};

export default AIUsageStatusBar;
