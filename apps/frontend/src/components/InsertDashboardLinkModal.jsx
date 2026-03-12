// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect } from 'react';
import Modal from './Modal';
import { Button } from './ui/button';
import { Alert } from './ui/alert';

/**
 * InsertDashboardLinkModal - Modal for selecting a dashboard to insert as a link
 *
 * Similar to SaveDashboardModal but simplified:
 * - Only shows existing dashboards (no "create new" option)
 * - Returns selected dashboard info for link insertion
 */
const InsertDashboardLinkModal = ({ isOpen, onClose, onSelect, apiClient }) => {
  const [dashboards, setDashboards] = useState([]);
  const [selectedDashboard, setSelectedDashboard] = useState(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState(null);

  // Load dashboards when modal opens
  useEffect(() => {
    if (isOpen) {
      loadDashboards();
      setSelectedDashboard(null);
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
      setError('Failed to load dashboards');
      setDashboards([]);
    } finally {
      setIsLoading(false);
    }
  };

  const handleSelect = (dashboard) => {
    setSelectedDashboard(dashboard);
    setError(null);
  };

  const handleInsert = () => {
    if (!selectedDashboard) return;
    onSelect(selectedDashboard);
    onClose();
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
      title="Insert Dashboard Link"
      size="lg"
      footer={
        <>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="default"
            onClick={handleInsert}
            disabled={!selectedDashboard}
          >
            Insert Link
          </Button>
        </>
      }
    >
      {/* Subtitle */}
      <p className="text-sm text-muted-foreground mb-4">Select a dashboard to insert a link to</p>

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
        ) : dashboards.length === 0 ? (
          <div className="text-center py-8">
            <p className="text-sm text-muted-foreground">No dashboards found</p>
            <p className="text-xs text-muted-foreground/70 mt-1">Create a dashboard first to link to it</p>
          </div>
        ) : (
          <div className="space-y-2">
            {dashboards.map((dashboard) => (
              <div
                key={dashboard.dashboard_id}
                onClick={() => handleSelect(dashboard)}
                className={`
                  border-2 rounded-lg p-4 cursor-pointer transition-all
                  ${selectedDashboard?.dashboard_id === dashboard.dashboard_id
                    ? 'border-primary bg-primary/10'
                    : 'border-border hover:border-input hover:bg-accent'
                  }
                `}
              >
                <div className="flex items-center gap-3">
                  <div className={`flex-shrink-0 w-10 h-10 rounded-lg flex items-center justify-center ${
                    selectedDashboard?.dashboard_id === dashboard.dashboard_id ? 'bg-primary' : 'bg-accent'
                  }`}>
                    <svg className={`w-5 h-5 ${selectedDashboard?.dashboard_id === dashboard.dashboard_id ? 'text-white' : 'text-muted-foreground'}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
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
                  {selectedDashboard?.dashboard_id === dashboard.dashboard_id && (
                    <svg className="w-5 h-5 text-primary flex-shrink-0" fill="currentColor" viewBox="0 0 20 20">
                      <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.707-9.293a1 1 0 00-1.414-1.414L9 10.586 7.707 9.293a1 1 0 00-1.414 1.414l2 2a1 1 0 001.414 0l4-4z" clipRule="evenodd" />
                    </svg>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </Modal>
  );
};

export default InsertDashboardLinkModal;
