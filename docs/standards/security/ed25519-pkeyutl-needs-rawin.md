# Use raw input mode for Ed25519 pkeyutl signatures

**Rule:** Every `openssl pkeyutl -sign` or `-verify` invocation using an Ed25519 key must pass `-rawin`. Test the command with the OpenSSL version used by CI, and test that removing `-rawin` fails. A command that works with one OpenSSL version is not enough evidence for another.

Without `-rawin`, OpenSSL 3.0.x can reject Ed25519 signing or verification even when the key and input are valid. A signer that fails may silently produce an empty signature if a pipeline only checks the final `base64` command.

WRONG:

```sh
openssl pkeyutl -sign -inkey "$KEY_FILE" -in "$HASH_FILE"
openssl pkeyutl -verify -pubin -inkey "$PUBLIC_KEY" -in "$HASH_FILE" -sigfile "$SIGNATURE_FILE"
```

RIGHT:

```sh
openssl pkeyutl -sign -rawin -inkey "$KEY_FILE" -in "$HASH_FILE"
openssl pkeyutl -verify -rawin -pubin -inkey "$PUBLIC_KEY" -in "$HASH_FILE" -sigfile "$SIGNATURE_FILE"
```

KYO-712 fixed this in `scripts/sign-review.sh`. The 2026-09-23 review log for KYO-747 records the same failure in `scripts/sign-verification.sh`, including a mutation test that made the suite fail when `-rawin` was removed. Its review also identified a separate verification call in `~/.local/bin/gh` that needs the same check. See `scripts/sign-review.sh` for the OpenSSL version evidence and the adjacent explanation of this flag.
