---
title: Releases
description: Build, sign, and publish Yapr.app with the manual Release workflow.
---

# Releases

Releases use one manual `Release` workflow, a main-only `release` environment, immutable exact tags, and a movable major alias. Each GitHub Release carries `Yapr-X.Y.Z.zip`: the app signed with the self-signed "Yapr Release Signing" certificate. There is no paid Apple account, so releases are not notarized and users must approve the first launch (the release notes explain how). The stable certificate keeps users' microphone, Accessibility, and Keychain grants across updates.

The maintainer starts each operation. Dry runs and explicit confirmation guard against unintended publication. The workflow uses only `GITHUB_TOKEN`; there is no second publish path.

## Trunk and tag policy

- All changes land on `main` first. `main` is the only source of new work.
- Release branches (`release/vX.Y`) receive only squash backports of commits that already exist on `main`.
- Never merge a release branch back into `main`. Fix forward on `main` and backport the fix.
- Never delete a published release or its tag. Correct a bad release by publishing a new patch version.
- Exact version tags (`X.Y.Z`, `X.Y.Z-rc.N`) are immutable.
- The major alias tag (`X`) is movable. It points at the highest stable release of that major line.

Versions are canonical SemVer with no leading `v`: `1.0.0` or `1.0.0-rc.1`. The version lives in `app/Cargo.toml`; `draft` sets it on the release branch and the app bundle reads it from there.

## One-time repository setup

1. Create a `release` environment with no required reviewers, administrator bypass disabled, and a sole custom deployment branch policy of `main`.
2. Run `scripts/make-release-cert.sh` once. It creates the signing certificate in `~/.config/yapr-release` and stores it in the `release` environment as `RELEASE_SIGNING_P12_BASE64` and `RELEASE_SIGNING_P12_PASSWORD`. Back that folder up. A new certificate makes macOS ask every user for permissions again.
3. Enable auto-merge and squash merge. Allow GitHub Actions to create pull requests so `backport` can open them.
4. Enable Immutable Releases so published release tags cannot be moved or deleted.
5. Keep branch rules compatible with workflow-created version commits and branch deletion. Do not create a ruleset that targets tags.
6. Do not pre-create release branches or tags. The workflow creates every line it owns.

## Operations

Every operation accepts `dry_run`, which defaults to `true`. A dry run plans the work and writes artifacts without changing the repository or building. Always pass `--ref main`.

```bash
gh workflow run Release --ref main \
  -f operation=cut \
  -f version=0.1.0 \
  -f dry_run=true
```

Read the plan in the run summary, then re-run with `-f dry_run=false`. Dispatch one operation at a time and wait for it.

| Operation | Required fields | Effect |
| --- | --- | --- |
| `cut` | `version` (patch `0`) | Creates `release/vX.Y` from `main`. |
| `backport` | `release_line`, `commits` | Cherry-picks main SHAs and opens a squash PR into the line. |
| `draft` | `release_line`, `version` | Sets the Cargo version, builds and signs `Yapr-X.Y.Z.zip`, then pushes the version commit, tags, and creates a draft GitHub Release with the zip. Nothing is pushed if the build fails. |
| `publish` | `version`, `confirmation='publish <version>'` | Publishes the draft and moves major alias `X` for the highest stable of that major. |
| `cancel` | `version`, `confirmation='cancel <version>'` | Deletes an unpublished draft and its exact tag. |
| `retire` | `release_line`, `confirmation='retire <release_line>'` | Deletes a release branch. Published tags stay. |
| `restore` | `release_line` | Recreates a retired line at its highest stable tag. |

A first release:

```bash
gh workflow run Release --ref main -f operation=cut -f version=0.1.0 -f dry_run=false
gh workflow run Release --ref main -f operation=draft -f release_line=release/v0.1 -f version=0.1.0 -f dry_run=false
gh workflow run Release --ref main -f operation=publish -f version=0.1.0 -f confirmation='publish 0.1.0' -f dry_run=false
```

Download the zip from the draft and test it before `publish`.

## Local packaging

`scripts/package-release.sh VERSION OUTPUT.zip` is what `draft` runs. It signs with `Yapr Release Signing` unless `SIGN_IDENTITY` names another identity, and fails if the app ends up signed by anything else.
