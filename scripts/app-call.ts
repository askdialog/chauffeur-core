// One call to the daemon the app's way, for the end-to-end scripts: over its WebSocket on
// 127.0.0.1 with the token, where `chat.shell` is taken (the Unix socket the CLI uses refuses it).
// Usage: bun scripts/app-call.ts <method> '<params as JSON>'. Prints the result as JSON; an error
// goes to stderr with exit code 1.
import { readFileSync } from "node:fs";
import { join } from "node:path";

const home = process.env.CHAUFFEUR_HOME;
if (!home) throw new Error("CHAUFFEUR_HOME is not set");
const [method, params = "{}"] = process.argv.slice(2);
const { port } = JSON.parse(readFileSync(join(home, "daemon.json"), "utf8"));
const token = readFileSync(join(home, "token"), "utf8").trim();
const socket = new WebSocket(`ws://127.0.0.1:${port}/ws?token=${encodeURIComponent(token)}`);
socket.onopen = () => socket.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: JSON.parse(params) }));
socket.onmessage = (event) => {
  const reply = JSON.parse(String(event.data));
  if (reply.id !== 1) return;
  if (reply.error) {
    console.error(reply.error.message);
    process.exit(1);
  }
  console.log(JSON.stringify(reply.result));
  process.exit(0);
};
socket.onerror = () => {
  console.error(`could not reach the daemon on port ${port}`);
  process.exit(1);
};
