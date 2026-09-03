# Scanning Markdown for a convention marker means excluding code blocks

A scanner that looks for a convention line — `Closes KYO-NN` in a PR body, a heading in a
ticket description, a directive in an `.md` file — is usually written as a per-line regex
over `splitlines()`. That is correct for the *prose* of a Markdown document and wrong for
the document, because Markdown has a construct whose entire purpose is "these lines are
not content": the fenced block, and its indented cousin.

The false positive this produces is not exotic. The most likely place for a literal
`Closes KYO-NN` line to appear is the documentation of the `Closes KYO-NN` convention — and
in this repo that lives inside a fenced example block in `CLAUDE.md`. Any PR that edits or
quotes that section carries the marker in its own body. When a live sample of 50 recently
merged PRs was taken during review, 20 of them (40%) contained fenced blocks.

Which direction the mistake breaks matters as much as the match itself. A scanner that
drives an irreversible action — marking somebody else's ticket Done — must prefer missing a
candidate to inventing one, and must say so where the next person will read it.

**Rule:** Any line-oriented matcher run over Markdown-bearing text must model the format
before it matches:

- Track fenced blocks for both delimiters (```` ``` ```` and `~~~`), and require the closing
  fence to use the *same* character — one kind does not close the other.
- Test indentation on the **untrimmed** line, before any `strip()`. Trimming first destroys
  the only signal (4+ leading spaces, or a leading tab) that marks an indented code block,
  and makes `    Closes KYO-97` byte-identical to a real one.
- Keep the fence state local to one document, so an unterminated fence in a malformed body
  cannot leak into the next one.
- Anchor the match itself (`^closes\s+kyo-(\d+)…`), don't free-text search.
- Validate against the real corpus, not invented fixtures: pull actual PR bodies
  (`gh pr list --search … --json body`) and grep the docs that *document* the convention,
  which are the highest-probability false positive you have.

```python
# WRONG — every line of the body is a candidate, including the ones Markdown has
# already marked as "not content".
for line in body.splitlines():
    m = CLOSES_RE.match(line.strip())

# RIGHT — scripts/reconcile-merged-tickets.sh, extract_closed_tickets(): fence state
# first, indentation checked on the raw line, CLOSES_RE only on what survives.
for raw_line in body.splitlines():
    if in_fence is not None:
        m_fence = FENCE_RE.match(raw_line)
        if m_fence and m_fence.group(1)[0] == in_fence:
            in_fence = None
        continue
    m_fence = FENCE_RE.match(raw_line)
    if m_fence:
        in_fence = m_fence.group(1)[0]
        continue
    if raw_line.startswith("    ") or raw_line.startswith("\t"):
        continue
    m = CLOSES_RE.match(raw_line.strip())
```

Flagged in **KYO-617** (`2026-09-03`, `13:01`) — the sole 🟡 on `reconcile-merged-tickets.sh`,
reproduced live by the reviewer: a body containing a fenced `Closes KYO-99` and a body
containing a 4-space-indented `Closes KYO-97` both produced real candidate rows, in a script
whose output drives marking tickets Done. It was not covered by the ticket's accepted-gaps
list, and the reviewer recommended fixing it in the same diff rather than deferring, on the
grounds that the trigger is already common in this repo's own PR bodies. Cycle 2 (`14:20`)
landed the fence/indent gate with 9 new assertions and was verified against a hand-built
adversarial fixture — backtick fence, tilde fence, 4-space indent, tab indent, blockquote,
and an unterminated fence followed by an unrelated body — confirming both that all five
code-block shapes are excluded and that fence state does not leak across PRs. The
trim-then-match ordering was the specific defect that let the indented case through, and
the fixed function now says so in its own docstring, along with the deliberate
false-positive-is-worse-than-false-negative trade for an unterminated fence.

The other rule in this section,
[no-byte-slicing-strings-use-chars-take.md](no-byte-slicing-strings-use-chars-take.md),
is the same failure with a different substrate: the naive string operation is correct on the
data you wrote the fixtures with and wrong on the data users actually produce. Closest
overlap anywhere in the corpus is
`error-handling/validate-a-suppression-predicate-anchored-not-by-substring-deletion.md`
(in flight on the KYO-629 branch; not on `origin/main` as of `2026-09-04`), which covers
anchoring a *predicate* match rather than deleting a known sub-term and checking what is
left — the same "match the structure, not a substring" instinct applied to a lint's `cfg`
predicate instead of to a document's line grammar.
