// A design chat's own browser, for Claude and Codex alike: Playwright MCP without a window, so that
// the agent checks its screens without opening windows on the user's screen.

import { tmpdir } from "node:os";
import { basename, join } from "node:path";

/** The server's name; the user's Playwright, if any, opens a window. */
export const DESIGN_BROWSER = "headless-browser";

/**
 * Pinned, so that a design chat does not fetch a new release at each start. `file://` URLs are
 * blocked unless file access is unrestricted, and the screens are files of the design folder;
 * automatically named screenshots go to a temporary folder, not to the notes.
 */
export function designBrowserCommand(designDir: string): { command: string; args: string[] } {
  return {
    command: "npx",
    args: [
      "-y",
      "@playwright/mcp@0.0.83",
      "--headless",
      "--isolated",
      "--allow-unrestricted-file-access",
      "--output-dir",
      join(tmpdir(), "chauffeur-design-browser", basename(designDir)),
    ],
  };
}
