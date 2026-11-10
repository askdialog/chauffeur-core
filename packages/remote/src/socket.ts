import { AbstractMessageStream, type SendResult } from "@libp2p/utils";
import type { Uint8ArrayList } from "uint8arraylist";
import { MAX_FRAME_BYTES, relayUrl } from "./invitation";
import { silentLog } from "./log";

export interface Socket {
  readonly readyState: number;
  readonly bufferedAmount: number;
  binaryType: string;
  onopen: (() => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onerror: (() => void) | null;
  onclose: (() => void) | null;
  send(data: string | Uint8Array): void;
  close(code?: number, reason?: string): void;
}

export class RelayStream extends AbstractMessageStream {
  constructor(private readonly socket: Socket) {
    super({ log: silentLog, maxReadBufferLength: 256 * 1024, maxWriteBufferLength: 256 * 1024 });
  }

  receive(data: unknown) {
    if (!(data instanceof ArrayBuffer) || data.byteLength === 0 || data.byteLength > MAX_FRAME_BYTES) {
      this.abort(new Error("Invalid relay frame"));
      return;
    }
    this.onData(new Uint8Array(data));
  }

  sendData(data: Uint8ArrayList): SendResult {
    if (this.socket.readyState !== 1 || this.socket.bufferedAmount > 256 * 1024) {
      throw new Error("Relay connection is closed or too slow");
    }
    // Noise can coalesce packets, so split the transport buffer without touching
    // Noise framing. The receiver treats WebSocket payloads as a byte stream.
    const bytes = data.subarray();
    for (let offset = 0; offset < bytes.length; offset += MAX_FRAME_BYTES) {
      this.socket.send(bytes.subarray(offset, offset + MAX_FRAME_BYTES));
    }
    return { sentBytes: bytes.byteLength, canSendMore: true };
  }

  sendReset() { this.socket.close(1000, "session ended"); }
  sendPause() {}
  sendResume() {}
  async close() {
    this.socket.close(1000, "session ended");
    this.onTransportClosed();
  }
}

export function joinRelay(
  origin: string,
  room: string,
  role: "host" | "phone",
  token: string,
  createSocket: (url: string) => Socket = (url) => new WebSocket(url) as unknown as Socket,
  signal: AbortSignal = AbortSignal.timeout(60_000),
  endpoint: "rooms" | "devices" | "channels" = "rooms",
): Promise<RelayStream> {
  const url = relayUrl(origin);
  url.pathname = `/v1/${endpoint}/${encodeURIComponent(room)}`;
  return new Promise((resolve, reject) => {
    const socket = createSocket(url.toString());
    socket.binaryType = "arraybuffer";
    const stream = new RelayStream(socket);
    let ready = false;
    let heartbeat: ReturnType<typeof setInterval> | undefined;
    let pongTimeout: ReturnType<typeof setTimeout> | undefined;
    const fail = () => {
      clearInterval(heartbeat);
      clearTimeout(pongTimeout);
      if (!ready) reject(new Error("Relay unavailable, invitation expired, or device already connected"));
      stream.onTransportClosed(new Error("Relay disconnected"));
      socket.close();
      signal.removeEventListener("abort", fail);
    };
    if (signal.aborted) { fail(); return; }
    signal.addEventListener("abort", fail, { once: true });
    socket.onopen = () => socket.send(JSON.stringify({ role, token }));
    socket.onmessage = (event) => {
      if (!ready) {
        if (event.data !== '{"type":"ready"}') { fail(); return; }
        ready = true;
        if (endpoint !== "rooms") heartbeat = setInterval(() => {
          try {
            // One outstanding deadline. A later ping must not overwrite it.
            if (pongTimeout) return;
            socket.send('{"type":"ping"}');
            pongTimeout = setTimeout(fail, 3_000);
          } catch { fail(); }
        }, 2_000);
        signal.removeEventListener("abort", fail);
        resolve(stream);
      } else {
        if (event.data === '{"type":"pong"}') { clearTimeout(pongTimeout); pongTimeout = undefined; }
        else stream.receive(event.data);
      }
    };
    socket.onerror = fail;
    socket.onclose = fail;
  });
}
