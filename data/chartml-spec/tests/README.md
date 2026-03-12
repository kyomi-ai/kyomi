# ChartML Specification Tests

This directory contains automated tests that validate the ChartML specification for consistency and correctness.

## Purpose

These tests ensure that:
1. All examples in `EXAMPLES.md` are syntactically valid YAML
2. All examples conform to the JSON schema (`chartml_schema.json`)
3. The three master documents stay synchronized
4. No spec changes break existing examples

## Documents Being Tested

This test suite validates:
- **[`SPECIFICATION.md`](../SPECIFICATION.md)** - Language specification (human-readable)
- **[`chartml_schema.json`](../chartml_schema.json)** - JSON Schema (machine-readable validation)
- **[`EXAMPLES.md`](../EXAMPLES.md)** - All 42 ChartML examples

See [`README.md`](../README.md) for an overview of the specification directory.

---

## Test Files

### `validate-examples.test.js`

Validates all ChartML examples against the JSON schema.

**What it does:**
1. Extracts all ` ```chartml ` code blocks from `EXAMPLES.md`
2. Parses each block as YAML
3. Validates each parsed component against `chartml_schema.json` using Ajv
4. Reports any validation errors with line numbers

**Coverage:**
- 42 ChartML examples
- All component types: Source, Params, Chart
- All chart types: bar, line, area, pie, doughnut, scatter, table, metric
- All parameter types: multiselect, select, daterange, number, text
- Inline and referenced data sources
- Aggregation features: dimensions, measures, filters, sort, limit

---

## Running Tests

```bash
# Install dependencies (first time only)
cd docs/chartml-spec/tests
npm install

# Run all tests
npm test

# Run tests in watch mode (auto-rerun on file changes)
npm run test:watch

# Run tests with verbose output
npm run test:verbose
```

---

## Expected Output

When all tests pass:
```
Found 42 ChartML examples in EXAMPLES.md

PASS ./validate-examples.test.js
  ChartML Examples Validation
    ✓ EXAMPLES.md file exists
    ✓ chartml_schema.json file exists
    ✓ Schema is valid JSON
    ✓ Schema has oneOf with all component types
    ✓ All examples were validated
    Validate each example
      ✓ Example #1 (line 19) is valid ChartML
      ✓ Example #2 (line 106) is valid ChartML
      ...
      ✓ Example #42 (line 2198) is valid ChartML

✅ Successfully validated 42 ChartML examples

Test Suites: 1 passed, 1 total
Tests:       47 passed, 47 total
```

---

## When Tests Fail

If a test fails, it means the specification is inconsistent:

**Schema validation failed:**
- Example doesn't match schema definition
- Either fix the example or update the schema
- Ensure all three master documents stay in sync

**YAML parsing failed:**
- Syntax error in example
- Fix the YAML in `EXAMPLES.md`

**After fixing:**
1. Re-run `npm test`
2. Ensure all 47 tests pass
3. Commit all three master documents together

---

## CI Integration

These tests **should** be run in CI to ensure:
- Spec changes don't break examples
- Schema changes are reflected in examples
- Examples stay synchronized with the specification
- No one commits broken ChartML to the repository

**TODO:** Add to GitHub Actions workflow.

---

## Adding New Tests

To add new validation tests:

1. Create a new `.test.js` file in this directory
2. Import the schema: `const schema = require('../chartml_schema.json')`
3. Use Ajv for validation:
   ```javascript
   const Ajv = require('ajv');
   const ajv = new Ajv({ allErrors: true, verbose: true });
   const validate = ajv.compile(schema);
   const valid = validate(yourChartMLObject);
   ```
4. Run `npm test` to execute all tests

---

## Dependencies

See `package.json`:
- **jest** - Test framework
- **ajv** - JSON Schema validator
- **yaml** - YAML parser

---

## Questions?

Before modifying tests:
1. Read [`SPECIFICATION.md`](../SPECIFICATION.md) to understand ChartML
2. Review [`chartml_schema.json`](../chartml_schema.json) for schema structure
3. Check [`EXAMPLES.md`](../EXAMPLES.md) for example patterns
