import { KeyRound } from "lucide-react";
import { useContext, useState } from "react";
import type { SessionRequestItem } from "@/lib/chat";
import { openExternal } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { client, toast } from "@/state/app";
import { Button } from "../ui";
import { RequestFrame, type Respond, WorkspaceIdContext } from "./ChatItems";

const OUTCOMES: Record<string, string> = {
  answered: "logged in",
  denied: "declined",
  cancelled: "not answered",
};

/**
 * A command waits on a declared session that does not hold (docs/adr/declared-sessions.md). Log in
 * starts its login and opens the link it prints; the daemon closes the card once the session
 * holds. Not now declines, and the command stops.
 */
export function SessionRequestCard({
  item,
  readOnly,
  respond,
}: {
  item: SessionRequestItem;
  readOnly: boolean;
  respond: Respond;
}) {
  const workspace = useContext(WorkspaceIdContext);
  const [busy, setBusy] = useState<"login" | "decline" | null>(null);
  const [started, setStarted] = useState(false);
  const open = item.state === "open" && !readOnly && workspace !== null;

  const login = async () => {
    if (workspace === null) return;
    setBusy("login");
    try {
      const result = await client.call("session.login", { workspace, name: item.session });
      setStarted(true);
      if (result.url) await openExternal(result.url);
    } catch (error) {
      toast(`Could not start the login: ${errorMessage(error)}`, "error");
    } finally {
      setBusy(null);
    }
  };
  const decline = async () => {
    setBusy("decline");
    try {
      await respond(item.requestId, { type: "deny", message: null });
    } finally {
      setBusy(null);
    }
  };

  return (
    <RequestFrame
      icon={<KeyRound size={14} />}
      title={`Log in to ${item.label}?`}
      state={item.state}
      outcome={item.outcome ? (OUTCOMES[item.outcome] ?? item.outcome) : null}
      outcomeTone={item.outcome === "answered" ? "success" : "neutral"}
    >
      <div className="text-[12px] text-muted">
        {started ? "Finish the login in your browser: this card closes once it holds." : "A command waits on this session, which has expired."}
      </div>
      {open && (
        <div className="mt-2 flex gap-1.5">
          <Button variant="primary" size="sm" busy={busy === "login"} onClick={() => void login()}>
            Log in
          </Button>
          <Button size="sm" busy={busy === "decline"} onClick={() => void decline()}>
            Not now
          </Button>
        </div>
      )}
    </RequestFrame>
  );
}
