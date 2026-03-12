// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Feedback Context Collector
 *
 * Passively collects context information for feedback submissions:
 * - Console errors (last 10)
 * - Failed API requests (last 5)
 * - Recent page visits (last 5)
 * - Browser and OS information
 *
 * Usage:
 *   import { feedbackContext } from './lib/feedbackContext';
 *
 *   // Initialize once on app mount
 *   feedbackContext.init();
 *
 *   // Track page visits (call from router)
 *   feedbackContext.addPageVisit('/dashboard');
 *
 *   // Get context for feedback submission
 *   const context = feedbackContext.getContext();
 *
 *   // Clear after submission
 *   feedbackContext.clear();
 */

class FeedbackContextCollector {
  constructor() {
    this.recentPages = [];
    this.consoleErrors = [];
    this.failedRequests = [];
    this.maxItems = { pages: 5, errors: 10, requests: 5 };
    this.initialized = false;
    this.originalConsoleError = null;
  }

  /**
   * Initialize the collector - sets up console error interception.
   * Should be called once when the app mounts.
   */
  init() {
    if (this.initialized) {
      return;
    }

    this.interceptConsoleErrors();
    this.initialized = true;
  }

  /**
   * Intercept console errors to capture them for feedback.
   */
  interceptConsoleErrors() {
    // Store original for restoration if needed
    this.originalConsoleError = console.error;

    const self = this;
    console.error = function(...args) {
      // Capture error context
      self.addConsoleError({
        level: 'error',
        message: args.map((a) => {
          if (a instanceof Error) {
            return `${a.name}: ${a.message}`;
          }
          if (typeof a === 'object') {
            try {
              return JSON.stringify(a);
            } catch {
              return String(a);
            }
          }
          return String(a);
        }).join(' '),
        timestamp: new Date().toISOString(),
      });

      // Call original
      self.originalConsoleError.apply(console, args);
    };
  }

  /**
   * Add a page visit to the recent pages list.
   * @param {string} path - The page path (e.g., '/dashboard')
   */
  addPageVisit(path) {
    // Don't add duplicate consecutive visits
    const lastVisit = this.recentPages[this.recentPages.length - 1];
    if (lastVisit && lastVisit.path === path) {
      return;
    }

    this.recentPages.push({
      path,
      timestamp: new Date().toISOString(),
    });

    if (this.recentPages.length > this.maxItems.pages) {
      this.recentPages.shift();
    }
  }

  /**
   * Add a console error to the errors list.
   * @param {Object} error - Error object with level, message, timestamp
   */
  addConsoleError(error) {
    this.consoleErrors.push(error);

    if (this.consoleErrors.length > this.maxItems.errors) {
      this.consoleErrors.shift();
    }
  }

  /**
   * Add a failed API request to the list.
   * Called from apiClient error interceptor.
   * @param {Object} request - Request info with method, url, status
   */
  addFailedRequest(request) {
    this.failedRequests.push({
      method: request.method?.toUpperCase() || 'UNKNOWN',
      url: request.url || 'unknown',
      status: request.status || 0,
      timestamp: new Date().toISOString(),
    });

    if (this.failedRequests.length > this.maxItems.requests) {
      this.failedRequests.shift();
    }
  }

  /**
   * Get browser information from user agent.
   * @returns {string} Browser name and version
   */
  getBrowserInfo() {
    const ua = navigator.userAgent;

    // Chrome
    const chromeMatch = ua.match(/Chrome\/(\d+(\.\d+)?)/);
    if (chromeMatch && !ua.includes('Edg/')) {
      return `Chrome ${chromeMatch[1]}`;
    }

    // Edge
    const edgeMatch = ua.match(/Edg\/(\d+(\.\d+)?)/);
    if (edgeMatch) {
      return `Edge ${edgeMatch[1]}`;
    }

    // Firefox
    const firefoxMatch = ua.match(/Firefox\/(\d+(\.\d+)?)/);
    if (firefoxMatch) {
      return `Firefox ${firefoxMatch[1]}`;
    }

    // Safari
    const safariMatch = ua.match(/Version\/(\d+(\.\d+)?).+Safari/);
    if (safariMatch) {
      return `Safari ${safariMatch[1]}`;
    }

    return 'Unknown Browser';
  }

  /**
   * Get OS information from user agent.
   * @returns {string} OS name and version
   */
  getOSInfo() {
    const ua = navigator.userAgent;

    // macOS
    if (ua.includes('Mac OS X')) {
      const match = ua.match(/Mac OS X (\d+[._]\d+)/);
      if (match) {
        return `macOS ${match[1].replace('_', '.')}`;
      }
      return 'macOS';
    }

    // Windows
    if (ua.includes('Windows')) {
      if (ua.includes('Windows NT 10.0')) {
        return 'Windows 10/11';
      }
      if (ua.includes('Windows NT 6.3')) {
        return 'Windows 8.1';
      }
      if (ua.includes('Windows NT 6.2')) {
        return 'Windows 8';
      }
      return 'Windows';
    }

    // Linux
    if (ua.includes('Linux')) {
      if (ua.includes('Android')) {
        const match = ua.match(/Android (\d+(\.\d+)?)/);
        if (match) {
          return `Android ${match[1]}`;
        }
        return 'Android';
      }
      return 'Linux';
    }

    // iOS
    if (ua.includes('iPhone') || ua.includes('iPad')) {
      const match = ua.match(/OS (\d+_\d+)/);
      if (match) {
        return `iOS ${match[1].replace('_', '.')}`;
      }
      return 'iOS';
    }

    return 'Unknown OS';
  }

  /**
   * Get the complete context object for feedback submission.
   * @returns {Object} Context object with current state and recent activity
   */
  getContext() {
    return {
      url: window.location.pathname,
      browser: this.getBrowserInfo(),
      os: this.getOSInfo(),
      screen_size: `${window.innerWidth}x${window.innerHeight}`,
      recent_pages: [...this.recentPages],
      console_errors: [...this.consoleErrors],
      failed_requests: [...this.failedRequests],
    };
  }

  /**
   * Clear collected errors and failed requests after feedback submission.
   * Page history is preserved as it may be useful for subsequent reports.
   */
  clear() {
    this.consoleErrors = [];
    this.failedRequests = [];
    // Note: Don't clear recentPages - still useful for subsequent reports
  }

  /**
   * Reset all collected data including page history.
   * Used primarily for testing.
   */
  reset() {
    this.recentPages = [];
    this.consoleErrors = [];
    this.failedRequests = [];
  }
}

// Export singleton instance
export const feedbackContext = new FeedbackContextCollector();
