// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { Button } from '../ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../ui/select';
import Modal from '../Modal';
import { Alert } from '../ui/alert';
import { toast } from '../../lib/toast';

export default function TransferOwnershipModal({
  isOpen,
  onClose,
  members,
  currentUserId,
  workspaceName,
  apiClient,
  onTransferInitiated
}) {
  const [selectedUserId, setSelectedUserId] = useState('');
  const [confirmationInput, setConfirmationInput] = useState('');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [step, setStep] = useState(1); // 1 = select user, 2 = confirm

  // Filter out current user and get eligible members
  const eligibleMembers = members.filter(m => m.user_id !== currentUserId && !m.is_owner);

  const handleNext = () => {
    if (!selectedUserId) {
      toast.error('Please select a member to transfer ownership to');
      return;
    }
    setStep(2);
  };

  const handleBack = () => {
    setStep(1);
    setConfirmationInput('');
  };

  const handleTransfer = async () => {
    if (confirmationInput !== workspaceName) {
      toast.error('Workspace name does not match');
      return;
    }

    setIsSubmitting(true);
    try {
      const response = await apiClient.post('/api/v1/workspaces/ownership/transfer', {
        to_user_id: selectedUserId
      });

      const selectedMember = eligibleMembers.find(m => m.user_id === selectedUserId);
      toast.success(
        `Ownership transfer request sent to ${selectedMember.email}. They will receive a notification to accept or decline.`,
        { duration: 6000 }
      );

      if (onTransferInitiated) {
        onTransferInitiated(response.data);
      }

      // Reset and close
      setStep(1);
      setSelectedUserId('');
      setConfirmationInput('');
      onClose();
    } catch (error) {
      toast.error('Failed to initiate transfer: ' + (error.response?.data?.detail || error.message));
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleClose = () => {
    setStep(1);
    setSelectedUserId('');
    setConfirmationInput('');
    onClose();
  };

  const selectedMember = eligibleMembers.find(m => m.user_id === selectedUserId);

  return (
    <Modal
      show={isOpen}
      onClose={handleClose}
      title="Transfer Workspace Ownership"
      size="md"
      footer={
        step === 1 ? (
          <>
            <Button variant="outline" onClick={handleClose}>
              Cancel
            </Button>
            <Button
              variant="default"
              onClick={handleNext}
              disabled={!selectedUserId}
            >
              Next
            </Button>
          </>
        ) : (
          <>
            <Button variant="outline" onClick={handleBack}>
              Back
            </Button>
            <Button
              variant="destructive"
              onClick={handleTransfer}
              disabled={isSubmitting || confirmationInput !== workspaceName}
            >
              {isSubmitting ? 'Transferring...' : 'Transfer Ownership'}
            </Button>
          </>
        )
      }
    >
      {step === 1 && (
        <div className="space-y-4">
          <Alert variant="warning">
            <AlertTriangle className="h-4 w-4" />
            <div className="ml-2">
              <strong>Warning:</strong> Transferring ownership will remove your owner privileges.
              You will no longer be able to manage billing, delete the workspace, or transfer ownership again.
            </div>
          </Alert>

          <div>
            <label className="block text-sm font-medium text-foreground mb-2">
              Select New Owner
            </label>
            <Select value={selectedUserId} onValueChange={setSelectedUserId}>
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Choose a workspace member..." />
              </SelectTrigger>
              <SelectContent>
                {eligibleMembers.length === 0 ? (
                  <div className="px-2 py-4 text-sm text-muted-foreground text-center">
                    No eligible members. Invite members first.
                  </div>
                ) : (
                  eligibleMembers.map((member) => (
                    <SelectItem key={member.user_id} value={member.user_id}>
                      <div className="flex flex-col">
                        <span className="font-medium">{member.name || member.email}</span>
                        <span className="text-xs text-muted-foreground">{member.email}</span>
                      </div>
                    </SelectItem>
                  ))
                )}
              </SelectContent>
            </Select>
          </div>

          <div className="bg-muted p-4 rounded-md space-y-2">
            <h4 className="font-medium text-sm text-foreground">What happens when you transfer ownership?</h4>
            <ul className="text-sm text-muted-foreground space-y-1 list-disc list-inside">
              <li>The new owner will have full control of the workspace</li>
              <li>They can manage billing, delete the workspace, and remove members</li>
              <li>You will remain as a workspace admin (unless the new owner changes your role)</li>
              <li>The transfer request expires in 7 days if not accepted</li>
            </ul>
          </div>
        </div>
      )}

      {step === 2 && (
        <div className="space-y-4">
          <Alert variant="error">
            <AlertTriangle className="h-4 w-4" />
            <div className="ml-2">
              <strong>Final Confirmation Required</strong>
              <p className="mt-1">This action cannot be undone once the recipient accepts.</p>
            </div>
          </Alert>

          <div className="bg-muted p-4 rounded-md space-y-2">
            <div className="text-sm">
              <span className="text-muted-foreground">Transfer ownership to:</span>
              <div className="mt-1 font-medium text-foreground">
                {selectedMember?.name || selectedMember?.email}
              </div>
              <div className="text-xs text-muted-foreground">{selectedMember?.email}</div>
            </div>
          </div>

          <div>
            <label className="block text-sm font-medium text-foreground mb-2">
              Type the workspace name to confirm: <span className="font-mono text-primary">{workspaceName || '(unnamed workspace)'}</span>
            </label>
            <input
              type="text"
              value={confirmationInput}
              onChange={(e) => setConfirmationInput(e.target.value)}
              placeholder="Enter workspace name"
              className="w-full px-3 py-2 border border-border rounded-md bg-background text-foreground focus:outline-none focus:ring-2 focus:ring-primary"
              autoFocus
            />
          </div>

          {confirmationInput && confirmationInput !== workspaceName && (
            <p className="text-sm text-destructive">Workspace name does not match</p>
          )}
        </div>
      )}
    </Modal>
  );
}
