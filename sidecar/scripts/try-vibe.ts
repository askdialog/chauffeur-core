// Drives a real Mistral Vibe session through the sidecar's VibeAgent, with a fake host that prints
// the chat's events, lists two repos and allows every permission request. For trying the agent by
// hand: `bun scripts/try-vibe.ts <folder> "<prompt>" [resume id]`. Uses the user's own Vibe login.

import { VibeAgent } from "../src/agents/vibe/agent.ts";
import type { SessionHost } from "../src/agents/agent.ts";
import { createLogger } from "../src/log.ts";
import type { ChatEvent } from "../src/protocol.ts";

const [cwd, prompt, resume] = process.argv.slice(2);
if (!cwd || !prompt) {
  console.error('usage: bun scripts/try-vibe.ts <folder> "<prompt>" [resume id]');
  process.exit(2);
}
const log = createLogger();
const agent = new VibeAgent({}, log);
let ended: () => void = () => {};
const done = new Promise<void>((resolve) => (ended = resolve));
let session: ReturnType<VibeAgent["createSession"]>;
const host: SessionHost = {
  chatId: "try",
  log,
  emit(event: ChatEvent) {
    if (event.type === "text_delta" || event.type === "thinking_delta") return;
    console.log(JSON.stringify(event).slice(0, 300));
    if (event.type === "permission_request") session.respond(event.request_id, { type: "allow", always: false });
    if (event.type === "turn_ended") ended();
  },
  createWorktree: async () => "/tmp",
  removeWorktree: async () => {},
  listRepos: async () => ({
    workspace: "try",
    explore: false,
    state: "ready",
    repos: [
      { name: "web", description: "The web app", path: cwd },
      { name: "analytics", description: "Dashboards", path: null },
    ],
  }) as never,
  addRepo: async () => {
    throw new Error("not in this script");
  },
  delegate: async () => {
    throw new Error("not in this script");
  },
  delegateWait: async () => {
    throw new Error("not in this script");
  },
  delegateStop: async () => {},
  delegateDetach: async () => {},
  ended: () => console.log("session ended on its own"),
};
session = agent.createSession(
  {
    chat_id: "try",
    workspace_id: "try",
    agent: "vibe",
    cwd,
    additional_directories: [],
    fast: false,
    permission_mode: "default",
    read_only: false,
    env: {},
    prompt,
    prompt_attachments: [],
    prompt_suggestions: false,
    delegate: false,
    append_system_prompt: "You work in a Chauffeur workspace. Answer in French.",
    ...(resume ? { resume_session_id: resume } : {}),
  },
  host,
);
console.log(JSON.stringify(await session.start()));
await done;
await session.stop();
console.log("models:", JSON.stringify((await agent.listModels()).map((m) => m.id)));
console.log("usage:", JSON.stringify(await agent.usage()));
process.exit(0);
