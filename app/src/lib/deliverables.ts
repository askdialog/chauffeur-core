// Explicit document declarations in an agent's reply. Ordinary HTML source stays code.
/** A document (HTML or PDF), or a slide deck named by its deck.json (docs/adr/slide-decks.md). */
export type Deliverable = { title: string; path: string; exports: string[]; kind: "document" | "deck" };

export function deliverableFormat(path: string): string {
  if (/(^|\/)deck\.json$/i.test(path)) return "Deck";
  return /\.pdf$/i.test(path) ? "PDF" : "HTML";
}

function artifactPath(value: unknown, extension: RegExp): value is string {
  return typeof value === "string" && value.length <= 1024
    && value.startsWith(".chauffeur/artifacts/") && extension.test(value)
    && !/[\\%?#\u0000-\u001f]/.test(value)
    && value.split("/").every((part) => part !== ".." && part !== "." && part !== "");
}

function documentPath(value: unknown): value is string {
  return artifactPath(value, /\.(html?|pdf)$/i);
}

/** A deck lives in its own folder: `.chauffeur/artifacts/<deck>/deck.json`. */
function deckPath(value: unknown): value is string {
  return artifactPath(value, /^\.chauffeur\/artifacts\/[^/]+\/(.+\/)?deck\.json$/);
}

/** Malformed or unsupported declarations stay visible as code, including while streaming. */
export function parseDeliverable(source: string): Deliverable | null {
  try {
    const value: unknown = JSON.parse(source);
    if (!value || typeof value !== "object" || !("title" in value) || !("path" in value)) return null;
    const kind = "kind" in value && value.kind === "deck" ? "deck" : "document";
    if ("kind" in value && value.kind !== "deck" && value.kind !== "document") return null;
    if (typeof value.title !== "string" || !value.title.trim() || value.title.length > 200) return null;
    const path = value.path;
    if (typeof path !== "string" || !(kind === "deck" ? deckPath(path) : documentPath(path))) return null;
    const exports = "exports" in value ? value.exports : [];
    if (!Array.isArray(exports) || exports.length > 4 || !exports.every(documentPath)) return null;
    return { title: value.title.trim(), path, exports: kind === "deck" ? [] : [...new Set(exports)].filter((other) => other !== path), kind };
  } catch {
    return null;
  }
}

/** A browser receives only the one-document ticket, never the app's connection token. */
export function deliverableDownloadUrl(base: string, ticket: string): string {
  const url = new URL(base);
  url.search = "";
  url.hash = "";
  url.searchParams.set("ticket", ticket);
  return url.toString();
}

/** Where version `n` of a deliverable's file lies: in its folder's `.versions/<n>/`; null for a file without a folder of its own. */
export function versionPath(path: string, n: number): string | null {
  const match = /^(\.chauffeur\/artifacts\/[^/]+)\/(.+)$/.exec(path);
  return match ? `${match[1]}/.versions/${n}/${match[2]}` : null;
}
