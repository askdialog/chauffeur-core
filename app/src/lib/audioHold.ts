// A running AudioContext keeps the web view's audio threads busy, silent or not (about 9 % of a
// core in WebKitGTK): each one rests suspended while nothing uses it.

/** How long a context stays running once nothing holds it, so that sounds in a row share it. */
export const REST_MS = 250;

type Suspendable = Pick<AudioContext, "suspend">;

const holds = new WeakMap<Suspendable, { count: number; rest: ReturnType<typeof setTimeout> | null }>();

/**
 * Holds `context` running: it is suspended once every hold taken on it was let go, REST_MS after
 * the last. Gives the function that lets this hold go (calling it again does nothing).
 */
export function holdAudio(context: Suspendable): () => void {
  const hold = holds.get(context) ?? { count: 0, rest: null };
  holds.set(context, hold);
  hold.count += 1;
  if (hold.rest) clearTimeout(hold.rest);
  hold.rest = null;
  let held = true;
  return () => {
    if (!held) return;
    held = false;
    hold.count -= 1;
    if (hold.count > 0) return;
    hold.rest = setTimeout(() => {
      hold.rest = null;
      void context.suspend().catch(() => {});
    }, REST_MS);
  };
}
