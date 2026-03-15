# Knowledge Files — Design Specification

## Problem

The current knowledge system (LearningsManager) presents knowledge as a database table — rows with fields for insight, context, learning_type, scope, etc. This feels foreign to users. They want to read and write knowledge the way they'd write documentation: as markdown files organized in folders.

At the same time, the AI agent needs to be a first-class participant — creating, updating, and organizing knowledge files through natural conversation, not just appending to a single "Agent Notes" file.

## Design Principles

1. **Markdown files are the source of truth.** Users and agents author knowledge as plain markdown documents organized in a folder tree. The database is a retrieval index, not the canonical store.
2. **Co-authored.** The agent can create, read, update, and organize any file in the tree — same as the user. Over time, the knowledge base builds itself through use.
3. **Documents are the retrieval unit.** Chunks are indexed for semantic search, but when a chunk matches, the full parent document is returned to the agent. Context is never fragmented.
4. **Simple to use.** No special syntax, no YAML frontmatter, no learning types or scope dropdowns. Just write markdown, organize in folders.

---

## User Experience

### Knowledge Page Layout

Modeled after Claude.ai's Projects Skills panel:

```
┌─────────────────────┬──────────────────────────────────────────┐
│  Knowledge    [+] 🔍│  Revenue / Metrics.md               [✏️] │
│                     │                                          │
│  📁 Revenue         │  # Metrics                               │
│     📄 Metrics      │                                          │
│     📄 Billing notes│  ## MRR (Monthly Recurring Revenue)      │
│  📁 Product         │                                          │
│     📄 Events       │  Sum of all active subscription amounts, │
│     📄 Funnels      │  from `billing.subscriptions` where      │
│  📁 Data quirks     │  `status = 'active'`. Excludes one-time  │
│     📄 BigQuery     │  charges.                                │
│     📄 Orders table │                                          │
│  📄 General notes   │  Related: billing.subscriptions,         │
│                     │  billing.invoices                        │
│                     │                                          │
│                     │  ## Churn Rate                            │
│                     │  ...                                      │
└─────────────────────┴──────────────────────────────────────────┘
```

**Left sidebar:**
- Collapsible folder tree
- `+` button to create file or folder
- Search/filter across all files
- Drag to reorder and reorganize
- Right-click context menu: rename, move, delete

**Right pane:**
- Markdown editor with rendered preview (toggle or live)
- Auto-saves on blur/debounce
- Shows last modified timestamp and author (user or agent)

### Authoring Conventions

No special syntax required. Users write normal markdown. However, the system recognizes optional patterns to extract richer metadata:

**Headings as chunk boundaries:**
```markdown
## MRR (Monthly Recurring Revenue)
Content here becomes one chunk, pointing back to this document.

## Churn Rate
Another chunk, same parent document.
```

**Table references (optional, improves retrieval precision):**
```markdown
Related: billing.subscriptions, billing.invoices
```
Or inline backticks: `` `billing.subscriptions` `` anywhere in text.

**SQL examples (optional, surfaced to agent as reference queries):**
~~~markdown
```sql
SELECT date_trunc('month', created_at), sum(amount) / 100.0
FROM billing.subscriptions
WHERE status = 'active'
GROUP BY 1
```
~~~

None of these are required. A file with nothing but plain prose still gets indexed and retrieved.

---

## Agent Interaction

### Knowledge Tree in System Prompt

The agent always has the full file tree available — injected into the system prompt at conversation start (and refreshed if files change mid-conversation). This gives the agent spatial orientation without needing a tool call.

```xml
<knowledge_tree>
📁 Revenue
   📄 Metrics.md
   📄 Billing notes.md
📁 Product
   📄 Event definitions.md
   📄 Funnel stages.md
📁 Data quirks
   📄 BigQuery.md
   📄 Orders table.md
📄 General notes.md
</knowledge_tree>
```

File names are descriptive enough. The tree is lightweight — just names and structure. The semantic index handles finding the right content.

The agent can then decide: "The user is asking about revenue — I see `Revenue/Metrics.md` in the tree, let me read that directly" instead of always going through vector search. **Two retrieval paths: navigate by structure, or search by semantics.** The agent picks whichever fits the question.

**No automatic content injection.** The tree structure is injected (file names and folders only — no document content). The agent decides what to read, when. No background retrieval running on every turn, no document content shoved in whether the agent needs it or not.

### Agent Tools

Replace `SaveLearningTool` with file-oriented tools:

#### `SearchKnowledge`
Semantic search over the knowledge base. Chunks match, but full parent documents are returned (see Retrieval section below).

**Parameters:**
- `query` (required): natural language search
- `limit` (default: 5): max documents to return

#### `ReadKnowledgeFile`
Read a specific file by path. The agent uses this when it already knows which file to look at (from the tree, from a previous conversation, or from search results).

**Parameters:**
- `path` (required): e.g. `Revenue/Metrics.md`

#### `ListKnowledgeFiles`
Browse the file tree. Returns folder structure with file names and summaries. Useful for exploring a subtree in detail or when the system prompt tree hasn't refreshed yet.

**Parameters:**
- `path` (optional): subtree to list, defaults to root
- `include_headings` (default: false): if true, also returns `##` headings within each file — gives the agent a table-of-contents view without reading the full file

#### `WriteKnowledgeFile`
Create a new file or overwrite an existing one. Primarily for creating new files. For updating existing files, prefer `EditKnowledgeFile`.

**Parameters:**
- `path` (required): e.g. `Data quirks/BigQuery.md`
- `content` (required): full markdown content
- `create_folders` (default: true): create parent folders if they don't exist
- `content_hash` (optional): hash from a prior `ReadKnowledgeFile` response. Required when overwriting an existing file — prevents clobbering concurrent user edits. Implemented as an atomic compare-and-swap: `UPDATE ... SET content = $new WHERE content_hash = $provided_hash`, check rows affected. If 0 rows, the file changed — reject the write.

#### `EditKnowledgeFile`
Targeted string replacement within a file. The agent sends only the old and new text — no need to read the full file into context.

**Parameters:**
- `path` (required): file to edit
- `old_text` (required): exact string to find in the current file content
- `new_text` (required): replacement string

**Concurrency safety:** If the user has edited the file since the agent last read it, the `old_text` simply won't match and the edit fails with an error. The agent can then re-read the file and retry. The string match itself is the concurrency guard — no locking or hashing needed.

### Agent Behavior Guidelines (System Prompt)

The agent's system prompt should instruct it to:

1. **Read before writing.** Always `ListKnowledgeFiles` and/or `ReadKnowledgeFile` before creating new files to avoid duplicates.
2. **Update over create.** If a relevant file exists, add a section rather than creating a new file.
3. **Organize sensibly.** Place files in existing folders where they fit. Create new folders only when a clear new domain emerges.
4. **Write for humans.** Knowledge files are read by users. Use clear language, not agent-internal jargon.
5. **Document discoveries naturally.** When the agent learns something useful during a conversation (a column is in cents, a table has a quirk, a metric has a specific formula), update the relevant knowledge file.

### Example: Knowledge Building Through Conversation

**Conversation 1:**
> User: "What's our MRR?"

Agent figures it out by querying the schema. Answers the question. Then:
- Calls `ListKnowledgeFiles` — tree is empty (new workspace)
- Calls `WriteKnowledgeFile` path=`Metrics.md` with MRR definition, formula, related tables

**Conversation 3:**
> User: "Show me churn by month"

Agent figures it out. Then:
- Calls `ReadKnowledgeFile` path=`Metrics.md` — sees MRR is there, ends with "Excludes one-time charges."
- Calls `EditKnowledgeFile` path=`Metrics.md` old_text=`"Excludes one-time charges."` new_text=`"Excludes one-time charges.\n\n## Churn Rate\n\nPercentage of customers who cancel..."` — appends new section

**Conversation 7:**
> User: "The orders total field — is that dollars or cents?"

Agent checks, answers "cents." Then:
- Calls `ListKnowledgeFiles` — no data quirks files exist
- Calls `WriteKnowledgeFile` path=`Data Notes/Orders.md` with the cents/dollars note

**User later:**
Opens Knowledge page, sees the tree has grown. Reviews the files, fixes a wording issue in Metrics.md, moves "Orders.md" into a "Billing" folder. Done.

---

## Retrieval Architecture

### Core Principle: Chunks Index, Documents Return

Traditional RAG returns matched chunks. This system uses chunks only as search pointers — the matched chunk identifies a document, and the **full document** is what gets injected into the agent's context.

**Why this matters:**
- A chunk about "Churn Rate" makes more sense when the agent also sees the MRR definition above it in the same document
- Users write coherent documents with flow and context — fragmenting them loses that
- The agent gets the same view the user sees when they open the file
- Fewer, higher-quality context injections vs. many disjointed snippets

### Data Model

#### `knowledge_files` table
```sql
CREATE TABLE knowledge_files (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id    UUID NOT NULL REFERENCES workspaces(id),
    parent_id       UUID REFERENCES knowledge_files(id),  -- folder nesting
    name            TEXT NOT NULL,                         -- display name
    is_folder       BOOLEAN NOT NULL DEFAULT false,
    content         TEXT,                                  -- markdown (null for folders)
    content_hash    TEXT,                                  -- for change detection on sync
    sort_order      INTEGER NOT NULL DEFAULT 0,
    created_by      UUID REFERENCES users(id),
    updated_by      UUID REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Two partial indexes because UNIQUE doesn't work with NULL (NULL != NULL in SQL).
-- Without these, duplicate root-level file names would silently succeed.
CREATE UNIQUE INDEX knowledge_files_root_name_unique
    ON knowledge_files (workspace_id, name)
    WHERE parent_id IS NULL;

CREATE UNIQUE INDEX knowledge_files_child_name_unique
    ON knowledge_files (workspace_id, parent_id, name)
    WHERE parent_id IS NOT NULL;
```

#### `knowledge_chunks` table
```sql
CREATE TABLE knowledge_chunks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id         UUID NOT NULL REFERENCES knowledge_files(id) ON DELETE CASCADE,
    workspace_id    UUID NOT NULL,                         -- denormalized for search
    content         TEXT NOT NULL,                         -- chunk text
    chunk_index     INTEGER NOT NULL,                      -- position in document
    embedding       vector(384) NOT NULL,                  -- BGE-small-en-v1.5
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX knowledge_chunks_embedding_idx
    ON knowledge_chunks USING hnsw (embedding vector_cosine_ops);
```

### Chunking Strategy

Fixed-size chunking. On file save:

1. **Split the document into fixed-size chunks** (~500 tokens / ~2,000 characters) with overlap (~100 tokens / ~400 characters). No markdown parsing needed — just split the text.
2. **Each chunk stores:** the chunk text, its position in the document (`chunk_index`), and an embedding of the chunk text.
3. **Short files** (under the chunk size) produce a single chunk.
4. **Extract table references:** Scan the full file content for backtick-wrapped table names. Store in `knowledge_file_tables`.
5. **Delete old chunks, insert new ones.** Chunks are ephemeral index entries — the file content is truth.

### Two Retrieval Paths

The agent has two ways to find knowledge — it picks based on the situation:

**Path A: Navigate by structure (tree → read)**
The agent sees the knowledge tree in its system prompt. When the user's question maps clearly to a known file or folder, the agent calls `ReadKnowledgeFile` directly. No vector search needed.

Example: User asks "what's our MRR?" → agent sees `Revenue/Metrics.md` in the tree → reads it directly.

**Path B: Search by semantics (vector → documents)**
When the agent doesn't know where to look, or the question spans multiple domains, it uses `SearchKnowledge` to find relevant documents via embedding similarity.

Example: User asks "why are last month's numbers off?" → agent searches → gets back `Revenue/Metrics.md` and `Data quirks/BigQuery.md` because chunks in both matched.

Both paths return full documents, not chunks.

### Retrieval Pipeline (Path B: SearchKnowledge)

```
Query
  │
  ▼
Embed query (BGE asymmetric, query prefix)
  │
  ▼
Search knowledge_chunks (cosine similarity, top-K)
  │
  ▼
Group matched chunks by file_id
  │
  ▼
Score each file = max(chunk scores)        ← best chunk determines file relevance
  │
  ▼
Quality gate: file score >= MIN_SIMILARITY (0.25)
  │
  ▼
Deduplicate against already-injected files (delta injection)
  │
  ▼
Fetch full file content for surviving files
  │
  ▼
Token budget: add files by descending score until budget exhausted
  │
  ▼
Format as context blocks:
  ┌──────────────────────────────────────┐
  │ ## Revenue / Metrics.md              │
  │                                      │
  │ {full file content}                  │
  │                                      │
  │ ## Data Notes / Orders.md            │
  │                                      │
  │ {full file content}                  │
  └──────────────────────────────────────┘
```

### Token Budget Considerations

Returning full documents instead of chunks uses more tokens per retrieval hit. Mitigations:

1. **File size guidance.** The UI could show a soft warning when a file exceeds ~2000 words, suggesting the user split it into multiple files. Smaller, focused documents = better retrieval precision and lower token cost.
2. **Fewer, better results.** Instead of returning 10 chunks from 7 different sources (current behavior), return 3-5 complete documents. The agent gets fewer sources but each one is coherent and complete.
3. **Score threshold is the primary gate.** If only 1 document scores above threshold, return 1. Don't pad with low-relevance results just to fill the budget.
4. **Large file fallback.** If a matched file exceeds the remaining token budget, return the matched chunk text with the file path as a header (e.g. `## Revenue/Metrics.md (partial)`). This gives the agent enough to decide whether to `ReadKnowledgeFile` for the full content.

### Graph Expansion (Simple)

One relationship: a knowledge file references tables. That's it — no categories, no column-level refs, no metric refs.

Populated automatically: when a file is saved, scan for backtick-wrapped identifiers matching the pattern `identifier.identifier` (at least one dot, no spaces, no SQL keywords). For example `` `billing.subscriptions` `` matches, but `` `amount` ``, `` `status = 'active'` ``, and `` `SELECT` `` do not. Store matches in `knowledge_file_tables`.

```sql
CREATE TABLE knowledge_file_tables (
    file_id         UUID NOT NULL REFERENCES knowledge_files(id) ON DELETE CASCADE,
    workspace_id    UUID NOT NULL,
    table_full_name TEXT NOT NULL,
    PRIMARY KEY (file_id, table_full_name)
);
```

This enables two simple expansions:
- **Table matched in catalog** → also inject knowledge files that reference that table
- **Knowledge file matched** → also inject the catalog entries for tables it references

Both are single SQL joins. No traversal depth, no scoring heuristics — just "these things go together."

---

## Migration Path

### Phase 1: Data Model + Backend
- Create `knowledge_files`, `knowledge_chunks`, and `knowledge_file_tables` tables
- Implement fixed-size chunking and embedding on file save
- Implement CRUD API endpoints for files and folders
- Implement `WriteKnowledgeFile`, `EditKnowledgeFile`, `ReadKnowledgeFile`, `ListKnowledgeFiles`, `SearchKnowledge` agent tools
- **Disable `SaveLearningTool` immediately.** Don't run both systems in parallel — the agent will split knowledge between them and users will have to check two places. Clean cut.

### Phase 2: Retrieval Integration
- Add `search_knowledge_files()` to VectorSearch trait
- Implement `SearchKnowledge` tool using the retrieval pipeline (returns full documents)
- Wire up graph expansion for file ↔ table references
- Remove old auto-injection logic for learnings/metrics from the per-turn retrieval pipeline

### Phase 3: Frontend
- Build Knowledge page with sidebar file tree + markdown editor
- File/folder CRUD (create, rename, move, delete)
- Drag-and-drop reordering
- Auto-save with debounce
- Search across all files

### Phase 4: Migration + Cleanup
- Migrate existing `agent_learnings` into knowledge files (group by datasource/topic)
- Migrate `workspaces.business_knowledge` and `users.knowledge` text blobs into files
- Remove LearningsManager component
- Clean up `ConversationContext` — remove `injected_learnings` and `injected_metrics` fields
- Drop `agent_learnings` and `learning_references` tables

---

## Relationship to the Database Catalog

The database catalog (datasource schema) and knowledge files are two distinct knowledge sources that serve different purposes. The retrieval pipeline searches both in parallel and merges results into a unified context block.

### What Each Source Provides

| | Database Catalog | Knowledge Files |
|---|---|---|
| **Contains** | Table names, column names, data types, descriptions | Business context, metric definitions, data quirks, institutional knowledge |
| **Populated by** | Automatic catalog sync from datasources | Users and agent through conversation |
| **Answers** | "What tables/columns exist?" | "What does this data mean? How should I use it?" |
| **Retrieval unit** | Table (with its columns) | Full document |
| **Search indexes** | `datasource_table_cache` embeddings, `column_embeddings` | `knowledge_chunks` embeddings |

### How They Work Together

The catalog tells the agent **what exists**. Knowledge files tell the agent **what it means**.

Example: User asks "What's our monthly revenue trend?"

1. **Catalog search** finds `billing.subscriptions` (columns: `amount`, `status`, `created_at`) and `billing.invoices` (columns: `total`, `paid_at`)
2. **Knowledge search** finds `Revenue/Metrics.md` which explains that MRR uses `billing.subscriptions` where `status = 'active'`, and that `amount` is in cents

The agent gets both. Without the catalog, it wouldn't know the tables exist. Without the knowledge file, it wouldn't know `amount` is in cents or which status filter to use.

### Different Retrieval Modes

**Catalog (tables/columns):** Auto-injected on every turn via the existing retrieval pipeline. Unchanged from today.

**Knowledge files:** Agent-pulled. The agent sees the file tree in its system prompt, and uses `ReadKnowledgeFile` or `SearchKnowledge` when it needs context. No auto-injection of document content.

### How They Connect

The `knowledge_file_tables` join table (see Graph Expansion above) bridges them. When the agent calls `SearchKnowledge`, the results include graph expansion — matched tables pull in related knowledge files and vice versa. One join each way, no complexity.

---

## What This Doesn't Change

- **Datasource schema indexing** — `datasource_table_cache` and `column_embeddings` stay exactly as-is. The catalog is still auto-indexed from datasource catalogs. Knowledge files don't replace or duplicate schema information.
- **Embedding model** — BGE-small-en-v1.5, ONNX, compiled into binary.
- **Token budgeting** — still enforced for catalog auto-injection. Knowledge files are agent-pulled, so token usage is bounded by what the agent chooses to read.

---

## Open Questions

1. **Permissions.** Should files have per-file permissions, or is workspace membership sufficient? Leaning toward workspace-level only for simplicity.
2. **Versioning.** Should we track edit history per file? Git-style diffs would be nice but might be over-engineering for v1. Could add later.
3. **Max file size.** Should there be a hard limit? Suggested: 10,000 words (~40k chars). Above that, retrieval token cost becomes problematic and the user should split the file.
4. **File count limits.** Per workspace? Suggested: soft limit at 100 files, hard limit at 500. Most workspaces won't need more than 20-30.
