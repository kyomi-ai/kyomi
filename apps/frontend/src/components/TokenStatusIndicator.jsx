// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Token Status Indicator Component
 *
 * Shows users the status of their authentication tokens and
 * provides visual feedback about token refresh activities.
 */

import React, { useState, useEffect } from 'react';
import { useAuth } from '../context/AuthContext';
import { StatusBadge } from '@/components/ui/status-badge';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';

const TokenStatusIndicator = () => {
  const [tokenStatus, setTokenStatus] = useState('checking');
  const [refreshing, setRefreshing] = useState(false);
  const { isAuthenticated, authService, user } = useAuth();

  useEffect(() => {
    checkTokenStatus();
    
    // Check token status periodically
    const interval = setInterval(checkTokenStatus, 30000); // Every 30 seconds
    
    return () => clearInterval(interval);
  }, [isAuthenticated]);

  const checkTokenStatus = async () => {
    try {
      if (!isAuthenticated) {
        setTokenStatus('unauthenticated');
        return;
      }

      const accessToken = await authService.getAccessToken();
      if (accessToken) {
        setTokenStatus('authenticated');
      } else {
        setTokenStatus('expired');
      }
    } catch (error) {
      setTokenStatus('error');
    }
  };

  const handleRefreshTokens = async () => {
    try {
      setRefreshing(true);
      await authService.refreshTokens();
      setTokenStatus('authenticated');
    } catch (error) {
      setTokenStatus('error');
    } finally {
      setRefreshing(false);
    }
  };

  const getStatusDisplay = () => {
    switch (tokenStatus) {
      case 'checking':
        return {
          icon: '⏳',
          text: 'Checking...',
          variant: 'default'
        };
      case 'authenticated':
        return {
          icon: '✅',
          text: 'Authenticated',
          variant: 'success'
        };
      case 'expired':
        return {
          icon: '⚠️',
          text: 'Token Expired',
          variant: 'warning'
        };
      case 'error':
        return {
          icon: '❌',
          text: 'Auth Error',
          variant: 'error'
        };
      case 'unauthenticated':
      default:
        return {
          icon: '🔒',
          text: 'Not Authenticated',
          variant: 'default'
        };
    }
  };

  const status = getStatusDisplay();

  // Don't show indicator for unauthenticated users
  if (tokenStatus === 'unauthenticated') {
    return null;
  }

  return (
    <StatusBadge variant={status.variant} className="gap-2">
      <span>{status.icon}</span>
      <span>{refreshing ? 'Refreshing...' : status.text}</span>

      {user && (
        <span className="text-xs opacity-75">
          ({user.email})
        </span>
      )}

      {(tokenStatus === 'expired' || tokenStatus === 'error') && !refreshing && (
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              onClick={handleRefreshTokens}
              className="ml-1 text-xs underline hover:no-underline"
              aria-label="Refresh tokens"
            >
              Refresh
            </button>
          </TooltipTrigger>
          <TooltipContent>Refresh tokens</TooltipContent>
        </Tooltip>
      )}
    </StatusBadge>
  );
};

export default TokenStatusIndicator;