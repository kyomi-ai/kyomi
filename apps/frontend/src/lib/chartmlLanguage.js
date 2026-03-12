// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ChartML Monaco Language Configuration
 *
 * SINGLE SOURCE OF TRUTH: All configuration is derived from the JSON Schema.
 * This file contains NO hardcoded spec details - everything is extracted dynamically.
 *
 * Registers chartml as a first-class Monaco language with:
 * - Schema-driven syntax highlighting
 * - Schema-driven autocomplete
 * - Schema-driven validation (backend-first with client-side fallback)
 * - Embedded language support in markdown
 */

import * as yaml from 'js-yaml';
import apiClient from '../api/apiClient';

/**
 * Extract all property names recursively from a JSON Schema object
 * This builds the keyword list for syntax highlighting
 */
function extractPropertyNames(schema, names = new Set()) {
  if (!schema || typeof schema !== 'object') return names;

  // Process definitions section (ChartML v1.0 structure)
  if (schema.definitions) {
    Object.values(schema.definitions).forEach(def => {
      extractPropertyNames(def, names);
    });
  }

  if (schema.properties) {
    Object.keys(schema.properties).forEach(key => {
      names.add(key);
      extractPropertyNames(schema.properties[key], names);
    });
  }

  if (schema.items) {
    extractPropertyNames(schema.items, names);
  }

  if (schema.oneOf) {
    schema.oneOf.forEach(subSchema => extractPropertyNames(subSchema, names));
  }

  if (schema.anyOf) {
    schema.anyOf.forEach(subSchema => extractPropertyNames(subSchema, names));
  }

  return names;
}

/**
 * Extract all enum values from a JSON Schema
 * This builds the list of valid constant values for syntax highlighting
 */
function extractEnumValues(schema, values = new Set()) {
  if (!schema || typeof schema !== 'object') return values;

  // Process definitions section (ChartML v1.0 structure)
  if (schema.definitions) {
    Object.values(schema.definitions).forEach(def => {
      extractEnumValues(def, values);
    });
  }

  if (schema.enum) {
    schema.enum.forEach(val => values.add(String(val)));
  }

  // Handle const values (used in ChartML v1.0 for type discrimination)
  if (schema.const !== undefined) {
    values.add(String(schema.const));
  }

  if (schema.properties) {
    Object.values(schema.properties).forEach(prop => {
      extractEnumValues(prop, values);
    });
  }

  if (schema.items) {
    extractEnumValues(schema.items, values);
  }

  if (schema.oneOf) {
    schema.oneOf.forEach(subSchema => extractEnumValues(subSchema, values));
  }

  if (schema.anyOf) {
    schema.anyOf.forEach(subSchema => extractEnumValues(subSchema, values));
  }

  return values;
}

/**
 * Escape special regex characters in a string
 */
function escapeRegex(str) {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Register chartml language with Monaco
 * @param {monaco} monaco - Monaco editor instance
 * @param {Object} schema - ChartML JSON Schema
 */
export function registerChartmlLanguage(monaco, schema) {
  // Check if already registered - language registration is global to Monaco and should only happen once
  const languages = monaco.languages.getLanguages();
  const isRegistered = languages.some(lang => lang.id === 'chartml');

  if (isRegistered) {
    return;
  }


  // Extract keywords and enum values from schema
  const keywords = Array.from(extractPropertyNames(schema));
  const enumValues = Array.from(extractEnumValues(schema));

  // Escape special regex characters and build pattern for keywords (property names)
  // Match at start of line (with optional whitespace and optional YAML list indicator) followed by colon
  const escapedKeywords = keywords.map(escapeRegex);
  const keywordPattern = escapedKeywords.length > 0
    ? `^\\s*(?:-\\s+)?(${escapedKeywords.join('|')})(?=\\s*:)`
    : '^\\s*$'; // No keywords found - match nothing

  // Escape and build pattern for enum values
  // Match complete words using word boundaries (not part of longer strings)
  const escapedEnums = enumValues.map(escapeRegex);
  const enumPattern = escapedEnums.length > 0
    ? `(${escapedEnums.join('|')})`
    : '^$'; // No enums found - match nothing

  // Register the language
  monaco.languages.register({ id: 'chartml' });

  // Define syntax highlighting rules using Monarch tokenizer
  // IMPORTANT: Order matters! First match wins. Check strings/comments BEFORE keywords.
  monaco.languages.setMonarchTokensProvider('chartml', {
    tokenizer: {
      root: [
        // Comments FIRST - so keywords in comments aren't highlighted
        [/#.*$/, 'comment'],

        // Strings SECOND - so keywords in strings aren't highlighted
        [/"([^"\\]|\\.)*$/, 'string.invalid'],
        [/'([^'\\]|\\.)*$/, 'string.invalid'],
        [/"/, 'string', '@doubleQuotedString'],
        [/'/, 'string', '@singleQuotedString'],

        // Property names THIRD (extracted from schema) - handles both regular and list items
        // This must come BEFORE the standalone list indicator rule
        [new RegExp(keywordPattern), 'keyword'],

        // After colon - enter value state to check for enums or treat as plain value
        [/:\s*/, 'delimiter.key-value', '@valueStart'],

        // YAML list indicator (for list items that aren't property names)
        // This comes AFTER keyword pattern, so `- name:` is handled above
        [/^\s*-\s/, 'delimiter'],

        // Brackets and operators
        [/[{}()\[\]]/, '@brackets'],
        [/,/, 'delimiter'],

        // Invalid trailing spaces
        [/\s+$/, 'invalid'],
      ],

      // State for handling values after colon
      valueStart: [
        // CRITICAL: If we see a new line starting with a property name (word followed by colon),
        // it means the previous line had no value. Pop back to root and let root handle it.
        [new RegExp(keywordPattern), { token: '@rematch', next: '@pop' }],

        // Empty value - catch immediate end of line
        [/$/, { token: '', next: '@pop' }],

        // Check if it's a quoted string
        [/\s*"/, { token: 'string', next: '@doubleQuotedString' }],
        [/\s*'/, { token: 'string', next: '@singleQuotedString' }],

        // Skip any leading whitespace (but not newlines)
        [/[ \t]+/, ''],

        // Check if it's a number
        [/-?\d+(\.\d+)?(?=\s|$)/, { token: 'number', next: '@pop' }],

        // Check if it's an enum value (schema-defined, includes booleans)
        [new RegExp(enumPattern + '(?=\\s|$)'), { token: 'type', next: '@pop' }],

        // Otherwise, treat rest of line as unquoted value (SQL queries, plain text, list items)
        [/.+/, { token: 'value', next: '@pop' }]
      ],

      doubleQuotedString: [
        [/[^\\"]+/, 'string'],
        [/\\./, 'string.escape'],
        [/"/, 'string', '@pop']
      ],

      singleQuotedString: [
        [/[^\\']+/, 'string'],
        [/\\./, 'string.escape'],
        [/'/, 'string', '@pop']
      ]
    }
  });

  // Define light theme for chartml
  monaco.editor.defineTheme('chartml-theme', {
    base: 'vs',
    inherit: true,
    rules: [
      { token: 'keyword', foreground: '0000FF', fontStyle: 'bold' },
      { token: 'type', foreground: '267f99', fontStyle: 'bold' },
      { token: 'string', foreground: 'A31515' },
      { token: 'value', foreground: '000000' },  // Unquoted YAML values (SQL, URLs, plain text) - plain black
      { token: 'number', foreground: '098658' },
      { token: 'comment', foreground: '008000', fontStyle: 'italic' },
      { token: 'delimiter.key-value', foreground: '000000' },
      { token: 'invalid', background: 'FF0000', foreground: 'FFFFFF' }
    ],
    colors: {}
  });

  // Define dark theme for chartml
  monaco.editor.defineTheme('chartml-dark', {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'keyword', foreground: '569cd6', fontStyle: 'bold' },
      { token: 'type', foreground: '4ec9b0', fontStyle: 'bold' },
      { token: 'string', foreground: 'ce9178' },
      { token: 'value', foreground: 'f1f5f9' },
      { token: 'number', foreground: 'b5cea8' },
      { token: 'comment', foreground: '6a9955', fontStyle: 'italic' },
      { token: 'delimiter.key-value', foreground: 'f1f5f9' },
      { token: 'invalid', background: 'FF0000', foreground: 'FFFFFF' }
    ],
    colors: {
      'editor.background': '#262626',
      'editor.foreground': '#f1f5f9',
      'editor.lineHighlightBackground': '#383838',
      'editor.selectionBackground': '#3b5998',
    }
  });

  // Configure language features
  monaco.languages.setLanguageConfiguration('chartml', {
    comments: {
      lineComment: '#'
    },
    brackets: [
      ['{', '}'],
      ['[', ']'],
      ['(', ')']
    ],
    autoClosingPairs: [
      { open: '{', close: '}' },
      { open: '[', close: ']' },
      { open: '(', close: ')' },
      { open: '"', close: '"' },
      { open: "'", close: "'" }
    ],
    surroundingPairs: [
      { open: '{', close: '}' },
      { open: '[', close: ']' },
      { open: '(', close: ')' },
      { open: '"', close: '"' },
      { open: "'", close: "'" }
    ],
    indentationRules: {
      increaseIndentPattern: /^.*[:{]\s*$/,
      decreaseIndentPattern: /^\s*[}\]]/
    }
  });

}

/**
 * Get the primary component definition from schema
 * For schemas with definitions, finds the most complete definition (most properties)
 * For flat schemas, returns the schema itself
 * @param {Object} schema - JSON Schema
 * @returns {Object} Component schema definition
 */
function getChartSchema(schema) {
  // Flat schema: properties defined at root level (valid JSON Schema format)
  if (schema.properties && !schema.definitions) {
    return schema;
  }

  // Schema with definitions: find the definition with the most properties
  // This is likely the main component type (Chart, Source, etc.)
  if (schema.definitions) {
    let largestDef = null;
    let maxProps = 0;

    for (const [name, definition] of Object.entries(schema.definitions)) {
      if (definition.properties) {
        const propCount = Object.keys(definition.properties).length;
        if (propCount > maxProps) {
          maxProps = propCount;
          largestDef = { name, definition };
        }
      }
    }

    if (largestDef) {
      return largestDef.definition;
    }
  }

  // Could not find suitable definition
  return { properties: {} };
}

/**
 * Build completions for a specific schema level
 * @param {Object} schema - Schema object
 * @param {monaco} monaco - Monaco instance
 * @param {string} prefix - Property path prefix
 * @returns {Array} Completion suggestions
 */
function buildCompletionsFromSchema(schema, monaco, prefix = '') {
  const suggestions = [];

  if (!schema || !schema.properties) return suggestions;

  Object.entries(schema.properties).forEach(([key, prop]) => {
    let insertText = `${key}: `;
    let kind = monaco.languages.CompletionItemKind.Property;

    // Add value hints based on schema type
    // Only provide specific values if defined in schema (no hardcoded defaults)
    if (prop.enum && prop.enum.length > 0) {
      // Use first enum value as default
      insertText = `${key}: ${prop.enum[0]}`;
      kind = monaco.languages.CompletionItemKind.Value;
    } else if (prop.const !== undefined) {
      // Const value specified in schema
      insertText = `${key}: ${prop.const}`;
      kind = monaco.languages.CompletionItemKind.Value;
    } else if (prop.type === 'string') {
      insertText = `${key}: "\${1:value}"`;
    } else if (prop.type === 'number') {
      insertText = `${key}: \${1:0}`;
    } else if (prop.type === 'array') {
      insertText = `${key}:\n  - `;
    } else if (prop.type === 'object') {
      insertText = `${key}:\n  `;
    }
    // Note: For booleans without enum/const, just insert "key: " with no value hint
    // We don't hardcode true/false - if schema wants those, it should define them as enum

    suggestions.push({
      label: key,
      kind: kind,
      insertText: insertText,
      insertTextRules: monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
      documentation: prop.description
        ? {
            value: `**${key}** *(${prop.type || 'property'})*\n\n${prop.description}`
          }
        : `Property: ${key}`,
      detail: prop.type || 'property',
      sortText: `0_${key}` // Sort properties first
    });

    // Add enum values as separate completions
    if (prop.enum && prop.enum.length > 1) {
      prop.enum.forEach((enumVal, idx) => {
        suggestions.push({
          label: `${key}: ${enumVal}`,
          kind: monaco.languages.CompletionItemKind.Value,
          insertText: `${key}: ${enumVal}`,
          documentation: {
            value: prop.description
              ? `**${key}: ${enumVal}**\n\n${prop.description}`
              : `Set **${key}** to **${enumVal}**`
          },
          detail: 'enum value',
          sortText: `1_${key}_${idx}` // Sort enum values after properties
        });
      });
    }
  });

  return suggestions;
}

/**
 * Register JSON Schema-based autocomplete provider
 * @param {monaco} monaco - Monaco editor instance
 * @param {Object} schema - JSON Schema for ChartML
 */
/**
 * Core completion logic - can be called directly or through Monaco provider
 */
export function provideChartmlCompletions(monaco, schema, model, position) {

      // Determine the component type from the document
      const fullContent = model.getValue();
      const typeMatch = fullContent.match(/^\s*type:\s*(\w+)/m);
      const componentType = typeMatch ? typeMatch[1] : 'chart';


      // Get the schema for this component type
      let chartSchema = schema;
      if (schema.definitions) {
        // Map component types to their definition names (capitalize first letter)
        const defName = componentType.charAt(0).toUpperCase() + componentType.slice(1);

        if (schema.definitions[defName]) {
          chartSchema = schema.definitions[defName];
        } else {
          // Fallback to finding Chart or largest definition
          chartSchema = getChartSchema(schema);
        }
      }

      // Get current line text
      const textUntilPosition = model.getValueInRange({
        startLineNumber: position.lineNumber,
        startColumn: 1,
        endLineNumber: position.lineNumber,
        endColumn: position.column
      });


      // Calculate indent level (number of leading spaces)
      const leadingWhitespace = textUntilPosition.match(/^\s*/)[0];
      const indentLevel = leadingWhitespace.length;
      const trimmedText = textUntilPosition.trim();

      // Check if we're completing a VALUE (after colon + optional space)
      const colonMatch = trimmedText.match(/^(\S+):\s*(.*)$/);


      if (colonMatch) {
        // We're completing a value after "propertyName: "
        const propertyName = colonMatch[1];
        const partialValue = colonMatch[2];


        // Special case: root-level "type" should show all possible component types
        // Extract from oneOf definitions in the schema
        if (indentLevel === 0 && propertyName === 'type' && schema.oneOf && schema.definitions) {
          const typeValues = new Set();

          // Extract all type const values from definitions referenced in oneOf
          schema.oneOf.forEach(option => {
            if (option.$ref) {
              // Extract definition name from $ref like "#/definitions/Chart"
              const defName = option.$ref.split('/').pop();
              const definition = schema.definitions[defName];

              if (definition) {
                // Handle oneOf within the definition (like Source has multiple variants)
                if (definition.oneOf) {
                  definition.oneOf.forEach(variant => {
                    if (variant.properties?.type?.const !== undefined) {
                      typeValues.add(variant.properties.type.const);
                    }
                  });
                }
                // Handle direct definition
                else if (definition.properties?.type?.const !== undefined) {
                  typeValues.add(definition.properties.type.const);
                }
              }
            }
          });

          if (typeValues.size > 0) {
            const suggestions = Array.from(typeValues).map(value => ({
              label: String(value),
              kind: monaco.languages.CompletionItemKind.EnumMember,
              insertText: String(value),
              documentation: {
                value: `**${value}**\n\nComponent type discriminator`
              },
              detail: 'Component type'
            }));
            return { suggestions };
          }
        }

        // Parse content for context (fullContent already declared at top)
        const lines = fullContent.split('\n');
        const currentLineIndex = position.lineNumber - 1;

        // Build context path by tracking indent hierarchy from current position backwards
        const contextPath = [];
        let currentIndent = indentLevel;

        for (let i = currentLineIndex - 1; i >= 0; i--) {
          const line = lines[i];
          if (!line.trim()) continue;

          const lineIndent = line.match(/^(\s*)/)[1].length;

          // If we've gone back to a parent level (less indent)
          if (lineIndent < currentIndent) {
            // Check if this is a property (has colon)
            const match = line.match(/^\s*([a-zA-Z_][a-zA-Z0-9_]*):/);
            if (match) {
              const propName = match[1];
              contextPath.unshift(propName); // Add to front of path
              currentIndent = lineIndent;

              // Stop if we've reached root level (indent 0)
              if (lineIndent === 0) break;
            }
          }
        }


        // Navigate to the schema at this context path
        let inSection = contextPath[0] || null;

        // Helper to resolve $ref
        const resolveRef = (ref) => {
          if (!ref || !ref.startsWith('#/definitions/')) return null;
          const defName = ref.replace('#/definitions/', '');
          return schema.definitions?.[defName];
        };

        // Helper to get schema from oneOf by merging enum/const values
        const resolveOneOf = (oneOfSchema) => {
          const merged = { properties: {} };

          oneOfSchema.forEach(variant => {
            if (variant.properties) {
              Object.keys(variant.properties).forEach(propName => {
                const propSchema = variant.properties[propName];

                // Handle const values - convert to enum when merging multiple
                if (propSchema.const !== undefined) {
                  if (!merged.properties[propName]) {
                    // First const value - convert to enum array
                    merged.properties[propName] = {
                      enum: [propSchema.const],
                      description: propSchema.description
                    };
                  } else if (merged.properties[propName].enum) {
                    // Add to existing enum array
                    merged.properties[propName].enum.push(propSchema.const);
                  }
                }
                // Handle enum fields - merge all possible values
                else if (propSchema.enum) {
                  if (merged.properties[propName]?.enum) {
                    merged.properties[propName].enum = [
                      ...new Set([...merged.properties[propName].enum, ...propSchema.enum])
                    ];
                  } else {
                    merged.properties[propName] = propSchema;
                  }
                }
                // Other property types
                else if (!merged.properties[propName]) {
                  merged.properties[propName] = propSchema;
                }
              });
            }
          });

          return merged;
        };

        // Navigate through the context path to find the right schema
        let currentSchema = chartSchema;


        for (let i = 0; i < contextPath.length; i++) {
          const pathPart = contextPath[i];

          if (!currentSchema.properties || !currentSchema.properties[pathPart]) {
            break;
          }

          let nextSchema = currentSchema.properties[pathPart];

          // Resolve $ref
          if (nextSchema.$ref) {
            nextSchema = resolveRef(nextSchema.$ref) || nextSchema;
          }

          // Handle oneOf
          if (nextSchema.oneOf) {
            nextSchema = resolveOneOf(nextSchema.oneOf);
          }

          // Handle arrays - descend into items
          if (nextSchema.type === 'array' && nextSchema.items) {
            nextSchema = nextSchema.items;

            // Resolve $ref in items
            if (nextSchema.$ref) {
              nextSchema = resolveRef(nextSchema.$ref) || nextSchema;
            }

            // Handle oneOf in items
            if (nextSchema.oneOf) {
              nextSchema = resolveOneOf(nextSchema.oneOf);
            }
          }

          currentSchema = nextSchema;
        }

        // Now look for the property in the current schema
        let propertySchema = null;

        if (currentSchema.properties && currentSchema.properties[propertyName]) {
          propertySchema = currentSchema.properties[propertyName];
        }

        // If property has enum values, suggest those
        if (propertySchema?.enum) {
          const suggestions = propertySchema.enum.map(value => ({
            label: String(value),
            kind: monaco.languages.CompletionItemKind.EnumMember,
            insertText: String(value),
            documentation: {
              value: propertySchema.description
                ? `**${value}**\n\n${propertySchema.description}`
                : `Enum value for **${propertyName}**`
            },
            detail: propertySchema.description || `${propertyName} value`
          }));
          return { suggestions };
        }

        // If property has a const value, suggest that
        if (propertySchema?.const !== undefined) {
          const suggestions = [{
            label: String(propertySchema.const),
            kind: monaco.languages.CompletionItemKind.Constant,
            insertText: String(propertySchema.const),
            documentation: {
              value: propertySchema.description
                ? `**${propertySchema.const}**\n\n${propertySchema.description}`
                : `Constant value for **${propertyName}**`
            },
            detail: propertySchema.description || `${propertyName} constant value`
          }];
          return { suggestions };
        }

        // No specific suggestions for this value
        return { suggestions: [] };
      }

      // We're completing a property name
      // Root level properties
      if (indentLevel === 0) {
        const suggestions = buildCompletionsFromSchema(chartSchema, monaco);
        return { suggestions };
      }

      // Parse content for context (fullContent already declared at top)
      const lines = fullContent.split('\n');
      const currentLineIndex = position.lineNumber - 1;

      // Look backwards to find which section we're in (allow leading whitespace)
      // Extract section names dynamically from schema (no hardcoding!)
      const sectionNames = chartSchema.properties ? Object.keys(chartSchema.properties) : [];

      let inSection = null;
      for (let i = currentLineIndex; i >= 0; i--) {
        const line = lines[i];
        // Check against all top-level properties from schema
        for (const sectionName of sectionNames) {
          const regex = new RegExp(`^\\s*${sectionName}:`);
          if (regex.test(line)) {
            inSection = sectionName;
            break;
          }
        }
        if (inSection) break;
      }


      if (inSection && indentLevel === 2) {
        const suggestions = buildCompletionsFromSchema(chartSchema.properties[inSection], monaco, `${inSection}.`);
        return { suggestions };
      }

      return { suggestions: [] };
}

// Track if completion provider is already registered (global state)
let chartmlCompletionProviderRegistered = false;

export function registerChartmlCompletionProvider(monaco, schema) {
  if (chartmlCompletionProviderRegistered) {
    return null;
  }

  const provider = monaco.languages.registerCompletionItemProvider('chartml', {
    triggerCharacters: [' ', ':', '\n', '-'],
    provideCompletionItems: (model, position) => {
      return provideChartmlCompletions(monaco, schema, model, position);
    }
  });

  chartmlCompletionProviderRegistered = true;
  return provider;
}

/**
 * Call backend validation API and convert response to Monaco markers
 * @param {string} chartmlYaml - ChartML YAML content
 * @param {monaco} monaco - Monaco editor instance
 * @returns {Promise<Array>} Promise resolving to array of Monaco markers, or null if backend unavailable
 */
async function callBackendValidation(chartmlYaml, monaco) {

  const response = await apiClient.post('/api/v1/chartml/validate', {
    chartml: chartmlYaml
  });

  if (response.data.valid) {
    return [];
  }

  // Convert backend errors to Monaco markers
  const markers = response.data.errors.map(error => ({
    severity: 8, // monaco.MarkerSeverity.Error
    startLineNumber: error.line,
    startColumn: error.column,
    endLineNumber: error.line,
    endColumn: error.column + 10, // Highlight ~10 characters
    message: error.message
  }));

  return markers;
}

/**
 * Validate a standalone chartml document (not in markdown)
 * Uses backend validation - throws if backend unavailable
 * @param {string} chartmlYaml - ChartML YAML content
 * @param {Object} schema - ChartML JSON Schema (unused - kept for API compatibility)
 * @param {monaco} monaco - Monaco editor instance
 * @returns {Promise<Array>} Promise resolving to array of Monaco editor markers for errors
 */
export async function validateChartmlDocument(chartmlYaml, schema, monaco) {
  // Call backend validation - will throw if unavailable
  return await callBackendValidation(chartmlYaml, monaco);
}

/**
 * Register validation provider for chartml
 * @param {monaco} monaco - Monaco editor instance
 * @param {Object} schema - JSON Schema for ChartML
 */
export function registerChartmlValidation(monaco, schema) {
  /**
   * Helper to find line/column for a property name in the model
   */
  function findPropertyPosition(model, propertyPath) {
    const content = model.getValue();
    const lines = content.split('\n');

    // For nested properties, just search for the last part
    const propertyName = propertyPath.split('.').pop();
    const searchPattern = new RegExp(`^\\s*(?:-\\s+)?${propertyName}\\s*:`);

    for (let i = 0; i < lines.length; i++) {
      if (searchPattern.test(lines[i])) {
        const line = lines[i];
        const match = line.match(new RegExp(`(${propertyName})`));
        if (match) {
          return {
            lineNumber: i + 1,
            startColumn: match.index + 1,
            endColumn: match.index + propertyName.length + 1
          };
        }
      }
    }

    return { lineNumber: 1, startColumn: 1, endColumn: 1 };
  }

  /**
   * Validate properties recursively against schema
   */
  function validateProperties(obj, schemaObj, path, model, markers) {
    if (!obj || typeof obj !== 'object' || Array.isArray(obj)) return;
    if (!schemaObj || !schemaObj.properties) return;

    const allowedProps = Object.keys(schemaObj.properties);
    const additionalAllowed = schemaObj.additionalProperties !== false;

    Object.keys(obj).forEach(key => {
      if (!allowedProps.includes(key) && !additionalAllowed) {
        // Invalid property name
        const fullPath = path ? `${path}.${key}` : key;
        const pos = findPropertyPosition(model, fullPath);

        markers.push({
          severity: monaco.MarkerSeverity.Error,
          startLineNumber: pos.lineNumber,
          startColumn: pos.startColumn,
          endLineNumber: pos.lineNumber,
          endColumn: pos.endColumn,
          message: `Unknown property "${key}". Valid properties: ${allowedProps.join(', ')}`
        });
      } else if (allowedProps.includes(key)) {
        // Recursively validate nested objects
        const propSchema = schemaObj.properties[key];
        if (propSchema.type === 'object' && typeof obj[key] === 'object') {
          validateProperties(obj[key], propSchema, `${path ? path + '.' : ''}${key}`, model, markers);
        }
      }
    });
  }

  // Validate on content change
  function validateChartml(model) {
    const markers = [];

    try {
      const content = model.getValue();
      const parsed = yaml.load(content);

      if (!parsed || typeof parsed !== 'object') {
        markers.push({
          severity: monaco.MarkerSeverity.Error,
          startLineNumber: 1,
          startColumn: 1,
          endLineNumber: 1,
          endColumn: 1,
          message: 'ChartML must be a valid YAML object'
        });
        monaco.editor.setModelMarkers(model, 'chartml', markers);
        return;
      }

      // Validate all properties recursively
      validateProperties(parsed, schema, '', model, markers);

      // Check required fields from schema (generic, schema-driven)
      if (schema.required) {
        schema.required.forEach(requiredField => {
          if (!parsed[requiredField]) {
            markers.push({
              severity: monaco.MarkerSeverity.Error,
              startLineNumber: 1,
              startColumn: 1,
              endLineNumber: 1,
              endColumn: 1,
              message: `Required field "${requiredField}" is missing`
            });
          }
        });
      }

      // Note: Detailed enum validation is handled by validateProperties() recursively
      // No hardcoded field paths here - everything is schema-driven

    } catch (error) {
      // YAML parsing error
      markers.push({
        severity: monaco.MarkerSeverity.Error,
        startLineNumber: error.mark?.line || 1,
        startColumn: error.mark?.column || 1,
        endLineNumber: error.mark?.line || 1,
        endColumn: (error.mark?.column || 1) + 10,
        message: `YAML Error: ${error.message}`
      });
    }

    monaco.editor.setModelMarkers(model, 'chartml', markers);
  }

  return validateChartml;
}

/**
 * Extend markdown language to recognize ```chartml blocks
 * This enables embedded chartml support in markdown editors
 * @param {monaco} monaco - Monaco editor instance
 */
export function extendMarkdownForChartml(monaco) {
  // Monaco's markdown already supports embedded languages in code blocks automatically
  // When a language with ID 'chartml' is registered, markdown will:
  // 1. Recognize ```chartml code blocks
  // 2. Apply chartml syntax highlighting inside those blocks
  // 3. Keep normal markdown highlighting everywhere else
  // 4. Enable autocomplete when cursor is inside a chartml block

  // No additional configuration needed - it just works!
}

/**
 * Validate chartml blocks within markdown content
 * Uses backend validation for each block - throws if backend unavailable
 * @param {string} markdown - Markdown content with chartml blocks
 * @param {Object} schema - ChartML JSON Schema (unused - kept for API compatibility)
 * @param {monaco} monaco - Monaco editor instance
 * @param {Object} options - Validation options
 * @param {boolean} options.skipBigQuery - Skip BigQuery dry runs (faster validation for editor)
 * @returns {Promise<Array>} Promise resolving to array of Monaco editor markers for errors
 */
export async function validateChartmlInMarkdown(markdown, schema, monaco, options = {}) {
  // Call the markdown-aware validation endpoint
  // This endpoint validates all blocks with scope-aware parameter checking
  const response = await apiClient.post('/api/v1/chartml/validate-markdown', {
    markdown: markdown,
    skip_bigquery: options.skipBigQuery || false
  });

  if (response.data.valid) {
    return [];
  }

  // Convert backend errors to Monaco markers
  const markers = response.data.errors.map(error => ({
    severity: 8, // monaco.MarkerSeverity.Error
    startLineNumber: error.line,
    startColumn: error.column,
    endLineNumber: error.line,
    endColumn: error.column + 10,
    message: error.message
  }));

  return markers;
}

