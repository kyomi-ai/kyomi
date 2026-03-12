/**
 * Test: Validate all ChartML examples against the JSON schema
 *
 * This test parses EXAMPLES.md, extracts all ```chartml blocks,
 * and validates each one against chartml_schema.json
 */

const fs = require('fs');
const path = require('path');
const yaml = require('yaml');
const Ajv = require('ajv');

// Paths to spec files
const SPEC_DIR = path.join(__dirname, '..');
const EXAMPLES_PATH = path.join(SPEC_DIR, 'EXAMPLES.md');
const SCHEMA_PATH = path.join(SPEC_DIR, 'chartml_schema.json');

/**
 * Extract all ```chartml code blocks from markdown
 */
function extractChartmlBlocks(markdown) {
  const blocks = [];
  const regex = /```chartml\n([\s\S]*?)\n```/g;
  let match;
  let index = 0;

  while ((match = regex.exec(markdown)) !== null) {
    blocks.push({
      index: index++,
      lineNumber: markdown.substring(0, match.index).split('\n').length,
      content: match[1]
    });
  }

  return blocks;
}

// Load data before describe blocks
const schemaContent = fs.readFileSync(SCHEMA_PATH, 'utf8');
const schema = JSON.parse(schemaContent);

const examplesContent = fs.readFileSync(EXAMPLES_PATH, 'utf8');
const examples = extractChartmlBlocks(examplesContent);

console.log(`\nFound ${examples.length} ChartML examples in EXAMPLES.md\n`);

describe('ChartML Examples Validation', () => {
  let ajv;

  beforeAll(() => {
    // Initialize Ajv validator
    ajv = new Ajv({
      allErrors: true,
      verbose: true,
      strict: false
    });
  });

  test('EXAMPLES.md file exists', () => {
    expect(fs.existsSync(EXAMPLES_PATH)).toBe(true);
  });

  test('chartml_schema.json file exists', () => {
    expect(fs.existsSync(SCHEMA_PATH)).toBe(true);
  });

  test('Schema is valid JSON', () => {
    expect(schema).toBeDefined();
    expect(schema.$schema).toBe('http://json-schema.org/draft-07/schema#');
  });

  test('Schema has oneOf with all component types', () => {
    expect(schema.oneOf).toBeDefined();
    expect(schema.oneOf).toHaveLength(6);
    expect(schema.definitions.Source).toBeDefined();
    expect(schema.definitions.Params).toBeDefined();
    expect(schema.definitions.StyleComponent).toBeDefined();
    expect(schema.definitions.Config).toBeDefined();
    expect(schema.definitions.Chart).toBeDefined();
    expect(schema.definitions.ComponentArray).toBeDefined();
  });

  describe('Validate each example', () => {
    examples.forEach((example) => {
      test(`Example #${example.index + 1} (line ${example.lineNumber}) is valid ChartML`, () => {
        let parsed;

        // Parse YAML
        try {
          parsed = yaml.parse(example.content);
        } catch (error) {
          throw new Error(`YAML parsing failed: ${error.message}\n\nContent:\n${example.content}`);
        }

        // Validate against schema
        const validate = ajv.compile(schema);
        const valid = validate(parsed);

        if (!valid) {
          const errors = validate.errors.map(err => {
            return `  - ${err.instancePath || 'root'}: ${err.message}${err.params ? ' (' + JSON.stringify(err.params) + ')' : ''}`;
          }).join('\n');

          throw new Error(
            `Schema validation failed for example #${example.index + 1} (line ${example.lineNumber}):\n\n` +
            `Errors:\n${errors}\n\n` +
            `Parsed data:\n${JSON.stringify(parsed, null, 2)}`
          );
        }

        expect(valid).toBe(true);
      });
    });
  });

  test('All examples were validated', () => {
    expect(examples.length).toBeGreaterThan(0);
    console.log(`\n✅ Successfully validated ${examples.length} ChartML examples\n`);
  });
});
