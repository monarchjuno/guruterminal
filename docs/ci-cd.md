# CI and releases

Targets: macOS 13+ on Apple Silicon, Windows x64. Repository identity is `monarchjuno/guruterminal`.

This page is the release procedure. Product behavior is not defined here. Workflows and scripts in the repo are the source of truth for smoke details.

## GitHub

Require pull requests for `main`. Restrict `v*` tag creation, update, and deletion. Protect the `release`, `release-qualification`, and `stable-release` environments. If an environment uses selected deployment branches and tags, allow the `v*` **tag** pattern for each tag-triggered release job. Signing secrets live only in `release`. `stable-release` must require at least one reviewer, prevent self-review, and disallow administrator bypass, so the person who dispatches promotion cannot approve or force the publication. Enable immutable GitHub Releases.

Before every RC tag and stable promotion, run the checked-in, read-only audit
from an authenticated GitHub CLI session:

```sh
python3 apps/guruterminal/scripts/check-github-release-setup.py
```

It verifies the public repository identity, immutable releases, `main`
pull-request protection, an active `v*` tag rule that restricts creation,
updates, and deletion, each required protected environment, and the **names** of every
secret referenced by `release.yml`. Those names must be present only in the
`release` environment, never at repository scope or in either non-signing
environment. It never prints or reads secret values and does not change GitHub
state. A nonzero result names every missing item. The auditor intentionally
derives the required release-secret names from the workflow so a future signing
change cannot silently make the checklist stale.

The immutable-release endpoint requires repository administration read access.
Release workflows intentionally do not receive an administrative token, so the
maintainer's authenticated, read-only audit is the release gate for that setting
rather than adding a broader long-lived credential to every release job.

## CI

PRs and `main` run frontend, Rust, the finance and OpenBB Python sidecars,
compute, repository checks, native interaction, and target-platform package
smokes. Source checks split across Ubuntu (web, Python) and macOS (Rust) and do
not block native or package jobs. Repeat compiles restore a per-job Rust cache.
Pull requests that only touch documentation skip native and package jobs while
still reporting the required check names. Renderer-only pull requests skip
package smokes. `main` and `workflow_dispatch` always run the full set. Actions
are pinned to commit SHAs. Toolchain pins: Rust 1.97.1, uv 0.11.2, Syft 1.50.0.

The macOS native job stages the same pinned sidecars used by the package smoke
and drives an isolated native WebView through onboarding, Agent, Marketplace,
Memory, Chat lifecycle, accessibility, and restart-persistence flows. It uses
no provider or connector credentials; the explicitly authorized live Chat suite
is a pre-release acceptance step rather than CI input.

A release tag is accepted only from a `main` commit whose `ci.yml` push run succeeded. The tag workflow builds packages; it does not re-run that source gate.

Automatic updates exist only in signed macOS and Windows release builds.

## Release

Version is SemVer. Tags are `vX.Y.Z` or `vX.Y.Z-rc.N` and must match Cargo, npm, lockfiles, Tauri, and the base macOS plist version. Use the checked-in version helper rather than hand-editing those copies:

```sh
python3 apps/guruterminal/scripts/set-version.py --version X.Y.Z-rc.N
python3 apps/guruterminal/scripts/set-version.py --check --version X.Y.Z-rc.N
```

The command rejects an already-diverged tree before writing any file, updates every authored version and lockfile copy together, and keeps `Info.plist` at the SemVer base version for RCs. Use `--dry-run` to review a transition first.

Release runs use a non-cancelling serialized queue. `GITHUB_RUN_NUMBER` is a
lower bound for the macOS `CFBundleVersion`, and the release workflow allocates
the larger of that number and one more than every retained
`RELEASE-METADATA.json` build counter. The product SemVer remains the updater
version; the separate build counter is recorded in `RELEASE-METADATA.json` and
the qualification workflow compares it to the signed candidate DMG. This keeps
an RC, a stable promotion, and a retried run from reusing a macOS build number.

1. Set the product to `X.Y.Z-rc.N`, commit it, push it to `main`, and wait for the exact commit's CI run to pass. Create `vX.Y.Z-rc.N`; the workflow publishes that non-draft prerelease.
2. On clean machines, complete the acceptance flow in `GURU_TERMINAL.md`: ordinary Chat writes justified Wiki or Lens, and a later relevant turn uses it. A Memory write that never changes later work does not qualify.
3. After RC acceptance, set the product to `X.Y.Z`, ensure the matching
   `CHANGELOG.md` heading has its final ISO date (replace `Unreleased` if
   present), commit, push, and wait for that exact commit's CI run to pass.
   Create `vX.Y.Z`; its workflow requires the matching published RC and creates
   the stable **draft** candidate.
4. Qualify that stable draft: matching RC → stable for the first stable release,
   then current Latest → strictly newer stable on both platforms. Scripts:
   `verify-release-assets.py`, `serve-update-candidate.py`,
   `release-qualification.py`.
5. Promote the qualified stable draft with the `stable-release` workflow. Promotion does not build or upload files.

## Candidate update qualification

Run an in-app update qualification only on disposable, isolated macOS and Windows
test machines. Do not redirect `github.com` or trust a test certificate on a
developer's daily-use machine or on a production network.

1. Download the unchanged stable draft assets and run `verify-release-assets.py`
   with the candidate tag and source commit. Record the SHA-256 printed by
   `release-qualification.py digest --release-metadata RELEASE-METADATA.json`.
2. In each isolated test machine, arrange for only that machine's
   `https://github.com` traffic to resolve to its loopback address, and trust a
   disposable TLS certificate whose subject alternative name is `github.com`.
   The local candidate server binds to `127.0.0.1` and needs port 443 because the
   signed app has a canonical, portless GitHub updater endpoint.
3. Serve the verified candidate assets for the duration of the test:

   ```sh
   sudo python3 apps/guruterminal/scripts/serve-update-candidate.py \
     --assets /absolute/path/to/candidate-assets \
     --certificate /absolute/path/to/github-com-test-cert.pem \
     --private-key /absolute/path/to/github-com-test-key.pem
   ```

   On Windows, perform the equivalent from an elevated PowerShell (for example,
   `py -3 ...`); `sudo` above is macOS/Linux notation.

4. On a clean machine, install the published predecessor. The matching RC is
   valid only for the first stable release; afterward the predecessor must be
   the current Latest release and the candidate version must be strictly newer.
   Use the app's explicit update flow to install the candidate, restart it, and
   verify both the installed version and retained local data. Capture durable,
   credential-free HTTPS evidence for each platform.
5. On each clean signed candidate, complete the `GURU_TERMINAL.md` Marketplace,
   Chat, and Memory flow, including a later turn that uses written Memory and an
   explicit Wiki or Lens Revert. Capture durable credential-free HTTPS evidence
   that identifies the candidate tag and candidate-set digest; OAuth or
   connector credentials must never appear in that evidence.
6. Dispatch `release-qualification.yml` from the candidate `vX.Y.Z` tag (not
   `main`) with the two update-evidence URLs, the two product-acceptance URLs,
   the identical candidate-set digest for both platforms, and all four
   confirmation flags. Its receipt seals the exact tested asset set; use its
   successful run ID to dispatch `promote-release.yml` from that same candidate
   tag.

Sites links to `releases/latest/download/GuruTerminal-macOS-arm64.dmg` and `GuruTerminal-Windows-x64.exe`. The app feed is `https://github.com/monarchjuno/guruterminal/releases/latest/download/latest.json`. Install and restart need explicit user confirmation.

A bad release is removed from the stable feed and replaced by a higher patch. No storage downgrade or silent rollback. V1 sends no telemetry.
