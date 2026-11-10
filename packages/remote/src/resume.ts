import { peerIdFromString } from "@libp2p/peer-id";
import { fromString } from "uint8arrays/from-string";
import { relayUrl } from "./invitation";

// Delivered only inside the approved Noise channel; never included in a URL,
// relay registration or Zustand. The role token alone grants no daemon access.
export type ResumeConfig = {
  version: 1;
  relay: string;
  deviceId: string;
  phoneToken: string;
  hostPeerId: string;
  phonePeerId: string;
  secret: string;
};

export function parseResume(value: unknown): ResumeConfig {
  if (!value || typeof value !== "object") throw new Error("Invalid saved Mac");
  const config = value as ResumeConfig;
  const token = /^[A-Za-z0-9_-]{43}$/;
  if (config.version !== 1 || typeof config.relay !== "string" ||
      typeof config.deviceId !== "string" || !/^[A-Za-z0-9_-]{22}$/.test(config.deviceId) ||
      typeof config.phoneToken !== "string" || !token.test(config.phoneToken) ||
      typeof config.secret !== "string" || !token.test(config.secret)) throw new Error("Invalid saved Mac");
  relayUrl(config.relay);
  for (const identity of [config.hostPeerId, config.phonePeerId]) {
    if (typeof identity !== "string" || identity.length > 128) throw new Error("Invalid device identity");
    const peer = peerIdFromString(identity);
    if (peer.type !== "Ed25519" || !peer.publicKey) throw new Error("Unsupported device identity");
  }
  return { version: 1, relay: config.relay, deviceId: config.deviceId, phoneToken: config.phoneToken, hostPeerId: config.hostPeerId, phonePeerId: config.phonePeerId, secret: config.secret };
}

export function resumePrologue(config: ResumeConfig): Uint8Array {
  return fromString(JSON.stringify(["chauffeur-resume-v1", config.relay, config.deviceId, config.hostPeerId, config.phonePeerId, config.secret]));
}
