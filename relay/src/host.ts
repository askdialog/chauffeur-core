// Packaged daemon child. Credentials arrive only on stdin, never command arguments.
const input = await Bun.stdin.text();
if (input.length > 8192) throw new Error("Invalid mobile host configuration");
const config = JSON.parse(input);
if (typeof config.relay !== "string" || typeof config.adminToken !== "string" || !/^[a-f0-9]{64}$/.test(config.controlToken)) throw new Error("Invalid mobile host configuration");
process.env.RELAY_ORIGIN = config.relay;
process.env.RELAY_ADMIN_TOKEN = config.adminToken;
process.env.CHAUFFEUR_HOST_CONTROL_TOKEN = config.controlToken;
await import("./phone-test");
export {};
