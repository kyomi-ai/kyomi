// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { UserPlus, Trash2, ArrowRightLeft } from 'lucide-react';
import { Button } from '../ui/button';
import { Badge } from '../ui/badge';
import { Spinner } from '../ui/spinner';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../ui/select';
import Modal from '../Modal';
import ConfirmDialog from '../ConfirmDialog';
import useConfirm from '../../hooks/useConfirm';
import { toast } from '../../lib/toast';
import TransferOwnershipModal from './TransferOwnershipModal';
import { useWebSocket } from '../../context/WebSocketContext';

export default function TeamManagement({ user, apiClient, workspaceInfo }) {
  const { isOpen, dialogProps, confirm } = useConfirm();
  const { subscribe } = useWebSocket();
  // Team management state
  const [showInviteModal, setShowInviteModal] = useState(false);
  const [showTransferOwnershipModal, setShowTransferOwnershipModal] = useState(false);
  const [inviteEmail, setInviteEmail] = useState('');
  const [inviteRole, setInviteRole] = useState('user');
  const [invitations, setInvitations] = useState([]);
  const [invitationsLoading, setInvitationsLoading] = useState(false);
  const [members, setMembers] = useState([]);
  const [membersLoading, setMembersLoading] = useState(false);
  const [currentUserLimit, setCurrentUserLimit] = useState(null);
  const [ownershipTransfers, setOwnershipTransfers] = useState([]);
  const [transfersLoading, setTransfersLoading] = useState(false);

  // Check if current user is the owner
  const isCurrentUserOwner = members.find(m => m.user_id === user?.user_id)?.is_owner || false;

  // Load invitations, members, and transfers on mount
  useEffect(() => {
    fetchInvitations();
    fetchMembers();
    fetchOwnershipTransfers();
  }, [apiClient]);

  // Sync currentUserLimit with workspaceInfo
  useEffect(() => {
    if (workspaceInfo?.user_limit) {
      setCurrentUserLimit(workspaceInfo.user_limit);
    }
  }, [workspaceInfo]);

  // Subscribe to ownership transfer notifications
  useEffect(() => {
    if (!subscribe) return;

    const unsubscribe = subscribe('ownership_transfer_offered', (data) => {
      // Show toast notification
      toast.info(
        `${data.from_user_email} has offered to transfer workspace ownership to you. Check the Team tab to review.`,
        { duration: 10000 }
      );

      // Refresh transfers list
      fetchOwnershipTransfers();
    });

    return () => {
      if (unsubscribe) unsubscribe();
    };
  }, [subscribe]);

  // Team management handlers
  const fetchInvitations = async () => {
    if (!apiClient) return;

    try {
      setInvitationsLoading(true);
      const response = await apiClient.get('/api/v1/workspaces/invitations');
      setInvitations(response.data);
    } catch (error) {
      toast.error(`Failed to load invitations: ${error.response?.data?.detail || error.message}. Please try refreshing the page.`);
    } finally {
      setInvitationsLoading(false);
    }
  };

  const handleCreateInvitation = async () => {
    if (!inviteEmail || !apiClient) return;

    try {
      await apiClient.post('/api/v1/workspaces/invitations', {
        email: inviteEmail,
        role: inviteRole
      });

      // Reset form and close modal
      setInviteEmail('');
      setInviteRole('user');
      setShowInviteModal(false);

      // Refresh invitations list
      await fetchInvitations();
    } catch (error) {
      const errorMessage = error.response?.data?.detail || error.message;

      // Check if error is about user limit
      if (errorMessage.toLowerCase().includes('user limit') || errorMessage.toLowerCase().includes('maximum') || errorMessage.toLowerCase().includes('exceeded')) {
        // Provide helpful upgrade guidance based on current tier
        const currentTier = user?.subscription_tier || 'free';

        if (currentTier === 'team') {
          // Team tier - can purchase additional users
          toast.error(
            'User Limit Reached: Your team has reached its user limit. Go to Settings → Billing → Team Size to add more users.',
            { duration: 6000 }
          );
        } else {
          // Free, Basic, or Pro tier - need to upgrade to Team
          toast.error(
            'User Limit Reached: Your current plan supports only 1 user. Upgrade to the Team plan in Settings → Billing to collaborate with more users.',
            { duration: 6000 }
          );
        }
      } else {
        // Generic error
        toast.error('Failed to create invitation: ' + errorMessage);
      }
    }
  };

  const handleCancelInvitation = async (invitationId) => {
    if (!apiClient) return;

    const confirmed = await confirm({
      title: 'Cancel Invitation?',
      message: 'Are you sure you want to cancel this invitation?',
      confirmText: 'Cancel Invitation',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }

    try {
      await apiClient.delete(`/api/v1/workspaces/invitations/${invitationId}`);
      // Refresh invitations list
      await fetchInvitations();
    } catch (error) {
      toast.error('Failed to cancel invitation: ' + (error.response?.data?.detail || error.message));
    }
  };

  // Member management handlers
  const fetchMembers = async () => {
    if (!apiClient) return;

    try {
      setMembersLoading(true);
      const response = await apiClient.get('/api/v1/workspaces/members');
      setMembers(response.data);
    } catch (error) {
      toast.error(`Failed to load team members: ${error.response?.data?.detail || error.message}. Please try refreshing the page.`);
    } finally {
      setMembersLoading(false);
    }
  };

  const handleUpdateMemberRole = async (userId, newRole) => {
    if (!apiClient) return;

    try {
      await apiClient.patch(`/api/v1/workspaces/members/${userId}/role`, {
        role: newRole
      });

      // Refresh members list
      await fetchMembers();
    } catch (error) {
      toast.error('Failed to update member role: ' + (error.response?.data?.detail || error.message));
    }
  };

  const handleRemoveMember = async (userId) => {
    if (!apiClient) return;

    const confirmed = await confirm({
      title: 'Remove Team Member?',
      message: 'Are you sure you want to remove this member from the workspace?',
      confirmText: 'Remove Member',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }

    try {
      await apiClient.delete(`/api/v1/workspaces/members/${userId}`);

      // Refresh members list
      await fetchMembers();
    } catch (error) {
      toast.error('Failed to remove member: ' + (error.response?.data?.detail || error.message));
    }
  };

  // Ownership transfer handlers
  const fetchOwnershipTransfers = async () => {
    if (!apiClient) return;

    try {
      setTransfersLoading(true);
      const response = await apiClient.get('/api/v1/workspaces/ownership/transfers');
      setOwnershipTransfers(response.data || []);
    } catch (error) {
      // Don't show error toast - transfers are optional
    } finally {
      setTransfersLoading(false);
    }
  };

  const handleCancelTransfer = async (transferId) => {
    if (!apiClient) return;

    const confirmed = await confirm({
      title: 'Cancel Ownership Transfer?',
      message: 'Are you sure you want to cancel this ownership transfer request?',
      confirmText: 'Cancel Transfer',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }

    try {
      await apiClient.delete(`/api/v1/workspaces/ownership/transfer/${transferId}`);
      toast.success('Ownership transfer cancelled');

      // Refresh transfers list
      await fetchOwnershipTransfers();
    } catch (error) {
      toast.error('Failed to cancel transfer: ' + (error.response?.data?.detail || error.message));
    }
  };

  return (
    <div className="p-6" style={{display: 'block', padding: '1.5rem'}}>
      <div className="flex flex-col sm:flex-row sm:justify-between sm:items-center gap-4 mb-6">
        <div className="min-w-0">
          <h2 className="text-lg sm:text-xl font-semibold text-foreground mb-1 sm:mb-2">Team Members</h2>
          <p className="text-xs sm:text-sm text-muted-foreground">
            Invite team members to collaborate.
            {currentUserLimit && currentUserLimit < 999999 && ` Limit: ${currentUserLimit}.`}
            {currentUserLimit === 999999 && ' Unlimited members.'}
          </p>
        </div>
        <div className="flex gap-2 flex-shrink-0">
          {isCurrentUserOwner && (
            <Button
              variant="outline"
              onClick={() => setShowTransferOwnershipModal(true)}
              title="Transfer Ownership"
            >
              <ArrowRightLeft className="h-4 w-4 sm:mr-2" />
              <span className="hidden sm:inline">Transfer Ownership</span>
            </Button>
          )}
          <Button
            variant="default"
            onClick={() => setShowInviteModal(true)}
            title="Invite Member"
          >
            <UserPlus className="h-4 w-4 sm:mr-2" />
            <span className="hidden sm:inline">Invite Member</span>
          </Button>
        </div>
      </div>

      {/* Pending Invitations */}
      <div className="mb-6">
        <h3 className="text-base sm:text-lg font-semibold text-foreground mb-4">Pending Invitations</h3>
        {invitationsLoading ? (
          <div className="text-center py-8">
            <Spinner size="lg" className="text-muted-foreground mx-auto" />
            <p className="text-muted-foreground mt-2">Loading invitations...</p>
          </div>
        ) : invitations.length === 0 ? (
          <div className="text-center py-8 bg-muted rounded-lg border-2 border-dashed border-border">
            <p className="text-muted-foreground">No pending invitations</p>
          </div>
        ) : (
          <div className="space-y-3">
            {invitations.map((invitation) => (
              <div
                key={invitation.invitation_id}
                className="border border-border rounded-lg p-3 sm:p-4 bg-background hover:bg-muted/50 transition-colors"
              >
                <div className="flex flex-col sm:flex-row sm:items-center gap-3 sm:gap-4">
                  {/* Invitation info */}
                  <div className="flex-1 min-w-0">
                    <div className="flex flex-wrap items-center gap-2 mb-1">
                      <span className="text-sm font-medium text-foreground truncate">{invitation.email}</span>
                      <Badge variant={invitation.role === 'admin' ? 'secondary' : 'default'} className="flex-shrink-0">
                        {invitation.role}
                      </Badge>
                    </div>
                    <div className="text-xs text-muted-foreground">
                      Invited {new Date(invitation.created_at).toLocaleDateString()}
                      <span className="mx-1">•</span>
                      Expires {new Date(invitation.expires_at).toLocaleDateString()}
                    </div>
                  </div>

                  {/* Cancel button */}
                  <div className="flex items-center pt-2 sm:pt-0 border-t sm:border-t-0 border-border flex-shrink-0">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => handleCancelInvitation(invitation.invitation_id)}
                      title="Cancel invitation"
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Pending Ownership Transfers */}
      {ownershipTransfers.length > 0 && (
        <div className="mb-6">
          <h3 className="text-lg font-semibold text-foreground mb-4">
            {isCurrentUserOwner ? 'Pending Ownership Transfers' : 'Ownership Transfer Offers'}
          </h3>
          {transfersLoading ? (
            <div className="text-center py-8">
              <Spinner size="lg" className="text-muted-foreground mx-auto" />
              <p className="text-muted-foreground mt-2">Loading transfers...</p>
            </div>
          ) : (
            <div className="space-y-4">
              {ownershipTransfers.map((transfer) => {
                const isRecipient = transfer.to_user_id === user?.user_id;
                const isInitiator = transfer.from_user_id === user?.user_id;

                return (
                  <div key={transfer.transfer_id} className={`border rounded-lg p-4 ${isRecipient ? 'border-primary bg-primary/5' : 'border-border bg-background'}`}>
                    {isRecipient ? (
                      // Recipient view - prominent call to action
                      <div className="space-y-4">
                        <div className="flex items-start gap-3">
                          <ArrowRightLeft className="h-5 w-5 text-primary mt-1 flex-shrink-0" />
                          <div className="flex-1">
                            <h4 className="font-semibold text-foreground mb-1">You've been offered workspace ownership</h4>
                            <p className="text-sm text-muted-foreground mb-2">
                              {transfer.from_user_email} wants to transfer ownership of this workspace to you.
                            </p>
                            <div className="flex items-center gap-4 text-xs text-muted-foreground">
                              <span>Requested: {new Date(transfer.created_at).toLocaleDateString()}</span>
                              <span>Expires: {new Date(transfer.expires_at).toLocaleDateString()}</span>
                            </div>
                          </div>
                        </div>
                        <div className="flex gap-2">
                          <Button
                            variant="default"
                            onClick={() => window.location.href = `/accept-ownership/${transfer.transfer_id}`}
                          >
                            Review & Accept
                          </Button>
                        </div>
                      </div>
                    ) : isInitiator ? (
                      // Initiator view - simple table row
                      <div className="flex items-center justify-between">
                        <div className="flex-1">
                          <div className="text-sm font-medium text-foreground">Pending transfer to {transfer.to_user_email}</div>
                          <div className="flex items-center gap-4 text-xs text-muted-foreground mt-1">
                            <span>Requested: {new Date(transfer.created_at).toLocaleDateString()}</span>
                            <span>Expires: {new Date(transfer.expires_at).toLocaleDateString()}</span>
                          </div>
                        </div>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => handleCancelTransfer(transfer.transfer_id)}
                          title="Cancel transfer"
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>
                    ) : null}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}

      {/* Workspace Members */}
      <div className="mb-6">
        <h3 className="text-base sm:text-lg font-semibold text-foreground mb-4">Workspace Members</h3>
        {membersLoading ? (
          <div className="text-center py-8">
            <Spinner size="lg" className="text-muted-foreground mx-auto" />
            <p className="text-muted-foreground mt-2">Loading members...</p>
          </div>
        ) : members.length === 0 ? (
          <div className="text-center py-8 bg-muted rounded-lg border-2 border-dashed border-border">
            <p className="text-muted-foreground">No members found</p>
          </div>
        ) : (
          <div className="space-y-3">
            {members.map((member) => (
              <div
                key={member.user_id}
                className="border border-border rounded-lg p-3 sm:p-4 bg-background hover:bg-muted/50 transition-colors"
              >
                <div className="flex flex-col sm:flex-row sm:items-center gap-3 sm:gap-4">
                  {/* Member info */}
                  <div className="flex items-center gap-3 flex-1 min-w-0">
                    <div className="h-8 w-8 rounded-full bg-primary/10 flex items-center justify-center text-primary font-medium flex-shrink-0">
                      {member.email.charAt(0).toUpperCase()}
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-sm font-medium text-foreground truncate">{member.name || member.email}</span>
                        {member.is_owner && (
                          <Badge variant="default" className="text-xs flex-shrink-0">Owner</Badge>
                        )}
                      </div>
                      <div className="text-xs sm:text-sm text-muted-foreground truncate">{member.email}</div>
                    </div>
                  </div>

                  {/* Controls for non-owners */}
                  {!member.is_owner && (
                    <div className="flex items-center gap-2 sm:gap-3 pt-2 sm:pt-0 border-t sm:border-t-0 border-border flex-shrink-0">
                      <Select
                        value={member.role === 'workspace_admin' ? 'admin' : 'user'}
                        onValueChange={(value) => handleUpdateMemberRole(member.user_id, value)}
                        disabled={member.user_id === user?.user_id}
                      >
                        <SelectTrigger className="w-[100px] sm:w-[120px] h-8 text-xs">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="user">User</SelectItem>
                          <SelectItem value="admin">Admin</SelectItem>
                        </SelectContent>
                      </Select>

                      {/* Joined date - hidden on mobile */}
                      <span className="hidden sm:inline text-xs text-muted-foreground whitespace-nowrap">
                        Joined {new Date(member.joined_at).toLocaleDateString()}
                      </span>

                      {/* Remove button */}
                      {member.user_id !== user?.user_id && (
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => handleRemoveMember(member.user_id)}
                          title="Remove member"
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      )}
                    </div>
                  )}

                  {/* Joined date for owner - desktop only */}
                  {member.is_owner && (
                    <span className="hidden sm:inline text-xs text-muted-foreground whitespace-nowrap flex-shrink-0">
                      Joined {new Date(member.joined_at).toLocaleDateString()}
                    </span>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Invite Member Modal */}
      <Modal
        show={showInviteModal}
        onClose={() => {
          setShowInviteModal(false);
          setInviteEmail('');
          setInviteRole('user');
        }}
        title="Invite Team Member"
        size="md"
        footer={
          <>
            <Button
              variant="outline"
              onClick={() => {
                setShowInviteModal(false);
                setInviteEmail('');
                setInviteRole('user');
              }}
            >
              Cancel
            </Button>
            <Button
              variant="default"
              onClick={handleCreateInvitation}
              disabled={!inviteEmail}
            >
              Send Invitation
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <div>
            <label className="block text-sm font-medium text-foreground mb-1">
              Email Address
            </label>
            <input
              type="email"
              value={inviteEmail}
              onChange={(e) => setInviteEmail(e.target.value)}
              placeholder="colleague@example.com"
              className="w-full px-3 py-2 border border-border rounded-md bg-background text-foreground focus:outline-none focus:ring-2 focus:ring-primary"
            />
          </div>

          <div>
            <label className="block text-sm font-medium text-foreground mb-1">
              Role
            </label>
            <Select value={inviteRole} onValueChange={setInviteRole}>
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="user">User - Full feature access</SelectItem>
                <SelectItem value="admin">Admin - Can manage workspace settings</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>
      </Modal>

      {/* Confirm Dialog */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />

      {/* Transfer Ownership Modal */}
      <TransferOwnershipModal
        isOpen={showTransferOwnershipModal}
        onClose={() => setShowTransferOwnershipModal(false)}
        members={members}
        currentUserId={user?.user_id}
        workspaceName={workspaceInfo?.name}
        apiClient={apiClient}
        onTransferInitiated={() => {
          // Refresh members and transfers list to show any updates
          fetchMembers();
          fetchOwnershipTransfers();
        }}
      />
    </div>
  );
}
