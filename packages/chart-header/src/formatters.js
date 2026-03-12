// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Format timestamp as relative time (e.g., "2 minutes ago", "just now")
 * For older messages, shows actual dates using browser's locale preferences
 *
 * @param {number|string} timestamp - Unix timestamp in milliseconds or ISO string
 * @returns {string} Formatted relative time string respecting browser locale
 */
export function formatRelativeTime(timestamp) {
  const ts = typeof timestamp === 'string' ? new Date(timestamp).getTime() : timestamp;
  const date = new Date(ts);

  const now = Date.now();
  const diff = now - ts;
  const seconds = Math.floor(diff / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);

  if (seconds < 10) return 'just now';
  if (seconds < 60) return `${seconds} seconds ago`;
  if (minutes === 1) return '1 minute ago';
  if (minutes < 60) return `${minutes} minutes ago`;
  if (hours === 1) return '1 hour ago';
  if (hours < 24) return `${hours} hours ago`;

  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (date.toDateString() === yesterday.toDateString()) {
    return `yesterday ${date.toLocaleTimeString(undefined, {
      hour: 'numeric',
      minute: '2-digit'
    })}`;
  }

  if (days < 7) {
    return date.toLocaleDateString(undefined, {
      weekday: 'short',
      hour: 'numeric',
      minute: '2-digit'
    });
  }

  return date.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric'
  });
}

/**
 * Format timestamp as compact relative time (e.g., "5m", "3h", "2d")
 *
 * @param {number|string} timestamp - Unix timestamp in milliseconds or ISO string
 * @returns {string} Compact formatted relative time string
 */
export function formatRelativeTimeCompact(timestamp) {
  const ts = typeof timestamp === 'string' ? new Date(timestamp).getTime() : timestamp;
  const diff = Date.now() - ts;
  const seconds = Math.floor(diff / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);

  if (seconds < 60) return 'now';
  if (minutes < 60) return `${minutes}m`;
  if (hours < 24) return `${hours}h`;
  if (days < 7) return `${days}d`;

  return new Date(ts).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}
