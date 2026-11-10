import { expect, test } from "bun:test";
import { AbstractMessageStream } from "@libp2p/utils";
import type { Uint8ArrayList } from "uint8arraylist";
import { JsonChannel } from "../src/channel";
import { silentLog } from "../src/log";
class Stream extends AbstractMessageStream {
  frames: Uint8Array[] = [];
  constructor() { super({ log: silentLog }); }
  sendData(data: Uint8ArrayList) { this.frames.push(data.subarray().slice()); return { sentBytes: data.byteLength, canSendMore: true }; }
  sendReset() {} sendPause() {} sendResume() {}
  async close() { this.onTransportClosed(); }
}
test("framed JSON survives every byte boundary", async () => {
  const sending = new Stream(); const sender = new JsonChannel(sending);
  sender.send({ type: "approval_required" }); sender.send({ type: "result", value: "été 🛠️" });
  for (let i = 0; i < 100 && sending.frames.length < 2; i++) await Bun.sleep(1);
  const receiving = new Stream(); const receiver = new JsonChannel(receiving);
  const messages: unknown[] = []; receiver.onmessage = (message) => messages.push(message);
  for (const frame of sending.frames) for (const byte of frame) receiving.onData(Uint8Array.of(byte));
  expect(messages).toEqual([{ type: "approval_required" }, { type: "result", value: "été 🛠️" }]);
  expect(receiver.failure).toBeNull(); sender.close(); receiver.close();
});
test("rejects a length exceeding the bounded application frame", () => {
  const receiving = new Stream(); const receiver = new JsonChannel(receiving);
  receiving.onData(Uint8Array.of(127, 255, 255, 255));
  expect(receiver.failure).toBe("channel_frame");
});
