import { describe, expect, test } from "bun:test";
import type { LoginSessionInfo } from "@chauffeur/protocol";
import { sessionStateText } from "./sessions";

const base: LoginSessionInfo = {
  name: "aws",
  label: "AWS SSO (staging)",
  repo: "app",
  state: "holds",
  expires_at: null,
  checked_at: null,
  logging_in: false,
};
const now = Date.parse("2026-11-18T10:00:00Z");

describe("sessionStateText", () => {
  test("says when a session that holds ends, in minutes then hours", () => {
    expect(sessionStateText({ ...base, expires_at: "2026-11-18T10:40:00Z" }, now)).toBe("Ends in 40 min");
    expect(sessionStateText({ ...base, expires_at: "2026-11-18T13:00:00Z" }, now)).toBe("Ends in 3 h");
    expect(sessionStateText(base, now)).toBe("Signed in");
  });

  test("says a running login over the state it replaces", () => {
    expect(sessionStateText({ ...base, state: "expired", logging_in: true }, now)).toBe("Login running…");
    expect(sessionStateText({ ...base, state: "expired" }, now)).toBe("Expired");
  });
});
