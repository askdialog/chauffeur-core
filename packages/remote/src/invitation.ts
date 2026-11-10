import { peerIdFromString } from "@libp2p/peer-id";
import { fromString } from "uint8arrays/from-string";
import { toString } from "uint8arrays/to-string";

export const INVITATION_LIFETIME_MS = 5 * 60_000;
export const MAX_FRAME_BYTES = 128 * 1024;

export type Invitation = {
  version: 1;
  relay: string;
  room: string;
  phoneToken: string;
  hostPeerId: string;
  pairingKey: string;
  expiresAt: number;
};

export function randomToken(bytes = 32): string {
  return toString(crypto.getRandomValues(new Uint8Array(bytes)), "base64url");
}

// HTTPS/WSS in deployment. Loopback is solely for tests and adb reverse; a LAN
// hostname is not an excuse to expose relay credentials over an unencrypted link.
export function relayUrl(value: string): URL {
  const url = new URL(value);
  const loopback = ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
  if ((url.protocol !== "wss:" && !(loopback && url.protocol === "ws:")) ||
      url.username || url.password || url.search || url.hash || url.pathname !== "/") {
    throw new Error("Use a wss:// relay origin (ws:// loopback is for local testing only)");
  }
  return url;
}

export function parseInvitation(text: string, now = Date.now()): Invitation {
  if (text.length > 4096) throw new Error("Pairing code is too large");
  const value = JSON.parse(text) as Partial<Invitation>;
  const token = /^[A-Za-z0-9_-]{43}$/;
  if (!value || value.version !== 1 || typeof value.relay !== "string" ||
      typeof value.room !== "string" || !/^[A-Za-z0-9_-]{22}$/.test(value.room) ||
      typeof value.phoneToken !== "string" || !token.test(value.phoneToken) ||
      typeof value.pairingKey !== "string" || !token.test(value.pairingKey) ||
      typeof value.hostPeerId !== "string" || value.hostPeerId.length > 128 ||
      typeof value.expiresAt !== "number" || !Number.isSafeInteger(value.expiresAt) ||
      value.expiresAt <= now || value.expiresAt > now + INVITATION_LIFETIME_MS + 10_000) {
    throw new Error("Invalid or expired pairing code; generate a new code on your Mac");
  }
  relayUrl(value.relay);
  const peer = peerIdFromString(value.hostPeerId);
  if (peer.type !== "Ed25519" || !peer.publicKey) throw new Error("Unsupported device identity");
  if (fromString(value.pairingKey, "base64url").length !== 32) throw new Error("Invalid pairing key");
  return value as Invitation;
}

export function pairingPrologue(invitation: Invitation): Uint8Array {
  // The secret is carried only in the QR, never sent to the relay. Noise binds
  // it into the handshake. The role tokens alone cannot authorize a new peer.
  return fromString(JSON.stringify([
    "chauffeur-pairing-v1", invitation.relay, invitation.room,
    invitation.hostPeerId, invitation.expiresAt, invitation.pairingKey,
  ]));
}
