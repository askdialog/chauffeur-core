import { expect, spyOn } from "bun:test";
import type { RoutineCreateByHandParams } from "@chauffeur/protocol";
import { domTest } from "@/test/dom";
import { repo, repoPage } from "@/test/repos";

domTest("adding a second repo does not create the routine before Create is clicked", import.meta.path, async ({ render, click, fill, until }) => {
  const { NewRoutineDialog } = await import("./NewRoutineDialog");
  const { client, useApp } = await import("@/state/app");
  const before = useApp.getState();
  const repos = [repo("api", true), repo("web")];
  useApp.setState({ repos, reposError: null, reposRevision: 0, reposSources: true, reposSync: { refreshedAt: null, githubError: null }, newRoutineShown: true, models: [], loadout: [] });
  const created: RoutineCreateByHandParams[] = [];
  const rpc = spyOn(client, "call").mockImplementation(async (method, params) => {
    if (method === "repos.cached") return { repos, github_sources: true };
    if (method === "repos.page") return repoPage(repos);
    if (method === "routine.create_by_hand") {
      created.push(params as RoutineCreateByHandParams);
      throw new Error("stop here");
    }
    throw new Error(`Unexpected RPC: ${method}`);
  });
  try {
    await render(<NewRoutineDialog />);
    await fill('[data-testid="new-routine-name"]', "Triage");
    await fill('[data-testid="new-routine-description"]', "Triage the new tickets");
    await click('[data-testid="new-routine-add-repo"]');
    await until(() => !!document.querySelector('[data-repo="api"]'));
    await click('[data-repo="api"]');
    // The form is now valid: closing and opening the picker again with Add repo must not submit it.
    await click('[data-testid="new-routine-add-repo"]');
    await until(() => !document.querySelector('[data-repo="web"]'));
    await click('[data-testid="new-routine-add-repo"]');
    await until(() => !!document.querySelector('[data-repo="web"]'));
    await click('[data-repo="web"]');
    expect(created).toEqual([]);

    await click('[data-testid="new-routine-create"]');
    await until(() => created.length === 1);
    expect(created[0]?.repos).toEqual(["api", "web"]);
  } finally {
    await render(null);
    rpc.mockRestore();
    useApp.setState(before, true);
  }
});
