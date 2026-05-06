# ChartML v1.0 Migration Plan

**Date:** 2025-10-25
**Status:** Planning
**Goal:** Apply ChartML v1.0 specification changes to codebase

---

## Summary of Changes

ChartML v1.0 finalizes the specification with the following terminology:

| OLD (what code currently uses) | NEW (v1.0 spec) |
|--------------------------------|-----------------|
| `extract:` with nested `source:` | `data:` (string reference OR Source object) |
| `transform:` | `aggregate:` |
| Source object: `source: "bigquery"` | Source object: `provider: "bigquery"` |
| Inline: `data: [...]` | Inline: `rows: [...]` |
| `type: "dataset"` | `type: "source"` |
| `type: "parameters"` | `type: "params"` |
| Snake_case operators: `not_in`, `is_null` | camelCase: `notIn`, `isNull` |
| Snake_case aggregations: `count_distinct` | camelCase: `countDistinct` |

---

## Files Requiring Updates

### 1. Backend - Schema Loading ✅ DONE
- [x] `/apps/backend/src/api/routers/chartml_validation.py` - Load schema from master location

**Status:** Fixed - now loads from `docs/chartml-spec/chartml_schema.json`

---

### 2. Backend - Agent Prompts/Tools 🔴 CRITICAL

#### `/apps/backend/src/api/chat/bigquery_tools.py`
**Function:** `get_chartml_spec_tool()` (line 1460)
**Problem:** Hardcoded ChartML spec with OLD terminology
**Fix:** Read from master file `docs/chartml-spec/SPECIFICATION.md`

**Current (WRONG):**
```python
return """ChartML v2 Basic Structure:

```chartml
title: "Chart Title"

extract:
  source: bigquery
  query: |
    SELECT ...
```
"""
```

**Should be:**
```python
def get_chartml_spec_tool() -> str:
    """Read ChartML specification from master file"""
    spec_path = os.path.join(
        os.path.dirname(__file__),
        "..", "..", "..", "..", "..",
        "docs", "chartml-spec", "SPECIFICATION.md"
    )
    with open(spec_path, "r") as f:
        return f.read()
```

---

#### `/apps/backend/src/api/agent/bigquery_agent.py`
**Function:** `BIGQUERY_AGENT_SYSTEM_PROMPT` (line 26)
**Problem:** References "ChartML v2" and old terminology
**Fix:** Update prompt to reference v1.0 and correct terminology

---

### 3. Frontend - Schema File 🔴 CRITICAL

#### `/apps/frontend/src/schemas/chartmlSchema.js`
**Problem:** OUTDATED copy of schema with v0.x/v1.x old terminology
**Fix:** DELETE this file entirely - frontend already fetches from backend

**Evidence it's outdated:**
- Uses `type: "dataset"` (line 33) instead of `type: "source"`
- Uses `type: "parameters"` (line 92) instead of `type: "params"`
- Uses `extract:` (line 188) instead of `data:`
- Uses `transform:` (line 191) instead of `aggregate:`
- Uses snake_case operators: `not_in`, `is_null`, etc. (line 403)
- Uses snake_case aggregations: `count_distinct`, `percentile_25`, etc. (line 349)

**Frontend already has correct pattern:**
- `schemaService.js` fetches from backend `/api/v1/chartml/schema`
- `monacoYamlSetup.js` uses schemaService
- No direct imports of chartmlSchema.js found

**Action:** Safe to delete

---

### 4. Frontend - Pipeline/Renderer 🟡 MAJOR

#### `/apps/frontend/src/lib/chartmlPipeline.js`
**Problem:** Expects old terminology (`extract`, `transform`, `dataset`)
**Fix:** Add backward compatibility - accept BOTH old and new terminology

**Current signature:**
```javascript
const { extract, transform, visualize, ...otherProps } = chartmlSpec;
```

**Should support:**
```javascript
const {
  // Old terminology (backward compatibility)
  extract,
  transform,
  dataset,

  // New terminology (v1.0)
  data,
  aggregate,
  source,

  visualize,
  ...otherProps
} = chartmlSpec;

// Normalize to new terminology
const normalizedData = data || extract;
const normalizedAggregate = aggregate || transform;
```

**Changes needed:**
1. Accept both `data` and `extract` properties
2. Accept both `aggregate` and `transform` properties
3. Handle `provider:` instead of `source:` in Source objects
4. Handle `rows:` instead of `data:` for inline data
5. Handle `type: "source"` and `type: "dataset"` references
6. Handle `type: "params"` and `type: "parameters"` references

---

#### `/apps/frontend/src/lib/transformSQLBuilder.js`
**Assumption:** Likely references old terminology
**Action:** Review and update if needed

---

### 5. Frontend - Monaco Editor ✅ ALREADY CORRECT

The Monaco editor setup already fetches schema from backend:
- `schemaService.js` - Fetches from `/api/v1/chartml/schema`
- `monacoYamlSetup.js` - Uses schemaService
- `chartmlLanguage.js` - Receives schema as parameter

**No changes needed** - will automatically use new schema once backend serves it.

---

### 6. Other Files to Review 🟢 MINOR

These files reference old terminology but may not need immediate updates:

- `/apps/frontend/src/lib/markdownChartMLParser.js` - May need backward compatibility
- `/apps/frontend/src/components/ChartMLv2Wrapper.jsx` - May need backward compatibility
- `/apps/frontend/src/lib/datasetResolver.js` - May reference "dataset" terminology
- `/apps/frontend/src/components/ChartBuilderModal.jsx` - May generate old syntax
- Test files - Will need updates after main changes

---

## Migration Strategy

### Phase 1: Backend Updates 🔴
1. ✅ Update schema path in chartml_validation.py
2. Update get_chartml_spec_tool() to read from master file
3. Update agent system prompts to reference v1.0

### Phase 2: Frontend Schema Cleanup 🔴
1. Delete outdated chartmlSchema.js
2. Verify schemaService.js works correctly
3. Test Monaco editor autocomplete

### Phase 3: Pipeline Backward Compatibility 🟡
1. Update chartmlPipeline.js to accept both old and new terminology
2. Normalize old terminology to new internally
3. Test with both old and new ChartML blocks

### Phase 4: Testing 🟢
1. Run backend tests
2. Run frontend tests
3. Test ChartML editor in browser
4. Test agent ChartML generation
5. Test backward compatibility with old ChartML blocks

### Phase 5: Documentation 📝
1. Update any remaining docs referencing old terminology
2. Add migration guide for users with old ChartML
3. Update examples in codebase

---

## Backward Compatibility Strategy

**Goal:** Don't break existing dashboards with old ChartML syntax

**Approach:** Pipeline normalizes old terminology to new internally

```javascript
function normalizeChartmlSpec(spec) {
  return {
    // Component type normalization
    type: spec.type === 'dataset' ? 'source' :
          spec.type === 'parameters' ? 'params' :
          spec.type,

    // Data layer normalization
    data: spec.data || spec.extract,

    // Aggregate layer normalization
    aggregate: spec.aggregate || spec.transform,

    // Source object normalization
    provider: spec.provider || spec.source,

    // Inline data normalization
    rows: spec.rows || spec.data,

    // Preserve everything else
    ...spec
  };
}
```

---

## Testing Checklist

- [ ] Backend schema endpoint returns new v1.0 schema
- [ ] Agent uses new ChartML v1.0 examples
- [ ] Monaco editor autocomplete shows new properties
- [ ] Old ChartML blocks still render correctly (backward compat)
- [ ] New ChartML blocks render correctly
- [ ] ChartML validation uses new schema
- [ ] All tests pass

---

## Risk Assessment

**HIGH RISK:**
- Breaking existing dashboards if backward compatibility not implemented
- Agent generating invalid ChartML if spec not updated

**MEDIUM RISK:**
- Monaco editor showing wrong autocomplete (confuses users)
- Validation rejecting valid v1.0 ChartML

**LOW RISK:**
- Test failures (can be fixed)
- Documentation inconsistencies (can be updated)

---

## Next Steps

1. Get user approval for migration strategy
2. Implement Phase 1 (backend updates)
3. Implement Phase 2 (schema cleanup)
4. Implement Phase 3 (pipeline compatibility)
5. Test thoroughly
6. Deploy and monitor
