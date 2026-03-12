// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useCallback } from 'react';

/**
 * Hook for managing status bar dismissal with optional localStorage persistence.
 *
 * @param {string} storageKey - Key for localStorage persistence (e.g., 'ai-usage-warning-dismissed-warning')
 * @param {Object} options - Configuration options
 * @param {number} options.expiryHours - Hours until dismissal expires (default: 24). Set to 0 for no expiry.
 * @param {boolean} options.persist - Whether to persist to localStorage (default: true)
 * @returns {Object} - { isDismissed, handleDismiss, resetDismiss }
 */
export function useStatusBarDismiss(storageKey, options = {}) {
  const { expiryHours = 24, persist = true } = options;
  const [isDismissed, setIsDismissed] = useState(false);

  // Check if warning was dismissed recently
  useEffect(() => {
    if (!persist || !storageKey) {
      return;
    }

    const dismissedData = localStorage.getItem(storageKey);

    if (dismissedData) {
      try {
        const { timestamp } = JSON.parse(dismissedData);
        const now = Date.now();
        const hoursSinceDismissed = (now - timestamp) / (1000 * 60 * 60);

        // If dismissed within expiry period, keep it dismissed
        if (expiryHours === 0 || hoursSinceDismissed < expiryHours) {
          setIsDismissed(true);
        } else {
          // Expired - remove from localStorage
          localStorage.removeItem(storageKey);
        }
      } catch {
        // Invalid data - remove it
        localStorage.removeItem(storageKey);
      }
    }
  }, [storageKey, expiryHours, persist]);

  // Handle dismiss action
  // Note: When used as onClick handler, metadata will be the event object - ignore it
  const handleDismiss = useCallback((metadata = {}) => {
    // Ignore React events (when used as onClick handler)
    const isEvent = metadata && (metadata.nativeEvent || metadata.target);
    const safeMetadata = isEvent ? {} : metadata;

    if (persist && storageKey) {
      try {
        localStorage.setItem(storageKey, JSON.stringify({
          timestamp: Date.now(),
          ...safeMetadata
        }));
      } catch (e) {
        // Fallback if metadata can't be serialized
        localStorage.setItem(storageKey, JSON.stringify({
          timestamp: Date.now()
        }));
      }
    }
    setIsDismissed(true);
  }, [storageKey, persist]);

  // Reset dismiss state
  const resetDismiss = useCallback(() => {
    if (persist && storageKey) {
      localStorage.removeItem(storageKey);
    }
    setIsDismissed(false);
  }, [storageKey, persist]);

  return { isDismissed, handleDismiss, resetDismiss };
}

export default useStatusBarDismiss;
