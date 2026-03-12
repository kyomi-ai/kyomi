// SPDX-License-Identifier: AGPL-3.0-or-later
import { useRef, useEffect, forwardRef, useImperativeHandle, useCallback, useState } from 'react';
import Editor from '@monaco-editor/react';

// Configure Monaco environment to suppress web worker warnings
if (typeof window !== 'undefined') {
  window.MonacoEnvironment = {
    getWorker: function () {
      // Return a dummy worker to suppress warnings
      // We don't need workers for our use case
      return new Worker(
        URL.createObjectURL(
          new Blob(['self.onmessage = () => {}'], { type: 'text/javascript' })
        )
      );
    }
  };
}

/**
 * MonacoSQLEditor - UNCONTROLLED SQL editor using Monaco Editor
 *
 * Features:
 * - SQL syntax highlighting
 * - Cursor position display (updated via DOM, no re-renders)
 * - Keyboard shortcuts (Cmd/Ctrl+Enter to run query)
 * - Line numbers and active line highlighting
 * - Custom placeholder text
 *
 * IMPORTANT: This is an UNCONTROLLED component
 * - value prop is only used for INITIAL value (defaultValue)
 * - Parent component does NOT control the editor content
 * - Use ref.getValue() to get current value
 * - Use ref.getSelectedOrFullText() to get selection or full text
 * - Use ref.setValue(text) to set value programmatically
 * - Use ref.insertTextAtCursor(text) to insert at cursor
 * - onChange still fires for side effects (dry run validation)
 * - This architecture prevents re-renders and cursor jumping
 *
 * @param {Object} props
 * @param {string} props.value - Initial SQL text value (used only on mount as defaultValue)
 * @param {Function} props.onChange - Called when user types OR changes selection (for parent side-effects like dry run)
 * @param {Function} props.onCursorChange - Optional callback when cursor moves, receives {line, column}
 * @param {Function} props.onMount - Optional callback when editor finishes mounting
 * @param {Function} props.onRunQuery - Called when Cmd/Ctrl+Enter is pressed
 * @param {string} props.placeholder - Placeholder text shown when editor is empty
 * @param {boolean} props.disabled - If true, editor is read-only
 * @param {number} props.fontSize - Font size in pixels (default: 12, scales with root font size)
 * @param {React.Ref} ref - Ref exposing getValue(), getSelectedOrFullText(), setValue(), insertTextAtCursor()
 */
const MonacoSQLEditor = forwardRef(({
  value = '',
  onChange,
  onCursorChange,
  onMount,
  onRunQuery,
  placeholder = 'Enter SQL query...',
  disabled = false,
  fontSize = 11,  // Reduced from 12 to maintain proportion with 14px root (was 12px with 16px root)
  editorTheme = 'light',  // 'light' or 'dark' - controls Monaco theme
}, ref) => {
  const editorRef = useRef(null);
  const monacoRef = useRef(null);
  const isUpdatingFromParent = useRef(false);
  const isUserTyping = useRef(false);
  const lastValueRef = useRef(value);

  // Refs for values used inside Monaco command closures (bound once at mount).
  // Without refs, the closure captures stale prop values and never updates.
  const onRunQueryRef = useRef(onRunQuery);
  onRunQueryRef.current = onRunQuery;
  const disabledRef = useRef(disabled);
  disabledRef.current = disabled;
  const cursorPositionRef = useRef(null); // Ref to cursor position DOM element
  const [isEmpty, setIsEmpty] = useState(!value); // Track if editor is empty for placeholder

  // Expose methods to parent via ref
  // This allows parent components to interact with the editor imperatively
  // without causing re-renders through props
  useImperativeHandle(ref, () => ({
    /**
     * Get the current value of the editor
     * @returns {string} Current SQL text
     */
    getValue: () => {
      return editorRef.current?.getValue() || '';
    },

    /**
     * Set the editor value programmatically (e.g., from SQLCopilot)
     * @param {string} text - New SQL text to set
     */
    setValue: (text) => {
      if (editorRef.current) {
        const editor = editorRef.current;
        const position = editor.getPosition();

        // Update value
        editor.setValue(text);

        // Update isEmpty state for placeholder visibility
        setIsEmpty(!text || text.trim().length === 0);

        // Try to restore cursor position if valid
        if (position) {
          const model = editor.getModel();
          if (model) {
            const lineCount = model.getLineCount();
            if (position.lineNumber <= lineCount) {
              editor.setPosition(position);
            }
          }
        }

        editor.focus();
      }
    },

    /**
     * Insert text at current cursor position
     * @param {string} text - Text to insert
     */
    insertTextAtCursor: (text) => {
      if (editorRef.current) {
        const editor = editorRef.current;
        const position = editor.getPosition();
        const range = new monacoRef.current.Range(
          position.lineNumber,
          position.column,
          position.lineNumber,
          position.column
        );

        editor.executeEdits('', [{
          range: range,
          text: text,
          forceMoveMarkers: true
        }]);

        editor.focus();
      }
    },

    /**
     * Get the selected text if any, otherwise get the full editor value
     * @returns {string} Selected SQL text or full editor content
     */
    getSelectedOrFullText: () => {
      if (!editorRef.current) {
        return '';
      }

      const editor = editorRef.current;
      const selection = editor.getSelection();
      const model = editor.getModel();

      if (!model) {
        return '';
      }

      // Check if there's a non-empty selection
      if (selection && !selection.isEmpty()) {
        const selectedText = model.getValueInRange(selection);
        return selectedText;
      }

      // No selection, return full text
      const fullText = editor.getValue() || '';
      return fullText;
    },

    /**
     * Set error markers in the editor (red squiggly underlines)
     * @param {Array} errors - Array of error objects with { line, column, message }
     * Pass empty array to clear markers
     */
    setErrorMarkers: (errors) => {
      if (!editorRef.current || !monacoRef.current) {
        return;
      }

      const model = editorRef.current.getModel();
      if (!model) {
        return;
      }

      const markers = errors.map(error => ({
        severity: monacoRef.current.MarkerSeverity.Error,
        startLineNumber: error.line,
        startColumn: error.column || 1,
        endLineNumber: error.line,
        endColumn: error.endColumn || (error.column || 1) + (error.length || 100), // Underline token or rest of line
        message: error.message
      }));

      // Set markers on the model (owner: 'sql-validator')
      monacoRef.current.editor.setModelMarkers(model, 'sql-validator', markers);
    }
  }));

  // Update editor when value prop changes (for external updates like tab switching)
  // But NOT when user is typing (prevents cursor jumping)
  useEffect(() => {
    if (!editorRef.current) return;
    if (isUserTyping.current) return;

    const currentValue = editorRef.current.getValue();
    if (value !== currentValue) {
      const position = editorRef.current.getPosition();
      editorRef.current.setValue(value);
      if (position) {
        editorRef.current.setPosition(position);
      }
    }
  }, [value]);

  // Handle editor mount
  const handleEditorDidMount = (editor, monaco) => {
    editorRef.current = editor;
    monacoRef.current = monaco;

    // Define custom light theme - softer versions of default Monaco colors
    monaco.editor.defineTheme('kyomi-light', {
      base: 'vs',
      inherit: true,
      rules: [
        { token: 'comment', foreground: '6a9955' }, // Softer green (default: 008000)
        { token: 'keyword', foreground: '4169c0' }, // Softer blue (default: 0000ff)
        { token: 'string', foreground: 'b85c5c' }, // Softer red (default: a31515)
        { token: 'number', foreground: '3fa371' }, // Softer green (default: 098658)
        { token: 'type', foreground: '4d8f8f' }, // Softer teal (default: 267f99)
        { token: 'predefined', foreground: '6b6bb8' }, // Softer purple for functions
      ],
      colors: {
        'editor.background': '#ffffff',
        'editor.foreground': '#333333',
        'editor.lineHighlightBackground': '#f5f5f5',
        'editor.selectionBackground': '#c9d6e8',
      }
    });

    // Define custom dark theme - matches Kyomi dark mode palette
    monaco.editor.defineTheme('kyomi-dark', {
      base: 'vs-dark',
      inherit: true,
      rules: [
        { token: 'comment', foreground: '6a9955' },
        { token: 'keyword', foreground: '569cd6' },
        { token: 'string', foreground: 'ce9178' },
        { token: 'number', foreground: 'b5cea8' },
        { token: 'type', foreground: '4ec9b0' },
        { token: 'predefined', foreground: 'dcdcaa' },
      ],
      colors: {
        'editor.background': '#262626', // --color-card in dark mode
        'editor.foreground': '#f1f5f9', // --color-foreground in dark mode
        'editor.lineHighlightBackground': '#383838', // --color-border in dark mode
        'editor.selectionBackground': '#3b5998',
      }
    });

    // Apply theme based on current mode
    monaco.editor.setTheme(editorTheme === 'dark' ? 'kyomi-dark' : 'kyomi-light');

    // Add Cmd/Ctrl+Enter keyboard shortcut to run query.
    // Uses refs (onRunQueryRef, disabledRef) so the closure always reads the
    // latest prop values — editor.addCommand only runs once at mount.
    editor.addCommand(
      monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter,
      () => {
        const currentValue = editor.getValue() || '';
        if (currentValue.trim() && !disabledRef.current && onRunQueryRef.current) {
          onRunQueryRef.current();
        }
      }
    );

    // Track cursor position changes - update DOM directly (no re-renders)
    editor.onDidChangeCursorPosition((e) => {
      // Update cursor position display directly via DOM manipulation
      if (cursorPositionRef.current) {
        cursorPositionRef.current.textContent = `Ln ${e.position.lineNumber}, Col ${e.position.column}`;
      }

      // Also call parent callback if provided (for other side effects)
      if (onCursorChange) {
        onCursorChange({
          line: e.position.lineNumber,
          column: e.position.column
        });
      }
    });

    // Track selection changes - trigger onChange for dry run validation
    editor.onDidChangeCursorSelection(() => {

      // When selection changes, trigger onChange so dry run re-validates
      // This allows dry run to work on highlighted text
      if (onChange && !isUpdatingFromParent.current) {
        // Mark that user is typing to prevent external value updates
        isUserTyping.current = true;
        setTimeout(() => {
          isUserTyping.current = false;
        }, 100);
        onChange(editor.getValue());
      }
    });

    // Focus editor
    editor.focus();

    // Notify parent that editor is ready
    if (onMount) {
      onMount();
    }
  };

  // NOTE: This component is now UNCONTROLLED
  // The 'value' prop is only used for initial value when editor mounts
  // To update the editor programmatically, use ref.setValue()
  // This prevents re-renders and cursor jumping issues

  // Update read-only state when disabled changes
  useEffect(() => {
    if (editorRef.current) {
      editorRef.current.updateOptions({ readOnly: disabled });
    }
  }, [disabled]);

  // Update Monaco theme when editorTheme prop changes (e.g., user toggles dark mode)
  useEffect(() => {
    if (monacoRef.current) {
      monacoRef.current.editor.setTheme(editorTheme === 'dark' ? 'kyomi-dark' : 'kyomi-light');
    }
  }, [editorTheme]);

  // Call onChange immediately for parent side effects
  // Note: Parent should NOT store this in state if component is uncontrolled
  const handleChange = useCallback((newValue) => {
    // Update isEmpty state for placeholder visibility
    setIsEmpty(!newValue || newValue.trim().length === 0);

    if (!isUpdatingFromParent.current && onChange) {
      lastValueRef.current = newValue;
      onChange(newValue);
    }
  }, [onChange]);

  return (
    <div className="relative h-full w-full">
      {/* Placeholder overlay - shown when editor is empty */}
      {isEmpty && placeholder && (
        <div
          className="absolute inset-0 pointer-events-none z-10 p-4 text-muted-foreground whitespace-pre-wrap"
          style={{
            fontSize: `${fontSize}px`,
            lineHeight: '1.5'
          }}
        >
          {placeholder}
        </div>
      )}

      {/* Cursor position indicator - updated via DOM manipulation (no re-renders) */}
      <div
        ref={cursorPositionRef}
        className="absolute bottom-1 right-2 z-10 px-2 py-0.5 text-xs text-muted-foreground bg-muted border border-border rounded pointer-events-none select-none font-mono"
      >
        Ln 1, Col 1
      </div>

      <Editor
        height="100%"
        language="mysql"
        defaultValue={value}
        onChange={handleChange}
        onMount={handleEditorDidMount}
        options={{
          // Basic editor options
          fontSize,
          lineNumbers: 'on',

          // Highlighting
          renderLineHighlight: 'all',
          renderLineHighlightOnlyWhenFocus: false,

          // Minimap
          minimap: {
            enabled: false
          },

          // Scrollbar
          scrollbar: {
            vertical: 'auto',
            horizontal: 'auto',
            verticalScrollbarSize: 10,
            horizontalScrollbarSize: 10
          },
          scrollBeyondLastLine: false,

          // Read-only state
          readOnly: disabled,

          // Selection and cursor
          cursorStyle: 'line',
          cursorBlinking: 'blink',
          selectOnLineNumbers: true,

          // Indentation
          tabSize: 2,
          insertSpaces: true,
          detectIndentation: true,

          // Brackets
          matchBrackets: 'always',
          autoClosingBrackets: 'always',
          autoClosingQuotes: 'always',

          // Word wrap
          wordWrap: 'off',

          // Suggestions / autocomplete
          quickSuggestions: false,
          suggestOnTriggerCharacters: false,

          // Other features
          folding: false,
          glyphMargin: false,
          lineDecorationsWidth: 5,
          lineNumbersMinChars: 3,

          // Performance
          automaticLayout: true
        }}
      />
    </div>
  );
});

MonacoSQLEditor.displayName = 'MonacoSQLEditor';

export default MonacoSQLEditor;
