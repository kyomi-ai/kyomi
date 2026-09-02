# Assert visibility, not DOM presence, for a control the user must see

A Playwright locator's `count()` (and its cousins `toBeAttached()`, a bare
`querySelector` truthiness check) answers "does this element exist in the
DOM?" — not "can the user see it?" Leptos, like React, routinely keeps a
control mounted but hidden: a `display: none` branch, a collapsed
`<Show when=...>` that still renders its fallback slot, an element sized to
zero behind a sibling. `count()` is satisfied by all of these. When the
defect under test is precisely "the control never appears" — a gated button
that should reveal itself once a precondition is met, a warning banner that
should show for a given auth mode — a `count()` assertion can pass on the
exact broken build, because the element it's counting was never the thing
in question.

This is not hypothetical: it is why this rule exists. A customer reported
that BigQuery's "Validate & Discover Projects" button never appeared after
they supplied a service-account key. Had the regression spec for that
defect asserted
`await page.locator('button:has-text("Validate & Discover Projects")').count()`,
it would have returned `1` on the broken build — the button was in the DOM,
just hidden — and the spec would have gone green on the bug it was written
to catch.

**Rule:** When a test's job is to prove a control is visible to the user —
especially when the acceptance criterion is literally "X appears" or "X is
hidden until Y" — assert `isVisible()` (or `toBeVisible()` in `expect()`
form), never `count()` or DOM-attachment alone. `count()` remains correct
for a different question — "how many rows did this render?" — where DOM
presence is what's actually being verified; it's the wrong tool only when
visibility is the thing under test.

```js
// WRONG — passes whether the button is visible or display:none'd, so it
// goes green on exactly the defect it exists to catch.
const count = await page
  .locator('button:has-text("Validate & Discover Projects")')
  .count();
check('"Validate & Discover Projects" is visible', count > 0);

// RIGHT — isVisible() reflects actual visibility (layout, display, opacity,
// visibility, zero-size ancestors), not mere DOM presence.
const visible = await page
  .locator('button:has-text("Validate & Discover Projects")')
  .first()
  .isVisible()
  .catch(() => false);
check('"Validate & Discover Projects" is visible', visible);
```

Origin: `scripts/e2e-regression/bigquery-create-modal.cjs`, written against
a customer's reported defect (KYO-404, 405, 408, 411, 413, 417) and carried
into its own header doc-comment as project convention; formalized as a
standard under KYO-602.
