// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useCallback, useRef } from 'react';
import { XMarkIcon, ClockIcon, ArrowUturnLeftIcon, DocumentArrowDownIcon, DocumentArrowUpIcon } from '@heroicons/react/24/outline';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';
import { Button } from './ui/button';
import ConfirmDialog from './ConfirmDialog';
import useConfirm from '../hooks/useConfirm';
import apiClient from '../api/apiClient';

// Hook to detect mobile screen size
function useIsMobile() {
  const [isMobile, setIsMobile] = useState(() =>
    typeof window !== 'undefined' && window.innerWidth < 768
  );

  useEffect(() => {
    const checkMobile = () => setIsMobile(window.innerWidth < 768);
    window.addEventListener('resize', checkMobile);
    return () => window.removeEventListener('resize', checkMobile);
  }, []);

  return isMobile;
}

/**
 * DashboardHistoryPanel - Shows version history for a dashboard
 *
 * Displays all versions, allows previewing previous versions in the editor,
 * viewing diffs, and restoring to a previous version.
 */
export function DashboardHistoryPanel({
  isOpen,
  onClose,
  dashboardId,
  onPreviewVersion,
  onRestoreVersion,
}) {
  const { isOpen: confirmOpen, dialogProps, confirm } = useConfirm();
  const [versions, setVersions] = useState([]);
  const [currentVersion, setCurrentVersion] = useState(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState(null);
  const [previewingVersion, setPreviewingVersion] = useState(null);
  const [showDiff, setShowDiff] = useState(false);
  const [diffData, setDiffData] = useState(null);
  const [isDiffLoading, setIsDiffLoading] = useState(false);
  const [isRestoring, setIsRestoring] = useState(false);
  const isMobile = useIsMobile();

  // Resize state (desktop only)
  const [width, setWidth] = useState(384);
  const [isResizing, setIsResizing] = useState(false);
  const resizeStartX = useRef(0);
  const resizeStartWidth = useRef(384);

  // Fetch versions when panel opens
  useEffect(() => {
    if (isOpen && dashboardId) {
      fetchVersions();
      setPreviewingVersion(null);
      setShowDiff(false);
    }
  }, [isOpen, dashboardId]);

  // Notify parent when preview ends (panel closes)
  useEffect(() => {
    if (!isOpen && onPreviewVersion) {
      onPreviewVersion(null);
    }
  }, [isOpen, onPreviewVersion]);

  const fetchVersions = async () => {
    setIsLoading(true);
    setError(null);
    try {
      const response = await apiClient.get(`/api/v1/dashboards/${dashboardId}/versions`);
      setVersions(response.data.versions || []);
      setCurrentVersion(response.data.current_version || null);
    } catch (err) {
      setError('Failed to load version history');
    } finally {
      setIsLoading(false);
    }
  };

  const handlePreviewVersion = async (versionNumber) => {
    try {
      const response = await apiClient.get(`/api/v1/dashboards/${dashboardId}/versions/${versionNumber}`);
      setPreviewingVersion(response.data);
      setShowDiff(false);
      if (onPreviewVersion) {
        onPreviewVersion(response.data);
      }
    } catch (err) {
      setError('Failed to load version');
    }
  };

  const handleExitPreview = () => {
    setPreviewingVersion(null);
    if (onPreviewVersion) {
      onPreviewVersion(null);
    }
  };

  const handlePreviewCurrentVersion = () => {
    // For current version, we already have the content - no API call needed
    if (currentVersion) {
      setPreviewingVersion(currentVersion);
      setShowDiff(false);
      if (onPreviewVersion) {
        onPreviewVersion(currentVersion);
      }
    }
  };

  const handleViewDiff = async (fromVersion, toVersion) => {
    setIsDiffLoading(true);
    setError(null);
    try {
      const response = await apiClient.get(
        `/api/v1/dashboards/${dashboardId}/versions/diff`,
        { params: { from_version: fromVersion, to_version: toVersion } }
      );
      setDiffData(response.data);
      setShowDiff(true);
      setPreviewingVersion(null);
      if (onPreviewVersion) {
        onPreviewVersion(null);
      }
    } catch (err) {
      setError('Failed to load diff');
    } finally {
      setIsDiffLoading(false);
    }
  };

  const handleRestore = async (versionNumber) => {
    const confirmed = await confirm({
      title: 'Restore Version?',
      message: `Restore to version ${versionNumber}? This will create a new version with that content.`,
      confirmText: 'Restore',
      variant: 'default'
    });
    if (!confirmed) {
      return;
    }

    setIsRestoring(true);
    setError(null);
    try {
      const response = await apiClient.post(
        `/api/v1/dashboards/${dashboardId}/versions/${versionNumber}/restore`
      );
      await fetchVersions();
      setPreviewingVersion(null);
      setShowDiff(false);
      if (onPreviewVersion) {
        onPreviewVersion(null);
      }
      if (onRestoreVersion) {
        onRestoreVersion(response.data);
      }
    } catch (err) {
      setError('Failed to restore version');
    } finally {
      setIsRestoring(false);
    }
  };

  const formatDate = (dateString) => {
    const date = new Date(dateString);
    const now = new Date();
    const diffMs = now - date;
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMs / 3600000);
    const diffDays = Math.floor(diffMs / 86400000);

    if (diffMins < 1) return 'Just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;

    return date.toLocaleDateString('en-US', {
      month: 'short',
      day: 'numeric',
      year: date.getFullYear() !== now.getFullYear() ? 'numeric' : undefined,
    });
  };

  const formatTime = (dateString) => {
    const date = new Date(dateString);
    return date.toLocaleTimeString('en-US', {
      hour: 'numeric',
      minute: '2-digit',
    });
  };

  // Resize handlers
  const handleResizeStart = useCallback((e) => {
    resizeStartX.current = e.clientX;
    resizeStartWidth.current = width;
    setIsResizing(true);
    e.preventDefault();
  }, [width]);

  useEffect(() => {
    if (!isResizing) return;

    const handleMouseMove = (e) => {
      const diff = resizeStartX.current - e.clientX;
      const newWidth = resizeStartWidth.current + diff;
      setWidth(Math.max(320, Math.min(newWidth, 600)));
    };

    const handleMouseUp = () => {
      setIsResizing(false);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };

    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);

    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
  }, [isResizing]);

  if (!isOpen) return null;

  // Panel content
  const panelContent = (
    <div className={`flex flex-col flex-1 min-w-0 ${isMobile ? 'h-full' : ''}`}>
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0">
        <div className="flex items-center gap-2">
          <ClockIcon className="w-5 h-5 text-primary" />
          <span className="font-medium text-foreground">Version History</span>
        </div>
        <button
          onClick={onClose}
          className="p-1 text-muted-foreground hover:text-foreground rounded-md hover:bg-accent"
          aria-label="Close history"
        >
          <XMarkIcon className="w-5 h-5" />
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto">
        {isLoading ? (
          <div className="flex items-center justify-center py-12">
            <img src="/kyomi_animated_logo.svg" alt="Loading" className="w-8 h-8" />
          </div>
        ) : error ? (
          <div className="p-4 text-center">
            <p className="text-error-foreground mb-2">{error}</p>
            <Button variant="outline" size="sm" onClick={fetchVersions}>
              Retry
            </Button>
          </div>
        ) : showDiff && diffData ? (
          /* Diff View */
          <div className="p-4">
            <div className="flex items-center justify-between mb-4">
              <h3 className="text-sm font-medium text-foreground">
                Changes: v{diffData.from_version} → v{diffData.to_version}
              </h3>
              <button
                onClick={() => setShowDiff(false)}
                className="text-sm text-primary hover:text-primary/80"
              >
                Back to list
              </button>
            </div>
            <div className="bg-muted rounded-lg p-3 mb-4">
              <div className="flex items-center gap-4 text-sm">
                <span className="text-success-foreground">+{diffData.additions} additions</span>
                <span className="text-error-foreground">-{diffData.deletions} deletions</span>
              </div>
            </div>
            <DiffViewer diff={diffData.diff} />
          </div>
        ) : !currentVersion && versions.length === 0 ? (
          /* Empty State - only show if no currentVersion and no historical versions */
          <div className="p-6 text-center">
            <ClockIcon className="w-12 h-12 mx-auto text-muted-foreground/50 mb-3" />
            <p className="text-muted-foreground text-sm">No version history yet</p>
            <p className="text-muted-foreground text-xs mt-1">
              Versions are created when you save changes
            </p>
          </div>
        ) : (
          /* Version List */
          <div className="divide-y divide-border/50">
            {/* Preview banner */}
            {previewingVersion && (
              <div className="px-4 py-3 bg-warning border-b border-warning-border">
                <div className="flex items-center justify-between">
                  <div>
                    <p className="text-sm font-medium text-warning-foreground">
                      Previewing Version {previewingVersion.version_number}
                    </p>
                    <p className="text-xs text-warning-foreground mt-0.5">
                      Click "Exit Preview" to return to current version
                    </p>
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={handleExitPreview}
                    className="text-warning-foreground border-warning-border hover:bg-warning"
                  >
                    Exit Preview
                  </Button>
                </div>
              </div>
            )}

            {/* Current Saved Version - Always shown at top */}
            {currentVersion && (
              <div
                onClick={() => previewingVersion?.is_current ? handleExitPreview() : handlePreviewCurrentVersion()}
                className={`px-4 py-3 transition-colors cursor-pointer ${
                  previewingVersion?.is_current ? 'bg-warning' : 'hover:bg-accent'
                }`}
              >
                <div className="flex items-start justify-between">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="text-sm font-medium text-foreground">
                        Version {currentVersion.version_number}
                      </span>
                      <span className="px-1.5 py-0.5 bg-primary/10 text-primary text-xs rounded font-medium">
                        Latest
                      </span>
                      {previewingVersion?.is_current && (
                        <span className="px-1.5 py-0.5 bg-warning text-warning-foreground text-xs rounded">
                          Previewing
                        </span>
                      )}
                    </div>
                    <p className="text-xs text-muted-foreground mt-0.5">
                      {formatDate(currentVersion.created_at)} at {formatTime(currentVersion.created_at)}
                    </p>
                    <p className="text-xs text-muted-foreground mt-1">
                      {currentVersion.change_summary}
                    </p>
                  </div>
                </div>

                {/* Actions for current version - only diff with previous if there are historical versions */}
                {versions.length > 0 && (
                  <div className="flex items-center gap-2 mt-2" onClick={(e) => e.stopPropagation()}>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          onClick={() => handleViewDiff(versions[0].version_number, currentVersion.version_number)}
                          disabled={isDiffLoading}
                          className="p-1.5 text-muted-foreground hover:text-foreground hover:bg-accent rounded transition-colors"
                        >
                          <DocumentArrowDownIcon className="w-4 h-4" />
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>Compare with previous</TooltipContent>
                    </Tooltip>
                  </div>
                )}
              </div>
            )}

            {/* Historical Versions */}
            {versions.map((version, index) => {
              const prevVersion = versions[index + 1];
              const isPreviewing = previewingVersion?.version_number === version.version_number && !previewingVersion?.is_current;

              return (
                <div
                  key={version.version_id}
                  onClick={() => isPreviewing ? handleExitPreview() : handlePreviewVersion(version.version_number)}
                  className={`px-4 py-3 transition-colors cursor-pointer ${
                    isPreviewing ? 'bg-warning' : 'hover:bg-accent'
                  }`}
                >
                  <div className="flex items-start justify-between">
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="text-sm font-medium text-foreground">
                          Version {version.version_number}
                        </span>
                        {isPreviewing && (
                          <span className="px-1.5 py-0.5 bg-warning text-warning-foreground text-xs rounded">
                            Previewing
                          </span>
                        )}
                      </div>
                      <p className="text-xs text-muted-foreground mt-0.5">
                        {formatDate(version.created_at)} at {formatTime(version.created_at)}
                      </p>
                      <p className="text-xs text-muted-foreground mt-1">
                        {version.change_summary || 'No summary'}
                      </p>
                      <p className="text-xs text-muted-foreground/70 mt-0.5">
                        by {version.created_by?.name || version.created_by?.email}
                      </p>
                    </div>
                  </div>

                  {/* Actions - stopPropagation so clicking these doesn't trigger card preview */}
                  <div className="flex items-center gap-2 mt-2" onClick={(e) => e.stopPropagation()}>
                    {/* Compare with previous historical version */}
                    {prevVersion && (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={() => handleViewDiff(prevVersion.version_number, version.version_number)}
                            disabled={isDiffLoading}
                            className="p-1.5 text-muted-foreground hover:text-foreground hover:bg-accent rounded transition-colors"
                          >
                            <DocumentArrowDownIcon className="w-4 h-4" />
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>Compare with previous</TooltipContent>
                      </Tooltip>
                    )}

                    {/* Compare with current version */}
                    {currentVersion && (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={() => handleViewDiff(version.version_number, currentVersion.version_number)}
                            disabled={isDiffLoading}
                            className="p-1.5 text-muted-foreground hover:text-primary hover:bg-accent rounded transition-colors"
                          >
                            <DocumentArrowUpIcon className="w-4 h-4" />
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>Compare with current</TooltipContent>
                      </Tooltip>
                    )}

                    {/* Restore button - available for all historical versions */}
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          onClick={() => handleRestore(version.version_number)}
                          disabled={isRestoring}
                          className="p-1.5 text-muted-foreground hover:text-primary hover:bg-primary/10 rounded transition-colors"
                        >
                          <ArrowUturnLeftIcon className="w-4 h-4" />
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>Restore this version</TooltipContent>
                    </Tooltip>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );

  // Mobile: Slide-in panel with backdrop
  if (isMobile) {
    return (
      <>
        <div
          className="fixed top-32 left-0 right-0 bottom-0 bg-black/50 z-40"
          onClick={onClose}
        />
        <div className="fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl">
          {panelContent}
        </div>
      </>
    );
  }

  // Desktop: Resizable sidebar (like Copilot)
  return (
    <div
      className="border-l border-border bg-card flex h-full overflow-hidden"
      style={{ width: `${width}px` }}
    >
      {/* Resize Handle */}
      <Tooltip>
        <TooltipTrigger asChild>
          <div
            className="flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10"
            onMouseDown={handleResizeStart}
            aria-label="Drag to resize"
          >
            <div className="w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded transition-colors" />
          </div>
        </TooltipTrigger>
        <TooltipContent>Drag to resize</TooltipContent>
      </Tooltip>

      {panelContent}
      <ConfirmDialog isOpen={confirmOpen} {...dialogProps} />
    </div>
  );
}

/**
 * DiffViewer - Simple diff visualization component
 */
function DiffViewer({ diff }) {
  if (!diff) return null;

  const lines = diff.split('\n');

  return (
    <div className="font-mono text-xs bg-foreground rounded-lg overflow-x-auto">
      <pre className="p-4">
        {lines.map((line, index) => {
          let className = 'text-background/80';
          if (line.startsWith('+') && !line.startsWith('+++')) {
            className = 'text-green-400 bg-green-900/30';
          } else if (line.startsWith('-') && !line.startsWith('---')) {
            className = 'text-red-400 bg-red-900/30';
          } else if (line.startsWith('@@')) {
            className = 'text-blue-400';
          } else if (line.startsWith('+++') || line.startsWith('---')) {
            className = 'text-background/50';
          }

          return (
            <div key={index} className={`${className} px-2 -mx-2`}>
              {line || ' '}
            </div>
          );
        })}
      </pre>
    </div>
  );
}

export default DashboardHistoryPanel;
