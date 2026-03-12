# ChartML v1.0 Specification - Issues Found

**Analysis Date:** 2025-10-25
**Analysis Type:** Comprehensive cross-reference deep dive
**Total Issues Found:** 25 (4 Critical, 8 Major, 13 Minor)

---

## 🔴 CRITICAL ISSUES (Must Fix Before v1.0)

### Issue #1: Missing `style` Property in Chart Schema
**Status:** 🔴 CRITICAL - PENDING FIX
**Location:** `chartml_schema.json` line 81-119 (Chart definition)
**Problem:** The Chart schema does NOT include a `style` property at the chart level, but SPECIFICATION.md line 296 explicitly documents it.

**Current State:**
```yaml
# SPECIFICATION.md says this is valid:
type: chart
version: 1
style:        # ← NOT in schema!
  height: 400
```

Schema Chart properties (lines 85-118): `type`, `version`, `title`, `params`, `layout`, `data`, `aggregate`, `visualize` - NO `style`

**Fix Required:**
Add `"style": { "$ref": "#/definitions/Style" }` to Chart.properties

**Why It Matters:**
Charts cannot have top-level height or other styling without this. The spec says they can, but validation will fail.

---

### Issue #2: Filter `value` Required for `isNull`/`isNotNull`
**Status:** 🔴 CRITICAL - PENDING FIX
**Location:** Schema line 321, SPECIFICATION.md line 382-383
**Problem:** Schema requires `value` for ALL filter operators, but `isNull` and `isNotNull` should NOT have values.

**Current State:**
```json
// Schema line 321 - WRONG for isNull/isNotNull
"required": ["field", "operator", "value"]
```

**Fix Required:**
Make `value` conditionally required - not required when operator is `isNull` or `isNotNull`. Need to use oneOf pattern to handle this.

**Why It Matters:**
Filters like `{field: "x", operator: "isNull", value: ???}` are nonsensical. Users will be forced to provide dummy values.

---

### Issue #3: Missing Enum Constraint on `dimension.type`
**Status:** 🔴 CRITICAL - PENDING FIX
**Location:** Schema line 246-248, SPECIFICATION.md line 343
**Problem:** Dimension object type is documented as enum `["string", "number", "date"]` in spec but schema doesn't specify this constraint.

**Current State:**
```json
// Schema line 248 - allows ANY string
"type": {
  "type": "string",
  "description": "Data type for casting"
}
```

**Fix Required:**
```json
"type": {
  "type": "string",
  "enum": ["string", "number", "date"],
  "description": "Data type for casting"
}
```

**Why It Matters:**
Users could write `type: "boolean"` and it would pass validation but fail at runtime.

---

### Issue #4: Title Placement Confusion
**Status:** 🔴 CRITICAL - PENDING FIX
**Location:** SPECIFICATION.md lines 96, 262, 296, 437, 556, 693
**Problem:** SPECIFICATION is inconsistent about where `title` belongs. Examples show:
- `chart.title` (most common)
- `visualize.style.title` (some examples)
- No documentation about precedence

**Current State:**
Users can specify title in THREE places with unclear precedence.

**Fix Required:**
1. Standardize on **`chart.title` ONLY**
2. Remove `visualize.style.title` from schema entirely
3. Update all examples to use `chart.title`
4. Document in SPEC that title is chart-level metadata, not visual styling

**Reasoning:**
- Title is semantic metadata (what the chart is), not visual styling (how it looks)
- 90% of examples already use `chart.title`
- No deprecation needed - we haven't released yet
- Clean, simple rule: one place for title

**Why It Matters:**
Users won't know where to put title. Different examples show different patterns. This creates confusion and inconsistent dashboards.

---

### Issue #5: Metric Chart Label Confusion
**Status:** 🔴 CRITICAL - PENDING FIX
**Location:** EXAMPLES.md lines 102-170, SPECIFICATION.md lines 495-504
**Problem:** Metric charts have 3 different ways to show labels, with confusing rules:
- Option 1: `visualize.label` → inside card
- Option 2: `chart.title` + `style.showLabel: false` → above card
- Option 3: Automatic field name → inside card

Plus `visualize.style.showLabel` toggle makes it even more confusing.

**Fix Required:**
Simplify to TWO clear locations:
1. **`chart.title`** (optional) → Shows **above** card (consistent with all charts)
2. **`visualize.label`** (optional) → Shows **inside** card (metric-specific)
3. **Remove `visualize.style.showLabel`** → No toggle needed
4. **No automatic field name display** → Explicit is better

**Examples After Fix:**
```yaml
# Just the number (no labels)
visualize:
  type: metric
  value: revenue
  format: "$,.0f"

# Label inside only
visualize:
  type: metric
  value: revenue
  label: "Total Revenue"
  format: "$,.0f"

# Title above + label inside
title: "Q1 Performance"
visualize:
  type: metric
  value: revenue
  label: "Revenue"
  format: "$,.0f"
```

**Why It Matters:**
Too many options creates confusion. Need clear, simple rules for metric card labeling.

---

## 🟡 MAJOR ISSUES (Should Fix Before v1.0)

### Issue #6: `marks.text` Data Type Mismatch
**Status:** 🟡 MAJOR
**Location:** Schema line 577-589, SPECIFICATION.md line 423
**Problem:** SPECIFICATION says `marks.text: field_name` (string), but schema defines it as object with `{field, format}`.

**Fix Required:**
Schema should allow BOTH string and object (like marks.color does):
```json
"text": {
  "oneOf": [
    { "type": "string" },
    {
      "type": "object",
      "properties": {
        "field": { "type": "string" },
        "format": { "type": "string" }
      },
      "required": ["field"]
    }
  ]
}
```

---

### Issue #7: Missing `marks.shape` Documentation
**Status:** 🟡 MAJOR
**Location:** Schema lines 563-575, SPECIFICATION.md line 408
**Problem:** Schema defines `marks.shape` but SPECIFICATION.md never documents it.

**Fix Required:**
Add section to SPECIFICATION.md about shape encoding for scatter plots.

---

### Issue #8: Parameter Layout Grid Wrapping Behavior Unclear
**Status:** 🟡 MAJOR
**Location:** SPECIFICATION.md lines 146-154
**Problem:** Auto-calculated column span rules say "4+ parameters: 3 columns each" but 5 params × 3 = 15 columns. Does it wrap? Stack? Shrink?

**Fix Required:**
Document wrapping behavior clearly.

---

### Issue #9: Metric Chart `label` Property Missing from SPECIFICATION
**Status:** 🟡 MAJOR (will be fixed with Issue #5)
**Location:** Schema lines 410-412, SPECIFICATION.md lines 495-504
**Problem:** Schema defines `label` property for metric charts, SPEC doesn't document it.

**Note:** This will be resolved when we fix Issue #5.

---

### Issue #10: Annotations Not Documented in Main Visualize Section
**Status:** 🟡 MAJOR
**Location:** SPECIFICATION.md line 401-438, Schema lines 429-435
**Problem:** Annotations are in the schema and examples but NOT in the main Visualize Layer section of SPEC.

**Fix Required:**
Add annotations documentation to the main Visualize Layer section in SPECIFICATION.md.

---

### Issue #11: Missing Cache TTL Format Documentation
**Status:** 🟡 MAJOR
**Location:** SPECIFICATION.md line 78, Schema line 50
**Problem:** Cache ttl format examples shown but exact format not specified.

**Fix Required:**
Document exact format:
```markdown
### Cache Time-to-Live Format

The `ttl` field accepts duration strings:
- Format: `<number><unit>`
- Units: `s` (seconds), `m` (minutes), `h` (hours), `d` (days)
- Examples: `"30s"`, `"5m"`, `"6h"`, `"1d"`, `"7d"`
```

---

### Issue #12: Provider-Specific Required Fields Not Enforced
**Status:** 🟡 MAJOR
**Location:** Schema lines 34-44, SPECIFICATION.md lines 75-76
**Problem:** Schema says `query` is required for bigquery, `rows` for inline, `endpoint` for api, but schema doesn't actually enforce this.

**Fix Required:**
Use conditional schema validation with oneOf pattern to enforce provider-specific required fields.

---

### Issue #13: Missing Format String Reference Documentation
**Status:** 🟡 MAJOR
**Location:** Multiple locations using `format:` property
**Problem:** Format strings like `"$,.0f"`, `".1%"`, `"~s"` are used extensively but never documented.

**Fix Required:**
Add format reference section to SPECIFICATION.md with d3-format link.

---

## 🟢 MINOR ISSUES (Can Defer to v1.1)

### Issue #14: Inconsistent Code Block Language in Examples
**Status:** 🟢 MINOR
**Location:** EXAMPLES.md lines 1971-2009, 2031-2090, 2173-2188
**Problem:** Some examples use ` ```yaml ` instead of ` ```chartml `.

**Fix:** Change all to ` ```chartml `.

---

### Issue #15: Missing Example for API Provider
**Status:** 🟢 MINOR
**Location:** SPECIFICATION.md lines 74-79, EXAMPLES.md
**Problem:** API provider is documented but has ZERO examples.

**Fix:** Add API source example to EXAMPLES.md.

---

### Issue #16: Table Pivot Syntax Confusing
**Status:** 🟢 MINOR
**Location:** EXAMPLES.md lines 1318-1375
**Problem:** Pivot table uses `rows` and `columns` differently than chart X/Y axes.

**Fix:** Add clarifying comment explaining table rows/columns are different from chart axes.

---

### Issue #17: Scatter Plot Documentation Minimal
**Status:** 🟢 MINOR
**Location:** SPECIFICATION.md line 413, EXAMPLES.md line 1093
**Problem:** Scatter is listed as chart type but has minimal documentation.

**Fix:** Add scatter plot section to SPECIFICATION.md explaining continuous X and Y requirements.

---

### Issue #18: Combinator Default Value Not Clear
**Status:** 🟢 MINOR
**Location:** Schema line 296, SPECIFICATION.md line 367
**Problem:** Schema says combinator defaults to 'and' but spec doesn't document it's optional.

**Fix:** Document that combinator is optional and defaults to "and".

---

### Issue #19: Empty Aggregate Object Behavior Not Documented
**Status:** 🟢 MINOR
**Location:** SPECIFICATION.md line 279, Schema line 222
**Problem:** What happens if `aggregate: {}` is specified with no properties?

**Fix:** Document that empty aggregate passes data through unchanged.

---

### Issue #20: Missing Reference to README.md
**Status:** 🟢 MINOR
**Location:** SPECIFICATION.md line 9, EXAMPLES.md line 8
**Problem:** Both docs reference `README.md` but we verified it exists (from earlier work).

**Note:** No issue - README.md exists.

---

### Issue #21: Visualization Type Order Differs
**Status:** 🟢 MINOR
**Location:** Schema line 358, SPECIFICATION.md line 287
**Problem:** Chart types listed in different order between schema and spec.

**Fix:** Match order for visual consistency (doesn't affect validation).

---

### Issue #22: `axes.x` Not Documented
**Status:** ✅ RESOLVED
**Location:** Schema lines 601-604, SPECIFICATION.md line 425-434
**Problem:** Schema defines `axes.x` but spec only shows `axes.left` and `axes.right`.

**Resolution:** Introduced semantic axis keys `axes.columns` (category axis) and `axes.rows` (measure axis) that automatically resolve to the correct positional axis based on chart orientation. Positional keys (`axes.x`, `axes.left`, `axes.right`) still work but are no longer recommended.

---

### Issue #23: Annotation Point Markers Not Supported
**Status:** 🟢 MINOR (Feature Request for Future)
**Location:** Schema line 632
**Problem:** Annotations only support line and band, no point markers.

**Note:** Consider for v1.1 - not blocking for v1.0.

---

### Issue #24: DataLabels `fontSize` No Min/Max Validation
**Status:** 🟢 MINOR
**Location:** Schema line 526-528
**Problem:** fontSize is number but no validation - could be negative or huge.

**Fix:** Add `"minimum": 1, "maximum": 72` to fontSize property.

---

### Issue #25: `visualize.style.title` Should Be Removed
**Status:** 🟢 MINOR (will be fixed with Issue #4)
**Location:** Schema, SPECIFICATION.md
**Problem:** Duplicate of chart.title - should remove to avoid confusion.

**Note:** This will be resolved when we fix Issue #4.

---

## Summary

**Critical Issues to Fix Now:** 5 issues (#1-5)
**Major Issues to Fix Before v1.0:** 8 issues (#6-13)
**Minor Issues for v1.1:** 12 issues (#14-25)

**Files Requiring Updates:**
- `chartml_schema.json`: 10+ changes needed
- `SPECIFICATION.md`: 12+ additions/clarifications needed
- `EXAMPLES.md`: 5+ improvements suggested

**Next Steps:**
1. Fix all 5 critical issues
2. Run validation tests
3. Decide on major issues (fix now vs defer)
4. Create GitHub issues for minor items to track for v1.1
