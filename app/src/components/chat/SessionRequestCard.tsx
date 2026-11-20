import { KeyRound } from "lucide-react";
import { useCallback, useContext, useEffect, useRef, useState } from "react";
import type { SessionRequestItem } from "@/lib/chat";
import { isMac } from "@/lib/keys";
import { isTauri, openExternal, windowFocused } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { client, toast, useApp } from "@/state/app";
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
  // What the login printed: the code its page asks for, when it asks one.
  const [printed, setPrinted] = useState("");
  const open = item.state === "open" && !readOnly && workspace !== null;

  const local = useApp(state => {
    const target = workspace && state.workspaces[workspace];
    return !!target && !target.moved_to;
  });
  const attempted = useRef<string | null>(null);
  const login = useCallback(async (automatic = false) => {
    if (workspace === null) return;
    setBusy("login");
    try {
      const result = await client.call("session.login", {
        workspace,
        name: item.session,
        remote: false,
        automatic_request: automatic ? item.requestId : undefined,
      });
      if (automatic && !result.url && !result.output) return;
      setStarted(true);
      setPrinted(result.output);
      if (result.url) await openExternal(result.url);
    } catch (error) {
      toast(`Could not start the login: ${errorMessage(error)}`, "error");
    } finally {
      setBusy(null);
    }
  }, [workspace, item.session, item.requestId]);

  useEffect(() => {
    if (!open || !local || !item.browserConsent || !isMac || !isTauri) return;
    const start = () => {
      if (!windowFocused() || attempted.current === item.requestId) return;
      attempted.current = item.requestId;
      void login(true);
    };
    start();
    window.addEventListener("focus", start);
    return () => window.removeEventListener("focus", start);
  }, [open, local, item.browserConsent, item.requestId, login]);

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
      {open && (
        <div className="text-[12px] text-muted">
          {started ? "Finish the login in your browser. Work resumes automatically." : "A command waits on this session, which has expired."}
        </div>
      )}
      {started && printed && item.state === "open" && (
        <pre className="mt-1.5 max-h-40 overflow-auto whitespace-pre-wrap rounded bg-panel px-2 py-1.5 font-mono text-[11.5px] text-muted selectable" data-testid="session-login-output">
          {printed}
        </pre>
      )}
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
