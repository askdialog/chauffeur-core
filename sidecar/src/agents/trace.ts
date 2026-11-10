import { appendFileSync } from "node:fs";

/** Appends every raw agent message, as JSON lines, to a debugging trace file. */
export function traceTo(file: string): (chatId: string, message: unknown) => void {
  return (chatId, message) => {
    try {
      appendFileSync(file, `${JSON.stringify({ chat_id: chatId, message })}\n`);
    } catch {
      // Tracing is best effort.
    }
  };
}
