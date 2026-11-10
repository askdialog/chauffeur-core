# Use Apple's system cryptography in the iOS companion

- **Date:** 2026-11-07
- **Status:** accepted

## Context

The first iOS TestFlight upload reached Apple's encryption-documentation step.
Chauffeur bundled JavaScript implementations of Noise's cryptographic primitives
and libp2p identity signatures. Justin asked to try CryptoKit, then approved
integrating the successful interoperability prototype into the native Expo app.
The intended distribution remains TestFlight and the public stores.

## Decision

Use Apple's system CryptoKit for iOS key agreement, key derivation, authenticated
encryption, hashes and Ed25519 identity generation/signing/verification. Keep the
existing Noise protocol, QR secret, pinned identities, reconnect profile and
opaque relay. Android and the Mac retain their existing implementations.

The mobile repository owns the Expo module and iOS-specific dependency resolution.
No vendored Core sources or installed dependencies are patched. The iOS build must
reject bundled alternative cryptographic implementations rather than silently
falling back to them. Saved Ed25519 identities remain compatible.

## Consequences

A new native build is required. Test the actual module against the existing Mac,
including pairing, reconnect, revocation and saved identities on physical iPhones.
Audit both the JavaScript bundle and native archive. Passing a macOS CryptoKit
interop test does not prove iPhone behaviour, legal exemption or Apple acceptance.
Only update store declarations based on the audited app that is actually shipped.
