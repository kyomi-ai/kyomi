// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { createContext, useContext, useState, useEffect, useCallback } from 'react';
import { useAuth } from './AuthContext';

// Export context so TrialCapabilitiesProvider can use the same context reference
export const CapabilitiesContext = createContext();

export const useCapabilities = () => {
  const context = useContext(CapabilitiesContext);
  if (!context) {
    throw new Error('useCapabilities must be used within a CapabilitiesProvider');
  }
  return context;
};

export const CapabilitiesProvider = ({ children }) => {
  const { apiClient, user } = useAuth();
  const [capabilities, setCapabilities] = useState(null);
  const [loading, setLoading] = useState(true);

  // Fetch capabilities from backend
  const fetchCapabilities = useCallback(async () => {
    if (!user || !apiClient) return;

    try {
      const response = await apiClient.get('/api/v1/workspaces/settings');
      const rawCapabilities = response.data.capabilities;

      setCapabilities(rawCapabilities);
    } catch (error) {
    } finally {
      setLoading(false);
    }
  }, [user, apiClient]);

  // Fetch on mount and when user/apiClient changes
  useEffect(() => {
    fetchCapabilities();
  }, [fetchCapabilities]);

  // Helper function for feature checks
  const can = (feature) => {
    if (!capabilities) return false;

    // Map feature names to capability checks
    const checks = {
      use_ai_chat: capabilities.ai_chat_enabled,
      generate_sql: capabilities.ai_sql_generation_enabled,
      use_autocomplete: capabilities.ai_autocomplete_enabled,
      use_chart_copilot: capabilities.ai_chart_copilot_enabled,
      manage_users: capabilities.user_management_enabled,
      share_dashboards: capabilities.dashboard_sharing_enabled,
      export_data: capabilities.export_enabled,
      use_api: capabilities.api_access_enabled,
    };

    return checks[feature] ?? false;
  };

  const value = {
    capabilities,
    loading,
    can,
    refetch: fetchCapabilities, // Allow manual refresh

    // Specific getters for common checks
    get aiEnabled() {
      return capabilities?.ai_chat_enabled ?? false;
    },

    get creditsExhausted() {
      return capabilities?.credits_exhausted ?? false;
    },

    get subscriptionTier() {
      return capabilities?.subscription_tier ?? 'free';
    },

    get bigqueryMode() {
      // Backend returns 'backend_proxy' (Arrow streaming) or 'direct_api' (JSON)
      // based on subscription tier and arrow_download_enabled setting
      return capabilities?.bigquery_mode;
    },

    get bigqueryAccessLevel() {
      // Backend returns 'none', 'demo', or 'full' based on OAuth status
      return capabilities?.bigquery_access_level ?? 'none';
    },

    get isEnterprise() {
      return capabilities?.subscription_tier === 'enterprise';
    },

    get creditsRemaining() {
      return capabilities?.credits_remaining ?? 0;
    },

    get creditsLimit() {
      return capabilities?.credits_limit ?? 0;
    },

    get creditsPercentageUsed() {
      return capabilities?.percentage_used ?? 0;
    },

    get max_dashboards() {
      return capabilities?.max_dashboards ?? 0;
    },

    get kyomiWatchEnabled() {
      return capabilities?.kyomi_watch_enabled ?? false;
    },

    get arrowStreamingEnabled() {
      return capabilities?.arrow_streaming_enabled ?? false;
    },

    get multiUserEnabled() {
      return capabilities?.multi_user_enabled ?? false;
    },

    get slackIntegrationEnabled() {
      return capabilities?.slack_integration_enabled ?? false;
    },

    get mcpAccessEnabled() {
      return capabilities?.mcp_access_enabled ?? false;
    },
  };

  return <CapabilitiesContext.Provider value={value}>{children}</CapabilitiesContext.Provider>;
};
