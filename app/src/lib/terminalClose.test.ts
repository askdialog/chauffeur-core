import { describe, expect, test } from "bun:test";
import { closeToConfirm } from "./terminalClose";

const terminals = [
  { id: "a", title: "web" },
  { id: "b", title: "" },
  { id: "c", title: "api" },
];
const label = (t: { title: string }, index: number) => t.title || `Terminal ${index + 1}`;
const idle = { running: false, unseen: false };
const busy = { running: true, unseen: false };

describe("closing terminals", () => {
  test("closes at once when no shell among them runs a command", () => {
    expect(closeToConfirm(terminals, ["a"], { a: idle, b: busy }, label)).toBeNull();
    expect(closeToConfirm(terminals, ["a", "c"], {}, label)).toBeNull();
  });

  test("asks first, naming in tab order those whose command it would stop", () => {
    expect(closeToConfirm(terminals, ["a"], { a: busy }, label)).toEqual({ ids: ["a"], running: ["web"] });
    expect(closeToConfirm(terminals, ["c", "b", "a"], { a: idle, b: busy, c: busy }, label)).toEqual({
      ids: ["c", "b", "a"],
      running: ["Terminal 2", "api"],
    });
  });
});
