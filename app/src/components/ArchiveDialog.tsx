// Archiving a workspace, from its header, the sidebar or its chat's window in the Office: at once, without asking (a workspace can
// be restored for 30 days); it leaves the sidebar and goes on in the background, a toast tells how
// it ended, and a workspace that could not be archived comes back with its error. This dialog only
// opens when the daemon refuses: it says why and offers to archive anyway. One that runs on another
// machine comes back first, its agents interrupted, and stays in the sidebar until it is back.

import type { WorkspaceInfo } from "@chauffeur/protocol";
import { XCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { workspaceName } from "@/lib/format";
import { errorMessage } from "@/lib/rpc";
import { transferCopy } from "@/lib/transfer";
import { client, toast, upsertWorkspace, useApp } from "@/state/app";
import { Button, Modal } from "./ui";

/** Starts archiving `workspace` in the background; the daemon's refusal, if any. */
export async function archiveWorkspace(workspace: WorkspaceInfo, force = false): Promise<string | null> {
  try {
    const archiving = await client.call("workspace.archive", { workspace: workspace.id, force, background: true });
    upsertWorkspace(archiving);
    if (archiving.moved_to) {
      toast(transferCopy(navigator.language.startsWith("fr")).archivingStarted(workspaceName(archiving), archiving.moved_to));
    }
    return null;
  } catch (err) {
    return errorMessage(err);
  }
}

export function ArchiveDialog({
  open,
  onClose,
  workspace,
  refusal = null,
}: {
  open: boolean;
  onClose: () => void;
  workspace: WorkspaceInfo;
  /** Why the daemon refused to archive it. */
  refusal?: string | null;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(refusal);

  useEffect(() => {
    if (open) setError(refusal);
  }, [open, refusal]);

  const children = Object.values(useApp.getState().workspaces).filter(
    (w) => w.parent_id === workspace.id && w.state !== "archived",
  );
  const failed = workspace.state === "cleanup_failed";
  const moved = workspace.moved_to;

  /** Archiving goes on in the background: the workspace leaves the sidebar, a toast tells how it ended. */
  const run = async (force: boolean) => {
    setBusy(true);
    setError(null);
    const refused = await archiveWorkspace(workspace, force);
    setBusy(false);
    if (refused) setError(refused);
    else onClose();
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`Archive ${workspaceName(workspace)}?`}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          {(error || failed) && !moved && (
            <Button variant="danger" onClick={() => void run(true)} busy={busy} data-testid="archive-anyway">
              Archive anyway
            </Button>
          )}
          <Button variant="primary" onClick={() => void run(false)} busy={busy} data-testid="confirm-archive">
            {failed ? "Archive again" : "Archive"}
          </Button>
        </>
      }
    >
      <div className="space-y-2 text-[12.5px] text-muted">
          {workspace.explore ? (
            <p>
              Archiving stops the exploration's agents and processes and removes its checkouts; your clones are untouched.
              Chats stay readable. To explore again, start a new exploration.
            </p>
          ) : (
            <p>
              Archiving stops every process of the workspace, runs each repo's archive script, saves uncommitted work in a
              snapshot kept 30 days and removes the worktrees. Chats become read-only. You can restore it later.
            </p>
          )}
          {moved && (
            <p className="text-attention">
              It runs on {moved}: it comes back here first, its agents interrupted, then is archived. {moved} stops if no
              other workspace runs there.
            </p>
          )}
          {children.length > 0 && (
            <p className="text-attention">
              Its {children.length} child workspace{children.length > 1 ? "s are" : " is"} archived too:{" "}
              {children.map(workspaceName).join(", ")}.
            </p>
          )}
          {workspace.activity === "working" && (
            <p className="text-attention">An agent is working in this workspace.</p>
          )}
          <p className="text-faint">It goes on in the background; the workspace's log shows each step.</p>
          {error && (
            <div className="flex items-start gap-2 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-[12.5px] text-danger selectable">
              <XCircle size={14} className="mt-0.5 shrink-0" />
              <span>{error}</span>
            </div>
          )}
      </div>
    </Modal>
  );
}
