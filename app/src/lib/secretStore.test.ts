import { expect, test } from "bun:test";
import { secretStore } from "./secretStore";

test("credentials are said to be in the Keychain on macOS and in the system keyring elsewhere", () => {
  expect(secretStore(false, true)).toBe("the Keychain");
  expect(secretStore(true, true)).toBe("le trousseau");
  expect(secretStore(false, false)).toBe("the system keyring");
  expect(secretStore(true, false)).toBe("le trousseau du système");
});
