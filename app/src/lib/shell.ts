// A composer's text starting with "!" is a shell command, as in Claude Code's bash mode: the
// daemon runs it in the workspace folder (`chat.shell`) and the agent reads it with the next
// message, instead of a message sent to the agent; `\!` escapes it. Not offered in an
// exploration's chats nor in a design chat (the daemon refuses them too).

/** The command a composer's text holds, or null when the text is a message. */
export function shellCommand(text: string): string | null {
  const match = /^\s*!\s*(\S[\s\S]*?)\s*$/.exec(text);
  return match ? match[1] : null;
}

/** The message sent for a text that is no command: a leading `\!` sends `!` as text. */
export function messageText(text: string): string {
  return text.replace(/^(\s*)\\!/, "$1!");
}

/** Whether the composer shows its shell mode: the text starts with "!". */
export function inShellMode(text: string): boolean {
  return text.trimStart().startsWith("!");
}

/** How a command ended, for its item: `exit 0`, `timed out`, `killed`; null while it runs. */
export function shellEnding(item: { running: boolean; exitCode: number | null; timedOut: boolean }): string | null {
  if (item.running) return null;
  if (item.timedOut) return "timed out";
  return item.exitCode == null ? "killed" : `exit ${item.exitCode}`;
}

/** Lines of output shown before "Show all". */
export const SHELL_PREVIEW_LINES = 6;

/** The output shown folded, and how many lines it hides. */
export function foldedOutput(output: string): { text: string; hidden: number } {
  const lines = output.replace(/\n+$/, "").split("\n");
  if (lines.length <= SHELL_PREVIEW_LINES) return { text: lines.join("\n"), hidden: 0 };
  return { text: lines.slice(0, SHELL_PREVIEW_LINES).join("\n"), hidden: lines.length - SHELL_PREVIEW_LINES };
}
