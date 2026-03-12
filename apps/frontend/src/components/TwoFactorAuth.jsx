// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { Shield, Copy } from 'lucide-react';
import { API_CONFIG } from '../config/api.js';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';
import { Alert, AlertDescription } from './ui/alert';
import { Button } from './ui/button';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from './ui/card';
import { Input } from './ui/input';
import { Label } from './ui/label';
import { StatusBadge } from './ui/status-badge';
import ConfirmDialog from './ConfirmDialog';
import useConfirm from '../hooks/useConfirm';

export default function TwoFactorAuth({ authService, onStatusChange, apiClient }) {
  const [totpStatus, setTotpStatus] = useState(null);
  const [loading, setLoading] = useState(true);
  const [setupData, setSetupData] = useState(null);
  const [verificationCode, setVerificationCode] = useState('');
  const [setupLoading, setSetupLoading] = useState(false);
  const [enableLoading, setEnableLoading] = useState(false);
  const [disableLoading, setDisableLoading] = useState(false);
  const [error, setError] = useState('');
  const [success, setSuccess] = useState('');
  const [showSetup, setShowSetup] = useState(false);
  const { isOpen, dialogProps, confirm } = useConfirm();

  useEffect(() => {
    loadTotpStatus();
  }, [authService]);

  const loadTotpStatus = async () => {
    if (!apiClient) return;

    try {
      setLoading(true);
      const response = await apiClient.get(API_CONFIG.endpoints.totp.status);
      const data = response.data;
      setTotpStatus(data);
      if (onStatusChange) {
        onStatusChange(data.enabled);
      }
    } catch (err) {
      setError('Failed to load 2FA status');
    } finally {
      setLoading(false);
    }
  };

  const handleSetup2FA = async () => {
    try {
      setSetupLoading(true);
      setError('');
      setSuccess('');

      if (!apiClient) {
        setError('API client not available');
        return;
      }

      const response = await apiClient.post(API_CONFIG.endpoints.totp.setup);
      const data = response.data;
      setSetupData(data);
      setShowSetup(true);
    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to setup 2FA');
    } finally {
      setSetupLoading(false);
    }
  };

  const handleEnable2FA = async () => {
    if (!verificationCode.trim()) {
      setError('Please enter the verification code');
      return;
    }

    try {
      setEnableLoading(true);
      setError('');

      const response = await apiClient.post(API_CONFIG.endpoints.totp.enable, {
        verification_code: verificationCode
      });

      setSuccess(response.data.message);
      setShowSetup(false);
      setSetupData(null);
      setVerificationCode('');
      await loadTotpStatus();
    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to enable 2FA');
    } finally {
      setEnableLoading(false);
    }
  };

  const handleDisable2FA = async () => {
    const confirmed = await confirm({
      title: 'Disable Two-Factor Authentication?',
      message: 'Are you sure you want to disable 2FA? This will make your account less secure.',
      confirmText: 'Disable 2FA',
      variant: 'destructive'
    });

    if (!confirmed) return;

    try {
      setDisableLoading(true);
      setError('');

      const response = await apiClient.post(API_CONFIG.endpoints.totp.disable);

      setSuccess(response.data.message);
      await loadTotpStatus();
    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to disable 2FA');
    } finally {
      setDisableLoading(false);
    }
  };

  const copyToClipboard = async (text) => {
    try {
      await navigator.clipboard.writeText(text);
      setSuccess('Copied to clipboard!');
      setTimeout(() => setSuccess(''), 2000);
    } catch (err) {
    }
  };

  if (loading) {
    return (
      <Card>
        <CardContent className="pt-6">
          <div className="animate-pulse">
            <div className="h-4 bg-muted rounded mb-2"></div>
            <div className="h-4 bg-muted rounded w-2/3"></div>
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <>
      {error && (
        <Alert variant="error">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {success && (
        <Alert variant="success">
          <AlertDescription>{success}</AlertDescription>
        </Alert>
      )}

      {!showSetup && (
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <div>
                <CardTitle>
                  Two-Factor Authentication
                  {totpStatus?.enabled && (
                    <StatusBadge variant="success" className="ml-3 align-middle">Enabled</StatusBadge>
                  )}
                </CardTitle>
                <CardDescription>
                  {totpStatus?.enabled
                    ? 'Protect your account with TOTP codes from authenticator apps'
                    : 'Add an extra layer of security with time-based codes'
                  }
                </CardDescription>
              </div>
              <div className="flex items-center space-x-2">
                {totpStatus?.enabled ? (
                  <Button
                    variant="outline"
                    onClick={handleDisable2FA}
                    disabled={disableLoading}
                  >
                    {disableLoading ? 'Disabling...' : 'Disable 2FA'}
                  </Button>
                ) : (
                  <Button
                    onClick={handleSetup2FA}
                    disabled={setupLoading}
                  >
                    <Shield size={16} />
                    <span>{setupLoading ? 'Setting up...' : 'Setup 2FA'}</span>
                  </Button>
                )}
              </div>
            </div>
          </CardHeader>

          {!totpStatus?.enabled && (
            <CardContent>
              <Alert variant="info">
                <AlertDescription>
                  <p className="font-medium mb-1">Why enable 2FA?</p>
                  <ul className="list-disc list-inside space-y-1">
                    <li>Adds an extra layer of security to your account</li>
                    <li>Works with Google Authenticator, Authy, and other TOTP apps</li>
                    <li>Protects your account even if your password is compromised</li>
                  </ul>
                </AlertDescription>
              </Alert>
            </CardContent>
          )}
        </Card>
      )}

      {showSetup && setupData && (
        <Card>
          <CardHeader>
            <CardTitle>Setup Two-Factor Authentication</CardTitle>
            <CardDescription>
              Scan the QR code with your authenticator app (Google Authenticator, Authy, etc.) or enter the key manually
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            <div className="flex flex-col items-center space-y-4">
              <div className="p-4 bg-background border border-border rounded-lg">
                <img
                  src={setupData.qr_code}
                  alt="2FA QR Code"
                  className="w-48 h-48"
                />
              </div>

              <div className="w-full max-w-md">
                <Label className="mb-2">Or enter this key manually:</Label>
                <div className="flex items-center space-x-2">
                  <Input
                    type="text"
                    value={setupData.secret}
                    readOnly
                    className="flex-1 font-mono"
                  />
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => copyToClipboard(setupData.secret)}
                        aria-label="Copy to clipboard"
                      >
                        <Copy size={16} />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>Copy to clipboard</TooltipContent>
                  </Tooltip>
                </div>
              </div>
            </div>

            <div className="border-t border-border pt-6">
              <div className="max-w-md mx-auto space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="verification-code">
                    Enter the 6-digit code from your authenticator app:
                  </Label>
                  <Input
                    id="verification-code"
                    type="text"
                    value={verificationCode}
                    onChange={(e) => setVerificationCode(e.target.value)}
                    placeholder="000000"
                    maxLength="6"
                    className="text-center text-lg font-mono tracking-wider"
                  />
                </div>

                <div className="flex space-x-3">
                  <Button
                    variant="outline"
                    className="flex-1"
                    onClick={() => {
                      setShowSetup(false);
                      setSetupData(null);
                      setVerificationCode('');
                      setError('');
                    }}
                  >
                    Cancel
                  </Button>
                  <Button
                    className="flex-1"
                    onClick={handleEnable2FA}
                    disabled={enableLoading || verificationCode.length !== 6}
                  >
                    {enableLoading ? 'Enabling...' : 'Enable 2FA'}
                  </Button>
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
    </>
  );
}
