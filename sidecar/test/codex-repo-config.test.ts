import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mergeRepoLayers, readRepoLayers, threadOverrides, withRepoLayers, type RepoLayer } from "../src/agents/codex/repo-config.ts";
import { codexDeclaration } from "../src/agents/mcp-env.ts";
import { FakeAppServer } from "./support/codex-fakes.ts";

const folders: string[] = [];
afterEach(() => {
  for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true });
});

/** A workspace folder with these repos, each with a `.codex/config.toml` (its content does not matter: Codex is faked). */
function workspace(...repos: Array<{ name: string; config?: boolean }>): string {
  const root = mkdtempSync(join(tmpdir(), "codex-repo-config-"));
  folders.push(root);
  for (const repo of repos) {
    mkdirSync(join(root, repo.name, ".codex"), { recursive: true });
    if (repo.config !== false) writeFileSync(join(root, repo.name, ".codex", "config.toml"), "# faked\n");
  }
  return root;
}

const project = (folder: string, config: unknown, disabledReason: string | null = null) => ({ name: { type: "project", dotCodexFolder: folder }, version: "1", config, disabledReason });
const user = (config: unknown) => ({ name: { type: "user", file: "/home/j/.codex/config.toml", profile: null }, version: "2", config, disabledReason: null });

function fakeServer(): FakeAppServer {
  return new FakeAppServer({ executable: "codex", args: [], cwd: "/w", env: {}, handlers: { notification: () => {}, request: async () => null }, log: undefined as never });
}

const layer = (dir: string, config: RepoLayer["config"]): RepoLayer => ({ dir, folder: `${dir}/.codex`, config });

describe("reading the repos' layers", () => {
  test("Codex is asked in each repo with a config of its own; a trusted repo's layer is kept, an untrusted one's is not", async () => {
    const root = workspace({ name: "alpha" }, { name: "beta" }, { name: "gamma", config: false });
    const [alpha, beta, gamma] = ["alpha", "beta", "gamma"].map((name) => join(root, name));
    const app = fakeServer();
    const untrusted = `To load project-local config, hooks, and exec policies, add ${beta} as a trusted project in /home/j/.codex/config.toml.`;
    app.configByCwd = {
      [alpha!]: {
        config: {},
        origins: {},
        layers: [
          project(`${alpha}/.codex`, {
            mcp_servers: { notes: { command: "notes-mcp" } },
            model_reasoning_effort: "high",
            hooks: { PreToolUse: [{ hooks: [{ type: "command", command: "lint" }] }] },
            projects: { "/elsewhere": { trust_level: "trusted" } },
            project_root_markers: [],
            bypass_hook_trust: true,
          }),
          // Above the repo (an unusual project_root_markers): not the repo's to give.
          project(`${root}/.codex`, { mcp_servers: { above: { command: "x" } } }),
          user({ mcp_servers: { mine: { command: "mine" } } }),
        ],
      },
      [beta!]: { config: {}, origins: {}, layers: [project(`${beta}/.codex`, { mcp_servers: { evil: { command: "rm" } } }, untrusted), user({})] },
    };

    const read = await readRepoLayers(app, [alpha!, beta!, gamma!]);

    expect(app.sent("config/read")).toEqual([
      { includeLayers: true, cwd: alpha },
      { includeLayers: true, cwd: beta },
    ]);
    // Hooks, trust, project roots and hook-trust bypass stay out.
    expect(read.enabled).toEqual([{ dir: alpha!, folder: `${alpha}/.codex`, config: { mcp_servers: { notes: { command: "notes-mcp" } }, model_reasoning_effort: "high" } }]);
    expect(read.untrusted).toEqual([{ dir: beta!, reason: untrusted }]);
    expect(read.failed).toEqual([]);
  });

  test("an untrusted repo with nothing in its config says nothing; a config Codex cannot read is reported", async () => {
    const root = workspace({ name: "alpha" }, { name: "beta" });
    const [alpha, beta] = ["alpha", "beta"].map((name) => join(root, name));
    const app = fakeServer();
    app.configByCwd = { [alpha!]: { config: {}, origins: {}, layers: [project(`${alpha}/.codex`, {}, "untrusted")] } };
    app.handlers["config/read"] = (params) => {
      const cwd = (params as { cwd: string }).cwd;
      if (cwd === beta) throw new Error("Error parsing project config file");
      return app.configByCwd[cwd];
    };
    const read = await readRepoLayers(app, [alpha!, beta!]);
    expect(read).toEqual({ enabled: [], untrusted: [], failed: [{ dir: beta!, error: "Error parsing project config file" }] });
  });
});

describe("merging the repos' layers", () => {
  test("the first repo's server is kept whole, a different one of a later repo is a clash; other settings: the first repo that sets them", () => {
    const merged = mergeRepoLayers([
      layer("/w/alpha", { mcp_servers: { shared: { command: "a-mcp", env: { MODE: "a" } }, same: { url: "https://same" } }, tools: { web_search: true } }),
      layer("/w/beta", { mcp_servers: { shared: { command: "b-mcp", args: ["--x"] }, same: { url: "https://same" }, own: { url: "https://own" } }, tools: { web_search: false, view_image: true }, model: "gpt-x" }),
    ]);
    expect(merged.config).toEqual({
      tools: { web_search: true, view_image: true },
      model: "gpt-x",
      mcp_servers: { shared: { command: "a-mcp", env: { MODE: "a" } }, same: { url: "https://same" }, own: { url: "https://own" } },
    });
    expect(merged.clashes).toEqual(["MCP server shared of beta clashes with alpha's; the first one is kept"]);
    // Each repo gave only the servers it won.
    expect(merged.given.map((given) => given.config.mcp_servers)).toEqual([
      { shared: { command: "a-mcp", env: { MODE: "a" } }, same: { url: "https://same" } },
      { own: { url: "https://own" } },
    ]);
  });
});

describe("the thread's overrides", () => {
  test("a repo's tables go whole, except one holding Chauffeur's own keys, which goes key by key under them", () => {
    const own = { "sandbox_workspace_write.writable_roots": ["/w/alpha"], "hooks.state": { k: { trusted_hash: "h" } } };
    const overrides = threadOverrides(
      { mcp_servers: { "dotted.name": { command: "x" } }, sandbox_workspace_write: { writable_roots: ["/elsewhere"], network_access: true }, model: "gpt-x" },
      own,
    );
    expect(overrides).toEqual({
      "sandbox_workspace_write.writable_roots": ["/w/alpha"],
      "hooks.state": { k: { trusted_hash: "h" } },
      // Server names are never split: the table goes whole.
      mcp_servers: { "dotted.name": { command: "x" } },
      "sandbox_workspace_write.network_access": true,
      model: "gpt-x",
    });
  });

  test("without repo config, the overrides are Chauffeur's own", () => {
    expect(threadOverrides({}, { "hooks.state": {} })).toEqual({ "hooks.state": {} });
  });
});

describe("where a repo's server is declared", () => {
  test("a server a repo gave is declared in the repo's file, with its variables; the user's servers stay the user's", () => {
    const read = {
      config: { mcp_servers: { mine: { command: "mine", env_vars: ["MINE"] }, linear: { url: "https://user.example" } } },
      layers: [{ name: { type: "sessionFlags" }, config: {}, disabledReason: null }, user({ mcp_servers: { mine: { command: "mine" }, linear: { url: "https://user.example" } } })],
    };
    const given = mergeRepoLayers([
      layer("/home/j/chauffeur/workspaces/ws/alpha", { mcp_servers: { linear: { url: "https://mcp.linear.app/mcp", env_http_headers: { Authorization: "LINEAR_TOKEN" } } } }),
      layer("/home/j/chauffeur/workspaces/ws/beta", { mcp_servers: { linear: { url: "https://other" }, db: { command: "db", env_vars: ["DATABASE_URL"] } } }),
    ]).given;
    const seen = withRepoLayers(read, given);
    const env = { HOME: "/home/j" };
    expect(codexDeclaration("linear", seen, env)).toEqual({ variables: ["LINEAR_TOKEN"], where: "~/chauffeur/workspaces/ws/alpha/.codex/config.toml [mcp_servers.linear]" });
    expect(codexDeclaration("db", seen, env)).toEqual({ variables: ["DATABASE_URL"], where: "~/chauffeur/workspaces/ws/beta/.codex/config.toml [mcp_servers.db]" });
    expect(codexDeclaration("mine", seen, env)).toEqual({ variables: ["MINE"], where: "~/.codex/config.toml [mcp_servers.mine]" });
    // The layers rank below the session flags, above the user's.
    expect(seen.layers!.map((entry) => entry.name.type)).toEqual(["sessionFlags", "project", "project", "user"]);
  });

  test("no repo layer: Codex's reading as it is", () => {
    const read = { config: {}, layers: [] };
    expect(withRepoLayers(read, [])).toBe(read);
  });
});
