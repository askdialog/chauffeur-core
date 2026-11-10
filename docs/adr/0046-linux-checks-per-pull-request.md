# 0046. Pull requests are checked on Linux, macOS is checked at release

- **Date:** 2026-10-30
- **Status:** accepted

## Context

`main` requires the `Validate` check, and `Validate` ran on a macOS runner for every push of every pull request. It took 15 minutes (median) at 0.062 $ a minute, about 0.8 $ a run, and the first two days of CI (2026-10-28 and 2026-10-29) cost 113 $ in macOS minutes for 139 runs, 94 % of the CI bill.

What it caught did not need macOS: of its 18 real failures, 15 were `Check Rust` (format, clippy, tests) and 3 the sidecar's tests; on the 29 runs where the `Linux` job also finished, `Linux` saw every Rust failure `Validate` saw, and failed twice where `Validate` passed. Justin asked for the slow, costly check to leave each pull request and for the effort to go into how Chauffeur is released.

## Decision

The `Validate` check that `main` requires is a Linux check: it passes when the Linux jobs pass. The macOS checks run on release pull requests (`release/vX.Y.Z`), on `main` when its dependencies or toolchain change, and by hand. The release verifies the artifacts it publishes, not only the sources they were built from. Asked and decided by Justin, 2026-10-30.

## Consequences

The required check keeps its name (`Validate`), so `.github/main-ruleset.json` and the ruleset do not change; the up-to-date rule stays.

A regression that only macOS shows (code under `cfg(target_os = "macos")`, `e2e-cli.sh`, the packaged app) is no longer seen on its pull request but on the next release pull request, which cannot merge without it passing. How the jobs are arranged, what moves where and the order of the changes are in the plan of the change that applies this decision.

Before this change, [0036](0036-release-automation.md) relied on the version pull request having passed the macOS `Validate` on the exact tree; it still does, since a release pull request runs the macOS checks.
