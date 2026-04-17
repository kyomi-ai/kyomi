# Releasing Kyomi to production

Production deploys are triggered by pushing a version tag from this repo.
Pushing to `main` does not deploy anything.

## How a release flows

```
git tag v2026.04.17.1 && git push --tags
  │
  ├── release.yml (public repo)
  │     → builds standalone binaries + GHCR Docker image
  │       for self-hosted users
  │
  └── notify-private.yml (public repo)
        → repository_dispatch to kyomi-ai/kyomi-private
          → sync-submodule.yml (private repo)
             → bumps kyomi submodule pointer to the tagged commit
             → dispatches deploy-api.yml
               → builds prod image on self-hosted runner
               → pushes to local registry
               → kubectl set image → k8s rollout
```

`ci.yml` (fast cargo check, clippy, lint) runs on PRs targeting `main`.
Merges to `main` themselves do not run any deploy workflow, and the
submodule pointer in `kyomi-private` does not move until a tag is pushed.

## Tag format

`v{YYYY}.{MM}.{DD}.{N}` where `N` is the release number for that day
(1-based). Examples:

- `v2026.04.17.1` — first release of 2026-04-17
- `v2026.04.17.2` — second release the same day

## Pre-release checklist

1. Confirm the PRs you want in this release are merged to main.
2. Confirm CI is green on main.
3. `git fetch origin && git checkout main && git pull --ff-only`.
4. Tag: `git tag v$(date +%Y.%m.%d).1` (increment `.1` → `.2` if there's
   already a release today).
5. `git push --tags`.

## Monitoring

- Public repo → Actions → watch `Release` and `Notify Private Repo`.
- Private repo → Actions → watch `Sync Submodule` and `Deploy API`.
- Smoke check: `curl -fsS https://app.kyomi.ai/api/v1/health`.

## Rollback

Pods run an image tagged with the short SHA of the release, so the k8s
deployment retains the previous ReplicaSet for one-shot rollback:

```bash
kubectl -n kyomi rollout undo deployment/kyomi-api
```

Or pin explicitly to a known-good short-SHA:

```bash
kubectl -n kyomi set image deployment/kyomi-api \
  kyomi-api=$REGISTRY/kyomi-api:<short-sha>
kubectl -n kyomi rollout status deployment/kyomi-api
```

## Hotfix / out-of-band releases

The process is the same — cut a tag. Keep the tag in date-number form;
don't use semver suffixes like `-hotfix`. If a release needs to ship
without a prior merge to main (rare), branch from the previous tag,
apply the fix, cherry-pick it back to main afterwards, and tag the
branch:

```bash
git checkout -b hotfix/v2026.04.17.2 v2026.04.17.1
# apply fix, commit
git tag v2026.04.17.2 && git push --tags
# cherry-pick back to main so main doesn't regress
git checkout main && git cherry-pick <fix-sha> && git push
```
