// SPDX-License-Identifier: AGPL-3.0-or-later
import { useEffect } from 'react';
import { useRegisterSW } from 'virtual:pwa-register/react';

// Check for updates every 5 minutes (iOS is aggressive with caching)
const UPDATE_CHECK_INTERVAL = 5 * 60 * 1000;

/**
 * Nuclear option: Clear all caches and service workers, then reload.
 * Used when updates fail to apply cleanly (white screen scenario on iOS).
 */
async function clearCachesAndReload() {
  try {
    // Unregister all service workers
    if ('serviceWorker' in navigator) {
      const registrations = await navigator.serviceWorker.getRegistrations();
      await Promise.all(registrations.map(reg => reg.unregister()));
    }

    // Clear all caches
    if ('caches' in window) {
      const cacheNames = await caches.keys();
      await Promise.all(cacheNames.map(name => caches.delete(name)));
    }

    // Hard reload (bypass cache)
    window.location.reload();
  } catch (err) {
    // Still try to reload
    window.location.reload();
  }
}

/**
 * PWAUpdatePrompt - Silently activates new service worker versions.
 *
 * When a new service worker is detected, it is activated immediately without
 * prompting the user. The updated cached assets take effect on the next
 * navigation — no forced reload, no popup.
 *
 * iOS-specific handling:
 * - Checks for updates on visibility change (when user returns to app)
 * - Periodic update checks every 5 minutes
 * - Auto-reloads if update fails to apply cleanly
 */
const PWAUpdatePrompt = () => {
  const {
    needRefresh: [needRefresh],
    updateServiceWorker,
  } = useRegisterSW({
    onRegistered(registration) {
      // Check for updates periodically (iOS doesn't always check automatically)
      if (registration) {
        // Initial check
        registration.update();

        // Periodic checks
        setInterval(() => {
          registration.update();
        }, UPDATE_CHECK_INTERVAL);

        // Check when page becomes visible (iOS background tab handling)
        document.addEventListener('visibilitychange', () => {
          if (document.visibilityState === 'visible') {
            registration.update();
          }
        });
      }
    },
    onRegisterError() {
      // Clear all caches and reload
      clearCachesAndReload();
    },
  });

  // Silently activate the new service worker without reloading
  useEffect(() => {
    if (needRefresh) {
      updateServiceWorker(false);
    }
  }, [needRefresh]);

  // No UI — updates are silent
  return null;
};

export default PWAUpdatePrompt;
