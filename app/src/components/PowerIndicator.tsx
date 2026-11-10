// A discreet icon at the foot of the sidebar while Chauffeur keeps the machine awake
// (`keep_awake`): a cup while it holds off idle sleep, a sun while sleep is off even with the lid
// closed, a low battery when that was given back, a warning when something did not work. Its
// tooltip says why; a click opens Settings › Sleep. Read every ten seconds from `power.get`.

import type { PowerState } from "@chauffeur/protocol";
import { AlertTriangle, BatteryLow, Coffee, Sun } from "lucide-react";
import { useEffect, useState } from "react";
import { isMac } from "@/lib/keys";
import { type PowerHint, powerHint } from "@/lib/power";
import { client, openSettings, useApp } from "@/state/app";
import { cx } from "./ui";

const POLL_MS = 10_000;

export function PowerIndicator() {
  const connected = useApp((s) => s.connection.status === "open");
  const [state, setState] = useState<PowerState | null>(null);
  useEffect(() => {
    if (!connected) return;
    const load = () =>
      client
        .call("power.get", {})
        .then(setState)
        .catch(() => {});
    // Not read while the window is hidden: read again as it shows.
    const loadShown = () => {
      if (!document.hidden) load();
    };
    load();
    const timer = setInterval(loadShown, POLL_MS);
    window.addEventListener("focus", load);
    document.addEventListener("visibilitychange", loadShown);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", load);
      document.removeEventListener("visibilitychange", loadShown);
    };
  }, [connected]);
  return <PowerIndicatorView hint={powerHint(state, isMac)} />;
}

export function PowerIndicatorView({ hint }: { hint: PowerHint | null }) {
  if (!hint) return null;
  const icon = {
    awake: <Coffee size={13} />,
    lid_closed: <Sun size={13} />,
    low_battery: <BatteryLow size={13} />,
    warning: <AlertTriangle size={13} />,
  }[hint.kind];
  return (
    <button
      onClick={() => openSettings(true, "sleep")}
      title={hint.text}
      aria-label={hint.text}
      className={cx(
        "inline-flex h-6 w-6 items-center justify-center rounded-md hover:bg-hover",
        hint.kind === "awake" && "text-faint hover:text-muted",
        hint.kind === "lid_closed" && "text-accent",
        (hint.kind === "low_battery" || hint.kind === "warning") && "text-attention",
      )}
      data-testid="power-indicator"
      data-kind={hint.kind}
    >
      {icon}
    </button>
  );
}
