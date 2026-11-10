// The app's side of the daemon's design_capture.js (story 51): asks a framed screen, over
// postMessage, for the element under a point (a comment's pin) and whether earlier pins'
// selectors still match (a stale pin). The frame has an opaque origin, so a reply is trusted by
// the request number the app gave, never by its origin.

import type { PinElement } from "@chauffeur/protocol";
import { type CaptureReply, parseCaptureReply } from "@/lib/comments";

const TIMEOUT_MS = 1500;

let nextRequest = 1;
const waiting = new Map<number, (reply: CaptureReply) => void>();
let listening = false;

function listen() {
  if (listening) return;
  listening = true;
  window.addEventListener("message", (event) => {
    const reply = parseCaptureReply(event.data);
    if (!reply) return;
    const resolve = waiting.get(reply.request);
    if (!resolve) return;
    waiting.delete(reply.request);
    resolve(reply);
  });
}

/** One question to a frame; null when it does not answer (not loaded, no script, too slow). */
function ask(frame: HTMLIFrameElement, message: Record<string, unknown>): Promise<CaptureReply | null> {
  listen();
  const target = frame.contentWindow;
  if (!target) return Promise.resolve(null);
  const request = nextRequest++;
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      waiting.delete(request);
      resolve(null);
    }, TIMEOUT_MS);
    waiting.set(request, (reply) => {
      clearTimeout(timer);
      resolve(reply);
    });
    target.postMessage({ ...message, request }, "*");
  });
}

/** The element under a point of the screen, in percent of its size; null for none or no answer. */
export async function elementAt(frame: HTMLIFrameElement, x: number, y: number): Promise<PinElement | null> {
  const reply = await ask(frame, { type: "chauffeur:element", x, y });
  return reply?.kind === "element" ? reply.element : null;
}

/** Which of the selectors still match something in the screen; null when the frame does not answer. */
export async function selectorsFound(frame: HTMLIFrameElement, selectors: readonly string[]): Promise<Record<string, boolean> | null> {
  if (selectors.length === 0) return {};
  const reply = await ask(frame, { type: "chauffeur:check", selectors });
  return reply?.kind === "check" ? reply.found : null;
}
