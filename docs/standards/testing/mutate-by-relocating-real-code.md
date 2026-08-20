# Mutate by relocating the real code, not by a cheap proxy — especially on a tests-only PR

A PR that adds only tests and comments is the class where a green suite proves least: there is no production change to point at, so the only evidence the tests are load-bearing is a mutation that turns them red. Two refinements follow from that.

First, **the reviewer re-runs the mutations rather than trusting the implementer's report.** A mutation table in a PR body is a claim, not a result.

Second, **prefer physically relocating the code under test over a proxy that merely disables it.** For an ordering invariant — "this read must happen before that `DELETE`" — moving the block below the `DELETE` is the honest mutation; a `WHERE 1 = 0` on the read is the cheap one. The relocation also proves the mutated arrangement still *compiles*, which is what makes "a future refactor could reintroduce this" a real risk rather than a hypothetical. A proxy that does not compile in the shape a refactor would actually produce has not tested the risk the comment claims to guard.

Applied in the KYO-313 review (2026-08-10), a zero-production-logic PR of 6 regression tests pinning the capture-visibility-before-`DELETE` ordering at three hard-delete sites: 5 mutations were applied, compiled and run, and all killed their targets. Mutation B physically moved the `shared_map` construction below the deletes and reproduced the exact failure the comment predicts (`got delta: []`).
