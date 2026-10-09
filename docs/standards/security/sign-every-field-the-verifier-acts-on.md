# Sign every field the verifier acts on, not just the one that names the change

A signature authorises exactly the bytes that were fed to the signing key. Every other byte
in the artefact the verifier reads afterwards is unauthenticated input — including bytes
that make the check *weaker*.

The design that produces the gap is the sensible-looking one. You sign the value that
identifies what was approved: a diff hash, an artefact digest, a manifest checksum. Later,
someone adds an escape hatch beside it — an override reason, an exemption flag, an expiry,
an allowlist entry — and adds it as a *new, additive field* precisely so that older
verifiers and older artefacts keep working. Additive-to-the-file is the right instinct for
compatibility and the wrong one for authentication: the signed payload does not grow with
the file, so the one field an attacker actually wants is the one field nobody has to forge
a signature for. They append it by hand, in a text editor, with no key.

Three things keep it invisible:

- **The artefact still verifies.** The signature over the identifying value is genuine, so
  every existing check passes and the file looks exactly as trustworthy as it did before.
- **The escape hatch is added in a different change than the signing scheme**, usually by
  someone reasoning about file format compatibility rather than about what is
  authenticated. Nothing in that framing raises the question.
- **The test suite cannot catch it**, because it signs and verifies through the same code
  path. Sign-then-verify round-trips are green for both the honest and the forged file; the
  only input that separates them is one nobody writes by accident — a genuinely signed
  artefact with the field appended afterwards.

**Rule:** Before shipping a verifier, enumerate every field it reads and branches on. Each
one is either inside the signed payload or it is attacker-controlled — there is no third
category, and "it's only a reason string" is not one either if the verifier's behaviour
changes when it is present. Any field that *relaxes* the check must be in the payload.
Preserve backward compatibility by deriving the payload's shape from the artefact's own
contents on both sides — sign `<identity>` alone when the optional field is absent, sign
`<identity>` plus the field when it is present, and have the verifier reconstruct the same
value from whatever the file actually holds — rather than by leaving the field out of the
payload. Then prove it with a forgery test: take a validly signed artefact, hand-append the
field with no private key, and assert the verifier rejects it and never reports the relaxed
outcome.

```sh
# WRONG — a reconstruction. This form was corrected during review and never
# committed, so there is no `<sha>^` to quote; it is reassembled from the
# review finding and from the shipped header comment that describes it
# ("A first version of this recorded line 3 as plain, unsigned text: the
# Ed25519 signature covered only the diff hash on line 1, never line 3").
# The defect is the gap between what is hashed into $HASH_FILE and what is
# written to the file: line 3 is emitted but never signed, so appending it
# afterwards costs nothing.
echo -n "$DIFF_HASH" > "$HASH_FILE"
SIGNATURE=$(openssl pkeyutl -sign -inkey "$KEY_FILE" -in "$HASH_FILE" | base64 -w 0)
{
    printf '%s\n' "$DIFF_HASH"
    printf '%s\n' "$SIGNATURE"
    if [ "$ALLOW_UNSTAGED" -eq 1 ]; then
        printf 'ALLOW-UNSTAGED:%s\n' "$ALLOW_UNSTAGED_REASON"
    fi
} > .review-approval

# RIGHT — quoted verbatim from scripts/sign-review.sh on `main` today
# (landed as `abe205f8`, PR #516, merged 2026-09-12); its preceding comment
# is elided. The `{ ... } > .review-approval` block on `main` is
# byte-identical to the block shown above, which is the point: nothing
# about how the file is written had to change, only what goes into
# $HASH_FILE. The `SIGNATURE=` line above is the WRONG block's
# reconstruction, not a quote of `main` — `main`'s reads
# `openssl pkeyutl -sign -rawin -inkey "$KEY_FILE" -in "$HASH_FILE" | base64 -w 0`.
# `-rawin` was added by this same PR (`git log -S"-rawin" --
# scripts/sign-review.sh` finds no earlier introduction) to make Ed25519
# signing work on OpenSSL 3.0.x; it governs how the bytes in $HASH_FILE get
# signed, not which bytes end up in it, so it is orthogonal to this rule
# and irrelevant to the WRONG/RIGHT contrast below.
if [ "$ALLOW_UNSTAGED" -eq 1 ]; then
    printf '%s\nALLOW-UNSTAGED:%s' "$DIFF_HASH" "$ALLOW_UNSTAGED_REASON" > "$HASH_FILE"
else
    echo -n "$DIFF_HASH" > "$HASH_FILE"
fi

# RIGHT (verification half) — .githooks/pre-commit on the same branch.
# It rebuilds the signed value from the file's own contents, so a line 3 that
# was appended after signing changes what gets verified and fails, while a
# file that never had one verifies exactly as before.
    if [ -n "$line3" ]; then
        signed_value="$(printf '%s\n%s' "$hash" "$line3")"
    else
        signed_value="$hash"
    fi
```

Precedent — **KYO-712**, *"bind review signature to the diff it approved"*, `2026-09-11`
review log, the **initial** entry (2 🔴). Finding #2 is this rule, and the reviewer
reproduced it rather than reasoning about it: signed cleanly with no `--allow-unstaged`,
added an untracked trigger file, hand-appended the `ALLOW-UNSTAGED:` line, and watched the
real hook print *"skipped for unstaged/untracked files — acknowledged at signing"* for a
file *"that was never reviewed and never actually acknowledged by the reviewer."* The
cycle-2 entry records the fix and, more usefully, the test that now pins it — a case that
hand-appends a forged line to a validly-signed approval with no key and asserts both that
the commit is blocked and that the "acknowledged at signing" string is never printed.
Cycle 1's own review notes that the pre-existing suite *"honestly discloses that it does
not test `.githooks/pre-commit`'s Check 2/2b logic — which is exactly where findings 1 and
3 above live"*: the round-trip tests that did exist could not have found this.

Distinct from [unused-security-helper-worse-than-none.md](unused-security-helper-worse-than-none.md):
there the control has no caller at all, and the remedy is to grep for production callers.
Here the control is called, on every commit, and returns the right answer about the input
it was given — the input is just smaller than the decision.

Distinct from [prove-a-flagged-secret-is-fake-before-suppressing-the-scanner.md](prove-a-flagged-secret-is-fake-before-suppressing-the-scanner.md),
the other rule in this section about an escape hatch in a security gate: that one is about
*justifying and scoping* a suppression a human deliberately adds. This one is about an
escape hatch that is legitimate by design and merely unauthenticated, so no amount of
justifying it helps — the fix is cryptographic, not editorial.

Distinct from [../error-handling/empty-on-failure-must-not-look-like-a-real-result.md](../error-handling/empty-on-failure-must-not-look-like-a-real-result.md):
that rule is about a check that fails and reports success. This check does not fail; it
succeeds over a payload that omits the field the decision turned on.

See also [../comments-documentation/no-guarantee-stronger-than-code-enforces.md](../comments-documentation/no-guarantee-stronger-than-code-enforces.md)
and [../error-handling/a-check-in-one-arm-does-not-guard-the-others.md](../error-handling/a-check-in-one-arm-does-not-guard-the-others.md).
The same review's other 🔴 was the branch-coverage failure that second rule names, and its
header comment stated the guarantee the code did not keep — three failures of the same
gate, on three different axes, in one diff. When a change touches a gate, check the payload
*and* the branch *and* the prose; clearing one says nothing about the others.
