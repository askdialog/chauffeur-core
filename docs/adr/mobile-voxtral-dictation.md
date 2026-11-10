# Mobile dictation through Voxtral and the GCP relay

- **Date:** 2026-11-08
- **Status:** accepted

## Context

Louis requested speech-to-text in the mobile composer. Dialog's widget already
uses Voxtral realtime tokens. Justin asked to reuse that approach and confirmed
that Chauffeur's service and dedicated Mistral key must stay on GCP, independently
of Dialog's AWS API.

## Decision

Decided with Justin 2026-11-08: the phone captures microphone audio and sends it
directly to Mistral with a temporary realtime token. The existing Chauffeur GCP
relay issues that token to an authenticated, paired phone. A dedicated Mistral
API key stays in Google Secret Manager, accessible to the relay service account;
it is never shipped in the mobile app or handed to a Mac.

Transcription appears in the current message draft. Stopping dictation leaves
editable text, and only the user's Send action sends a chat message. The app
identifies Mistral as the audio recipient before recording. It stops capturing
when the conversation loses focus, the phone loses its authorized Mac connection,
or the app backgrounds. No recording is saved by Chauffeur.

## Consequences

- The relay's new HTTP endpoint uses the remembered device's phone bearer token;
  temporary pairing rooms and the administrator/host tokens do not authorize it.
  The registered phone and its host must both be connected. The relay checks
  authorization again after issuance and applies bounded issuance rates.
- The relay receives no audio or transcript. The normal encrypted chat transport
  remains unchanged. The direct HTTPS voice endpoint is a deliberate addition to
  the phone's previously Mac-forwarded operations, not a new daemon write path.
- The provider token is model-scoped and held only in memory. Mistral's tokens
  are reusable until the provider's expiry; revoking a phone blocks
  new issuance, but cannot revoke a token already accepted by Mistral.
- Captures last at most two minutes, with bounded socket buffering and startup /
  finalization timeouts. Interrupted sessions retain already transcribed draft text.
- The native microphone module requires new iOS/Android builds and physical
  device validation. The relay returns unavailable until its dedicated key is
  configured. Store privacy disclosures must include optional audio processing
  by Mistral before a build with dictation is distributed.
