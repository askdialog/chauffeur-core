# Core CI checks the changed surface and packages Linux on a separate schedule

- **Date:** 2026-11-10
- **Status:** accepted

## Context

Justin asked to simplify and shorten CI while preparing Core's MIT publication. A recent Core
run took 6m50s of execution, including 5m13s in Rust and 29s in JavaScript checks. The Linux
desktop job took 10m30s. UI-only pull requests still ran Rust; every main push built a package.

## Decision

Keep the required `check` status. Repository checks run first, then independent Rust and
JavaScript jobs run in parallel when needed. Main and manual runs check both. A pull request
limited to documentation skips both; one limited to JavaScript skips Rust. Protocol bindings,
native app code, sidecar inputs consumed by Rust tests, build files and unknown paths run both.
Change detection uses the PR's merge-base diff and includes deleted and renamed source paths.
The final status fails if detection or any selected check fails, is cancelled or unexpectedly skips.

Keep the existing Rust and JavaScript check coverage and main-only cache writes. Rust retains
Bun because lifecycle tests execute TypeScript sidecar stand-ins. Formatting, Clippy and tests
have separate steps so timings and failures are visible.

Build and smoke-test the Linux package on packaging/native-app/dependency changes, nightly
on main, and on manual dispatch. Ordinary Rust edits still run Rust checks; the next nightly
package checks installation. The distribution's release validation remains unchanged.

## Consequences

UI-only pull requests no longer wait for Rust compilation and lifecycle tests. Full Rust runs
retain their existing tests and therefore their cost. New paths default to full validation rather
than silently receiving no checks. Python tests cover selection and the required status gate.
The existing branch-protection status does not need renaming.
