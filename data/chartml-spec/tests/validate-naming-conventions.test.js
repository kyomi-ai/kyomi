/**
 * Test: Validate naming conventions across ChartML specification
 *
 * Enforces that all ChartML keywords use camelCase to differentiate
 * from user data fields (which typically use snake_case from databases).
 *
 * ChartML keywords = camelCase
 * User data fields = snake_case (user's choice, often from database)
 */

const fs = require('fs');
const path = require('path');

// Paths to spec files
const SPEC_DIR = path.join(__dirname, '..');
const SPEC_PATH = path.join(SPEC_DIR, 'SPECIFICATION.md');
const SCHEMA_PATH = path.join(SPEC_DIR, 'chartml_schema.json');
const EXAMPLES_PATH = path.join(SPEC_DIR, 'EXAMPLES.md');

// Snake_case patterns that should NOT appear in ChartML keywords
const FORBIDDEN_SNAKE_CASE_KEYWORDS = [
  // Old aggregation functions (should be camelCase now)
  'count_distinct',
  'percentile_25',
  'percentile_50',
  'percentile_75',
  'percentile_90',
  'percentile_95',
  'percentile_99',

  // Old operators (should be camelCase now)
  'not_in',
  'is_null',
  'is_not_null',
  'starts_with',
  'ends_with'
];

// camelCase versions that SHOULD appear
const REQUIRED_CAMELCASE_KEYWORDS = {
  aggregation_functions: [
    'countDistinct',
    'percentile25',
    'percentile50',
    'percentile75',
    'percentile90',
    'percentile95',
    'percentile99'
  ],
  operators: [
    'notIn',
    'isNull',
    'isNotNull',
    'startsWith',
    'endsWith'
  ]
};

/**
 * Check if a file contains forbidden snake_case keywords
 */
function checkForForbiddenSnakeCase(filePath, content, description) {
  const errors = [];

  FORBIDDEN_SNAKE_CASE_KEYWORDS.forEach(keyword => {
    // Look for the keyword in contexts where it would be a ChartML keyword
    // (not a user data field name)
    const patterns = [
      new RegExp(`"${keyword}"`, 'g'),  // In JSON schema enum
      new RegExp(`\`${keyword}\``, 'g'),  // In markdown code
      new RegExp(`aggregation:\\s*${keyword}`, 'g'),  // aggregation: count_distinct
      new RegExp(`operator:\\s*"${keyword}"`, 'g')  // operator: "not_in"
    ];

    patterns.forEach(pattern => {
      const matches = content.match(pattern);
      if (matches) {
        errors.push({
          file: path.basename(filePath),
          keyword,
          count: matches.length,
          description
        });
      }
    });
  });

  return errors;
}

/**
 * Check if schema contains required camelCase keywords
 */
function checkSchemaContainsCamelCase(schema) {
  const errors = [];

  // Check aggregation functions
  const aggregationEnum = schema.definitions.Aggregate.properties.measures.items.properties.aggregation.enum;
  REQUIRED_CAMELCASE_KEYWORDS.aggregation_functions.forEach(func => {
    if (!aggregationEnum.includes(func)) {
      errors.push({
        type: 'missing_camelCase',
        location: 'schema aggregation enum',
        keyword: func
      });
    }
  });

  // Check operators (now in oneOf structure due to isNull/isNotNull having no value)
  const filterRulesOneOf = schema.definitions.Aggregate.properties.filters.properties.rules.items.oneOf;
  const nullCheckOps = filterRulesOneOf[0].properties.operator.enum;  // isNull, isNotNull
  const valueOps = filterRulesOneOf[1].properties.operator.enum;      // all other operators
  const allOperators = [...nullCheckOps, ...valueOps];

  REQUIRED_CAMELCASE_KEYWORDS.operators.forEach(op => {
    if (!allOperators.includes(op)) {
      errors.push({
        type: 'missing_camelCase',
        location: 'schema operator enum',
        keyword: op
      });
    }
  });

  return errors;
}

/**
 * Check if SPECIFICATION.md documents required camelCase keywords
 */
function checkSpecificationDocumentsCamelCase(content) {
  const errors = [];

  // Check that aggregation functions are documented
  REQUIRED_CAMELCASE_KEYWORDS.aggregation_functions.forEach(func => {
    if (!content.includes(`\`${func}\``)) {
      errors.push({
        type: 'missing_documentation',
        location: 'SPECIFICATION.md',
        keyword: func,
        message: 'Aggregation function not documented in backticks'
      });
    }
  });

  // Check that operators are documented
  REQUIRED_CAMELCASE_KEYWORDS.operators.forEach(op => {
    if (!content.includes(`\`${op}\``)) {
      errors.push({
        type: 'missing_documentation',
        location: 'SPECIFICATION.md',
        keyword: op,
        message: 'Operator not documented in backticks'
      });
    }
  });

  return errors;
}

describe('ChartML Naming Conventions', () => {
  let specContent, schemaContent, schema, examplesContent;

  beforeAll(() => {
    specContent = fs.readFileSync(SPEC_PATH, 'utf8');
    schemaContent = fs.readFileSync(SCHEMA_PATH, 'utf8');
    schema = JSON.parse(schemaContent);
    examplesContent = fs.readFileSync(EXAMPLES_PATH, 'utf8');
  });

  describe('Forbidden snake_case keywords', () => {
    test('SPECIFICATION.md should not contain snake_case ChartML keywords', () => {
      const errors = checkForForbiddenSnakeCase(SPEC_PATH, specContent, 'SPECIFICATION.md');

      if (errors.length > 0) {
        const errorMsg = errors.map(e =>
          `  - Found "${e.keyword}" ${e.count} time(s) in ${e.file}`
        ).join('\n');

        fail(`Snake_case ChartML keywords found (should be camelCase):\n${errorMsg}`);
      }

      expect(errors).toHaveLength(0);
    });

    test('chartml_schema.json should not contain snake_case ChartML keywords', () => {
      const errors = checkForForbiddenSnakeCase(SCHEMA_PATH, schemaContent, 'chartml_schema.json');

      if (errors.length > 0) {
        const errorMsg = errors.map(e =>
          `  - Found "${e.keyword}" ${e.count} time(s) in ${e.file}`
        ).join('\n');

        fail(`Snake_case ChartML keywords found (should be camelCase):\n${errorMsg}`);
      }

      expect(errors).toHaveLength(0);
    });

    test('EXAMPLES.md should not contain snake_case ChartML keywords', () => {
      const errors = checkForForbiddenSnakeCase(EXAMPLES_PATH, examplesContent, 'EXAMPLES.md');

      if (errors.length > 0) {
        const errorMsg = errors.map(e =>
          `  - Found "${e.keyword}" ${e.count} time(s) in ${e.file}`
        ).join('\n');

        fail(`Snake_case ChartML keywords found (should be camelCase):\n${errorMsg}`);
      }

      expect(errors).toHaveLength(0);
    });
  });

  describe('Required camelCase keywords', () => {
    test('Schema should define all camelCase aggregation functions', () => {
      const errors = checkSchemaContainsCamelCase(schema);
      const aggregationErrors = errors.filter(e => e.location === 'schema aggregation enum');

      if (aggregationErrors.length > 0) {
        const errorMsg = aggregationErrors.map(e =>
          `  - Missing "${e.keyword}" in aggregation enum`
        ).join('\n');

        fail(`Missing camelCase aggregation functions:\n${errorMsg}`);
      }

      expect(aggregationErrors).toHaveLength(0);
    });

    test('Schema should define all camelCase operators', () => {
      const errors = checkSchemaContainsCamelCase(schema);
      const operatorErrors = errors.filter(e => e.location === 'schema operator enum');

      if (operatorErrors.length > 0) {
        const errorMsg = operatorErrors.map(e =>
          `  - Missing "${e.keyword}" in operator enum`
        ).join('\n');

        fail(`Missing camelCase operators:\n${errorMsg}`);
      }

      expect(operatorErrors).toHaveLength(0);
    });

    test('SPECIFICATION.md should document all camelCase keywords', () => {
      const errors = checkSpecificationDocumentsCamelCase(specContent);

      if (errors.length > 0) {
        const errorMsg = errors.map(e =>
          `  - ${e.keyword}: ${e.message}`
        ).join('\n');

        fail(`Missing camelCase keyword documentation:\n${errorMsg}`);
      }

      expect(errors).toHaveLength(0);
    });
  });

  test('Summary: All ChartML keywords use camelCase', () => {
    console.log('\n✅ Naming Convention Summary:');
    console.log('   - ChartML keywords: camelCase (countDistinct, notIn, startsWith, etc.)');
    console.log('   - User data fields: snake_case (sale_date, total_revenue, etc.)');
    console.log('   - Clear differentiation maintained across all specification documents\n');
  });
});
