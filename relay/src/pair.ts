// Connection proof only: no daemon access, no filesystem writes, no agent API.
// The daemon will own durable identities and trusted-device records in the next
// integration. This temporary identity dies with this command.
import {
  generateIdentity, peerIdFromPrivateKey, randomToken, relayUrl,
  joinRelay, securePairing, parseInvitation, JsonChannel, type Invitation,
} from "@chauffeur/remote";
import qrcode from "qrcode-terminal";

const origin = process.env.RELAY_ORIGIN ?? "ws://127.0.0.1:41240";
const adminToken = process.env.RELAY_ADMIN_TOKEN;
if (!adminToken) throw new Error("Set RELAY_ADMIN_TOKEN to create a pairing invitation");
const url = relayUrl(origin);
url.protocol = url.protocol === "wss:" ? "https:" : "http:";
url.pathname = "/v1/rooms";
const response = await fetch(url, {
  method: "POST", headers: { Authorization: `Bearer ${adminToken}` }, signal: AbortSignal.timeout(10_000),
});
if (!response.ok) throw new Error(`Could not create pairing invitation (${response.status})`);
const room = await response.json() as { room: string; hostToken: string; phoneToken: string; expiresAt: number };
const identity = await generateIdentity("Ed25519");
const invitation: Invitation = {
  version: 1, relay: origin, room: room.room, phoneToken: room.phoneToken,
  hostPeerId: peerIdFromPrivateKey(identity).toString(),
  pairingKey: randomToken(), expiresAt: room.expiresAt,
};
parseInvitation(JSON.stringify(invitation));
if (typeof room.hostToken !== "string" || !/^[A-Za-z0-9_-]{43}$/.test(room.hostToken)) {
  throw new Error("Invalid relay admission token");
}
console.info("Scan locally in Chauffeur. This one-time code expires in five minutes. Do not share it.");
qrcode.generate(JSON.stringify(invitation), { small: true });
if (process.argv.includes("--json")) console.info(JSON.stringify(invitation));

const raw = await joinRelay(origin, room.room, "host", room.hostToken, undefined, AbortSignal.timeout(5 * 60_000));
const secure = await securePairing(raw, identity, invitation, "host");
console.info("Authenticated phone connected. This session only confirms the encrypted connection.");
const channel = new JsonChannel(secure.connection);
channel.onmessage = (value) => {
  // A fixed, bounded exchange: never treat arbitrary bytes as agent commands.
  if ((value as { type?: string })?.type !== "check") {
    secure.connection.abort(new Error("Unexpected connection check"));
    return;
  }
  channel.send({ type: "verified" });
};
process.on("SIGINT", () => { void secure.connection.close(); });
