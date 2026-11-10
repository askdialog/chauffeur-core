import { expect, test } from "bun:test";
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import WebSocket from "ws";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { connectDaemon } from "../src/daemon";
import { generateIdentity, peerIdFromPrivateKey } from "@chauffeur/remote";
import type { RemoteStateResult, RemoteDevice, RemoteDevicesResult } from "@chauffeur/protocol";

// Explicit binary opt-in. Every path is temporary; no real user's state, Linear
// requests or agent sessions. Run with CHAUFFEUR_TEST_DAEMON=/path/to/chauffeurd.
const binary = process.env.CHAUFFEUR_TEST_DAEMON;
test.skipIf(!binary)("existing daemon protocol serves workspaces, chats, history and subscriptions over Unix", async () => {
  const folder = await mkdtemp(join(tmpdir(), "chauffeur-mobile-daemon-"));
  const env = { ...process.env, CHAUFFEUR_HOME: join(folder, "home"), CHAUFFEUR_WORKSPACES_DIR: join(folder, "workspaces"), CHAUFFEUR_REPOS_DIR: join(folder, "repos"), CHAUFFEUR_PORT_OVERRIDE: "0", GIT_AUTHOR_NAME: "Mobile test", GIT_AUTHOR_EMAIL: "test@example.invalid", GIT_COMMITTER_NAME: "Mobile test", GIT_COMMITTER_EMAIL: "test@example.invalid" };
  Reflect.deleteProperty(env, "CHAUFFEUR_WORKSPACE_ID"); Reflect.deleteProperty(env, "CHAUFFEUR_PARENT");
  async function run(command: string[]) { const process = Bun.spawn(command, { env, stdout: "pipe", stderr: "pipe" }); const [out, error, exit] = await Promise.all([new Response(process.stdout).text(), new Response(process.stderr).text(), process.exited]); if (exit) throw new Error(error); return out; }
  let daemon: ReturnType<typeof Bun.spawn> | undefined;
  let client: Awaited<ReturnType<typeof connectDaemon>> | undefined;
  try {
    await mkdir(join(folder, "seed")); await mkdir(env.CHAUFFEUR_HOME);
    await writeFile(join(folder, "seed", "README.md"), "Mobile integration fixture\n");
    await run(["git", "init", "-q", "-b", "main", join(folder, "seed")]);
    await run(["git", "-C", join(folder, "seed"), "add", "."]);
    await run(["git", "-C", join(folder, "seed"), "commit", "-q", "-m", "fixture"]);
    await run(["git", "clone", "-q", "--bare", join(folder, "seed"), join(folder, "origin", "demo.git")]);
    await run(["git", "clone", "-q", join(folder, "origin", "demo.git"), join(folder, "clones", "demo")]);
    await writeFile(join(env.CHAUFFEUR_HOME, "settings.toml"), `github_org = ""\ndefault_repos = ["demo"]\nclone_roots = [${JSON.stringify(join(folder, "clones"))}]\nlinear_enabled = false\n`);
    daemon = Bun.spawn([binary!], { env, stdout: "ignore", stderr: "ignore" });
    for (let i = 0; i < 100; i++) { try { client = await connectDaemon(join(env.CHAUFFEUR_HOME, "chauffeurd.sock")); break; } catch { await Bun.sleep(50); } }
    expect(client).toBeDefined();
    const metadata = JSON.parse(await readFile(join(env.CHAUFFEUR_HOME, "daemon.json"), "utf8")) as { port: number };
    const token = (await readFile(join(env.CHAUFFEUR_HOME, "token"), "utf8")).trim();
    const browser = new WebSocket(`ws://127.0.0.1:${metadata.port}/ws`, { headers: { Authorization: `Bearer ${token}` } });
    try {
      await new Promise<void>((resolve, reject) => { browser.once("open", resolve); browser.once("error", reject); });
      for (const [method, params] of [
        ["remote.state", {}],
        ["remote.push_get", {device_id:"unauthorized"}],
        ["remote.push_set", {device_id:"unauthorized", config:{token:null, preferences:{enabled:false, approvals:true, completed:true, errors:true, show_workspace:false, language:"fr"}}}],
      ] as const) {
        const denied = new Promise<any>((resolve) => browser.once("message", (data) => resolve(JSON.parse(data.toString()))));
        browser.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }));
        const result = await denied;
        expect(result.result).toBeUndefined();
        expect(result.error.message).toContain("private Unix socket");
      }
    } finally { browser.terminate(); }
    await client!.call("workspace.create", { repos: ["demo"], tickets: [], name: "mobile-check", background: false, explore: true });
    const workspaces = await client!.call("workspace.list", { include_archived: false }) as { workspaces: { id: string }[] };
    expect(workspaces.workspaces.length).toBe(1);
    const workspace = workspaces.workspaces[0].id;
    const chat = await client!.call("chat.create", { workspace, title: "Phone conversation", agent: "claude" }) as { id: string };
    const chats = await client!.call("chat.list", { workspace }) as { chats: { id: string }[] };
    expect(chats.chats.some((entry) => entry.id === chat.id)).toBe(true);
    const history = await client!.call("events.history", { chat: chat.id, after_seq: 0, limit: 25 }) as { events: unknown[] };
    expect(Array.isArray(history.events)).toBe(true);
    const subscription = await client!.call("events.subscribe", { all: false, registry: false, workspaces: [], chats: [chat.id], ephemeral: true, after_seq: 0 }) as { subscription: string };
    expect(typeof subscription.subscription).toBe("string");
    await client!.call("events.unsubscribe", { subscription: subscription.subscription });
    const initial = await client!.call("remote.state", {}) as RemoteStateResult;
    const phone = await generateIdentity("Ed25519");
    const device = await client!.call("remote.approve", { peer_id: peerIdFromPrivateKey(phone).toString(), name: "Pixel test", relay: "ws://127.0.0.1:41240" }) as RemoteDevice;
    const publicDevices = await client!.call("remote.devices", {}) as RemoteDevicesResult;
    expect(publicDevices.devices[0].id).toBe(device.info.id);
    expect(JSON.stringify(publicDevices)).not.toContain(device.secret);
    client!.close(); daemon.kill(); await daemon.exited;
    daemon = Bun.spawn([binary!], { env, stdout: "ignore", stderr: "ignore" });
    client = undefined;
    for (let i = 0; i < 100; i++) { try { client = await connectDaemon(join(env.CHAUFFEUR_HOME, "chauffeurd.sock")); break; } catch { await Bun.sleep(50); } }
    expect(client).toBeDefined();
    const restored = await client!.call("remote.state", {}) as RemoteStateResult;
    expect(restored.identity_seed).toEqual(initial.identity_seed);
    expect(restored.devices[0].secret).toBe(device.secret);
    await client!.call("remote.check", { device_id: device.info.id });
    await client!.call("remote.revoke", { device_id: device.info.id });
    const denied = await client!.call("remote.check", { device_id: device.info.id }).then(() => null, (error: Error) => error);
    expect(denied).toBeInstanceOf(Error);
  } finally {
    client?.close(); daemon?.kill(); if (daemon) await daemon.exited;
    await rm(folder, { recursive: true, force: true });
  }
}, 20_000);
