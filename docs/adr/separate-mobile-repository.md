# The iOS and Android companion has its own source repository

- **Date:** 2026-11-05
- **Status:** accepted

## Context

The companion was initially developed beside Core, Android first. Preparing iOS
and TestFlight gives the phone app its own signing, build numbers and delivery
lifecycle, while the daemon and Mac host keep their existing responsibilities.

## Decision

Decided with Justin 2026-11-05: extract the iOS and Android app together into the
`chauffeur-mobile` repository, starting with a fresh source
snapshot rather than importing Core's history. Preserve the existing Expo project,
application identifiers and mobile version. The preparation already done for iOS
moves with the application.

Core owns the shared client contracts. Mobile consumes a verified, commit-pinned
SDK snapshot containing protocol types, transport/client lifecycle and pure shared
conversation helpers; its builds do not require a sibling Core checkout or
cross-repository CI credentials.

## Consequences

Core retains the daemon, Mac mobile host, opaque relay, shared transport and their
integration tests. The app owns its React Native interface, store metadata,
platform configuration, CI and EAS delivery. The desktop distribution continues to
release independently; this extraction does not change the frozen 0.31.0 source.

Only the original colocated source choice is replaced; pairing, authorization,
end-to-end encryption and daemon ownership from ADRs 0023 and 0028 are unchanged.
No Apple app, signing credential, TestFlight build or store publication is created
by moving the source.
