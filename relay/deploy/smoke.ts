// Explicit cloud probe: ephemeral identities and synthetic data only. The
// operator credential is supplied via the environment, never printed.
import { strict as assert } from "node:assert";
import {
  generateIdentity, joinRelay, peerIdFromPrivateKey, randomToken, relayUrl,
  securePairing, type Invitation, type Socket,
} from "@chauffeur/remote";

const origin = relayUrl(process.env.RELAY_ORIGIN ?? "").origin;
const token = process.env.RELAY_ADMIN_TOKEN;
assert(token, "RELAY_ADMIN_TOKEN required");
const http = origin.replace(/^ws/, "http");
assert.equal((await fetch(`${http}/health`)).status, 200);
assert.equal((await fetch(`${http}/v1/rooms`, { method: "POST" })).status, 401);
const response = await fetch(`${http}/v1/rooms`, {
  method: "POST", headers: { Authorization: `Bearer ${token}` },
});
assert.equal(response.status, 201);
const room = await response.json() as { room: string; hostToken: string; phoneToken: string; expiresAt: number };
const hostIdentity = await generateIdentity("Ed25519");
const phoneIdentity = await generateIdentity("Ed25519");
const invitation: Invitation = {
  version: 1, relay: origin, room: room.room, phoneToken: room.phoneToken,
  hostPeerId: peerIdFromPrivateKey(hostIdentity).toString(),
  pairingKey: randomToken(), expiresAt: room.expiresAt,
};
const captured: Uint8Array[] = [];
const capture = (url: string): Socket => {
  const socket = new WebSocket(url);
  const send = socket.send.bind(socket);
  socket.send = ((data: string | Uint8Array) => {
    if (typeof data !== "string") captured.push(data.slice());
    send(data as never);
  }) as typeof socket.send;
  return socket as unknown as Socket;
};
const signal = AbortSignal.timeout(20_000);
const [hostRaw, phoneRaw] = await Promise.all([
  joinRelay(origin, room.room, "host", room.hostToken, capture, signal),
  joinRelay(origin, room.room, "phone", room.phoneToken, undefined, signal),
]);
try {
  const [host, phone] = await Promise.all([
    securePairing(hostRaw, hostIdentity, invitation, "host", signal),
    securePairing(phoneRaw, phoneIdentity, invitation, "phone", signal),
  ]);
  assert.equal(phone.remotePeer.toString(), invitation.hostPeerId);
  assert.equal(host.remotePeer.toString(), peerIdFromPrivateKey(phoneIdentity).toString());
  const message = `synthetic-cloud-probe-${randomToken()}`;
  const incoming = phone.connection[Symbol.asyncIterator]().next();
  host.connection.send(new TextEncoder().encode(message));
  const received = await incoming;
  assert.equal(new TextDecoder().decode(received.value!.subarray()), message);
  const wire = captured.map(bytes => new TextDecoder().decode(bytes)).join("");
  assert(!wire.includes(message));
  assert(!wire.includes(invitation.pairingKey));
  await host.connection.close();
  console.info("Cloud smoke OK: trusted TLS, authorization, pinned Noise peers and encrypted delivery.");
} finally {
  await hostRaw.close();
  await phoneRaw.close();
}
