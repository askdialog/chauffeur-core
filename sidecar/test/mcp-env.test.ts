import { describe, expect, test } from "bun:test";
import {
  claudeDeclaration,
  type CodexConfigLayer,
  codexDeclaration,
  codexDeclaredIn,
  codexMissingEnv,
  codexVariables,
  referencedVariables,
  unsetReferences,
} from "../src/agents/mcp-env.ts";
import { createDispatcher, replaceEnvironment } from "../src/dispatch.ts";
import type { SessionManager } from "../src/sessions.ts";

describe("MCP servers lacking an environment variable", () => {
  test("Codex names the variable in its error", () => {
    expect(
      codexMissingEnv("MCP client for `slack` failed to start: MCP startup failed: Environment variable SLACK_MCP_TOKEN for MCP server 'slack' is not set"),
    ).toEqual(["SLACK_MCP_TOKEN"]);
    expect(codexMissingEnv("Environment variable A_B for MCP server 'x' is empty")).toEqual(["A_B"]);
    expect(codexMissingEnv("MCP startup failed: timed out handshaking with MCP server after 30s")).toEqual([]);
    expect(codexMissingEnv(null)).toEqual([]);
  });

  test("a Codex server is declared in its config's table, or by its plugin", () => {
    expect(codexDeclaredIn("slack", null, { HOME: "/Users/j" })).toBe("~/.codex/config.toml [mcp_servers.slack]");
    expect(codexDeclaredIn("my server", null, { HOME: "/Users/j", CODEX_HOME: "/Users/j/codex" })).toBe('~/codex/config.toml [mcp_servers."my server"]');
    expect(codexDeclaredIn("slack", "slack@openai", {})).toBe("the plugin slack@openai");
  });

  test("a Codex server's variables: its bearer token's, its headers', those it forwards", () => {
    expect(
      codexVariables({
        url: "https://x",
        bearer_token_env_var: "TOKEN",
        env_http_headers: { Authorization: "AUTH", "X-Team": "TEAM" },
        http_headers: { "X-Literal": "not-a-variable" },
      }),
    ).toEqual(["TOKEN", "AUTH", "TEAM"]);
    // `env` holds values, not variable names; a remote-sourced variable is not the session's.
    expect(codexVariables({ command: "x", env: { MODE: "ro" }, env_vars: ["DATABASE_URL", { name: "LOCAL" }, { name: "REMOTE", source: "remote" }, { name: "L2", source: "local" }] })).toEqual([
      "DATABASE_URL",
      "LOCAL",
      "L2",
    ]);
    // Started in another environment, it reads that one's.
    expect(codexVariables({ command: "x", env_vars: ["A"], environment_id: "devbox" })).toEqual([]);
    expect(codexVariables({ command: "x", env_vars: ["A"], environment_id: "local" })).toEqual(["A"]);
  });

  test("a Codex server's entry comes from its merged config, its file from the layer that declares it", () => {
    const env = { HOME: "/home/j" };
    const user: CodexConfigLayer = {
      name: { type: "user", file: "/home/j/.codex/config.toml" },
      config: { mcp_servers: { slack: { url: "https://slack", bearer_token_env_var: "USER_TOKEN" }, linear: { url: "https://linear" } }, model: "never-read" },
      disabledReason: null,
    };
    const project: CodexConfigLayer = {
      name: { type: "project", dotCodexFolder: "/w/.codex" },
      // Only a header added to the user's server: still the user's server.
      config: { mcp_servers: { slack: { env_http_headers: { "X-Team": "TEAM" } }, linear: { url: "https://linear.project" } } },
      disabledReason: null,
    };
    const untrusted: CodexConfigLayer = {
      name: { type: "project", dotCodexFolder: "/w/sub/.codex" },
      config: { mcp_servers: { slack: { url: "https://evil" } } },
      disabledReason: "To load project config, add /w as a trusted project in ~/.codex/config.toml.",
    };
    const read = {
      config: {
        mcp_servers: {
          slack: { url: "https://slack", bearer_token_env_var: "USER_TOKEN", env_http_headers: { "X-Team": "TEAM" } },
          linear: { url: "https://linear.project" },
        },
      },
      layers: [untrusted, project, user],
    };
    expect(codexDeclaration("slack", read, env)).toEqual({ variables: ["USER_TOKEN", "TEAM"], where: "~/.codex/config.toml [mcp_servers.slack]" });
    // Declared again by the project, over the user's: the project's file.
    expect(codexDeclaration("linear", read, env)).toEqual({ variables: [], where: "/w/.codex/config.toml [mcp_servers.linear]" });
    expect(codexDeclaration("absent", read, env)).toBeUndefined();
    const managed: CodexConfigLayer = { name: { type: "mdm" }, config: { mcp_servers: { corp: { url: "https://corp" } } }, disabledReason: null };
    expect(codexDeclaration("corp", { config: { mcp_servers: { corp: { url: "https://corp" } } }, layers: [managed] }, env)?.where).toBe(
      "Codex's managed configuration [mcp_servers.corp]",
    );
    // Without its layers, the user's config.
    expect(codexDeclaration("corp", { config: { mcp_servers: { corp: { url: "https://corp" } } }, layers: null }, env)?.where).toBe(
      "~/.codex/config.toml [mcp_servers.corp]",
    );
  });

  test("Claude Code's ${VAR} without a value, not those with a default or set", () => {
    const config = { url: "${BASE}/mcp", headers: { Authorization: "Bearer ${TOKEN}" }, args: ["${MODE:-fast}", "${EMPTY}"] };
    expect(unsetReferences(config, { BASE: "https://x", EMPTY: "" })).toEqual(["TOKEN", "EMPTY"]);
    expect(unsetReferences({ command: "$HOME/bin/x" }, {})).toEqual([]);
    // Every variable it takes a value from, set, defaulted or not.
    expect(referencedVariables([config, { env: { B: "${TOKEN}" } }])).toEqual(["BASE", "TOKEN", "MODE", "EMPTY"]);
  });

  test("a Claude Code server's declaration comes from its scope's file, nothing else of it", () => {
    const files: Record<string, unknown> = {
      "/home/j/.claude.json": {
        oauthAccount: { emailAddress: "never@read.example" },
        mcpServers: { slack: { env: { TOKEN: "${SLACK_MCP_TOKEN}" } } },
        projects: { "/w": { mcpServers: { local: { url: "${LOCAL_URL}" } } } },
      },
      "/repos/b/.mcp.json": { mcpServers: { proj: { args: ["${PROJ}"] } } },
      "/w/.mcp.json": { mcpServers: { proj: { args: ["${PROJ}"] } } },
    };
    const context = { env: { HOME: "/home/j" }, cwd: "/w", directories: ["/repos/a", "/repos/b"], readJson: (path: string) => files[path] };
    expect(claudeDeclaration("slack", "user", context)).toEqual({ config: { env: { TOKEN: "${SLACK_MCP_TOKEN}" } }, where: "~/.claude.json" });
    expect(claudeDeclaration("local", "local", context)?.where).toBe("~/.claude.json (the servers of /w)");
    expect(claudeDeclaration("proj", "project", context)?.where).toBe("/repos/b/.mcp.json");
    expect(claudeDeclaration("slack", "claudeai", context)).toBeUndefined();
    expect(claudeDeclaration("slack", "user", { ...context, env: { HOME: "/home/j", CLAUDE_CONFIG_DIR: "/cfg" } })).toBeUndefined();
  });
});

describe("the environment agents get", () => {
  test("is replaced in place: whoever holds it sees the new one", () => {
    const env: Record<string, string | undefined> = { PATH: "/bin", GONE: "1" };
    const held = env;
    replaceEnvironment(env, { PATH: "/usr/bin:/bin", SLACK_MCP_TOKEN: "t" });
    expect(held).toEqual({ PATH: "/usr/bin:/bin", SLACK_MCP_TOKEN: "t" });
  });

  test("the daemon sets it with sidecar.set_env", async () => {
    const env: Record<string, string | undefined> = { OLD: "1" };
    const dispatch = createDispatcher({} as SessionManager, env);
    expect(await dispatch("sidecar.set_env", { env: { NEW: "2" } })).toEqual({});
    expect(env).toEqual({ NEW: "2" });
    expect(() => dispatch("sidecar.set_env", { env: { BAD: 3 } })).toThrow("env.BAD must be a string");
  });
});
