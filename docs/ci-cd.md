# CI and releases

Targets: macOS 13+ on Apple Silicon, Windows x64. Repository identity is `monarchjuno/guruterminal`.

This page is the release procedure. Product behavior is not defined here. Workflows and scripts in the repo are the source of truth for smoke details.

## GitHub

Protect `main` and `v*` tags. Protect the `release`, `release-qualification`, and `stable-release` environments. Signing secrets live only in `release`. Final publication has a different reviewer in `stable-release`. Enable immutable GitHub Releases.

## CI

PRs and `main` run frontend, Rust, the finance and OpenBB Python sidecars,
compute, repository checks, and target-platform package smokes. Actions are pinned
to commit SHAs. Toolchain pins: Rust 1.97.1, uv 0.11.2, Syft 1.50.0.

The macOS CI job also stages the same pinned sidecars used by the package smoke
and drives an isolated native WebView through onboarding, Agent, Marketplace,
Memory, Chat lifecycle, accessibility, and restart-persistence flows. It uses
no provider or connector credentials; the explicitly authorized live Chat suite
is a pre-release acceptance step rather than CI input.

A release tag is accepted only from a `main` commit whose `ci.yml` push run succeeded. The tag workflow builds packages; it does not re-run that source gate.

Automatic updates exist only in signed macOS and Windows release builds.

## Release

Version is SemVer. Tags are `vX.Y.Z` or `vX.Y.Z-rc.N` and must match Cargo, npm, lockfiles, and Tauri.

Release runs use a non-cancelling serialized queue. `GITHUB_RUN_NUMBER` is a
lower bound for the macOS `CFBundleVersion`, and the release workflow allocates
the larger of that number and one more than every retained
`RELEASE-METADATA.json` build counter. The product SemVer remains the updater
version; the separate build counter is recorded in `RELEASE-METADATA.json` and
the qualification workflow compares it to the signed candidate DMG. This keeps
an RC, a stable promotion, and a retried run from reusing a macOS build number.

1. Publish a non-draft `vX.Y.Z-rc.N`.
2. On clean machines, complete the acceptance flow in `GURU_TERMINAL.md`: ordinary Chat writes justified Wiki or Lens, and a later relevant turn uses it. A Memory write that never changes later work does not qualify.
3. Qualify the signed candidate (N-1 → N update on both platforms). Scripts: `verify-release-assets.py`, `serve-update-candidate.py`, `release-qualification.py`.
4. Promote that existing draft with the `stable-release` workflow. Promotion does not build or upload files.

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

4. On a clean machine, install the published predecessor (the matching RC for a
   first stable release is valid), use the app's explicit update flow to install
   the candidate, restart it, and verify both the installed version and retained
   local data. Capture durable, credential-free HTTPS evidence for each platform.
5. Dispatch `release-qualification.yml` with both evidence URLs, the identical
   candidate-set digest for both platforms, and the two confirmation flags. Its
   receipt seals the exact tested asset set; use its successful run ID when
   dispatching `promote-release.yml`.

Sites links to `releases/latest/download/GuruTerminal-macOS-arm64.dmg` and `GuruTerminal-Windows-x64.exe`. The app feed is `https://github.com/monarchjuno/guruterminal/releases/latest/download/latest.json`. Install and restart need explicit user confirmation.

A bad release is removed from the stable feed and replaced by a higher patch. No storage downgrade or silent rollback. V1 sends no telemetry.
