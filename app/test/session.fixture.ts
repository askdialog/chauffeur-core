// Isolated process: the app store and browser mocks must not affect the other tests.
import assert from "node:assert/strict";
import { mock } from "bun:test";
import type { EventEnvelope, SubscribeParams } from "@chauffeur/protocol";
const subscriptions: Array<{params: SubscribeParams; receive: (e: EventEnvelope) => void; closed: boolean}> = [];
const store = Object.assign(() => {}, {
  getState: () => ({ workspaces: { workspace: { moved_to: "worker" } } }),
  subscribe: () => () => {},
});
mock.module("../src/state/app", () => ({
  useApp: store, forgetFileIndex: () => {}, reviewerCommented: () => {},
  client: {
    call: async () => ({ terminals: [] }),
    subscribe: (params: SubscribeParams, receive: (e: EventEnvelope) => void) => {
      const record = { params, receive, closed: false }; subscriptions.push(record);
      return {close: () => {record.closed = true;}};
    },
  },
}));
Object.assign(globalThis, {window: {addEventListener: () => {}, removeEventListener: () => {}}});
const {WorkspaceSession} = await import("../src/state/session");
const session = new WorkspaceSession("workspace");
assert.equal(subscriptions.length, 1, "do not open older history before the tail cutoff is known");
assert.equal(subscriptions[0].params.replay?.part, "tail", "remote history uses staged replay too");
const event = (seq: number): EventEnvelope => ({seq, ts:"2026-11-06T00:00:00Z", event:{scope:"chat",workspace_id:"workspace",chat_id:"chat",event:{type:"user_message",text:String(seq),queued:false,attachments:[]}}});
const replayed: EventEnvelope = {seq:null,ts:"",event:{scope:"workspace",workspace_id:"workspace",event:{type:"replayed"}}};
const tail = subscriptions[0];
for(let seq=101;seq<=103;seq++) tail.receive(event(seq));
tail.receive(replayed);
assert.equal(subscriptions.length, 2);
assert.equal(subscriptions[1].params.replay?.part, "before_tail");
assert.equal(session.state.partial, true);
// More live events advance the server's tail cutoff before the second request arrives.
for(let seq=104;seq<=105;seq++) tail.receive(event(seq));
const head=subscriptions[1];
for(let seq=1;seq<=102;seq++) head.receive(event(seq));
head.receive(replayed);
await Bun.sleep(50);
assert.equal(session.state.partial, false);
assert.deepEqual(session.state.chats.chat.items.filter(item=>item.kind==='user').map(item=>item.text), Array.from({length:105},(_,i)=>String(i+1)));
assert(head.closed);
session.moved(null);
assert(tail.closed);
assert.equal(session.state.partial,true);
assert.equal(subscriptions[2].params.after_seq,0);
session.close();
console.log("Remote tail/head replay is complete and deduplicated; moving resets its cursor.");
