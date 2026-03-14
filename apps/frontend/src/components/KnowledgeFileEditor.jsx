// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useRef, useCallback } from 'react';
import { TiptapDashboardEditor } from './tiptap/TiptapDashboardEditor';
import MonacoMarkdownEditor from './MonacoMarkdownEditor';
import { useAuth } from '../context/AuthContext';
import { useTheme } from '../context/ThemeContext';
import { toast } from '../lib/toast';
import { Button } from './ui/button';
import { Code, Eye, Loader2, Check, AlertTriangle } from 'lucide-react';

/**
 * KnowledgeFileEditor — visual markdown editor for knowledge files with auto-save
 * and conflict detection (409 handling via content_hash).
 *
 * Uses TiptapDashboardEditor (WYSIWYG) by default, with a source mode toggle
 * for raw markdown editing via MonacoMarkdownEditor.
 *
 * Props:
 *   file        — selected file object (from list endpoint), or null
 *   filePath    — breadcrumb path string (e.g., "Folder / File.md")
 *   workspaceId — workspace ID string
 *   onSaved     — callback after successful save (to refresh tree)
 */
export default function KnowledgeFileEditor({ file, filePath, workspaceId, onSaved }) {
  const { apiClient } = useAuth();
  const { resolvedTheme } = useTheme();

  // Editor state
  const [content, setContent] = useState('');
  const [contentHash, setContentHash] = useState(null);
  const [loading, setLoading] = useState(false);
  const [saveStatus, setSaveStatus] = useState(null); // null | 'saving' | 'saved' | 'conflict'
  const [editorMode, setEditorMode] = useState('visual'); // 'visual' | 'source'
  const [fileMeta, setFileMeta] = useState(null); // { updated_at, updated_by }

  // Refs for debounce and stale-request prevention
  const debounceTimerRef = useRef(null);
  const loadedFileIdRef = useRef(null);
  const contentRef = useRef(content);
  contentRef.current = content;
  const contentHashRef = useRef(contentHash);
  contentHashRef.current = contentHash;

  // Load full file content when file prop changes
  useEffect(() => {
    if (!file || !workspaceId) {
      setContent('');
      setContentHash(null);
      setFileMeta(null);
      setSaveStatus(null);
      loadedFileIdRef.current = null;
      return;
    }

    // Cancel any pending save when switching files
    if (debounceTimerRef.current) {
      clearTimeout(debounceTimerRef.current);
      debounceTimerRef.current = null;
    }

    const fileId = file.id;
    loadedFileIdRef.current = fileId;
    setLoading(true);
    setSaveStatus(null);

    apiClient
      .get(`/api/v1/workspaces/${workspaceId}/knowledge-files/${fileId}`)
      .then((response) => {
        // Guard against stale response if user switched files
        if (loadedFileIdRef.current !== fileId) return;
        const data = response.data;
        setContent(data.content ?? '');
        setContentHash(data.content_hash);
        setFileMeta({ updated_at: data.updated_at, updated_by: data.updated_by });
        setLoading(false);
      })
      .catch((err) => {
        if (loadedFileIdRef.current !== fileId) return;
        toast.error('Failed to load file');
        setLoading(false);
      });
  }, [file?.id, workspaceId, apiClient]);

  // Save function
  const saveContent = useCallback(
    async (contentToSave, hashToSend) => {
      if (!file || !workspaceId) return;
      const fileId = file.id;

      setSaveStatus('saving');
      try {
        const response = await apiClient.patch(
          `/api/v1/workspaces/${workspaceId}/knowledge-files/${fileId}`,
          { content: contentToSave, content_hash: hashToSend }
        );
        // Guard against stale save if user switched files
        if (loadedFileIdRef.current !== fileId) return;

        const data = response.data;
        setContentHash(data.content_hash);
        setFileMeta({ updated_at: data.updated_at, updated_by: data.updated_by });
        setSaveStatus('saved');
        if (onSaved) onSaved();
      } catch (err) {
        if (loadedFileIdRef.current !== fileId) return;

        if (err.response?.status === 409) {
          setSaveStatus('conflict');
          toast.error('File was modified elsewhere. Reload?', {
            duration: 10000,
            action: {
              label: 'Reload',
              onClick: () => reloadFile(fileId),
            },
          });
        } else {
          setSaveStatus(null);
          toast.error('Failed to save file');
        }
      }
    },
    [file?.id, workspaceId, apiClient, onSaved]
  );

  // Reload after conflict
  const reloadFile = useCallback(
    async (fileId) => {
      try {
        const response = await apiClient.get(
          `/api/v1/workspaces/${workspaceId}/knowledge-files/${fileId}`
        );
        if (loadedFileIdRef.current !== fileId) return;
        const data = response.data;
        setContent(data.content ?? '');
        setContentHash(data.content_hash);
        setFileMeta({ updated_at: data.updated_at, updated_by: data.updated_by });
        setSaveStatus(null);
      } catch {
        toast.error('Failed to reload file');
      }
    },
    [workspaceId, apiClient]
  );

  // onChange handler with debounced auto-save
  const handleChange = useCallback(
    (newContent) => {
      setContent(newContent);
      setSaveStatus(null);

      // Clear previous debounce
      if (debounceTimerRef.current) {
        clearTimeout(debounceTimerRef.current);
      }

      // Schedule save after 1.5s of inactivity
      // Read latest values from refs at fire time, not schedule time,
      // to avoid stale contentHash if a save completes between scheduling and firing
      debounceTimerRef.current = setTimeout(() => {
        saveContent(contentRef.current, contentHashRef.current);
      }, 1500);
    },
    [saveContent]
  );

  // Cleanup debounce timer on unmount
  useEffect(() => {
    return () => {
      if (debounceTimerRef.current) {
        clearTimeout(debounceTimerRef.current);
      }
    };
  }, []);

  const isDark = resolvedTheme === 'dark';

  // Empty state: no file selected
  if (!file) {
    return (
      <div className="flex-1 flex items-center justify-center text-muted-foreground text-sm">
        Select a file to edit
      </div>
    );
  }

  // Loading state
  if (loading) {
    return (
      <div className="flex-1 flex items-center justify-center text-muted-foreground text-sm">
        <Loader2 className="w-4 h-4 animate-spin mr-2" />
        Loading...
      </div>
    );
  }

  // Mode toggle rendered in the Tiptap toolbar's rightSlot (visual mode)
  // or in the file info bar (source mode)
  const modeToggle = (
    <div className="flex items-center border border-border rounded-md overflow-hidden">
      <Button
        variant={editorMode === 'visual' ? 'secondary' : 'ghost'}
        size="sm"
        className="h-7 rounded-none px-2"
        onClick={() => setEditorMode('visual')}
      >
        <Eye className="w-3.5 h-3.5 mr-1" />
        Visual
      </Button>
      <Button
        variant={editorMode === 'source' ? 'secondary' : 'ghost'}
        size="sm"
        className="h-7 rounded-none px-2"
        onClick={() => setEditorMode('source')}
      >
        <Code className="w-3.5 h-3.5 mr-1" />
        Source
      </Button>
    </div>
  );

  return (
    <div className="flex-1 flex flex-col min-h-0">
      {/* File info toolbar */}
      <div className="flex items-center justify-between px-4 py-2 border-b border-border bg-card flex-shrink-0">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-foreground truncate max-w-[400px]">
            {filePath || file.name}
          </span>
          {/* Save status indicator */}
          {saveStatus === 'saving' && (
            <span className="flex items-center gap-1 text-xs text-muted-foreground">
              <Loader2 className="w-3 h-3 animate-spin" />
              Saving...
            </span>
          )}
          {saveStatus === 'saved' && (
            <span className="flex items-center gap-1 text-xs text-success-foreground">
              <Check className="w-3 h-3" />
              Saved
            </span>
          )}
          {saveStatus === 'conflict' && (
            <span className="flex items-center gap-1 text-xs text-destructive">
              <AlertTriangle className="w-3 h-3" />
              Conflict!
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          {/* Last modified info */}
          {fileMeta?.updated_at && (
            <span className="text-xs text-muted-foreground hidden md:inline">
              Updated {new Date(fileMeta.updated_at).toLocaleString()}
              {fileMeta.updated_by ? ` by ${fileMeta.updated_by}` : ''}
            </span>
          )}
          {modeToggle}
        </div>
      </div>

      {/* Content area */}
      <div className="flex-1 min-h-0 overflow-hidden">
        {editorMode === 'visual' ? (
          <TiptapDashboardEditor
            key={file.id}
            content={content}
            onChange={handleChange}
            placeholder="Start writing..."
            readOnly={saveStatus === 'conflict'}
          />
        ) : (
          <MonacoMarkdownEditor
            value={content}
            onChange={handleChange}
            placeholder="Start writing..."
            disabled={saveStatus === 'conflict'}
            editorTheme={isDark ? 'dark' : 'light'}
          />
        )}
      </div>
    </div>
  );
}
