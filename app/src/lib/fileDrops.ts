// Files dropped from the desktop on Linux. WebKitGTK gives the page their types but neither their
// names nor their bytes, so the shell takes the drop (app/src-tauri/src/drops.rs) and says where it
// happened; the drop target under that point reads the files from the shell by their index.

/** Put on an element files are dropped on, for a native drop to find it. */
export const FILE_DROP_ATTRIBUTE = "data-file-drop";

/** A file of a native drop, as the shell describes it: never its path. */
export type DroppedFile = { name: string; mediaType: string; size: number };
/** A native drop: its number, where it happened in the web view's pixels (zoom included), its files. */
export type FileDrop = { drop: number; x: number; y: number; files: DroppedFile[] };

type Point = Pick<Document, "elementFromPoint">;

/**
 * Whether a drag brings files a drop target takes. On Linux (`native`) WebKitGTK lists a file's
 * drag as a URI list without "Files", and the shell's drop delivers the files.
 */
export function bringsFiles(types: readonly string[], native: boolean): boolean {
  return types.includes("Files") || (native && types.includes("text/uri-list"));
}

const targets = new WeakMap<Element, (drop: FileDrop) => void>();

/** Makes `element` take native drops; returns the function that undoes it. */
export function registerDropTarget(element: Element, take: (drop: FileDrop) => void): () => void {
  targets.set(element, take);
  return () => {
    if (targets.get(element) === take) targets.delete(element);
  };
}

/** Hands a native drop to the drop target under it, at the app's `zoom`; false when there is none. */
export function deliverFileDrop(root: Point, drop: FileDrop, zoom: number): boolean {
  const target = root.elementFromPoint(drop.x / zoom, drop.y / zoom)?.closest(`[${FILE_DROP_ATTRIBUTE}]`);
  const take = target ? targets.get(target) : undefined;
  take?.(drop);
  return take !== undefined;
}
