// Keys the embedded terminal sends to the shell differently from xterm.js, to behave as Terminal.app
// and iTerm2 do. App shortcuts are not here: `forApp` takes them before the shell sees them.

import { isMac } from "./keys";

const ESC = "\x1b";

/**
 * What the shell receives for this key press in place of xterm's own sequence, or null to leave it to
 * xterm. On macOS, ⌥← and ⌥→ send ESC b and ESC f, the word moves zsh and bash bind by default:
 * xterm sends ESC[1;3D and ESC[1;3C, which they leave unbound. ⌥⌫ is left to xterm, which already
 * sends ESC DEL (delete the previous word).
 */
export function shellSequence(
  event: Pick<KeyboardEvent, "type" | "key" | "altKey" | "metaKey" | "ctrlKey" | "shiftKey">,
  mac = isMac,
): string | null {
  if (!mac || event.type !== "keydown") return null;
  if (!event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) return null;
  if (event.key === "ArrowLeft") return `${ESC}b`;
  if (event.key === "ArrowRight") return `${ESC}f`;
  return null;
}
