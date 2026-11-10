import type { WorkspaceInfo } from "@chauffeur/protocol";
import { useState } from "react";
import { workspaceName } from "@/lib/format";
import { errorMessage } from "@/lib/rpc";
import { client, toast, upsertWorkspace } from "@/state/app";
import { Button, inputClass, Modal } from "./ui";

/** Names the workspace from a chosen number of recent messages across its chats. */
export function RetitleWorkspaceDialog({ workspace, onClose }: { workspace: WorkspaceInfo; onClose: () => void }) {
  const [count, setCount] = useState("10");
  const [busy, setBusy] = useState(false);
  const lastMessages = Number(count);
  const valid = Number.isSafeInteger(lastMessages) && lastMessages > 0;
  const rename = async () => {
    if (!valid || busy) return;
    setBusy(true);
    try {
      upsertWorkspace(await client.call("workspace.retitle", { workspace: workspace.id, last_messages: lastMessages }));
      onClose();
    } catch (error) {
      toast(`Could not auto-rename ${workspaceName(workspace)}: ${errorMessage(error)}`, "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      open
      onClose={onClose}
      title="Auto-rename workspace"
      width={420}
      footer={<>
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button variant="primary" busy={busy} disabled={!valid || busy} onClick={() => void rename()} data-testid="confirm-auto-rename">Rename</Button>
      </>}
    >
      <form onSubmit={(event) => { event.preventDefault(); void rename(); }} className="space-y-3 text-[12.5px] text-muted">
        <p>Generate a name for {workspaceName(workspace)} from its latest user and assistant messages across all chats.</p>
        <label className="block space-y-1.5">
          <span>Number of recent messages</span>
          <input className={inputClass} autoFocus type="number" min={1} step={1} value={count} onChange={(event) => setCount(event.target.value)} disabled={busy} data-testid="auto-rename-count" />
        </label>
      </form>
    </Modal>
  );
}
