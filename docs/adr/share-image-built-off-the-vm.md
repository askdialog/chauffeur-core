# The share service's image is built off the relay's VM

- **Date:** 2026-11-03
- **Status:** accepted

## Context

The relay VM (an e2-small) built the share service's image itself at each deploy: a quarter of an hour of `cargo build` with 2 GB of swap, and a reset of the VM that cut the relay. Each build left its 1.8 GB Rust stage behind; on 2026-11-03 the 10 GB disk was full, the build of the workspace share routes failed, and sharing a workspace answered 404 until the disk was resized by hand.

## Decision

Asked by Justin 2026-11-03: Cloud Build builds the share service's image, in the project's region, into an Artifact Registry repository of the project; the relay VM pulls it by digest and builds nothing. `deploy/gcp.sh` applies it by running the VM's startup script again over IAP SSH, without restarting the VM.

Asked by Justin the same day: the office server's VM does the same (its image in the same repository), and the repository keeps the five newest images of each service.

## Consequences

- A deploy builds in a few minutes on an e2-highcpu-8 and takes the VM about a minute; the relay is no longer down for it, Caddy restarts for a moment.
- The VM needs no Rust toolchain, build stages nor swap; its service account reads the repository (`roles/artifactregistry.reader`). The image is tagged by date and commit; the metadata names the digest, which the startup script checks.
- Deploying needs IAP SSH to the VM and Cloud Build in the project; the sources Cloud Build reads are staged in the relay's artifacts bucket (`cloud-build/`).
- The Office's VM builds nothing either; `scripts/gcp-registry.sh` makes the repository, sets its cleanup policy (keep the five newest of each image, delete the rest, applied about daily) and lets each VM's service account read it.
