// SPDX-License-Identifier: AGPL-3.0-or-later
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';

/**
 * RightSidebar - Unified sidebar for SQL Editor
 *
 * Features:
 * - Displays content based on active tab (controlled by parent)
 * - Resizable width
 * - Keeps all tab content mounted to preserve state
 * - Optional header action for each tab
 *
 * Tabs are now on the vertical icon bar in SQLEditor
 */
const RightSidebar = ({
  isOpen,
  onClose,
  width = 30, // percentage
  onResizeStart,
  activeTab, // 'catalog' or 'history'
  catalogContent,
  historyContent,
  catalogHeaderAction, // Optional action for catalog tab header
  historyHeaderAction, // Optional action for history tab header
}) => {
  const tabLabels = {
    catalog: 'Catalog',
    history: 'History',
  };

  const headerActions = {
    catalog: catalogHeaderAction,
    history: historyHeaderAction,
  };

  return (
    <div
      className={`flex-shrink-0 bg-card flex flex-col h-full relative transition-all duration-300 ${
        isOpen ? 'opacity-100' : 'opacity-0 w-0 overflow-hidden'
      }`}
      style={{ width: isOpen ? `${width}%` : '0' }}
    >
      {/* Resize Handle (left edge) - outside main content so it's always clickable */}
      <Tooltip>
        <TooltipTrigger asChild>
          <div
            className="absolute left-0 top-1/2 -translate-y-1/2 flex items-center justify-center cursor-col-resize z-10 px-1 -mx-2"
            onMouseDown={onResizeStart}
            aria-label="Drag to resize"
          >
            <div className="w-1 h-12 bg-border hover:bg-muted-foreground rounded transition-colors" />
          </div>
        </TooltipTrigger>
        <TooltipContent>Drag to resize</TooltipContent>
      </Tooltip>

      {/* Header */}
      <div className="px-4 h-11 border-b border-border bg-muted flex items-center justify-between">
        <h3 className="text-sm font-semibold text-foreground">
          {tabLabels[activeTab]}
        </h3>
        {headerActions[activeTab]}
      </div>

      {/* Tab Content - All tabs remain mounted, only active one visible */}
      <div className="flex-1 overflow-hidden relative">
        <div className={`absolute inset-0 overflow-hidden ${activeTab === 'catalog' ? 'block' : 'hidden'}`}>
          {catalogContent}
        </div>
        <div className={`absolute inset-0 overflow-hidden ${activeTab === 'history' ? 'block' : 'hidden'}`}>
          {historyContent}
        </div>
      </div>
    </div>
  );
};

export default RightSidebar;
