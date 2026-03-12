// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * TabBar Component
 *
 * Displays a horizontal bar of tabs with controls to manage them.
 */

import { memo, useMemo } from 'react';
import ResultTab from './ResultTab';
import { Tooltip, TooltipTrigger, TooltipContent } from '../../../components/ui/tooltip';
import useRenderLogger from '../../../hooks/useRenderLogger';

/**
 * TabBar component
 *
 * Displays tabs horizontally with scroll support and a new tab button.
 *
 * Usage:
 * ```jsx
 * <TabBar
 *   tabs={tabs}
 *   activeTabId={activeTabId}
 *   onTabClick={handleTabClick}
 *   onTabClose={handleTabClose}
 *   onTogglePin={handleTogglePin}
 *   onNewTab={handleNewTab}
 *   maxTabs={10}
 * />
 * ```
 */
const TabBar = ({
  tabs,
  activeTabId,
  onTabClick,
  onTabClose,
  onTogglePin,
  onNewTab,
  maxTabs = 10,
  editorRef,
  onDatasourceChange,
}) => {
  // Development: Log re-renders
  useRenderLogger('TabBar', { tabCount: tabs.length, activeTabId });

  const unpinnedCount = tabs.filter(t => !t.pinned).length;
  const canAddTab = unpinnedCount < 5; // Max 5 unpinned tabs

  // Memoize tab elements to prevent creating new callback functions on every render
  // This is critical for tab switching performance with 200+ rows
  const tabElements = useMemo(() => {
    return tabs.map((tab) => (
      <ResultTab
        key={tab.id}
        tab={tab}
        allTabs={tabs}
        isActive={tab.id === activeTabId}
        onClick={() => onTabClick(tab.id)}
        onClose={() => onTabClose(tab.id)}
        onTogglePin={() => onTogglePin(tab.id)}
        editorRef={editorRef}
        onDatasourceChange={onDatasourceChange}
      />
    ));
  }, [tabs, activeTabId, onTabClick, onTabClose, onTogglePin, editorRef, onDatasourceChange]);

  return (
    <div className="flex items-center bg-muted border-b border-border">
      {/* Tabs container with horizontal scroll */}
      <div className="flex-1 flex overflow-x-auto overflow-y-hidden scrollbar-thin">
        {tabs.length === 0 ? (
          <div className="px-4 py-2 text-xs text-muted-foreground italic">
            No results yet. Run a query to see results.
          </div>
        ) : (
          tabElements
        )}
      </div>

      {/* New tab button */}
      {onNewTab && (
        <div className="flex-shrink-0 border-l border-border">
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={onNewTab}
                disabled={!canAddTab}
                className="px-3 py-2 hover:bg-accent transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                aria-label={
                  canAddTab
                    ? 'Create new result tab'
                    : `Maximum ${maxTabs} tabs allowed`
                }
              >
                <svg
                  className="w-4 h-4 text-muted-foreground"
                  fill="none"
                  stroke="currentColor"
                  viewBox="0 0 24 24"
                >
                  <path
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    strokeWidth={2}
                    d="M12 4v16m8-8H4"
                  />
                </svg>
              </button>
            </TooltipTrigger>
            <TooltipContent>
              {canAddTab
                ? 'Create new result tab'
                : `Maximum ${maxTabs} tabs allowed`}
            </TooltipContent>
          </Tooltip>
        </div>
      )}
    </div>
  );
};

export default memo(TabBar);
