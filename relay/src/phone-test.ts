import { pairingHtml, pairingScript } from "./pairing-page";
// Local host. --daemon enables durable trust owned by chauffeurd, with explicit
// Mac approval after Noise authentication. This client never writes credentials.
import QRCode from "qrcode";
import {
  generateIdentity, joinRelay, peerIdFromPrivateKey, randomToken,
  securePairing, JsonChannel, relayUrl, type Invitation,
} from "@chauffeur/remote";
import { startRelay } from "./server";
import { createBridge } from "./bridge";
import { TrustedHost } from "./trusted-host";

function portFromEnvironment(key: string, fallback: number) {
  const port = Number(process.env[key] ?? fallback);
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error(`Invalid ${key}`);
  return port;
}
const pagePort = portFromEnvironment("CHAUFFEUR_PAIRING_PAGE_PORT", 41241);
const localPort = portFromEnvironment("CHAUFFEUR_LOCAL_RELAY_PORT", 41240);
const externalRelay = process.env.RELAY_ORIGIN;
const relayOrigin = externalRelay ? relayUrl(externalRelay).origin : `ws://127.0.0.1:${localPort}`;
const adminToken = externalRelay ? process.env.RELAY_ADMIN_TOKEN : randomToken();
if (!adminToken || !/^[A-Za-z0-9_-]{43,128}$/.test(adminToken)) throw new Error("A valid RELAY_ADMIN_TOKEN is required for the external relay");
const relay = externalRelay ? undefined : startRelay({ adminToken, port: localPort, diagnostic: process.argv.includes("--diagnose") ? (message) => console.info(`[relay] ${message}`) : undefined });
const relayHttpOrigin = relayOrigin.replace(/^ws/, "http");
const origin = `http://127.0.0.1:${pagePort}`;
let svg = "";
let status = "Generate a code when Chauffeur is open on your Pixel.";
let generation = 0;
let cancel = () => {};
let lastIssued = 0;
let expiresAt = 0;
const daemonMode = process.argv.includes("--daemon");
const trusted = daemonMode ? new TrustedHost({ relay: relayOrigin, adminToken }) : undefined;
trusted?.start();
let bridge: ReturnType<typeof createBridge> | undefined;
let peer = "";
let authorized = false;
let paired = false;

async function invite() {
  if (Date.now() - lastIssued < 2000) throw new Error("Please wait before generating another code");
  lastIssued = Date.now();
  cancel();
  bridge = undefined; peer = ""; authorized = false; paired = false;
  const current = ++generation;
  const controller = new AbortController();
  let close = () => {};
  let endReason = "transport_closed";
  cancel = () => { controller.abort(); close(); };
  svg = "";
  status = "Preparing your code…";
  const identity = trusted ? await trusted.hostIdentity() : await generateIdentity("Ed25519");
  const response = await fetch(`${relayHttpOrigin}/v1/rooms`, {
    method: "POST", headers: { Authorization: `Bearer ${adminToken}` },
    signal: controller.signal,
  });
  if (!response.ok) throw new Error("Could not create invitation");
  const room = await response.json() as { room: string; hostToken: string; phoneToken: string; expiresAt: number };
  expiresAt = room.expiresAt;
  const invitation: Invitation = {
    version: 1, relay: relayOrigin, room: room.room,
    phoneToken: room.phoneToken, hostPeerId: peerIdFromPrivateKey(identity).toString(),
    pairingKey: randomToken(), expiresAt: room.expiresAt,
  };
  svg = await QRCode.toString(JSON.stringify(invitation), {
    type: "svg", width: 480, margin: 4, errorCorrectionLevel: "M",
  });
  status = "Scan this code in Chauffeur on your Pixel. It expires in five minutes.";
  void (async () => {
    const timer = setTimeout(() => controller.abort(), 5 * 60_000);
    try {
      const raw = await joinRelay(invitation.relay, room.room, "host", room.hostToken, undefined, controller.signal);
      close = () => { void raw.close(); };
      const secure = await securePairing(raw, identity, invitation, "host", controller.signal);
      close = () => { void secure.connection.close(); };
      if (current !== generation) { close(); return; }
      secure.connection.addEventListener("close", () => {
        if (current === generation) { svg = ""; peer = ""; authorized = false; bridge = undefined; status = paired ? "Phone paired. It reconnects automatically; revoke its access below." : `Session ended (${endReason}). Generate a new code to try again.`; }
      }, { once: true });
      if (daemonMode) {
        const channel = new JsonChannel(secure.connection);
        bridge = createBridge(channel, undefined, (reason) => {
          endReason = reason;
          console.info(`[remote] Session closed: ${reason}`);
        });
        peer = secure.remotePeer.toString();
        svg = "";
        status = "Phone authenticated. Compare its identity below with your phone, then approve access.";
        return;
      }
      const channel = new JsonChannel(secure.connection);
      channel.onmessage = (value) => {
        if ((value as { type?: string })?.type !== "check") {
          secure.connection.abort(new Error("Unexpected check"));
          return;
        }
        channel.send({ type: "verified" });
        svg = "";
        status = "Encrypted connection verified with your Pixel.";
        console.info("Encrypted phone connection verified.");
      };
    } catch {
      close();
      if (current === generation) { svg = ""; peer = ""; authorized = false; bridge = undefined; status = "Connection ended or code expired. Generate a fresh code."; }
    } finally {
      clearTimeout(timer);
    }
  })();
}

const html = pairingHtml(daemonMode);
const script = pairingScript;

const page = Bun.serve({
  hostname: "127.0.0.1", port: pagePort,
  async fetch(request) {
    const url = new URL(request.url);
    const headers = {
      "Cache-Control": "no-store",
      "Referrer-Policy": "no-referrer",
      "Cross-Origin-Resource-Policy": "same-origin",
      "Content-Security-Policy": "default-src 'self'; style-src 'unsafe-inline'; script-src 'self'; frame-ancestors 'none'",
    };
    if (url.origin !== origin || request.headers.get("sec-fetch-site") === "cross-site") {
      return new Response("Forbidden", { status: 403, headers });
    }
    if ((url.pathname === "/approve" || url.pathname === "/revoke") && request.method === "POST") {
      if (request.headers.get("origin") !== origin || url.searchParams.get("generation") !== String(generation) || !peer || url.searchParams.get("peer") !== peer) return new Response("Forbidden", { status: 403, headers });
      if (url.pathname === "/revoke") { bridge?.close(); cancel(); return new Response("ok", { headers }); }
      if (!bridge || authorized) return new Response("No pending phone", { status: 409, headers });
      const approvingSession = bridge; const approvingGeneration = generation;
      const approvingPeer = peer;
      try {
        const resume = trusted ? await trusted.approve(approvingPeer, "Phone") : undefined;
        if (bridge !== approvingSession || generation !== approvingGeneration) return new Response("Session changed", { status: 409, headers });
        await approvingSession.approve(resume ? { resume } : {});
        authorized = true; paired = Boolean(resume); status = "Phone approved. It can now reconnect automatically.";
        return new Response("ok", { headers });
      } catch { if (generation === approvingGeneration) status = "Could not connect to the Chauffeur daemon. Generate a fresh code."; return new Response("Daemon unavailable", { status: 503, headers }); }
    }
    if (url.pathname === "/forget" && request.method === "POST" && trusted) {
      if (request.headers.get("origin") !== origin) return new Response("Forbidden", { status: 403, headers });
      const id = url.searchParams.get("device") ?? "";
      if (!trusted.publicDevices().some((device) => device.id === id)) return new Response("Unknown device", { status: 404, headers });
      try { await trusted.revoke(id); return new Response("ok", { headers }); }
      catch { return new Response("Daemon unavailable", { status: 503, headers }); }
    }
    if (url.pathname === "/new" && request.method === "POST") {
      if (request.headers.get("origin") !== origin) return new Response("Forbidden", { status: 403, headers });
      try { await invite(); return new Response("ok", { headers }); }
      catch { status = trusted ? "Start the updated Chauffeur daemon before pairing a phone." : "Could not create code. Try again."; return new Response("Could not create code", { status: 503, headers }); }
    }
    if (request.method !== "GET") return new Response("Not found", { status: 404, headers });
    if (url.pathname === "/") return new Response(html, { headers: { ...headers, "Content-Type": "text/html; charset=utf-8" } });
    if (url.pathname === "/app.js") return new Response(script, { headers: { ...headers, "Content-Type": "application/javascript" } });
    if (url.pathname === "/health") {
      const control = process.env.CHAUFFEUR_HOST_CONTROL_TOKEN;
      if (!control || request.headers.get("x-chauffeur-host") !== control) return new Response("Forbidden", { status: 403, headers });
      return Response.json({ daemonAvailable: Boolean(trusted && !trusted.availabilityError()), connectedDevices: trusted?.publicDevices().filter(device => device.connected).length ?? 0 }, { headers });
    }
    if (url.pathname === "/status") return Response.json({ status: trusted?.availabilityError() ?? status, hasCode: Boolean(svg), generation, expiresAt, peer, awaitingApproval: Boolean(bridge && !authorized), authorized, devices: trusted?.publicDevices() ?? [] }, { headers });
    if (url.pathname === "/qr" && svg) return new Response(svg, { headers: { ...headers, "Content-Type": "image/svg+xml" } });
    return new Response("Not found", { status: 404, headers });
  },
});

console.info(`Local Pixel test ready: ${origin}`);
console.info(externalRelay ? "External TLS relay configured. The pairing page stays on Mac loopback." : "Use wireless ADB to reverse tcp:41240 and the Metro port; no LAN plaintext listener is opened.");
function stop() { cancel(); trusted?.stop(); page.stop(true); relay?.stop(); }
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
