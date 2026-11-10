// What the user started writing in a chat's composer, kept when they look at another chat or
// workspace and come back: the text survives a restart of the app, the attachments only while it runs.
// A composer shown while its draft is written from elsewhere (a design chat's comments, "Keep
// these") is told, so that it takes the text.

import type { PendingImage } from "./images";

const KEY = "chauffeur.drafts";
/** Drafts kept at most: the oldest go first. */
const MAX = 200;

type Store = Pick<Storage, "getItem" | "setItem">;

export class Drafts {
  private texts: Record<string, string>;
  private attached = new Map<string, PendingImage[]>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private listeners = new Map<string, Set<(text: string) => void>>();

  constructor(private storage: Store | null) {
    try {
      this.texts = JSON.parse(storage?.getItem(KEY) ?? "{}") ?? {};
    } catch {
      this.texts = {};
    }
  }

  text(key: string): string {
    return this.texts[key] ?? "";
  }

  setText(key: string, text: string) {
    if ((this.texts[key] ?? "") === text) return;
    delete this.texts[key];
    if (text) this.texts[key] = text;
    for (const listener of this.listeners.get(key) ?? []) listener(text);
    const keys = Object.keys(this.texts);
    for (const old of keys.slice(0, Math.max(0, keys.length - MAX))) delete this.texts[old];
    // Written a moment after the last keystroke, not on every one.
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => this.flush(), 300);
  }

  /** Called with a chat's new text each time it changes; returns what stops the calls. */
  subscribe(key: string, listener: (text: string) => void): () => void {
    const set = this.listeners.get(key) ?? new Set();
    set.add(listener);
    this.listeners.set(key, set);
    return () => {
      set.delete(listener);
      if (set.size === 0) this.listeners.delete(key);
    };
  }

  files(key: string): PendingImage[] {
    return this.attached.get(key) ?? [];
  }

  setFiles(key: string, files: PendingImage[]) {
    if (files.length) this.attached.set(key, files);
    else this.attached.delete(key);
  }

  flush() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.storage?.setItem(KEY, JSON.stringify(this.texts));
  }
}

export const drafts = new Drafts(typeof localStorage === "undefined" ? null : localStorage);

// A draft typed just before the app closes is written all the same.
if (typeof window !== "undefined") window.addEventListener("beforeunload", () => drafts.flush());
