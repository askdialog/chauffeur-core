// Opt-in real ChatGPT account proof. Credentials arrive on stdin, never in transferred state.
import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { spawnAppServer } from "../../src/agents/codex/app-server.ts";
import { silentLogger } from "../../src/log.ts";
import { CODEX_APP_SERVER_VERSION } from "../../src/version.ts";

type Manifest = { sessionId: string; marker: string; compactState: unknown[]; stages: string[] };
const [rootArg, stage] = process.argv.slice(2);
assert(rootArg && stage, "expected isolated root and stage");
const root = resolve(rootArg);
const home = join(root, "home");
const codexHome = join(home, ".codex");
const cwd = join(root, "workspace");
const transfer = join(root, "transfer");
for (const path of [home, codexHome, cwd, transfer]) mkdirSync(path, { recursive: true, mode: 0o700 });
const auth = JSON.parse(await Bun.stdin.text()) as { accessToken: string; chatgptAccountId: string };
assert(auth.accessToken && auth.chatgptAccountId, "missing external account credentials");
const manifestPath = join(transfer, "manifest.json");
const previous = existsSync(manifestPath) ? JSON.parse(readFileSync(manifestPath, "utf8")) as Manifest : undefined;
if (previous) cpSync(join(transfer, "native"), codexHome, { recursive: true });
// Never load personal configs, API keys, MCPs, skills or refresh credentials.
writeFileSync(join(codexHome, "config.toml"), 'model = "gpt-6-luna"\ncli_auth_credentials_store = "ephemeral"\n', { mode: 0o600 });
const marker = previous?.marker ?? `chauffeur-memory-${crypto.randomUUID()}`;
let done: (error?: string) => void = () => {};
let compactEvents = 0;
let answer = "";
const server = spawnAppServer({
  executable: "codex", args: [], cwd, env: { PATH: process.env.PATH, HOME: home, CODEX_HOME: codexHome }, log: silentLogger,
  handlers: {
    notification(message) {
      if (message.method === "item/completed") {
        if (message.params.item.type === "contextCompaction") compactEvents++;
        if (message.params.item.type === "agentMessage") answer += message.params.item.text;
      }
      if (message.method === "turn/completed") done(message.params.turn.status === "completed" ? undefined : "native turn failed");
    },
    async request() { throw new Error("unexpected tool, approval or auth refresh; abort this proof and renew the native login separately"); },
  },
});
const stop = () => { void server.close().finally(() => process.exit(1)); };
process.once("SIGINT", stop);
process.once("SIGTERM", stop);
const deadline = setTimeout(() => { console.error("hosted compaction phase timed out"); stop(); }, 180_000);
try {
  const init = await server.request("initialize", { clientInfo: { name: "chauffeur_hosted_portability", title: "Disposable hosted compaction proof", version: "1" },
    capabilities: { experimentalApi: true, requestAttestation: false, optOutNotificationMethods: [] } });
  assert(init.userAgent.includes(CODEX_APP_SERVER_VERSION), "Codex differs from pinned protocol");
  server.notify("initialized");
  await server.request("account/login/start", { type: "chatgptAuthTokens", ...auth });
  const account = await server.request("account/read", { refreshToken: false });
  assert(account.account?.type === "chatgpt", "expected native ChatGPT account; API key fallback is forbidden");
  const common = { cwd, model: "gpt-6-luna", approvalPolicy: "never" as const, sandbox: "read-only" as const,
    config: { "features.shell_tool": false },
    developerInstructions: "This is a synthetic memory portability test. Never use tools, inspect files, spawn agents or access the network yourself. Answer only the user's memory question." };
  const response = previous
    ? await server.request("thread/resume", { threadId: previous.sessionId, excludeTurns: true, ...common })
    : await server.request("thread/start", common);
  assert(!previous || response.thread.id === previous.sessionId, "native session ID changed");
  const path = response.thread.path;
  assert(path, "missing native rollout");
  const records = () => readFileSync(path, "utf8").trimEnd().split("\n").map((line) => JSON.parse(line) as Record<string, unknown>);
  const compactState = () => records().filter((entry) => entry.type === "compacted");
  if (previous) assert.deepEqual(compactState(), previous.compactState, "compaction state changed before destination inference");
  async function turn(operation: () => Promise<unknown>): Promise<void> {
    answer = "";
    const completed = new Promise<void>((resolve, reject) => { done = (error) => error ? reject(new Error(error)) : resolve(); });
    await operation();
    await Promise.race([completed, server.exited.then(() => { throw new Error("native server exited during turn"); })]);
  }
  const send = (text: string) => turn(() => server.request("turn/start", { threadId: response.thread.id,
    input: [{ type: "text", text, text_elements: [] }], effort: "low" }));
  if (!previous) {
    await send(`Remember this exact identifier for all later memory questions: ${marker}. Reply with only the identifier. It must survive compaction. Do not use tools.`);
    assert(answer.includes(marker), "source model did not repeat the synthetic identifier");
    await turn(() => server.request("thread/compact/start", { threadId: response.thread.id }));
    assert(compactEvents === 1, "native runtime did not report exactly one source compaction");
    const state = compactState();
    assert(state.length === 1, "missing unique persisted compaction record");
    assert(hasOpaqueCompaction(state), "source used a text summary instead of an encrypted remote compaction item");
  }
  // The identifier appears only in source history, never in any destination prompt or workspace file.
  await send("What exact identifier did I ask you to remember? Reply with only that identifier. Do not use tools.");
  assert(answer.includes(marker), "model did not recall the source identifier after compaction/resume");
  await server.close();
  const state = compactState();
  if (previous) {
    assert.deepEqual(state, previous.compactState, "source opaque compaction state changed on destination");
    assert(compactEvents === 0, "destination silently recompacted the session");
  }
  assert(!records().some((entry) => entry.type === "response_item" && hasToolCall(entry)), "model used tools in memory-only proof");
  assert(!existsSync(join(codexHome, "auth.json")), "external login unexpectedly persisted credentials");
  const rel = relative(codexHome, path);
  assert(rel.startsWith("sessions/") && !rel.includes(".."), "rollout escaped selected session scope");
  mkdirSync(dirname(join(transfer, "native", rel)), { recursive: true, mode: 0o700 });
  cpSync(path, join(transfer, "native", rel));
  const manifest: Manifest = { sessionId: response.thread.id, marker, compactState: state, stages: [...previous?.stages ?? [], stage] };
  writeFileSync(manifestPath, JSON.stringify(manifest), { mode: 0o600 });
  console.log(JSON.stringify({ stage, platform: process.platform, sessionId: manifest.sessionId,
    opaqueCompactionPreserved: true, memoryProbePassed: true, compactions: compactEvents, auth: "external-chatgpt-no-refresh", version: CODEX_APP_SERVER_VERSION }));
} finally {
  clearTimeout(deadline);
  await server.close();
  process.removeListener("SIGINT", stop);
  process.removeListener("SIGTERM", stop);
}

function hasOpaqueCompaction(value: unknown): boolean {
  if (!value || typeof value !== "object") return false;
  const item = value as Record<string, unknown>;
  return (item.type === "compaction" && typeof item.encrypted_content === "string" && item.encrypted_content.length > 0)
    || Object.values(item).some(hasOpaqueCompaction);
}

function hasToolCall(value: unknown): boolean {
  if (!value || typeof value !== "object") return false;
  const item = value as Record<string, unknown>;
  return (typeof item.type === "string" && ["function_call", "custom_tool_call", "web_search_call", "local_shell_call"].includes(item.type))
    || Object.values(item).some(hasToolCall);
}
