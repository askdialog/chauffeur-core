import { expect, test } from "bun:test";
import { noise, pureJsCrypto } from "@chainsafe/libp2p-noise";
import type { Upgrader } from "@libp2p/interface";
import { streamPair } from "@libp2p/utils";
import {
  generateIdentity, peerIdFromPrivateKey, randomToken,
  pairingPrologue, securePairing, type Invitation,
} from "../src";
import { silentLogger } from "../src/log";

test("phone rejects a different signed identity even with the correct QR secret and prologue", async () => {
  const host = await generateIdentity("Ed25519");
  const phone = await generateIdentity("Ed25519");
  const attacker = await generateIdentity("Ed25519");
  const invitation: Invitation = {
    version: 1, relay: "wss://relay.example", room: randomToken(16),
    phoneToken: randomToken(), hostPeerId: peerIdFromPrivateKey(host).toString(),
    pairingKey: randomToken(), expiresAt: Date.now() + 60_000,
  };
  const [hostRaw, phoneRaw] = await streamPair();
  // Bypass the honest host's local invitation check to simulate an actively
  // malicious relay/endpoint. Only the phone's Noise peer pin can stop this.
  const malicious = noise({ crypto: pureJsCrypto, prologueBytes: pairingPrologue(invitation) })({
    privateKey: attacker,
    peerId: peerIdFromPrivateKey(attacker),
    logger: silentLogger,
    upgrader: { getStreamMuxers: () => new Map() } as Upgrader,
  });
  const results = await Promise.allSettled([
    malicious.secureInbound(hostRaw, { skipStreamMuxerNegotiation: true, signal: AbortSignal.timeout(1000) }),
    securePairing(phoneRaw, phone, invitation, "phone", AbortSignal.timeout(1000)),
  ]);
  hostRaw.abort(new Error("test ended"));
  phoneRaw.abort(new Error("test ended"));
  expect(results[1].status).toBe("rejected");
  expect(results[0].status).toBe("rejected");
});
