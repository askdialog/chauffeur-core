# Remote connection primitive

Shared TypeScript transport for the connection proof in `relay/` and `mobile/`.
Uses ChainSafe's standalone libp2p Noise encrypter, explicitly selecting its pure
JavaScript crypto implementation for React Native. It does not start libp2p,
join a public peer network, discover peers or write any files.

## Protocol

1. The Mac creates a five-minute room over HTTPS, authenticated with an operator
   credential. The relay returns separate, random host/phone admission tokens.
2. The Mac uses its daemon-owned Ed25519 identity (temporary in proof-only mode)
   and a random 256-bit pairing
   secret. The local QR includes the relay origin, room, phone admission token,
   host peer identity, expiry and pairing secret. It contains no Mac private key.
3. Each endpoint opens WSS and authenticates its role in the first text frame.
   Tokens never appear in URLs. Each room accepts at most one host and one phone.
4. The relay sends `ready` only when both roles are admitted. Data frames are
   binary; the relay forwards opaque bytes without parsing Noise. A fixed
   ping/pong control exchange detects stalled foreground connections.
5. Endpoints run `Noise_XX_25519_ChaChaPoly_SHA256`. The phone pins the host identity
   from the QR. A versioned Noise prologue binds the room, origin, expiry, pinned
   host identity and QR-only secret to the authenticated handshake transcript.
   Knowing both relay admission tokens does not suffice to finish the handshake.
6. Length-prefixed JSON over the encrypted byte stream confirms the connection.
   Proof-only mode grants no access. The local Mac host can instead request Mac
   approval of the authenticated phone, then provision its durable device grant.

Each attempt creates fresh ephemeral Noise keys. Authentication, encryption and
ordered transport counters are supplied by Noise, not an application cipher.
Do not persist transport cipher state or resume old cipher counters after a drop.
QR rooms end on disconnect or after five minutes. An approved phone receives a
device profile only through that encrypted session: pinned host and phone IDs,
random per-device secret, relay origin/device ID and phone-role admission token.
The daemon retains the corresponding credentials and stable Mac identity; the
phone stores its profile and private identity in device-only SecureStore.

Reconnect uses `/v1/devices/<id>` with a separate, versioned Noise prologue that
binds the relay, device ID, both pinned identities and reconnect secret. Both
sides verify the other's signed identity, even if a malicious relay learns the
role tokens or is given the correct prologue. No cipher state is reused. The
host re-registers its opaque relay metadata after a restart, so a saved profile
does not depend on the relay retaining state. Device sessions have no QR TTL.

## Trust and limits

- The relay sees IP addresses, room membership, timing and sizes. It can deny,
  delay or interrupt service. It cannot silently substitute the QR-pinned host or
  decrypt application data when endpoint keys and the QR remain private.
- A QR is a short-lived physical authorization capability: do not share screenshots
  or paste it into chat/logs. No short human password is used as its secret.
- Noise's logger is deliberately a no-op, including when upstream key-dump debug
  flags are enabled. Do not replace it with a general debug logger.
- The phone identity and approved Mac profile use native SecureStore. Invitations
  and cipher keys remain in memory. The Mac proof-only mode stays temporary;
  the daemon-backed host uses its persisted identity and device grants.
- This is a protocol integration proof, not a completed production security
  assessment. Android bundling does not replace execution on a real phone.
- The daemon-backed host checks device grants before forwarding each allowed RPC
  and disconnects revoked devices. It never forwards private `remote.*` methods,
  terminals, files or settings. The app replays persisted events after its last
  sequence and never automatically replays writes after an uncertain send.
- Push and multi-user relay accounts remain. A deployed HTTPS/WSS relay has
  been validated with a physical Pixel and synthetic encrypted smoke tests.
  The relay operator credential must not become a shared end-user login scheme.

## Checks

```sh
bun install --frozen-lockfile
bun run typecheck
cd ../../relay
bun install --frozen-lockfile
bun run typecheck
bun test
```

Tests exercise real loopback WebSockets and the real Noise implementation. They
do not access a user's daemon, keys, repositories or `~/.chauffeur`.

References: [Noise specification](https://noiseprotocol.org/noise.html),
[libp2p Noise specification](https://github.com/libp2p/specs/blob/master/noise/README.md),
[ChainSafe implementation](https://github.com/ChainSafe/js-libp2p-noise).
