// The file linker of a workspace's chat and Files tab, built on its file index from the daemon.

import { useEffect, useMemo } from "react";
import { type FileRef, resolveFileRef, resolveWorkspacesPath } from "@/lib/files";
import { loadFileIndex, openFile, revealFile, useApp } from "@/state/app";
import type { FileLinker } from "./FileLinks";

/** A linker for a workspace's chat; null until its files are listed. */
export function useWorkspaceFileLinker(workspaceId: string, workspacePath: string | null): FileLinker | null {
  const index = useApp((s) => s.fileIndex[workspaceId]?.index ?? null);
  // Moved to another machine: its files are there, Finder shows none of them.
  const here = useApp((s) => !s.workspaces[workspaceId]?.moved_to);
  useEffect(() => {
    void loadFileIndex(workspaceId);
  }, [workspaceId]);
  return useMemo(
    () =>
      index && {
        resolve: (text: string) => resolveFileRef(text, index, workspacePath),
        resolveFresh: async (text: string) => {
          await loadFileIndex(workspaceId, 0);
          const fresh = useApp.getState().fileIndex[workspaceId]?.index;
          return fresh ? resolveFileRef(text, fresh, workspacePath) : null;
        },
        resolveFolder: (text: string) => (here ? resolveWorkspacesPath(text, workspacePath) : null),
        open: (ref: FileRef) => openFile(workspaceId, ref.path, ref.line),
        canReveal: here,
        reveal: (ref: FileRef) => {
          if (here) void revealFile(workspaceId, ref.path);
        },
        revealPath: (path: string) => {
          if (here) void revealFile(workspaceId, path);
        },
      },
    [index, workspaceId, workspacePath, here],
  );
}
