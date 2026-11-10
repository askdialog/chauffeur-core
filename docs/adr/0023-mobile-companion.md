# 0023. Mobile companion, relay and pairing

- **Date:** 2026-10-18
- **Status:** accepted

## Context

After V1, Justin agreed to build a phone companion to follow and drive agents remotely, through a relay that can read nothing.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Mobile follow-up (2026-10-18):

> Agreed with Justin after V1: build the companion in this repository, Android first,
> with Expo, TypeScript, Expo Router, Zustand, a dedicated React Native interface,
> and EAS Build (start on the free plan). The first delivery is the native scaffold
> and build configuration, followed by a tested relay/Noise connection proof.
>
> Remote control will use a relay hosted in GCP, with
> end-to-end encryption between authorized devices. The relay must not be able to
> read commands, conversations or attachments. Pairing must authenticate device keys
> independently of the relay; push content stays generic. The daemon remains the
> owner of workspace and agent state. The connection proof uses ChainSafe's libp2p
> Noise implementation (XX/25519/ChaChaPoly/SHA256), a QR-pinned host identity and a
> QR-only 256-bit secret bound to the handshake prologue. It grants no agent access.
> The connection proof was verified on a Pixel phone over wireless ADB.
> The next slice, approved by Justin, exposes workspace and chat lists, conversation
> history and live events, sending/steering messages, interrupting a turn, and answering
> permissions/questions. A local Mac host connects through the daemon's private Unix
> socket only after explicit approval of the Noise-authenticated phone identity on
> the Mac. Its RPC allowlist excludes terminals, files, settings and workspace
> mutations. The phone never receives the local daemon token. Workspace access and
> a real message round trip were verified on a Pixel phone.
>
> Durable pairing and reconnect (approved by Justin, 2026-10-18): chauffeurd keeps a
> stable Ed25519 identity seed and up to 16 approved devices in its private SQLite
> state. Device-trust RPCs are available only over the private Unix socket, including
> when a TCP client has the local app token. A five-minute QR still bootstraps trust, with explicit Mac approval. Only
> inside that encrypted connection does the phone receive its reconnect profile:
> the two pinned identities, a per-device secret and relay role credential. It keeps
> one saved Mac in device-only SecureStore, never in Zustand or conversation state.
> Every reconnect uses a fresh Noise handshake, a distinct resume prologue and
> mutual identity pins. Relay credentials alone cannot impersonate either device.
>
> Approved devices use renewable, opaque relay registrations instead of expiring
> QR rooms. A relay restart loses only its in-memory registrations: the Mac
> re-registers the same daemon-owned credentials, and the phone retries with capped
> backoff. No conversation, identity private key or reconnect secret goes to the
> relay. The Mac host must be running for agents to be reachable remotely.
>
> The phone reconnects after network loss, app restart and return to the foreground.
> Backgrounding disconnects its UI transport; agents continue in the daemon. On
> reconnect the current conversation fetches persisted events after its last applied
> sequence, deduplicates them and re-subscribes. Drafts and the current view stay in
> memory. No commands are queued offline or automatically replayed after a lost
> acknowledgement. Push is implemented separately as described below; background sockets are not
> a replacement for push on iOS or Android.
>
> The Mac pairing page lists devices and can revoke them. Revocation is committed
> by the daemon first, checked before every forwarded RPC, and closes open sessions
> within the host's one-second device refresh (immediately from its own revoke
> action). A connected phone forgets the Mac on the authenticated revocation notice;
> an offline revoked phone can retain an unusable local profile until it is forgotten
> or paired again. Disconnect retains trust; Forget Mac removes local credentials.
> The local session bridge is separate from the opaque relay and must never be
> deployed with it. This local follow-up still uses wireless ADB; Internet/GCP
> deployment is a separate validation.
> Cloud agent execution and reversible
> workspace transfer are separate later work.
>
> Cloud relay follow-up (Justin, 2026-10-18): use a reserved GCP public IPv4 for
> now, without a DNS hostname. HTTPS/WSS must validate normally using a public IP
> certificate with automatic renewal; E2E identity verification remains unchanged.
> A DNS hostname can be added later. Only opaque relay source is deployed; daemon
> state, private endpoint keys, conversations and the local Mac bridge stay local.

From `SPEC.md`, Notes:

> Mobile follow-up (Justin, 2026-10-18): phone pairing must open within Chauffeur
> on Mac, not an external browser. Settings > Mobile and the app menu open a
> separate native window for the existing private loopback pairing page. That
> window has no native capabilities and navigation is restricted to the pairing
> origin. Closing it must never close a chat in the main window. Network handovers
> must recover without another QR: the relay expires silent device sockets using
> incoming application heartbeats even when outbound traffic continues.

From `SPEC.md`, Integrated Mac mobile host (approved 2026-10-18):

> The daemon supervises a bundled `chauffeur-mobile-host` process after the user
> turns on mobile access. It starts again with the daemon, restarts after a crash,
> and stops on disable or daemon shutdown. Closing the desktop window leaves it
> running with the agents. No Bun, gcloud, workspace helper or login script is
> required on the installed Mac. The relay operator credential stays in the daemon's
> existing secret store (Mac Keychain); only enabled/origin are stored in SQLite.
> Credentials reach the child on stdin, never argv or a public RPC result. Changing
> relay requires its own credential. Mobile clients cannot configure the host.
> Settings > Mobile shows readiness, errors and connected-phone count, and opens
> the isolated native pairing window once ready. Disabling preserves trust grants.
> This remains the single-operator relay setup, not multi-user account management.

From `SPEC.md`, Mobile push notifications and pairing window (approved 2026-10-18):

> Android push uses Expo Notifications' native FCM token, registered over the
> mutually pinned E2E channel and persisted only with the daemon-owned device grant.
> The phone explicitly opts in and chooses approvals/questions, completed turns,
> errors, and whether workspace names appear. Workspace names are allowed by Justin;
> when enabled, the name is visible to GCP/FCM and the lock screen. Command text,
> conversation messages and agent output never enter push payloads. All preferences
> and labels are FR/EN. Android channels control sound/vibration in system settings.
> No prompt at startup. Revocation removes registration with the grant; OS-denied
> notifications disable the saved preference on foreground reconnect. New builds
> are required for native notification support; older clients display an update hint.
>
> The Mac host watches only live persisted events while the phone is disconnected.
> No replay of old events or push while connected in the foreground. Sends use the
> relay's host-role credential and its renewable device registration; the relay
> bounds/rate-limits inputs and emits only allowlisted fields to FCM. Its VM identity
> has only cloudmessaging.messages.create; no service-account key is exported.
> FCM tokens are delivery metadata, not E2E conversation keys. The relay remains
> where its operator deploys it; FCM is Google's delivery service. Tapping opens the workspace list and
> reconnects; notifications cannot authorize or execute agent actions. Mac must be
> awake/online. iOS APNs credentials and physical-device validation remain separate.
>
> The Mac loopback pairing page follows Chauffeur's compact dark UI, with a centered
> QR, clear scan/approval steps, explicit peer identity verification, per-connection
> relay labels, QR expiry, and discreet revocation with confirmation. No external
> assets or additional native capabilities. FR/EN follow the OS language.

## Consequences

Android first, with Expo, in `mobile/`; sessions are end-to-end encrypted (Noise) and the daemon stays the owner of state; push payloads carry no conversation content. Overview: [`docs/mobile.md`](../mobile.md).
