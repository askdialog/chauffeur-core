// A small card at the foot of the sidebar when a newer Chauffeur is published. Update downloads
// and installs it through the daemon, agents or not; Restart then reopens the app on it, when the
// user chooses, and the new app replaces the daemon. The cross hides that version until a newer one
// comes. Restarting interrupts the agents' turns: while some work, the card says so at that point
// and lets the user restart anyway or wait.

import type { UpdateStatus } from "@chauffeur/protocol";
import { ArrowUpCircle, X } from "lucide-react";
import { useEffect, useState } from "react";
import { isTauri, relaunchApp } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { DISMISSED_KEY, restartWarning, updateCard, updating } from "@/lib/update";
import { client, toast, useApp } from "@/state/app";
import { Button, IconButton, Spinner } from "./ui";

/** The daemon looks every 10 minutes: the card shows what it found within a minute. */
const POLL_MS = 60_000;
const FOLLOW_MS = 1_000;
const CHECK_AGAIN_MS = 60_000;
/** When the window last asked GitHub, across the card's renders. */
let lastCheck = 0;

function restart() {
  relaunchApp().catch((error) => toast(`Could not reopen Chauffeur: ${errorMessage(error)}`, "error"));
}

export function UpdateCard() {
  const connected = useApp((s) => s.connection.status === "open");
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [dismissed, setDismissed] = useState(() => localStorage.getItem(DISMISSED_KEY));
  /** Why restarting now would hurt: agents work. */
  const [warning, setWarning] = useState<string | null>(null);

  useEffect(() => {
    if (!connected) return;
    const load = () =>
      client
        .call("update.status", {})
        .then(setStatus)
        .catch(() => {});
    // Coming back to the window asks GitHub now, at most once a minute.
    const check = () => {
      if (Date.now() - lastCheck < CHECK_AGAIN_MS) return;
      lastCheck = Date.now();
      void client
        .call("update.check", {})
        .then(setStatus)
        .catch(() => {});
    };
    load();
    const timer = setInterval(load, updating(status) ? FOLLOW_MS : POLL_MS);
    window.addEventListener("focus", check);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", check);
    };
  }, [connected, status?.phase]);

  const card = updateCard(status, dismissed);

  if (!card) return null;

  const install = async () => {
    try {
      setStatus(await client.call("update.install", {}));
    } catch (error) {
      toast(`Could not update Chauffeur: ${errorMessage(error)}`, "error");
    }
  };
  /** Restarts at once unless agents work, which the card then says before asking again. */
  const askRestart = async () => {
    const power = await client.call("power.get", {}).catch(() => null);
    const warning = power && restartWarning(power);
    if (warning) setWarning(warning);
    else restart();
  };
  const dismiss = () => {
    localStorage.setItem(DISMISSED_KEY, card.version);
    setDismissed(card.version);
    setWarning(null);
  };

  return (
    <div
      className="mx-2 mb-2 shrink-0 rounded-lg border border-line bg-raised p-2.5 text-[12px] shadow-sm"
      data-testid="update-card"
      data-kind={card.kind}
    >
      <div className="flex items-start gap-2">
        <ArrowUpCircle size={14} className="mt-px shrink-0 text-accent" />
        <div className="min-w-0 flex-1">
          <div className="font-medium text-fg">
            {card.kind === "installed" ? `Chauffeur ${card.version} is installed` : `Chauffeur ${card.version} is available`}
          </div>
          {card.kind === "installing" && (
            <div className="mt-0.5 flex items-center gap-1.5 text-muted">
              <Spinner size={11} /> {card.step}
            </div>
          )}
          {card.kind === "installed" && !isTauri && (
            <div className="mt-0.5 text-muted">Quit and reopen Chauffeur to run it.</div>
          )}
          {card.kind === "failed" && (
            <div className="mt-0.5 break-words text-danger" data-testid="update-error">
              {card.error}
            </div>
          )}
          {warning && (
            <div className="mt-0.5 text-attention" data-testid="update-warning">
              {warning}
            </div>
          )}
        </div>
        {(card.kind === "available" || card.kind === "failed") && (
          <IconButton title="Hide until the next version" onClick={dismiss} className="-mr-1 -mt-1 h-5 w-5">
            <X size={12} />
          </IconButton>
        )}
      </div>
      {card.kind === "installed" && isTauri && (
        <div className="mt-2 flex justify-end gap-1.5">
          {warning ? (
            <>
              <Button size="sm" variant="ghost" onClick={() => setWarning(null)}>
                Later
              </Button>
              <Button size="sm" variant="primary" onClick={restart} data-testid="update-anyway">
                Restart anyway
              </Button>
            </>
          ) : (
            <Button size="sm" variant="primary" onClick={askRestart} data-testid="update-restart">
              Restart
            </Button>
          )}
        </div>
      )}
      {(card.kind === "available" || card.kind === "failed") && (
        <div className="mt-2 flex justify-end">
          <Button size="sm" variant="primary" onClick={install} data-testid="update-install">
            {card.kind === "failed" ? "Try again" : "Update"}
          </Button>
        </div>
      )}
    </div>
  );
}
