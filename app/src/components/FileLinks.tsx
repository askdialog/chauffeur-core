// Files named in a chat open in the Files tab: the chat provides a linker built on the
// workspace's file index (WorkspaceFileLinker.ts), and the markdown and tool cards use it. A
// right-click on one also shows it in Finder. Without a linker (the share pages), names stay text.

import { FileText, FolderOpen } from "lucide-react";
import { createContext, useCallback, useContext, useState } from "react";
import type { FileRef } from "@/lib/files";
import { isMac } from "@/lib/keys";
import { ContextMenu, type MenuItem } from "./ContextMenu";

export type FileLinker = {
  resolve: (text: string) => FileRef | null;
  /** Resolves against the files listed again: a file the agent made after the list was taken. */
  resolveFresh: (text: string) => Promise<FileRef | null>;
  /** A path inside the folder of the workspaces (a folder, another workspace's file) that Finder can show. */
  resolveFolder: (text: string) => string | null;
  open: (ref: FileRef) => void;
  /** Its files are on this machine, so Finder can show them: not a moved workspace's. */
  canReveal: boolean;
  reveal: (ref: FileRef) => void;
  /** Shows a path in Finder: `resolveFolder`'s, or one under the workspace folder. */
  revealPath: (path: string) => void;
};

export const FileLinkContext = createContext<FileLinker | null>(null);

export function useFileLinker(): FileLinker | null {
  return useContext(FileLinkContext);
}

export const REVEAL_LABEL = isMac ? "Show in Finder" : "Show in folder";

/**
 * A right-click menu for a file of the workspace: open it in the Files tab, or show it in
 * Finder. `onContextMenu(ref)` goes on the element naming the file; `menu` is rendered beside it.
 */
export function useFileMenu(linker: FileLinker | null) {
  const [menu, setMenu] = useState<{ x: number; y: number; ref: FileRef } | null>(null);
  const onContextMenu = useCallback(
    (ref: FileRef | null) => (event: React.MouseEvent) => {
      if (!ref || !linker) return;
      event.preventDefault();
      event.stopPropagation();
      setMenu({ x: event.clientX, y: event.clientY, ref });
    },
    [linker],
  );
  const items = (ref: FileRef): MenuItem[] =>
    linker
      ? [
          { label: "Open", icon: <FileText size={13} />, shortcut: "o", onSelect: () => linker.open(ref) },
          ...(linker.canReveal
            ? [{ label: REVEAL_LABEL, icon: <FolderOpen size={13} />, shortcut: "f", onSelect: () => linker.reveal(ref) }]
            : []),
        ]
      : [];
  // React bubbles the menu's clicks to the element it is rendered in (a tool card's toggle).
  const element = menu && linker && (
    <span className="contents" onClick={(event) => event.stopPropagation()}>
      <ContextMenu x={menu.x} y={menu.y} items={items(menu.ref)} onClose={() => setMenu(null)} />
    </span>
  );
  return { onContextMenu, menu: element };
}
