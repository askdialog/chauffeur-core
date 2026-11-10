# A worker installs the toolchains each repo declares

- **Date:** 2026-11-02
- **Status:** accepted

## Context

A workspace sent to a GCP worker ran its repos' setup scripts on a VM with no node, pnpm or uv: a web repo's `pnpm install --frozen-lockfile` failed with `pnpm: command not found` on the first real send (2026-10-31), leaving it without dependencies there. Installing one team's tools in the worker is ruled out: outside the Office, Chauffeur names no team's defaults. Justin chose between a setup script of the worker's own declared by each repo and the versions a repo already declares.

## Decision

Decided with Justin 2026-11-02: on a GCP worker, before each repo's setup script, Chauffeur installs the toolchain versions the repo declares: the tools of `.tool-versions` and mise's configuration, node from `.nvmrc` or `.node-version`, python from `.python-version`, uv for a `uv.lock`, and the pnpm or yarn `packageManager` of `package.json`.

## Consequences

- `worker_install.sh` installs mise from its signed apt repository, turns on its idiomatic version files for node and python, installs uv, and puts mise's shims first on the login shell's `PATH`, which the daemon, the setup scripts, terminals and agents get.
- The Mac's driver holds the setup scripts at activation on a worker it created (`toolchains` in `cloud.transfer.destination_activate`) and asks `cloud.transfer.destination_setup` once the transfer's slot is free: the worker runs `mise trust` for a mise configuration, `mise install`, then `corepack enable`, `corepack install` and `mise reshim` for a `packageManager`, and starts the setup scripts. The peer protocol goes to version 2.
- What was installed is a line of the transfer's log; a failure is a warning naming the repo, and its setup script runs all the same. The Mac, machines declared by hand and the way back install nothing.
- What a repo declares nothing for is not installed: a repo that relies on tools it does not name still needs them on the worker.
