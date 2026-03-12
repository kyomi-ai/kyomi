// SPDX-License-Identifier: AGPL-3.0-or-later
import { createContext, useContext, useState, useEffect } from 'react';
import apiClient from '../api/apiClient';
import authService from '../services/authService';
import { API_CONFIG } from '../config/api.js';
import { Spinner } from '../components/ui/spinner';
import { useTheme } from './ThemeContext';

// Export context so TrialAuthProvider can use the same context reference
export const AuthContext = createContext();

export function useAuth() {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error('useAuth must be used within an AuthProvider');
  }
  return context;
}

// Authentication states
const AUTH_STATES = {
  UNAUTHENTICATED: 'unauthenticated',
  CHALLENGE_REQUIRED: 'challenge_required',
  AUTHENTICATED: 'authenticated'
};

const CHALLENGE_TYPES = {
  MFA_REQUIRED: 'MFA_REQUIRED',
  NEW_PASSWORD_REQUIRED: 'NEW_PASSWORD_REQUIRED'
};

export function AuthProvider({ children }) {
  const [authState, setAuthState] = useState(AUTH_STATES.UNAUTHENTICATED);
  const [user, setUser] = useState(null);
  const [loading, setLoading] = useState(true);
  const [challenge, setChallenge] = useState(null); // { type, session, user_info }
  const [userChartMLConfig, setUserChartMLConfig] = useState(null);
  const [workspaceChartMLConfig, setWorkspaceChartMLConfig] = useState(null);
  const { setThemeFromServer } = useTheme();

  const apiClientInstance = apiClient;

  useEffect(() => {
    initializeAuth();

    // Listen for authentication failures from API client
    const handleAuthFailed = (event) => {
      setAuthState(AUTH_STATES.UNAUTHENTICATED);
      setUser(null);
      setChallenge(null);
      setLoading(false);
    };

    window.addEventListener('auth-failed', handleAuthFailed);

    // Debug: Allow manual logout trigger from console
    window.debugAuth = {
      forceLogout: () => {
        handleAuthFailed({ detail: { reason: 'debug', message: 'Debug logout' } });
      },
      checkAuthState: () => {
        return { authState, user, loading };
      }
    };

    return () => {
      window.removeEventListener('auth-failed', handleAuthFailed);
    };
  }, []);

  const fetchChartMLConfigs = async () => {
    try {
      // Fetch user ChartML config
      const userConfigResponse = await apiClientInstance.get('/api/v1/users/me/chartml-config');
      if (userConfigResponse.data && userConfigResponse.data.config) {
        setUserChartMLConfig(userConfigResponse.data.config);
      }

      // Fetch workspace ChartML config
      const workspaceConfigResponse = await apiClientInstance.get('/api/v1/workspaces/chartml-config');
      if (workspaceConfigResponse.data && workspaceConfigResponse.data.config) {
        setWorkspaceChartMLConfig(workspaceConfigResponse.data.config);
      }
    } catch (error) {
      // Non-critical error - don't fail authentication
    }
  };

  const initializeAuth = async () => {
    try {
      // With HTTPOnly cookies, we must check authentication with the server

      // Use the new initializeAuth method from authService
      const isAuthenticated = await authService.initializeAuth();

      if (isAuthenticated) {
        const currentUser = authService.getCurrentUser();
        if (currentUser) {
          setUser(currentUser);
          setAuthState(AUTH_STATES.AUTHENTICATED);
          setThemeFromServer(currentUser.extra_metadata?.theme);
          window.kyomi?.identify?.(currentUser.user_id);
          // Fetch ChartML configs after authentication
          fetchChartMLConfigs();
        } else {
          // Authentication successful but no user data, fetch it
          try {
            const response = await apiClientInstance.get('/api/v1/auth/profile');
            setUser(response.data);
            setAuthState(AUTH_STATES.AUTHENTICATED);
            setThemeFromServer(response.data.extra_metadata?.theme);
            window.kyomi?.identify?.(response.data.user_id);
            // Fetch ChartML configs after authentication
            fetchChartMLConfigs();
          } catch (profileError) {
            authService.clearTokens();
            setAuthState(AUTH_STATES.UNAUTHENTICATED);
            setUser(null);
          }
        }
      } else {
        // No valid authentication found
        setAuthState(AUTH_STATES.UNAUTHENTICATED);
        setUser(null);
      }
    } catch (error) {
      authService.clearTokens();
      setAuthState(AUTH_STATES.UNAUTHENTICATED);
      setUser(null);
      setChallenge(null);
    } finally {
      setLoading(false);
    }
  };

  const login = async (email, password, totpCode = null) => {
    try {
      setLoading(true);
      
      // If this is a challenge completion, use challenge session
      let actualEmail = email;
      let actualPassword = password;
      
      if (totpCode && challenge && challenge.type === CHALLENGE_TYPES.MFA_REQUIRED) {
        // Use stored credentials from challenge (secure - memory only, cleared on completion)
        actualEmail = challenge.user_info?.email || email;
        actualPassword = challenge.user_info?.password || password;
      }
      
      const result = await authService.login(actualEmail, actualPassword, totpCode);
      
      if (result.success) {
        // Fetch complete user profile including workspace_id (consistent with passkey login)
        try {
          const response = await apiClientInstance.get('/api/v1/auth/profile');
          setUser(response.data);
          setAuthState(AUTH_STATES.AUTHENTICATED);
          setThemeFromServer(response.data.extra_metadata?.theme);
          window.kyomi?.identify?.(response.data.user_id);
          setChallenge(null); // Clear challenge on success
          return { success: true };
        } catch (profileError) {
          // Fallback to result.user if profile fetch fails
          setUser(result.user);
          setAuthState(AUTH_STATES.AUTHENTICATED);
          window.kyomi?.identify?.(result.user?.user_id);
          setChallenge(null);
          return { success: true };
        }
      } else {
        // Check for email verification required (403 with special flag)
        if (result.verificationRequired) {
          setChallenge(null);
          setAuthState(AUTH_STATES.UNAUTHENTICATED);
          return {
            success: false,
            error: result.error,
            verificationRequired: true
          };
        }

        let errorMsg;
        if (typeof result.error === 'string') {
          errorMsg = result.error;
        } else if (result.error instanceof Error) {
          errorMsg = result.error.message;
        } else if (result.error && typeof result.error === 'object') {
          errorMsg = result.error.detail || result.error.message || JSON.stringify(result.error);
        } else {
          errorMsg = String(result.error);
        }


        // Handle 2FA-related errors when we have an active challenge
        if (challenge && challenge.type === CHALLENGE_TYPES.MFA_REQUIRED) {
          const hasValidCredentials = challenge.user_info?.email && challenge.user_info?.password;
          
          // If this is a 2FA verification error, keep the challenge active
          if (errorMsg.includes('Invalid 2FA verification code') || errorMsg.includes('invalid 2FA')) {
            // Return error but maintain challenge state for correction
            return { 
              success: false, 
              error: errorMsg,
              challenge: challenge // Keep challenge active
            };
          }
          
          // If we don't have credentials for other errors, reset state
          if (!hasValidCredentials) {
            setChallenge(null);
            setAuthState(AUTH_STATES.UNAUTHENTICATED);
            
            return { 
              success: false, 
              error: "Session expired. Please sign in again.",
              shouldResetLogin: true
            };
          }
        }
        
        // Check if this is a challenge (2FA required)
        if (errorMsg.includes('2FA verification code is required')) {
          const challengeData = {
            type: CHALLENGE_TYPES.MFA_REQUIRED,
            user_info: { 
              email: actualEmail, 
              password: actualPassword // Store in memory only - cleared on completion/error
            }
          };

          setChallenge(challengeData);
          setAuthState(AUTH_STATES.CHALLENGE_REQUIRED);

          // Return challenge instead of error
          return { 
            success: false, 
            challenge: challengeData,
            error: errorMsg 
          };
        } else {
          setChallenge(null);
          setAuthState(AUTH_STATES.UNAUTHENTICATED);
        }
        
        return { success: false, error: errorMsg };
      }
    } catch (error) {
      setChallenge(null);
      setAuthState(AUTH_STATES.UNAUTHENTICATED);
      return { success: false, error: error.message };
    } finally {
      setLoading(false);
    }
  };

  const loginWithPasskey = async (passkeyResult) => {
    try {
      setLoading(true);

      // With HTTPOnly-only authentication, passkeyResult contains the API response
      // Tokens are automatically set as HTTPOnly cookies, not in JSON response
      if (passkeyResult && passkeyResult.success && passkeyResult.user) {
        // Fetch complete user profile including workspace_id (just like initialization does)
        try {
          const response = await apiClientInstance.get('/api/v1/auth/profile');
          setUser(response.data);
          setAuthState(AUTH_STATES.AUTHENTICATED);
          setThemeFromServer(response.data.extra_metadata?.theme);
          window.kyomi?.identify?.(response.data.user_id);
          return { success: true };
        } catch (profileError) {
          // Fallback to passkeyResult.user if profile fetch fails
          setUser(passkeyResult.user);
          setAuthState(AUTH_STATES.AUTHENTICATED);
          window.kyomi?.identify?.(passkeyResult.user?.user_id);
          return { success: true };
        }
      } else {
        const error = passkeyResult?.error || 'Invalid passkey login response';
        return { success: false, error };
      }
    } catch (error) {
      return { success: false, error: error.message };
    } finally {
      setLoading(false);
    }
  };

  const logout = async () => {
    try {
      setLoading(true);
      await authService.logout();
      window.kyomi?.identify?.("");
      setAuthState(AUTH_STATES.UNAUTHENTICATED);
      setUser(null);
      setChallenge(null);
      return { success: true };
    } catch (error) {
      // Clear local state even if backend call fails
      authService.clearTokens();
      setAuthState(AUTH_STATES.UNAUTHENTICATED);
      setUser(null);
      setChallenge(null);
      return { success: false, error: error.message };
    } finally {
      setLoading(false);
    }
  };

  const logoutAll = async () => {
    try {
      setLoading(true);
      await authService.logoutAll();
      setAuthState(AUTH_STATES.UNAUTHENTICATED);
      setUser(null);
      setChallenge(null);
      return { success: true };
    } catch (error) {
      // Clear local state even if backend call fails
      authService.clearTokens();
      setAuthState(AUTH_STATES.UNAUTHENTICATED);
      setUser(null);
      setChallenge(null);
      return { success: false, error: error.message };
    } finally {
      setLoading(false);
    }
  };

  const getSessions = async () => {
    try {
      const sessions = await authService.getSessions();
      return { success: true, sessions };
    } catch (error) {
      return { success: false, error: error.message };
    }
  };

  const revokeSession = async (tokenId) => {
    try {
      await authService.revokeSession(tokenId);
      return { success: true };
    } catch (error) {
      return { success: false, error: error.message };
    }
  };

  // Helper computed properties for backward compatibility
  const isAuthenticated = authState === AUTH_STATES.AUTHENTICATED;
  const isChallengePending = authState === AUTH_STATES.CHALLENGE_REQUIRED;

  const refreshUser = async () => {
    try {
      const response = await apiClientInstance.get('/api/v1/auth/profile');
      setUser(response.data);
      setAuthState(AUTH_STATES.AUTHENTICATED);
      return { success: true };
    } catch (error) {
      return { success: false, error: error.message };
    }
  };

  const value = {
    // Authentication state
    isAuthenticated,
    user,
    loading,

    // Challenge-based auth (2FA flow)
    authState,
    isChallengePending,
    challenge,

    // ChartML configurations
    userChartMLConfig,
    workspaceChartMLConfig,
    fetchChartMLConfigs, // Expose for Settings page to refresh after save

    // Methods
    login,
    loginWithPasskey,
    logout,
    logoutAll,
    getSessions,
    revokeSession,
    refreshUser,
    apiClient: apiClientInstance,
    authService, // Expose authService for advanced use cases
  };

  // Only show loading screen on initial app load, not during login operations
  if (loading && !isAuthenticated && !user) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-muted">
        <Spinner size="xl" className="text-primary" />
        <span className="sr-only">Loading...</span>
      </div>
    );
  }

  return (
    <AuthContext.Provider value={value}>
      {children}
    </AuthContext.Provider>
  );
}