// Back to the code from a design chat (story 47), kept pure so that it is tested in
// designHandoff.test.ts: the message "Send to source chat" writes into a chat's draft, the chat it
// goes to, the screens Claude published to claude.ai with its Artifact tool, and the screen the
// header's Open › Browser shows.

import type { CanvasScreen, ChatInfo, JsonValue, OpenApp } from "@chauffeur/protocol";
import type { ShownSection } from "./canvas";
import type { ChatItem } from "./chat";
import { openChats } from "./chatTabs";

// --- Send to source chat ---

/** A screen whose file exists: a missing one has neither a URL nor a time it changed. */
function present(screen: CanvasScreen): boolean {
  return screen.url != null || screen.modified_ms != null;
}

/**
 * What is sent: the selected screens of the page, by section, or every screen of the page when
 * none of them is selected. Missing files and sections left empty are left out.
 */
export function screensToSend(sections: ShownSection[], selection: readonly string[]): ShownSection[] {
  const onPage = sections.some((section) => section.screens.some((screen) => selection.includes(screen.file)));
  return sections
    .map((section) => ({
      ...section,
      screens: section.screens.filter((screen) => present(screen) && (!onPage || selection.includes(screen.file))),
    }))
    .filter((section) => section.screens.length > 0);
}

/** A screen's path under the workspace folder, where the source chat's agent runs. */
export function screenPath(designChat: string, file: string): string {
  return `.chauffeur/designs/${designChat}/${file.replace(/^\.\//, "")}`;
}

/** The message: the screens by section, each with its path, asking to implement them. */
export function handoffMessage(designTitle: string, designChat: string, sections: ShownSection[]): string {
  const count = sections.reduce((n, section) => n + section.screens.length, 0);
  const lines = [`Implement ${count === 1 ? "this screen" : "these screens"} from the design chat “${designTitle}”:`];
  for (const section of sections) {
    lines.push("", `${section.title}:`);
    for (const screen of section.screens) {
      lines.push(`- ${screen.label?.trim() || screen.file}: \`${screenPath(designChat, screen.file)}\``);
    }
  }
  lines.push(
    "",
    `Open each file to see the screen, then build ${count === 1 ? "it" : "them"} with the repo's own components, matching the design as closely as they allow.`,
  );
  return lines.join("\n");
}

/** The draft with the message below what the user had written, or the message alone. */
export function appendDraft(draft: string, message: string): string {
  const kept = draft.trimEnd();
  return kept ? `${kept}\n\n${message}` : message;
}

/** The chat "Send to source chat" goes to: the design chat's source, closed or not, unless deleted. */
export function sourceChatOf(design: ChatInfo, chats: ChatInfo[]): ChatInfo | null {
  if (!design.source_chat) return null;
  return chats.find((c) => c.id === design.source_chat && c.workspace_id === design.workspace_id) ?? null;
}

/** What "Send to a chat…" lists: the workspace's other open chats that write code. */
export function chatsToSendTo(design: ChatInfo, chats: ChatInfo[]): ChatInfo[] {
  return openChats(chats).filter((c) => c.workspace_id === design.workspace_id && c.id !== design.id && c.kind !== "design");
}

// --- Open on claude.ai ---

function field(value: JsonValue | undefined, key: string): JsonValue | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? value[key] : undefined;
}

/** A published file's path relative to the design folder; null when it is outside it. */
export function designFile(path: string, dir: string | null): string | null {
  if (!path) return null;
  if (path.startsWith("/")) {
    if (!dir) return null;
    const folder = dir.endsWith("/") ? dir : `${dir}/`;
    return path.startsWith(folder) ? path.slice(folder.length) : null;
  }
  // The design chat's agent runs in its folder: a relative path is under it.
  const file = path.replace(/^(\.\/)+/, "");
  return file.split("/").includes("..") ? null : file;
}

/** The claude.ai URL of a publish's result; null for another action's, an error or anything else. */
export function publishedUrl(output: string): string | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(output);
  } catch {
    return null;
  }
  const url = field(parsed as JsonValue, "url");
  return typeof url === "string" && /^https:\/\//.test(url) ? url : null;
}

/**
 * The screens Claude published with its Artifact tool, each with its last URL, by file (relative to
 * the design folder). Only publishes count (no action, or "publish"), of a file of the folder,
 * whose result came back without an error.
 */
export function publishedScreens(items: readonly ChatItem[], dir: string | null): Record<string, string> {
  const published: Record<string, string> = {};
  for (const item of items) {
    if (item.kind !== "tool" || item.name !== "Artifact" || !item.result || item.result.isError) continue;
    const action = field(item.input, "action");
    if (action != null && action !== "publish") continue;
    const path = field(item.input, "file_path");
    const file = typeof path === "string" ? designFile(path, dir) : null;
    const url = publishedUrl(item.result.output);
    if (file && url) published[file] = url;
  }
  return published;
}

// --- Open from a design chat ---

/** A screen of the canvas page shown, as Open › Browser needs it. */
export type PageScreen = { file: string; url: string | null };

export function pageScreens(sections: ShownSection[]): PageScreen[] {
  return sections.flatMap((section) => section.screens.map((screen) => ({ file: screen.file, url: screen.url ?? null })));
}

/** What Open › Browser shows: the first selected screen with a URL, else the page's first one. */
export function browserUrl(screens: readonly PageScreen[], selection: readonly string[]): string | null {
  const selected = screens.find((screen) => screen.url && selection.includes(screen.file));
  return selected?.url ?? screens.find((screen) => screen.url)?.url ?? null;
}

/**
 * The app the Open button names for a design folder: the default, unless it is a terminal (a
 * design folder never opens in one), then the first editor, else the file manager, as the daemon
 * picks.
 */
export function designApp(apps: readonly OpenApp[]): OpenApp | null {
  const app = apps.find((a) => a.is_default);
  if (app && app.kind !== "terminal") return app;
  return apps.find((a) => a.kind === "editor") ?? apps.find((a) => a.kind === "files") ?? null;
}
