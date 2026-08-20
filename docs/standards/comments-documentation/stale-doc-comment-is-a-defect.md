# A doc comment that outlived its behavior is a defect, not a nit

When behavior deliberately changes, the doc comment above it becomes an active lie — and it is more dangerous than no comment, because the next reader (or agent) trusts it over the code. Tests written against the old comment fail, and the failure looks like a bug in the code rather than a stale contract.

**Rule:** Changing a route's or function's observable contract means updating its doc comment in the same commit. When you find a stale one, correct it and cite the ticket that changed the behavior, so the next reader can see *when* the contract moved rather than guessing which of the two is current.

Flagged in KYO-256: `apps/server/src/routes/mcp.rs` documented `present-and-invalid returns 404 (forces re-initialize)` long after the route was changed to auto-heal invalid sessions into a new 200 response — and three contract tests were still asserting the documented 404.
