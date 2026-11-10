// Separate opt-in runner: real native subscription inference, no API-key fallback or credential export.
import assert from "node:assert/strict";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { CODEX_APP_SERVER_VERSION } from "../../src/version.ts";

if (process.argv.includes("--help")) {
  console.log("Opt-in hosted Codex compaction proof: bun scripts/session-portability/hosted-run.ts --use-chatgpt-account\nUses existing file-backed ChatGPT login quota and real OpenAI requests. No refresh token is exported. Requires Docker network access. All generated state is deleted, including on failure.");
  process.exit(0);
}
assert(process.argv.slice(2).join(" ") === "--use-chatgpt-account", "explicit --use-chatgpt-account is required; this test uses real subscription quota");
assert(process.platform === "darwin", "start this round trip on macOS");
const sourceHome = process.env.CODEX_HOME ?? join(homedir(), ".codex");
const login = JSON.parse(readFileSync(join(sourceHome, "auth.json"), "utf8")) as {
  auth_mode?: string; OPENAI_API_KEY?: string; tokens?: { access_token?: string; account_id?: string };
};
assert(login.auth_mode === "chatgpt" && !login.OPENAI_API_KEY, "requires native ChatGPT login; no API key fallback");
const accessToken = login.tokens?.access_token;
const chatgptAccountId = login.tokens?.account_id;
assert(accessToken && chatgptAccountId, "native account has no file-backed access token/account ID");
const claims = JSON.parse(Buffer.from(accessToken.split(".")[1]!, "base64url").toString()) as { exp?: number };
assert(claims.exp && claims.exp * 1000 > Date.now() + 10 * 60_000, "access token expires too soon; renew native login separately, then rerun");
const credentials = JSON.stringify({ accessToken, chatgptAccountId });
const root = realpathSync(mkdtempSync(join(tmpdir(), "chauffeur-hosted-portability-")));
const sidecar = resolve(import.meta.dir, "../..");
const image = `chauffeur-hosted-portability:${CODEX_APP_SERVER_VERSION}-${process.pid}`;
const container = `chauffeur-hosted-portability-${process.pid}`;
const abort = new AbortController();
const stop = () => abort.abort();
const deadline = setTimeout(stop, 10 * 60_000);
process.once("SIGINT", stop);
process.once("SIGTERM", stop);
console.log("Real-account proof: disposable conversation, ChatGPT subscription quota, Mac → Linux → Mac.");
try {
  const bootstrap = join(root, "preflight");
  mkdirSync(bootstrap, { mode: 0o700 });
  const environment = { PATH: process.env.PATH!, HOME: bootstrap, CODEX_HOME: join(bootstrap, ".codex") };
  assert((await command(["codex", "--version"], environment)).trim() === `codex-cli ${CODEX_APP_SERVER_VERSION}`, "wrong Codex version");
  assert((await command(["docker", "info", "--format", "{{.OSType}}"])).trim() === "linux", "Docker must run Linux");
  const context = join(root, "image");
  mkdirSync(context);
  const build = await Bun.build({ entrypoints: [join(import.meta.dir, "hosted-phase.ts")], target: "bun" });
  assert(build.success && build.outputs[0], "could not bundle hosted phase");
  writeFileSync(join(context, "phase.js"), await build.outputs[0].text());
  writeFileSync(join(context, "package.json"), JSON.stringify({ private: true, dependencies: { "@openai/codex": CODEX_APP_SERVER_VERSION } }));
  cpSync(join(import.meta.dir, "Dockerfile"), join(context, "Dockerfile"));
  await command(["docker", "build", "-q", "-t", image, context]);
  let previous: string | undefined;
  for (const stage of ["mac-source", "linux", "mac-return"]) {
    const phase = join(root, stage);
    mkdirSync(phase, { mode: 0o700 });
    if (previous) cpSync(join(previous, "transfer"), join(phase, "transfer"), { recursive: true });
    console.log(`== hosted Codex: ${stage}`);
    const args = stage === "linux"
      ? ["docker", "run", "--rm", "-i", "--name", container, "--mount", `type=bind,source=${phase},target=/state`, image, "/state", stage]
      : [process.execPath, "run", join(import.meta.dir, "hosted-phase.ts"), phase, stage];
    const output = await command(args, environment, credentials);
    // Never forward arbitrary native diagnostics to logs; only the phase's safe structured evidence.
    const evidence = JSON.parse(output.trim()) as { stage: string; opaqueCompactionPreserved: boolean };
    assert(evidence.stage === stage && evidence.opaqueCompactionPreserved, "phase evidence missing");
    console.log(JSON.stringify(evidence));
    assertNoCredentialFiles(phase);
    previous = phase;
  }
  console.log("PASS: native hosted opaque compaction accepted on Linux and Mac return; same session ID and successful synthetic memory probes. No credential refresh/persistence proof.");
} finally {
  for (const args of [["docker", "rm", "-f", container], ["docker", "image", "rm", image]]) {
    await Bun.spawn(args, { stdout: "ignore", stderr: "ignore" }).exited;
  }
  rmSync(root, { recursive: true, force: true });
  process.removeListener("SIGINT", stop);
  process.removeListener("SIGTERM", stop);
  clearTimeout(deadline);
}

async function command(args: string[], env?: Record<string, string>, input?: string): Promise<string> {
  const proc = Bun.spawn(args, { cwd: sidecar, env, stdin: input === undefined ? "ignore" : "pipe", stdout: "pipe", stderr: "pipe", signal: abort.signal });
  if (input !== undefined) { (proc.stdin as import("bun").FileSink).write(input); (proc.stdin as import("bun").FileSink).end(); }
  const [code, output, errors] = await Promise.all([proc.exited, new Response(proc.stdout).text(), new Response(proc.stderr).text()]);
  if (code !== 0) {
    // Access token/account ID must never appear in diagnostics, even for a native protocol error.
    const safe = errors.split(accessToken!).join("[redacted]").split(chatgptAccountId!).join("[redacted]");
    throw new Error(`${args[0]} failed (${code}): ${safe.slice(-2500)}`);
  }
  return output;
}

function assertNoCredentialFiles(dir: string): void {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) assertNoCredentialFiles(path);
    else if (entry.isFile()) {
      assert(entry.name !== "auth.json", "native login unexpectedly persisted credentials");
      const bytes = readFileSync(path);
      assert(!bytes.includes(Buffer.from(accessToken!)), "access token persisted in generated state");
    }
  }
}
