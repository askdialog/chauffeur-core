// The user's terminal theme (Settings › Appearance, `terminal_theme`): the terminal takes its
// colours, the code (Files editor, Diff, code in chats) its background, foreground and 16 ANSI
// colours, mapped onto the syntax colours as terminal editors do. With `theme_interface`, the
// interface too: its neutral colours and accent only, the colours that carry a meaning (errors,
// success, attention, working, diffs) stay the app's own.

import type { Palette } from "@chauffeur/protocol";
import type { ITheme } from "@xterm/xterm";

export const DEFAULT_TERMINAL_THEME: ITheme = {
  background: "#0c0c0e",
  foreground: "#d4d4d8",
  cursor: "#d4d4d8",
  selectionBackground: "#3b3f6b",
  black: "#1d1d21",
  brightBlack: "#6c6c75",
};

const ANSI_NAMES = [
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "brightBlack",
  "brightRed",
  "brightGreen",
  "brightYellow",
  "brightBlue",
  "brightMagenta",
  "brightCyan",
  "brightWhite",
] as const;

/** The terminal's colours: the theme's, else the app's own. */
export function terminalTheme(palette: Palette | null | undefined): ITheme {
  if (!palette) return DEFAULT_TERMINAL_THEME;
  const theme: ITheme = {
    background: palette.background,
    foreground: palette.foreground,
    cursor: palette.cursor ?? palette.foreground,
    // A theme without one: its foreground, faint.
    selectionBackground: palette.selection ?? `${palette.foreground}40`,
  };
  ANSI_NAMES.forEach((name, i) => {
    if (palette.ansi[i]) theme[name] = palette.ansi[i];
  });
  return theme;
}

/** The CSS variables a theme sets (index.css holds the app's own values). */
export function paletteVariables(palette: Palette): Record<string, string> {
  const ansi = (i: number) => palette.ansi[i] ?? palette.foreground;
  return {
    "--code-bg": palette.background,
    "--code-fg": palette.foreground,
    "--terminal-bg": palette.background,
    "--hl-comment": ansi(8),
    "--hl-keyword": ansi(5),
    "--hl-string": ansi(2),
    "--hl-number": ansi(3),
    "--hl-function": ansi(4),
    "--hl-type": ansi(6),
    "--hl-property": ansi(11),
    "--hl-tag": ansi(1),
    "--hl-deletion": ansi(1),
    "--hl-meta": ansi(8),
  };
}

/** WCAG relative luminance of a `#rrggbb` colour, 0 (black) to 1 (white). */
export function luminance(hex: string): number {
  const [r, g, b] = channels(hex).map((c) => {
    const v = c / 255;
    return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio of two colours, 1 to 21. */
export function contrast(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (light + 0.05) / (dark + 0.05);
}

/** A light background: black text reads better on it than white. */
export function isLight(background: string): boolean {
  return contrast(background, "#000000") > contrast(background, "#ffffff");
}

/** The hint under Settings › Appearance's "Also colour the interface": what it takes, or why it
 * cannot (no theme file yet, a light theme). */
export function interfaceHint(themePath: string, palette: Palette | null | undefined): string {
  if (!themePath) return "Choose a terminal theme file above first, such as pywal's ~/.cache/wal/colors.json: the interface takes its colours from it.";
  if (palette && isLight(palette.background)) return "Ignored: this theme is light; Chauffeur's interface is dark only.";
  return "Its background, panels, text and accent (its blue), text lightened where it would be hard to read; errors, diffs and statuses keep their colours.";
}

function channels(hex: string): number[] {
  return [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
}

/** `a` moved toward `b` by `t`, from 0 (`a`) to 1 (`b`). */
function mix(a: string, b: string, t: number): string {
  const to = channels(b);
  return `#${channels(a)
    .map((c, i) => Math.round(c + (to[i] - c) * t).toString(16).padStart(2, "0"))
    .join("")}`;
}

/** `colour` moved toward `target` in small steps until it is `fine`; `target` at worst. */
function until(colour: string, target: string, fine: (colour: string) => boolean): string {
  for (let step = 0; step < 20; step++) {
    const moved = mix(colour, target, step / 20);
    if (fine(moved)) return moved;
  }
  return target;
}

/** The interface's colours a dark theme sets (index.css's `@theme` holds the app's own); null for a
 * light theme, since the interface is dark only. Surfaces step up from the theme's background toward
 * white as the app's own greys do. Text is lightened until it reads on the lightest surface it sits
 * on (4.5:1, muted and faint text 3:1). The accent is the theme's blue (color4): terminal themes give
 * it to prompts, links and directories, as the app gives its own blue-violet accent; red, green and
 * yellow already mean danger, success and attention. It is darkened until a button's white label
 * reads on it, then lightened until it reads on the background. */
export function interfaceVariables(palette: Palette): Record<string, string> | null {
  const bg = palette.background;
  if (isLight(bg)) return null;
  const surface = (t: number) => mix(bg, "#ffffff", t);
  const hover = surface(0.079);
  const reads = (ratio: number, on = hover) => (colour: string) => contrast(colour, on) >= ratio;
  const fg = until(palette.foreground, "#ffffff", reads(4.5));
  const dimmed = (t: number) => until(mix(fg, bg, t), fg, reads(3));
  const onWhite = until(palette.ansi[4] ?? fg, "#000000", reads(3, "#ffffff"));
  const accent = until(onWhite, "#ffffff", reads(4.5, bg));
  return {
    "--color-bg": bg,
    "--color-sidebar": surface(0.017),
    "--color-panel": surface(0.033),
    "--color-raised": surface(0.058),
    "--color-hover": hover,
    "--color-line": surface(0.096),
    "--color-line-strong": surface(0.154),
    "--color-fg": fg,
    "--color-muted": dimmed(0.32),
    "--color-faint": dimmed(0.56),
    "--color-accent": accent,
    "--color-accent-strong": until(mix(accent, "#000000", 0.1), "#000000", reads(4.5, "#ffffff")),
  };
}

const NO_PALETTE: Palette = { background: "#000000", foreground: "#ffffff", cursor: null, selection: null, ansi: [] };
const VARIABLES = [...Object.keys(paletteVariables(NO_PALETTE)), ...Object.keys(interfaceVariables(NO_PALETTE) ?? {})];

/** Sets the theme's colours on the page, the interface's too if asked; what it does not set goes
 * back to the app's own. */
export function applyPalette(
  palette: Palette | null | undefined,
  themeInterface: boolean,
  root: HTMLElement = document.documentElement,
) {
  const variables = palette ? { ...paletteVariables(palette), ...(themeInterface ? interfaceVariables(palette) : null) } : {};
  for (const name of VARIABLES) {
    if (name in variables) root.style.setProperty(name, variables[name]);
    else root.style.removeProperty(name);
  }
}
