import { readFile } from "node:fs/promises";
import { apnsSender } from "./apns";
import { fcmSender } from "./fcm";
import { mistralSessions } from "./mistral";
import { startRelay } from "./server";

const adminToken = process.env.RELAY_ADMIN_TOKEN;
if (!adminToken)
  throw new Error("Set RELAY_ADMIN_TOKEN before starting the relay");
const port = Number(process.env.PORT ?? "41240");
if (!Number.isInteger(port) || port < 0 || port > 65535)
  throw new Error("Invalid PORT");
let emitted = 0;
let suppressed = 0;
function log(event: object) {
  // Bound volume under connection floods. No request objects or payloads.
  if (emitted++ < 200)
    console.info(JSON.stringify({ ts: new Date().toISOString(), ...event }));
  else suppressed++;
}
const summary = setInterval(() => {
  console.info(
    JSON.stringify({
      ts: new Date().toISOString(),
      event: "relay_health",
      uptimeSeconds: Math.floor(process.uptime()),
      rssBytes: process.memoryUsage().rss,
      suppressed,
    }),
  );
  emitted = 0;
  suppressed = 0;
}, 60_000);
summary.unref();
const fcm = process.env.FCM_PROJECT
  ? fcmSender(process.env.FCM_PROJECT)
  : undefined;
const appleFields = [
  process.env.APNS_TEAM_ID,
  process.env.APNS_KEY_ID,
  process.env.APNS_KEY_FILE,
  process.env.APNS_TOPIC,
];
if (appleFields.some(Boolean) && !appleFields.every(Boolean))
  throw new Error(
    "Set APNS_TEAM_ID, APNS_KEY_ID, APNS_KEY_FILE and APNS_TOPIC together.",
  );
if (
  process.env.APNS_ENVIRONMENT &&
  !["production", "sandbox"].includes(process.env.APNS_ENVIRONMENT)
)
  throw new Error("APNS_ENVIRONMENT must be production or sandbox.");
const apns = appleFields.every(Boolean)
  ? apnsSender({
      teamId: process.env.APNS_TEAM_ID!,
      keyId: process.env.APNS_KEY_ID!,
      topic: process.env.APNS_TOPIC!,
      privateKey: await readFile(process.env.APNS_KEY_FILE!, "utf8"),
      sandbox: process.env.APNS_ENVIRONMENT === "sandbox",
    })
  : undefined;
const relay = startRelay({
  adminToken,
  port,
  hostname: process.env.RELAY_HOST ?? "127.0.0.1",
  telemetry: log,
  voice: process.env.MISTRAL_API_KEY ? mistralSessions(process.env.MISTRAL_API_KEY) : undefined,
  push:
    fcm || apns
      ? async (message) => {
          const sender = message.provider === "apns" ? apns : fcm;
          if (!sender)
            throw new Error(
              "Push provider unavailable; configure the relay before enabling notifications.",
            );
          return sender(message);
        }
      : undefined,
});
log({ event: "relay_started", port: relay.server.port });
process.on("SIGTERM", () => relay.stop());
process.on("SIGINT", () => relay.stop());
