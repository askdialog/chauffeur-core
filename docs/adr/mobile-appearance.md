# Mobile appearance follows the system or an explicit preference

- **Date:** 2026-11-08
- **Status:** accepted

## Context

Justin explicitly approved a mobile Appearance preference on 2026-11-08:
System by default, with Light and Dark overrides remembered on the phone.
Changing appearance must preserve the user's ongoing work and review session.

## Decision

Connection preferences expose Appearance before and after pairing, with System
(default), Light and Dark choices. Selection applies immediately and survives app
restarts. System follows subsequent operating-system appearance changes. The
resolved appearance applies reactively to native companion screens, navigation,
system chrome and keyboards where the platform supports it.

Changing appearance must not remount screens, discard drafts or model selections,
reset navigation, or replace a hosted review's WebView or session. App palettes
and styles are immutable and update reactively. Only the appearance preference is
remembered locally, separately from paired-Mac identities and conversations. A
stale startup read cannot overwrite a newer explicit choice. A failed save tells
the user to select again while leaving the interface usable.

Canvas and preview documents retain their own colors: the companion does not
force-darken them or inject theme CSS. Native review chrome follows the selected
appearance; the hosted review service owns its website theme and session.

## Consequences

The companion's native Expo configuration supports automatic appearance and
includes expo-system-ui. These native configuration and dependency changes need
a new native build. JavaScript exports and mocked tests do not establish device
keyboard, dialog, safe-area or release behavior; physical iPhone and Android
release checks remain required before distribution.

The preference belongs to the phone. It adds no daemon setting, wire contract,
conversation storage or pairing change. This decision does not authorize a paid
EAS build, store submission or signing-credential change.
