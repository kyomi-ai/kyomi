# Agent Learning System Specification

**Version:** 1.0 MVP
**Date:** 2025-01-15
**Status:** Implementation in Progress

## Overview

The Agent Learning System enables Kyomi's AI agent to automatically learn from conversations and apply those learnings in future interactions. This creates a self-improving system where the agent becomes smarter over time without requiring manual knowledge base updates.

## Motivation

### The Problem

Users frequently need to correct or guide the agent:
- "No, use the transactions table for revenue, not analytics.old_revenue"
- "When I say MAU, I mean Monthly Active Users from the mobile app, not web"
- "Always exclude test accounts ending in @test.com"

Currently, these corrections are lost after the conversation ends. Users must re-teach the same lessons in every new chat session.

### The Solution

**Automatic Learning**: The agent recognizes when it learns something valuable and saves it automatically using a `save_learning` tool.

**Workspace-scoped**: Learnings are shared across all users in a workspace, so when one person teaches the agent, everyone benefits.

**Semantic Retrieval**: When starting a new conversation, the system retrieves relevant past learnings and injects them into the agent's context.

**Admin Management**: Workspace admins can review, disable, or delete learnings through a simple UI.

## Design Decisions

### 1. Workspace-scoped (Not User-scoped)

**Decision**: Learnings are shared across the entire workspace.

**Rationale**:
- Table locations and data facts are workspace truths, not personal preferences
- Terminology (e.g., "MAU means mobile app") applies to all team members
- Faster collective learning - one correction benefits everyone
- Aligns with existing Workspace Knowledge concept
- User Knowledge already exists for personal preferences

**Trade-off**: Individual user preferences will need to go in User Knowledge (manual) rather than auto-learnings.

### 2. Simple On/Off (No Voting/Scoring)

**Decision**: Learnings can only be enabled or disabled (plus deleted).

**Rationale**:
- Simpler MVP - less complexity to build and maintain
- Easier for admins to understand and manage
- Avoid premature optimization - we don't yet know how voting would be used
- Can add voting/quality signals later if needed

**Trade-off**: No automatic quality filtering. Admins must manually review bad learnings.

### 3. Natural Language (No Structured Schema)

**Decision**: Learnings are stored as free-form text written by the LLM.

**Rationale**:
- More flexible - can capture any type of insight
- LLM writes in its own words, easier to inject back into prompts
- Avoids forcing patterns into rigid categories
- Can capture corrections, clarifications, preferences, domain knowledge

**Trade-off**: Harder to analyze programmatically. Can't easily query "all table corrections."

### 4. Tool-based (Not Post-processing)

**Decision**: Agent explicitly calls `save_learning` tool during conversation.

**Rationale**:
- No extra LLM API calls (happens during normal processing)
- Agent has full context of what it learned and why
- Real-time learning (available immediately)
- Agent can explain its reasoning

**Trade-off**: Relies on agent to recognize learnings. Might miss some. (Can add async analysis later as safety net.)

## Architecture

### Knowledge Hierarchy

```
┌──────────────────────────────────────────────────────┐
│ LAYER 1: Workspace Knowledge (Manual, Canonical)     │
│ - Admin-written markdown                             │
│ - Business definitions, metrics, policies            │
│ - HIGHEST PRIORITY                                   │
└────────────────────┬─────────────────────────────────┘
                     │ overrides
                     ↓
┌──────────────────────────────────────────────────────┐
│ LAYER 2: User Knowledge (Manual, Personal)           │
│ - Personal preferences, SQL shortcuts                │
│ - HIGH PRIORITY (for this user)                      │
└────────────────────┬─────────────────────────────────┘
                     │ overrides
                     ↓
┌──────────────────────────────────────────────────────┐
│ LAYER 3: Agent Learnings (Auto, Workspace) - NEW     │
│ - Automatically discovered insights                  │
│ - Semantic retrieval (top 5 relevant)                │
│ - Can be disabled/deleted by admin                   │
│ - MEDIUM PRIORITY                                    │
└────────────────────┬─────────────────────────────────┘
                     │
                     ↓
┌──────────────────────────────────────────────────────┐
│ LAYER 4: Session Context (Auto, Temporary)           │
│ - Recent conversation history                        │
│ - LOWEST PRIORITY (just context)                     │
└──────────────────────────────────────────────────────┘
```

### Data Flow

```
┌─────────────────────────────────────────────────────────────┐
│ 1. LEARNING CAPTURE                                         │
│                                                             │
│  User: "No, use transactions table for revenue"            │
│          ↓                                                  │
│  Agent realizes: "I was corrected"                          │
│          ↓                                                  │
│  Agent calls: save_learning({                               │
│    insight: "For revenue queries, use sales.transactions   │
│             instead of analytics.old_revenue. The old      │
│             table is deprecated.",                          │
│    context: "User explicitly corrected me"                  │
│  })                                                         │
│          ↓                                                  │
│  LearningService:                                           │
│    - Generates embedding for insight                        │
│    - Stores in agent_learnings table                        │
│    - Returns learning_id                                    │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│ 2. LEARNING RETRIEVAL (Next Conversation)                   │
│                                                             │
│  User: "Show me revenue trends"                             │
│          ↓                                                  │
│  System builds prompt:                                      │
│    - Embed user query                                       │
│    - Semantic search agent_learnings (top 5)                │
│    - Only retrieve enabled=true                             │
│    - Filter by similarity > 0.6                             │
│          ↓                                                  │
│  Inject into system prompt:                                 │
│    "Relevant insights from past conversations:             │
│     - For revenue queries, use sales.transactions..."      │
│          ↓                                                  │
│  Agent uses learning:                                       │
│    - Goes directly to correct table                         │
│    - No trial-and-error needed                              │
│    - Saves tokens and time                                  │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│ 3. ADMIN MANAGEMENT                                         │
│                                                             │
│  Admin opens Knowledge page → Learnings tab                 │
│          ↓                                                  │
│  Views all learnings:                                       │
│    - "For revenue queries, use..." [ENABLED] [DELETE]      │
│    - "MAU means mobile app users..." [DISABLED] [DELETE]   │
│          ↓                                                  │
│  Admin actions:                                             │
│    - Toggle off bad/outdated learnings                      │
│    - Delete incorrect learnings                             │
│    - Review what agent has learned                          │
└─────────────────────────────────────────────────────────────┘
```

## Database Schema

### Table: agent_learnings

```sql
CREATE TABLE agent_learnings (
    learning_id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    workspace_id VARCHAR(50) NOT NULL,

    -- The learning content
    insight TEXT NOT NULL,
    context TEXT,  -- Optional: why/how this was learned
    embedding vector(384),  -- For semantic search

    -- Simple lifecycle
    enabled BOOLEAN DEFAULT TRUE,

    -- Tracking (for admin visibility)
    learned_from_session VARCHAR(50),
    learned_from_user VARCHAR(50),
    created_at TIMESTAMP DEFAULT NOW(),
    times_used INTEGER DEFAULT 0,
    last_used_at TIMESTAMP,

    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id)
);

-- Indexes
CREATE INDEX idx_learnings_embedding ON agent_learnings
USING hnsw (embedding vector_cosine_ops);

CREATE INDEX idx_learnings_workspace ON agent_learnings(workspace_id, enabled);
```

**Field Descriptions**:
- `learning_id`: Unique identifier
- `workspace_id`: Scopes learning to workspace (shared by all users)
- `insight`: The actual learning in natural language (required)
- `context`: Optional explanation of how/why this was learned
- `embedding`: 384-dim vector for semantic similarity search (all-MiniLM-L6-v2)
- `enabled`: Admin can toggle learnings off without deleting
- `learned_from_session`: Which chat session generated this learning
- `learned_from_user`: Which user's interaction led to this learning
- `times_used`: Counter incremented each time learning is retrieved
- `last_used_at`: Timestamp of most recent retrieval

## Agent Tool: save_learning

### Tool Definition

```python
{
    "name": "save_learning",
    "description": """Save important insights you discover during the conversation.

    Use this when you learn something that would help in future conversations:
    - User corrects which table/column to use
    - User clarifies terminology (e.g., "MAU means Monthly Active Users in the iOS app")
    - You discover a better approach after trial and error
    - User provides domain knowledge about their data
    - You find that certain tables are better for certain queries

    Write learnings in a way that your future self would find helpful.
    Be specific about when this learning applies.""",

    "input_schema": {
        "type": "object",
        "properties": {
            "insight": {
                "type": "string",
                "description": "What you learned, written as advice to your future self. Be specific and actionable."
            },
            "context": {
                "type": "string",
                "description": "Optional: What happened that taught you this? Why is this useful?"
            }
        },
        "required": ["insight"]
    }
}
```

### Example Invocations

**Table Correction:**
```json
{
  "insight": "For revenue queries, use sales.transactions table instead of analytics.old_revenue. The old_revenue table was deprecated in Jan 2024 and is missing recent data.",
  "context": "User asked for revenue trends. I initially used analytics.old_revenue but user corrected me to use sales.transactions instead. The corrected query returned complete data."
}
```

**Terminology Clarification:**
```json
{
  "insight": "When user refers to MAU, they mean Monthly Active Users from the mobile app (mobile_app_analytics.daily_users table), not web analytics. The mobile app is their primary product.",
  "context": "User asked for MAU data. I initially queried web_analytics.users but they clarified they meant mobile app users."
}
```

**Data Quality Discovery:**
```json
{
  "insight": "The staging.customers table is more up-to-date than prod.customers. There is a ~2 hour sync delay to production. Use staging for recent customer data.",
  "context": "User pointed out customer counts were outdated. Switching to staging schema resolved the issue."
}
```

**Business Rule:**
```json
{
  "insight": "When calculating customer metrics, always exclude test accounts (emails ending in @test.com or @company.com). These are internal test accounts, not real customers.",
  "context": "User noticed inflated customer counts. They explained that test accounts were being included."
}
```

## API Endpoints

### Backend Routes

**File**: `apps/backend/src/api/routers/workspaces.py`

```python
@router.get("/workspace/learnings")
async def get_learnings(
    workspace_id: str = Depends(get_workspace_id)
) -> List[LearningResponse]:
    """Get all learnings for workspace (admin view)"""
    pass

@router.patch("/workspace/learnings/{learning_id}")
async def update_learning(
    learning_id: str,
    update: LearningUpdate,
    workspace_id: str = Depends(get_workspace_id)
) -> SuccessResponse:
    """Update learning (toggle enabled status)"""
    pass

@router.delete("/workspace/learnings/{learning_id}")
async def delete_learning(
    learning_id: str,
    workspace_id: str = Depends(get_workspace_id)
) -> SuccessResponse:
    """Delete a learning"""
    pass
```

### Request/Response Models

```python
class LearningResponse(BaseModel):
    learning_id: str
    workspace_id: str
    insight: str
    context: Optional[str]
    enabled: bool
    created_at: datetime
    times_used: int
    last_used_at: Optional[datetime]
    learned_from_user: str

class LearningUpdate(BaseModel):
    enabled: bool

class SuccessResponse(BaseModel):
    success: bool
```

## Service Layer

### LearningService

**File**: `apps/backend/src/api/services/learning_service.py`

```python
class LearningService:
    """Manages agent learnings with semantic search"""

    async def save_learning(
        self,
        workspace_id: str,
        user_id: str,
        session_id: str,
        insight: str,
        context: Optional[str] = None
    ) -> str:
        """Save a new learning with embedding"""
        # Generate embedding
        # Store in database
        # Return learning_id
        pass

    async def get_relevant_learnings(
        self,
        workspace_id: str,
        query: str,
        limit: int = 5,
        min_similarity: float = 0.6
    ) -> List[Learning]:
        """Get enabled learnings relevant to query via semantic search"""
        # Embed query
        # Search with pgvector
        # Filter by enabled=true and similarity threshold
        # Return top matches
        pass

    async def get_all_learnings(
        self,
        workspace_id: str
    ) -> List[Learning]:
        """Get all learnings for admin view (no filtering)"""
        pass

    async def update_learning(
        self,
        learning_id: str,
        workspace_id: str,
        updates: dict
    ) -> None:
        """Update learning (toggle enabled)"""
        pass

    async def delete_learning(
        self,
        learning_id: str,
        workspace_id: str
    ) -> None:
        """Delete a learning"""
        pass

    async def increment_usage(
        self,
        learning_id: str
    ) -> None:
        """Increment times_used counter"""
        pass
```

## Frontend UI

### Knowledge Page Enhancement

**File**: `apps/frontend/src/pages/Knowledge.jsx`

Add a third tab to the existing Knowledge page:

```jsx
<Tabs value={activeTab} onValueChange={setActiveTab}>
  <TabsList>
    <TabsTrigger value="workspace">Workspace</TabsTrigger>
    <TabsTrigger value="user">Personal</TabsTrigger>
    <TabsTrigger value="learnings">Learnings</TabsTrigger>  {/* NEW */}
  </TabsList>

  <TabsContent value="workspace">
    {/* Existing workspace knowledge editor */}
  </TabsContent>

  <TabsContent value="user">
    {/* Existing user knowledge editor */}
  </TabsContent>

  <TabsContent value="learnings">
    <LearningsManager />  {/* NEW COMPONENT */}
  </TabsContent>
</Tabs>
```

### LearningsManager Component

**File**: `apps/frontend/src/components/LearningsManager.jsx`

**Features**:
- List all learnings for workspace
- Show enabled/disabled status
- Toggle enabled/disabled
- Delete learnings
- Show metadata (date, usage count)

**UI Layout**:
```
┌─────────────────────────────────────────────────────────────┐
│ Auto-Learnings                    3 active                  │
│ Insights the AI has learned from conversations              │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│ ┌───────────────────────────────────────────────────────┐ │
│ │ For revenue queries, use sales.transactions instead   │ │
│ │ of analytics.old_revenue. The old table is deprecated│ │
│ │                                                       │ │
│ │ Learned Jan 10, 2025 • Used 23 times                  │ │
│ │                                         [ON]  [🗑️]    │ │
│ └───────────────────────────────────────────────────────┘ │
│                                                             │
│ ┌───────────────────────────────────────────────────────┐ │
│ │ MAU means Monthly Active Users from mobile app        │ │
│ │                                                       │ │
│ │ Learned Jan 12, 2025 • Used 8 times                   │ │
│ │                                         [ON]  [🗑️]    │ │
│ └───────────────────────────────────────────────────────┘ │
│                                                             │
│ ┌───────────────────────────────────────────────────────┐ │
│ │ Exclude test accounts (@test.com) from customer       │ │
│ │ metrics                                               │ │
│ │                                                       │ │
│ │ Learned Jan 08, 2025 • Used 2 times                   │ │
│ │                                         [OFF] [🗑️]    │ │
│ └───────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

## System Prompt Integration

### Retrieval and Injection

**File**: `apps/backend/src/api/agent/chat_agent_adapter.py`

Modified `_build_system_prompt()` method:

```python
async def _build_system_prompt(self, user_query: str = None):
    """Build system prompt with all knowledge layers"""

    base_prompt = get_base_system_prompt()

    # Layer 1: Workspace Knowledge (existing)
    workspace_knowledge = self.workspace.business_knowledge or ""

    # Layer 2: User Knowledge (existing)
    user_knowledge = self.user.knowledge or ""

    # Layer 3: Agent Learnings (NEW)
    learnings_section = ""
    if user_query:
        learnings = await learning_service.get_relevant_learnings(
            workspace_id=self.workspace.workspace_id,
            query=user_query,
            limit=5,
            min_similarity=0.6
        )

        if learnings:
            # Increment usage counters
            for learning in learnings:
                await learning_service.increment_usage(learning.learning_id)

            # Format for prompt
            learning_items = "\n".join([
                f"- {l.insight}" for l in learnings
            ])

            learnings_section = f"""

## Relevant Insights from Past Conversations

These were learned from previous interactions. Apply when relevant:

{learning_items}

Note: If these conflict with Workspace Knowledge above, follow the canonical knowledge.
"""

    # Combine all layers
    return f"""
{base_prompt}

## Workspace Knowledge
{workspace_knowledge}

## Your Personal Preferences
{user_knowledge}
{learnings_section}
"""
```

## Implementation Phases

### Phase 1: Core Backend (Week 1)

**Goals**:
- Agent can save learnings
- Learnings are retrieved and injected into prompts
- Basic functionality working

**Tasks**:
1. ✅ Create database migration for `agent_learnings` table
2. ✅ Create `learning_service.py` with:
   - `save_learning()` - Store learning with embedding
   - `get_relevant_learnings()` - Semantic search retrieval
   - `increment_usage()` - Track usage
3. ✅ Add `save_learning` tool to agent tool list
4. ✅ Integrate learning retrieval in `chat_agent_adapter.py`
5. ✅ Test: Agent can save and retrieve learnings

**Deliverables**:
- Database table created
- Service layer implemented
- Agent tool functional
- Learnings injected into prompts

### Phase 2: Admin UI (Week 2)

**Goals**:
- Admins can view all learnings
- Admins can toggle/delete learnings
- Visibility into what agent has learned

**Tasks**:
1. ✅ Add backend API endpoints:
   - `GET /workspace/learnings` - List all
   - `PATCH /workspace/learnings/:id` - Toggle enabled
   - `DELETE /workspace/learnings/:id` - Delete
2. ✅ Add "Learnings" tab to `Knowledge.jsx`
3. ✅ Create `LearningsManager.jsx` component:
   - List learnings
   - Toggle enabled/disabled
   - Delete learnings
   - Show metadata
4. ✅ Test: Can view, toggle, and delete learnings in UI

**Deliverables**:
- API endpoints implemented
- UI component built
- Admin can manage learnings

### Phase 3: End-to-End Testing & Polish (Week 3)

**Goals**:
- Full workflow tested in browser
- Bug fixes and refinements
- Documentation updated

**Tasks**:
1. ✅ Test complete flow:
   - User corrects agent → Agent saves learning
   - New chat → Learning retrieved and applied
   - Admin views in UI → Toggles/deletes learning
2. ✅ Verify prompt injection works correctly
3. ✅ Test edge cases:
   - No relevant learnings found
   - Disabled learnings not retrieved
   - Multiple similar learnings
4. ✅ Polish UI (loading states, error handling)
5. ✅ Update documentation

**Deliverables**:
- Fully working end-to-end
- Edge cases handled
- Production-ready

## Success Metrics

### Technical Metrics

- **Learning Creation Rate**: How many learnings saved per 100 conversations?
- **Learning Usage Rate**: What % of learnings are actually retrieved and used?
- **Retrieval Relevance**: Are retrieved learnings semantically relevant? (spot check)
- **Token Savings**: Does learning reduce trial-and-error iterations?

### User Metrics

- **Admin Engagement**: Do admins review the learnings tab?
- **Disable Rate**: What % of learnings are disabled? (indicates quality)
- **Delete Rate**: What % are deleted? (indicates bad learnings)

### Quality Indicators

- **Good**: High usage count, never disabled, frequently retrieved
- **Bad**: Created but never used, disabled by admin, low similarity scores

## Future Enhancements (Post-MVP)

### Potential Additions

1. **Voting/Scoring**: Let users upvote/downvote learnings
2. **Promotion Workflow**: Promote good learnings to canonical Workspace Knowledge
3. **Async Analysis**: Safety net to catch learnings agent missed
4. **User-level Learnings**: Personal learnings in addition to workspace
5. **Learning Decay**: Reduce weight of old learnings over time
6. **Conflict Detection**: Alert when learnings contradict each other
7. **Analytics Dashboard**: Visualize what's being learned over time
8. **Export/Import**: Share learnings across workspaces

### Not in Scope (Yet)

- ❌ Complex ranking algorithms
- ❌ Automatic quality scoring
- ❌ Learning categorization/tagging
- ❌ Cross-workspace learning sharing
- ❌ Learning versioning/history

## Examples

### Example 1: Table Correction

**Conversation:**
```
User: "Show me monthly revenue trends"

Agent: [Searches catalog for "revenue"]
Agent: [Finds analytics.old_revenue and sales.transactions]
Agent: [Picks analytics.old_revenue]
Agent: "Here's the revenue data from analytics.old_revenue..."

User: "That table is deprecated. Use sales.transactions instead."

Agent: [Realizes it was corrected]
Agent: [Calls save_learning({
  insight: "For revenue queries, use sales.transactions instead of
           analytics.old_revenue. The old_revenue table is deprecated.",
  context: "User explicitly corrected table choice"
})]
Agent: [Retrieves data from sales.transactions]
Agent: "You're right, here's the updated data from sales.transactions..."
```

**Next conversation (same or different user):**
```
User: "What was our revenue last quarter?"

System: [Retrieves learnings for "revenue last quarter"]
System: [Finds: "For revenue queries, use sales.transactions..."]
System: [Injects into prompt]

Agent: [Sees learning in context]
Agent: [Goes directly to sales.transactions]
Agent: "Here's Q4 revenue from sales.transactions..." ✓
```

### Example 2: Terminology Clarification

**Conversation:**
```
User: "Show me MAU trends"

Agent: [Searches for "MAU trends"]
Agent: [Finds web_analytics.users and mobile_app_analytics.daily_users]
Agent: [Picks web_analytics.users]
Agent: "Here are the monthly active users from web analytics..."

User: "No, I meant MAU for the mobile app, not web"

Agent: [Realizes misunderstanding]
Agent: [Calls save_learning({
  insight: "MAU (Monthly Active Users) refers to mobile app users from
           mobile_app_analytics.daily_users table, not web analytics.",
  context: "User clarified MAU means mobile app, not web"
})]
Agent: "Got it, here's mobile app MAU from mobile_app_analytics..."
```

**Next conversation:**
```
User: "How's MAU looking this month?"

System: [Retrieves learnings for "MAU this month"]
System: [Finds: "MAU refers to mobile app users..."]
System: [Injects into prompt]

Agent: [Sees learning]
Agent: [Queries mobile_app_analytics.daily_users directly]
Agent: "Here's this month's mobile app MAU..." ✓
```

### Example 3: Business Rule Discovery

**Conversation:**
```
User: "How many customers do we have?"

Agent: [Queries customers table]
Agent: "You have 1,247 customers"

User: "That seems high. Make sure you're excluding test accounts -
      anything with @test.com or @company.com email addresses"

Agent: [Re-runs with filter]
Agent: [Calls save_learning({
  insight: "When calculating customer counts, always exclude test accounts
           (emails ending in @test.com or @company.com). These are internal
           test accounts.",
  context: "User pointed out inflated counts due to test accounts"
})]
Agent: "You're right, excluding test accounts: 1,089 customers"
```

**Next conversation:**
```
User: "Show me customer growth by month"

System: [Retrieves learnings for "customer growth"]
System: [Finds: "exclude test accounts..."]
System: [Injects into prompt]

Agent: [Sees learning]
Agent: [Automatically adds WHERE email NOT LIKE '%@test.com' AND ...]
Agent: "Here's customer growth (excluding test accounts)..." ✓
```

## Conclusion

The Agent Learning System transforms Kyomi from a stateless AI into a continuously improving assistant that learns from every interaction. By automatically capturing corrections, clarifications, and domain knowledge, the system reduces repetitive user effort and improves response quality over time.

This MVP focuses on simplicity: automatic capture, semantic retrieval, and basic admin controls. Future iterations can add sophistication (voting, analytics, promotion) based on real-world usage patterns.

**Key Principle**: Start simple, learn from usage, iterate based on data.
