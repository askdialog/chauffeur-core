import { afterAll, beforeAll, expect, test } from "bun:test";
import { createLocalJWKSet, exportJWK, generateKeyPair, SignJWT, type JWTPayload } from "jose";
import { googleMobileAccess } from "../src/mobile-access";
import { startRelay } from "../src/server";
import { joinRelay, randomToken } from "@chauffeur/remote";

const audience = "chauffeur.apps.googleusercontent.com";
const member = { sub: "member", email: "person@askdialog.com", email_verified: true, hd: "askdialog.com" };
const guest = { sub: "guest", email: "friend@gmail.com", email_verified: true };
let sign: (claims: JWTPayload, target?: string) => Promise<string>;
let accessWith: (invitations: string[]) => ReturnType<typeof googleMobileAccess>;
let verify: ReturnType<typeof googleMobileAccess>;
let relay: ReturnType<typeof startRelay>;
let origin: string;
const adminToken = randomToken();

beforeAll(async () => {
  const { publicKey, privateKey } = await generateKeyPair("RS256");
  const keys = createLocalJWKSet({ keys: [{ ...await exportJWK(publicKey), kid: "google" }] });
  sign = (claims, target = audience) => new SignJWT(claims)
    .setProtectedHeader({ alg: "RS256", kid: "google" })
    .setIssuer("https://accounts.google.com")
    .setAudience(target)
    .setExpirationTime(claims.exp ?? "1h")
    .sign(privateKey);
  accessWith = invitations => googleMobileAccess(audience, "askdialog.com", invitations, keys);
  verify = accessWith([" Friend@gmail.com "]);
  relay = startRelay({ adminToken, port: 0, mobileAccess: verify });
  origin = `http://127.0.0.1:${relay.server.port}`;
});
afterAll(() => relay?.stop());

test("only verified identities from the Dialog Workspace or on the invitation list receive access", async () => {
  expect(await verify(await sign(member))).toMatchObject({ subject: "member" });
  expect(await verify(await sign(guest))).toMatchObject({ subject: "guest" });
  for (const claims of [
    { ...member, hd: "another.com" },
    { ...member, hd: undefined },
    { ...guest, email: "stranger@gmail.com" },
    { ...guest, email_verified: false },
    { ...guest, azp: "some-other-client" },
    { ...guest, exp: 1 },
  ]) {
    expect(await verify(await sign(claims))).toBeNull();
  }
  expect(await verify(await sign(guest, "some-other-client"))).toBeNull();
});

function request(path: string, credential: string, method = "POST", body?: object) {
  return fetch(`${origin}${path}`, {
    method,
    headers: { Authorization: `Bearer ${credential}`, "Content-Type": "application/json" },
    body: body && JSON.stringify(body),
  });
}

test("an invited host can pair and renew its own device while another approved account cannot take it over", async () => {
  const guestToken = await sign(guest);
  const memberToken = await sign(member);
  const stranger = await sign({ ...guest, email: "stranger@gmail.com" });
  expect((await request("/v1/mobile-access", stranger)).status).toBe(403);
  expect((await request("/v1/mobile-access", guestToken)).status).toBe(204);
  expect((await request("/v1/rooms", guestToken)).status).toBe(201);
  expect((await request("/v1/rooms", stranger)).status).toBe(401);
  const device = { deviceId: randomToken(16), hostToken: randomToken(), phoneToken: randomToken() };
  expect((await request("/v1/devices", guestToken, "POST", device)).status).toBe(200);
  expect((await request("/v1/devices", memberToken, "POST", device)).status).toBe(401);
  expect((await request("/v1/devices", adminToken, "POST", device)).status).toBe(401);
  expect((await request("/v1/devices", await sign(guest), "POST", device)).status).toBe(200);
  expect((await request(`/v1/devices/${device.deviceId}`, memberToken, "DELETE")).status).toBe(401);
  expect((await request(`/v1/devices/${device.deviceId}`, guestToken, "DELETE")).status).toBe(200);
});

test("an existing operator pairing can move to Google authorization without creating a new device", async () => {
  const device = { deviceId: randomToken(16), hostToken: randomToken(), phoneToken: randomToken() };
  expect((await request("/v1/devices", adminToken, "POST", device)).status).toBe(200);
  const memberToken = await sign(member);
  expect((await request("/v1/devices", memberToken, "POST", { ...device, phoneToken: randomToken() })).status).toBe(401);
  expect((await request("/v1/devices", memberToken, "POST", device)).status).toBe(200);
});

test("applying a policy without a former guest disconnects a live phone and blocks its return", async () => {
  const guestToken = await sign(guest);
  const device = { deviceId: randomToken(16), hostToken: randomToken(), phoneToken: randomToken() };
  const original = startRelay({ adminToken, port: 0, mobileAccess: verify });
  const address = `127.0.0.1:${original.server.port}`;
  const register = (address: string) => fetch(`http://${address}/v1/devices`, {
    method: "POST",
    headers: { Authorization: `Bearer ${guestToken}` },
    body: JSON.stringify(device),
  });
  try {
    expect((await register(address)).status).toBe(200);
    const signal = AbortSignal.timeout(1000);
    const [, phone] = await Promise.all([
      joinRelay(`ws://${address}`, device.deviceId, "host", device.hostToken, undefined, signal, "devices"),
      joinRelay(`ws://${address}`, device.deviceId, "phone", device.phoneToken, undefined, signal, "devices"),
    ]);
    const disconnected = phone[Symbol.asyncIterator]().next().then(
      () => null,
      (error: unknown) => error,
    );
    original.stop();
    expect(await disconnected).toBeInstanceOf(Error);

    const updated = startRelay({ adminToken, port: 0, mobileAccess: accessWith([]), requireGoogle: true });
    try {
      const updatedAddress = `127.0.0.1:${updated.server.port}`;
      expect((await register(updatedAddress)).status).toBe(401);
      expect((await fetch(`http://${updatedAddress}/v1/devices/${device.deviceId}`)).status).toBe(404);
      const createRoom = (token: string) => fetch(`http://${updatedAddress}/v1/rooms`, {
        method: "POST", headers: { Authorization: `Bearer ${token}` },
      });
      expect((await createRoom(adminToken)).status).toBe(401);
      expect((await fetch(`http://${updatedAddress}/v1/devices`, {
        method: "POST",
        headers: { Authorization: `Bearer ${adminToken}` },
        body: JSON.stringify(device),
      })).status).toBe(401);
      expect((await createRoom(await sign(member))).status).toBe(201);
    } finally { updated.stop(); }
  } finally { original.stop(); }
});
