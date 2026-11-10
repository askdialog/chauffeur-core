import { expect, test } from "bun:test";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

test("the deployed archive starts by itself in an empty directory", async () => {
  const folder = await mkdtemp(join(tmpdir(), "chauffeur-relay-package-"));
  let relay: ReturnType<typeof Bun.spawn> | undefined;
  try {
    const archive = join(folder, "relay.tar.gz");
    const packaged = Bun.spawn(["bash", new URL("../deploy/package.sh", import.meta.url).pathname, archive], { stdout: "pipe", stderr: "pipe" });
    expect(await packaged.exited).toBe(0);
    const root = join(folder, "runtime");
    await mkdir(root);
    expect(await Bun.spawn(["tar", "-xzf", archive, "-C", root]).exited).toBe(0);
    relay = Bun.spawn([process.execPath, join(root, "main.ts")], {
      cwd: root,
      env: { PATH: process.env.PATH, RELAY_ADMIN_TOKEN: "synthetic-package-test-token".repeat(3), PORT: "0", RELAY_HOST: "127.0.0.1" },
      stdout: "pipe", stderr: "pipe",
    });
    if (!(relay.stdout instanceof ReadableStream)) throw new Error("Missing relay output");
    const reader = relay.stdout.getReader();
    let output = "";
    const timeout = setTimeout(() => relay?.kill(), 5000);
    try {
      while (!output.includes("\n")) {
        const next = await reader.read();
        if (next.done) throw new Error(`Relay exited before listening: ${output} ${await new Response(relay.stderr instanceof ReadableStream ? relay.stderr : null).text()}`);
        output += new TextDecoder().decode(next.value);
      }
      const started = JSON.parse(output.split("\n")[0]!) as { event: string; port: number };
      expect(started.event).toBe("relay_started");
      expect((await fetch(`http://127.0.0.1:${started.port}/health`)).status).toBe(200);
      expect((await fetch(`http://127.0.0.1:${started.port}/v1/rooms`, { method: "POST" })).status).toBe(401);
    } finally { clearTimeout(timeout); reader.releaseLock(); }
  } finally {
    relay?.kill();
    if (relay) await relay.exited;
    await rm(folder, { recursive: true, force: true });
  }
}, 15_000);
