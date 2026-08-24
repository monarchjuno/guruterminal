# CI and releases

Targets: macOS 13+ on Apple Silicon, Windows x64. Repository identity is `monarchjuno/guruterminal`.

This page is the release procedure. Product behavior is not defined here. Workflows and scripts in the repo are the source of truth for smoke details.

## GitHub

Protect `main` and `v*` tags. Protect the `release`, `release-qualification`, and `stable-release` environments. Signing secrets live only in `release`. Final publication has a different reviewer in `stable-release`. Enable immutable GitHub Releases.

## CI

PRs and `main` run frontend, Rust, the finance and OpenBB Python sidecars,
compute, repository checks, and signed-target package smokes. Actions are pinned
to commit SHAs. Toolchain pins: Rust 1.97.1, uv 0.11.2, Syft 1.50.0.

A release tag is accepted only from a `main` commit whose `ci.yml` push run succeeded. The tag workflow builds packages; it does not re-run that source gate.

Automatic updates exist only in signed macOS and Windows release builds.

## Release

Version is SemVer. Tags are `vX.Y.Z` or `vX.Y.Z-rc.N` and must match Cargo, npm, lockfiles, and Tauri.

1. Publish a non-draft `vX.Y.Z-rc.N`.
2. On clean machines, complete the acceptance flow in `GURU_TERMINAL.md`: ordinary Chat writes justified Wiki or Lens, and a later relevant turn uses it. A Memory write that never changes later work does not qualify.
3. Qualify the signed candidate (N-1 → N update on both platforms). Scripts: `verify-release-assets.py`, `serve-update-candidate.py`, `release-qualification.py`.
4. Promote that existing draft with the `stable-release` workflow. Promotion does not build or upload files.

Sites links to `releases/latest/download/GuruTerminal-macOS-arm64.dmg` and `GuruTerminal-Windows-x64.exe`. The app feed is `https://github.com/monarchjuno/guruterminal/releases/latest/download/latest.json`. Install and restart need explicit user confirmation.

A bad release is removed from the stable feed and replaced by a higher patch. No storage downgrade or silent rollback. V1 sends no telemetry.
