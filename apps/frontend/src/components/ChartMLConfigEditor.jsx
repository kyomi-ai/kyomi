// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useRef, useEffect, useCallback } from 'react';
import Editor, { loader } from '@monaco-editor/react';
import * as monaco from 'monaco-editor';
import * as yaml from 'js-yaml';
import { getChartmlSchema } from '../schemas/schemaService';
import {
  registerChartmlLanguage,
  registerChartmlCompletionProvider,
  validateChartmlDocument
} from '../lib/chartmlLanguage';
import { useMonacoEditor } from '../hooks/useMonacoEditor';

// Configure loader to use npm package instead of CDN
loader.config({ monaco });

/**
 * ChartML Configuration Editor
 * Simple Monaco editor for ChartML config/source blocks
 * Uses exact same setup as ChartBuilderModal
 */
export default function ChartMLConfigEditor({
  value,
  onChange,
  onValidationChange,
  placeholder = "# Enter ChartML configuration\ntype: config\nversion: 1\nstyle: autumn_forest",
  height = "400px",
  readOnly = false
}) {
  const [yamlText, setYamlText] = useState(value || placeholder);
  const [yamlError, setYamlError] = useState(null);
  const { handleEditorWillMount, handleEditorDidMount, editorRef, monacoRef } = useMonacoEditor();
  const [baseSchema, setBaseSchema] = useState(null);
  const validationTimerRef = useRef(null);

  // Fetch base schema from backend on mount
  useEffect(() => {
    async function fetchSchema() {
      try {
        const schema = await getChartmlSchema();
        setBaseSchema(schema);
      } catch (error) {
      }
    }
    fetchSchema();
  }, []);

  // Register chartml language when schema is loaded (EXACT SAME AS ChartBuilderModal)
  useEffect(() => {
    if (!monacoRef.current || !baseSchema || !editorRef.current) return;


    try {
      // Register chartml as a Monaco language (idempotent - skips if already registered globally)
      registerChartmlLanguage(monacoRef.current, baseSchema);

      // Register completion provider (idempotent - skips if already registered globally)
      registerChartmlCompletionProvider(monacoRef.current, baseSchema);

    } catch (error) {
    }
  }, [baseSchema, editorRef.current]);

  // Schema validation with 3-second debounce (EXACT SAME AS ChartBuilderModal)
  useEffect(() => {
    if (!monacoRef.current || !baseSchema || !editorRef.current) return;

    // Clear previous timer
    if (validationTimerRef.current) {
      clearTimeout(validationTimerRef.current);
    }

    // Debounce validation by 3 seconds
    validationTimerRef.current = setTimeout(async () => {
      const editor = editorRef.current;
      const monaco = monacoRef.current;
      const model = editor.getModel();
      if (!model) return;

      const content = model.getValue();

      try {
        // Call async backend validation
        const markers = await validateChartmlDocument(content, baseSchema, monaco);

        // Set markers on the model
        monaco.editor.setModelMarkers(model, 'chartmlSchema', markers);


        // Extract errors for parent component
        const errors = markers.filter(m => m.severity === monaco.MarkerSeverity.Error);
        const warnings = markers.filter(m => m.severity === monaco.MarkerSeverity.Warning);

        setYamlError(errors.length > 0 ? errors[0].message : null);

        if (onValidationChange) {
          onValidationChange({
            valid: errors.length === 0,
            errors: errors.map(e => e.message),
            warnings: warnings.map(w => w.message)
          });
        }
      } catch (error) {
        setYamlError(error.message);
        if (onValidationChange) {
          onValidationChange({ valid: false, errors: [error.message], warnings: [] });
        }
      }
    }, 3000);

    return () => {
      if (validationTimerRef.current) {
        clearTimeout(validationTimerRef.current);
      }
    };
  }, [yamlText, baseSchema, onValidationChange]);


  // Handle text change
  const handleChange = useCallback((newValue) => {
    setYamlText(newValue || '');
    if (onChange) {
      onChange(newValue || '');
    }
  }, [onChange]);

  // Update editor when external value changes
  useEffect(() => {
    if (!editorRef.current) return;

    const currentValue = editorRef.current.getValue();
    if (value !== currentValue && value !== yamlText) {
      setYamlText(value || '');
      editorRef.current.setValue(value || '');
    }
  }, [value]);

  return (
    <div className="border border-input rounded-lg overflow-hidden">
      <Editor
        key="chartml-config-editor"
        height={height}
        defaultLanguage="chartml"
        path="chartml-config.yaml"
        defaultValue={yamlText}
        onChange={handleChange}
        beforeMount={handleEditorWillMount}
        onMount={handleEditorDidMount}
        theme="vs-light"
        keepCurrentModel={true}
        options={{
          minimap: { enabled: false },
          fontSize: 13,
          lineNumbers: 'on',
          scrollBeyondLastLine: false,
          automaticLayout: true,
          tabSize: 2,
          wordWrap: 'on',
          readOnly: readOnly,
          padding: { top: 12, bottom: 12 }
        }}
      />

      {/* Validation Error Display */}
      {yamlError && (
        <div className="px-4 py-3 bg-error border-t border-error-border text-sm text-error-foreground">
          <span className="font-medium">Validation Error:</span> {yamlError}
        </div>
      )}
    </div>
  );
}
