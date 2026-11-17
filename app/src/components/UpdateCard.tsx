// A small card at the foot of the sidebar when a newer Chauffeur is published. One click updates
// and restarts: Update now, or When idle, downloads and installs it through the
// daemon (agents or not), then reopens the app on it, at once or as soon as no agent works, and the
// new app replaces the daemon. The cross hides that version until a newer one comes. Restarting
// interrupts the agents' turns: when some work at the click, Update now says so first and
// offers to restart anyway or when idle. A version installed without a planned restart (from the
// CLI, or a plan cancelled) keeps Restart and Restart when idle. In a browser the app cannot reopen
// itself: Update only installs, and the user reopens Chauffeur.

import type { PowerState, UpdateStatus } from "@chauffeur/protocol";
import { ArrowUpCircle, X } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { isTauri, relaunchApp } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import {
  DISMISSED_KEY,
  idleForRestart,
  plannedRestart,
  type RestartPlan,
  restartWarning,
  UPDATE_REVEAL_EVENT,
  updateCard,
  updating,
  waitingForIdle,
} from "@/lib/update";
import { client, toast, useApp } from "@/state/app";
import { Button, IconButton, Spinner } from "./ui";

/** The daemon looks every 10 minutes: the card shows what it found within a minute. */
const POLL_MS = 60_000;
const FOLLOW_MS = 1_000;
const CHECK_AGAIN_MS = 60_000;
/** How often the card reads the agents while it waits to restart until none works. */
const IDLE_POLL_MS = 3_000;
/** When the window last asked GitHub, across the card's renders. */
let lastCheck = 0;
/** The restart the user planned, kept should the card mount again. */
let planned: RestartPlan | null = null;
/** Set once the app is being reopened, so that it is asked once. */
let relaunching = false;

function restart() {
  if (relaunching) return;
  relaunching = true;
  relaunchApp().catch((error) => {
    relaunching = false;
    toast(`Could not reopen Chauffeur: ${errorMessage(error)}`, "error");
  });
}

export function UpdateCard() {
  const connected = useApp((s) => s.connection.status === "open");
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [dismissed, setDismissed] = useState(() => localStorage.getItem(DISMISSED_KEY));
  /** Why restarting now would hurt: agents work. */
  const [warning, setWarning] = useState<string | null>(null);
  const [plan, setPlan] = useState<RestartPlan | null>(planned);
  const [power, setPower] = useState<PowerState | null>(null);

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

  // Check for Updates… in the app menu shows the card again, a hidden version included.
  useEffect(() => {
    const reveal = () => {
      setDismissed(null);
      void client
        .call("update.status", {})
        .then(setStatus)
        .catch(() => {});
    };
    window.addEventListener(UPDATE_REVEAL_EVENT, reveal);
    return () => window.removeEventListener(UPDATE_REVEAL_EVENT, reveal);
  }, []);

  const phase = status?.phase;

  // Once installed, the planned restart happens: at once, or when a read of the agents finds none
  // working. A failed installation drops the plan: the user chooses again under the error.
  useEffect(() => {
    planned = plan;
    if (!plan || !connected) return;
    if (phase === "failed") {
      setPlan(null);
      return;
    }
    if (phase !== "installed") return;
    if (plan === "now") {
      restart();
      return;
    }
    let done = false;
    const look = () =>
      client
        .call("power.get", {})
        .then((state) => {
          if (done) return;
          setPower(state);
          if (idleForRestart(state)) {
            done = true;
            restart();
          }
        })
        .catch(() => {});
    look();
    const timer = setInterval(look, IDLE_POLL_MS);
    return () => {
      done = true;
      clearInterval(timer);
    };
  }, [plan, connected, phase]);

  const card = updateCard(status, dismissed);

  if (!card) return null;

  const install = async () => {
    try {
      setStatus(await client.call("update.install", {}));
    } catch (error) {
      setPlan(null);
      toast(`Could not update Chauffeur: ${errorMessage(error)}`, "error");
    }
  };
  /** Plans the restart, and installs the update first when it is not yet. */
  const go = (next: RestartPlan) => {
    setWarning(null);
    setPlan(next);
    if (card.kind === "available" || card.kind === "failed") void install();
  };
  /** Updates and restarts at once unless agents work, which the card then says before asking again. */
  const askNow = async () => {
    const power = await client.call("power.get", {}).catch(() => null);
    const warning = power && restartWarning(power);
    if (warning) setWarning(warning);
    else go("now");
  };
  const cancel = () => {
    setPlan(null);
    setPower(null);
  };
  const dismiss = () => {
    localStorage.setItem(DISMISSED_KEY, card.version);
    setDismissed(card.version);
    setWarning(null);
  };

  const offered = card.kind === "available" || card.kind === "failed";
  const waiting = card.kind === "installed" && plan === "idle";

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
            <div className="mt-0.5 flex items-start gap-1.5 text-muted" data-testid="update-step">
              <Spinner size={11} className="mt-[3px] shrink-0" />
              <span>
                {card.step}
                {plan && ` ${plannedRestart(plan)}`}
              </span>
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
          {waiting && (
            <div className="mt-0.5 flex items-start gap-1.5 text-muted" data-testid="update-waiting">
              <Spinner size={11} className="mt-[3px] shrink-0" />
              <span>{waitingForIdle(power)}</span>
            </div>
          )}
          {warning && (
            <div className="mt-0.5 text-attention" data-testid="update-warning">
              {warning}
            </div>
          )}
        </div>
        {offered && (
          <IconButton title="Hide until the next version" onClick={dismiss} className="-mr-1 -mt-1 h-5 w-5">
            <X size={12} />
          </IconButton>
        )}
      </div>
      {isTauri ? (
        <Actions
          card={card.kind}
          plan={plan}
          warning={warning !== null}
          onNow={askNow}
          onAnyway={() => go("now")}
          onIdle={() => go("idle")}
          onCancel={cancel}
        />
      ) : (
        offered && (
          <div className="mt-2 flex justify-end">
            <Button size="sm" variant="primary" onClick={install} data-testid="update-install">
              {card.kind === "failed" ? "Try again" : "Update"}
            </Button>
          </div>
        )
      )}
    </div>
  );
}

/** The card's buttons in the desktop app, where it can reopen itself. */
function Actions(props: {
  card: "available" | "installing" | "installed" | "failed";
  plan: RestartPlan | null;
  warning: boolean;
  onNow: () => void;
  onAnyway: () => void;
  onIdle: () => void;
  onCancel: () => void;
}) {
  const { card, plan, warning } = props;
  const row = (children: ReactNode) => <div className="mt-2 flex flex-wrap justify-end gap-1.5">{children}</div>;
  if (warning) {
    return row(
      <>
        <Button size="sm" variant="ghost" onClick={props.onAnyway} data-testid="update-anyway">
          Restart anyway
        </Button>
        <Button size="sm" variant="primary" onClick={props.onIdle} data-testid="update-when-idle">
          When idle
        </Button>
      </>,
    );
  }
  if (plan && (card === "installing" || card === "installed")) {
    return row(
      <>
        <Button size="sm" variant="ghost" onClick={props.onCancel} data-testid="update-cancel">
          Cancel
        </Button>
        {card === "installed" && (
          <Button size="sm" variant="primary" onClick={props.onAnyway} data-testid="update-now">
            Restart now
          </Button>
        )}
      </>,
    );
  }
  if (card === "installing") return null;
  // Short labels while the update is offered, so that both buttons hold on one line in the sidebar.
  const installed = card === "installed";
  return row(
    <>
      <Button size="sm" variant="ghost" onClick={props.onIdle} data-testid="update-when-idle">
        {installed ? "Restart when idle" : "When idle"}
      </Button>
      <Button size="sm" variant="primary" onClick={props.onNow} data-testid="update-restart">
        {installed ? "Restart" : "Update now"}
      </Button>
    </>,
  );
}
