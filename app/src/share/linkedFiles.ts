// A shared markdown file's linked files (docs/adr/share-linked-files.md): the daemon pushes them
// beside the file with `links.json`, which says, for each shared file, what each name it writes
// opens. The reviewer's page reads a name as the daemon does (`crates/chauffeurd/src/linked_files.rs`,
// `normalize`) and looks it up there.

import type { DesignComment } from "@chauffeur/protocol";

export const MANIFEST = "links.json";

/** Shared file → (name as written, normalized → shared file it opens). */
export type LinkManifest = Record<string, Record<string, string>>;

/** A name as the app's file linker reads it: no `file://`, quotes, punctuation, line suffix nor leading `./`. */
export function normalizeRef(text: string): string | null {
  let ref = text.trim().replace(/^file:\/\//, "").replace(/^["'`(<]+|[)"'`>.,;:]+$/g, "");
  const anchor = /#L\d+(?:-L?\d+)?$/.exec(ref);
  if (anchor) ref = ref.slice(0, anchor.index);
  else {
    const suffix = /:\d+(?::\d+|-\d+)?$/.exec(ref);
    if (suffix && suffix.index > 0) ref = ref.slice(0, suffix.index);
  }
  if (!ref || ref.includes(" ") || /^[a-z]+:\/\//i.test(ref)) return null;
  ref = ref.replace(/^\.\//, "");
  return ref || null;
}

/** The shared file a name written in `from` opens, if the share carries it. */
export function linkedTarget(manifest: LinkManifest, from: string, text: string): string | null {
  const ref = normalizeRef(safeDecode(text));
  return (ref && manifest[from]?.[ref]) || null;
}

function safeDecode(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}

/** Every file the share carries: the shared one, then each one a manifest names. */
export function carriedFiles(document: string, manifest: LinkManifest): string[] {
  const all = new Set<string>([document]);
  for (const names of Object.values(manifest)) for (const target of Object.values(names)) all.add(target);
  return [...all];
}

/** The comments on `file`: its threads, and their replies (a reply takes its thread's file). */
export function commentsOn(comments: DesignComment[], file: string): DesignComment[] {
  const threads = new Set(comments.filter((c) => !c.reply_to && c.screen === file).map((c) => c.id));
  return comments.filter((c) => (c.reply_to ? threads.has(c.reply_to) : threads.has(c.id)));
}

/** A shared file's name for a reader: its base name (`_chauffeur/s/MODELISATION.md` → `MODELISATION.md`). */
export function displayName(file: string): string {
  return file.slice(file.lastIndexOf("/") + 1);
}
