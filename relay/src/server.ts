import type { PushMessage } from "./fcm";
import { timingSafeEqual, createHash } from "node:crypto";
import type { ServerWebSocket } from "bun";

const MAX_FRAME = 128 * 1024;
const MAX_BUFFER = 256 * 1024;
const ROOM_LIFETIME = 5 * 60_000;
const DEVICE_LEASE = 24 * 60 * 60_000;
type Role = "host" | "phone";
type SocketData = { trace: string; openedAt: number; forwardedBytes: number; lastReceivedAt: number; roomId: string; role?: Role; timer?: ReturnType<typeof setTimeout> };
type Socket = ServerWebSocket<SocketData>;
type Room = {
  hostToken: string;
  phoneToken: string;
  expiresAt: number;
  host?: Socket;
  phone?: Socket;
  timer?: ReturnType<typeof setTimeout>;
  persistent?: boolean;
  voice?: { window: number; count: number; inflight: boolean };
};

function digest(value: string) { return createHash("sha256").update(value).digest(); }
function matches(value: string, expected: string) { return timingSafeEqual(digest(value), digest(expected)); }
function token(bytes = 32) { return Buffer.from(crypto.getRandomValues(new Uint8Array(bytes))).toString("base64url"); }

export type RelayLog = {
  event: "connection_opened" | "role_authenticated" | "connection_closed" | "forwarding_stopped" | "push_sent" | "push_failed";
  trace: string;
  role?: Role;
  code?: number;
  durationMs?: number;
  forwardedBytes?: number;
  reason?: "invalid_frame" | "peer_unavailable" | "backpressure" | "send_failed" | "peer_timeout" | "push_unregistered" | "push_delivery_failed";
};

export function startRelay(options: { adminToken: string; hostname?: string; port?: number; maxRooms?: number; roomLifetimeMs?: number; diagnostic?: (message: string) => void; telemetry?: (event: RelayLog) => void; peerTimeoutMs?: number; voice?: (signal: AbortSignal) => Promise<string>; voiceTimeoutMs?: number; push?: (message: PushMessage) => Promise<"ok"|"unregistered"> }) {
  if (!/^[A-Za-z0-9_-]{43,128}$/.test(options.adminToken)) {
    throw new Error("RELAY_ADMIN_TOKEN must be a randomly generated token of at least 32 bytes");
  }
  const voiceTimeout = options.voiceTimeoutMs ?? 10_000;
  if (!Number.isInteger(voiceTimeout) || voiceTimeout < 1 || voiceTimeout > 10_000) throw new Error("Invalid voice timeout");
  const voicePending = new Set<AbortController>();
  let voiceWindow = 0;
  let voiceCount = 0;
  const rooms = new Map<string, Room>();
  const pushRates = new Map<string, number>();
  let pushWindow = 0; let pushCount = 0;
  const pending = new Set<Socket>();
  const all = new Set<Socket>();
  const maxRooms = options.maxRooms ?? 100;
  const roomLifetime = options.roomLifetimeMs ?? ROOM_LIFETIME;
  if (!Number.isInteger(roomLifetime) || roomLifetime < 1 || roomLifetime > ROOM_LIFETIME) {
    throw new Error("Room lifetime must not exceed five minutes");
  }

  // Track incoming application traffic only. Outbound data and native WS pings
  // can keep a dead mobile TCP connection alive for minutes after a handover.
  const peerTimeout = options.peerTimeoutMs ?? 10_000;
  if (!Number.isFinite(peerTimeout) || peerTimeout < 1) throw new Error("Invalid peer timeout");
  const watchdog = setInterval(() => {
    for (const room of rooms.values()) {
      if (!room.persistent || !room.host || !room.phone) continue;
      for (const socket of [room.host, room.phone]) {
        if (Date.now() - socket.data.lastReceivedAt > peerTimeout) {
          options.telemetry?.({ event: "forwarding_stopped", trace: socket.data.trace, role: socket.data.role, reason: "peer_timeout" });
          disconnectRoom(socket.data.roomId);
          break;
        }
      }
    }
  }, Math.min(1000, peerTimeout));
  watchdog.unref();

  function endRoom(id: string, reason = "session_ended") {
    const room = rooms.get(id);
    if (!room) return;
    options.diagnostic?.(`${reason} after ${Date.now() - (room.expiresAt - roomLifetime)}ms`);
    rooms.delete(id);
    clearTimeout(room.timer);
    room.host?.close(1000, "session ended");
    room.phone?.close(1000, "session ended");
  }

  function disconnectRoom(id: string) {
    const room = rooms.get(id);
    if (!room?.persistent) { endRoom(id); return; }
    const host = room.host; const phone = room.phone;
    room.host = undefined; room.phone = undefined;
    host?.close(1000, "session ended"); phone?.close(1000, "session ended");
  }

  function isAdmin(request: Request) {
    const auth = request.headers.get("authorization") ?? "";
    return auth.length <= 256 && matches(auth, `Bearer ${options.adminToken}`);
  }

  const server = Bun.serve<SocketData>({
    hostname: options.hostname ?? "127.0.0.1",
    port: options.port ?? 41240,
    maxRequestBodySize: 8192,
    async fetch(request, server) {
      const url = new URL(request.url);
      const headers = { "Cache-Control": "no-store" };
      if (url.pathname === "/health" && request.method === "GET") return new Response("ok", { headers });
      if (url.pathname === "/v1/rooms" && request.method === "POST") {
        if (!isAdmin(request)) {
          return new Response("Unauthorized", { status: 401, headers });
        }
        if (rooms.size >= maxRooms) return new Response("Relay capacity reached", { status: 503, headers });
        const id = token(16);
        const hostToken = token();
        const phoneToken = token();
        const expiresAt = Date.now() + roomLifetime;
        const key = `rooms/${id}`;
        const timer = setTimeout(() => endRoom(key), roomLifetime);
        timer.unref();
        rooms.set(key, { hostToken, phoneToken, expiresAt, timer });
        return Response.json({ room: id, hostToken, phoneToken, expiresAt }, { status: 201, headers });
      }
      if (url.pathname === "/v1/devices" && request.method === "POST") {
        if (!isAdmin(request)) return new Response("Unauthorized", { status: 401, headers });
        try {
          const registration = await request.json() as { deviceId?: unknown; hostToken?: unknown; phoneToken?: unknown };
          if (typeof registration.deviceId !== "string" || !/^[A-Za-z0-9_-]{22}$/.test(registration.deviceId) ||
              typeof registration.hostToken !== "string" || !/^[A-Za-z0-9_-]{43}$/.test(registration.hostToken) ||
              typeof registration.phoneToken !== "string" || !/^[A-Za-z0-9_-]{43}$/.test(registration.phoneToken)) throw new Error();
          const key = `devices/${registration.deviceId}`;
          const previous = rooms.get(key);
          if (!previous && rooms.size >= maxRooms) return new Response("Relay capacity reached", { status: 503, headers });
          const unchanged = previous && matches(previous.hostToken, registration.hostToken) && matches(previous.phoneToken, registration.phoneToken);
          if (!unchanged && previous) endRoom(key);
          if (unchanged) clearTimeout(previous.timer);
          const expiresAt = Date.now() + DEVICE_LEASE;
          const timer = setTimeout(() => endRoom(key), DEVICE_LEASE); timer.unref();
          // Renewal retains the same authorization generation and its mint budget.
          if (unchanged) Object.assign(previous, { expiresAt, timer });
          else rooms.set(key, { hostToken: registration.hostToken, phoneToken: registration.phoneToken, expiresAt, timer, persistent: true });
          return Response.json({ registered: true }, { headers });
        } catch { return new Response("Invalid registration", { status: 400, headers }); }
      }
      const voiceMatch = /^\/v1\/devices\/([A-Za-z0-9_-]{22})\/voice-session$/.exec(url.pathname);
      if (voiceMatch && request.method === "POST") {
        if (url.search || request.body !== null) return new Response("Voice session takes no parameters", { status: 400, headers });
        const key = `devices/${voiceMatch[1]}`;
        const room = rooms.get(key);
        const auth = request.headers.get("authorization") ?? "";
        const authorized = () => room?.persistent && rooms.get(key) === room && room.expiresAt > Date.now() &&
          auth.length <= 256 && matches(auth, `Bearer ${room.phoneToken}`) &&
          !matches(auth, `Bearer ${room.hostToken}`) && !matches(auth, `Bearer ${options.adminToken}`);
        if (!room || !authorized()) return new Response("Unauthorized", { status: 401, headers });
        const host = room.host;
        const phone = room.phone;
        const connected = () => host && phone && room.host === host && room.phone === phone &&
          host.readyState === 1 && phone.readyState === 1 &&
          Date.now() - host.data.lastReceivedAt <= peerTimeout && Date.now() - phone.data.lastReceivedAt <= peerTimeout;
        if (!connected()) return new Response("Connect the phone and Mac before dictating", { status: 409, headers });
        if (!options.voice) return new Response("Voice unavailable", { status: 503, headers });
        const now = Date.now();
        const budget = room.voice ??= { window: now, count: 0, inflight: false };
        if (now - budget.window >= 60_000) { budget.window = now; budget.count = 0; }
        if (now - voiceWindow >= 60_000) { voiceWindow = now; voiceCount = 0; }
        if (budget.inflight || budget.count >= 6 || voiceCount >= 60 || voicePending.size >= 4) {
          return new Response("Try voice again later", { status: 429, headers: { ...headers, "Retry-After": "60" } });
        }
        budget.count++; voiceCount++; budget.inflight = true;
        const controller = new AbortController();
        voicePending.add(controller);
        const timeout = setTimeout(() => controller.abort(), voiceTimeout);
        try {
          // The race also bounds injected providers that do not implement AbortSignal.
          const cancelled = new Promise<never>((_, reject) => controller.signal.addEventListener("abort", () => reject(new Error("Voice timeout")), { once: true }));
          const token = await Promise.race([options.voice(controller.signal), cancelled]);
          if (!authorized()) return new Response("Unauthorized", { status: 401, headers });
          if (!connected()) return new Response("Reconnect the phone and Mac before dictating", { status: 409, headers });
          return new Response(token, { headers: { ...headers, "Content-Type": "text/plain; charset=utf-8" } });
        } catch {
          return new Response("Voice provider unavailable", { status: controller.signal.aborted ? 504 : 502, headers });
        } finally {
          clearTimeout(timeout);
          voicePending.delete(controller);
          budget.inflight = false;
        }
      }
      const pushMatch = /^\/v1\/devices\/([A-Za-z0-9_-]{22})\/push$/.exec(url.pathname);
      if (pushMatch && request.method === "POST" && !url.search) {
        const room = rooms.get(`devices/${pushMatch[1]}`);
        const credential = request.headers.get("authorization")?.replace(/^Bearer /, "") ?? "";
        if (!room?.persistent || room.expiresAt <= Date.now() || !matches(credential, room.hostToken)) return new Response("Unauthorized", {status:401,headers});
        if (!options.push) return new Response("Push unavailable", {status:503,headers});
        const now=Date.now();if(now-pushWindow>60_000){pushWindow=now;pushCount=0;pushRates.clear();}
        if(pushCount>=120 || now-(pushRates.get(pushMatch[1])??0)<1000)return new Response("Try later",{status:429,headers});
        try {
          const value=await request.json() as PushMessage;
          if((value.provider !== undefined && !["fcm", "apns"].includes(value.provider)) || (value.provider === "apns" && !/^(?:[a-fA-F0-9]{2}){8,2048}$/.test(value.token)) || typeof value.token!=="string" || !/^[A-Za-z0-9_:-]{16,4096}$/.test(value.token) || !["approval","completed","error","idle"].includes(value.kind) || !["fr","en"].includes(value.language) || (value.workspace!==undefined && (typeof value.workspace!=="string" || value.workspace.length>80 || /[\u0000-\u001f\u007f]/.test(value.workspace)))) return new Response("Invalid push",{status:400,headers});
          // Only allowlisted fields leave the relay. Never forward arbitrary request payloads.
          pushRates.set(pushMatch[1],now);pushCount++;
          const result=await options.push({provider:value.provider ?? "fcm",token:value.token,kind:value.kind,language:value.language,workspace:value.workspace});
          options.telemetry?.({event:result==="unregistered"?"push_failed":"push_sent",trace:crypto.randomUUID(),...(result==="unregistered"?{reason:"push_unregistered" as const}:{})});
          return new Response("ok",{status:result==="unregistered"?410:200,headers});
        } catch { options.telemetry?.({event:"push_failed",trace:crypto.randomUUID(),reason:"push_delivery_failed"}); return new Response("Push unavailable",{status:503,headers}); }
      }
      const match = /^\/v1\/(rooms|devices)\/([A-Za-z0-9_-]{22})$/.exec(url.pathname);
      const key = match ? `${match[1]}/${match[2]}` : "";
      if (request.method === "DELETE" && match?.[1] === "devices") {
        if (!isAdmin(request)) return new Response("Unauthorized", { status: 401, headers });
        endRoom(key); return new Response("ok", { headers });
      }
      if (request.method === "GET" && match && !url.search) {
        if (all.size >= 256 || !rooms.has(key)) return new Response("Unavailable", { status: 404, headers });
        if (server.upgrade(request, { data: { roomId: key, trace: crypto.randomUUID(), openedAt: Date.now(), lastReceivedAt: Date.now(), forwardedBytes: 0 } })) return;
      }
      return new Response("Not found", { status: 404, headers });
    },
    websocket: {
      maxPayloadLength: MAX_FRAME,
      idleTimeout: 30,
      perMessageDeflate: false,
      backpressureLimit: MAX_BUFFER,
      closeOnBackpressureLimit: true,
      open(socket) {
        options.telemetry?.({ event: "connection_opened", trace: socket.data.trace });
        all.add(socket);
        pending.add(socket);
        socket.data.timer = setTimeout(() => socket.close(1008, "authentication required"), 5000);
        socket.data.timer.unref();
      },
      message(socket, data) {
        socket.data.lastReceivedAt = Date.now();
        const room = rooms.get(socket.data.roomId);
        if (!room || room.expiresAt <= Date.now()) { socket.close(1008, "session expired"); return; }
        if (!socket.data.role) {
          try {
            if (typeof data !== "string" || data.length > 256) throw new Error();
            const auth = JSON.parse(data) as { role?: unknown; token?: unknown };
            const role = auth.role;
            if ((role !== "host" && role !== "phone") || typeof auth.token !== "string" ||
                !matches(auth.token, role === "host" ? room.hostToken : room.phoneToken) || room[role]) {
              throw new Error();
            }
            socket.data.role = role;
            options.telemetry?.({ event: "role_authenticated", trace: socket.data.trace, role });
            room[role] = socket;
            clearTimeout(socket.data.timer);
            pending.delete(socket);
            if (room.host && room.phone) {
              room.host.data.lastReceivedAt = Date.now();
              room.phone.data.lastReceivedAt = Date.now();
              room.host.send('{"type":"ready"}');
              room.phone.send('{"type":"ready"}');
            }
          } catch { socket.close(1008, "authentication failed"); }
          return;
        }
        if (data === '{"type":"ping"}') { socket.send('{"type":"pong"}'); return; }
        // From here on the relay accepts only opaque binary data. It does not
        // parse Noise or agent messages, persist payloads or log request URLs.
        const other = socket.data.role === "host" ? room.phone : room.host;
        if (typeof data === "string" || data.byteLength === 0 || !other || other.getBufferedAmount() > MAX_BUFFER) {
          const reason = typeof data === "string" || data.byteLength === 0 ? "invalid_frame" : !other ? "peer_unavailable" : "backpressure";
          options.telemetry?.({ event: "forwarding_stopped", trace: socket.data.trace, role: socket.data.role, reason });
          disconnectRoom(socket.data.roomId);
          return;
        }
        options.diagnostic?.(`forward ${socket.data.role} ${data.byteLength} bytes`);
        if (other.sendBinary(data) === 0) {
          options.telemetry?.({ event: "forwarding_stopped", trace: socket.data.trace, role: socket.data.role, reason: "send_failed" });
          disconnectRoom(socket.data.roomId);
        } else socket.data.forwardedBytes += data.byteLength;
      },
      close(socket, code) {
        options.telemetry?.({ event: "connection_closed", trace: socket.data.trace, role: socket.data.role,
          code, durationMs: Date.now() - socket.data.openedAt, forwardedBytes: socket.data.forwardedBytes });
        options.diagnostic?.(`close ${socket.data.role ?? "unauthenticated"} code ${code}`);
        all.delete(socket);
        pending.delete(socket);
        clearTimeout(socket.data.timer);
        if (socket.data.role && rooms.get(socket.data.roomId)?.[socket.data.role] === socket) disconnectRoom(socket.data.roomId);
      },
    },
  });

  return {
    server,
    stop() {
      clearInterval(watchdog);
      for (const controller of voicePending) controller.abort();
      for (const id of rooms.keys()) endRoom(id);
      for (const socket of pending) { clearTimeout(socket.data.timer); socket.close(); }
      server.stop(true);
    },
  };
}
