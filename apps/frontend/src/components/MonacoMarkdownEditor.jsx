// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useRef, useEffect, useState } from 'react';
import Editor from '@monaco-editor/react';
import {
  registerChartmlLanguage,
  registerChartmlCompletionProvider,
  provideChartmlCompletions,
  validateChartmlInMarkdown
} from '../lib/chartmlLanguage';
import { getChartmlSchema } from '../schemas/schemaService';

// Track if markdown completion provider is registered (module-level, persists across component mounts)
let markdownChartmlProviderRegistered = false;

/**
 * MonacoMarkdownEditor - Markdown editor using Monaco Editor
 *
 * A drop-in replacement for CodeMirror Markdown editor with:
 * - Markdown syntax highlighting
 * - Cursor position tracking
 * - Scroll position tracking (for sync with preview)
 * - Line numbers and active line highlighting
 * - Custom placeholder text
 *
 * Note: ChartML code blocks will have YAML syntax highlighting but no
 * schema validation. Use Chart Builder Modal for editing ChartML.
 *
 * @param {Object} props
 * @param {string} props.value - Markdown text value (controlled)
 * @param {Function} props.onChange - Called when text changes
 * @param {Function} props.onCursorChange - Called when cursor moves, receives {line, column}
 * @param {Function} props.onScroll - Called when editor scrolls, receives scroll position
 * @param {string} props.placeholder - Placeholder text shown when editor is empty
 * @param {boolean} props.disabled - If true, editor is read-only
 * @param {number} props.fontSize - Font size in pixels (default: 13)
 * @param {Object} props.editorRef - Ref to expose editor instance to parent
 * @param {boolean} props.skipBigQuery - Skip BigQuery dry runs for faster validation (default: false)
 */
const MonacoMarkdownEditor = ({
  value = '',
  onChange,
  onCursorChange,
  onScroll,
  onMount,
  placeholder = 'Start typing markdown...',
  disabled = false,
  fontSize = 11,  // Reduced from 13 to maintain proportion with 14px root (was 13px with 16px root)
  editorRef: externalEditorRef,
  skipBigQuery = true,
  editorTheme = 'light',  // 'light' or 'dark' - controls Monaco theme
}) => {
  const editorRef = useRef(null);
  const monacoRef = useRef(null);
  const [schema, setSchema] = useState(null);
  const schemaValidationTimerRef = useRef(null);

  // Load ChartML schema on mount - BEFORE mounting editor
  useEffect(() => {
    getChartmlSchema()
      .then(schema => {
        setSchema(schema);
      })
      .catch(error => {
      });
  }, []);

  // Handle before editor mount - register chartml language BEFORE editor renders
  const handleEditorWillMount = (monaco) => {
    if (!schema) return;

    try {
      // Register chartml language with full autocomplete support (idempotent - skips if already registered)
      registerChartmlLanguage(monaco, schema);

      // Register completion provider for chartml (idempotent - skips if already registered)
      registerChartmlCompletionProvider(monaco, schema);

      // Register completion provider for MARKDOWN that delegates to chartml inside code blocks
      // (idempotent - skip if already registered)
      if (markdownChartmlProviderRegistered) {
        return;
      }

      monaco.languages.registerCompletionItemProvider('markdown', {
        triggerCharacters: [' ', ':', '\n', '-'],

        provideCompletionItems: (model, position) => {
          // Get full document text
          const fullText = model.getValue();
          const lines = fullText.split('\n');

          // Find if we're inside a chartml code block
          let inChartmlBlock = false;
          let blockStartLine = -1;

          for (let i = 0; i < position.lineNumber; i++) {
            const line = lines[i];
            if (/^```chartml\s*$/.test(line)) {
              inChartmlBlock = true;
              blockStartLine = i;
            } else if (inChartmlBlock && /^```\s*$/.test(line)) {
              inChartmlBlock = false;
              blockStartLine = -1;
            }
          }

          // If we're inside a chartml block, provide chartml completions
          if (inChartmlBlock && blockStartLine >= 0) {

            // Extract chartml content from the code block
            const blockLines = [];
            for (let i = blockStartLine + 1; i < lines.length; i++) {
              if (/^```\s*$/.test(lines[i])) break;
              blockLines.push(lines[i]);
            }

            const chartmlContent = blockLines.join('\n');
            const relativeLineNumber = position.lineNumber - blockStartLine - 1;

            // Create sub-model with just the chartml content (standard Monaco pattern)
            const uri = monaco.Uri.parse('inmemory://chartml-temp.yaml');
            let tempModel = monaco.editor.getModel(uri);
            if (tempModel) {
              tempModel.setValue(chartmlContent);
            } else {
              tempModel = monaco.editor.createModel(chartmlContent, 'chartml', uri);
            }

            // Query completions from chartml provider using sub-model
            const tempPosition = new monaco.Position(relativeLineNumber, position.column);
            const result = provideChartmlCompletions(monaco, schema, tempModel, tempPosition);

            return result;
          }

          // Not in a chartml block, no suggestions
          return { suggestions: [] };
        }
      });

      markdownChartmlProviderRegistered = true;
    } catch (error) {
    }
  };

  // Handle editor mount
  const handleEditorDidMount = async (editor, monaco) => {
    editorRef.current = editor;
    monacoRef.current = monaco;

    // Define custom themes for markdown (same chrome colors as SQL editor)
    monaco.editor.defineTheme('kyomi-md-light', {
      base: 'vs',
      inherit: true,
      rules: [],
      colors: {
        'editor.background': '#ffffff',
        'editor.foreground': '#333333',
        'editor.lineHighlightBackground': '#f5f5f5',
        'editor.selectionBackground': '#c9d6e8',
      }
    });

    monaco.editor.defineTheme('kyomi-md-dark', {
      base: 'vs-dark',
      inherit: true,
      rules: [],
      colors: {
        'editor.background': '#262626',
        'editor.foreground': '#f1f5f9',
        'editor.lineHighlightBackground': '#383838',
        'editor.selectionBackground': '#3b5998',
      }
    });

    // Apply theme based on current mode
    monaco.editor.setTheme(editorTheme === 'dark' ? 'kyomi-md-dark' : 'kyomi-md-light');

    // Expose editor to parent if ref provided
    if (externalEditorRef) {
      externalEditorRef.current = editor;
    }

    // Track cursor position changes
    if (onCursorChange) {
      editor.onDidChangeCursorPosition((e) => {
        onCursorChange({
          line: e.position.lineNumber,
          column: e.position.column
        });
      });
    }

    // Track scroll position changes
    if (onScroll) {
      editor.onDidScrollChange((e) => {
        onScroll({
          scrollTop: e.scrollTop,
          scrollLeft: e.scrollLeft,
          scrollHeight: e.scrollHeight,
          scrollWidth: e.scrollWidth
        });
      });
    }

    // Focus editor
    editor.focus();

    // Call onMount callback if provided
    if (onMount) {
      onMount(editor, monaco);
    }
  };

  // Update read-only state when disabled changes
  useEffect(() => {
    if (editorRef.current) {
      editorRef.current.updateOptions({ readOnly: disabled });
    }
  }, [disabled]);

  // Update Monaco theme when editorTheme prop changes (e.g., user toggles dark mode)
  useEffect(() => {
    if (monacoRef.current) {
      monacoRef.current.editor.setTheme(editorTheme === 'dark' ? 'kyomi-md-dark' : 'kyomi-md-light');
    }
  }, [editorTheme]);

  // Track if change is from user typing vs external update
  const isTypingRef = useRef(false);
  const valueRef = useRef(value);

  // Update editor value manually only for external changes (not from typing)
  useEffect(() => {
    if (!editorRef.current || !monacoRef.current) return;

    const currentValue = editorRef.current.getValue();

    // Only update if value changed AND we're not currently typing
    if (value !== currentValue && !isTypingRef.current) {
      const position = editorRef.current.getPosition();
      editorRef.current.setValue(value || '');
      if (position) {
        editorRef.current.setPosition(position);
      }
    }

    valueRef.current = value;
  }, [value]);

  // Schema validation with 3-second debounce
  useEffect(() => {
    if (!editorRef.current || !monacoRef.current || !schema) {
      return;
    }

    // Clear previous timer
    if (schemaValidationTimerRef.current) {
      clearTimeout(schemaValidationTimerRef.current);
    }


    // Debounce validation by 3 seconds
    schemaValidationTimerRef.current = setTimeout(async () => {

      const editor = editorRef.current;
      const monaco = monacoRef.current;
      const model = editor.getModel();
      if (!model) {
        return;
      }

      const content = model.getValue();

      // Call async backend validation (with client-side fallback)
      const markers = await validateChartmlInMarkdown(content, schema, monaco, { skipBigQuery });

      if (markers.length > 0) {
      }

      // Set markers on the model
      monaco.editor.setModelMarkers(model, 'chartmlSchema', markers);
    }, 3000);

    return () => {
      if (schemaValidationTimerRef.current) {
        clearTimeout(schemaValidationTimerRef.current);
      }
    };
  }, [value, schema]);


  // Show loading indicator until schema is loaded
  if (!schema) {
    return (
      <div className="relative h-full w-full flex items-center justify-center">
        <div className="text-muted-foreground">Loading editor...</div>
      </div>
    );
  }

  return (
    <div className="relative h-full w-full">
      {/* Placeholder overlay - shown when editor is empty */}
      {!value && placeholder && (
        <div
          className="absolute text-muted-foreground whitespace-pre-wrap"
          style={{
            fontSize: `${fontSize}px`,
            fontFamily: 'Menlo, Monaco, "Courier New", monospace',
            lineHeight: '19px',
            top: '-3px',
            left: '30px',
            right: '0',
            pointerEvents: 'none',
            zIndex: 1,
            padding: '4px 10px 4px 0'
          }}
        >
          {placeholder}
        </div>
      )}

      <Editor
        height="100%"
        language="markdown"
        defaultValue={value}
        onChange={(newValue) => {
          isTypingRef.current = true;
          if (onChange) onChange(newValue);
          // Reset typing flag after a short delay
          setTimeout(() => {
            isTypingRef.current = false;
          }, 100);
        }}
        beforeMount={handleEditorWillMount}
        onMount={handleEditorDidMount}
        theme={editorTheme === 'dark' ? 'kyomi-md-dark' : 'kyomi-md-light'}
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
            vertical: 'visible',
            horizontal: 'visible',
            verticalScrollbarSize: 10,
            horizontalScrollbarSize: 10
          },

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

          // Word wrap - important for markdown
          wordWrap: 'on',
          wordWrapColumn: 80,
          wrappingIndent: 'indent',

          // Suggestions / autocomplete - disabled in markdown, manual trigger only
          quickSuggestions: false,  // Disable automatic suggestions in markdown
          suggestOnTriggerCharacters: true,  // But enable trigger characters (:, space, etc)
          acceptSuggestionOnEnter: 'on',
          tabCompletion: 'on',

          // Other features
          folding: false,
          glyphMargin: false,
          lineDecorationsWidth: 5,
          lineNumbersMinChars: 3,

          // Performance
          automaticLayout: true,

          // Scrolling
          scrollBeyondLastLine: false
        }}
      />
    </div>
  );
};

export default MonacoMarkdownEditor;
