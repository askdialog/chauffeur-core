import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { UserItem } from "@/lib/chat";
import { ChatRowView, SendNowContext } from "./ChatItems";

const message = (queued: boolean): UserItem => ({
  kind: "user",
  key: "u1",
  ts: "2026-11-03T16:52:59Z",
  depth: 0,
  text: "dis moi où on en est",
  queued,
  attachments: [],
  messageId: "m1",
});

const render = (item: UserItem, sendNow: (() => void) | null) =>
  renderToStaticMarkup(
    <SendNowContext.Provider value={sendNow}>
      <ChatRowView row={item} last working readOnly={false} respond={async () => {}} />
    </SendNowContext.Provider>,
  );

test("a message steering the turn offers to send it now where the chat can be interrupted", () => {
  expect(render(message(true), () => {})).toContain('data-testid="send-now"');
  expect(render(message(true), null)).not.toContain('data-testid="send-now"');
  expect(render(message(false), () => {})).not.toContain('data-testid="send-now"');
});
