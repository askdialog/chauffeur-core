import { expect, test } from "bun:test";
import { generateKeyPairSync, verify } from "node:crypto";
import { apnsSender } from "../src/apns";
const { privateKey, publicKey } = generateKeyPairSync("ec", {
  namedCurve: "prime256v1",
});
const config = {
  teamId: "ABCDEFGHIJ",
  keyId: "0123456789",
  topic: "com.askdialog.chauffeur",
  privateKey: privateKey.export({ type: "pkcs8", format: "pem" }).toString(),
};
const message = {
  provider: "apns" as const,
  token: "a".repeat(64),
  kind: "approval" as const,
  language: "fr" as const,
};

test("Apple receives a signed alert with only approved content and a short expiration", async () => {
  const sender = apnsSender(config, async (origin, headers, body) => {
    expect(origin).toBe("https://api.push.apple.com");
    expect(headers[":path"]).toBe(`/3/device/${message.token}`);
    expect(headers["apns-topic"]).toBe(config.topic);
    expect(headers["apns-push-type"]).toBe("alert");
    expect(Number(headers["apns-expiration"])).toBeGreaterThan(
      Date.now() / 1000,
    );
    const jwt = headers.authorization!.replace("bearer ", "");
    const [head, claims, signature] = jwt.split(".");
    expect(JSON.parse(Buffer.from(head!, "base64url").toString())).toEqual({
      alg: "ES256",
      kid: config.keyId,
    });
    expect(JSON.parse(Buffer.from(claims!, "base64url").toString()).iss).toBe(
      config.teamId,
    );
    expect(
      verify(
        "sha256",
        Buffer.from(`${head}.${claims}`),
        { key: publicKey, dsaEncoding: "ieee-p1363" },
        Buffer.from(signature!, "base64url"),
      ),
    ).toBe(true);
    expect(JSON.parse(body)).toEqual({
      aps: {
        alert: {
          title: "Chauffeur",
          body: "Une autorisation ou une réponse est nécessaire.",
        },
        sound: "default",
      },
      chauffeur: "update",
    });
    return { status: 200 };
  });
  expect(await sender(message)).toBe("ok");
});

test("sandbox is explicit and activity notifications have no sound", async () => {
  const sender = apnsSender(
    { ...config, sandbox: true },
    async (origin, _headers, body) => {
      expect(origin).toBe("https://api.sandbox.push.apple.com");
      expect(JSON.parse(body).aps.sound).toBeUndefined();
      expect(JSON.parse(body).aps.alert.title).toBe("Example");
      return { status: 200 };
    },
  );
  await sender({ ...message, token: "b".repeat(128), kind: "completed", workspace: "Example" });
});

test("only a confirmed unregistered Apple token disables delivery", async () => {
  expect(
    await apnsSender(config, async () => ({
      status: 410,
      reason: "Unregistered",
    }))(message),
  ).toBe("unregistered");
  for (const response of [
    { status: 400, reason: "BadDeviceToken" },
    { status: 403, reason: "InvalidProviderToken" },
    { status: 500 },
  ]) {
    await expect(
      apnsSender(config, async () => response)(message),
    ).rejects.toThrow("APNs delivery failed");
  }
});

test("invalid Apple tokens are refused before contacting a provider", async () => {
  let called = false;
  const sender = apnsSender(config, async () => {
    called = true;
    return { status: 200 };
  });
  await expect(
    sender({ ...message, token: "not-an-apple-token" }),
  ).rejects.toThrow("Invalid APNs device token");
  expect(called).toBe(false);
});

test("the relay preserves Apple delivery metadata and rejects unknown providers", async () => {
  const { startRelay } = await import("../src/server");
  const sent: unknown[] = [];
  const admin = "a".repeat(64),
    hostToken = "h".repeat(43),
    phoneToken = "p".repeat(43),
    deviceId = "d".repeat(22);
  const relay = startRelay({
    adminToken: admin,
    port: 0,
    push: async (value) => {
      sent.push(value);
      return "ok";
    },
  });
  const url = `http://127.0.0.1:${relay.server.port}`;
  try {
    await fetch(`${url}/v1/devices`, {
      method: "POST",
      headers: { Authorization: `Bearer ${admin}` },
      body: JSON.stringify({ deviceId, hostToken, phoneToken }),
    });
    const deliver = (value: object) =>
      fetch(`${url}/v1/devices/${deviceId}/push`, {
        method: "POST",
        headers: { Authorization: `Bearer ${hostToken}` },
        body: JSON.stringify(value),
      });
    expect((await deliver({ ...message, provider: "other" })).status).toBe(400);
    expect(
      (await deliver({ ...message, token: "invalid_apple_token" })).status,
    ).toBe(400);
    expect(
      (await deliver({ ...message, command: "must never leave the relay" }))
        .status,
    ).toBe(200);
    expect(sent).toEqual([message]);
  } finally {
    relay.stop();
  }
});
