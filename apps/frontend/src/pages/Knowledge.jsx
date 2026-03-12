// SPDX-License-Identifier: AGPL-3.0-or-later
import React from 'react';
import LearningsManager from '../components/LearningsManager';

/**
 * Knowledge - Workspace knowledge base
 *
 * NOTE (Feb 2026): This page previously had three tabs: "Learnings",
 * "My Knowledge", and "Workspace Knowledge". The latter two were markdown
 * document editors (Tiptap WYSIWYG + Monaco source) that saved to
 * `users.knowledge` and `workspaces.business_knowledge` DB columns.
 * That content gets injected verbatim into the LLM system prompt via
 * prompt.rs (build_system_prompt → load_user_info / load_workspace_knowledge).
 *
 * We removed the UI because these markdown docs are "dumb context" — the
 * entire document is dumped into every prompt regardless of relevance. This
 * doesn't scale and conflicts with the FalkorDB knowledge graph, which
 * retrieves only relevant learnings per conversation. The plan is to
 * eventually chunk/embed these documents into the graph for context-aware
 * retrieval instead of brute-force prompt injection.
 *
 * The backend endpoints and prompt injection logic are intentionally LEFT
 * INTACT so existing saved knowledge still works. Only the UI is disabled.
 *
 * Backend endpoints (still active):
 *   - GET/PUT /api/v1/users/me/knowledge
 *   - GET/PUT /api/v1/workspaces/{id}/knowledge
 * Prompt injection (still active):
 *   - kyomi-agent/src/prompt.rs: build_system_prompt()
 */
const Knowledge = () => {
  return (
    <div className="flex flex-col h-full bg-muted" style={{flexDirection: 'column'}}>
      {/* Header */}
      <div className="h-16 border-b border-border bg-card px-6 flex-shrink-0 flex items-center justify-between">
        <h1 className="text-2xl font-semibold text-foreground">Knowledge</h1>
      </div>

      {/* Content Area */}
      <div className="flex-1 flex flex-col overflow-hidden p-4 md:p-6">
        <div className="flex-1 min-h-0 overflow-hidden flex flex-col bg-card rounded-xl shadow-sm border border-border p-4 md:p-6">
          <LearningsManager />
        </div>
      </div>
    </div>
  );
};

export default Knowledge;
