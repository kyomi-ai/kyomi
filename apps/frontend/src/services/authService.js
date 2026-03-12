// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Enhanced Authentication Service with Dual-Token Support
 * 
 * Features:
 * - Access tokens (short-lived, in memory)
 * - Refresh tokens (long-lived, HTTPOnly cookies)
 * - Automatic token refresh
 * - Session management
 * - No backdoors or security compromises
 */

import { API_CONFIG } from '../config/api.js';

class AuthService {
  constructor() {
    this.accessToken = null;
    this.user = null;
    this.refreshPromise = null; // Prevent multiple simultaneous refresh attempts
    
    // Initialize from storage (refresh token is now in HTTPOnly cookie)
    this.loadTokensFromStorage();
  }

  /**
   * Load tokens from secure storage on initialization
   */
  loadTokensFromStorage() {
    try {
      // Both access and refresh tokens are now in HTTPOnly cookies
      // We only store user info in localStorage (no sensitive data)
      const storedUser = localStorage.getItem('user');
      // Check if storedUser is not null, not undefined string, and not empty
      if (storedUser && storedUser !== 'undefined' && storedUser !== 'null') {
        this.user = JSON.parse(storedUser);
      } else {
        // Clear invalid values
        this.user = null;
        if (storedUser) {
          localStorage.removeItem('user');
        }
      }

      // Access token will be provided via HTTPOnly cookies automatically
      // We don't store or retrieve it from localStorage anymore
    } catch (error) {
      // Don't clear HTTPOnly cookies on localStorage parse error
      // The cookies are the source of truth - let initializeAuth() verify them
      this.user = null;
      localStorage.removeItem('user');
    }
  }

  /**
   * Store user data securely (tokens are in HTTPOnly cookies)
   */
  storeUserData(user) {
    try {
      this.user = user;
      
      // Only store user info in localStorage (tokens are HTTPOnly cookies)
      localStorage.setItem('user', JSON.stringify(user));
      
    } catch (error) {
      throw new Error('User data storage failed');
    }
  }

  /**
   * Clear all tokens and user data
   */
  clearTokens() {
    this.user = null;
    this.refreshPromise = null;

    // Clear only user data from localStorage (tokens are in HTTPOnly cookies)
    localStorage.removeItem('user');

    // Clear HTTPOnly cookies by making a logout call to backend
    // This is needed when tokens are invalid/expired
    this._clearHttpOnlyCookies();

  }

  /**
   * Internal method to clear HTTPOnly cookies via backend call
   */
  async _clearHttpOnlyCookies() {
    try {
      // Call backend logout to clear HTTPOnly cookies - don't await to avoid blocking
      fetch(`${API_CONFIG.baseURL}/api/v1/auth/logout`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
      }).catch(() => {
        // Silently ignore errors - this is just cleanup
      });
    } catch {
      // Silently ignore errors - this is just cleanup
    }
  }

  /**
   * Login with email and password, optionally with TOTP code for 2FA
   */
  async login(email, password, totpCode = null) {
    try {
      const loginData = { email, password };
      if (totpCode) {
        loginData.totp_code = totpCode;
      }

      const response = await fetch(`${API_CONFIG.baseURL}/api/v1/auth/login`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
        body: JSON.stringify(loginData),
      });

      if (!response.ok) {
        // Check for 403 with verification required header
        if (response.status === 403 && response.headers.get('x-verification-required')) {
          let errorData;
          try {
            errorData = await response.json();
          } catch {
            errorData = { detail: 'Please verify your email before signing in.' };
          }
          return {
            success: false,
            error: errorData.detail || 'Please verify your email before signing in.',
            verificationRequired: true
          };
        }

        let errorData;
        try {
          errorData = await response.json();
        } catch (parseError) {
          throw new Error(`Login failed: ${response.status} ${response.statusText}`);
        }

        // Extract error message with comprehensive handling for all edge cases
        let errorMessage;
        
        if (typeof errorData === 'string') {
          errorMessage = errorData;
        } else if (errorData && typeof errorData === 'object') {
          // Handle object responses - check all possible error fields
          if (errorData.detail) {
            errorMessage = errorData.detail;
          } else if (errorData.message) {
            errorMessage = errorData.message;
          } else if (errorData.error) {
            // Handle nested error objects
            if (typeof errorData.error === 'string') {
              errorMessage = errorData.error;
            } else if (errorData.error.message) {
              errorMessage = errorData.error.message;
            } else if (errorData.error.detail) {
              errorMessage = errorData.error.detail;
            } else {
              errorMessage = JSON.stringify(errorData.error);
            }
          } else if (errorData.errors && Array.isArray(errorData.errors)) {
            // Handle validation errors array nested in errors property
            errorMessage = errorData.errors.map(err => err.msg || err.message || err.detail || JSON.stringify(err)).join(', ');
          } else if (errorData.msg) {
            errorMessage = errorData.msg;
          } else if (errorData.description) {
            errorMessage = errorData.description;
          } else {
            // Last resort: try to extract any meaningful text
            const keys = Object.keys(errorData);
            errorMessage = `Login failed: ${response.status} (${keys.join(', ')}: ${JSON.stringify(errorData)})`;
          }
        } else {
          errorMessage = `Login failed: ${response.status}`;
        }
        
        
        // Prevent [object Object] errors by ensuring we always return a string
        if (typeof errorMessage !== 'string') {
          errorMessage = String(errorMessage);
        }
        
        throw new Error(errorMessage);
      }

      const data = await response.json();
      
      // Store user data (both tokens automatically stored as HTTPOnly cookies)
      this.storeUserData(data.user);
      
      return {
        success: true,
        user: data.user,
        expires_in: data.expires_in
      };
    } catch (error) {
      this.clearTokens();
      
      // Handle different error types more robustly
      let errorMessage;
      if (error.message) {
        errorMessage = error.message;
      } else if (typeof error === 'string') {
        errorMessage = error;
      } else if (error.detail) {
        errorMessage = error.detail;
      } else {
        errorMessage = 'Login failed. Please try again.';
      }
      
      return {
        success: false,
        error: errorMessage
      };
    }
  }

  /**
   * Refresh access token using refresh token
   */
  async refreshTokens() {
    // Prevent multiple simultaneous refresh attempts
    if (this.refreshPromise) {
      return this.refreshPromise;
    }

    // Refresh token is in HTTPOnly cookie, no need to check client-side
    this.refreshPromise = this._performTokenRefresh();
    
    try {
      const result = await this.refreshPromise;
      return result;
    } finally {
      this.refreshPromise = null;
    }
  }

  /**
   * Internal method to perform token refresh
   */
  async _performTokenRefresh() {
    try {
      const response = await fetch(`${API_CONFIG.baseURL}/api/v1/auth/refresh`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
      });

      if (!response.ok) {
        const errorData = await response.json();
        throw new Error(errorData.detail || `Token refresh failed: ${response.status}`);
      }

      const data = await response.json();
      
      // Store user data (tokens are automatically set as HTTPOnly cookies)
      this.storeUserData(data.user);
      
      return {
        success: true,
        access_token: data.access_token
      };
    } catch (error) {
      // Don't log expected errors (no refresh token when not logged in)
      const isExpectedError = error.message?.includes('No refresh token') ||
                              error.message?.includes('401');
      if (!isExpectedError) {
      }

      // If refresh fails, clear all tokens and redirect to login
      this.clearTokens();

      throw error;
    }
  }

  /**
   * Logout from current session
   */
  async logout() {
    try {
      // Call backend logout to revoke refresh token (sent as cookie)
      await fetch(`${API_CONFIG.baseURL}/api/v1/auth/logout`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
      });
      
    } catch (error) {
      // Continue with client-side logout even if backend call fails
    }
    
    this.clearTokens();
    return { success: true };
  }

  /**
   * Logout from all devices/sessions
   */
  async logoutAll() {
    try {
      // Call backend to revoke all tokens (refresh token sent as cookie)
      await fetch(`${API_CONFIG.baseURL}/api/v1/auth/logout-all`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
      });
      
    } catch (error) {
    }
    
    this.clearTokens();
    return { success: true };
  }

  /**
   * Check if authentication is valid by making a test request
   * With HTTPOnly cookies, we can't check tokens client-side
   * If initial request fails with 401, attempt token refresh
   */
  async checkAuthStatus() {
    try {
      const response = await fetch(`${API_CONFIG.baseURL}/api/v1/auth/profile`, {
        credentials: 'include' // Include HTTPOnly cookies
      });

      if (response.ok) {
        const data = await response.json();
        this.storeUserData(data);
        return true;
      }

      // If profile request failed with 401, try refreshing tokens
      if (response.status === 401) {
        try {
          await this.refreshTokens();

          // Retry profile request after refresh
          const retryResponse = await fetch(`${API_CONFIG.baseURL}/api/v1/auth/profile`, {
            credentials: 'include'
          });

          if (retryResponse.ok) {
            const data = await retryResponse.json();
            this.storeUserData(data);
            return true;
          }
        } catch (refreshError) {
          // Don't log expected errors (no refresh token when not logged in)
          const isExpectedError = refreshError.message?.includes('No refresh token') ||
                                  refreshError.message?.includes('401');
          if (!isExpectedError) {
          }
        }
      }

      return false;
    } catch {
      return false;
    }
  }

  /**
   * Check if user is authenticated
   */
  isAuthenticated() {
    // With HTTPOnly cookies, we can only check if user data exists
    // The actual authentication is verified by the server on each request
    return !!this.user;
  }

  /**
   * Get current user data
   */
  getCurrentUser() {
    return this.user;
  }

  /**
   * Get active sessions for current user
   */
  async getSessions() {
    try {
      const response = await fetch(`${API_CONFIG.baseURL}/api/v1/auth/sessions`, {
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies for authentication
      });

      if (!response.ok) {
        throw new Error(`Failed to get sessions: ${response.status}`);
      }

      const data = await response.json();
      return data.sessions;
    } catch (error) {
      throw error;
    }
  }

  /**
   * Revoke a specific session (refresh token) by token ID
   */
  async revokeSession(tokenId) {
    const response = await fetch(`${API_CONFIG.baseURL}/api/v1/auth/sessions/${tokenId}`, {
      method: 'DELETE',
      headers: {
        'Content-Type': 'application/json',
      },
      credentials: 'include',
    });

    if (!response.ok) {
      const data = await response.json();
      throw new Error(data.detail || `Failed to revoke session: ${response.status}`);
    }

    return await response.json();
  }

  /**
   * Start Google OAuth login flow
   */
  async startGoogleOAuth() {
    try {
      const response = await fetch(`${API_CONFIG.baseURL}/api/v1/auth/google/login`, {
        method: 'GET',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include'
      });

      if (!response.ok) {
        const errorData = await response.json();
        throw new Error(errorData.detail || `Google OAuth start failed: ${response.status}`);
      }

      const data = await response.json();

      // Store state for CSRF protection
      localStorage.setItem('google_oauth_state', data.state);

      // Redirect to Google OAuth
      window.location.href = data.authorization_url;

      return {
        success: true,
        authorization_url: data.authorization_url
      };
    } catch (error) {
      return {
        success: false,
        error: error.message || 'Failed to start Google OAuth login'
      };
    }
  }

  /**
   * Handle Google OAuth callback
   */
  async handleGoogleOAuthCallback(code, state) {
    try {
      // Verify state matches what we stored (CSRF protection)
      const storedState = localStorage.getItem('google_oauth_state');
      if (!storedState || storedState !== state) {
        throw new Error('Invalid state parameter. Possible CSRF attack.');
      }

      // Clear stored state
      localStorage.removeItem('google_oauth_state');

      const response = await fetch(`${API_CONFIG.baseURL}/api/v1/auth/google/callback`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
        body: JSON.stringify({
          code: code,
          state: state
        }),
      });

      if (!response.ok) {
        let errorData;
        try {
          errorData = await response.json();
        } catch {
          throw new Error(`Google OAuth callback failed: ${response.status} ${response.statusText}`);
        }

        const errorMessage = errorData.detail || errorData.message || 'Google OAuth login failed';
        throw new Error(errorMessage);
      }

      const data = await response.json();

      // Store user data (tokens are automatically set as HTTPOnly cookies)
      this.storeUserData(data.user);

      return {
        success: true,
        user: data.user,
        message: data.message
      };
    } catch (error) {
      this.clearTokens();

      return {
        success: false,
        error: error.message || 'Google OAuth login failed'
      };
    }
  }

  /**
   * Initialize authentication state on app startup
   * For HTTPOnly cookies, we need to check server-side
   */
  async initializeAuth() {
    try {
      // Check if we have valid authentication via HTTPOnly cookies
      const isValid = await this.checkAuthStatus();
      return isValid;
    } catch (error) {
      this.clearTokens();
      return false;
    }
  }
}

// Export singleton instance
const authService = new AuthService();
export default authService;