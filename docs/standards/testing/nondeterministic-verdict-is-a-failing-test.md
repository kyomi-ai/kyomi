# A verdict that flips between runs is a failing test — and "I ran it twice" is not the check

A test whose aggregate exit code changes across runs against *unchanged* code and an unchanged server has already told you it does not know the answer. It is worse than a red test: a red test blocks, whereas a flaky one merges on the run that happened to be green and then spends its life being re-run until it agrees. The usual defence — "I ran it twice and got the same result" — is not evidence, because two runs of a coin-flip agree half the time, and because the claim is almost always made from memory rather than from two recorded exit codes.

The failure is nearly always a real race between the assertion and something the app legitimately does — a cache invalidation, a refetch, a remount landing mid-observation. That makes it tempting to treat as environmental. It is not: the test is observing a window the app does not guarantee, and the fix is to make the window deterministic (delay or gate the interfering response for the duration of the observation), never to loosen the assertion, add a retry, or widen a timeout until it usually passes.

**Rule:** Before claiming an E2E or integration spec is verified, run it at least three consecutive times and record each run's pass count and exit code in the report. If any two differ, the spec is not done. Close the race at its source — hold the interfering route until the observation completes — and prove the fix by re-running the same three times. Do not report a determinism claim you did not measure.

```javascript
// WRONG — the observation races the cache-invalidation remount; the aggregate
// exit code flips FAIL(7/8) / PASS(8/8) / FAIL(7/8) across three identical runs.
await clickConnect(page)
const enabledAfter = await pollUntilEnabled(nextButton, 40, 50)
check('Next enables after OAuth success', enabledAfter === true)

// RIGHT — hold the one response that can remount mid-observation, so the
// window the assertion needs is guaranteed rather than hoped for.
await armListDatasourcesDelay(page)   // stalls **/leptos-api/list_datasources*
try {
  await clickConnect(page)
  const enabledAfter = await pollUntilEnabled(nextButton, 40, 50)
  check('Next enables after OAuth success', enabledAfter === true)
} finally {
  await releaseListDatasourcesDelay(page)
}
```

Flagged in KYO-424 (`scripts/e2e-regression/datasource-create-oauth-contract.cjs`). Cycle 1 ran the spec three consecutive times against the same code and server and got FAIL(7/8), PASS(8/8), FAIL(7/8) — against a submitted claim of "verified on two consecutive runs, identical verdicts". Cycle 2 closed it by delaying the `list_datasources` response for the duration of Arm A's observation poll, so the KYO-429 remount could not land mid-assertion, then re-ran three times and recorded `8/8 passed, EXIT CODE: 0` on each. The assertion itself was never weakened.
