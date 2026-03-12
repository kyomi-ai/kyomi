// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * API Client with Automatic Token Refresh
 *
 * ⚠️ CRITICAL: This is the ONLY way to make authenticated API calls in this application.
 *
 * Features:
 * - Automatic access token refresh on 401 responses
 * - Request queuing during token refresh
 * - Proper error handling
 * - Integration with dual-token auth service
 *
 * ==================================================================================
 * 🚨 IMPORTANT FOR ALL DEVELOPERS 🚨
 * ==================================================================================
 *
 * ALWAYS use this apiClient singleton for ALL backend API calls at /api/v1/*
 *
 * ✅ CORRECT:
 *   import apiClient from '../api/apiClient.js';
 *   const response = await apiClient.get('/api/v1/endpoint');
 *
 * ❌ WRONG - Bypasses token refresh:
 *   const response = await fetch('/api/v1/endpoint');
 *
 * ❌ WRONG - Bypasses interceptors:
 *   import axios from 'axios';
 *   const response = await axios.get('/api/v1/endpoint');
 *
 * WHY THIS MATTERS:
 * - Access tokens expire after 15 minutes
 * - This client automatically refreshes tokens and retries failed requests
 * - Without it, users get 401 errors and are logged out unexpectedly
 *
 * See frontend/API_GUIDELINES.md for complete documentation.
 * ==================================================================================
 */

import axios from 'axios';
import { API_CONFIG } from '../config/api.js';
import authService from '../services/authService.js';
import { feedbackContext } from '../lib/feedbackContext.js';

class APIClient {
  constructor(baseURL = API_CONFIG.baseURL) {
    this.isRefreshing = false;
    this.failedQueue = [];
    
    this.client = axios.create({
      baseURL,
      headers: {
        'Content-Type': 'application/json',
      },
      withCredentials: true, // Include HTTPOnly cookies in requests
    });

    this.setupInterceptors();
  }

  setupInterceptors() {
    // Request interceptor - add credits exhausted header for backend protection
    this.client.interceptors.request.use(
      async (config) => {
        // HTTPOnly cookies are automatically included via withCredentials: true
        // No need to manually add Authorization headers

        return config;
      },
      (error) => {
        return Promise.reject(error);
      }
    );

    // Response interceptor for automatic token refresh
    this.client.interceptors.response.use(
      (response) => response, // Pass through successful responses
      async (error) => {
        const originalRequest = error.config;

        // Check if it's a 401 error and we haven't already tried to refresh
        if (error.response?.status === 401 && !originalRequest._retry) {
          if (this.isRefreshing) {
            // If we're already refreshing, queue this request
            return new Promise((resolve, reject) => {
              this.failedQueue.push({ resolve, reject });
            }).then(() => {
              // No token needed - cookies are handled automatically
              return this.client(originalRequest);
            }).catch(err => {
              return Promise.reject(err);
            });
          }

          originalRequest._retry = true;
          this.isRefreshing = true;

          try {
            // Attempt to refresh tokens via the server endpoint
            const refreshResult = await authService.refreshTokens();
            
            if (refreshResult.success) {
              // No need to extract token - HTTPOnly cookies updated by server
              
              // Process queued requests (no token needed)
              this.processQueue(null, null);
              
              // Retry original request (cookies updated automatically)
              return this.client(originalRequest);
            } else {
              throw new Error('Token refresh failed - no access token received');
            }
          } catch (refreshError) {
            
            // Refresh failed, process queue with error and redirect to login
            this.processQueue(refreshError, null);
            
            // Clear auth state and redirect to login
            authService.clearTokens();
            
            // Notify the app that authentication failed
            // This will trigger AuthContext to update its state
            window.dispatchEvent(new CustomEvent('auth-failed', {
              detail: { reason: 'token_refresh_failed', message: 'Session expired' }
            }));
            
            // Only redirect if we're not already on the login page
            if (window.location.pathname !== '/login') {
              window.location.href = '/login';
            }
            
            return Promise.reject(refreshError);
          } finally {
            this.isRefreshing = false;
          }
        }

        // Fallback: If any 401 error makes it this far without being handled,
        // check if it's an OAuth token revoked error before logging out
        if (error.response?.status === 401) {
          const errorDetail = error.response?.data?.detail;

          // Check if this is an OAuth token revoked error
          if (errorDetail && typeof errorDetail === 'object' && errorDetail.error === 'oauth_token_revoked') {
            // Don't dispatch auth-failed - let the calling code handle the reconnect flow
            return Promise.reject(error);
          }

          // For all other 401 errors, log the user out
          window.dispatchEvent(new CustomEvent('auth-failed', {
            detail: { reason: 'unhandled_401', message: 'Authentication failed' }
          }));
        }

        // Track failed request for feedback context
        if (error.response) {
          feedbackContext.addFailedRequest({
            method: error.config?.method,
            url: error.config?.url,
            status: error.response.status,
          });
        }

        return Promise.reject(error);
      }
    );
  }

  /**
   * Process queued requests after token refresh
   */
  processQueue(error) {
    this.failedQueue.forEach(({ resolve, reject }) => {
      if (error) {
        reject(error);
      } else {
        resolve(); // No token needed with HTTPOnly cookies
      }
    });

    this.failedQueue = [];
  }

  // Health check
  async healthCheck() {
    const response = await this.client.get('/health');
    return response.data;
  }

  // Authentication methods
  async login(email, password) {
    // Use authService directly for login (doesn't need token)
    return authService.login(email, password);
  }

  async logout() {
    return authService.logout();
  }

  async logoutAll() {
    return authService.logoutAll();
  }

  async getSessions() {
    return authService.getSessions();
  }

  async getCurrentUser() {
    const response = await this.client.get('/api/v1/auth/profile');
    return response.data;
  }

  // User management
  async createUser(userData) {
    const response = await this.client.post('/api/v1/users/', userData);
    return response.data;
  }

  async getUsers() {
    const response = await this.client.get('/api/v1/users/');
    return response.data;
  }

  async updateUser(userId, userData) {
    const response = await this.client.put(`/api/v1/users/${userId}`, userData);
    return response.data;
  }

  async deleteUser(userId) {
    const response = await this.client.delete(`/api/v1/users/${userId}`);
    return response.data;
  }

  // Token management
  async createToken(tokenData) {
    const response = await this.client.post('/api/v1/users/tokens', tokenData);
    return response.data;
  }

  async getUserTokens(userEmail) {
    const response = await this.client.get(`/api/v1/users/tokens/${encodeURIComponent(userEmail)}`);
    return response.data;
  }

  // BigQuery operations
  async executeQuery(queryRequest) {
    const response = await this.client.post('/api/v1/bigquery/query', queryRequest);
    return response.data;
  }

  async searchTables(searchRequest) {
    const response = await this.client.post('/api/v1/bigquery/search', searchRequest);
    return response.data;
  }

  async getTableInfo(tableRequest) {
    const response = await this.client.post('/api/v1/bigquery/info', tableRequest);
    return response.data;
  }

  async getBigQueryCatalog() {
    const response = await this.client.get('/api/v1/bigquery/catalog');
    return response.data;
  }

  async sampleTable(sampleRequest) {
    const response = await this.client.post('/api/v1/bigquery/sample', sampleRequest);
    return response.data;
  }

  async refreshCatalog(datasourceSlug, refreshRequest = {}) {
    const response = await this.client.post(`/api/v1/datasources/${datasourceSlug}/catalog/refresh`, refreshRequest);
    return response.data;
  }

  // Hints management
  async createHint(hintData) {
    const response = await this.client.post('/api/v1/hints', hintData);
    return response.data;
  }

  async getHints(filters = {}) {
    const response = await this.client.get('/api/v1/hints', { params: filters });
    return response.data;
  }

  // 2FA operations
  async get2FAStatus() {
    const response = await this.client.get('/api/v1/auth/2fa/status');
    return response.data;
  }

  async setup2FA() {
    const response = await this.client.post('/api/v1/auth/2fa/setup');
    return response.data;
  }

  async enable2FA(verificationCode) {
    const response = await this.client.post('/api/v1/auth/2fa/enable', {
      verification_code: verificationCode
    });
    return response.data;
  }

  async disable2FA() {
    const response = await this.client.post('/api/v1/auth/2fa/disable');
    return response.data;
  }

  async verify2FA(verificationCode) {
    const response = await this.client.post('/api/v1/auth/2fa/verify', {
      verification_code: verificationCode
    });
    return response.data;
  }

  // Passkey operations
  async startPasskeyRegistration(email, deviceName) {
    const response = await this.client.post(API_CONFIG.endpoints.passkeys.registerStart, {
      email,
      device_name: deviceName
    });
    return response.data;
  }

  async completePasskeyRegistration(credential, challengeId, deviceName) {
    const response = await this.client.post(`${API_CONFIG.endpoints.passkeys.registerComplete}?challenge_id=${challengeId}`, {
      credential,
      device_name: deviceName
    });
    return response.data;
  }

  async startPasskeyLogin(email) {
    const response = await this.client.post('/api/v1/auth/passkeys/login/start', {
      email
    });
    return response.data;
  }

  async completePasskeyLogin(credential, challengeId) {
    const response = await this.client.post(`/api/v1/auth/passkeys/login/complete?challenge_id=${challengeId}`, {
      credential
    });
    return response.data;
  }

  async listPasskeys() {
    const response = await this.client.get('/api/v1/auth/passkeys/list');
    return response.data;
  }

  // Chat API methods
  async getChatStatus() {
    const response = await this.client.get('/api/v1/chat/status');
    return response.data;
  }

  async getAvailableModels() {
    const response = await this.client.get('/api/v1/chat/models');
    return response.data;
  }

  async sendMessage(message, sessionId = null) {
    const payload = { message };
    if (sessionId) {
      payload.session_id = sessionId;
    }
    const response = await this.client.post('/api/v1/chat/message', payload);
    return response.data;
  }

  async sendMessageStream(message, sessionId = null, onChunk = null, onComplete = null, onError = null, onStart = null, provider = 'claude', model_name = null) {
    try {
      const payload = { message };
      if (sessionId) {
        payload.session_id = sessionId;
      }
      if (provider) {
        payload.provider = provider;
      }
      if (model_name) {
        payload.model_name = model_name;
      }

      const response = await fetch('/api/v1/chat/message/stream', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include',  // Include HTTPOnly cookies
        body: JSON.stringify(payload)
      });


      if (!response.ok) {
        if (response.status === 401) {
          try {
            // Attempt to refresh tokens
            const refreshResult = await authService.refreshTokens();
            if (refreshResult.success) {
              // Retry the request with refreshed tokens
              return this.sendMessageStream(message, sessionId, onChunk, onComplete, onError, onStart, provider, model_name);
            } else {
              throw new Error('Token refresh failed');
            }
          } catch (refreshError) {
            authService.clearTokens();
            window.dispatchEvent(new CustomEvent('auth-failed', {
              detail: { reason: 'token_refresh_failed', message: 'Session expired' }
            }));
            if (window.location.pathname !== '/login') {
              window.location.href = '/login';
            }
            throw refreshError;
          }
        }
        const errorText = await response.text();
        throw new Error(`HTTP error! status: ${response.status} - ${errorText}`);
      }

      const reader = response.body.getReader();
      const decoder = new TextDecoder();

      while (true) {
        const { done, value } = await reader.read();
        if (done) {
          break;
        }

        const chunk = decoder.decode(value);
        const lines = chunk.split('\n');

        for (const line of lines) {
          if (line.startsWith('data: ')) {
            try {
              const jsonStr = line.slice(6);
              const data = JSON.parse(jsonStr);
              
              if (data.type === 'start') {
                if (onStart) onStart(data);
                continue;
              } else if (data.type === 'chunk') {
                if (onChunk) onChunk(data.content);
              } else if (data.type === 'complete') {
                if (onComplete) onComplete(data);
                return data;
              } else if (data.type === 'error') {
                if (onError) onError(new Error(data.content));
                return { error: data.content };
              }
            } catch {
              continue;
            }
          }
        }
      }
    } catch (error) {
      if (onError) onError(error);
      throw error;
    }
  }

  async searchChatMessages(query) {
    const response = await this.client.get('/api/v1/chat/search', {
      params: { query }
    });
    return response.data;
  }

  async getChatSessions(pinnedOnly = false) {
    const response = await this.client.get('/api/v1/chat/sessions', {
      params: { pinned_only: pinnedOnly }
    });
    return response.data;
  }

  async getSessionMessages(sessionId) {
    const response = await this.client.get(`/api/v1/chat/sessions/${sessionId}/messages`);
    return response.data;
  }

  async deleteSession(sessionId) {
    const response = await this.client.delete(`/api/v1/chat/sessions/${sessionId}`);
    return response.data;
  }

  async bulkDeleteSessions(sessionIds) {
    const response = await this.client.post('/api/v1/chat/sessions/bulk-delete', {
      session_ids: sessionIds,
    });
    return response.data;
  }

  async updateSessionTitle(sessionId, title) {
    const response = await this.client.put(`/api/v1/chat/sessions/${sessionId}`, { title });
    return response.data;
  }

  async startNewSession() {
    const response = await this.client.post('/api/v1/chat/sessions');
    return response.data;
  }

  async shareSession(sessionId) {
    const response = await this.client.post(`/api/v1/chat/sessions/${sessionId}/share`);
    return response.data;
  }

  async unshareSession(sessionId) {
    const response = await this.client.post(`/api/v1/chat/sessions/${sessionId}/unshare`);
    return response.data;
  }

  async markSessionRead(sessionId, lastMessageId = null) {
    const response = await this.client.post(`/api/v1/chat/sessions/${sessionId}/read`,
      lastMessageId ? { last_message_id: lastMessageId } : {}
    );
    return response.data;
  }

  async sendMessageWebSocket(message, sessionId = null, timeContext = null, skipAi = false, clientMsgId = null) {
    /**
     * Send message via WebSocket endpoint - content is delivered via WebSocket in real-time
     * Returns session metadata, while actual response content comes through WebSocket
     * Model selection is handled by backend from workspace settings
     *
     * @param message - The user's message text
     * @param sessionId - Optional existing session ID
     * @param timeContext - Optional { current_time_user_tz }
     * @param skipAi - If true, saves message but doesn't generate AI response (for shared conversation comments)
     * @param clientMsgId - Client-generated message ID for deduplication
     */
    try {
      const payload = { message };
      if (sessionId) {
        payload.session_id = sessionId;
      }
      if (timeContext) {
        if (timeContext.current_time_user_tz) {
          payload.current_time_user_tz = timeContext.current_time_user_tz;
        }
      }
      if (skipAi) {
        payload.skip_ai = true;
      }
      if (clientMsgId) {
        payload.client_msg_id = clientMsgId;
      }

      const response = await this.client.post('/api/v1/chat/message/websocket', payload);
      return response.data;
    } catch (error) {
      throw error;
    }
  }

  // Dashboard methods
  async createDashboard(title, content) {
    const response = await this.client.post('/api/v1/dashboards', {
      title,
      content
    });
    return response.data;
  }

  async listDashboards() {
    const response = await this.client.get('/api/v1/dashboards');
    return response.data;
  }

  async getDashboard(dashboardId) {
    const response = await this.client.get(`/api/v1/dashboards/${dashboardId}`);
    return response.data;
  }

  async updateDashboard(dashboardId, updates) {
    const response = await this.client.patch(`/api/v1/dashboards/${dashboardId}`, updates);
    return response.data;
  }

  async deleteDashboard(dashboardId) {
    const response = await this.client.delete(`/api/v1/dashboards/${dashboardId}`);
    return response.data;
  }

  // Generic HTTP methods for direct API access
  async get(url, config = {}) {
    const response = await this.client.get(url, config);
    return response;
  }

  async post(url, data = {}, config = {}) {
    const response = await this.client.post(url, data, config);
    return response;
  }

  async put(url, data = {}, config = {}) {
    const response = await this.client.put(url, data, config);
    return response;
  }

  async delete(url, config = {}) {
    const response = await this.client.delete(url, config);
    return response;
  }

  async patch(url, data = {}, config = {}) {
    const response = await this.client.patch(url, data, config);
    return response;
  }
}

// Export singleton instance
const apiClient = new APIClient();
export default apiClient;