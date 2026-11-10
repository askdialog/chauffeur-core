# A transfer updates a worker that runs another Chauffeur version

- **Date:** 2026-11-02
- **Status:** accepted

## Context

A transfer needs every daemon on one Chauffeur version. A stopped worker is installed again when it starts, but a running one kept the version it was installed with: once the Mac updated, a workspace on that worker could neither come back nor be archived ("The source runs Chauffeur 0.27.0 and this one 0.28.0; update them to the same version"), and Chauffeur offered no way to update it. Justin asked that this be done automatically, so that it does not happen again.

## Decision

Decided with Justin 2026-11-02: when a transfer finds a ready GCP worker of this Mac on another Chauffeur version, as its source or its destination, it updates that worker first, then goes on. Workers are not updated when the Mac updates.

## Consequences

- The update copies the Linux bundle of the Mac's version and runs `worker_install.sh` again, as starting a stopped worker does, then waits for the worker's daemon to answer on that version (up to two minutes). The transfer's start waits meanwhile; a send to Cloud says so in its log.
- The installation restarts the worker's daemon: the agents working on it are interrupted, in every workspace it runs, not only the one transferred. Nothing outside a transfer interrupts them.
- A failed update refuses the transfer with its error; the worker stays ready, on its old version, and the next transfer tries again.
- A machine declared by hand in `[transfer]` is not a worker of this Mac: it still has to be updated by its owner, and a transfer to or from it is still refused while versions differ.
