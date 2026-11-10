import type { MessageStream } from "@libp2p/interface";

// Noise exposes a byte stream, not application-message boundaries. Length prefix
// every JSON message so large history responses and coalesced events stay intact.
export class JsonChannel {
  private buffer = new Uint8Array(0);
  private closed = false;
  private queue: Uint8Array[] = [];
  private queuedBytes = 0;
  private pumping = false;
  onmessage: (message: unknown) => void = () => {};
  onclose: () => void = () => {};
  failure: string | null = null;
  constructor(private stream: MessageStream) {
    stream.addEventListener("message", (event) => {
      if (this.closed) return;
      let stage = "frame";
      try {
        const chunk = event.data.subarray();
        const next = new Uint8Array(this.buffer.length + chunk.length);
        next.set(this.buffer); next.set(chunk, this.buffer.length); this.buffer = next;
        while (this.buffer.length >= 4) {
          const size = new DataView(this.buffer.buffer, this.buffer.byteOffset, 4).getUint32(0);
          if (!size || size > 1024 * 1024) throw new Error("Invalid message size");
          if (this.buffer.length < size + 4) break;
          stage = "json";
          const message: unknown = JSON.parse(new TextDecoder().decode(this.buffer.subarray(4, size + 4)));
          this.buffer = this.buffer.slice(size + 4);
          stage = "handler";
          this.onmessage(message);
          if (this.closed) return;
          stage = "frame";
        }
      } catch { this.failure = `channel_${stage}`; this.close(); }
    });
    stream.addEventListener("close", (event) => {
      if (!this.failure && event.error) this.failure = `stream_${/^[A-Za-z]{1,60}$/.test(event.error.name) ? event.error.name : "error"}`;
      this.finish();
    }, { once: true });
    if (["closed", "aborted", "reset"].includes(stream.status)) queueMicrotask(() => this.finish());
  }
  send(message: unknown) {
    if (this.closed) throw new Error("Connection closed");
    const body = new TextEncoder().encode(JSON.stringify(message));
    if (!body.length || body.length > 1024 * 1024) throw new Error("Message exceeds session limit");
    const frame = new Uint8Array(body.length + 4);
    new DataView(frame.buffer).setUint32(0, body.length); frame.set(body, 4);
    if (this.queuedBytes + frame.length > 2 * 1024 * 1024) { this.close(); throw new Error("Connection is too slow"); }
    this.queue.push(frame); this.queuedBytes += frame.length;
    void this.pump();
  }
  private async pump() {
    if (this.pumping) return;
    this.pumping = true;
    try {
      while (this.queue.length && !this.closed) {
        const frame = this.queue[0];
        for (let offset = 0; offset < frame.length && !this.closed; offset += 16 * 1024) {
          if (!this.stream.send(frame.subarray(offset, offset + 16 * 1024))) await this.stream.onDrain({ signal: AbortSignal.timeout(10_000) });
          // Let the WebSocket flush between chunks, including on Hermes where
          // the native bridge updates bufferedAmount asynchronously.
          await new Promise((resolve) => setTimeout(resolve, 0));
        }
        if (!this.closed) { this.queue.shift(); this.queuedBytes -= frame.length; }
      }
    } catch { this.failure = "channel_write"; this.close(); }
    finally { this.pumping = false; }
  }
  close() { if (!this.closed) { this.finish(); void this.stream.close(); } }
  private finish() {
    if (this.closed) return;
    this.closed = true; this.buffer = new Uint8Array(0); this.queue = []; this.queuedBytes = 0; this.onclose();
  }
}
