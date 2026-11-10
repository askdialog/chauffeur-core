import { expect, test } from "bun:test";

test("an empty Terminal panel opens once and never duplicates a pending login terminal", async () => {
  const child = Bun.spawn([process.execPath, new URL("../../test/terminal-open.fixture.tsx", import.meta.url).pathname], { stdout: "pipe", stderr: "pipe" });
  const [out, err, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (code !== 0) throw new Error(`${out}\n${err}`);
  expect(out).toContain("Initial terminal opening is coordinated");
});
