import { expect, test } from "bun:test";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const script = new URL("../deploy/configure-voice.sh", import.meta.url).pathname;

test("voice configuration grants only the fixed secret to the existing VM account and never exposes stdin", async () => {
  const folder = await mkdtemp(join(tmpdir(), "chauffeur-voice-deploy-"));
  try {
    const log = join(folder, "calls");
    const gcloud = join(folder, "gcloud");
    await writeFile(gcloud, `#!/usr/bin/env bash
set -eu
printf '%s\\n' "$*" >> "$CALL_LOG"
case "$*" in
  'compute instances describe '*) printf '%s\\n' "\${FAKE_ACCOUNT:-chauffeur-relay@synthetic-project.iam.gserviceaccount.com}" ;;
  'secrets describe '*) exit 1 ;;
  'secrets versions add '*) input=$(cat); [[ "$input" == synthetic-mistral-key-1234 ]] ;;
  'secrets versions describe '*) echo ENABLED ;;
esac
`);
    await chmod(gcloud, 0o700);
    const run = async (mode: string, key: string, account?: string) => {
      const process = Bun.spawn(["bash", "-x", script, mode], {
        env: { PATH: `${folder}:/usr/bin:/bin`, GCP_PROJECT: "synthetic-project", GCP_REGION: "europe-west9", GCP_ZONE: "europe-west9-b", GCP_INSTANCE: "existing-chauffeur", CALL_LOG: log, FAKE_ACCOUNT: account },
        stdin: new Blob([key]), stdout: "pipe", stderr: "pipe",
      });
      return { code: await process.exited, output: await new Response(process.stdout).text() + await new Response(process.stderr).text() };
    };
    const created = await run("--stdin", "synthetic-mistral-key-1234\n");
    if (created.code !== 0) throw new Error(created.output);
    expect(created.code).toBe(0);
    expect(created.output).not.toContain("synthetic-mistral-key-1234");
    let calls = await readFile(log, "utf8");
    expect(calls).not.toContain("synthetic-mistral-key-1234");
    expect(calls).toContain("secrets create chauffeur-mistral-api-key --replication-policy=user-managed --locations=europe-west9");
    expect(calls).toContain("secrets add-iam-policy-binding chauffeur-mistral-api-key --member=serviceAccount:chauffeur-relay@synthetic-project.iam.gserviceaccount.com --role=roles/secretmanager.secretAccessor");
    expect(calls).toContain("--metadata=chauffeur-voice-secret=chauffeur-mistral-api-key");
    expect(calls).not.toContain("projects add-iam-policy-binding");
    expect(calls).not.toContain("instances create");
    await writeFile(log, "");
    expect((await run("--existing-secret", "")).code).toBe(0);
    calls = await readFile(log, "utf8");
    expect(calls).not.toContain("versions access");
    expect(calls).not.toContain("secrets create");
    expect(calls).not.toContain("versions add");
    await writeFile(log, "");
    expect((await run("--stdin", "synthetic-mistral-key-1234", "unrelated@example.invalid")).code).toBe(1);
    expect(await readFile(log, "utf8")).not.toContain("secrets");
    await writeFile(log, "");
    expect((await run("--stdin", "invalid\nMISTRAL_API_KEY=injection")).code).toBe(1);
    expect(await readFile(log, "utf8")).not.toContain("secrets");
  } finally { await rm(folder, { recursive: true, force: true }); }
});
