// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useRef } from 'react';
import Modal from './Modal';
import { Button } from './ui/button';
import { Alert } from './ui/alert';
import { useCapabilities } from '../context/CapabilitiesContext';

const SaveDashboardModal = ({ isOpen, onClose, onSave, messageContent, apiClient }) => {
  const [newDashboardTitle, setNewDashboardTitle] = useState('');
  const [dashboards, setDashboards] = useState([]);
  const [selectedDashboardId, setSelectedDashboardId] = useState(null);
  const [isCreatingNew, setIsCreatingNew] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [error, setError] = useState(null);
  const capabilities = useCapabilities();

  // Load dashboards when modal opens
  useEffect(() => {
    if (isOpen) {
      loadDashboards();
      setNewDashboardTitle('');
      setSelectedDashboardId(null);
      setIsCreatingNew(false);
      setError(null);
    }
  }, [isOpen]);

  const loadDashboards = async () => {
    setIsLoading(true);
    try {
      const data = await apiClient.listDashboards();
      // Backend returns array directly, not wrapped in {dashboards: [...]}
      setDashboards(Array.isArray(data) ? data : []);
    } catch (error) {
      setDashboards([]);
    } finally {
      setIsLoading(false);
    }
  };

  const handleSelectNew = () => {
    // Check if at dashboard limit for free tier
    const maxDashboards = capabilities.max_dashboards || 0;
    if (maxDashboards > 0 && dashboards.length >= maxDashboards) {
      setError(`Free tier is limited to ${maxDashboards} dashboards. Please upgrade to create more dashboards.`);
      return;
    }
    setIsCreatingNew(true);
    setSelectedDashboardId(null);
    setError(null);
  };

  const handleSelectExisting = (dashboardId) => {
    setSelectedDashboardId(dashboardId);
    setIsCreatingNew(false);
    setError(null);
  };

  const handleSave = async () => {
    if (isCreatingNew && !newDashboardTitle.trim()) {
      return;
    }
    if (!isCreatingNew && !selectedDashboardId) {
      return;
    }

    setIsSaving(true);
    setError(null);
    try {
      const mode = isCreatingNew ? 'new' : 'existing';
      const value = isCreatingNew ? newDashboardTitle.trim() : selectedDashboardId;
      await onSave(mode, value, messageContent);
      setNewDashboardTitle('');
      setSelectedDashboardId(null);
      setIsCreatingNew(false);
      onClose();
    } catch (error) {
      // Extract error message from various possible error formats
      const errorMessage = error.response?.data?.detail ||
                          error.response?.data?.message ||
                          error.message ||
                          'Failed to save dashboard. Please try again.';
      setError(errorMessage);
    } finally {
      setIsSaving(false);
    }
  };

  const handleKeyPress = (e) => {
    if (e.key === 'Enter' && !e.shiftKey && isCreatingNew && newDashboardTitle.trim()) {
      e.preventDefault();
      handleSave();
    } else if (e.key === 'Escape') {
      onClose();
    }
  };

  const formatDate = (dateString) => {
    const date = new Date(dateString);
    const now = new Date();
    const diffInMs = now - date;
    const diffInDays = Math.floor(diffInMs / (1000 * 60 * 60 * 24));

    if (diffInDays < 1) return 'Today';
    if (diffInDays === 1) return 'Yesterday';
    if (diffInDays < 7) return `${diffInDays} days ago`;

    return date.toLocaleDateString('en-US', {
      month: 'short',
      day: 'numeric',
      year: date.getFullYear() !== now.getFullYear() ? 'numeric' : undefined
    });
  };

  return (
    <Modal
      show={isOpen}
      onClose={onClose}
      title="Save to Dashboard"
      size="lg"
      footer={
        <>
          <Button variant="outline" onClick={onClose} disabled={isSaving}>
            Cancel
          </Button>
          <Button
            variant="default"
            onClick={handleSave}
            disabled={
              isSaving ||
              (isCreatingNew && !newDashboardTitle.trim()) ||
              (!isCreatingNew && !selectedDashboardId)
            }
          >
            {isSaving ? 'Saving...' : isCreatingNew ? 'Create Dashboard' : 'Add to Dashboard'}
          </Button>
        </>
      }
    >
      {/* Subtitle */}
      <p className="text-sm text-muted-foreground mb-4">Create a new dashboard or add to an existing one</p>

      {/* Error Alert */}
      {error && (
        <Alert variant="error" className="mb-4">
          {error}
        </Alert>
      )}

      {/* Body */}
      <div className="flex-1 overflow-y-auto">
          {isLoading ? (
            <div className="flex items-center justify-center py-12">
              <div className="text-muted-foreground">Loading dashboards...</div>
            </div>
          ) : (
            <div className="space-y-2">
              {/* Create New Dashboard Option */}
              {(() => {
                const maxDashboards = capabilities.max_dashboards || 0;
                const atLimit = maxDashboards > 0 && dashboards.length >= maxDashboards;

                return (
                  <div
                    onClick={handleSelectNew}
                    className={`
                      border-2 rounded-lg p-4 transition-all
                      ${atLimit
                        ? 'border-border bg-muted opacity-60 cursor-not-allowed'
                        : isCreatingNew
                          ? 'border-primary bg-primary/10 cursor-pointer'
                          : 'border-border hover:border-input hover:bg-accent cursor-pointer'
                      }
                    `}
                  >
                    <div className="flex items-center gap-3">
                      <div className={`flex-shrink-0 w-10 h-10 rounded-lg flex items-center justify-center ${
                        atLimit ? 'bg-muted' : isCreatingNew ? 'bg-primary' : 'bg-accent'
                      }`}>
                        <svg className={`w-5 h-5 ${atLimit ? 'text-muted-foreground' : isCreatingNew ? 'text-white' : 'text-muted-foreground'}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
                        </svg>
                      </div>
                      <div className="flex-1">
                        <h3 className={`text-base font-medium ${atLimit ? 'text-muted-foreground' : 'text-foreground'}`}>
                          Create New Dashboard
                          {atLimit && <span className="ml-2 text-xs font-normal text-error">(Limit reached)</span>}
                        </h3>
                        <p className={`text-sm mt-0.5 ${atLimit ? 'text-muted-foreground/70' : 'text-muted-foreground'}`}>
                          {atLimit ? `Free tier limited to ${maxDashboards} dashboards` : 'Start fresh with a new dashboard'}
                        </p>
                      </div>
                      {isCreatingNew && (
                        <svg className="w-5 h-5 text-primary flex-shrink-0" fill="currentColor" viewBox="0 0 20 20">
                          <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.707-9.293a1 1 0 00-1.414-1.414L9 10.586 7.707 9.293a1 1 0 00-1.414 1.414l2 2a1 1 0 001.414 0l4-4z" clipRule="evenodd" />
                        </svg>
                      )}
                    </div>

                    {/* Expand to show input when selected */}
                    {isCreatingNew && (
                      <div className="mt-4 pl-13">
                        <input
                          type="text"
                          value={newDashboardTitle}
                          onChange={(e) => setNewDashboardTitle(e.target.value)}
                          onKeyDown={handleKeyPress}
                          placeholder="Enter dashboard title..."
                          className="w-full px-4 py-2 border border-input rounded-lg focus:outline-none focus:ring-2 focus:ring-ring focus:border-transparent bg-background text-foreground"
                          autoFocus
                          disabled={isSaving}
                        />
                      </div>
                    )}
                  </div>
                );
              })()}

              {/* Divider */}
              {dashboards.length > 0 && (
                <div className="relative py-2">
                  <div className="absolute inset-0 flex items-center">
                    <div className="w-full border-t border-border"></div>
                  </div>
                  <div className="relative flex justify-center">
                    <span className="px-3 bg-background text-sm text-muted-foreground">or add to existing</span>
                  </div>
                </div>
              )}

              {/* Existing Dashboards List */}
              {dashboards.map((dashboard) => (
                <div
                  key={dashboard.dashboard_id}
                  onClick={() => handleSelectExisting(dashboard.dashboard_id)}
                  className={`
                    border-2 rounded-lg p-4 cursor-pointer transition-all
                    ${selectedDashboardId === dashboard.dashboard_id
                      ? 'border-primary bg-primary/10'
                      : 'border-border hover:border-input hover:bg-accent'
                    }
                  `}
                >
                  <div className="flex items-center gap-3">
                    <div className={`flex-shrink-0 w-10 h-10 rounded-lg flex items-center justify-center ${
                      selectedDashboardId === dashboard.dashboard_id ? 'bg-primary' : 'bg-accent'
                    }`}>
                      <svg className={`w-5 h-5 ${selectedDashboardId === dashboard.dashboard_id ? 'text-white' : 'text-muted-foreground'}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 17V7m0 10a2 2 0 01-2 2H5a2 2 0 01-2-2V7a2 2 0 012-2h2a2 2 0 012 2m0 10a2 2 0 002 2h2a2 2 0 002-2M9 7a2 2 0 012-2h2a2 2 0 012 2m0 10V7m0 10a2 2 0 002 2h2a2 2 0 002-2V7a2 2 0 00-2-2h-2a2 2 0 00-2 2" />
                      </svg>
                    </div>
                    <div className="flex-1 min-w-0">
                      <h3 className="text-base font-medium text-foreground truncate">
                        {dashboard.title || 'Untitled Dashboard'}
                      </h3>
                      <p className="text-sm text-muted-foreground mt-0.5">
                        {formatDate(dashboard.created_at)}
                      </p>
                    </div>
                    {selectedDashboardId === dashboard.dashboard_id && (
                      <svg className="w-5 h-5 text-primary flex-shrink-0" fill="currentColor" viewBox="0 0 20 20">
                        <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.707-9.293a1 1 0 00-1.414-1.414L9 10.586 7.707 9.293a1 1 0 00-1.414 1.414l2 2a1 1 0 001.414 0l4-4z" clipRule="evenodd" />
                      </svg>
                    )}
                  </div>
                </div>
              ))}

              {/* Empty state for no existing dashboards */}
              {dashboards.length === 0 && !isCreatingNew && (
                <div className="text-center py-8">
                  <p className="text-sm text-muted-foreground">No existing dashboards yet</p>
                </div>
              )}
            </div>
          )}
      </div>
    </Modal>
  );
};

export default SaveDashboardModal;
