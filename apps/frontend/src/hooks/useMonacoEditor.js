// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * useMonacoEditor Hook
 *
 * Provides shared Monaco editor setup for ChartML editors.
 * Handles worker environment configuration and editor/monaco refs.
 *
 * @returns {Object} { handleEditorWillMount, handleEditorDidMount, editorRef, monacoRef }
 *
 * @example
 * function MyEditor() {
 *   const { handleEditorWillMount, handleEditorDidMount, editorRef, monacoRef } = useMonacoEditor();
 *  
 *   return (
 *     <Editor
 *       beforeMount={handleEditorWillMount}
 *       onMount={handleEditorDidMount}
 *       ...
 *     />
 *   );
 * }
 */

import { useRef, useCallback } from 'react';

// Import workers using Vite's ?worker suffix
import editorWorker from 'monaco-editor/esm/vs/editor/editor.worker?worker';

export function useMonacoEditor() {
  const editorRef = useRef(null);
  const monacoRef = useRef(null);

  // Configure Monaco before editor mounts
  const handleEditorWillMount = useCallback((monaco) => {
    monacoRef.current = monaco;

    // Configure worker environment for Vite (only once)
    if (!window.MonacoEnvironment) {
      window.MonacoEnvironment = {
        getWorker(moduleId, label) {
          // Only need the editor worker for chartml (no YAML worker needed)
          if (label === 'editorWorkerService' || label === 'chartml') {
            return new editorWorker();
          }
          throw new Error(`Unknown worker label: ${label}`);
        }
      };
    }
  }, []);

  // Configure Monaco editor when it mounts
  const handleEditorDidMount = useCallback((editor, monaco) => {
    editorRef.current = editor;
    monacoRef.current = monaco;
  }, []);

  return {
    handleEditorWillMount,
    handleEditorDidMount,
    editorRef,
    monacoRef
  };
}
