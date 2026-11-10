// Reads a subagent's conversation with the SDK's own transcript readers, in a process of its own:
// the SDK resolves CLAUDE_CONFIG_DIR once per process, and tests point it at a temporary folder.
// Usage: bun read-subagent.ts '<AgentSubagentMessagesParams as JSON>'

import { claudeSubagentMessages, sdkTranscripts } from "../../src/agents/claude/subagents.ts";

const params = JSON.parse(process.argv[2] ?? "{}");
console.log(JSON.stringify(await claudeSubagentMessages(sdkTranscripts, params)));
