// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useMemo, useCallback, useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { Button } from './ui/button';
import { Input } from './ui/input';
import {
  DndContext,
  useDraggable,
  useDroppable,
  DragOverlay,
  PointerSensor,
  useSensor,
  useSensors,
} from '@dnd-kit/core';
import {
  ChevronRight,
  ChevronDown,
  FolderOpen,
  Folder,
  FileText,
  Plus,
  Search,
  Trash2,
  Pencil,
  FolderInput,
  GripVertical,
} from 'lucide-react';

/**
 * KnowledgeFileTree — collapsible file tree sidebar with context menu,
 * drag-and-drop, and server-side content search.
 */

// --- Tree building ---

function buildTree(entries) {
  const map = new Map();
  const roots = [];

  for (const entry of entries) {
    map.set(entry.id, { ...entry, children: [] });
  }

  for (const entry of entries) {
    const node = map.get(entry.id);
    if (entry.parent_id && map.has(entry.parent_id)) {
      map.get(entry.parent_id).children.push(node);
    } else {
      roots.push(node);
    }
  }

  const sortNodes = (nodes) => {
    nodes.sort((a, b) => {
      if (a.is_folder !== b.is_folder) return a.is_folder ? -1 : 1;
      if (a.sort_order !== b.sort_order) return a.sort_order - b.sort_order;
      return a.name.localeCompare(b.name);
    });
    for (const node of nodes) {
      if (node.children.length > 0) sortNodes(node.children);
    }
  };
  sortNodes(roots);
  return roots;
}

// --- Context Menu ---

function ContextMenu({ x, y, node, folders, onClose, onRename, onDelete, onMove }) {
  const menuRef = useRef(null);

  useEffect(() => {
    const handleClickOutside = (e) => {
      if (menuRef.current && !menuRef.current.contains(e.target)) {
        onClose();
      }
    };
    const handleEscape = (e) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('mousedown', handleClickOutside);
    document.addEventListener('keydown', handleEscape);
    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
      document.removeEventListener('keydown', handleEscape);
    };
  }, [onClose]);

  // Filter folders: exclude the node itself and its descendants
  const moveTargets = useMemo(() => {
    const excludeIds = new Set();
    const collectDescendants = (id, entries) => {
      excludeIds.add(id);
      for (const f of entries) {
        if (f.parent_id === id) collectDescendants(f.id, entries);
      }
    };
    if (node.is_folder) collectDescendants(node.id, folders);
    else excludeIds.add(node.id);

    return [
      { id: null, name: '/ (root)' },
      ...folders
        .filter((f) => f.is_folder && !excludeIds.has(f.id) && f.id !== node.parent_id)
    ];
  }, [node, folders]);

  const [showMoveSubmenu, setShowMoveSubmenu] = useState(false);

  return createPortal(
    <div
      ref={menuRef}
      className="fixed z-[1200] min-w-[160px] rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-md"
      style={{ left: x, top: y }}
    >
      <button
        className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
        onClick={() => { onRename(node); onClose(); }}
      >
        <Pencil className="w-3.5 h-3.5" />
        Rename
      </button>
      <div
        className="relative"
        onMouseEnter={() => setShowMoveSubmenu(true)}
        onMouseLeave={() => setShowMoveSubmenu(false)}
      >
        <button
          className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground justify-between"
        >
          <span className="flex items-center gap-2">
            <FolderInput className="w-3.5 h-3.5" />
            Move to
          </span>
          <ChevronRight className="w-3 h-3" />
        </button>
        {showMoveSubmenu && (
          <div className="absolute left-full top-0 ml-1 min-w-[140px] rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-md">
            {moveTargets.length === 0 ? (
              <div className="px-2 py-1.5 text-xs text-muted-foreground">No folders available</div>
            ) : (
              moveTargets.map((folder) => (
                <button
                  key={folder.id ?? 'root'}
                  className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
                  onClick={() => { onMove(node.id, folder.id); onClose(); }}
                >
                  <Folder className="w-3.5 h-3.5 text-warning-foreground" />
                  {folder.name}
                </button>
              ))
            )}
          </div>
        )}
      </div>
      <div className="my-1 h-px bg-muted" />
      <button
        className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-sm text-destructive hover:bg-destructive/10"
        onClick={() => { onDelete(node); onClose(); }}
      >
        <Trash2 className="w-3.5 h-3.5" />
        Delete
      </button>
    </div>,
    document.body
  );
}

// --- Draggable Tree Node ---

function TreeNode({
  node,
  depth,
  selectedId,
  expandedFolders,
  onSelect,
  onToggleExpand,
  onContextMenu,
  dragOverId,
}) {
  const isExpanded = expandedFolders.has(node.id);
  const isSelected = selectedId === node.id;
  const isDragOver = dragOverId === node.id && node.is_folder;

  const { attributes, listeners, setNodeRef: setDragRef, isDragging } = useDraggable({
    id: node.id,
    data: { node },
  });

  const { setNodeRef: setDropRef, isOver } = useDroppable({
    id: node.id,
    data: { node },
    disabled: !node.is_folder,
  });

  // Combine refs for folder nodes (both draggable and droppable)
  const combinedRef = (el) => {
    setDragRef(el);
    if (node.is_folder) setDropRef(el);
  };

  return (
    <>
      <div
        ref={combinedRef}
        className={`flex items-center gap-1 px-2 py-1 cursor-pointer rounded text-sm group
          ${isSelected ? 'bg-accent text-accent-foreground' : 'text-foreground'}
          ${isDragOver || isOver ? 'bg-primary/10 ring-1 ring-primary/30' : ''}
          ${isDragging ? 'opacity-40' : ''}
          hover:bg-accent/50`}
        style={{ paddingLeft: `${depth * 16 + 8}px` }}
        onClick={() => {
          if (node.is_folder) {
            onToggleExpand(node.id);
          } else {
            onSelect(node);
          }
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          onContextMenu(e, node);
        }}
        {...attributes}
        {...listeners}
      >
        {/* Drag handle (visible on hover) */}
        <GripVertical className="w-3 h-3 flex-shrink-0 text-muted-foreground/50 opacity-0 group-hover:opacity-100 cursor-grab" />

        {node.is_folder ? (
          <>
            {isExpanded ? (
              <ChevronDown className="w-3.5 h-3.5 flex-shrink-0 text-muted-foreground" />
            ) : (
              <ChevronRight className="w-3.5 h-3.5 flex-shrink-0 text-muted-foreground" />
            )}
            {isExpanded ? (
              <FolderOpen className="w-4 h-4 flex-shrink-0 text-warning-foreground" />
            ) : (
              <Folder className="w-4 h-4 flex-shrink-0 text-warning-foreground" />
            )}
          </>
        ) : (
          <>
            <span className="w-3.5" />
            <FileText className="w-4 h-4 flex-shrink-0 text-muted-foreground" />
          </>
        )}
        <span className="truncate flex-1">{node.name}</span>
      </div>
      {node.is_folder && isExpanded && node.children.map((child) => (
        <TreeNode
          key={child.id}
          node={child}
          depth={depth + 1}
          selectedId={selectedId}
          expandedFolders={expandedFolders}
          onSelect={onSelect}
          onToggleExpand={onToggleExpand}
          onContextMenu={onContextMenu}
          dragOverId={dragOverId}
        />
      ))}
    </>
  );
}

// --- Main component ---

export default function KnowledgeFileTree({
  entries,
  selectedId,
  onSelect,
  onCreateFile,
  onCreateFolder,
  onDelete,
  onRename,
  onMove,
  onReorder,
  workspaceId,
  apiClient,
}) {
  const [searchFilter, setSearchFilter] = useState('');
  const [searchResults, setSearchResults] = useState(null);
  const [expandedFolders, setExpandedFolders] = useState(new Set());
  const [contextMenu, setContextMenu] = useState(null);
  const [dragOverId, setDragOverId] = useState(null);

  const debounceRef = useRef(null);

  const tree = useMemo(() => buildTree(entries), [entries]);

  // Server-side content search with debounce
  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);

    if (!searchFilter.trim()) {
      setSearchResults(null);
      return;
    }

    debounceRef.current = setTimeout(async () => {
      if (!workspaceId || !apiClient) return;
      try {
        const response = await apiClient.get(
          `/api/v1/workspaces/${workspaceId}/knowledge-files/search`,
          { params: { q: searchFilter.trim() } }
        );
        setSearchResults(response.data);
      } catch {
        // Fall back to client-side name filter on error
        const lower = searchFilter.toLowerCase();
        setSearchResults(
          entries.filter((e) => e.name.toLowerCase().includes(lower))
        );
      }
    }, 300);

    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [searchFilter, workspaceId, apiClient, entries]);

  const toggleExpand = (id) => {
    setExpandedFolders((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleContextMenu = useCallback((e, node) => {
    setContextMenu({ x: e.clientX, y: e.clientY, node });
  }, []);

  // Build path string for a search result entry
  const getEntryPath = useCallback(
    (entry) => {
      const parts = [];
      let currentId = entry.parent_id;
      const entryMap = new Map(entries.map((e) => [e.id, e]));
      while (currentId) {
        const parent = entryMap.get(currentId);
        if (!parent) break;
        parts.unshift(parent.name);
        currentId = parent.parent_id;
      }
      return parts.length > 0 ? parts.join(' / ') : null;
    },
    [entries]
  );

  // --- Drag and drop ---

  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { distance: 5 },
    })
  );

  const handleDragOver = useCallback((event) => {
    const { over } = event;
    if (over?.data?.current?.node?.is_folder) {
      setDragOverId(over.id);
    } else {
      setDragOverId(null);
    }
  }, []);

  const handleDragEnd = useCallback(
    (event) => {
      setDragOverId(null);
      const { active, over } = event;
      if (!over || active.id === over.id) return;

      const draggedNode = active.data.current?.node;
      const targetNode = over.data.current?.node;
      if (!draggedNode || !targetNode) return;

      // Only allow dropping onto folders
      if (targetNode.is_folder) {
        // Don't move a folder into itself or its descendants
        if (draggedNode.is_folder) {
          const isDescendant = (parentId, checkId) => {
            let current = parentId;
            const entryMap = new Map(entries.map((e) => [e.id, e]));
            while (current) {
              if (current === checkId) return true;
              current = entryMap.get(current)?.parent_id;
            }
            return false;
          };
          if (isDescendant(targetNode.id, draggedNode.id) || targetNode.id === draggedNode.id) return;
        }

        // Don't move if already in that folder
        if (draggedNode.parent_id === targetNode.id) return;

        onMove(draggedNode.id, targetNode.id);

        // Auto-expand the target folder
        setExpandedFolders((prev) => new Set([...prev, targetNode.id]));
      }
    },
    [entries, onMove]
  );

  // Determine what to display
  const isSearching = searchFilter.trim().length > 0;
  const displayResults = isSearching ? searchResults : null;

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-3 py-2 border-b border-border flex items-center justify-between">
        <span className="text-sm font-medium text-foreground">Files</span>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            onClick={() => onCreateFile()}
            title="New File"
          >
            <Plus className="w-3.5 h-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            onClick={() => onCreateFolder()}
            title="New Folder"
          >
            <Folder className="w-3.5 h-3.5" />
          </Button>
        </div>
      </div>

      {/* Search */}
      <div className="px-3 py-2">
        <div className="relative">
          <Search className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground" />
          <Input
            value={searchFilter}
            onChange={(e) => setSearchFilter(e.target.value)}
            placeholder="Search files..."
            className="h-7 text-xs pl-7"
          />
        </div>
      </div>

      {/* Tree / Search results */}
      <div className="flex-1 overflow-y-auto py-1">
        {isSearching ? (
          // Search results (flat list)
          displayResults === null ? (
            <div className="px-3 py-4 text-center text-muted-foreground text-xs">
              Searching...
            </div>
          ) : displayResults.length === 0 ? (
            <div className="px-3 py-8 text-center text-muted-foreground text-xs">
              No files match your search.
            </div>
          ) : (
            displayResults.map((entry) => {
              const folderPath = getEntryPath(entry);
              return (
                <div
                  key={entry.id}
                  className={`flex items-center gap-1 px-3 py-1.5 cursor-pointer rounded text-sm hover:bg-accent/50 ${
                    selectedId === entry.id ? 'bg-accent text-accent-foreground' : 'text-foreground'
                  }`}
                  onClick={() => !entry.is_folder && onSelect(entry)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    handleContextMenu(e, entry);
                  }}
                >
                  {entry.is_folder ? (
                    <Folder className="w-4 h-4 flex-shrink-0 text-warning-foreground" />
                  ) : (
                    <FileText className="w-4 h-4 flex-shrink-0 text-muted-foreground" />
                  )}
                  <div className="flex flex-col min-w-0 flex-1">
                    <span className="truncate">{entry.name}</span>
                    {folderPath && (
                      <span className="text-xs text-muted-foreground truncate">{folderPath}</span>
                    )}
                  </div>
                </div>
              );
            })
          )
        ) : entries.length === 0 ? (
          <div className="px-3 py-8 text-center text-muted-foreground text-xs">
            No knowledge files yet. Click + to create one.
          </div>
        ) : (
          // Normal tree view with drag-and-drop
          <DndContext
            sensors={sensors}
            onDragOver={handleDragOver}
            onDragEnd={handleDragEnd}
          >
            {tree.map((node) => (
              <TreeNode
                key={node.id}
                node={node}
                depth={0}
                selectedId={selectedId}
                expandedFolders={expandedFolders}
                onSelect={onSelect}
                onToggleExpand={toggleExpand}
                onContextMenu={handleContextMenu}
                dragOverId={dragOverId}
              />
            ))}
            <DragOverlay>
              {/* Minimal drag preview */}
            </DragOverlay>
          </DndContext>
        )}
      </div>

      {/* Context menu */}
      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          node={contextMenu.node}
          folders={entries}
          onClose={() => setContextMenu(null)}
          onRename={onRename}
          onDelete={onDelete}
          onMove={onMove}
        />
      )}
    </div>
  );
}
