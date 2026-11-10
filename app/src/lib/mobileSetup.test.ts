import { expect, test } from "bun:test";
import { mobileSetupHint, relaySetupMissing } from "./mobileSetup";

const relay = "wss://relay.example";

test("without a saved credential the hint asks for it and says where the QR code shows", () => {
  const status = { enabled: false, relay, key_saved: false };
  expect(relaySetupMissing(status)).toBe(true);
  expect(mobileSetupHint(status)).toBe(
    "To turn mobile access on, enter the operator credential your relay's operator gave you under Relay configuration, then choose Save and enable. The QR code then shows in the window Connect phone opens.",
  );
});

test("without a relay either the hint asks for both", () => {
  const status = { enabled: false, relay: "", key_saved: false };
  expect(relaySetupMissing(status)).toBe(true);
  expect(mobileSetupHint(status)).toStartWith("To turn mobile access on, enter the relay address and the operator credential ");
});

test("a configured relay only needs mobile access turned on, and needs nothing once it is on", () => {
  expect(relaySetupMissing({ enabled: false, relay, key_saved: true })).toBe(false);
  expect(mobileSetupHint({ enabled: false, relay, key_saved: true })).toBe(
    "Turn mobile access on. The QR code then shows in the window Connect phone opens.",
  );
  expect(mobileSetupHint({ enabled: true, relay, key_saved: true })).toBeNull();
});
