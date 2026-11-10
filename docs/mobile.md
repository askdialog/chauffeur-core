# Mobile companion

The phone follows and drives a Mac's agents: workspace and chat lists, history and live events, sending and steering, interrupting, answering permissions and questions, and transfers. The Expo iOS and Android app is maintained in its own repository; the opaque relay and the Mac's host are in [`relay/`](../relay/README.md).

Decisions: [ADR 0023](adr/0023-mobile-companion.md) (companion, relay, pairing, host, push; 2026-10-18) and [ADR 0028](adr/0028-mobile-store-preparation.md) (store preparation; 2026-10-20). Their full text is in `SPEC.md`, Mobile follow-up.

## Source ownership

The phone app has its own repository, CI, EAS builds and store releases. It uses a
verified SDK snapshot pinned to a Core commit for protocol types, the shared
transport/client lifecycle and pure conversation helpers. Neither a sibling Core
checkout nor cross-repository CI credentials are needed to build it.
The daemon, Mac host, relay and their integration tests remain here. Existing
Expo project, application identifiers, version and pairing behavior are retained.
See [the decision](adr/separate-mobile-repository.md).

## How it connects

```
phone (Expo app) ── Noise XX over WSS ── relay (opaque) ── Mac mobile host ── Unix socket ── chauffeurd
```

- The relay forwards ciphertext only: it cannot read commands, conversations or attachments. It runs where its operator deploys it (`relay/deploy/gcp.sh` provisions one on GCP, on a reserved public IPv4 with a public IP certificate or on a hostname). Core names no relay: `[mobile] relay` in `settings.toml` (Settings › Mobile) is empty by default, and mobile access stays off until it is set.
- Pairing is a five-minute QR shown in a native window of the Mac app, approved on the Mac. Approved devices (16 at most) reconnect with a fresh, mutually pinned Noise handshake; the Mac revokes them from its pairing page.
- The daemon supervises the bundled `chauffeur-mobile-host` once mobile access is on (Settings › Mobile). The host forwards an allowlist of RPCs over the daemon's private Unix socket: no terminals, files or settings, and the phone never receives the daemon's token.
- Push (FCM on Android, APNs on iOS) is opt-in per category and carries no conversation content; workspace names only if the user allows it.

## Delivery notes

Moved from `docs/PLAN.md` on 2026-10-29.

Mobile follow-up (agreed with Justin, 2026-10-18): Android first, in `mobile/`,
using Expo, TypeScript, Expo Router, Zustand and EAS Build's free plan initially.
The scaffold has Android development APK, standalone preview APK and production
AAB profiles. The connection proof adds an opaque Bun relay, Noise transport,
expiring QR invitations, mobile scan/paste and an encrypted check with a temporary
Mac endpoint. Real WebSocket tests cover tampering, replay and authentication.
The encrypted connection proof ran on a Pixel phone. A session-only local
host now bridges an explicitly approved phone to an allowlist of existing daemon
RPCs over its private Unix socket. Mobile adds workspace/chat lists, history,
live events, send/steer, interrupt, permissions and questions, sharing the desktop's
pure conversation reducer. The next slice adds daemon-owned durable trust via
`remote.state/devices/approve/check/revoke`, renewable relay device registrations,
mutually pinned Noise reconnect, SecureStore profiles, foreground/network retry,
incremental history replay and revocation. QR bootstrap alone expires after five
minutes; remembered device sessions do not. Push implementation and store preparation are described below. GCP provisioning
scripts support an IP-only deployment, public IP TLS with
automated renewal, bounded local logs and an explicit cloud E2E probe. A deployment
was verified (TLS, synthetic Noise exchange, renewal dry run and service journals).
VM execution and workspace transfer came later.

Mac mobile integration: daemon supervision and secret-store configuration, compiled
bridge bundled by packaging/install scripts, desktop readiness and pairing entry.
Verification uses an isolated daemon and synthetic relay, never real device grants.

Mobile store preparation (2026-10-20): About/privacy/support entry points, isolated
public sample demo, iOS APNs delivery with backward-compatible provider metadata,
iOS export/build/submit commands, and store-preparation drafts/checklist. Pending:
the publisher's legal/contact details and published URLs, store accounts/signing, relay-only
Apple key provisioning, physical iPhone/release-build validation and live review
access. No production relay deployment or store submission is performed by this
preparation.

## Since then

The phone also follows, starts and brings back transfers ([on the phone](cloud-workspace-transfer.md#on-the-phone)) and opens a moved workspace's private web previews ([previews on the phone](workspace-relay.md#previews-on-the-phone)); a batch transfer from the phone is later work. What the app does today is documented in its own repository.

## Recent history and confirmed sends

A moved workspace stays actionable through the paired computer's daemon, which routes chat operations to its current execution host. The phone rereads the workspace location and scopes cached conversation cursors to that host; it disables actions during ownership handoff. The paired computer must remain reachable. Direct phone-to-worker pairing is not part of this path.

New hosts answer `events.history` with `tail: true` and an exclusive `before_seq` cursor. The phone displays recent messages immediately, reconstructs the older state separately, then subscribes from the reconstructed cursor. A partial preview never becomes the reconnect cache. Older hosts retain the forward-pagination fallback. Hidden screens stop their polling and chat subscriptions; foreground/focus resumes with a fresh read.

The phone calls `chat.send_confirmed` with a UUID retained alongside its in-memory draft. The daemon durably claims the identifier before executing, stores acceptance and carries the receipts with the workspace package. Retries confirm accepted sends without executing again. A crash or failure whose handoff cannot be confirmed leaves an uncertain receipt: it is refused rather than replayed. Reusing an identifier with different contents is refused. Older mobile hosts reject the method; the phone explains the required update and never falls back to a non-idempotent send. No conversation or draft is persisted on the phone.

## Models and workspace organization

The companion presents main workspaces in the Mac's pinned and section order.
Descendant activity contributes to the parent's status; searching for a child
shows its parent. An unavailable parent is named on a recovery entry so an active
child remains reachable; cycles also keep a reachable entry point.
Opening a workspace shows its conversations and children,
including when it has a single conversation. Children and their conversations
retain parent context. Section folding is local to the phone. See
[focused mobile navigation](adr/mobile-focused-navigation.md).

An approved phone may call `models.list` with the workspace id and `chat.configure`
with `chat`, `model`, `effort`, `fast`, `agent` and `confirm_agent` only. The bridge
rejects permission settings and unrelated fields. Catalogs and changes route to
the workspace's execution host. The daemon enforces provider-switch confirmation
and its existing restrictions. Older mobile hosts refuse these methods; the app
shows an update message rather than retrying a write through another endpoint.
See [the decision](adr/mobile-models-and-workspace-hierarchy.md).

## Dictation

The mobile composer uses Voxtral realtime transcription with a temporary token from the GCP relay. Audio goes directly from the phone to Mistral, never through the Mac or relay. The dedicated API key remains server-side in Secret Manager. The phone must be remembered and connected to its host; the new HTTP endpoint authenticates its device credential separately from the encrypted command bridge. No new daemon method or SDK snapshot is required. See [the decision](adr/mobile-voxtral-dictation.md) and `relay/deploy/configure-voice.sh` for deployment preparation.

## Private Canvas

Design chats offer Open Canvas and screen links. The bridge allows `design.canvas`
and `design.resource` only, sanitizing Mac preview URLs to availability markers.
Resource reads are descriptor-scoped to a local design folder, at most 16 MiB per
file and 256 KiB per response, with stable-file versions across chunks. The phone's
per-view capability URL exists only on its loopback while focused, foreground and
authorized; HTML has no native message bridge, credentials or persistent asset cache.
Moved designs require opening on the Mac for now. See [the decision](adr/mobile-private-canvas.md).
