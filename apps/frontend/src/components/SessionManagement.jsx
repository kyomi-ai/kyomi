// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Session Management Component
 *
 * Displays active sessions and allows users to manage their login sessions
 * across different devices.
 */

import React, { useState, useEffect } from 'react';
import { useAuth } from '../context/AuthContext';
import { Button } from './ui/button';
import { Alert, AlertDescription } from './ui/alert';
import { Badge } from './ui/badge';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from './ui/card';
import { Smartphone, Monitor, RefreshCw, Plug, X } from 'lucide-react';
import { Spinner } from './ui/spinner';
import ConfirmDialog from './ConfirmDialog';
import useConfirm from '../hooks/useConfirm';

const SessionManagement = () => {
  const { isOpen, dialogProps, confirm } = useConfirm();
  const [sessions, setSessions] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [revokingId, setRevokingId] = useState(null);
  const { getSessions, logoutAll, revokeSession } = useAuth();

  useEffect(() => {
    loadSessions();
  }, []);

  const loadSessions = async () => {
    try {
      setLoading(true);
      setError(null);
      const result = await getSessions();

      if (result.success) {
        setSessions(result.sessions || []);
      } else {
        setError(result.error || 'Failed to load sessions');
      }
    } catch (err) {
      setError('Failed to load sessions');
    } finally {
      setLoading(false);
    }
  };

  const handleLogoutAll = async () => {
    const confirmed = await confirm({
      title: 'Log Out From All Devices?',
      message: 'Are you sure you want to log out from all devices? You will need to log in again.',
      confirmText: 'Log Out All',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }

    try {
      setLoading(true);
      const result = await logoutAll();

      if (result.success) {
        // Redirect to login page after successful logout
        window.location.href = '/login';
      } else {
        setError(result.error || 'Failed to logout from all devices');
      }
    } catch (err) {
      setError('Failed to logout from all devices');
    } finally {
      setLoading(false);
    }
  };

  const handleRevokeSession = async (session) => {
    const displayName = session.oauth_client_name
      ? session.oauth_client_name
      : `${parseUserAgent(session.user_agent).browser} on ${parseUserAgent(session.user_agent).os}`;

    const confirmed = await confirm({
      title: 'Disconnect Session?',
      message: `Are you sure you want to disconnect "${displayName}"? That client will need to re-authenticate.`,
      confirmText: 'Disconnect',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }

    try {
      setRevokingId(session.token_id);
      setError(null);
      const result = await revokeSession(session.token_id);

      if (result.success) {
        await loadSessions();
      } else {
        setError(result.error || 'Failed to disconnect session');
      }
    } catch (err) {
      setError('Failed to disconnect session');
    } finally {
      setRevokingId(null);
    }
  };

  const formatDate = (dateString) => {
    try {
      const date = new Date(dateString);
      return date.toLocaleDateString('en-US', {
        month: 'short',
        day: 'numeric',
        year: 'numeric',
        hour: '2-digit',
        minute: '2-digit'
      });
    } catch {
      return 'Unknown';
    }
  };

  const parseUserAgent = (userAgent) => {
    if (!userAgent) return { browser: 'Unknown Browser', os: 'Unknown OS', isMobile: false };

    const ua = userAgent.toLowerCase();
    let browser = 'Unknown Browser';
    let os = 'Unknown OS';
    let isMobile = false;

    // Detect browser
    if (ua.includes('firefox') && !ua.includes('seamonkey')) {
      browser = 'Firefox';
    } else if (ua.includes('seamonkey')) {
      browser = 'Seamonkey';
    } else if (ua.includes('chrome') && !ua.includes('chromium') && !ua.includes('edg')) {
      browser = 'Chrome';
    } else if (ua.includes('chromium')) {
      browser = 'Chromium';
    } else if (ua.includes('safari') && !ua.includes('chrome')) {
      browser = 'Safari';
    } else if (ua.includes('edg')) {
      browser = 'Edge';
    } else if (ua.includes('opera') || ua.includes('opr')) {
      browser = 'Opera';
    }

    // Detect OS
    if (ua.includes('android')) {
      os = 'Android';
      isMobile = true;
    } else if (ua.includes('iphone') || ua.includes('ipad')) {
      os = ua.includes('ipad') ? 'iPad' : 'iPhone';
      isMobile = true;
    } else if (ua.includes('mac os x') || ua.includes('macintosh')) {
      os = 'macOS';
    } else if (ua.includes('windows')) {
      os = 'Windows';
    } else if (ua.includes('linux')) {
      os = 'Linux';
    }

    return { browser, os, isMobile };
  };

  const getDeviceIcon = (session) => {
    if (session.oauth_client_name) {
      return <Plug className="h-5 w-5 text-muted-foreground" />;
    }
    const { isMobile } = parseUserAgent(session.user_agent);
    if (isMobile) {
      return <Smartphone className="h-5 w-5 text-muted-foreground" />;
    }
    return <Monitor className="h-5 w-5 text-muted-foreground" />;
  };

  const getSessionDisplayName = (session) => {
    if (session.oauth_client_name) {
      return session.oauth_client_name;
    }
    const { browser, os } = parseUserAgent(session.user_agent);
    return `${browser} on ${os}`;
  };

  return (
    <>
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <div>
              <CardTitle>Active Sessions</CardTitle>
              <CardDescription>Manage your active login sessions across different devices.</CardDescription>
            </div>
            <Button
              variant="outline"
              onClick={loadSessions}
              disabled={loading}
              size="icon"
              title="Refresh sessions"
            >
              <RefreshCw className={`h-4 w-4 ${loading ? 'animate-spin' : ''}`} />
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          {error && (
            <Alert variant="error" className="mb-6">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          <div className="mb-6">
            {loading && sessions.length === 0 ? (
              <div className="text-center py-8">
                <Spinner size="lg" className="text-primary mx-auto" />
                <p className="text-muted-foreground mt-2">Loading sessions...</p>
              </div>
            ) : sessions.length === 0 ? (
              <div className="text-center py-8 bg-muted rounded-lg border-2 border-dashed border-border">
                <p className="text-muted-foreground">No active sessions found</p>
              </div>
            ) : (
              <div className="overflow-x-auto">
                <table className="min-w-full divide-y divide-border">
                  <thead className="bg-muted">
                    <tr>
                      <th className="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Device
                      </th>
                      <th className="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Location
                      </th>
                      <th className="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Last Active
                      </th>
                      <th className="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Created
                      </th>
                      <th className="px-6 py-3 text-right text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Actions
                      </th>
                    </tr>
                  </thead>
                  <tbody className="bg-background divide-y divide-border">
                    {sessions.map((session, index) => {
                      return (
                        <tr key={session.token_id || index}>
                          <td className="px-6 py-4 whitespace-nowrap">
                            <div className="flex items-center">
                              <div className="flex-shrink-0">
                                {getDeviceIcon(session)}
                              </div>
                              <div className="ml-3">
                                <div className="flex items-center gap-2">
                                  <div className="text-sm font-medium text-foreground">
                                    {getSessionDisplayName(session)}
                                  </div>
                                  {session.is_current && (
                                    <Badge variant="default">Current</Badge>
                                  )}
                                  {session.oauth_client_name && (
                                    <Badge variant="secondary">MCP</Badge>
                                  )}
                                </div>
                                {!session.oauth_client_name && (
                                  <div className="text-xs text-muted-foreground truncate max-w-xs mt-2">
                                    {session.user_agent || 'No user agent'}
                                  </div>
                                )}
                              </div>
                            </div>
                          </td>
                          <td className="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                            {session.ip_address
                              ? session.country_code
                                ? `${session.ip_address} (${session.country_code})`
                                : session.ip_address
                              : '—'}
                          </td>
                          <td className="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                            {formatDate(session.last_used || session.created_at)}
                          </td>
                          <td className="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                            {formatDate(session.created_at)}
                          </td>
                          <td className="px-6 py-4 whitespace-nowrap text-right">
                            {!session.is_current && (
                              <Button
                                variant="ghost"
                                size="sm"
                                onClick={() => handleRevokeSession(session)}
                                disabled={revokingId === session.token_id}
                                title="Disconnect this session"
                              >
                                {revokingId === session.token_id ? (
                                  <Spinner size="sm" />
                                ) : (
                                  <X className="h-4 w-4 text-muted-foreground hover:text-destructive" />
                                )}
                              </Button>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
          </div>

          {sessions.length > 1 && (
            <div className="border-t border-border pt-6">
              <div className="flex items-start justify-between mb-4">
                <div>
                  <h3 className="text-lg font-semibold text-foreground mb-2">Sign Out All Devices</h3>
                  <p className="text-sm text-muted-foreground">
                    This will end all active sessions and require you to log in again on all devices.
                  </p>
                </div>
              </div>
              <Button
                variant="destructive"
                onClick={handleLogoutAll}
                disabled={loading}
              >
                {loading ? 'Logging out...' : 'Log Out from All Devices'}
              </Button>
            </div>
          )}

          {sessions.length > 0 && (
            <Alert variant="info" className="mt-6">
              <AlertDescription>
                <strong>Security tip:</strong> If you see any unfamiliar sessions, log out from all devices immediately and change your password.
              </AlertDescription>
            </Alert>
          )}
        </CardContent>
      </Card>

      {/* Confirm Dialog */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
    </>
  );
};

export default SessionManagement;
