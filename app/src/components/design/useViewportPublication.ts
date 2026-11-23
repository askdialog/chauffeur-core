import type { DesignParticipant, DesignViewport } from "@chauffeur/protocol";
import { useEffect, useRef } from "react";

const PUBLISH_INTERVAL_MS = Math.ceil(1000 / 15);
const OWNER_HEARTBEAT_MS = 10_000;

export function followPeers(participants: DesignParticipant[], self: string) {
  return participants.filter((p) => p.session !== self)
    .map(({ session, author, view }) => ({ id: session, name: author.name || author.email, view }));
}

/** Coalesce motion to the latest viewport. Owner sessions also renew their lease while idle. */
export function useViewportPublication(
  view: DesignViewport | null, enabled: boolean, publish: (view: DesignViewport | null) => void, heartbeat = false,
) {
  const latest = useRef(view);
  const changed = useRef<() => void>(() => {});
  useEffect(() => {
    latest.current = view;
    changed.current();
  }, [view]);
  useEffect(() => {
    if (!enabled) return;
    let sent: DesignViewport | null = null;
    let last = -Infinity;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const flush = (renew = false) => {
      timer = undefined;
      if (!renew && sent === latest.current) return;
      sent = latest.current;
      last = performance.now();
      publish(sent);
    };
    changed.current = () => {
      if (timer !== undefined || sent === latest.current) return;
      const delay = last + PUBLISH_INTERVAL_MS - performance.now();
      if (delay > 0) timer = setTimeout(flush, delay);
      else flush();
    };
    changed.current();
    const interval = heartbeat ? setInterval(() => {
      if (sent && timer === undefined && performance.now() - last >= PUBLISH_INTERVAL_MS) flush(true);
    }, OWNER_HEARTBEAT_MS) : undefined;
    return () => {
      changed.current = () => {};
      clearTimeout(timer);
      clearInterval(interval);
      if (sent) publish(null);
    };
  }, [enabled, heartbeat, publish]);
}
