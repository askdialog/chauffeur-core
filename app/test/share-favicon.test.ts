import { expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

test("shared pages bundle the Chauffeur favicon under the served assets path", async () => {
  const app = resolve(import.meta.dir, "..");
  const output = await mkdtemp(join(tmpdir(), "chauffeur-share-build-"));
  try {
    const build = Bun.spawn(
      ["bun", "run", "vite", "build", "-c", "vite.share.config.ts", "--outDir", output, "--logLevel", "error"],
      { cwd: app, stdout: "inherit", stderr: "inherit" },
    );
    expect(await build.exited).toBe(0);

    const html = await Bun.file(join(output, "share.html")).text();
    const icon = html.match(/<link rel="icon" type="image\/png" href="\/(assets\/[^"/]+\.png)"/)?.[1];
    if (!icon) throw new Error("The share build has no PNG favicon under /assets/");

    expect(await Bun.file(join(output, icon)).bytes()).toEqual(
      await Bun.file(join(app, "public", "favicon.png")).bytes(),
    );
  } finally {
    await rm(output, { recursive: true, force: true });
  }
}, 30_000);
