import { expect, test } from "bun:test";
import type { ExtensionInfo, IdentityState } from "@chauffeur/protocol";
import { signInBannerShown, visibleExtensions } from "./members";

const extension = (id: "office" | "database"): ExtensionInfo => ({ id, name: id, description: "", publisher: "Chauffeur", version: "1", enabled: true, capabilities: [], package: null });
const identity = (change: Partial<IdentityState>): IdentityState => ({ email: null, team_domain: null, member: false, can_sign_in: true, ...change });

test("outside members the Office is not offered, and members get every extension", () => {
  const all = [extension("database"), extension("office")];
  expect(visibleExtensions(all, false).map((e) => e.id)).toEqual(["database"]);
  expect(visibleExtensions(all, true).map((e) => e.id)).toEqual(["database", "office"]);
});

test("the sign-in banner shows to a signed-out user who can sign in, until it is done", () => {
  expect(signInBannerShown(identity({}), false)).toBe(true);
  expect(signInBannerShown(identity({}), true)).toBe(false);
  expect(signInBannerShown(identity({ email: "a@b.com" }), false)).toBe(false);
  expect(signInBannerShown(identity({ can_sign_in: false }), false)).toBe(false);
  // Before the daemon answered, nothing is known yet.
  expect(signInBannerShown(null, false)).toBe(false);
});
