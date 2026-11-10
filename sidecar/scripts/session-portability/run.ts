import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import type { ClaudeContextBundle } from "../../src/agents/claude/context-bundle.ts";
import type { CodexContextBundle } from "../../src/agents/codex/context-bundle.ts";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { CLAUDE_AGENT_SDK_VERSION, CODEX_APP_SERVER_VERSION } from "../../src/version.ts";

if (process.argv.includes("--help")) {
  console.log("Run on macOS: bun scripts/session-portability/run.ts [--scenario text|tools|compact|fork|all]\nNeeds Docker Linux, Bun and pinned codex. No real credentials or model calls. KEEP=1 retains synthetic artifacts.");
  process.exit(0);
}
assert(process.platform === "darwin", "this proof must start on macOS and resume in Docker Linux");
const { values } = parseArgs({ args: process.argv.slice(2), options: { scenario: { type: "string", default: "all" } } });
assert(["text", "tools", "compact", "fork", "all"].includes(values.scenario!), "expected --scenario text|tools|compact|fork|all");
const scenarios = values.scenario === "all" ? ["text", "tools", "compact", "fork"] : [values.scenario!];
const sidecar = resolve(import.meta.dir, "../..");
const source = join(import.meta.dir, "phase.ts");
const root = realpathSync(mkdtempSync(join(tmpdir(), "chauffeur-session-portability-")));
const context = join(root, "image");
const image = `chauffeur-portability:${CODEX_APP_SERVER_VERSION}-${process.pid}`;
const container = `chauffeur-portability-${process.pid}-${Date.now()}`;
const abort = new AbortController();
const stop = () => abort.abort();
process.once("SIGINT", stop);
process.once("SIGTERM", stop);
console.log(`Disposable proof: ${root}`);
try {
  const preflight = join(root, "preflight");
  mkdirSync(join(preflight, ".codex"), { recursive: true, mode: 0o700 });
  const version = await command(["codex", "--version"], sidecar, true, isolatedEnvironment(preflight));
  assert(version.trim() === `codex-cli ${CODEX_APP_SERVER_VERSION}`, `install codex ${CODEX_APP_SERVER_VERSION} first`);
  assert((await command(["docker", "info", "--format", "{{.OSType}}"], sidecar)).trim() === "linux", "Docker must run Linux");
  mkdirSync(context);
  // Docker receives a minimal generated context, never the repo, personal homes or credentials.
  const build = await Bun.build({ entrypoints: [source], target: "bun", packages: "external" });
  assert(build.success && build.outputs[0], build.logs.join("\n"));
  writeFileSync(join(context, "phase.js"), await build.outputs[0].text());
  writeFileSync(join(context, "package.json"), JSON.stringify({ private: true, type: "module", dependencies: {
    "@anthropic-ai/claude-agent-sdk": CLAUDE_AGENT_SDK_VERSION, "@openai/codex": CODEX_APP_SERVER_VERSION,
  } }));
  cpSync(join(import.meta.dir, "Dockerfile"), join(context, "Dockerfile"));
  console.log("Preparing pinned Linux runtimes (local Docker image)…");
  await command(["docker", "build", "-q", "-t", image, context], sidecar, false);
  for (const scenario of scenarios) for (const agent of scenario === "fork" ? ["codex"] : ["claude", "codex"]) {
    let previous: string | undefined;
    for (const stage of ["mac-source", "linux", "mac-return"]) {
      const phaseRoot = join(root, scenario, agent, stage);
      const bootstrap = join(phaseRoot, "bootstrap");
      mkdirSync(bootstrap, { recursive: true, mode: 0o700 });
      if (previous) cpSync(join(previous, "transfer"), join(phaseRoot, "transfer"), { recursive: true });
      console.log(`== ${scenario} / ${agent}: ${stage}`);
      if (stage === "linux") {
        await command(["docker", "run", "--rm", "--name", container, "--network=none",
          "--mount", `type=bind,source=${phaseRoot},target=/state`,
          "-e", "HOME=/state/bootstrap", "-e", "CLAUDE_CONFIG_DIR=/state/bootstrap/.claude", "-e", "CODEX_HOME=/state/bootstrap/.codex",
          image, agent, "/state", stage, scenario], sidecar, false);
      } else {
        await command([process.execPath, "run", source, agent, phaseRoot, stage, scenario], sidecar, false, isolatedEnvironment(bootstrap));
      }
      previous = phaseRoot;
    }
    const manifest = JSON.parse(readFileSync(join(previous!, "transfer", `${agent}.json`), "utf8")) as { prompts: string[] };
    assert(manifest.prompts.length === 3, "round trip did not complete");
    if (scenario === "compact") await rejectMissingCompaction(agent, previous!);
    if (scenario === "fork") await rejectMissingHistoryBase(previous!);
  }
  console.log(`PASS (${scenarios.join(", ")}): native session IDs, history and selected fixture files survived Mac → Linux → Mac.`);
  console.log("Scope: deterministic inference only; not proof of account portability, compaction quality or complete workspace handoff.");
} finally {
  // Stop only this run's container, including interruption during a phase.
  const cleanup = Bun.spawn(["docker", "rm", "-f", container], { stdout: "ignore", stderr: "ignore" });
  await cleanup.exited;
  const removeImage = Bun.spawn(["docker", "image", "rm", image], { stdout: "ignore", stderr: "ignore" });
  await removeImage.exited;
  process.removeListener("SIGINT", stop);
  process.removeListener("SIGTERM", stop);
  if (process.env.KEEP === "1") console.log(`Synthetic artifacts retained: ${root}`);
  else rmSync(root, { recursive: true, force: true });
}

async function command(args: string[], cwd: string, capture = true, env?: Record<string, string>): Promise<string> {
  const proc = Bun.spawn(args, { cwd, env, stdin: "ignore", stdout: capture ? "pipe" : "inherit", stderr: "inherit", signal: abort.signal });
  const output = proc.stdout ? new Response(proc.stdout).text() : Promise.resolve("");
  const [code, text] = await Promise.all([proc.exited, output]);
  assert(code === 0, `${args[0]} failed (exit ${code})`);
  return text;
}

function isolatedEnvironment(home: string): Record<string, string> {
  assert(process.env.PATH, "PATH is required to locate the native runtimes");
  return { PATH: process.env.PATH, HOME: home, CLAUDE_CONFIG_DIR: join(home, ".claude"), CODEX_HOME: join(home, ".codex") };
}

async function rejectMissingCompaction(agent: string, previous: string): Promise<void> {
  const phaseRoot = join(root, "compact", agent, "missing-compaction");
  const bootstrap = join(phaseRoot, "bootstrap");
  mkdirSync(bootstrap, { recursive: true, mode: 0o700 });
  cpSync(join(previous, "transfer"), join(phaseRoot, "transfer"), { recursive: true });
  const store = join(phaseRoot, "transfer");
  if (agent === "claude") {
    const path = join(store, "claude-context.json");
    const bundle = JSON.parse(readFileSync(path, "utf8")) as ClaudeContextBundle;
    const file = bundle.files.find((file) => file.path === `${bundle.sessionId}.jsonl`)!;
    const entries = Buffer.from(file.content, "base64").toString("utf8").trimEnd().split("\n").map((line) => JSON.parse(line) as Record<string, unknown>);
    const damaged = entries.filter((entry) => !(entry.type === "system" && entry.subtype === "compact_boundary") && entry.isCompactSummary !== true);
    assert(damaged.length < entries.length, "negative fixture did not remove any Claude compaction records");
    const bytes = Buffer.from(damaged.map((entry) => JSON.stringify(entry)).join("\n") + "\n");
    file.content = bytes.toString("base64");
    file.sha256 = createHash("sha256").update(bytes).digest("hex");
    writeFileSync(path, JSON.stringify(bundle));
  } else {
    const path = join(store, "codex-context.json");
    const bundle = JSON.parse(readFileSync(path, "utf8")) as CodexContextBundle;
    assert(bundle.files.length === 1, "expected one selected compacted rollout");
    const selected = bundle.files[0]!;
    const entries = Buffer.from(selected.content, "base64").toString("utf8").trimEnd().split("\n").map((line) => JSON.parse(line) as Record<string, unknown>);
    const damaged = entries.filter((entry) => entry.type !== "compacted");
    assert(damaged.length < entries.length, "negative fixture did not remove any Codex compaction records");
    const bytes = Buffer.from(damaged.map((entry) => JSON.stringify(entry)).join("\n") + "\n");
    selected.content = bytes.toString("base64");
    selected.sha256 = createHash("sha256").update(bytes).digest("hex");
    writeFileSync(path, JSON.stringify(bundle));
  }
  const proc = Bun.spawn([process.execPath, "run", source, agent, phaseRoot, "missing-compaction", "compact"], {
    cwd: sidecar, env: isolatedEnvironment(bootstrap), stdin: "ignore", stdout: "pipe", stderr: "pipe", signal: abort.signal,
  });
  const [code, stdout, stderr] = await Promise.all([proc.exited, new Response(proc.stdout).text(), new Response(proc.stderr).text()]);
  writeFileSync(join(phaseRoot, "expected-failure.log"), stdout + stderr);
  assert(code !== 0 && /missing persisted native compaction records|did not persist compacted state|lost the source's native compacted summary|native compaction records changed/.test(stderr),
    `expected compaction-integrity refusal, got exit ${code}: ${stderr}`);
  assert(!existsSync(join(phaseRoot, "provider-request.json")), "damaged compaction state reached inference");
  console.log(`PASS: ${agent} rejects a transferred fixture with missing compaction state.`);
}

async function rejectMissingHistoryBase(previous: string): Promise<void> {
  const phaseRoot = join(root, "fork", "codex", "missing-parent");
  const bootstrap = join(phaseRoot, "bootstrap");
  mkdirSync(bootstrap, { recursive: true, mode: 0o700 });
  cpSync(join(previous, "transfer"), join(phaseRoot, "transfer"), { recursive: true });
  const path = join(phaseRoot, "transfer", "codex-context.json");
  const bundle = JSON.parse(readFileSync(path, "utf8")) as CodexContextBundle;
  assert(bundle.files.length > 1, "expected a real native history_base dependency");
  bundle.files.pop();
  writeFileSync(path, JSON.stringify(bundle));
  const proc = Bun.spawn([process.execPath, "run", source, "codex", phaseRoot, "missing-parent", "fork"], {
    cwd: sidecar, env: isolatedEnvironment(bootstrap), stdin: "ignore", stdout: "pipe", stderr: "pipe", signal: abort.signal,
  });
  const [code, stdout, stderr] = await Promise.all([proc.exited, new Response(proc.stdout).text(), new Response(proc.stderr).text()]);
  writeFileSync(join(phaseRoot, "expected-failure.log"), stdout + stderr);
  assert(code !== 0 && stderr.includes("Codex context is incomplete or unsafe"), `expected missing-parent refusal, got exit ${code}`);
  assert(!existsSync(join(phaseRoot, "provider-request.json")), "incomplete parent history reached inference");
  console.log("PASS: Codex rejects a fork with a missing parent rollout before inference.");
}
