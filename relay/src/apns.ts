import { createPrivateKey, sign } from "node:crypto";
import { connect } from "node:http2";
import type { PushMessage } from "./fcm";
import { notificationPayload } from "./fcm";

type ApnsResponse = { status: number; reason?: string };
type ApnsTransport = (
  origin: string,
  headers: Record<string, string>,
  body: string,
) => Promise<ApnsResponse>;

// APNs requires HTTP/2. Neither payloads, credentials nor provider error bodies are logged.
const transport: ApnsTransport = (origin, headers, body) =>
  new Promise((resolve, reject) => {
    const session = connect(origin);
    let settled = false;
    const finish = (result?: ApnsResponse) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      session.destroy();
      if (result) resolve(result);
      else
        reject(
          new Error(
            "APNs delivery failed; check relay credentials and connectivity.",
          ),
        );
    };
    const timeout = setTimeout(() => finish(), 5000);
    session.on("error", () => finish());
    session.on("close", () => finish());
    session.on("connect", () => {
      const request = session.request({ ":method": "POST", ...headers });
      let status = 0;
      let response = "";
      request.setEncoding("utf8");
      request.on("response", (value) => {
        status = Number(value[":status"]);
      });
      request.on("data", (chunk: string) => {
        response += chunk;
        if (response.length > 4096) finish();
      });
      request.on("error", () => finish());
      request.on("aborted", () => finish());
      request.on("end", () => {
        let reason: string | undefined;
        try {
          const value: unknown = JSON.parse(response);
          if (
            value &&
            typeof value === "object" &&
            "reason" in value &&
            typeof value.reason === "string"
          )
            reason = value.reason;
        } catch {
          /* Successful responses have no body. */
        }
        finish({ status, reason });
      });
      request.end(body);
    });
  });

/** Uses a relay-only Apple key; the Mac and phone never receive provider credentials. */
export function apnsSender(
  config: {
    teamId: string;
    keyId: string;
    topic: string;
    privateKey: string;
    sandbox?: boolean;
  },
  send: ApnsTransport = transport,
) {
  if (
    !/^[A-Z0-9]{10}$/.test(config.teamId) ||
    !/^[A-Z0-9]{10}$/.test(config.keyId) ||
    !/^[A-Za-z0-9.-]+$/.test(config.topic)
  ) {
    throw new Error(
      "Invalid APNs configuration; check team ID, key ID and bundle identifier.",
    );
  }
  const key = createPrivateKey(config.privateKey);
  if (
    key.asymmetricKeyType !== "ec" ||
    key.asymmetricKeyDetails?.namedCurve !== "prime256v1"
  )
    throw new Error("APNs requires an Apple P-256 signing key.");
  let cached: { value: string; created: number } | undefined;
  return async (message: PushMessage): Promise<"ok" | "unregistered"> => {
    if (!/^(?:[a-fA-F0-9]{2}){8,2048}$/.test(message.token))
      throw new Error("Invalid APNs device token; register this phone again.");
    const now = Math.floor(Date.now() / 1000);
    if (!cached || now - cached.created >= 3000) {
      const header = Buffer.from(
        JSON.stringify({ alg: "ES256", kid: config.keyId }),
      ).toString("base64url");
      const claims = Buffer.from(
        JSON.stringify({ iss: config.teamId, iat: now }),
      ).toString("base64url");
      const data = `${header}.${claims}`;
      const signature = sign("sha256", Buffer.from(data), {
        key,
        dsaEncoding: "ieee-p1363",
      }).toString("base64url");
      cached = { value: `${data}.${signature}`, created: now };
    }
    const notification = notificationPayload(message).message.notification;
    const response = await send(
      config.sandbox
        ? "https://api.sandbox.push.apple.com"
        : "https://api.push.apple.com",
      {
        ":path": `/3/device/${message.token}`,
        authorization: `bearer ${cached.value}`,
        "apns-topic": config.topic,
        "apns-push-type": "alert",
        "apns-priority": "10",
        "apns-expiration": String(now + 300),
        "apns-collapse-id": `chauffeur-${message.kind}`,
      },
      JSON.stringify({
        aps: {
          alert: notification,
          ...(message.kind === "approval" ? { sound: "default" } : {}),
        },
        chauffeur: "update",
      }),
    );
    if (response.status === 200) return "ok";
    if (response.status === 410 && response.reason === "Unregistered")
      return "unregistered";
    if (response.reason === "ExpiredProviderToken") cached = undefined;
    // Wrong environment/topic and revoked provider keys must not erase a device grant.
    throw new Error(
      "APNs delivery failed; check relay credentials and signing environment.",
    );
  };
}
