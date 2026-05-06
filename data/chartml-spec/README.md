# ChartML Specification - MASTER REFERENCE

⚠️ **THIS IS THE SINGLE SOURCE OF TRUTH FOR CHARTML** ⚠️

This directory contains the **authoritative ChartML v1.0 specification**. All implementations MUST read from these files. **DO NOT create copies anywhere in this repository.**

---

## 🎯 Master Files

### 1. **SPECIFICATION.md**
The complete ChartML language specification including:
- Component types (Source, Params, Chart)
- Data → Transform → Visualize pipeline
- All supported properties and their behavior
- Chart types and visualization options
- Parameter system and variable references
- Best practices and usage guidelines

**Location:** `docs/chartml-spec/SPECIFICATION.md`

### 2. **chartml_schema.json**
JSON Schema definition for validation and autocomplete:
- Backend validation of ChartML blocks
- Monaco Editor autocomplete and IntelliSense
- Frontend ChartML editor validation
- Automated testing

**Location:** `docs/chartml-spec/chartml_schema.json`

### 3. **EXAMPLES.md**
Real-world ChartML examples demonstrating all features:
- Complete dashboard configurations
- All chart types (bar, line, area, pie, table, metric, scatter)
- Common patterns and best practices
- Parameter usage and filtering
- Edge cases and advanced usage

**Location:** `docs/chartml-spec/EXAMPLES.md`

### 4. **tests/**
Automated validation tests:
- Validates all examples against JSON schema
- Ensures spec consistency
- Runs in CI/CD pipeline

**Location:** `docs/chartml-spec/tests/`

---

## ⚠️ CRITICAL: No Copies Policy

**NEVER create copies of these files anywhere in the codebase.**

### ❌ DO NOT:
- Copy `chartml_schema.json` to frontend or backend directories
- Duplicate ChartML examples in code
- Embed spec snippets in comments or documentation
- Create "simplified" or "subset" versions of the spec
- Store ChartML syntax in agent prompts or hardcoded strings

### ✅ INSTEAD:
- Backend: Read `chartml_schema.json` from this directory at runtime
- Frontend: Request schema from backend API endpoint
- Agent prompts: Reference this directory and let agents read files
- Documentation: Link to these master files
- Examples: Import from `EXAMPLES.md` programmatically

**Why?** Copies create version drift. When the spec changes, copies become outdated and cause bugs. One source of truth keeps everything synchronized.

---

## 🔄 Synchronization Requirements

**These three files MUST always be in sync:**

1. **Add new chart type** → Update SPECIFICATION.md, chartml_schema.json, EXAMPLES.md
2. **Change property behavior** → Update all three files
3. **Add transform capabilities** → Update all three files
4. **Change parameter types** → Update all three files

After any changes:
```bash
cd docs/chartml-spec/tests
npm test  # Validates all examples against schema
```

---

## 📚 ChartML v1.0 Quick Reference

### Component Types
```yaml
type: source      # Reusable data source
type: params      # Interactive dashboard parameters
type: chart       # Visualization
```

### Data Pipeline
```
Data → Transform → Visualize
```

### Basic Chart Structure
```yaml
type: chart
version: 1
title: "My Chart"

params:           # Optional - chart-level parameters
  - id: region
    type: multiselect
    label: "Region"
    options: ["US", "EU", "APAC"]
    default: ["US"]

data: source_name # Reference named Source (string)
# OR
data:             # Inline Source definition (object)
  provider: inline
  rows:
    - month: "Jan"
      revenue: 1200

transform:        # Optional - pipeline: sql → aggregate → forecast
  aggregate:
    dimensions: [month]
    measures:
      - column: revenue
        aggregation: sum
        name: total_revenue
    filters:
      rules:
        - field: region
          operator: in
          value: "$params.region"

visualize:        # Required - chart rendering
  type: bar
  columns: month
  rows: total_revenue
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
```

---

## 🛠️ Usage Guidelines

### For Developers
When implementing ChartML features:
1. **Read** `SPECIFICATION.md` to understand the full language
2. **Reference** `chartml_schema.json` for exact property definitions
3. **Study** `EXAMPLES.md` for practical usage patterns
4. **Test** against examples to ensure compatibility
5. **Never** create copies - always read from this directory

### For AI Assistants (Claude, etc.)
Before generating any ChartML code:
1. **ALWAYS** read `docs/chartml-spec/SPECIFICATION.md` first
2. **VERIFY** syntax against `docs/chartml-spec/chartml_schema.json`
3. **FOLLOW** patterns from `docs/chartml-spec/EXAMPLES.md`
4. **NEVER** guess ChartML syntax
5. **NEVER** use outdated terminology (dataset/extract/transform/parameters)

### Current Terminology (v1.0)
- ✅ `type: source` (not dataset)
- ✅ `provider:` (not source within Source)
- ✅ `data:` (not extract)
- ✅ `rows:` (not data for inline)
- ✅ `transform:` (pipeline with stages: `sql`, `aggregate`, `forecast`)
- ✅ `type: params` (not parameters)
- ✅ `params:` field (not parameters:)
- ✅ `$params.*` references

---

## 🧪 Validation Tests

All examples are automatically validated against the schema:

```bash
cd docs/chartml-spec/tests
npm install  # First time only
npm test     # Run validation tests
```

**Test coverage:**
- 42 ChartML examples
- All chart types
- All parameter types
- All transform/aggregation features
- Inline and referenced data sources

See `tests/README.md` for details.

---

## 🔗 Related Code (DO NOT copy spec here!)

These files implement ChartML but should **read** from this directory:

### Frontend
- `apps/frontend/src/lib/chartmlPipeline.js` - Pipeline execution
- `apps/frontend/src/components/ChartGridv2.jsx` - Chart rendering
- `apps/frontend/src/utils/chartParser.js` - YAML parsing
- Monaco editor configuration - Should fetch schema from backend

### Backend
- Agent prompts - Should reference this directory
- Validation endpoints - Should read `chartml_schema.json` from here
- API documentation - Should link to these files

### Tests
- `apps/frontend/src/lib/chartml_pipeline.test.js` - Pipeline tests
- `docs/chartml-spec/tests/` - Spec validation tests

---

## ❓ Questions?

**Before asking questions:**
1. Read `SPECIFICATION.md` for complete language reference
2. Check `EXAMPLES.md` for practical examples
3. Review `chartml_schema.json` for exact property definitions

**Location:** `docs/chartml-spec/`

**Remember:** This is the ONLY place where ChartML is defined. If you're implementing ChartML, you're reading from the right place. If you're making copies... STOP! 🛑
