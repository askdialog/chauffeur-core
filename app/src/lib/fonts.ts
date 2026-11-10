// The fonts the user picked (Settings › Appearance, `[appearance]` in settings.toml), applied to
// the page: CSS variables for the interface and the code, options for the terminal.

import type { Appearance } from "@chauffeur/protocol";

// Apple's fonts, then the generic families: where Apple's are missing (Linux), the desktop's
// fontconfig aliases decide, so no other named font may come between them.
export const DEFAULT_SANS = '-apple-system, BlinkMacSystemFont, "SF Pro Text", system-ui, sans-serif';
export const DEFAULT_MONO = '"SF Mono", ui-monospace, Menlo, Monaco, monospace';
export const DEFAULT_CODE_SIZE = 12;
export const DEFAULT_TERMINAL_SIZE = 12;

/** Fonts people often have, offered as suggestions (any installed font can be typed). */
export const SANS_SUGGESTIONS = ["Inter", "SF Pro Text", "Helvetica Neue", "IBM Plex Sans", "Geist"];
export const MONO_SUGGESTIONS = ["JetBrains Mono", "Fira Code", "SF Mono", "Menlo", "Berkeley Mono", "Geist Mono", "Cascadia Code", "IBM Plex Mono"];

/**
 * The user's font first, then the app's own as a fallback (a font that is not installed falls
 * through). A name with spaces is quoted unless the user quoted it or listed several.
 */
export function fontStack(family: string, fallback: string): string {
  const name = family.trim();
  if (!name) return fallback;
  const listed = name.includes(",") || /^["'].*["']$/.test(name) ? name : /\s/.test(name) ? `"${name}"` : name;
  return `${listed}, ${fallback}`;
}

/** The terminal's options for these settings. */
export function terminalFont(appearance: Appearance | null): { fontFamily: string; fontSize: number } {
  return {
    fontFamily: fontStack(appearance?.terminal_font ?? "", DEFAULT_MONO),
    fontSize: appearance?.terminal_size || DEFAULT_TERMINAL_SIZE,
  };
}

/** Sets the interface's and the code's fonts on the page. */
export function applyFonts(appearance: Appearance | null, root: HTMLElement = document.documentElement) {
  root.style.setProperty("--font-sans", fontStack(appearance?.interface_font ?? "", DEFAULT_SANS));
  root.style.setProperty("--font-mono", fontStack(appearance?.code_font ?? "", DEFAULT_MONO));
  root.style.setProperty("--code-size", `${appearance?.code_size || DEFAULT_CODE_SIZE}px`);
}
