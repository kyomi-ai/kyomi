// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState } from 'react';
import { Eye, EyeOff, Plus } from 'lucide-react';
import { Alert, AlertDescription } from './ui/alert';
import { Button } from './ui/button';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from './ui/card';
import { Input } from './ui/input';
import { Label } from './ui/label';
import { Spinner } from './ui/spinner';

export default function PasswordManager({ user, apiClient, onPasswordUpdate }) {
  const [isChangingPassword, setIsChangingPassword] = useState(false);
  const [isSettingPassword, setIsSettingPassword] = useState(false);
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [showCurrentPassword, setShowCurrentPassword] = useState(false);
  const [showNewPassword, setShowNewPassword] = useState(false);
  const [showConfirmPassword, setShowConfirmPassword] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [success, setSuccess] = useState('');

  const hasPassword = user?.has_password;

  const resetForm = () => {
    setCurrentPassword('');
    setNewPassword('');
    setConfirmPassword('');
    setError('');
    setSuccess('');
    setShowCurrentPassword(false);
    setShowNewPassword(false);
    setShowConfirmPassword(false);
  };

  const handleCancel = () => {
    setIsChangingPassword(false);
    setIsSettingPassword(false);
    resetForm();
  };

  const handleSubmit = async (e) => {
    e.preventDefault();
    setError('');
    setSuccess('');

    if (newPassword !== confirmPassword) {
      setError('New passwords do not match');
      return;
    }

    if (newPassword.length < 8) {
      setError('Password must be at least 8 characters long');
      return;
    }

    setLoading(true);

    try {
      let endpoint, payload;

      if (hasPassword) {
        endpoint = '/api/v1/auth/change-password';
        payload = {
          current_password: currentPassword,
          new_password: newPassword
        };
      } else {
        endpoint = '/api/v1/auth/set-password';
        payload = {
          new_password: newPassword
        };
      }

      const response = await apiClient.post(endpoint, payload);

      if (response.data) {
        setSuccess(response.data.message);
        resetForm();
        setIsChangingPassword(false);
        setIsSettingPassword(false);

        if (onPasswordUpdate) {
          onPasswordUpdate();
        }
      }

    } catch (error) {
      setError(error.response?.data?.detail || 'Password operation failed');
    } finally {
      setLoading(false);
    }
  };

  if (isChangingPassword || isSettingPassword) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>{hasPassword ? 'Change Password' : 'Set Password'}</CardTitle>
          <CardDescription>
            {hasPassword ? 'Enter your current password and choose a new one' : 'Create a password for your account'}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="space-y-4">
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

            {hasPassword && (
              <div className="space-y-2">
                <Label htmlFor="currentPassword">Current Password</Label>
                <div className="relative">
                  <Input
                    type={showCurrentPassword ? 'text' : 'password'}
                    id="currentPassword"
                    value={currentPassword}
                    onChange={(e) => setCurrentPassword(e.target.value)}
                    placeholder="Enter current password"
                    className="pr-12"
                    required
                  />
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    onClick={() => setShowCurrentPassword(!showCurrentPassword)}
                    className="absolute right-1 top-1/2 -translate-y-1/2 h-7 w-7"
                  >
                    {showCurrentPassword ? <EyeOff size={16} /> : <Eye size={16} />}
                  </Button>
                </div>
              </div>
            )}

            <div className="space-y-2">
              <Label htmlFor="newPassword">New Password</Label>
              <div className="relative">
                <Input
                  type={showNewPassword ? 'text' : 'password'}
                  id="newPassword"
                  value={newPassword}
                  onChange={(e) => setNewPassword(e.target.value)}
                  placeholder="Enter new password (min 8 characters)"
                  className="pr-12"
                  required
                  minLength={8}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  onClick={() => setShowNewPassword(!showNewPassword)}
                  className="absolute right-1 top-1/2 -translate-y-1/2 h-7 w-7"
                >
                  {showNewPassword ? <EyeOff size={16} /> : <Eye size={16} />}
                </Button>
              </div>
            </div>

            <div className="space-y-2">
              <Label htmlFor="confirmPassword">Confirm New Password</Label>
              <div className="relative">
                <Input
                  type={showConfirmPassword ? 'text' : 'password'}
                  id="confirmPassword"
                  value={confirmPassword}
                  onChange={(e) => setConfirmPassword(e.target.value)}
                  placeholder="Confirm new password"
                  className="pr-12"
                  required
                  minLength={8}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  onClick={() => setShowConfirmPassword(!showConfirmPassword)}
                  className="absolute right-1 top-1/2 -translate-y-1/2 h-7 w-7"
                >
                  {showConfirmPassword ? <EyeOff size={16} /> : <Eye size={16} />}
                </Button>
              </div>
            </div>

            <div className="flex gap-3 pt-2">
              <Button type="submit" disabled={loading}>
                {loading ? (
                  <>
                    <Spinner className="text-white" />
                    <span>{hasPassword ? 'Changing...' : 'Setting...'}</span>
                  </>
                ) : (
                  <span>{hasPassword ? 'Change Password' : 'Set Password'}</span>
                )}
              </Button>
              <Button
                type="button"
                variant="outline"
                onClick={handleCancel}
                disabled={loading}
              >
                Cancel
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card>
      <CardHeader>
        <div className="flex items-center justify-between">
          <div>
            <CardTitle>Password</CardTitle>
            <CardDescription>
              {hasPassword
                ? 'Change your account password'
                : 'Add password authentication to your account'
              }
            </CardDescription>
          </div>
          <Button
            onClick={() => hasPassword ? setIsChangingPassword(true) : setIsSettingPassword(true)}
          >
            {hasPassword ? (
              <span>Change Password</span>
            ) : (
              <>
                <Plus size={16} />
                <span>Set Password</span>
              </>
            )}
          </Button>
        </div>
      </CardHeader>
    </Card>
  );
}
