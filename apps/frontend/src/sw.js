// SPDX-License-Identifier: AGPL-3.0-or-later
/// <reference lib="webworker" />
/**
 * Custom service worker for Kyomi PWA.
 *
 * Handles:
 * 1. Precaching (Workbox injectManifest — replaces __WB_MANIFEST at build time)
 * 2. Navigation fallback to index.html (SPA routing)
 * 3. Runtime caching: Google Fonts (CacheFirst, 1yr), API calls (NetworkFirst, 5min)
 * 4. Push notification display and click handling
 *
 * This file replaces the auto-generated Workbox SW that was used with `generateSW`.
 * All existing caching behavior is preserved.
 */

import { cleanupOutdatedCaches, precacheAndRoute } from 'workbox-precaching'
import { registerRoute, NavigationRoute } from 'workbox-routing'
import { CacheFirst, NetworkFirst } from 'workbox-strategies'
import { ExpirationPlugin } from 'workbox-expiration'
import { CacheableResponsePlugin } from 'workbox-cacheable-response'

// ---------------------------------------------------------------------------
// 1. Precaching
// ---------------------------------------------------------------------------

// Cleanup old caches from previous SW versions.
cleanupOutdatedCaches()

// Precache all static assets injected by vite-plugin-pwa at build time.
// The __WB_MANIFEST token is replaced with the actual manifest entries.
precacheAndRoute(self.__WB_MANIFEST)

// ---------------------------------------------------------------------------
// 2. Navigation fallback (SPA)
// ---------------------------------------------------------------------------

// All navigation requests (except /api/*) fall back to index.html.
// This enables client-side routing to work for direct URL access.
const navigationHandler = new NavigationRoute(
  // Use the precache handler to serve index.html
  async ({ request }) => {
    const cache = await caches.open('workbox-precache-v2-' + self.registration.scope)
    const cachedResponse = await cache.match('/index.html')
    return cachedResponse || fetch(request)
  },
  {
    denylist: [/^\/api/],
  }
)
registerRoute(navigationHandler)

// ---------------------------------------------------------------------------
// 3. Runtime caching
// ---------------------------------------------------------------------------

// Google Fonts stylesheets (CacheFirst, 1 year)
registerRoute(
  ({ url }) => url.origin === 'https://fonts.googleapis.com',
  new CacheFirst({
    cacheName: 'google-fonts-cache',
    plugins: [
      new ExpirationPlugin({
        maxEntries: 10,
        maxAgeSeconds: 60 * 60 * 24 * 365, // 1 year
      }),
      new CacheableResponsePlugin({
        statuses: [0, 200],
      }),
    ],
  })
)

// Google Fonts webfont files (CacheFirst, 1 year)
registerRoute(
  ({ url }) => url.origin === 'https://fonts.gstatic.com',
  new CacheFirst({
    cacheName: 'gstatic-fonts-cache',
    plugins: [
      new ExpirationPlugin({
        maxEntries: 10,
        maxAgeSeconds: 60 * 60 * 24 * 365, // 1 year
      }),
      new CacheableResponsePlugin({
        statuses: [0, 200],
      }),
    ],
  })
)

// API calls (NetworkFirst, 5 minute cache)
registerRoute(
  ({ url }) => url.pathname.startsWith('/api/'),
  new NetworkFirst({
    cacheName: 'api-cache',
    networkTimeoutSeconds: 10,
    plugins: [
      new ExpirationPlugin({
        maxEntries: 50,
        maxAgeSeconds: 60 * 5, // 5 minutes
      }),
      new CacheableResponsePlugin({
        statuses: [0, 200],
      }),
    ],
  })
)

// ---------------------------------------------------------------------------
// 4. Push notifications
// ---------------------------------------------------------------------------

// Handle incoming push messages from the server.
self.addEventListener('push', (event) => {
  if (!event.data) return

  let data
  try {
    data = event.data.json()
  } catch {
    // If not JSON, show a generic notification
    data = {
      title: 'Kyomi',
      body: event.data.text(),
      icon: '/kyomi_icon_192.png',
    }
  }

  const isReport = data.type === 'watch_report'

  const options = {
    body: data.body || '',
    icon: data.icon || '/kyomi_icon_192.png',
    badge: '/kyomi_icon_192.png',
    // Tag prevents duplicate notifications for the same alert execution
    tag: data.execution_id ? `kyomi-watch-${data.execution_id}` : 'kyomi-notification',
    // Alerts require interaction (stay visible), reports auto-dismiss
    requireInteraction: !isReport,
    // Store the URL to navigate to when the notification is clicked
    data: {
      url: data.url || '/',
    },
  }

  event.waitUntil(self.registration.showNotification(data.title || 'Kyomi', options))
})

// Handle notification click — focus existing Kyomi tab or open a new one.
self.addEventListener('notificationclick', (event) => {
  event.notification.close()

  const targetUrl = event.notification.data?.url || '/'

  event.waitUntil(
    clients.matchAll({ type: 'window', includeUncontrolled: true }).then((clientList) => {
      // Try to find an existing Kyomi tab and focus it
      for (const client of clientList) {
        if (new URL(client.url).origin === self.location.origin) {
          client.focus()
          client.navigate(targetUrl)
          return
        }
      }
      // No existing tab — open a new one
      return clients.openWindow(targetUrl)
    })
  )
})

// Handle push subscription change (browser rotated keys)
self.addEventListener('pushsubscriptionchange', (event) => {
  // Re-subscribe and update the server with the new subscription.
  // The frontend will handle this on next load if this fails.
  event.waitUntil(
    self.registration.pushManager
      .subscribe(event.oldSubscription?.options || { userVisibleOnly: true })
      .then((newSubscription) => {
        return fetch('/api/v1/push/subscribe', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            endpoint: newSubscription.endpoint,
            p256dh: btoa(String.fromCharCode(...new Uint8Array(newSubscription.getKey('p256dh')))),
            auth: btoa(String.fromCharCode(...new Uint8Array(newSubscription.getKey('auth')))),
          }),
        })
      })
      .catch((err) => {
        console.error('Failed to re-subscribe after pushsubscriptionchange:', err)
      })
  )
})
