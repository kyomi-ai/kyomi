# ChartML JSON Schema Files

## Files

- **`chartml_schema.json`** - Human-readable schema with formatting (MASTER)
- **`chartml_schema.min.json`** - Minified schema for LLM prompts (AUTO-GENERATED)

## Editing the Schema

1. **Only edit `chartml_schema.json`** (the formatted version)
2. **Regenerate minified version** after changes:

```bash
cd kyomi
python3 -c "
import json
with open('docs/chartml-spec/chartml_schema.json', 'r') as f:
    schema = json.load(f)
with open('docs/chartml-spec/chartml_schema.min.json', 'w') as f:
    json.dump(schema, f, separators=(',', ':'))
print('✅ Minified schema regenerated')
"
```

## Why Minified?

The minified version is used by:
- Chart Copilot API (sent in system prompts to LLM)
- Any AI features that need the schema

**Token savings**: 31KB → 17KB (45% reduction) in JSON, even more when converted to YAML.

Since every space and newline counts as tokens, this saves ~$$ on every chart copilot request.

## Backend Usage

The backend (`chartml_validation.py`) loads `chartml_schema.min.json` and serves it via:
- `GET /api/v1/chartml/schema` - Returns minified JSON
- Frontend converts to YAML and sends to chart copilot

The schema is cached with `cache_control: ephemeral` on the backend, but the first request in each cache window (5 min) benefits from the smaller size.
