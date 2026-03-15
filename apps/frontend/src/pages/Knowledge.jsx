// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useCallback } from 'react';
import KnowledgeFileTree from '../components/KnowledgeFileTree';
import KnowledgeFileEditor from '../components/KnowledgeFileEditor';
import CreateKnowledgeItemModal from '../components/CreateKnowledgeItemModal';
import { useAuth } from '../context/AuthContext';
import { toast } from '../lib/toast';
import apiClient from '../api/apiClient';
import { Plus, FolderPlus } from 'lucide-react';
import { Button } from '../components/ui/button';

/**
 * Knowledge - Workspace knowledge base with file tree + editor.
 *
 * Layout: header + flex row (sidebar 288px + editor pane).
 * Data fetched via REST from /api/v1/workspaces/{workspaceId}/knowledge-files.
 */
const Knowledge = () => {
  const { user } = useAuth();
  const workspaceId = user?.workspace_id;

  const [tree, setTree] = useState([]);
  const [loading, setLoading] = useState(true);
  const [selectedFile, setSelectedFile] = useState(null);
  const [selectedFilePath, setSelectedFilePath] = useState('');

  // Modal state
  const [modalState, setModalState] = useState({
    show: false,
    title: '',
    defaultValue: '',
    submitLabel: 'Create',
    onSubmit: null,
  });

  // ---------------------------------------------------------------------------
  // Fetch tree
  // ---------------------------------------------------------------------------

  const fetchTree = useCallback(async () => {
    if (!workspaceId) return;
    try {
      const response = await apiClient.get(
        `/api/v1/workspaces/${workspaceId}/knowledge-files`
      );
      setTree(response.data);
    } catch (err) {
      toast.error('Failed to load knowledge files');
    } finally {
      setLoading(false);
    }
  }, [workspaceId]);

  useEffect(() => {
    fetchTree();
  }, [fetchTree]);

  // ---------------------------------------------------------------------------
  // CRUD handlers
  // ---------------------------------------------------------------------------

  const handleCreateFile = useCallback(
    (parentId = null) => {
      setModalState({
        show: true,
        title: 'New File',
        defaultValue: '',
        submitLabel: 'Create',
        onSubmit: async (name) => {
          try {
            await apiClient.post(
              `/api/v1/workspaces/${workspaceId}/knowledge-files`,
              { name, parent_id: parentId, is_folder: false }
            );
            await fetchTree();
            toast.success(`Created ${name}`);
          } catch (err) {
            toast.error(err?.response?.data?.error || 'Failed to create file');
          }
        },
      });
    },
    [workspaceId, fetchTree]
  );

  const handleCreateFolder = useCallback(
    (parentId = null) => {
      setModalState({
        show: true,
        title: 'New Folder',
        defaultValue: '',
        submitLabel: 'Create',
        onSubmit: async (name) => {
          try {
            await apiClient.post(
              `/api/v1/workspaces/${workspaceId}/knowledge-files`,
              { name, parent_id: parentId, is_folder: true }
            );
            await fetchTree();
            toast.success(`Created ${name}`);
          } catch (err) {
            toast.error(err?.response?.data?.error || 'Failed to create folder');
          }
        },
      });
    },
    [workspaceId, fetchTree]
  );

  const handleRename = useCallback(
    (file) => {
      setModalState({
        show: true,
        title: 'Rename',
        defaultValue: file.name,
        submitLabel: 'Rename',
        onSubmit: async (name) => {
          try {
            await apiClient.patch(
              `/api/v1/workspaces/${workspaceId}/knowledge-files/${file.id}`,
              { name }
            );
            await fetchTree();
            if (selectedFile?.id === file.id) {
              setSelectedFile((prev) => prev && { ...prev, name });
            }
            toast.success(`Renamed to ${name}`);
          } catch (err) {
            toast.error(err?.response?.data?.error || 'Failed to rename');
          }
        },
      });
    },
    [workspaceId, fetchTree, selectedFile]
  );

  const handleDelete = useCallback(
    async (file) => {
      if (!window.confirm(`Delete "${file.name}"? This cannot be undone.`)) return;
      try {
        await apiClient.delete(
          `/api/v1/workspaces/${workspaceId}/knowledge-files/${file.id}`
        );
        if (selectedFile?.id === file.id) {
          setSelectedFile(null);
          setSelectedFilePath('');
        }
        await fetchTree();
        toast.success(`Deleted ${file.name}`);
      } catch (err) {
        toast.error(err?.response?.data?.error || 'Failed to delete');
      }
    },
    [workspaceId, fetchTree, selectedFile]
  );

  const handleMove = useCallback(
    async (fileId, newParentId, sortOrder) => {
      try {
        await apiClient.patch(
          `/api/v1/workspaces/${workspaceId}/knowledge-files/${fileId}`,
          { parent_id: newParentId, sort_order: sortOrder }
        );
        await fetchTree();
      } catch (err) {
        toast.error(err?.response?.data?.error || 'Failed to move');
      }
    },
    [workspaceId, fetchTree]
  );

  // ---------------------------------------------------------------------------
  // File selection
  // ---------------------------------------------------------------------------

  const buildPath = useCallback(
    (file) => {
      const parts = [];
      let current = file;
      while (current) {
        parts.unshift(current.name);
        current = tree.find((e) => e.id === current.parent_id);
      }
      return parts.join(' / ');
    },
    [tree]
  );

  const handleSelectFile = useCallback(
    (file) => {
      if (file.is_folder) return;
      setSelectedFile(file);
      setSelectedFilePath(buildPath(file));
    },
    [buildPath]
  );

  // ---------------------------------------------------------------------------
  // Render
  // ---------------------------------------------------------------------------

  return (
    <div className="flex flex-col h-full bg-muted" style={{ flexDirection: 'column' }}>
      {/* Header */}
      <div className="h-16 border-b border-border bg-card px-6 flex-shrink-0 flex items-center justify-between">
        <h1 className="text-2xl font-semibold text-foreground">Knowledge</h1>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="sm" onClick={() => handleCreateFile()}>
            <Plus className="h-4 w-4 mr-1" />
            New File
          </Button>
          <Button variant="outline" size="sm" onClick={() => handleCreateFolder()}>
            <FolderPlus className="h-4 w-4 mr-1" />
            New Folder
          </Button>
        </div>
      </div>

      {/* Content: sidebar + editor */}
      <div className="flex-1 flex overflow-hidden">
        {/* Sidebar */}
        <div className="w-72 flex-shrink-0 border-r border-border bg-card overflow-y-auto">
          <KnowledgeFileTree
            entries={tree}
            selectedId={selectedFile?.id}
            onSelect={handleSelectFile}
            onCreateFile={handleCreateFile}
            onCreateFolder={handleCreateFolder}
            onRename={(fileId) => {
              const entry = tree.find((e) => e.id === fileId);
              if (entry) handleRename(entry);
            }}
            onDelete={(fileId) => {
              const entry = tree.find((e) => e.id === fileId);
              if (entry) handleDelete(entry);
            }}
            onMove={handleMove}
            workspaceId={workspaceId}
            apiClient={apiClient}
          />
        </div>

        {/* Editor pane */}
        <div className="flex-1 min-h-0 min-w-0 overflow-hidden">
          <KnowledgeFileEditor
            file={selectedFile}
            filePath={selectedFilePath}
            workspaceId={workspaceId}
            onSaved={fetchTree}
          />
        </div>
      </div>

      {/* Modal */}
      <CreateKnowledgeItemModal
        show={modalState.show}
        onClose={() => setModalState((s) => ({ ...s, show: false }))}
        onSubmit={async (name) => {
          if (modalState.onSubmit) {
            await modalState.onSubmit(name);
          }
          setModalState((s) => ({ ...s, show: false }));
        }}
        title={modalState.title}
        defaultValue={modalState.defaultValue}
        submitLabel={modalState.submitLabel}
      />
    </div>
  );
};

export default Knowledge;
