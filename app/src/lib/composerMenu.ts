// The keys of a menu above the composer (the `/` menu, the saved prompts' menu): one meaning each,
// so that both menus answer them alike.

import { hasMod } from "./keys";

export type MenuAction = "close" | "up" | "down" | "pick";

/**
 * What a key does in an open menu: Esc closes it, ↑↓ move, ↵ or Tab pick the selected row. Tab
 * with Shift is not a pick: ⇧Tab opens the saved prompts. Null for any other key.
 */
export function menuKey(key: Pick<KeyboardEvent, "key" | "shiftKey" | "ctrlKey" | "metaKey" | "altKey">): MenuAction | null {
  if (key.key === "Escape") return "close";
  if (key.key === "ArrowUp") return "up";
  if (key.key === "ArrowDown") return "down";
  if (key.key === "Tab" && !key.shiftKey && !key.ctrlKey && !key.metaKey && !key.altKey) return "pick";
  if (key.key === "Enter" && !key.shiftKey && !hasMod(key)) return "pick";
  return null;
}
