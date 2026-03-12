// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useNavigate, useSearchParams, Link } from 'react-router-dom';
import apiClient from '../api/apiClient';
import { useAuth } from '../context/AuthContext';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '../components/ui/card';
import { Button } from '../components/ui/button';
import { Input } from '../components/ui/input';
import { Alert, AlertTitle, AlertDescription } from '../components/ui/alert';
import { Label } from '../components/ui/label';
import { Spinner } from '../components/ui/spinner';

/**
 * AccountRecoveryComplete - Handles the recovery link click for password reset
 *
 * Route: /account/recover/complete?token=xxx
 *
 * Flow:
 * 1. User clicks recovery link from email
 * 2. This page verifies the recovery token with backend
 * 3. On success, shows "Set new password" form
 * 4. User sets a new password
 * 5. On success, user is logged in and redirected to home
 */
export default function AccountRecoveryComplete() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const { refreshUser } = useAuth();
  const token = searchParams.get('token');

  // States: 'verifying', 'ready', 'submitting', 'success', 'error'
  const [status, setStatus] = useState('verifying');
  const [recoverySessionId, setRecoverySessionId] = useState(null);
  const [hasPasskeys, setHasPasskeys] = useState(false);
  const [error, setError] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');

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
      const response = await apiClient.post('/api/v1/auth/recovery/verify', { token });
      const data = response.data;

      setRecoverySessionId(data.recovery_session_id);
      setHasPasskeys(data.has_passkeys || false);
      setStatus('ready');
    } catch (err) {
      setStatus('error');
      setError(
        err.response?.data?.detail ||
        'Invalid or expired recovery link. Please request a new one.'
      );
    }
  };

  const handleSetPassword = async (e) => {
    e.preventDefault();
    setError('');

    // Validate passwords match
    if (newPassword !== confirmPassword) {
      setError('Passwords do not match.');
      return;
    }

    // Validate password strength
    if (newPassword.length < 8) {
      setError('Password must be at least 8 characters long.');
      return;
    }

    setStatus('submitting');

    try {
      const response = await apiClient.post('/api/v1/auth/recovery/set-password', {
        recovery_session_id: recoverySessionId,
        new_password: newPassword
      });

      // The backend sets auth cookies during set-password.
      // Sync the auth context by refreshing the user profile.
      await refreshUser();

      setStatus('success');

      // Redirect to home after a short delay
      setTimeout(() => {
        navigate('/', { replace: true });
      }, 2000);
    } catch (err) {
      setStatus('ready'); // Allow retry
      setError(
        err.response?.data?.detail ||
        'Failed to set password. Please try again.'
      );
    }
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
            <Link to="/account/recover">
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
            <CardTitle className="text-xl">Password Updated!</CardTitle>
            <CardDescription>
              Your password has been set successfully. Redirecting you to the app...
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Spinner size="lg" className="text-primary mx-auto" />
          </CardContent>
        </Card>
      </div>
    );
  }

  // Render ready state - password form
  return (
    <div className="min-h-screen bg-background flex items-center justify-center p-4">
      <Card className="w-full max-w-md">
        <CardHeader className="text-center">
          <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-4">
            <svg className="w-8 h-8 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 7a2 2 0 012 2m4 0a6 6 0 01-7.743 5.743L11 17H9v2H7v2H4a1 1 0 01-1-1v-2.586a1 1 0 01.293-.707l5.964-5.964A6 6 0 1121 9z" />
            </svg>
          </div>
          <CardTitle className="text-xl">Set New Password</CardTitle>
          <CardDescription>
            Your identity is verified. Choose a new password for your account.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSetPassword} className="space-y-4">
            {hasPasskeys && (
              <Alert>
                <AlertTitle>Passkeys available</AlertTitle>
                <AlertDescription>
                  Your account also has passkeys registered. You can continue to use them after setting a new password.
                </AlertDescription>
              </Alert>
            )}

            {error && (
              <Alert variant="error">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <div className="space-y-2">
              <Label htmlFor="new-password">New password</Label>
              <Input
                id="new-password"
                type="password"
                value={newPassword}
                onChange={(e) => setNewPassword(e.target.value)}
                placeholder="At least 8 characters"
                autoComplete="new-password"
                autoFocus
                required
                minLength={8}
                className="h-11"
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="confirm-password">Confirm password</Label>
              <Input
                id="confirm-password"
                type="password"
                value={confirmPassword}
                onChange={(e) => setConfirmPassword(e.target.value)}
                placeholder="Re-enter your password"
                autoComplete="new-password"
                required
                minLength={8}
                className="h-11"
              />
            </div>

            <Button
              type="submit"
              disabled={status === 'submitting' || !newPassword || !confirmPassword}
              className="w-full"
              size="lg"
            >
              {status === 'submitting' ? (
                <div className="flex items-center justify-center space-x-2">
                  <Spinner className="text-white" />
                  <span>Setting password...</span>
                </div>
              ) : (
                'Set New Password'
              )}
            </Button>

            <div className="text-center pt-2 border-t border-border">
              <p className="text-xs text-muted-foreground mt-4">
                This recovery session is valid for 15 minutes.
              </p>
            </div>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
