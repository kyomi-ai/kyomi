// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Formatting Utilities
 *
 * Shared utility functions for formatting data across the application.
 */

/**
 * Format timestamp as relative time (e.g., "2 minutes ago", "just now")
 * For older messages, shows actual dates using browser's locale preferences
 * (e.g., "Yesterday", "Mon 3:45 PM", "Jan 15")
 *
 * @param {number|string} timestamp - Unix timestamp in milliseconds or ISO string
 * @returns {string} Formatted relative time string respecting browser locale
 *
 * @example
 * formatRelativeTime(Date.now() - 120000) // "2 minutes ago"
 * formatRelativeTime(Date.now() - 30) // "just now"
 * formatRelativeTime('2024-01-15T10:30:00Z') // "Jan 15" or "15 Jan" depending on locale
 */
export function formatRelativeTime(timestamp) {
  // Convert ISO string to timestamp if needed
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

  // For yesterday's messages - show "yesterday" with time
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (date.toDateString() === yesterday.toDateString()) {
    return `yesterday ${date.toLocaleTimeString(undefined, {
      hour: 'numeric',
      minute: '2-digit'
    })}`;
  }

  // For messages within the last week: show day name (e.g., "Mon 3:45 PM" or "Mo 15:45")
  if (days < 7) {
    return date.toLocaleDateString(undefined, {
      weekday: 'short',
      hour: 'numeric',
      minute: '2-digit'
    });
  }

  // For older messages: show date (e.g., "Jan 15" or "15 Jan" depending on locale)
  return date.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric'
  });
}

/**
 * Format timestamp as compact relative time (e.g., "5m", "3h", "2d")
 * Used for space-constrained UI elements like narrow chart headers
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

/**
 * Format number with thousands separators
 *
 * @param {number} num - Number to format
 * @returns {string} Formatted number string (e.g., "1,234,567")
 *
 * @example
 * formatNumber(1234567) // "1,234,567"
 * formatNumber(42) // "42"
 */
export function formatNumber(num) {
  return num.toLocaleString();
}

/**
 * Format bytes to human-readable string
 *
 * @param {number} bytes - Number of bytes
 * @param {number} decimals - Number of decimal places (default: 2)
 * @returns {string} Formatted byte string (e.g., "1.5 MB")
 *
 * @example
 * formatBytes(1536) // "1.50 KB"
 * formatBytes(1048576) // "1.00 MB"
 */
export function formatBytes(bytes, decimals = 2) {
  if (bytes === 0) return '0 Bytes';

  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ['Bytes', 'KB', 'MB', 'GB', 'TB'];

  const i = Math.floor(Math.log(bytes) / Math.log(k));

  return parseFloat((bytes / Math.pow(k, i)).toFixed(dm)) + ' ' + sizes[i];
}
