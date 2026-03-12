// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useCallback } from 'react';
import apiClient from '../api/apiClient';

/**
 * usePushNotifications — React hook for managing Web Push notification subscriptions.
 *
 * Returns:
 * - supported: Whether the browser supports push notifications
 * - permission: Current Notification permission ('default' | 'granted' | 'denied')
 * - isSubscribed: Whether the current browser is subscribed
 * - loading: Whether a subscribe/unsubscribe operation is in progress
 * - error: Error message if the last operation failed
 * - subscribe: Function to request permission and subscribe
 * - unsubscribe: Function to unsubscribe the current browser
 * - subscriptions: List of all user's device subscriptions (from server)
 * - refreshSubscriptions: Function to refresh the subscriptions list
 * - deleteSubscription: Function to delete a subscription by ID
 */
export default function usePushNotifications() {
  const [supported, setSupported] = useState(false);
  const [permission, setPermission] = useState('default');
  const [isSubscribed, setIsSubscribed] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);
  const [subscriptions, setSubscriptions] = useState([]);
  const [checkingStatus, setCheckingStatus] = useState(true);

  // Check browser support and current subscription status on mount
  useEffect(() => {
    const checkStatus = async () => {
      // Check browser support
      const pushSupported =
        'PushManager' in window &&
        'Notification' in window &&
        'serviceWorker' in navigator;

      setSupported(pushSupported);

      if (!pushSupported) {
        setCheckingStatus(false);
        return;
      }

      setPermission(Notification.permission);

      // Check if there's an existing push subscription on this browser
      try {
        const registration = await navigator.serviceWorker.ready;
        const subscription = await registration.pushManager.getSubscription();
        setIsSubscribed(!!subscription);
      } catch {
        // Service worker not ready yet — not subscribed
        setIsSubscribed(false);
      }

      setCheckingStatus(false);
    };

    checkStatus();
  }, []);

  // Fetch all subscriptions from the server (for the settings device list)
  const refreshSubscriptions = useCallback(async () => {
    try {
      const response = await apiClient.get('/api/v1/push/subscriptions');
      setSubscriptions(response.data.subscriptions || []);
    } catch {
      // Silently fail — subscriptions list is not critical
      setSubscriptions([]);
    }
  }, []);

  // Subscribe the current browser to push notifications
  const subscribe = useCallback(async () => {
    setLoading(true);
    setError(null);

    try {
      // 1. Request notification permission
      const perm = await Notification.requestPermission();
      setPermission(perm);

      if (perm !== 'granted') {
        setError('Notification permission denied');
        setLoading(false);
        return;
      }

      // 2. Get the VAPID public key from the server
      const vapidResponse = await apiClient.get('/api/v1/push/vapid-key');
      const vapidPublicKey = vapidResponse.data.public_key;

      // 3. Convert base64url VAPID key to Uint8Array for pushManager.subscribe()
      const applicationServerKey = urlBase64ToUint8Array(vapidPublicKey);

      // 4. Subscribe to push via the browser's PushManager
      const registration = await navigator.serviceWorker.ready;
      const pushSubscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey,
      });

      // 5. Extract keys and send to our backend
      const p256dh = arrayBufferToBase64url(pushSubscription.getKey('p256dh'));
      const auth = arrayBufferToBase64url(pushSubscription.getKey('auth'));

      // Detect device label from user agent
      const deviceLabel = detectDeviceLabel();

      await apiClient.post('/api/v1/push/subscribe', {
        endpoint: pushSubscription.endpoint,
        p256dh,
        auth,
        user_agent: navigator.userAgent,
        device_label: deviceLabel,
      });

      setIsSubscribed(true);
      await refreshSubscriptions();
    } catch (err) {
      const message = err.response?.data?.detail || err.message || 'Failed to subscribe';
      setError(message);
    } finally {
      setLoading(false);
    }
  }, [refreshSubscriptions]);

  // Unsubscribe the current browser
  const unsubscribe = useCallback(async () => {
    setLoading(true);
    setError(null);

    try {
      const registration = await navigator.serviceWorker.ready;
      const subscription = await registration.pushManager.getSubscription();

      if (subscription) {
        // Tell the server to remove this subscription
        await apiClient.post('/api/v1/push/unsubscribe', {
          endpoint: subscription.endpoint,
        });

        // Unsubscribe locally
        await subscription.unsubscribe();
      }

      setIsSubscribed(false);
      await refreshSubscriptions();
    } catch (err) {
      const message = err.response?.data?.detail || err.message || 'Failed to unsubscribe';
      setError(message);
    } finally {
      setLoading(false);
    }
  }, [refreshSubscriptions]);

  // Delete a subscription by ID (for removing other devices from settings)
  const deleteSubscription = useCallback(
    async (id) => {
      try {
        await apiClient.delete(`/api/v1/push/subscriptions/${id}`);
        await refreshSubscriptions();
      } catch (err) {
        const message = err.response?.data?.detail || err.message || 'Failed to delete';
        setError(message);
      }
    },
    [refreshSubscriptions]
  );

  return {
    supported,
    permission,
    isSubscribed,
    loading: loading || checkingStatus,
    error,
    subscribe,
    unsubscribe,
    subscriptions,
    refreshSubscriptions,
    deleteSubscription,
  };
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Convert a base64url string to a Uint8Array for use with pushManager.subscribe().
 * Web Push requires the applicationServerKey in this format.
 */
function urlBase64ToUint8Array(base64String) {
  const padding = '='.repeat((4 - (base64String.length % 4)) % 4);
  const base64 = (base64String + padding).replace(/-/g, '+').replace(/_/g, '/');
  const rawData = atob(base64);
  const outputArray = new Uint8Array(rawData.length);
  for (let i = 0; i < rawData.length; i++) {
    outputArray[i] = rawData.charCodeAt(i);
  }
  return outputArray;
}

/**
 * Convert an ArrayBuffer to a base64url string (no padding).
 */
function arrayBufferToBase64url(buffer) {
  const bytes = new Uint8Array(buffer);
  let binary = '';
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/**
 * Generate a user-friendly device label from the User-Agent string.
 */
function detectDeviceLabel() {
  const ua = navigator.userAgent;

  // Detect browser
  let browser = 'Browser';
  if (ua.includes('Firefox')) browser = 'Firefox';
  else if (ua.includes('Edg/')) browser = 'Edge';
  else if (ua.includes('Chrome')) browser = 'Chrome';
  else if (ua.includes('Safari')) browser = 'Safari';

  // Detect OS
  let os = '';
  if (ua.includes('Mac OS')) os = 'macOS';
  else if (ua.includes('Windows')) os = 'Windows';
  else if (ua.includes('Linux')) os = 'Linux';
  else if (ua.includes('Android')) os = 'Android';
  else if (ua.includes('iPhone') || ua.includes('iPad')) os = 'iOS';

  return os ? `${browser} on ${os}` : browser;
}
