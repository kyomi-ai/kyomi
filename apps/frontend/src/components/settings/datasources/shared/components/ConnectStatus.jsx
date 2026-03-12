// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { Button } from '@/components/ui/button';
import { Alert, AlertTitle, AlertDescription } from '@/components/ui/alert';
import { AlertTriangle } from 'lucide-react';
import ConfirmDialog from '../../../../ConfirmDialog';
import apiClient from '@/api/apiClient';
import CopyButton from './CopyButton';
import { DeploymentTabs } from './ConnectSetup';

/**
 * ConnectStatus - Shown in edit mode for Kyomi Connect datasources.
 * Displays connection status, deployment instructions, and allows token rotation/disconnect.
 */
export default function ConnectStatus({ datasourceId, datasourceType, datasourceName }) {
  const [status, setStatus] = useState(null);
  const [loading, setLoading] = useState(true);
  const [rotating, setRotating] = useState(false);
  const [newToken, setNewToken] = useState(null);
  const [disconnecting, setDisconnecting] = useState(false);
  const [showRotateConfirm, setShowRotateConfirm] = useState(false);
  const [showDisconnectConfirm, setShowDisconnectConfirm] = useState(false);
  const [deployTab, setDeployTab] = useState('linux');

  useEffect(() => {
    fetchStatus();
    const interval = setInterval(fetchStatus, 10000); // Poll every 10s
    return () => clearInterval(interval);
  }, [datasourceId]);

  const fetchStatus = async () => {
    try {
      const response = await apiClient.get(`/api/v1/datasources/${datasourceId}/connect/status`);
      setStatus(response.data);
    } catch (err) {
      console.error('Failed to fetch Connect status:', err);
    } finally {
      setLoading(false);
    }
  };

  const handleRotateToken = async () => {
    setShowRotateConfirm(true);
  };

  const confirmRotateToken = async () => {
    setShowRotateConfirm(false);
    setRotating(true);
    try {
      const response = await apiClient.post(`/api/v1/datasources/${datasourceId}/connect/rotate-token`);
      setNewToken(response.data.token);
      fetchStatus();
    } catch (err) {
      console.error('Failed to rotate token:', err);
    } finally {
      setRotating(false);
    }
  };

  const handleDisconnect = async () => {
    setShowDisconnectConfirm(true);
  };

  const confirmDisconnect = async () => {
    setShowDisconnectConfirm(false);
    setDisconnecting(true);
    try {
      await apiClient.post(`/api/v1/datasources/${datasourceId}/connect/disconnect`);
      setNewToken(null);
      fetchStatus();
    } catch (err) {
      console.error('Failed to disconnect:', err);
    } finally {
      setDisconnecting(false);
    }
  };

  if (loading) {
    return <div className="text-sm text-muted-foreground">Loading status...</div>;
  }

  return (
    <div className="space-y-6">
      {/* Connection status indicator */}
      <div className="flex items-center gap-3 p-4 rounded-lg border border-border">
        <div className={`w-3 h-3 rounded-full ${
          status?.connected ? 'bg-success-foreground animate-pulse' : 'bg-destructive'
        }`} />
        <div>
          <div className="text-sm font-medium text-foreground">
            {status?.connected ? 'Connected' : 'Disconnected'}
          </div>
          <div className="text-xs text-muted-foreground">
            {status?.connected
              ? 'Kyomi Connect agent is online'
              : 'Waiting for agent to connect'}
          </div>
        </div>
      </div>

      {/* New token display (after rotation) */}
      {newToken && (
        <div className="space-y-4">
          <Alert variant="warning">
            <AlertTriangle className="h-4 w-4" />
            <AlertTitle>New token generated</AlertTitle>
            <AlertDescription>
              Save this token now — it will not be shown again.
            </AlertDescription>
          </Alert>

          <div className="space-y-1.5">
            <label className="block text-sm font-medium text-foreground">
              New Connect Token
            </label>
            <div className="group flex items-center gap-2 rounded-lg border border-border bg-muted/50 px-3 py-2">
              <code className="flex-1 text-xs font-mono text-foreground truncate select-all">
                {newToken}
              </code>
              <CopyButton text={newToken} className="opacity-0 group-hover:opacity-100 shrink-0" />
            </div>
          </div>
        </div>
      )}

      {/* Deployment instructions — always visible */}
      <div className="space-y-2">
        <h4 className="text-sm font-medium text-foreground">Deployment Instructions</h4>
        {!newToken && (
          <p className="text-xs text-muted-foreground">
            Rotate the token above to generate a new one, then copy the commands below.
          </p>
        )}
        <DeploymentTabs
          token={newToken || '<YOUR_TOKEN>'}
          datasourceType={datasourceType}
          activeTab={deployTab}
          setActiveTab={setDeployTab}
        />
      </div>

      {/* Management buttons */}
      <div className="flex gap-3">
        <Button
          variant="outline"
          onClick={handleRotateToken}
          disabled={rotating}
        >
          {rotating ? 'Rotating...' : 'Rotate Token'}
        </Button>
        <Button
          variant="destructive"
          onClick={handleDisconnect}
          disabled={disconnecting}
        >
          {disconnecting ? 'Disconnecting...' : 'Disconnect'}
        </Button>
      </div>

      <p className="text-xs text-muted-foreground">
        Rotating the token generates a new token and immediately disconnects the current agent.
        The agent must be restarted with the new token.
      </p>

      {/* Rotate Token Confirmation Dialog */}
      <ConfirmDialog
        isOpen={showRotateConfirm}
        onConfirm={confirmRotateToken}
        onCancel={() => setShowRotateConfirm(false)}
        title="Rotate Token?"
        message="Rotating the token will disconnect the current Connect agent. You will need to restart it with the new token. Continue?"
        confirmText="Rotate"
        variant="destructive"
      />

      {/* Disconnect Confirmation Dialog */}
      <ConfirmDialog
        isOpen={showDisconnectConfirm}
        onConfirm={confirmDisconnect}
        onCancel={() => setShowDisconnectConfirm(false)}
        title="Disconnect Agent?"
        message="This will revoke the token and disconnect the Connect agent. You will need to redeploy it with a new token. Continue?"
        confirmText="Disconnect"
        variant="destructive"
      />
    </div>
  );
}
