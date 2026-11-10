// "Continue as workspace", asked from the header, the sidebar's menu or the composer's mode.

import type { RepoLeftovers, WorkspaceInfo } from "@chauffeur/protocol";
import { useEffect, useState } from "react";
import { workspaceName } from "@/lib/format";
import { leftoverLines, promoteLeftovers } from "@/lib/promote";
import { errorMessage } from "@/lib/rpc";
import { client, toast, upsertWorkspace } from "@/state/app";
import { Button, Modal } from "./ui";

/**
 * Turns an exploration into a workspace, its conversation kept. When its checkouts hold files
 * their commit does not (what its commands wrote), the daemon refuses and names them: the user
 * then discards them, for good, or keeps the exploration.
 */
export function PromoteDialog({ open, onClose, workspace }: { open: boolean; onClose: () => void; workspace: WorkspaceInfo }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [leftovers, setLeftovers] = useState<RepoLeftovers[] | null>(null);
  useEffect(() => {
    if (open) {
      setError(null);
      setLeftovers(null);
    }
  }, [open]);
  const promote = async (discard: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const promoted = await client.call("workspace.promote", { workspace: workspace.id, discard });
      upsertWorkspace(promoted);
      toast(`${workspaceName(promoted)} is now a workspace on ${promoted.branch}`);
      onClose();
    } catch (err) {
      const left = promoteLeftovers(err);
      if (left) setLeftovers(left);
      else setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Continue as a workspace"
      width={480}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {leftovers ? "Keep the exploration" : "Cancel"}
          </Button>
          {leftovers ? (
            <Button variant="danger" busy={busy} onClick={() => void promote(true)} data-testid="confirm-discard">
              Discard and continue
            </Button>
          ) : (
            <Button variant="primary" busy={busy} onClick={() => void promote(false)} data-testid="confirm-promote">
              Continue
            </Button>
          )}
        </>
      }
    >
      <div className="space-y-2.5 text-[12.5px] text-muted">
        {leftovers ? (
          <>
            <p>
              The exploration's commands (or you, in its terminal) left files in its checkouts that their commit does not
              have. Continuing deletes them for good, so that the workspace starts from the commit:
            </p>
            <div className="space-y-2" data-testid="promote-leftovers">
              {leftovers.map((r) => {
                const lines = leftoverLines(r.files);
                return (
                  <div key={r.repo}>
                    <div className="font-medium text-fg">{r.repo}</div>
                    <ul className="selectable pl-3 font-mono text-[11.5px]">
                      {lines.shown.map((f) => (
                        <li key={f}>{f}</li>
                      ))}
                      {lines.more > 0 && <li className="font-sans text-faint">and {lines.more} more</li>}
                    </ul>
                  </div>
                );
              })}
            </div>
          </>
        ) : (
          <>
            <p>The exploration becomes a workspace, its chats and their conversation kept:</p>
            <ul className="list-disc space-y-1 pl-5">
              <li>each repo gets a branch named after the workspace, from the commit it reads now;</li>
              <li>the files to copy (.env…), ports and setup scripts, as in a new workspace;</li>
              <li>from their next message, the agents can edit files, run them and commit.</li>
            </ul>
          </>
        )}
        {error && <p className="text-danger selectable">{error}</p>}
      </div>
    </Modal>
  );
}
