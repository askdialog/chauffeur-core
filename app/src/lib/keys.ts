// Shortcuts use ⌘ on macOS and Ctrl elsewhere; on Linux a second modifier (macOS's ⌃) is Alt.
// Labels are written the macOS way, `kbd("⌘⇧E")`, and translated for other platforms.

export const isMac =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

type Modifiers = Pick<KeyboardEvent, "metaKey" | "ctrlKey" | "altKey">;

/** ⌘ on macOS, Ctrl elsewhere. */
export function hasMod(event: Modifiers, mac = isMac): boolean {
  return mac ? event.metaKey : event.ctrlKey;
}

/** The modifier added to ⌘ for a second set of shortcuts: ⌃ on macOS, Alt elsewhere. */
export function hasSecondMod(event: Modifiers, mac = isMac): boolean {
  return mac ? event.ctrlKey : event.altKey;
}

/**
 * Whether the event comes from the embedded terminal. Outside macOS, Ctrl+K, Ctrl+N or Ctrl+O mean
 * something to the shell there, so app shortcuts leave them alone.
 */
export function fromTerminal(event: Pick<Event, "target">, mac = isMac): boolean {
  const target = event.target as { closest?: (selector: string) => unknown } | null;
  return !mac && typeof target?.closest === "function" && target.closest(".xterm") != null;
}

/** Whether the event comes from the Files editor in Vim mode, whose Esc is Vim's (leaving insert
 * mode), not the app's. */
export function fromVimEditor(event: Event): boolean {
  const target = event.target as { closest?: (selector: string) => unknown } | null;
  return typeof target?.closest === "function" && target.closest("[data-vim]") != null;
}

/**
 * The digit key pressed (1–9), by its place on the keyboard: on AZERTY the same key types "&"
 * without Shift, and ⌘⇧3–5 are macOS's screenshots.
 */
export function digitOf(event: Pick<KeyboardEvent, "code">): number | null {
  const match = /^(?:Digit|Numpad)([1-9])$/.exec(event.code);
  return match ? Number(match[1]) : null;
}

/** A shortcut's label for this platform: `kbd("⌃⌘1")` and `kbd("⌥⌘1")` are "Ctrl+Alt+1" outside macOS. */
export function kbd(mac: string, onMac = isMac): string {
  if (onMac) return mac;
  const keys: string[] = [];
  let rest = mac;
  for (const [symbol, name] of [
    ["⌃", "Ctrl+Alt"],
    ["⌘", "Ctrl"],
    ["⌥", "Alt"],
    ["⇧", "Shift"],
  ] as const) {
    if (rest.includes(symbol)) {
      if (!((symbol === "⌘" || symbol === "⌥") && keys.includes("Ctrl+Alt"))) keys.push(name);
      rest = rest.replace(symbol, "");
    }
  }
  const key = rest.replace("↵", "Enter").replace("⏎", "Enter");
  return [...keys, key].join("+");
}

/** The element an event targets, or null for a text node or no target (and in tests, without a DOM). */
export const asElement = (target: unknown): Element | null =>
  typeof (target as { closest?: unknown } | null)?.closest === "function" ? (target as Element) : null;

/**
 * Whether a key goes to a terminal or an editor that keeps its own shortcuts (the Files editor), so
 * that the chat's (⌘F, ⌘↑) leave it alone. The chat composer is an editor too, but not one of them.
 */
export function inForeignEditor(target: EventTarget | null): boolean {
  const element = asElement(target);
  return !!element && !element.closest("[data-composer]") && !!element.closest(".xterm, .cm-editor");
}

/**
 * ⌘[ goes back to the previous workspace, ⌘] forward again (Ctrl outside macOS); ⇧⌘[ and ⇧⌘] are
 * the tabs'. By the character typed, else by the key's place, since AZERTY types "[" with Shift.
 */
export function historyKey(
  event: Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
  mac = isMac,
): "back" | "forward" | null {
  if (!hasMod(event, mac) || hasSecondMod(event, mac) || event.altKey || event.shiftKey) return null;
  if (event.key === "[" || event.code === "BracketLeft") return "back";
  if (event.key === "]" || event.code === "BracketRight") return "forward";
  return null;
}

/** The tab `step` places after `current` (before it when negative), round the ends; null with no tab. */
export function stepTab(ids: string[], current: string | null | undefined, step: 1 | -1): string | null {
  if (ids.length === 0) return null;
  const index = current == null ? -1 : ids.indexOf(current);
  if (index < 0) return step > 0 ? ids[0] : ids[ids.length - 1];
  return ids[(index + step + ids.length) % ids.length];
}
