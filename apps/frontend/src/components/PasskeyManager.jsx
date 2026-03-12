// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Passkey Management Component
 *
 * Allows users to view, add, rename, and delete passkeys from their account.
 */

import React, { useState, useEffect } from 'react';
import { Button } from './ui/button';
import { Alert, AlertDescription } from './ui/alert';
import { Badge } from './ui/badge';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from './ui/card';
import { Key, Pencil, Trash2, RefreshCw, Plus, Smartphone, Monitor } from 'lucide-react';
import { Spinner } from './ui/spinner';
import ConfirmDialog from './ConfirmDialog';
import Modal from './Modal';
import useConfirm from '../hooks/useConfirm';
import PasskeyManagerUtil from '../utils/passkeys';

const PasskeyManager = () => {
  const { isOpen, dialogProps, confirm } = useConfirm();
  const [passkeys, setPasskeys] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [success, setSuccess] = useState(null);
  const [isSupported, setIsSupported] = useState(false);

  // Rename modal state
  const [renameModalOpen, setRenameModalOpen] = useState(false);
  const [renameCredentialId, setRenameCredentialId] = useState(null);
  const [renameDeviceName, setRenameDeviceName] = useState('');
  const [renameLoading, setRenameLoading] = useState(false);

  // Add passkey modal state
  const [addModalOpen, setAddModalOpen] = useState(false);
  const [addDeviceName, setAddDeviceName] = useState('');
  const [addLoading, setAddLoading] = useState(false);

  useEffect(() => {
    checkSupport();
    loadPasskeys();
  }, []);

  const checkSupport = async () => {
    const supported = await PasskeyManagerUtil.isAvailable();
    setIsSupported(supported);
  };

  const loadPasskeys = async () => {
    try {
      setLoading(true);
      setError(null);
      const result = await PasskeyManagerUtil.listPasskeys();
      setPasskeys(result.credentials || []);
    } catch (err) {
      setError(err.message || 'Failed to load passkeys');
    } finally {
      setLoading(false);
    }
  };

  const handleAddPasskey = async () => {
    try {
      setAddLoading(true);
      setError(null);
      setSuccess(null);

      const deviceName = addDeviceName.trim() || undefined;
      await PasskeyManagerUtil.addPasskey(deviceName);

      setSuccess('Passkey added successfully');
      setAddModalOpen(false);
      setAddDeviceName('');
      await loadPasskeys();
    } catch (err) {
      setError(err.message || 'Failed to add passkey');
    } finally {
      setAddLoading(false);
    }
  };

  const handleDeletePasskey = async (credentialId, deviceName) => {
    const confirmed = await confirm({
      title: 'Delete Passkey?',
      message: `Are you sure you want to delete "${deviceName}"? You will no longer be able to sign in with this passkey.`,
      confirmText: 'Delete',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }

    try {
      setLoading(true);
      setError(null);
      setSuccess(null);

      await PasskeyManagerUtil.deletePasskey(credentialId);

      setSuccess('Passkey deleted successfully');
      await loadPasskeys();
    } catch (err) {
      setError(err.message || 'Failed to delete passkey');
    } finally {
      setLoading(false);
    }
  };

  const openRenameModal = (credentialId, currentName) => {
    setRenameCredentialId(credentialId);
    setRenameDeviceName(currentName);
    setRenameModalOpen(true);
  };

  const handleRenamePasskey = async () => {
    if (!renameDeviceName.trim()) {
      setError('Device name cannot be empty');
      return;
    }

    try {
      setRenameLoading(true);
      setError(null);
      setSuccess(null);

      await PasskeyManagerUtil.renamePasskey(renameCredentialId, renameDeviceName.trim());

      setSuccess('Passkey renamed successfully');
      setRenameModalOpen(false);
      setRenameCredentialId(null);
      setRenameDeviceName('');
      await loadPasskeys();
    } catch (err) {
      setError(err.message || 'Failed to rename passkey');
    } finally {
      setRenameLoading(false);
    }
  };

  const formatDate = (dateString) => {
    if (!dateString) return 'Never';
    try {
      const date = new Date(dateString);
      return date.toLocaleDateString('en-US', {
        month: 'short',
        day: 'numeric',
        year: 'numeric',
      });
    } catch {
      return 'Unknown';
    }
  };

  const formatRelativeTime = (dateString) => {
    if (!dateString) return 'Never';
    try {
      const date = new Date(dateString);
      const now = new Date();
      const diffMs = now - date;
      const diffMins = Math.floor(diffMs / 60000);
      const diffHours = Math.floor(diffMs / 3600000);
      const diffDays = Math.floor(diffMs / 86400000);

      if (diffMins < 1) return 'Just now';
      if (diffMins < 60) return `${diffMins} min ago`;
      if (diffHours < 24) return `${diffHours} hour${diffHours > 1 ? 's' : ''} ago`;
      if (diffDays < 7) return `${diffDays} day${diffDays > 1 ? 's' : ''} ago`;
      return formatDate(dateString);
    } catch {
      return 'Unknown';
    }
  };

  const getDeviceIcon = (deviceName) => {
    if (!deviceName) return <Key className="h-5 w-5 text-muted-foreground" />;
    const name = deviceName.toLowerCase();
    if (name.includes('iphone') || name.includes('android') || name.includes('ipad')) {
      return <Smartphone className="h-5 w-5 text-muted-foreground" />;
    }
    if (name.includes('mac') || name.includes('windows') || name.includes('linux')) {
      return <Monitor className="h-5 w-5 text-muted-foreground" />;
    }
    return <Key className="h-5 w-5 text-muted-foreground" />;
  };

  // Clear success message after 5 seconds
  useEffect(() => {
    if (success) {
      const timer = setTimeout(() => setSuccess(null), 5000);
      return () => clearTimeout(timer);
    }
  }, [success]);

  return (
    <>
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <div>
              <CardTitle>Passkeys</CardTitle>
              <CardDescription>
                Passkeys let you sign in securely without a password using your device's biometrics.
              </CardDescription>
            </div>
            <Button
              variant="outline"
              onClick={loadPasskeys}
              disabled={loading}
              size="icon"
              title="Refresh passkeys"
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

          {success && (
            <Alert variant="success" className="mb-6">
              <AlertDescription>{success}</AlertDescription>
            </Alert>
          )}

          <div className="mb-6">
            {loading && passkeys.length === 0 ? (
              <div className="text-center py-8">
                <Spinner size="lg" className="text-primary mx-auto" />
                <p className="text-muted-foreground mt-2">Loading passkeys...</p>
              </div>
            ) : passkeys.length === 0 ? (
              <div className="text-center py-8 bg-muted rounded-lg border-2 border-dashed border-border">
                <Key className="h-12 w-12 text-muted-foreground mx-auto mb-4" />
                <p className="text-muted-foreground mb-4">No passkeys registered yet</p>
                {isSupported && (
                  <Button onClick={() => setAddModalOpen(true)}>
                    <Plus className="h-4 w-4 mr-2" />
                    Add Your First Passkey
                  </Button>
                )}
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
                        Created
                      </th>
                      <th className="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Last Used
                      </th>
                      <th className="px-6 py-3 text-right text-xs font-medium text-muted-foreground uppercase tracking-wider">
                        Actions
                      </th>
                    </tr>
                  </thead>
                  <tbody className="bg-background divide-y divide-border">
                    {passkeys.map((passkey) => (
                      <tr key={passkey.credential_id}>
                        <td className="px-6 py-4 whitespace-nowrap">
                          <div className="flex items-center">
                            <div className="flex-shrink-0">
                              {getDeviceIcon(passkey.device_name)}
                            </div>
                            <div className="ml-3">
                              <div className="text-sm font-medium text-foreground">
                                {passkey.device_name || 'Unnamed Device'}
                              </div>
                            </div>
                          </div>
                        </td>
                        <td className="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                          {formatDate(passkey.created_at)}
                        </td>
                        <td className="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                          {formatRelativeTime(passkey.last_used)}
                        </td>
                        <td className="px-6 py-4 whitespace-nowrap text-right">
                          <div className="flex justify-end gap-2">
                            <Button
                              variant="ghost"
                              size="icon"
                              onClick={() => openRenameModal(passkey.credential_id, passkey.device_name)}
                              title="Rename passkey"
                            >
                              <Pencil className="h-4 w-4" />
                            </Button>
                            {passkeys.length > 1 && (
                              <Button
                                variant="ghost"
                                size="icon"
                                onClick={() => handleDeletePasskey(passkey.credential_id, passkey.device_name)}
                                title="Delete passkey"
                                className="text-error-foreground hover:text-error-foreground hover:bg-error"
                              >
                                <Trash2 className="h-4 w-4" />
                              </Button>
                            )}
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>

          {isSupported && passkeys.length > 0 && (
            <div className="flex justify-start">
              <Button onClick={() => setAddModalOpen(true)}>
                <Plus className="h-4 w-4 mr-2" />
                Add Passkey
              </Button>
            </div>
          )}

          {!isSupported && (
            <Alert variant="warning">
              <AlertDescription>
                Passkeys are not supported on this device or browser. Try using a modern browser like Chrome, Safari, or Edge.
              </AlertDescription>
            </Alert>
          )}

          {passkeys.length === 1 && (
            <Alert variant="info" className="mt-6">
              <AlertDescription>
                <strong>Tip:</strong> Add a second passkey on another device to ensure you can always access your account.
              </AlertDescription>
            </Alert>
          )}
        </CardContent>
      </Card>

      {/* Confirm Dialog for Delete */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />

      {/* Add Passkey Modal */}
      <Modal
        show={addModalOpen}
        onClose={() => {
          setAddModalOpen(false);
          setAddDeviceName('');
        }}
        title="Add Passkey"
      >
        <div className="space-y-4">
          <p className="text-muted-foreground">
            You'll be prompted to use your device's biometrics (fingerprint, face, or PIN) to create a new passkey.
          </p>
          <div>
            <label className="block text-sm font-medium text-foreground mb-2">
              Device Name (optional)
            </label>
            <input
              type="text"
              value={addDeviceName}
              onChange={(e) => setAddDeviceName(e.target.value)}
              placeholder="e.g., My MacBook Pro"
              className="w-full px-3 py-2 border border-border rounded-md bg-background text-foreground focus:outline-none focus:ring-2 focus:ring-primary"
              maxLength={100}
            />
            <p className="text-xs text-muted-foreground mt-1">
              If left empty, we'll auto-detect your device name.
            </p>
          </div>
          <div className="flex justify-end gap-3 pt-4">
            <Button
              variant="outline"
              onClick={() => {
                setAddModalOpen(false);
                setAddDeviceName('');
              }}
              disabled={addLoading}
            >
              Cancel
            </Button>
            <Button onClick={handleAddPasskey} disabled={addLoading}>
              {addLoading ? 'Adding...' : 'Add Passkey'}
            </Button>
          </div>
        </div>
      </Modal>

      {/* Rename Passkey Modal */}
      <Modal
        show={renameModalOpen}
        onClose={() => {
          setRenameModalOpen(false);
          setRenameCredentialId(null);
          setRenameDeviceName('');
        }}
        title="Rename Passkey"
      >
        <div className="space-y-4">
          <div>
            <label className="block text-sm font-medium text-foreground mb-2">
              Device Name
            </label>
            <input
              type="text"
              value={renameDeviceName}
              onChange={(e) => setRenameDeviceName(e.target.value)}
              placeholder="e.g., My iPhone"
              className="w-full px-3 py-2 border border-border rounded-md bg-background text-foreground focus:outline-none focus:ring-2 focus:ring-primary"
              maxLength={100}
            />
          </div>
          <div className="flex justify-end gap-3 pt-4">
            <Button
              variant="outline"
              onClick={() => {
                setRenameModalOpen(false);
                setRenameCredentialId(null);
                setRenameDeviceName('');
              }}
              disabled={renameLoading}
            >
              Cancel
            </Button>
            <Button onClick={handleRenamePasskey} disabled={renameLoading || !renameDeviceName.trim()}>
              {renameLoading ? 'Saving...' : 'Save'}
            </Button>
          </div>
        </div>
      </Modal>
    </>
  );
};

export default PasskeyManager;
