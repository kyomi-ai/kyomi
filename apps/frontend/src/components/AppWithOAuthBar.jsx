// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { useQueryClient } from '@tanstack/react-query';
import { useAuth } from '../context/AuthContext';
import { useSidebar } from './Sidebar';
import CatalogStatusBar from './CatalogStatusBar';
import InvitationStatusBar from './InvitationStatusBar';
import AIUsageStatusBar from './AIUsageStatusBar';
import OwnershipTransferStatusBar from './OwnershipTransferStatusBar';
import Sidebar from './Sidebar';
import { toast } from '../lib/toast';

/**
 * Wrapper component that provides the main application layout:
 * - Sidebar (left side, collapsible)
 * - Content area (adjusts based on sidebar state)
 * - CatalogStatusBar (bottom, handles catalog indexing AND credential status)
 */
const AppWithOAuthBar = ({ children }) => {
  const { isSidebarCollapsed } = useSidebar();
  const { user, apiClient, refreshUser } = useAuth();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const websocketRef = useRef(null);

  // Catalog refresh status
  const [catalogStatus, setCatalogStatus] = useState('idle');
  const [catalogProgress, setCatalogProgress] = useState(null);
  const [catalogLastRefresh, setCatalogLastRefresh] = useState(null);

  // AI usage status
  const [aiUsageWarningLevel, setAiUsageWarningLevel] = useState(null);
  const [aiUsagePercentage, setAiUsagePercentage] = useState(0);
  const [aiUsageMessage, setAiUsageMessage] = useState('');

  // Invitation status
  const [invitations, setInvitations] = useState([]);

  // Ownership transfer status
  const [ownershipTransfers, setOwnershipTransfers] = useState([]);

  // Check catalog refresh status periodically (every 5 seconds when running, every 30 seconds when idle)
  useEffect(() => {
    if (!user) return;

    const checkCatalogStatus = async () => {
      try {
        const response = await apiClient.get('/api/v1/workspaces/catalog/status');
        const data = response.data;

        setCatalogStatus(data.status || 'idle');
        setCatalogProgress(data.progress || null);
        setCatalogLastRefresh(data.last_refresh || null);
      } catch (error) {
      }
    };

    // Check immediately on mount
    checkCatalogStatus();

    // Poll more frequently when catalog is running, less frequently when idle
    let intervalId;
    const startPolling = () => {
      // Clear existing interval if any
      if (intervalId) {
        clearInterval(intervalId);
      }

      // Set interval based on catalog status
      // Reduced from 5s/30s to 60s/300s since we have WebSocket push for real-time updates
      const pollInterval = catalogStatus === 'running' ? 60000 : 300000; // 60s when running, 300s (5min) when idle
      intervalId = setInterval(checkCatalogStatus, pollInterval);
    };

    startPolling();

    return () => {
      if (intervalId) {
        clearInterval(intervalId);
      }
    };
  }, [user, apiClient, catalogStatus]); // Re-run when catalogStatus changes to adjust polling frequency

  // Check AI usage status periodically (every 60 seconds)
  useEffect(() => {
    if (!user) return;

    const checkAIUsageStatus = async () => {
      try {
        const response = await apiClient.get('/api/v1/billing/ai-usage-status');
        const data = response.data;

        setAiUsageWarningLevel(data.warning_level);
        setAiUsagePercentage(data.percentage_used || 0);
        setAiUsageMessage(data.message || '');
      } catch (error) {
        // Don't show errors to user - this is background monitoring
      }
    };

    // Check immediately on mount
    checkAIUsageStatus();

    // Fallback poll every 5 minutes (reduced from 60s since we have WebSocket push)
    const intervalId = setInterval(checkAIUsageStatus, 5 * 60 * 1000);

    // Store function so WebSocket can trigger refresh
    window.checkAIUsageStatus = checkAIUsageStatus;

    return () => {
      clearInterval(intervalId);
      delete window.checkAIUsageStatus;
    };
  }, [user, apiClient]);

  // Check for pending invitations periodically (every 30 seconds)
  useEffect(() => {
    if (!user) return;

    const checkPendingInvitations = async () => {
      try {
        const response = await apiClient.get('/api/v1/workspaces/invitations/pending');
        const pending = response.data || [];
        setInvitations(pending);
      } catch (error) {
      }
    };

    // Check immediately on mount
    checkPendingInvitations();

    // Fallback poll every 5 minutes (reduced from 30s since we have WebSocket push)
    const intervalId = setInterval(checkPendingInvitations, 5 * 60 * 1000);

    // Store function so WebSocket can trigger refresh
    window.checkPendingInvitations = checkPendingInvitations;

    return () => {
      clearInterval(intervalId);
      delete window.checkPendingInvitations;
    };
  }, [user, apiClient]);

  // Check for pending ownership transfers periodically (every 30 seconds)
  useEffect(() => {
    if (!user) return;

    const checkPendingOwnershipTransfers = async () => {
      try {
        const response = await apiClient.get('/api/v1/workspaces/ownership/transfers');
        const transfers = response.data || [];
        // Filter for transfers where current user is the recipient
        // The API already filters for pending transfers, and sets is_recipient flag
        const receivedTransfers = transfers.filter(t => t.is_recipient === true);
        setOwnershipTransfers(receivedTransfers);
      } catch (error) {
      }
    };

    // Check immediately on mount
    checkPendingOwnershipTransfers();

    // Fallback poll every 5 minutes (reduced from 30s since we have WebSocket push)
    const intervalId = setInterval(checkPendingOwnershipTransfers, 5 * 60 * 1000);

    // Store function so WebSocket can trigger refresh
    window.checkPendingOwnershipTransfers = checkPendingOwnershipTransfers;

    return () => {
      clearInterval(intervalId);
      delete window.checkPendingOwnershipTransfers;
    };
  }, [user, apiClient]);

  // WebSocket connection for OAuth reconnection events
  useEffect(() => {
    if (!user) {
      return;
    }

    let reconnectTimeout;
    let shouldReconnect = true;

    const connectWebSocket = async () => {
      try {
        // Get WebSocket authentication token (same as Chat component)
        const tokenResponse = await apiClient.get('/api/v1/auth/websocket-token');
        const wsToken = tokenResponse.data.token;

        const workspaceUserId = `${user.workspace_id}_${user.user_id}`;
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const host = window.location.host;
        const wsUrl = `${protocol}//${host}/ws/${workspaceUserId}?token=${wsToken}`;

        const ws = new WebSocket(wsUrl);

        ws.onopen = () => {
          // WebSocket connected
        };

        ws.onmessage = (event) => {
          try {
            const data = JSON.parse(event.data);

            // Handle workspace removal notification
            if (data.type === 'workspace_removed') {
              const workspaceName = data.workspace_name || 'a workspace';
              toast.error(`${data.message || `You have been removed from "${workspaceName}"`}. You will be redirected to the home page.`, { duration: 6000 });
              // Reload to refresh user context and workspace access
              window.location.href = '/';
            }

            // Handle workspace invitation notification
            if (data.type === 'workspace_invitation') {
              // Trigger invitation check to show new invitation immediately
              if (window.checkPendingInvitations) {
                window.checkPendingInvitations();
              }
            }

            // Handle ownership transfer notification
            if (data.type === 'ownership_transfer_offered') {
              // Show toast notification
              toast.info(
                `${data.data.from_user_email} has offered to transfer workspace ownership to you.`,
                { duration: 6000 }
              );
              // Trigger ownership transfer check to show new transfer immediately
              if (window.checkPendingOwnershipTransfers) {
                window.checkPendingOwnershipTransfers();
              }
            }

            // Handle watch alert notification
            if (data.type === 'watch_alert') {
              const alertData = data.data || {};
              const alertTitle = alertData.alert_title;
              const summary = alertData.summary;
              const watchName = alertData.watch_name || 'Watch';
              // Show alert title if available, otherwise fall back to watch name
              const toastMessage = alertTitle
                ? `${alertTitle}`
                : `New alert from "${watchName}"`;
              toast.info(
                toastMessage,
                {
                  duration: 6000,
                  description: summary || (alertTitle ? `From: ${watchName}` : undefined),
                  action: {
                    label: 'View',
                    onClick: () => navigate('/watches/alerts')
                  }
                }
              );
              // Invalidate alerts count query to update sidebar badge
              queryClient.invalidateQueries({ queryKey: ['unread-alerts-count'] });
            }

            // Handle catalog status update (real-time push from indexer)
            if (data.type === 'catalog_status_update') {
              const statusData = data.data || {};
              setCatalogStatus(statusData.status || 'idle');
              setCatalogProgress(statusData.progress || null);
              // Note: last_refresh is updated when status goes idle after completing
            }

            // Handle AI usage update (real-time push after token consumption)
            if (data.type === 'ai_usage_update') {
              // Refresh AI usage status
              if (window.checkAIUsageStatus) {
                window.checkAIUsageStatus();
              }
            }
          } catch (error) {
          }
        };

        ws.onerror = (error) => {
        };

        ws.onclose = () => {
          // Automatically reconnect after 2 seconds if we should still be connected
          if (shouldReconnect) {
            reconnectTimeout = setTimeout(() => {
              connectWebSocket();
            }, 2000);
          }
        };

        websocketRef.current = ws;
      } catch (error) {

        // Retry connection after error
        if (shouldReconnect) {
          reconnectTimeout = setTimeout(() => {
            connectWebSocket();
          }, 5000);
        }
      }
    };

    connectWebSocket();

    return () => {
      // Disable reconnection when component unmounts
      shouldReconnect = false;

      // Clear any pending reconnection
      if (reconnectTimeout) {
        clearTimeout(reconnectTimeout);
      }

      // Close WebSocket
      if (websocketRef.current) {
        websocketRef.current.close();
      }
    };
  }, [user]);

  // Handle invitation acceptance
  const handleAcceptInvitation = async (invitationId) => {
    try {
      await apiClient.post(`/api/v1/workspaces/invitations/${invitationId}/accept`);

      // Remove this invitation from the list
      setInvitations(prev => prev.filter(inv => inv.invitation_id !== invitationId));

      // Refresh user profile to get updated workspace list
      await refreshUser();

      // Show success message
      toast.success('Successfully joined workspace! You can now switch to it using the workspace switcher.');
    } catch (error) {
      toast.error(`Failed to accept invitation: ${error.response?.data?.detail || error.message}`);
    }
  };

  // Handle invitation decline
  const handleDeclineInvitation = async (invitationId) => {
    try {
      await apiClient.post(`/api/v1/workspaces/invitations/${invitationId}/decline`);

      // Remove this invitation from the list
      setInvitations(prev => prev.filter(inv => inv.invitation_id !== invitationId));
    } catch (error) {
      toast.error(`Failed to decline invitation: ${error.response?.data?.detail || error.message}`);
    }
  };

  // Handle ownership transfer notification dismissal (doesn't decline, just hides)
  // Note: Accept/decline is handled on the /accept-ownership/:transferId page
  const handleDismissOwnershipTransfer = (transferId) => {
    // Just remove from local state - transfer is still pending in database
    setOwnershipTransfers(prev => prev.filter(t => t.transfer_id !== transferId));
  };

  return (
    <div className="h-screen flex flex-col" style={{display: 'flex', flexDirection: 'column', height: '100vh'}}>
      {/* Main app container - takes up viewport minus OAuth bar height */}
      <div className={`flex relative flex-1 overflow-hidden`} style={{display: 'flex', position: 'relative', flex: '1 1 0%', overflow: 'hidden'}}>
        {/* Sidebar - absolute positioned on left side */}
        <Sidebar />

        {/* Content Area - adjusts margin based on sidebar state */}
        <div
          className={`flex-1 h-full min-w-0 overflow-hidden transition-all duration-300 pt-16 md:pt-0 ${isSidebarCollapsed ? 'ml-0 md:ml-16' : 'ml-0 md:ml-80'}`}
          style={{
            flex: '1 1 0%',
            height: '100%',
            minWidth: 0,
            overflow: 'hidden'
          }}
        >
          {children}
        </div>
      </div>

      {/* Invitation Status Bar - at bottom (above other bars) */}
      {invitations.length > 0 && (
        <InvitationStatusBar
          invitations={invitations}
          onAccept={handleAcceptInvitation}
          onDecline={handleDeclineInvitation}
        />
      )}

      {/* Ownership Transfer Status Bar - at bottom (above other bars, below invitations) */}
      {ownershipTransfers.length > 0 && (
        <OwnershipTransferStatusBar
          transfers={ownershipTransfers}
          onDismiss={handleDismissOwnershipTransfer}
        />
      )}

      {/* AI Usage Status Bar - at bottom (above catalog bar) */}
      {aiUsageWarningLevel && aiUsageWarningLevel !== 'none' && (
        <AIUsageStatusBar
          warningLevel={aiUsageWarningLevel}
          percentageUsed={aiUsagePercentage}
          message={aiUsageMessage}
        />
      )}

      {/* CatalogStatusBar - unified bar for catalog indexing AND credential status */}
      {/* Always rendered - component handles its own visibility based on state */}
      <CatalogStatusBar
        status={catalogStatus}
        progress={catalogProgress}
        lastRefresh={catalogLastRefresh}
      />
    </div>
  );
};

export default AppWithOAuthBar;
