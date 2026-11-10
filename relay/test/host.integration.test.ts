import { expect, test } from "bun:test";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { generateIdentity, peerIdFromPrivateKey } from "@chauffeur/remote";
import { startRelay } from "../src/server";
import { connectDaemon } from "../src/daemon";
import type { MobileHostStatus } from "@chauffeur/protocol";

const binary = process.env.CHAUFFEUR_TEST_DAEMON;
const host = process.env.CHAUFFEUR_TEST_MOBILE_HOST;
const integration = binary && host ? test : test.skip;
integration("bundled host starts, restarts with daemon, disables, and never exposes its credential", async () => {
  const root = await mkdtemp(join(tmpdir(), "chauffeur-host-"));
  const state = join(root, "home"); await mkdir(state);
  await writeFile(join(state, "settings.toml"), "linear_enabled = false\n");
  const reservation = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response() });
  const port = reservation.port!; await reservation.stop(true);
  const key = "a".repeat(64);
  const relay = startRelay({ adminToken: key, port: 0 });
  const env: Record<string,string|undefined> = { ...process.env, CHAUFFEUR_HOME: state, CHAUFFEUR_WORKSPACES_DIR: join(root,"workspaces"), CHAUFFEUR_REPOS_DIR: join(root,"repos"), CHAUFFEUR_PORT_OVERRIDE: "0", CHAUFFEUR_MOBILE_HOST: host!, CHAUFFEUR_PAIRING_PAGE_PORT: String(port) };
  delete env.CHAUFFEUR_WORKSPACE_ID;
  let processHandle: ReturnType<typeof Bun.spawn> | undefined;
  let client: Awaited<ReturnType<typeof connectDaemon>> | undefined;
  async function start() {
    processHandle = Bun.spawn([binary!], { env, stdout: "ignore", stderr: "ignore" });
    for (let i=0;i<100;i++) { try { client = await connectDaemon(join(state,"chauffeurd.sock")); return; } catch { await Bun.sleep(50); } }
    throw new Error("Isolated daemon did not start");
  }
  async function waitFor(phase: string) {
    for (let i=0;i<100;i++) { const value = await client!.call("mobile.status", {}) as MobileHostStatus; if (value.phase===phase) return value; await Bun.sleep(100); }
    throw new Error(`Mobile host did not become ${phase}`);
  }
  try {
    await start();
    expect((await client!.call("mobile.status", {}) as MobileHostStatus).enabled).toBe(false);
    await client!.call("mobile.configure", { enabled: true, relay: `ws://127.0.0.1:${relay.server.port}`, operator_token: key });
    const ready = await waitFor("ready"); expect(ready.key_saved).toBe(true); expect(JSON.stringify(ready)).not.toContain(key);
    expect((await fetch(`http://127.0.0.1:${port}/health`)).status).toBe(403);
    expect((await fetch(`http://127.0.0.1:${port}/`)).status).toBe(200);
    const identity = await generateIdentity("Ed25519");
    await client!.call("remote.approve", { peer_id: peerIdFromPrivateKey(identity).toString(), name: "Synthetic phone", relay: `ws://127.0.0.1:${relay.server.port}` });
    const children = Bun.spawnSync(["pgrep", "-P", String(processHandle!.pid)]).stdout.toString().trim().split(/\s+/).filter(Boolean);
    expect(children).toHaveLength(1);
    process.kill(Number(children[0]), "SIGKILL");
    await Bun.sleep(1200);
    await waitFor("ready");
    client!.close(); processHandle!.kill("SIGTERM"); await processHandle!.exited;
    await start(); await waitFor("ready");
    await client!.call("mobile.configure", { enabled: false, relay: `ws://127.0.0.1:${relay.server.port}` }); await waitFor("disabled");
    let stopped = false;
    for (let i=0;i<50;i++) { try { await fetch(`http://127.0.0.1:${port}/`); await Bun.sleep(100); } catch { stopped=true; break; } }
    expect(stopped).toBe(true);
    const grants = await client!.call("remote.devices", {}) as {devices: unknown[]};
    expect(grants.devices).toHaveLength(1);
  } finally {
    client?.close(); processHandle?.kill("SIGTERM"); if (processHandle) await processHandle.exited;
    await relay.stop(); await rm(root, { recursive:true,force:true });
  }
}, 35_000);
