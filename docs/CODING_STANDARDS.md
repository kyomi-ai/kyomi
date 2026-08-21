# Coding Standards

This is the index for coding standards learned from code reviews — the standards themselves live one rule per file under `docs/standards/<section>/`. The set evolves over time — the orchestrator adds new rule files at the start of each `/agent-driven-development` session by mining recent review logs for recurring patterns.

**Read the relevant sections below before implementing any feature.** Every rule exists because agents have repeatedly made the same mistake, and a code reviewer had to catch it.

These standards are specific to patterns observed in this codebase. For general architecture principles, see `CLAUDE.md`. For the full anti-pattern checklist used by reviewers, see `.claude/agents/code-review-architect.md`.

---

## How to add a standard

Each rule lives in its own file: `docs/standards/<section-slug>/<rule-slug>.md`. **Adding a new standard creates exactly one new file and modifies no existing line** — not this index, not any other rule's file.

This is deliberate, not incidental style: two concurrently-open PRs that each append a standard to a single monolithic file collide in git on the same tail region, every time. That cost two rework cycles before the split (PRs #337/#339 on 2026-08-13, and #336 on 2026-08-12) — see KYO-375. A one-file-per-rule layout makes that class of collision structurally impossible: two new rules are two new files, and git has nothing to merge.

To add a rule:

1. Pick (or create) the section directory under `docs/standards/` — kebab-case of the section name, e.g. `docs/standards/testing/`.
2. Add `<rule-slug>.md` in that directory, with a single `# Rule Title` heading followed by the rule body (rationale, **Rule:**, a WRONG/RIGHT example, and the ticket(s) that motivated it — follow the shape of any existing rule file in the same section).
3. **Do not add a line to this index for the new rule.** This index enumerates sections only, precisely so that adding a rule never touches a shared line. `ls docs/standards/<section>/` is how a reader discovers what's in a section.
4. If the rule doesn't fit any existing section, add a new section: a new directory with a `README.md` (`# Section Name` heading + one italic blurb line), plus one link added here under *Standards*.

## Standards

- [Error Handling](standards/error-handling/) — *Standards for how errors should be propagated, contextualized, and reported.*
- [Leptos / Frontend Patterns](standards/leptos-frontend-patterns/) — *Standards specific to Leptos components, reactivity, SSR/hydration, and frontend architecture.*
- [Email Templates](standards/email-templates/) — *Standards for HTML email templates (alert.rs, email_service.rs, feedback_service.rs, analytics_notifications.rs).*
- [API & Server Functions](standards/api-server-functions/) — *Standards for server functions, REST endpoints, and the boundary between frontend and backend.*
- [Data & State Management](standards/data-state-management/) — *Standards for database access, caching, state synchronization, and data flow.*
- [Security](standards/security/) — *Standards for encryption, authentication, credential handling, and input validation.*
- [Code Organization](standards/code-organization/) — *Standards for module structure, imports, shared utilities, and avoiding duplication.*
- [Comments & Documentation](standards/comments-documentation/) — *Standards for what comments may claim, and keeping those claims true.*
- [String & Text Processing](standards/string-text-processing/) — *Standards for safe string manipulation in Rust.*
- [Testing](standards/testing/) — *Standards for test structure, assertions, and what must be tested.*
- [Version Control & Working Tree](standards/version-control-working-tree/) — *Standards for reasoning about repo state before drawing conclusions from it.*
