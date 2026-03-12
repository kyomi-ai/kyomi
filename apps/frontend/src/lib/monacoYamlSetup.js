// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Monaco YAML Setup - Configure monaco-yaml with JSON Schema
 */
import { setDiagnosticsOptions } from 'monaco-yaml';
import { getChartmlSchema } from '../schemas/schemaService';

/**
 * Configure monaco-yaml with ChartML schema
 * Must be called before Monaco editor is mounted
 */
export async function setupMonacoYaml() {
  const chartmlSchema = await getChartmlSchema();

  setDiagnosticsOptions({
    enableSchemaRequest: true,
    hover: true,
    completion: true,
    validate: true,
    format: true,
    schemas: [
      {
        uri: 'http://kyomi.ai/chartml-schema.json',
        fileMatch: ['*'],
        schema: chartmlSchema
      }
    ]
  });
}
