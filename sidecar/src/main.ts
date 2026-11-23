// chauffeur-sidecar: hosts every agent session of the daemon (see server.ts).
//
// `chauffeur-sidecar codex-hook pre-tool-use [read-only|design] [--clones <base64 JSON>]` is the
// sidecar run as Codex's PreToolUse hook: it answers one hook payload and exits, without loading
// the agents.

const [command, subcommand, ...rest] = process.argv.slice(2);

if (command === "codex-hook") {
  const { runCodexHook } = await import("./agents/codex/hook.ts");
  process.exit(await runCodexHook(subcommand, rest));
} else {
  await import("./server.ts");
}

export {};
