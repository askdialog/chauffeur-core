# The credential vault and the relay-hosted controller are removed

- **Date:** 2026-10-31
- **Status:** accepted

## Context

The cloud scope ([0027](0027-cloud-workspace-handoff-scope.md)) planned a persistent encrypted credential vault: native logins stored end to end in GCS, refreshed on a worker's behalf by scoped writers, delivered to enrolled workers, with a controller hosted on the relay VM. It was built (`packages/cloud`, the daemon's vault, refresh, native-session and controller-profile RPCs, Settings → Cloud, the relay's provisioned channels) and the controller installed on the relay VM on 2026-10-20. The shipped transfer path never used it: transfers run their own local controller ([0030](0030-transfers.md)), and a worker's agents keep their own login, signed in from the Mac. Justin asked for its removal.

## Decision

Decided with Justin 2026-10-31: the credential vault, its refresh delegation, worker enrollment and bootstrap delivery, Secret Service custody on the worker, the native account review and Codex cloud login, the control channel and the relay-hosted TypeScript controller are removed, with the Settings → Cloud section that served them only. What transfers use stays: the daemon's protected endpoint root and its database of pins and handoff state. A controller hosted on the relay VM, if one is ever wanted for transfers, will be built anew.

## Consequences

- `packages/cloud` and the `chauffeur-cloud-owner` binary go, with their end-to-end scripts and the documents describing them. The `cloud.controller.*`, `cloud.accounts.*` and `cloud.native.*` client methods and the vault, refresh, runtime and native peer methods go; `chauffeur cloud status|initialize|configure` too. The relay no longer reads `RELAY_CHANNELS_FILE`.
- The daemon's `cloud-vault-heads.sqlite` keeps its path and the tables transfers use (the endpoint root's pin, the handoff journal, the archive preparation keys); opening it drops the vault's tables. The endpoint root is no longer readable over any RPC.
- The controller service installed on the relay VM on 2026-10-20, and its state there, are left running: they have to be stopped and removed by hand. Nothing in the repository deploys or updates them any more.
- The approved scope's lines about a credential vault ([cloud-workspaces.md](../cloud-workspaces.md)) no longer describe a plan: workers keep their own logins.
