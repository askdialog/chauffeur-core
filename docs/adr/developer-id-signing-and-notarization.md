# The macOS app can be signed with a Developer ID and notarized

- **Date:** 2026-11-05
- **Status:** accepted

## Context

The disk image `scripts/package.sh` builds was signed ad hoc, for want of an Apple Developer account ([0001](0001-v1-scope.md)): macOS warns before opening a downloaded copy, and the user approves it in System Settings. An ad-hoc signature is also tied to the exact build, so the permissions macOS grants the app (camera and microphone, folders, automation) do not carry over to the next version. Dialog now has a Developer ID certificate and an App Store Connect API key, and Justin wants a disk image anyone can open without a warning.

## Decision

Asked by Justin, 2026-11-05:

- Core's packaging can sign the app with a Developer ID, under the hardened runtime and with a secure timestamp, and have Apple notarize the app and the disk image, stapling each ticket.
- Every value comes from the build's environment: `CHAUFFEUR_CODESIGN_IDENTITY`, then `CHAUFFEUR_NOTARY_KEY` (the API key's `.p8`), `CHAUFFEUR_NOTARY_KEY_ID` and, for a Team key, `CHAUFFEUR_NOTARY_ISSUER`. Unset, the build signs ad hoc as before. Core names no identity, team or key; a distribution supplies its own ([core-open-source-without-dialog-defaults](core-open-source-without-dialog-defaults.md)).
- Each program gets only the entitlements it needs (`scripts/entitlements/`): the app the camera, the microphone and Apple events; the daemon Apple events; the Bun-compiled sidecar and mobile host the JIT.

## Consequences

- `scripts/sign-macos.sh` signs inside out (the nested programs, then the app, then the disk image) and refuses an incomplete configuration before the build starts. A notarization Apple refuses fails the build and prints Apple's log.
- The checksum is computed after stapling; the updater (story 50) installs a signed app exactly as it installs an ad-hoc one.
- A Developer ID signature gives the app a stable identity: permissions macOS grants survive updates. Moving an installed ad-hoc app to a signed one asks for them once more. Keychain items are unaffected: Chauffeur reads and writes them through `/usr/bin/security`, whose identity does not change.
- Anything packed inside the app that Apple's notary service inspects must be signed the same way.
