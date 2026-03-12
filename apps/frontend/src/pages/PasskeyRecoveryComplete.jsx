// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useNavigate, useSearchParams, Link } from 'react-router-dom';
import { startRegistration } from '@simplewebauthn/browser';
import apiClient from '../api/apiClient';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '../components/ui/card';
import { Button } from '../components/ui/button';
import { Alert, AlertTitle, AlertDescription } from '../components/ui/alert';
import { Spinner } from '../components/ui/spinner';

/**
 * PasskeyRecoveryComplete - Handles the recovery link click
 *
 * Route: /auth/recover-passkey/complete?token=xxx
 *
 * Flow:
 * 1. User clicks recovery link from email
 * 2. This page verifies the recovery token with backend
 * 3. On success, shows "Create New Passkey" button
 * 4. User creates new passkey via WebAuthn (using recovery endpoint)
 * 5. Redirect to login with success message
 */
export default function PasskeyRecoveryComplete() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const token = searchParams.get('token');

  // States: 'verifying', 'ready', 'creating', 'success', 'error'
  const [status, setStatus] = useState('verifying');
  const [challengeData, setChallengeData] = useState(null);
  const [error, setError] = useState('');
  const [userInfo, setUserInfo] = useState(null);

  useEffect(() => {
    if (!token) {
      setStatus('error');
      setError('Missing recovery token. Please use the link from your email.');
      return;
    }

    verifyRecoveryToken();
  }, [token]);

  const verifyRecoveryToken = async () => {
    try {
      const response = await apiClient.post('/api/v1/auth/passkeys/recovery/verify', {
        token
      });

      const data = response.data;

      if (data.status === 'ready_for_passkey') {
        setChallengeData({
          challenge_id: data.challenge_id,
          options: data.options
        });
        setUserInfo(data.user);
        setStatus('ready');
      } else {
        setStatus('error');
        setError('Unexpected response from server. Please try again.');
      }
    } catch (err) {
      setStatus('error');
      setError(
        err.response?.data?.detail ||
        'Invalid or expired recovery link. Please request a new one.'
      );
    }
  };

  const handleCreatePasskey = async () => {
    if (!challengeData) {
      setError('Missing challenge data. Please refresh the page.');
      return;
    }

    setStatus('creating');
    setError('');

    try {
      // Create credential using WebAuthn
      const registrationResponse = await startRegistration({
        optionsJSON: challengeData.options.publicKey || challengeData.options
      });

      // Complete recovery registration on server (uses recovery-specific endpoint)
      await apiClient.post('/api/v1/auth/passkeys/recovery/register', {
        challenge_id: challengeData.challenge_id,
        credential: registrationResponse,
        device_name: getDeviceName()
      });

      setStatus('success');

      // Redirect to login after a short delay
      setTimeout(() => {
        navigate('/login', {
          state: { message: 'New passkey created successfully! Please sign in.' }
        });
      }, 2000);
    } catch (err) {
      setStatus('ready'); // Allow retry

      // Handle specific WebAuthn errors
      if (err.name === 'InvalidStateError') {
        setError('A passkey already exists for this device. Please try with a different device.');
      } else if (err.name === 'NotAllowedError') {
        setError('Passkey creation was cancelled or timed out. Please try again.');
      } else if (err.name === 'AbortError') {
        setError('Passkey creation was cancelled. Please try again.');
      } else if (err.name === 'NotSupportedError') {
        setError('Your device does not support passkeys. Please contact support for alternative recovery options.');
      } else {
        setError(
          err.response?.data?.detail ||
          err.message ||
          'Failed to create passkey. Please try again.'
        );
      }
    }
  };

  // Get a user-friendly device name
  const getDeviceName = () => {
    const ua = navigator.userAgent;
    const platform = navigator.platform || 'Unknown';

    const getBrowser = () => {
      if (/Chrome/.test(ua) && !/Edge/.test(ua)) return 'Chrome';
      if (/Firefox/.test(ua)) return 'Firefox';
      if (/Safari/.test(ua) && !/Chrome/.test(ua)) return 'Safari';
      if (/Edge/.test(ua)) return 'Edge';
      return 'Browser';
    };

    if (/iPhone|iPad|iPod/.test(ua)) return `iPhone/iPad (${getBrowser()})`;
    if (/Android/.test(ua)) return `Android (${getBrowser()})`;
    if (/Mac/.test(platform)) return `Mac (${getBrowser()})`;
    if (/Win/.test(platform)) return `Windows (${getBrowser()})`;
    if (/Linux/.test(platform)) return `Linux (${getBrowser()})`;

    return `${platform} (${getBrowser()})`;
  };

  // Render loading state
  if (status === 'verifying') {
    return (
      <div className="min-h-screen bg-background flex items-center justify-center p-4">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6">
            <div className="text-center space-y-4">
              <Spinner size="xl" className="text-primary mx-auto" />
              <p className="text-muted-foreground">Verifying recovery link...</p>
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  // Render error state
  if (status === 'error') {
    return (
      <div className="min-h-screen bg-background flex items-center justify-center p-4">
        <Card className="w-full max-w-md">
          <CardHeader className="text-center">
            <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-error/10 mx-auto mb-4">
              <svg className="w-8 h-8 text-error-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
              </svg>
            </div>
            <CardTitle className="text-xl">Recovery Link Invalid</CardTitle>
            <CardDescription className="text-error-foreground">
              {error}
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <Link to="/auth/recover-passkey">
              <Button variant="default" className="w-full">
                Request New Recovery Link
              </Button>
            </Link>
            <Link to="/login">
              <Button variant="outline" className="w-full">
                Back to Login
              </Button>
            </Link>
          </CardContent>
        </Card>
      </div>
    );
  }

  // Render success state
  if (status === 'success') {
    return (
      <div className="min-h-screen bg-background flex items-center justify-center p-4">
        <Card className="w-full max-w-md">
          <CardHeader className="text-center">
            <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-success/10 mx-auto mb-4">
              <svg className="w-8 h-8 text-success-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
              </svg>
            </div>
            <CardTitle className="text-xl">New Passkey Created!</CardTitle>
            <CardDescription>
              Your account is recovered. Redirecting you to sign in...
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Spinner size="lg" className="text-primary mx-auto" />
          </CardContent>
        </Card>
      </div>
    );
  }

  // Render ready state - main content
  return (
    <div className="min-h-screen bg-background flex items-center justify-center p-4">
      <Card className="w-full max-w-md">
        <CardHeader className="text-center">
          <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-4">
            <svg className="w-8 h-8 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 7a2 2 0 012 2m4 0a6 6 0 01-7.743 5.743L11 17H9v2H7v2H4a1 1 0 01-1-1v-2.586a1 1 0 01.293-.707l5.964-5.964A6 6 0 1121 9z" />
            </svg>
          </div>
          <CardTitle className="text-xl">Create New Passkey</CardTitle>
          <CardDescription>
            Your identity is verified. Create a new passkey to regain access to your account.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          {userInfo && (
            <div className="text-center text-sm text-muted-foreground">
              Recovering account: <span className="font-medium text-foreground">{userInfo.email}</span>
            </div>
          )}

          {error && (
            <Alert variant="destructive">
              <AlertTitle>Error</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          <Button
            onClick={handleCreatePasskey}
            disabled={status === 'creating'}
            className="w-full"
            size="lg"
          >
            {status === 'creating' ? (
              <div className="flex items-center justify-center space-x-2">
                <Spinner className="text-white" />
                <span>Creating passkey...</span>
              </div>
            ) : (
              <div className="flex items-center justify-center space-x-2">
                <span>Create New Passkey</span>
              </div>
            )}
          </Button>

          <p className="text-xs text-center text-muted-foreground">
            You will be prompted to use your fingerprint, face, or security key.
          </p>

          <div className="text-center pt-2 border-t border-border">
            <p className="text-xs text-muted-foreground mt-4">
              This recovery session is valid for 15 minutes. After creating your passkey, your old passkeys will remain active.
            </p>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
