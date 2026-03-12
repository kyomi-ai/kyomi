// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { ArrowRightLeft, CheckCircle, AlertCircle, Building2, User } from 'lucide-react';
import { Spinner } from '../components/ui/spinner';
import { Button } from '../components/ui/button';
import { Alert } from '../components/ui/alert';
import { toast } from '../lib/toast';
import apiClient from '../api/apiClient';

export default function AcceptOwnershipPage() {
  const { transferId } = useParams();
  const navigate = useNavigate();

  const [status, setStatus] = useState('loading'); // loading, ready, processing, success, error
  const [transfer, setTransfer] = useState(null);
  const [errorMessage, setErrorMessage] = useState('');

  useEffect(() => {
    if (!transferId) {
      setStatus('error');
      setErrorMessage('No transfer ID provided');
      return;
    }

    fetchTransferDetails();
  }, [transferId]);

  const fetchTransferDetails = async () => {
    try {
      setStatus('loading');
      const response = await apiClient.get('/api/v1/workspaces/ownership/transfers');

      // Find the transfer matching this ID
      const transferData = response.data.find(t => t.transfer_id === transferId);

      if (!transferData) {
        setStatus('error');
        setErrorMessage('Transfer request not found or has expired');
        return;
      }

      if (transferData.status !== 'pending') {
        setStatus('error');
        setErrorMessage(`This transfer request has already been ${transferData.status}`);
        return;
      }

      setTransfer(transferData);
      setStatus('ready');
    } catch (error) {
      setStatus('error');
      setErrorMessage(error.response?.data?.detail || error.message || 'Failed to load transfer details');
    }
  };

  const handleAccept = async () => {
    try {
      setStatus('processing');
      await apiClient.post(`/api/v1/workspaces/ownership/transfer/${transferId}/accept`);

      setStatus('success');
      toast.success('Ownership transfer accepted! You are now the workspace owner.', { duration: 6000 });

      // Redirect to settings page after 3 seconds
      setTimeout(() => {
        navigate('/settings/team', { replace: true });
      }, 3000);
    } catch (error) {
      setStatus('ready');
      toast.error('Failed to accept transfer: ' + (error.response?.data?.detail || error.message));
    }
  };

  const handleDecline = async () => {
    try {
      setStatus('processing');
      await apiClient.post(`/api/v1/workspaces/ownership/transfer/${transferId}/decline`);

      toast.success('Transfer request declined');

      // Redirect to dashboard after 2 seconds
      setTimeout(() => {
        navigate('/', { replace: true });
      }, 2000);
    } catch (error) {
      setStatus('ready');
      toast.error('Failed to decline transfer: ' + (error.response?.data?.detail || error.message));
    }
  };

  const getStatusIcon = () => {
    switch (status) {
      case 'loading':
        return (
          <Spinner size="xl" className="text-primary mx-auto" />
        );
      case 'ready':
        return <ArrowRightLeft className="h-16 w-16 text-primary mx-auto" />;
      case 'processing':
        return (
          <Spinner size="xl" className="text-primary mx-auto" />
        );
      case 'success':
        return <CheckCircle className="h-16 w-16 text-success mx-auto" />;
      case 'error':
        return <AlertCircle className="h-16 w-16 text-destructive mx-auto" />;
    }
  };

  return (
    <div className="min-h-screen bg-gradient-to-br from-background via-muted/30 to-muted/50 flex items-center justify-center p-4">
      <div className="w-full max-w-2xl">
        <div className="bg-card/80 backdrop-blur-sm rounded-2xl shadow-xl border border-border overflow-hidden">
          {/* Header */}
          <div className="p-8 text-center">
            <div className="w-20 h-20 bg-primary/10 rounded-2xl flex items-center justify-center mx-auto mb-6">
              <ArrowRightLeft className="text-primary" size={32} />
            </div>
            <h1 className="text-2xl font-bold text-foreground mb-2">Workspace Ownership Transfer</h1>
            <p className="text-muted-foreground">
              {status === 'loading' && 'Loading transfer details...'}
              {status === 'ready' && 'You have been offered workspace ownership'}
              {status === 'processing' && 'Processing your response...'}
              {status === 'success' && 'Transfer accepted successfully!'}
              {status === 'error' && 'Transfer request unavailable'}
            </p>
          </div>

          {/* Content Section */}
          <div className="px-8 pb-8">
            {/* Loading State */}
            {status === 'loading' && (
              <div className="text-center py-8">
                {getStatusIcon()}
                <p className="text-muted-foreground mt-4">Please wait...</p>
              </div>
            )}

            {/* Error State */}
            {status === 'error' && (
              <div className="space-y-4">
                <Alert variant="error">
                  <AlertCircle className="h-4 w-4" />
                  <div className="ml-2">
                    <strong>Error</strong>
                    <p className="mt-1">{errorMessage}</p>
                  </div>
                </Alert>
                <div className="text-center">
                  <Button
                    variant="outline"
                    onClick={() => navigate('/', { replace: true })}
                  >
                    Go to Dashboard
                  </Button>
                </div>
              </div>
            )}

            {/* Ready State - Show Transfer Details */}
            {status === 'ready' && transfer && (
              <div className="space-y-6">
                {/* Transfer Info */}
                <div className="bg-muted/50 rounded-xl p-6 border border-border">
                  <div className="space-y-4">
                    <div className="flex items-start gap-3">
                      <Building2 className="h-5 w-5 text-muted-foreground mt-0.5" />
                      <div className="flex-1">
                        <div className="text-sm text-muted-foreground">Workspace</div>
                        <div className="text-lg font-semibold text-foreground">
                          {transfer.workspace_name || 'Unnamed Workspace'}
                        </div>
                      </div>
                    </div>

                    <div className="flex items-start gap-3">
                      <User className="h-5 w-5 text-muted-foreground mt-0.5" />
                      <div className="flex-1">
                        <div className="text-sm text-muted-foreground">Current Owner</div>
                        <div className="text-lg font-medium text-foreground">
                          {transfer.from_user_email}
                        </div>
                      </div>
                    </div>

                    <div className="pt-2 border-t border-border">
                      <div className="text-sm text-muted-foreground">Expires</div>
                      <div className="text-foreground">
                        {new Date(transfer.expires_at).toLocaleDateString()} at {new Date(transfer.expires_at).toLocaleTimeString()}
                      </div>
                    </div>
                  </div>
                </div>

                {/* Warning */}
                <Alert variant="warning">
                  <AlertCircle className="h-4 w-4" />
                  <div className="ml-2">
                    <strong>Important</strong>
                    <p className="mt-1">
                      By accepting ownership, you will become the workspace owner with full control over billing,
                      settings, and member management. The current owner will be downgraded to a workspace admin.
                    </p>
                  </div>
                </Alert>

                {/* What you'll be able to do */}
                <div className="bg-muted/50 rounded-xl p-6 border border-border">
                  <h3 className="font-semibold text-foreground mb-3">As the workspace owner, you will be able to:</h3>
                  <ul className="space-y-2 text-sm text-muted-foreground">
                    <li className="flex items-start gap-2">
                      <CheckCircle className="h-4 w-4 text-success mt-0.5 flex-shrink-0" />
                      <span>Manage workspace billing and subscription</span>
                    </li>
                    <li className="flex items-start gap-2">
                      <CheckCircle className="h-4 w-4 text-success mt-0.5 flex-shrink-0" />
                      <span>Delete the workspace</span>
                    </li>
                    <li className="flex items-start gap-2">
                      <CheckCircle className="h-4 w-4 text-success mt-0.5 flex-shrink-0" />
                      <span>Add and remove workspace members</span>
                    </li>
                    <li className="flex items-start gap-2">
                      <CheckCircle className="h-4 w-4 text-success mt-0.5 flex-shrink-0" />
                      <span>Transfer ownership to another member</span>
                    </li>
                    <li className="flex items-start gap-2">
                      <CheckCircle className="h-4 w-4 text-success mt-0.5 flex-shrink-0" />
                      <span>Configure workspace settings and integrations</span>
                    </li>
                  </ul>
                </div>

                {/* Action Buttons */}
                <div className="flex gap-3 justify-end">
                  <Button
                    variant="outline"
                    onClick={handleDecline}
                    className="px-6"
                  >
                    Decline
                  </Button>
                  <Button
                    variant="default"
                    onClick={handleAccept}
                    className="px-6"
                  >
                    Accept Ownership
                  </Button>
                </div>
              </div>
            )}

            {/* Processing State */}
            {status === 'processing' && (
              <div className="text-center py-8">
                {getStatusIcon()}
                <p className="text-muted-foreground mt-4">Processing...</p>
              </div>
            )}

            {/* Success State */}
            {status === 'success' && (
              <div className="space-y-4">
                <div className="text-center py-8">
                  {getStatusIcon()}
                  <div className="mt-4 font-semibold text-success">Success!</div>
                  <p className="text-muted-foreground mt-2">
                    You are now the owner of this workspace.
                  </p>
                </div>
                <Alert variant="success">
                  <CheckCircle className="h-4 w-4" />
                  <div className="ml-2">
                    <p>Redirecting to workspace settings in 3 seconds...</p>
                  </div>
                </Alert>
                <div className="text-center">
                  <Button
                    variant="default"
                    onClick={() => navigate('/settings/team', { replace: true })}
                  >
                    Go to Settings Now
                  </Button>
                </div>
              </div>
            )}
          </div>
        </div>

        {/* Footer */}
        <div className="text-center mt-8">
          <p className="text-sm text-muted-foreground">
            Need help? Contact{' '}
            <a href="mailto:support@kyomi.dev" className="text-primary hover:underline">
              support@kyomi.dev
            </a>
          </p>
        </div>
      </div>
    </div>
  );
}
