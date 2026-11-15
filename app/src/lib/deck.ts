// A slide deck's reader (docs/adr/slide-decks.md): what a slide's script says to the app, where a
// slide lies, how a slide is scaled into a box, and the order of slides after a drag.

import type { DeckSlide } from "@chauffeur/protocol";

/** What the daemon's deck_reader.js posts from a slide. */
export type ReaderMessage =
  | { kind: "edit"; tag: string; index: number; before: string; html: string; overflow: boolean }
  | { kind: "editing"; editing: boolean }
  | { kind: "key"; key: string };

export function parseReaderMessage(data: unknown): ReaderMessage | null {
  if (!data || typeof data !== "object") return null;
  const m = data as Record<string, unknown>;
  if (m.type === "chauffeur:edit") {
    if (typeof m.tag !== "string" || !/^[a-z][a-z0-9-]*$/.test(m.tag)) return null;
    if (typeof m.index !== "number" || !Number.isInteger(m.index) || m.index < 0) return null;
    if (typeof m.before !== "string" || typeof m.html !== "string") return null;
    return { kind: "edit", tag: m.tag, index: m.index, before: m.before, html: m.html, overflow: m.overflow === true };
  }
  if (m.type === "chauffeur:editing" && typeof m.editing === "boolean") return { kind: "editing", editing: m.editing };
  if (m.type === "chauffeur:key" && typeof m.key === "string") return { kind: "key", key: m.key };
  return null;
}

/** A slide's file as `files.read` names it: beside its deck.json. */
export function slidePath(deckPath: string, file: string): string {
  const dir = deckPath.slice(0, deckPath.lastIndexOf("/"));
  return `${dir}/${file}`;
}

/** The scale that fits a slide `width` × `height` into a box, never larger than 1. */
export function fitScale(boxWidth: number, boxHeight: number, width: number, height: number): number {
  if (boxWidth <= 0 || boxHeight <= 0 || width <= 0 || height <= 0) return 0;
  return Math.min(1, boxWidth / width, boxHeight / height);
}

/** The files in their new order once the slide at `from` is dropped before the one at `to` (or last). */
export function moved(files: readonly string[], from: number, to: number): string[] {
  if (from < 0 || from >= files.length) return [...files];
  const out = [...files];
  const [file] = out.splice(from, 1);
  const at = Math.max(0, Math.min(out.length, to > from ? to - 1 : to));
  out.splice(at, 0, file!);
  return out;
}

/** The slide to show after `key` from slide `at`, among those `present` may show; null for a key that does not move. */
export function step(slides: readonly DeckSlide[], at: number, key: string, present: boolean): number | null {
  const shown = slides.map((s, i) => ({ s, i })).filter(({ s }) => !present || !s.skip).map(({ i }) => i);
  if (shown.length === 0) return null;
  const position = Math.max(0, shown.findIndex((i) => i >= at));
  switch (key) {
    case "ArrowRight":
    case "ArrowDown":
    case "PageDown":
    case " ":
      return shown[Math.min(shown.length - 1, (shown[position] === at ? position : position - 1) + 1)] ?? null;
    case "ArrowLeft":
    case "ArrowUp":
    case "PageUp":
      return shown[Math.max(0, position - 1)] ?? null;
    case "Home":
      return shown[0] ?? null;
    case "End":
      return shown[shown.length - 1] ?? null;
    default:
      return null;
  }
}
