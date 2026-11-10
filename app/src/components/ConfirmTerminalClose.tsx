import { useState } from "react";
import type { TerminalClose } from "@/lib/terminalClose";
import { Button, Modal } from "./ui";

/** Asks before stopping a command to close its terminal, as a chat asks before stopping its agent. */
export function ConfirmTerminalClose({
  confirm,
  onCancel,
  onConfirm,
}: {
  confirm: TerminalClose;
  onCancel: () => void;
  onConfirm: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [first] = confirm.running;
  const lone = confirm.ids.length === 1;
  return (
    <Modal
      open
      onClose={onCancel}
      width={440}
      title={lone ? `Close '${first}'?` : `Close ${confirm.ids.length} terminals?`}
      footer={
        <>
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            variant="danger"
            autoFocus
            busy={busy}
            data-testid="confirm-terminal-close"
            onClick={() => {
              setBusy(true);
              void onConfirm();
            }}
          >
            Stop and close
          </Button>
        </>
      }
    >
      <div className="space-y-2 text-[12.5px] text-muted">
        {lone ? (
          <p className="text-attention">A command runs in it. Stop it and close?</p>
        ) : (
          <>
            <p className="text-attention">
              A command runs in {confirm.running.length === 1 ? "one of them" : `${confirm.running.length} of them`}. Stop{" "}
              {confirm.running.length === 1 ? "it" : "them"} and close?
            </p>
            <ul className="list-disc pl-5">
              {confirm.running.map((label, index) => (
                <li key={index}>{label}</li>
              ))}
            </ul>
          </>
        )}
        <p className="text-faint">A closed terminal cannot be reopened.</p>
      </div>
    </Modal>
  );
}
