// Explicit document declarations in an agent's reply. Ordinary HTML source stays code.
export type Deliverable = { title: string; path: string; exports: string[] };

export function deliverableFormat(path: string): string {
  return /\.pdf$/i.test(path) ? "PDF" : "HTML";
}

function documentPath(value: unknown): value is string {
  return typeof value === "string" && value.length <= 1024
    && value.startsWith(".chauffeur/artifacts/") && /\.(html?|pdf)$/i.test(value)
    && !/[\\%?#\u0000-\u001f]/.test(value)
    && value.split("/").every((part) => part !== ".." && part !== "." && part !== "");
}

/** Malformed or unsupported declarations stay visible as code, including while streaming. */
export function parseDeliverable(source: string): Deliverable | null {
  try {
    const value: unknown = JSON.parse(source);
    if (!value || typeof value !== "object" || !("title" in value) || !("path" in value)) return null;
    if (typeof value.title !== "string" || !value.title.trim() || value.title.length > 200 || !documentPath(value.path)) return null;
    const exports = "exports" in value ? value.exports : [];
    if (!Array.isArray(exports) || exports.length > 4 || !exports.every(documentPath)) return null;
    return { title: value.title.trim(), path: value.path, exports: [...new Set(exports)].filter((path) => path !== value.path) };
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
