// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Kyomi Analytics tracking utility
 *
 * Uses window.kyomi (from k.js) for event tracking.
 *
 * Usage:
 * import { trackEvent } from '../utils/analytics';
 * trackEvent('event_name', { props: { key: 'value' } });
 */

/**
 * Track a custom event via Kyomi Analytics.
 *
 * @param {string} eventName - The name of the event to track
 * @param {Object} options - Optional event properties
 * @param {Object} options.props - Custom properties to attach to the event
 */
export function trackEvent(eventName, options = {}) {
  if (typeof window !== 'undefined' && window.kyomi) {
    try {
      window.kyomi.track(eventName, options.props || undefined);
    } catch (error) {
    }
  }
}

/**
 * Track a pageview via Kyomi Analytics.
 * Note: k.js automatically tracks pageviews including SPA navigation,
 * so this is only needed for manual overrides.
 */
export function trackPageview() {
  if (typeof window !== 'undefined' && window.kyomi) {
    try {
      window.kyomi.track('pageview');
    } catch (error) {
    }
  }
}
