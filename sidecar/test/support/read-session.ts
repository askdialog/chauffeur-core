// Reads a session's conversation with the SDK's own transcript reader, in a process of its own:
// the SDK resolves CLAUDE_CONFIG_DIR once per process, and tests point it at a temporary folder.
// Usage: bun read-session.ts '<AgentSessionMessagesParams as JSON>'

import { claudeSessionMessages, sdkTranscripts } from "../../src/agents/claude/subagents.ts";

const params = JSON.parse(process.argv[2] ?? "{}");
console.log(JSON.stringify(await claudeSessionMessages(sdkTranscripts, params)));
