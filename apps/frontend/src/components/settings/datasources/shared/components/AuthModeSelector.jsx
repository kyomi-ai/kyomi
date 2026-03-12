// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/components/AuthModeSelector.jsx
import { useState, useEffect } from 'react';
import { Link } from 'react-router-dom';
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { AlertCircle } from 'lucide-react';

/**
 * AuthModeSelector - Generic auth mode picker
 *
 * Displays different UI based on permissions:
 * - Non-admin: Shows current mode label only (read-only)
 * - Admin: Shows dropdown to select mode with description
 *
 * When a mode has requiresBeta: true, shows a warning and requires
 * the user to check an acknowledgment checkbox before saving.
 *
 * @param {Array} modes - Array of { value, label, description, requiresBeta? }
 * @param {string} value - Current mode value
 * @param {function} onChange - Handler for mode change (newValue) => void
 * @param {boolean} canAdmin - Whether user can change the mode
 * @param {boolean} disabled - Whether selector is disabled
 * @param {function} onBetaAcknowledgedChange - Callback when beta acknowledgment changes (acknowledged: boolean) => void
 */
export function AuthModeSelector({
  modes,
  value,
  onChange,
  canAdmin,
  disabled = false,
  onBetaAcknowledgedChange,
}) {
  const currentMode = modes.find((m) => m.value === value) || modes[0];
  const requiresBeta = currentMode?.requiresBeta === true;

  // Initialize from localStorage - beta access is remembered across the app
  const [betaAcknowledged, setBetaAcknowledged] = useState(() => {
    try {
      return localStorage.getItem('hasBetaAccess') === 'true';
    } catch {
      return false;
    }
  });

  // Notify parent of initial state on mount
  useEffect(() => {
    if (onBetaAcknowledgedChange) {
      onBetaAcknowledgedChange(betaAcknowledged);
    }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Listen for changes from other components (e.g., login page)
  useEffect(() => {
    const handleStorageChange = (e) => {
      if (e.key === 'hasBetaAccess') {
        const newValue = e.newValue === 'true';
        setBetaAcknowledged(newValue);
        if (onBetaAcknowledgedChange) {
          onBetaAcknowledgedChange(newValue);
        }
      }
    };
    window.addEventListener('storage', handleStorageChange);
    return () => window.removeEventListener('storage', handleStorageChange);
  }, [onBetaAcknowledgedChange]);

  const handleAcknowledgmentChange = (e) => {
    const newValue = e.target.checked;
    setBetaAcknowledged(newValue);
    // Store in localStorage for sync across the app
    try {
      localStorage.setItem('hasBetaAccess', String(newValue));
    } catch {
      // Ignore storage errors
    }
    // Notify parent of checkbox state change
    if (onBetaAcknowledgedChange) {
      onBetaAcknowledgedChange(newValue);
    }
  };

  // Non-admin: read-only display
  if (!canAdmin) {
    return (
      <div className="space-y-2 pb-4 border-b border-border">
        <label className="block text-sm font-medium text-foreground">Authentication Mode</label>
        <p className="text-sm text-muted-foreground">{currentMode.label}</p>
      </div>
    );
  }

  // Admin: editable dropdown
  return (
    <div className="space-y-2 pb-4 border-b border-border">
      <label className="block text-sm font-medium text-foreground">Authentication Mode</label>
      <Select value={value} onValueChange={onChange} disabled={disabled}>
        <SelectTrigger>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {modes.map((mode) => (
            <SelectItem key={mode.value} value={mode.value}>
              {mode.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <p className="text-xs text-muted-foreground">
        {currentMode.description}
      </p>
      {requiresBeta && (
        <Alert variant="warning" className="mt-3">
          <AlertCircle className="h-4 w-4" />
          <AlertDescription>
            <p className="mb-2">
              This authentication method requires beta access.{' '}
              <Link to="/beta-signup" className="text-primary hover:underline font-medium">
                Request beta access
              </Link>
            </p>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={betaAcknowledged}
                onChange={handleAcknowledgmentChange}
                className="h-4 w-4 rounded border-input"
              />
              <span className="text-sm">I have beta access</span>
            </label>
          </AlertDescription>
        </Alert>
      )}
    </div>
  );
}
