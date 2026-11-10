// One host phase of the opt-in native session portability proof. No real account or daemon.
import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { query, type SessionStoreEntry } from "@anthropic-ai/claude-agent-sdk";
import { spawnAppServer } from "../../src/agents/codex/app-server.ts";
import { silentLogger } from "../../src/log.ts";
import { CODEX_APP_SERVER_VERSION, CLAUDE_AGENT_SDK_VERSION } from "../../src/version.ts";
import { captureCodexContext, restoreCodexContext } from "../../src/agents/codex/context-bundle.ts";
import { readThreadHistory } from "../../src/agents/codex/history.ts";
import { ClaudeProcess } from "../../src/agents/claude/process.ts";
import { captureClaudeContext, restoreClaudeContext, claudeProjectKey, claudeMainRecords } from "../../src/agents/claude/context-bundle.ts";
import { fixtureProvider } from "./provider.ts";
import { shellQuote, toolEvidence, type FixtureTool, type ToolEvidence } from "./tools.ts";

type Manifest = { agent: string; sessionId: string; marker: string; prompts: string[]; answers: string[]; tools: ToolEvidence[]; fileContent?: string; compactSummary?: string; compactState?: unknown[] };
const [agent, rootArg, stage, scenario = "text"] = process.argv.slice(2);
assert(agent === "claude" || agent === "codex", "expected claude or codex");
assert(rootArg && stage, "expected disposable root and stage");
assert(["text", "tools", "compact", "fork"].includes(scenario), "expected text, tools, compact or fork scenario");
const root = realpathSync(resolve(rootArg));
const state = join(root, "transfer");
const home = join(root, `home-${stage}`);
const cwd = join(root, `workspace-${stage}`);
for (const dir of [state, home, cwd]) mkdirSync(dir, { recursive: true, mode: 0o700 });
const manifestPath = join(state, `${agent}.json`);
const previous = existsSync(manifestPath) ? JSON.parse(readFileSync(manifestPath, "utf8")) as Manifest : undefined;
const marker = previous?.marker ?? `private-history-${crypto.randomUUID()}-été`;
const prompt = previous ? `Continue the conversation on ${stage}.` : `Remember this exact value: ${marker}`;
const answer = `native-answer-${stage}`;
const compactSummary = scenario === "compact" ? previous?.compactSummary ?? `Native compacted state ${crypto.randomUUID()} retains ${marker}.` : undefined;
const compactProbePrompt = "Continue after native compaction.";
let compactState: unknown[] | undefined;
const file = join(cwd, "fixture.txt");
const fileContent = scenario === "tools" ? `${previous?.fileContent ?? ""}file-${stage}-${crypto.randomUUID()}-été\n` : undefined;
if (previous?.fileContent) {
  cpSync(join(state, "workspace-files"), cwd, { recursive: true });
  assert.equal(readFileSync(file, "utf8"), previous.fileContent, "workspace file was not restored exactly");
}
const tools: FixtureTool[] = [];
if (fileContent) {
  const command = (cmd: string) => ({ code: `text(await tools.exec_command(${JSON.stringify({ cmd, workdir: cwd, login: false, max_output_tokens: 2000 })}));` });
  const read = (expected: string): FixtureTool => ({ id: `call_${crypto.randomUUID()}`, name: agent === "claude" ? "Read" : "exec",
    input: agent === "claude" ? { file_path: file } : command("cat fixture.txt"), expectedRead: expected });
  if (previous?.fileContent) tools.push(read(previous.fileContent));
  tools.push({ id: `call_${crypto.randomUUID()}`, name: agent === "claude" ? "Write" : "exec",
    input: agent === "claude" ? { file_path: file, content: fileContent }
      : command(`printf '%s' ${shellQuote(fileContent)} > fixture.txt`) });
  tools.push(read(fileContent));
}
const provider = fixtureProvider(answer, tools, (body) => writeFileSync(join(root, "provider-request.json"), JSON.stringify(body), { mode: 0o600 }));
const env = {
  PATH: process.env.PATH, HOME: home, TMPDIR: join(home, "tmp"),
  CLAUDE_CONFIG_DIR: join(home, ".claude"), CODEX_HOME: join(home, ".codex"),
  ANTHROPIC_API_KEY: "portability-fixture-not-a-real-key", ANTHROPIC_BASE_URL: provider.url,
  DISABLE_NONESSENTIAL_TRAFFIC: "1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
};
mkdirSync(env.TMPDIR, { recursive: true });
const deadline = setTimeout(() => { console.error("native portability phase timed out"); process.exit(1); }, 90_000);
try {
  const sessionId = agent === "claude" ? await claude() : await codex();
  assert(provider.requests.length > 0, "runtime never called the fixture provider");
  // Only inference history counts: the new prompt and fixture file do not contain the source conversation marker.
  for (const request of provider.requests) {
    const history = JSON.stringify(request);
    let cursor = 0;
    for (let index = 0; index < (previous?.prompts.length ?? 0); index++) {
      for (const text of [previous!.prompts[index]!, previous!.answers[index]!]) {
        const position = history.indexOf(JSON.stringify(text).slice(1, -1), cursor);
        assert(position >= 0, "lost or reordered an earlier user/assistant message");
        cursor = position + JSON.stringify(text).length - 2;
      }
    }
    assert(history.includes(marker), "lost random source-only marker");
    if (previous?.compactSummary) assert(history.includes(previous.compactSummary), "lost the source's native compacted summary");
    const nativeTools = toolEvidence(agent, request);
    assert.deepEqual(nativeTools.slice(0, previous?.tools.length ?? 0), previous?.tools ?? [], "lost, reordered or changed previous native tool calls/results");
  }
  assert(!previous || sessionId === previous.sessionId, "resume replaced the native session ID");
  const evidence = toolEvidence(agent, provider.requests.at(-1));
  if (compactSummary) assert(JSON.stringify(provider.requests.at(-1)).includes(compactSummary), "runtime did not load the compacted summary into active context");
  assert.equal(evidence.length, (previous?.tools.length ?? 0) + tools.length, "missing or duplicate native tool results");
  for (const tool of tools) assert(evidence.some((entry) => entry.id === tool.id), "missing completed tool call");
  if (fileContent) {
    assert.equal(readFileSync(file, "utf8"), fileContent, "native tool did not persist its file change");
    mkdirSync(join(state, "workspace-files"), { recursive: true });
    cpSync(file, join(state, "workspace-files", "fixture.txt"));
  }
  const manifest: Manifest = { agent, sessionId, marker, prompts: [...previous?.prompts ?? [], compactSummary && !previous ? compactProbePrompt : prompt],
    answers: [...previous?.answers ?? [], answer], tools: evidence, fileContent, compactSummary, compactState };
  writeFileSync(manifestPath, JSON.stringify(manifest), { mode: 0o600 });
  console.log(JSON.stringify({ agent, scenario, stage, platform: process.platform, sessionId, preservedTurns: manifest.prompts.length, preservedToolResults: evidence.length,
    compactedContext: !!compactSummary, compactionRequests: provider.compactRequests.length,
    sdk: agent === "claude" ? CLAUDE_AGENT_SDK_VERSION : CODEX_APP_SERVER_VERSION }));
} finally {
  clearTimeout(deadline);
  provider.close();
}

function verifyCompactedState(records: Array<Record<string, unknown>>): void {
  if (!compactSummary) return;
  compactState = records.filter((record) => agent === "claude"
    ? (record.type === "system" && record.subtype === "compact_boundary") || record.isCompactSummary === true
    : record.type === "compacted");
  assert(compactState.length > 0, "missing persisted native compaction records");
  if (previous) {
    assert(previous.compactState?.length, "missing source compaction reference");
    assert.deepEqual(compactState, previous.compactState, "native compaction records changed across handoff");
  }
}

async function claude(): Promise<string> {
  const file = join(state, "claude-context.json");
  const projects = join(env.CLAUDE_CONFIG_DIR, "projects");
  mkdirSync(projects, { recursive: true, mode: 0o700 });
  const key = await claudeProjectKey(cwd);
  let id = previous?.sessionId;
  let entries: SessionStoreEntry[] = [];
  let compacted = false;
  if (previous) {
    const bundle: unknown = JSON.parse(readFileSync(file, "utf8"));
    entries = claudeMainRecords(bundle) as SessionStoreEntry[];
    await restoreClaudeContext(bundle, projects, key);
  }
  async function consume(input: string, expectAnswer = true): Promise<void> {
    const native = new ClaudeProcess(() => {});
    const session = query({ prompt: input, options: {
      cwd, env, model: "claude-haiku-4-5-20251001", tools: scenario === "tools" ? ["Read", "Write"] : [], settingSources: [], maxTurns: 8,
      permissionMode: "acceptEdits",
      spawnClaudeCodeProcess: native.spawn, ...(id ? { resume: id } : {}),
    } });
    try {
      let result = false;
      for await (const message of session) {
        if (message.type === "system" && message.subtype === "compact_boundary") compacted = true;
        if (message.type === "result") {
          assert(message.subtype === "success", `Claude result: ${JSON.stringify(message)}`);
          if (expectAnswer) assert(message.result === answer, "fixture response did not complete");
          id = message.session_id;
          result = true;
        }
      }
      assert(result && id, "Claude returned no successful session");
    } finally {
      session.close();
      await native.stop(2_000);
    }
    assert(id, "missing native Claude ID");
    const bundle = await captureClaudeContext(projects, key, id);
    entries = claudeMainRecords(bundle) as SessionStoreEntry[];
    assert(JSON.stringify(entries).includes(answer), "Claude did not persist the completed answer");
    writeFileSync(file, JSON.stringify(bundle), { mode: 0o600 });
  }
  if (previous?.compactSummary) verifyCompactedState(entries);
  await consume(prompt);
  if (compactSummary && !previous) {
    provider.setCompactionSummary(compactSummary);
    await consume("/compact", false);
    assert(compacted && provider.compactRequests.length > 0, "Claude did not perform native compaction");
    provider.setCompactionSummary(undefined);
    await consume(compactProbePrompt);
    assert(JSON.stringify(entries).includes(compactSummary), "Claude did not persist its compacted state");
  }
  assert(id, "missing Claude session ID");
  verifyCompactedState(entries);
  return id;
}

async function codex(): Promise<string> {
  const native = join(state, "codex-context.json");
  if (previous) await restoreCodexContext(JSON.parse(readFileSync(native, "utf8")), env.CODEX_HOME);
  else mkdirSync(env.CODEX_HOME, { recursive: true });
  writeFileSync(join(env.CODEX_HOME, "config.toml"), `model = "gpt-6-luna"
model_provider = "portability"
[model_providers.portability]
name = "local portability fixture"
base_url = "${provider.url}/v1"
wire_api = "responses"
requires_openai_auth = false
`, { mode: 0o600 });
  let completed: (error?: string) => void = () => {};
  let compacted = false;
  const server = spawnAppServer({
    executable: process.env.CHAUFFEUR_PORTABILITY_CODEX ?? "codex", args: [], cwd, env, log: silentLogger,
    handlers: {
      notification(message) {
        if (message.method === "turn/completed") completed(message.params.turn.status === "completed" ? undefined : message.params.turn.error?.message ?? "native turn did not complete");
        if (message.method === "item/completed" && message.params.item.type === "contextCompaction") compacted = true;
      },
      async request() { throw new Error("no approvals expected for synthetic fixture tools"); },
    },
  });
  try {
    const init = await server.request("initialize", { clientInfo: { name: "chauffeur_portability", title: "Chauffeur portability proof", version: "1" },
      capabilities: { experimentalApi: false, requestAttestation: false, optOutNotificationMethods: [] } });
    assert(init.userAgent.includes(CODEX_APP_SERVER_VERSION), "Codex differs from pinned protocol");
    server.notify("initialized");
    const common = { cwd, approvalPolicy: "never" as const, sandbox: scenario === "tools" ? "danger-full-access" as const : "read-only" as const, model: "gpt-6-luna" };
    let response = previous
      ? await server.request("thread/resume", { threadId: previous.sessionId, excludeTurns: true, ...common })
      : await server.request("thread/start", common);
    if (previous?.compactSummary) {
      assert(response.thread.path, "missing restored native rollout");
      verifyCompactedState(readFileSync(response.thread.path, "utf8").trimEnd().split("\n").map((line) => JSON.parse(line) as Record<string, unknown>));
    }
    async function runTurn(operation: () => Promise<unknown>): Promise<void> {
      const turnDone = new Promise<void>((resolve, reject) => { completed = (error) => error ? reject(new Error(error)) : resolve(); });
      await operation();
      await Promise.race([turnDone, server.exited.then((reason) => { throw new Error(`Codex stopped: ${reason}`); })]);
    }
    const send = (text: string) => runTurn(() => server.request("turn/start", { threadId: response.thread.id, input: [{ type: "text", text, text_elements: [] }] }));
    await send(prompt);
    if (compactSummary && !previous) {
      provider.setCompactionSummary(compactSummary);
      await runTurn(() => server.request("thread/compact/start", { threadId: response.thread.id }));
      assert(compacted && provider.compactRequests.length > 0, "Codex did not perform native compaction");
      provider.setCompactionSummary(undefined);
      await send(compactProbePrompt);
    }
    if (scenario === "fork" && !previous) {
      response = await server.request("thread/fork", { threadId: response.thread.id, excludeTurns: true, ...common });
    }
    await server.close();
    // First establish the real on-disk layout. Only synthetic history stores are exported, no auth/config/home.
    const path = response.thread.path;
    assert(path, "Codex did not create persistent history");
    const transcript = readFileSync(path, "utf8");
    assert(JSON.stringify(readThreadHistory(path)).includes(answer), "Codex did not persist the completed answer");
    if (compactSummary) assert(transcript.includes(compactSummary), "Codex did not persist compacted state");
    verifyCompactedState(transcript.trimEnd().split("\n").map((line) => JSON.parse(line) as Record<string, unknown>));
    const bundle = await captureCodexContext(env.CODEX_HOME, path, response.thread.id);
    if (scenario === "fork") assert(bundle.files.length > 1, "fork proof must exercise a real native history_base dependency");
    writeFileSync(native, JSON.stringify(bundle), { mode: 0o600 });
    return response.thread.id;
  } finally {
    await server.close();
  }
}
