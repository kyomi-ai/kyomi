// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Toast Notification Utilities
 *
 * Wrapper around Sonner toast library for consistent usage across the application.
 *
 * Usage:
 * ```javascript
 * import { toast } from '../lib/toast';
 *
 * // Success messages
 * toast.success('Settings saved successfully!');
 *
 * // Error messages
 * toast.error('Failed to save settings');
 *
 * // Warning messages
 * toast.warning('This action cannot be undone');
 *
 * // Info messages
 * toast.info('New version available');
 *
 * // Loading state
 * const toastId = toast.loading('Saving...');
 * // Later, update it:
 * toast.success('Saved!', { id: toastId });
 * ```
 */

import { toast as sonnerToast } from 'sonner';

export const toast = {
  /**
   * Show a success toast notification
   * @param {string} message - The message to display
   * @param {Object} options - Additional options (duration, etc.)
   */
  success: (message, options = {}) => {
    return sonnerToast.success(message, {
      duration: 4000,
      ...options
    });
  },

  /**
   * Show an error toast notification
   * @param {string} message - The message to display
   * @param {Object} options - Additional options (duration, etc.)
   */
  error: (message, options = {}) => {
    return sonnerToast.error(message, {
      duration: 5000, // Errors stay a bit longer
      ...options
    });
  },

  /**
   * Show a warning toast notification
   * @param {string} message - The message to display
   * @param {Object} options - Additional options (duration, etc.)
   */
  warning: (message, options = {}) => {
    return sonnerToast.warning(message, {
      duration: 4000,
      ...options
    });
  },

  /**
   * Show an info toast notification
   * @param {string} message - The message to display
   * @param {Object} options - Additional options (duration, etc.)
   */
  info: (message, options = {}) => {
    return sonnerToast.info(message, {
      duration: 4000,
      ...options
    });
  },

  /**
   * Show a loading toast notification
   * Useful for async operations
   * @param {string} message - The message to display
   * @param {Object} options - Additional options
   * @returns {string|number} Toast ID that can be used to update the toast later
   */
  loading: (message, options = {}) => {
    return sonnerToast.loading(message, options);
  },

  /**
   * Dismiss a specific toast by ID
   * @param {string|number} toastId - The ID of the toast to dismiss
   */
  dismiss: (toastId) => {
    return sonnerToast.dismiss(toastId);
  },

  /**
   * Promise-based toast that automatically shows success/error based on promise resolution
   * @param {Promise} promise - The promise to track
   * @param {Object} messages - Object with loading, success, and error messages
   * @example
   * toast.promise(saveData(), {
   *   loading: 'Saving...',
   *   success: 'Saved successfully!',
   *   error: 'Failed to save'
   * });
   */
  promise: (promise, messages) => {
    return sonnerToast.promise(promise, messages);
  }
};

export default toast;
