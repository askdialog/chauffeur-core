import { noise, pureJsCrypto } from "@chainsafe/libp2p-noise";
import type { MessageStream, PrivateKey, SecuredConnection, Upgrader } from "@libp2p/interface";
import { peerIdFromPrivateKey, peerIdFromString } from "@libp2p/peer-id";
import { pairingPrologue, parseInvitation, type Invitation } from "./invitation";
import { silentLogger } from "./log";
import { parseResume, resumePrologue, type ResumeConfig } from "./resume";

export { generateKeyPair as generateIdentity, generateKeyPairFromSeed as identityFromSeed, privateKeyFromProtobuf, privateKeyToProtobuf } from "@libp2p/crypto/keys";
export { peerIdFromPrivateKey };

export async function securePairing(
  raw: MessageStream,
  identity: PrivateKey,
  invitation: Invitation,
  role: "host" | "phone",
  signal: AbortSignal = AbortSignal.timeout(15_000),
): Promise<SecuredConnection> {
  try {
    parseInvitation(JSON.stringify(invitation));
    if (role === "host" && peerIdFromPrivateKey(identity).toString() !== invitation.hostPeerId) throw new Error("Wrong Mac identity");
    return await secureTransport(raw, identity, pairingPrologue(invitation), role, role === "phone" ? invitation.hostPeerId : undefined, signal);
  } catch {
    raw.abort(new Error("Secure pairing failed"));
    throw new Error("Could not authenticate the other device; generate a new pairing code");
  }
}

export async function secureResume(raw: MessageStream, identity: PrivateKey, value: ResumeConfig, role: "host" | "phone", signal = AbortSignal.timeout(15_000)): Promise<SecuredConnection> {
  try {
    const config = parseResume(value);
    if (peerIdFromPrivateKey(identity).toString() !== (role === "host" ? config.hostPeerId : config.phonePeerId)) throw new Error("Saved identity changed");
    return await secureTransport(raw, identity, resumePrologue(config), role, role === "phone" ? config.hostPeerId : config.phonePeerId, signal);
  } catch {
    raw.abort(new Error("Secure reconnection failed"));
    throw new Error("Could not authenticate the saved device");
  }
}

async function secureTransport(raw: MessageStream, identity: PrivateKey, prologueBytes: Uint8Array, role: "host" | "phone", expectedPeer: string | undefined, signal: AbortSignal): Promise<SecuredConnection> {
  // Only the standalone encrypter: no public libp2p network or multiplexers.
  const encrypter = noise({ crypto: pureJsCrypto, prologueBytes })({
    privateKey: identity,
    peerId: peerIdFromPrivateKey(identity),
    logger: silentLogger,
    upgrader: {
      getStreamMuxers: () => new Map(),
      getConnectionEncrypters: () => new Map(),
    } as Upgrader,
  });
    return role === "phone"
      ? await encrypter.secureOutbound(raw, {
          remotePeer: expectedPeer ? peerIdFromString(expectedPeer) : undefined,
          skipStreamMuxerNegotiation: true,
          signal,
        })
      : await encrypter.secureInbound(raw, { remotePeer: expectedPeer ? peerIdFromString(expectedPeer) : undefined, skipStreamMuxerNegotiation: true, signal });
}
