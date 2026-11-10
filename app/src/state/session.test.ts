import { expect, test } from "bun:test";

test("a remote conversation merges overlapping history and live events without gaps", async () => {
  const child = Bun.spawn([process.execPath, new URL("../../test/session.fixture.ts", import.meta.url).pathname], {stdout:"pipe",stderr:"pipe"});
  const [out, err, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (code !== 0) throw new Error(`${out}\n${err}`);
  expect(out).toContain("Remote tail/head replay is complete");
});
