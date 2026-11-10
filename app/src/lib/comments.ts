// Comments of a design chat (story 51), kept pure so that they are tested in comments.test.ts:
// the threads and their numbers, what shows as a pin, which pins are stale, where a pin's bubble
// or thread opens, who may change what, the answers of the capture script, and the "Keep these"
// message.

import type { CommentAuthor, DesignComment, JsonValue, PinElement } from "@chauffeur/protocol";
import type { ShownSection } from "./canvas";
import { screenPath } from "./designHandoff";

/** A thread: its first comment, numbered in creation order across the design, and its replies. */
export type Thread = { comment: DesignComment; replies: DesignComment[]; number: number };

export function threadsOf(comments: readonly DesignComment[]): Thread[] {
  const firsts = comments.filter((c) => c.reply_to == null);
  return firsts.map((comment, index) => ({
    comment,
    replies: comments.filter((c) => c.reply_to === comment.id),
    number: index + 1,
  }));
}

export type Counts = { open: number; resolved: number; unsent: number };

/** Open and resolved threads, and the comments (threads and replies) not sent to the agent yet. */
export function counts(comments: readonly DesignComment[]): Counts {
  let open = 0;
  let resolved = 0;
  let unsent = 0;
  for (const c of comments) {
    if (c.reply_to == null) {
      if (c.resolved) resolved += 1;
      else open += 1;
    }
    if (!c.sent) unsent += 1;
  }
  return { open, resolved, unsent };
}

/** A comment being written: where, and the element found under the point (asked while the bubble opens). */
export type Draft = { file: string; x: number; y: number; whole: boolean; element: PinElement | null };

/**
 * Where the draft's bubble, else the open thread, sits on screen `file` drawn `width` × `height`:
 * at its pin, or in the corner for the whole screen; null when it is on another screen.
 */
export function popoverAnchor(file: string, width: number, height: number, draft: Draft | null, openThread: Thread | null): { x: number; y: number } | null {
  if (draft) {
    if (draft.file !== file) return null;
    return draft.whole ? { x: 10, y: 10 } : { x: (width * draft.x) / 100, y: (height * draft.y) / 100 };
  }
  if (openThread?.comment.screen !== file) return null;
  const pin = openThread.comment.pin;
  return pin ? { x: (width * pin.x) / 100, y: (height * pin.y) / 100 } : { x: 10, y: 10 };
}

/** A thread's pin as the canvas draws it: open threads only; a whole-screen one sits in the corner. */
export type DrawnPin = { thread: Thread; x: number; y: number; whole: boolean; stale: boolean };

/**
 * The pins of a screen, open threads only, in their numbers' order. `found` says, by selector,
 * whether the element is still in the screen (none known: nothing is stale).
 */
export function pinsOf(threads: readonly Thread[], screen: string, found: Readonly<Record<string, boolean>> | null): DrawnPin[] {
  return threads
    .filter((t) => t.comment.screen === screen && !t.comment.resolved)
    .map((thread) => {
      const pin = thread.comment.pin;
      return {
        thread,
        x: pin?.x ?? 0,
        y: pin?.y ?? 0,
        whole: pin == null,
        stale: isStale(thread.comment, found),
      };
    });
}

/** Which pins' selectors a version (`src`) of a screen was seen to hold, by selector. */
export type FoundIn = { src: string; selectors: Record<string, boolean> };

/**
 * `answer`, a frame's check of `src`, added to what was known: for the same version of the screen
 * a selector found in any frame stays found (the canvas shows the screen as served, full screen
 * and Present as the user left it: a modal opened there); a new version starts again.
 */
export function mergeFound(known: FoundIn | undefined, src: string, answer: Readonly<Record<string, boolean>>): FoundIn {
  if (!known || known.src !== src) return { src, selectors: { ...answer } };
  const selectors = { ...known.selectors };
  for (const [selector, found] of Object.entries(answer)) selectors[selector] = found || selectors[selector] === true;
  return { src, selectors };
}

/** A pin whose captured element is no longer found in its screen. */
export function isStale(comment: DesignComment, found: Readonly<Record<string, boolean>> | null): boolean {
  const selector = comment.pin?.element?.selector;
  if (!selector || !found) return false;
  return found[selector] === false;
}

/** The selectors to check in a screen: those of its open pins with a captured element. */
export function selectorsToCheck(threads: readonly Thread[], screen: string): string[] {
  const selectors = new Set<string>();
  for (const t of threads) {
    const selector = t.comment.pin?.element?.selector;
    if (t.comment.screen === screen && !t.comment.resolved && selector) selectors.add(selector);
  }
  return [...selectors];
}

/** Whether the user may edit or delete a comment: theirs, and not sent to the agent yet. */
export function canChange(comment: DesignComment, me: CommentAuthor): boolean {
  return comment.author.email === me.email && comment.sent == null;
}

/**
 * A thread's place in words, for the panel: a markdown file's passage, else the element's text,
 * else the position, else the whole screen (or file, for a file's comment).
 */
export function placeOf(comment: DesignComment): string {
  if (comment.quote) return `On “${comment.quote}”`;
  const pin = comment.pin;
  if (!pin) return /\.(md|mdx|markdown)$/i.test(comment.screen) ? "Whole file" : "Whole screen";
  const text = pin.element?.text?.trim();
  if (text) return `On “${text}”`;
  return `At ${Math.round(pin.x)}%, ${Math.round(pin.y)}%`;
}

/** What the capture script answers: the element under a point, or which selectors are still found. */
export type CaptureReply =
  | { kind: "element"; request: number; element: { selector: string; text: string } | null }
  | { kind: "check"; request: number; found: Record<string, boolean> };

export function parseCaptureReply(data: unknown): CaptureReply | null {
  if (!data || typeof data !== "object") return null;
  const m = data as Record<string, JsonValue | undefined>;
  if (typeof m.request !== "number") return null;
  if (m.type === "chauffeur:element") {
    const el = m.element;
    if (el == null) return { kind: "element", request: m.request, element: null };
    if (typeof el !== "object" || Array.isArray(el)) return null;
    const selector = el.selector;
    const text = el.text;
    if (typeof selector !== "string" || typeof text !== "string") return null;
    return { kind: "element", request: m.request, element: { selector, text } };
  }
  if (m.type === "chauffeur:check") {
    const found = m.found;
    if (!found || typeof found !== "object" || Array.isArray(found)) return null;
    const out: Record<string, boolean> = {};
    for (const [selector, value] of Object.entries(found)) out[selector] = value === true;
    return { kind: "check", request: m.request, found: out };
  }
  return null;
}

/** The screen's URL with the capture script: for the canvas only, never Present or full screen. */
export function withCapture(src: string): string {
  return `${src}${src.includes("?") ? "&" : "?"}capture=1`;
}

/**
 * "Keep these": the selected screens by section, asking to drop the other sections and the
 * other screens of theirs; null without a selection.
 */
export function keepMessage(designChat: string, sections: readonly ShownSection[], selection: readonly string[]): string | null {
  const kept = sections
    .map((section) => ({ section, screens: section.screens.filter((s) => selection.includes(s.file)) }))
    .filter((entry) => entry.screens.length > 0);
  if (kept.length === 0) return null;
  const lines = ["Keep these screens and remove the other sections (and the other screens of these sections) from canvas.json and the folder:"];
  for (const { section, screens } of kept) {
    lines.push("", `${section.title}:`);
    for (const screen of screens) lines.push(`- ${screen.label?.trim() || screen.file}: \`${screenPath(designChat, screen.file)}\``);
  }
  return lines.join("\n");
}
