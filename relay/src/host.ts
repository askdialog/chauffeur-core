// Packaged daemon child. Credentials arrive only on stdin, never command arguments.
import { createInterface } from "node:readline";
import { setHostCredential } from "./host-credential";

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity })[Symbol.asyncIterator]();
const { value: input } = await lines.next();
if (typeof input !== "string" || input.length > 8192) throw new Error("Invalid mobile host configuration");
const config = JSON.parse(input);
const initialCredential = config.managed === true ? config.credential : config.adminToken;
if (typeof config.relay !== "string" || typeof initialCredential !== "string" || !/^[a-f0-9]{64}$/.test(config.controlToken)) throw new Error("Invalid mobile host configuration");
const valid = (value: string) => config.managed === true
  ? value.length <= 4089 && /^[\w-]+\.[\w-]+\.[\w-]+$/.test(value)
  : /^[A-Za-z0-9_-]{43,128}$/.test(value);
if (!valid(initialCredential)) throw new Error("Invalid mobile host credential");
process.env.RELAY_ORIGIN = config.relay;
process.env.CHAUFFEUR_HOST_CONTROL_TOKEN = config.controlToken;
setHostCredential(initialCredential);
if (config.managed === true) {
  void (async () => {
    for await (const value of { [Symbol.asyncIterator]: () => lines }) {
      setHostCredential(valid(value) ? value : undefined);
    }
    setHostCredential(undefined);
  })();
}
await import("./phone-test");
