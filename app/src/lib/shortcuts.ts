// The app's keyboard shortcuts in one place: their default keys, what they do and where, and the
// keys the user gave some of them (`[keyboard.bindings]` of settings.toml, Settings › Keyboard).
// Handlers ask `pressed(event, "chat.new")` rather than reading keys themselves. Keys are written the
// macOS way, as `kbd()` labels them: ⌘ is Ctrl elsewhere, ⌃ (macOS's Control) Ctrl+Alt there, ⌥
// Alt. A shortcut of one view's tabs (a new chat, a new terminal) shares its keys with another view's.
// Keys other handlers read themselves (find in a chat, ⌘[ / ⌘] between workspaces, the Files editor's
// ⌘S, ⇧⌘W of the macOS menu)
// are listed as fixed, so that a key given to a shortcut that they already take shows as a conflict.
// Typing keys (the composer's ↵, a list's arrows) are not shortcuts here.

import type { ShortcutId } from "@chauffeur/protocol";
import { asElement, fromTerminal, inForeignEditor, isMac, kbd } from "./keys";
import type { ViewId } from "./splitLayout";

/** A shortcut whose keys cannot change: a row of digits, or keys another handler reads itself. */
type FixedId =
  | "workspace.goto"
  | "workspace.back"
  | "workspace.forward"
  | "model.pick"
  | "window.close"
  | "chat.find"
  | "find.step"
  | "files.save"
  | "composer.queue";
export type Id = ShortcutId | FixedId;

/** The keys the user gave shortcuts, by id. */
export type Overrides = Partial<Record<ShortcutId, string>>;

export type Shortcut = {
  /** What it does. */
  label: string;
  /** Where it acts. */
  where: string;
  /** Its keys on macOS, and elsewhere when they differ (none: macOS only). */
  mac: string[];
  other?: string[];
  /**
   * In the terminal outside macOS: "shell", its default keys stay the shell's (Ctrl+T, Ctrl+K…);
   * "app", the app takes them from the shell. Keys the user gave act there whatever this says.
   */
  terminal?: "shell" | "app";
  /** Its keys cannot change. */
  fixed?: true;
  /** Its keys in one label, for a row of digits. */
  range?: string;
  /**
   * The view whose tabs it acts on: "chat" while the Chat view is shown and the Terminal view is not
   * the one in use, "terminal" while it is. Two shortcuts of different views may share keys.
   */
  view?: "chat" | "terminal";
};

const digits = (modifiers: string) => Array.from({ length: 9 }, (_, i) => `${modifiers}${i + 1}`);

export const SHORTCUTS: Record<Id, Shortcut> = {
  "workspace.new": { label: "New workspace", where: "Anywhere", mac: ["⌘N"], terminal: "shell" },
  "exploration.new": { label: "New exploration, read-only", where: "Anywhere", mac: ["⇧⌘N"], terminal: "shell" },
  "switcher.open": { label: "Switch workspace", where: "Anywhere", mac: ["⌘K"], terminal: "shell" },
  "workspace.goto": { label: "Go to a workspace of the sidebar", where: "Anywhere", mac: digits("⌘"), terminal: "shell", fixed: true, range: "⌘1–9" },
  // `historyKey` reads them, by the key's place too: AZERTY types "[" with Shift.
  "workspace.back": { label: "The previous workspace shown", where: "Anywhere", mac: ["⌘["], terminal: "shell", fixed: true },
  "workspace.forward": { label: "The next workspace shown", where: "Anywhere", mac: ["⌘]"], terminal: "shell", fixed: true },
  "sidebar.toggle": { label: "Hide or show the sidebar", where: "Anywhere", mac: ["⌘B"], terminal: "shell" },
  "settings.open": { label: "Settings", where: "Anywhere", mac: ["⌘,"], terminal: "shell" },
  // The macOS menu's (File › Close Window); Linux has no menu.
  "window.close": { label: "Close the window", where: "Anywhere", mac: ["⇧⌘W"], other: [], fixed: true },
  "office.show": { label: "The 3D Office, and back", where: "Anywhere", mac: ["⇧⌘O"], terminal: "shell" },
  "zoom.in": { label: "Zoom in", where: "Anywhere", mac: ["⌘=", "⌘+"], terminal: "shell" },
  "zoom.out": { label: "Zoom out", where: "Anywhere", mac: ["⌘-"], terminal: "shell" },
  "zoom.reset": { label: "Actual size", where: "Anywhere", mac: ["⌘0"], terminal: "shell" },
  "files.goto": { label: "Go to a file", where: "A workspace", mac: ["⌘P"], terminal: "shell" },
  "files.search": { label: "Search the files", where: "A workspace", mac: ["⇧⌘F"], terminal: "shell" },
  "open.app": { label: "Open the workspace in an app", where: "A workspace", mac: ["⌘O"], terminal: "shell" },
  "view.split": { label: "Open or close the side pane", where: "A workspace", mac: ["⌘\\"], terminal: "shell" },
  "chat.new": { label: "New chat", where: "The Chat view", mac: ["⌘T"], terminal: "shell", view: "chat" },
  "chat.close": { label: "Close the chat", where: "The Chat view", mac: ["⌘W"], terminal: "shell", view: "chat" },
  "chat.reopen": {
    label: "Reopen the last closed chat",
    where: "The Chat view",
    mac: ["⇧⌘T"],
    terminal: "shell",
    view: "chat",
  },
  // Outside macOS, Ctrl+T and Ctrl+W stay the shell's (fzf, deleting a word), as in terminal apps.
  "terminal.new": { label: "New terminal", where: "The Terminal view", mac: ["⌘T"], other: ["⇧⌘T"], terminal: "app", view: "terminal" },
  "terminal.close": {
    label: "Close the terminal",
    where: "The Terminal view",
    mac: ["⌘W"],
    other: ["⇧⌘W"],
    terminal: "app",
    view: "terminal",
  },
  "tab.next": {
    label: "Next tab",
    where: "Chats and terminals",
    mac: ["⌃Tab", "⌘PgDn", "⇧⌘]"],
    other: ["⌘Tab", "⌘PgDn"],
    terminal: "app",
  },
  "tab.previous": {
    label: "Previous tab",
    where: "Chats and terminals",
    mac: ["⌃⇧Tab", "⌘PgUp", "⇧⌘["],
    other: ["⇧⌘Tab", "⌘PgUp"],
    terminal: "app",
  },
  // ⌃` by the key's place: ⌃² on AZERTY.
  "view.toggle-terminal": { label: "The Terminal view, or back to the Chat", where: "A workspace", mac: ["⌃`"], other: ["⌘`"], terminal: "app" },
  // Alt+1–5 outside macOS, where Ctrl+Alt+1–9 are the model favorites' (readline's Alt+digit is lost
  // in the terminal); on macOS ⌥ alone types a character.
  "view.chat": { label: "The Chat view", where: "A workspace", mac: ["⌥⌘1"], other: ["⌥1"], terminal: "app" },
  "view.diff": { label: "The Diff view", where: "A workspace", mac: ["⌥⌘2"], other: ["⌥2"], terminal: "app" },
  "view.files": { label: "The Files view", where: "A workspace", mac: ["⌥⌘3"], other: ["⌥3"], terminal: "app" },
  "view.terminal": { label: "The Terminal view", where: "A workspace", mac: ["⌥⌘4"], other: ["⌥4"], terminal: "app" },
  "view.database": { label: "The Database view", where: "A workspace", mac: ["⌥⌘6"], other: ["⌥6"], terminal: "app" },
  "view.processes": { label: "The Processes view", where: "A workspace", mac: ["⌥⌘5"], other: ["⌥5"], terminal: "app" },
  "chat.find": { label: "Find in the chat", where: "A chat", mac: ["⌘F"], fixed: true },
  "find.step": { label: "Next or previous match", where: "A chat's find bar", mac: ["⌘G", "⇧⌘G"], fixed: true },
  "chat.top": { label: "The chat's first message", where: "A chat", mac: ["⌘↑"] },
  "chat.bottom": { label: "The chat's latest message", where: "A chat", mac: ["⌘↓"] },
  "composer.queue": { label: "Queue the message for after the agent's turn", where: "The composer", mac: ["⌘↵"], fixed: true },
  "model.pick": { label: "Pick a favorite model", where: "The composer", mac: digits("⌃⌘"), fixed: true, range: "⌃⌘1–9" },
  "model.effort": { label: "Next effort level", where: "The composer", mac: ["⇧⌘/"] },
  "model.fast": { label: "Fast mode on or off", where: "The composer", mac: ["⇧⌘E"] },
  "files.save": { label: "Save the file", where: "The Files editor", mac: ["⌘S"], fixed: true },
};

export const SHORTCUT_IDS = Object.keys(SHORTCUTS) as Id[];

let active: Overrides = {};

/** Puts the user's keys in force, as the daemon read them from settings.toml. */
export function setOverrides(overrides: Overrides) {
  active = overrides;
}

/** The keys in force for `id`: the user's, else its defaults on this platform. */
export function keysOf(id: Id, mac = isMac, overrides = active): string[] {
  const own = overrides[id as ShortcutId];
  if (own !== undefined) return [own];
  const shortcut = SHORTCUTS[id];
  return mac ? shortcut.mac : (shortcut.other ?? shortcut.mac);
}

/** `id`'s keys as this platform names them: "Ctrl+Tab / Ctrl+PgDn". */
export function labelOf(id: Id, mac = isMac, overrides = active): string {
  const range = SHORTCUTS[id].range;
  return range ? kbd(range, mac) : keysOf(id, mac, overrides).map((keys) => kbd(keys, mac)).join(" / ");
}

type Binding = { ctrl: boolean; alt: boolean; shift: boolean; cmd: boolean; key: string };

/** Keys a binding names besides a single character, and the `KeyboardEvent.key` of each. */
const NAMED: Record<string, string> = {
  Tab: "Tab",
  PgUp: "PageUp",
  PgDn: "PageDown",
  Home: "Home",
  End: "End",
  Space: " ",
  Del: "Delete",
  "↑": "ArrowUp",
  "↓": "ArrowDown",
  "←": "ArrowLeft",
  "→": "ArrowRight",
  "↵": "Enter",
};
const FUNCTION_KEY = /^F([1-9]|1[0-2])$/;

/** Where a character sits on a US keyboard, to find it under Shift (⇧⌘/ types "?"). */
const US_CODES: Record<string, string> = {
  "/": "Slash",
  "\\": "Backslash",
  "[": "BracketLeft",
  "]": "BracketRight",
  ",": "Comma",
  ".": "Period",
  ";": "Semicolon",
  "'": "Quote",
  "`": "Backquote",
  "-": "Minus",
  "=": "Equal",
};

/**
 * A binding's modifiers and key; null when it is not one: modifiers among ⌃ ⌥ ⇧ ⌘ (each once, ⌘, ⌃
 * or ⌥ among them), then one key, a printable ASCII character or a named one. The daemon checks the
 * same (`binding_error` in settings.rs).
 */
export function parseBinding(keys: string): Binding | null {
  const chars = Array.from(keys);
  const modifiers: string[] = [];
  while (chars.length > 0 && "⌃⌥⇧⌘".includes(chars[0])) {
    if (modifiers.includes(chars[0])) return null;
    modifiers.push(chars.shift()!);
  }
  const key = chars.join("");
  const valid = /^[\x21-\x7e]$/.test(key) || FUNCTION_KEY.test(key) || key in NAMED;
  if (!valid || !(modifiers.includes("⌘") || modifiers.includes("⌃") || modifiers.includes("⌥"))) return null;
  return {
    ctrl: modifiers.includes("⌃"),
    alt: modifiers.includes("⌥"),
    shift: modifiers.includes("⇧"),
    cmd: modifiers.includes("⌘"),
    key: key.length === 1 ? key.toUpperCase() : key,
  };
}

/**
 * The one way of writing `keys` on this platform, to compare two: outside macOS, as Ctrl and Alt
 * (⌃ being both, ⌥⌘ the same Ctrl+Alt).
 */
function canonical(keys: string, mac: boolean): string | null {
  const binding = parseBinding(keys);
  if (!binding) return null;
  const { ctrl, alt, shift, cmd, key } = binding;
  if (mac) return `${ctrl ? "⌃" : ""}${alt ? "⌥" : ""}${shift ? "⇧" : ""}${cmd ? "⌘" : ""}${key}`;
  return `${ctrl || alt ? "⌥" : ""}${shift ? "⇧" : ""}${cmd || ctrl ? "⌘" : ""}${key}`;
}

type Press = Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey"> &
  Partial<Pick<KeyboardEvent, "getModifierState">>;

function matches({ ctrl, alt, shift, cmd, key }: Binding, event: Press, mac: boolean): boolean {
  const modifiers = mac
    ? event.metaKey === cmd && event.ctrlKey === ctrl && (event.altKey || !alt)
    : event.ctrlKey === (cmd || ctrl) && event.altKey === (ctrl || alt) && !event.metaKey;
  if (!modifiers) return false;
  // AltGr types characters (AZERTY's ~ # { [ on the digits), never a shortcut.
  if (!mac && event.getModifierState?.("AltGraph")) return false;
  // macOS's ⌥ in a binding types another character: its key is found by its place. Pressed for a
  // binding without it, it only counts as typing a character below.
  const placed = mac && alt;
  const option = mac && event.altKey && !alt;
  const named = NAMED[key] ?? (FUNCTION_KEY.test(key) ? key : undefined);
  if (named !== undefined) return event.key === named && event.shiftKey === shift && !option;
  if (/^[A-Z]$/.test(key)) {
    // The letter typed; on a non-Latin layout, or under ⌥, the letter's place.
    const typed = !placed && /^[\x20-\x7e]$/.test(event.key) ? event.key.toUpperCase() === key : event.code === `Key${key}`;
    return typed && event.shiftKey === shift && !option;
  }
  if (/^[0-9]$/.test(key)) {
    // By the key's place: on AZERTY the digit row types "&é"… and takes Shift for digits.
    const place = event.code === `Digit${key}` || event.code === `Numpad${key}` || event.key === key;
    return place && (!shift || event.shiftKey) && !option;
  }
  if (placed) return event.shiftKey === shift && US_CODES[key] === event.code;
  // Any other character: the one typed, whatever Shift or ⌥ it took on this layout (\ is ⌥⇧/ on a
  // French Mac); else, under Shift, by its place on a US keyboard.
  if (event.key === key) return !shift || event.shiftKey;
  // \ and ` by their place too: many layouts type another character there (# on a German keyboard,
  // ² on a French one).
  if ((key === "\\" || key === "`") && !shift && event.code === US_CODES[key]) return !event.shiftKey && !option;
  return shift && event.shiftKey && !option && US_CODES[key] === event.code;
}

/**
 * Whether the event means `id`. In the terminal outside macOS, a shortcut whose default keys are the
 * shell's acts only on keys the user gave it.
 */
export function pressed(event: Press & Pick<Event, "target">, id: Id, mac = isMac, overrides = active): boolean {
  if (SHORTCUTS[id].terminal === "shell" && overrides[id as ShortcutId] === undefined && fromTerminal(event, mac)) return false;
  return keysOf(id, mac, overrides).some((keys) => {
    const binding = parseBinding(keys);
    return binding !== null && matches(binding, event, mac);
  });
}

/** Whether a key pressed in the terminal is the app's, not the shell's: a shortcut the app takes from
 * it (switching tabs or views, a new terminal), or one the user gave keys to. */
export function forApp(event: Press & Pick<Event, "target">, mac = isMac, overrides = active): boolean {
  return SHORTCUT_IDS.some(
    (id) => (SHORTCUTS[id].terminal === "app" || overrides[id as ShortcutId] !== undefined) && pressed(event, id, mac, overrides),
  );
}

/**
 * Keys no shortcut may take, and what they are: the Edit menu's and macOS's own, and the terminal's
 * that a running program needs (outside macOS ⌘ is Ctrl: Ctrl+C interrupts, Ctrl+D ends input,
 * Ctrl+Z suspends; on macOS that is ⌃). The daemon refuses the same (`RESERVED` in settings.rs).
 */
export const RESERVED: Record<string, string> = {
  "⌘A": "Select All",
  "⌘C": "Copy, and the terminal's interrupt outside macOS",
  "⌘D": "the terminal's end of input outside macOS",
  "⌘H": "macOS's Hide",
  "⌘M": "macOS's Minimize",
  "⌘Q": "Quit",
  "⌘V": "Paste",
  "⌘X": "Cut",
  "⌘Z": "Undo, and the terminal's suspend outside macOS",
  "⇧⌘Z": "Redo",
  "⌃C": "the terminal's interrupt on macOS",
  "⌃D": "the terminal's end of input on macOS",
  "⌃Z": "the terminal's suspend on macOS",
  "⌃⌘F": "macOS's Full Screen",
};

/** What reserved `keys` are (as written, whatever the platform), or null. */
export function reservedAs(keys: string): string | null {
  const written = canonical(keys, true);
  return (written && RESERVED[written]) ?? null;
}

const MODIFIER_KEYS = ["Shift", "Control", "Alt", "Meta", "AltGraph", "CapsLock", "OS"];

/** The keys held, as this platform names them, ending in "…" while only modifiers are: "⇧⌘…", "Ctrl+K". */
function typedLabel(event: Press, key: string | null, mac: boolean): string {
  const rest = key === null ? "…" : key;
  if (mac) return `${event.ctrlKey ? "⌃" : ""}${event.altKey ? "⌥" : ""}${event.shiftKey ? "⇧" : ""}${event.metaKey ? "⌘" : ""}${rest}`;
  const names = [event.ctrlKey && "Ctrl", event.altKey && "Alt", event.shiftKey && "Shift", event.metaKey && "Super"].filter(Boolean);
  return [...names, kbd(rest, false)].join("+");
}

/** What Settings › Keyboard records from a key press. */
export type Recorded = {
  /** What was pressed, as this platform names it. */
  typed: string;
  /** The binding, written the macOS way (outside macOS Ctrl is ⌘, Ctrl+Alt ⌃⌘). */
  keys?: string;
  /** Why the keys pressed cannot be a binding. */
  refused?: string;
};

/**
 * What a key press records: the binding, or why there is none (a key without ⌘ or ⌃, or outside
 * macOS without Ctrl or Alt; AltGr, which types characters and never makes a shortcut; a key outside
 * ASCII; a reserved key). While only modifiers are held, `typed` alone says which.
 */
export function recordBinding(event: Press, mac = isMac): Recorded {
  if (MODIFIER_KEYS.includes(event.key)) return { typed: typedLabel(event, null, mac) };
  const cmd = mac ? event.metaKey : event.ctrlKey;
  const ctrl = mac ? event.ctrlKey : event.ctrlKey && event.altKey;
  const alt = mac ? event.altKey : event.altKey && !event.ctrlKey;
  const named = Object.keys(NAMED).find((name) => NAMED[name] === event.key) ?? (FUNCTION_KEY.test(event.key) ? event.key : undefined);
  const digit = /^(?:Digit|Numpad)([0-9])$/.exec(event.code)?.[1];
  const ascii = /^[\x21-\x7e]$/.test(event.key) ? event.key.toUpperCase() : undefined;
  const letter = /^Key([A-Z])$/.exec(event.code)?.[1];
  const place = Object.keys(US_CODES).find((char) => US_CODES[char] === event.code);
  // Under macOS's ⌥, which types another character, the key's place names it; \ and ` are found by
  // their place too, as `matches` finds them (² on AZERTY).
  const byPlace = place === "\\" || place === "`" ? place : undefined;
  const character =
    alt && mac
      ? (letter ?? place)
      : ascii && /^[A-Z]$/.test(ascii)
        ? ascii
        : ((event.shiftKey ? place : undefined) ?? ascii ?? letter ?? byPlace);
  const key = named ?? digit ?? character;
  const typed = typedLabel(event, key ?? (event.key.length === 1 ? event.key.toUpperCase() : event.key), mac);
  if (!mac && event.getModifierState?.("AltGraph")) return { typed, refused: "AltGr types characters: a shortcut takes Ctrl, Alt and Shift" };
  if (!mac && event.metaKey) return { typed, refused: "The Super key is the desktop's: a shortcut takes Ctrl, Alt and Shift" };
  if (!(cmd || ctrl || (alt && !mac))) return { typed, refused: mac ? "A shortcut needs ⌘ or ⌃" : "A shortcut needs Ctrl or Alt" };
  if (!key) return { typed, refused: `${event.key} is not a key a shortcut takes: use a letter, a digit, an ASCII sign or a named key` };
  const keys = `${ctrl ? "⌃" : ""}${alt ? "⌥" : ""}${event.shiftKey ? "⇧" : ""}${cmd ? "⌘" : ""}${key}`;
  const reserved = reservedAs(keys);
  if (reserved) return { typed, refused: `${kbd(keys, mac)} stays ${reserved}: press other keys` };
  return { typed, keys };
}

/** The overrides once `id` takes `keys`: none for it when they are its only default. */
export function rebind(overrides: Overrides, id: ShortcutId, keys: string, mac = isMac): Overrides {
  const others = { ...overrides };
  delete others[id];
  const defaults = keysOf(id, mac, {});
  return defaults.length === 1 && canonical(defaults[0], mac) === canonical(keys, mac) ? others : { ...others, [id]: keys };
}

/** The keys `keys` answers to, each written one way: a digit without ⇧ takes it with ⇧ too (`matches`). */
function covered(keys: string, mac: boolean): string[] {
  const binding = parseBinding(keys);
  const one = canonical(keys, mac);
  if (!binding || !one) return [];
  return /^[0-9]$/.test(binding.key) && !binding.shift ? [one, canonical(`⇧${keys}`, mac)!] : [one];
}

/** The other shortcuts that `id`'s keys also trigger: not those of another view's tabs. */
export function conflictsOf(id: Id, mac = isMac, overrides = active): Id[] {
  const mine = new Set(keysOf(id, mac, overrides).flatMap((keys) => covered(keys, mac)));
  const view = SHORTCUTS[id].view;
  return SHORTCUT_IDS.filter((other) => {
    const theirs = SHORTCUTS[other].view;
    if (other === id || (view && theirs && view !== theirs)) return false;
    return keysOf(other, mac, overrides).some((keys) => covered(keys, mac).some((one) => mine.has(one)));
  });
}

/** What a key does to the tabs of the chat or terminal view. */
export type TabKey = "new" | "close" | "reopen" | "next" | "previous";

const TAB_KEYS: Array<[TabKey, Id]> = [
  ["new", "chat.new"],
  ["close", "chat.close"],
  ["reopen", "chat.reopen"],
  ["next", "tab.next"],
  ["previous", "tab.previous"],
];

const TERMINAL_TAB_KEYS: Array<[TabKey, Id]> = [
  ["new", "terminal.new"],
  ["close", "terminal.close"],
  ["next", "tab.next"],
  ["previous", "tab.previous"],
];

/**
 * The tab shortcut a key press means: by default ⌘T opens a new chat, ⌘W closes the one shown, ⇧⌘T
 * reopens the last closed; Ctrl+Tab and Ctrl+Shift+Tab (on macOS too, as in browsers), ⌘PageDown and
 * ⌘PageUp, and on macOS ⇧⌘] and ⇧⌘[, show the next or previous tab. In the terminal outside macOS,
 * Ctrl+T and Ctrl+W stay the shell's; the tab-switching keys, which shells hardly use, are the app's.
 */
export function tabKey(event: Press & Pick<Event, "target">, mac = isMac, overrides = active): TabKey | null {
  return TAB_KEYS.find(([, id]) => pressed(event, id, mac, overrides))?.[0] ?? null;
}

/**
 * The same in the Terminal view: by default ⌘T opens a terminal and ⌘W closes the one shown;
 * outside macOS, where Ctrl+T and Ctrl+W stay the shell's, Ctrl+Shift+T and Ctrl+Shift+W.
 */
export function terminalTabKey(event: Press & Pick<Event, "target">, mac = isMac, overrides = active): TabKey | null {
  return TERMINAL_TAB_KEYS.find(([, id]) => pressed(event, id, mac, overrides))?.[0] ?? null;
}

const VIEW_KEYS: Array<[ViewId, Id]> = [
  ["chat", "view.chat"],
  ["diff", "view.diff"],
  ["files", "view.files"],
  ["terminal", "view.terminal"],
  ["processes", "view.processes"],
  ["database", "view.database"],
];

/** The shortcut that shows `view`, for its tab's tooltip. */
export function viewShortcut(view: ViewId): Id {
  return VIEW_KEYS.find(([one]) => one === view)?.[1] ?? "view.chat";
}

/**
 * The view a key press shows, from `current`, the view in use: by default ⌥⌘1–5 (Alt+1–5
 * outside macOS) show Chat, Diff, Files, Terminal or Processes; ⌃` (Ctrl+` outside macOS) the
 * Terminal, and from there the Chat again. The terminal and the editors leave them to the app.
 */
export function viewKey(event: Press & Pick<Event, "target">, current: ViewId, mac = isMac, overrides = active): ViewId | null {
  if (pressed(event, "view.toggle-terminal", mac, overrides)) return current === "terminal" ? "chat" : "terminal";
  return VIEW_KEYS.find(([, id]) => pressed(event, id, mac, overrides))?.[0] ?? null;
}

/** A shortcut pressed and its macOS menu item fired for the same key press act once; outside macOS,
 * which has no such menu, every press acts. */
const lastRun: Record<string, number> = {};
export function onceIn(action: string, run: () => void, now = Date.now(), mac = isMac) {
  if (mac && now - (lastRun[action] ?? 0) < 400) return;
  lastRun[action] = now;
  run();
}

/**
 * Where a key sends the chat: ⌘↑ / ⌘↓ by default (Ctrl elsewhere) to its first or latest message,
 * and Home / End when no text field has the focus. A text field with text keeps its keys to move its
 * caret; the terminal and the Files editor keep them all.
 */
export function chatJump(event: Press & Pick<Event, "target">, mac = isMac, overrides = active): "top" | "bottom" | null {
  const target = event.target as { tagName?: string; isContentEditable?: boolean; value?: string } | null;
  const field = target?.tagName === "TEXTAREA" || target?.tagName === "INPUT" || target?.isContentEditable === true;
  if (inForeignEditor(event.target)) return null;
  // The chat composer is an editor that is a text field all the same, and says whether it holds text.
  const composer = asElement(event.target)?.closest("[data-composer]")?.getAttribute("data-composer");
  const jump = pressed(event, "chat.top", mac, overrides) ? "top" : pressed(event, "chat.bottom", mac, overrides) ? "bottom" : null;
  if (jump) return field && (composer === "text" || (target?.value ?? "") !== "") ? null : jump;
  if (field || event.altKey || event.shiftKey || event.metaKey || event.ctrlKey) return null;
  return event.key === "Home" ? "top" : event.key === "End" ? "bottom" : null;
}
