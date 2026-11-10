import { expect, test } from "bun:test";
import { noise, pureJsCrypto } from "@chainsafe/libp2p-noise";
import { streamPair } from "@libp2p/utils";
import type { Upgrader } from "@libp2p/interface";
import { generateIdentity, peerIdFromPrivateKey, randomToken, resumePrologue, secureResume, type ResumeConfig } from "../src";
import { silentLogger } from "../src/log";

for (const role of ["host", "phone"] as const) test(`reconnect pins the ${role === "host" ? "phone" : "Mac"} even with stolen relay credentials and prologue`, async () => {
  const host = await generateIdentity("Ed25519"), phone = await generateIdentity("Ed25519"), attacker = await generateIdentity("Ed25519");
  const profile: ResumeConfig = { version: 1, relay: "wss://relay.example", deviceId: randomToken(16), phoneToken: randomToken(), hostPeerId: peerIdFromPrivateKey(host).toString(), phonePeerId: peerIdFromPrivateKey(phone).toString(), secret: randomToken() };
  const [honestRaw, attackerRaw] = await streamPair();
  const malicious = noise({ crypto: pureJsCrypto, prologueBytes: resumePrologue(profile) })({ privateKey: attacker, peerId: peerIdFromPrivateKey(attacker), logger: silentLogger, upgrader: { getStreamMuxers: () => new Map() } as Upgrader });
  const signal = AbortSignal.timeout(1000);
  const results = await Promise.allSettled([
    secureResume(honestRaw, role === "host" ? host : phone, profile, role, signal),
    role === "host" ? malicious.secureOutbound(attackerRaw, { skipStreamMuxerNegotiation: true, signal }) : malicious.secureInbound(attackerRaw, { skipStreamMuxerNegotiation: true, signal }),
  ]);
  honestRaw.abort(new Error("test ended")); attackerRaw.abort(new Error("test ended"));
  expect(results[0].status).toBe("rejected");
});
