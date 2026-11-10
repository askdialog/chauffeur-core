// The app's toasts, kept apart from its daemon state: the components the share pages reuse (the
// copy buttons) raise them too, without the daemon's connection. An error toast is reported when
// a reporter is installed (the app's, `errorReports.ts`; never on the share pages).

import { create } from "zustand";

/** Where an error toast's report stands: offered (`ask`), on its way, or done with a note. */
export type ReportState = "ask" | "sending" | "done" | "failed";

export type Toast = {
  id: number;
  level: "info" | "error";
  message: string;
  report?: ReportState;
  /** What became of the report, said under the message. */
  reportNote?: string;
  /** The daemon's failure the toast shows, taken when it came. */
  failure?: string;
};

/** Sends the report of an error toast; resolves with the note to show, throws when it failed. */
export type Reporter = {
  /** `null` when this build or the user's choice reports nothing. */
  mode: () => "always" | "ask" | null;
  /** The daemon's failure a toast with this text shows, if any. */
  failureOf: (message: string) => string | undefined;
  send: (message: string, failure: string | undefined) => Promise<string>;
  /** The user chose to always send: from now on, without asking. */
  alwaysSend: () => Promise<void>;
};

export const useToasts = create<{ toasts: Toast[] }>(() => ({ toasts: [] }));

let toastId = 0;
let reporter: Reporter | null = null;

export function setReporter(next: Reporter | null) {
  reporter = next;
}

export function toast(message: string, level: "info" | "error" = "info") {
  const id = ++toastId;
  const mode = level === "error" ? reporter?.mode() ?? null : null;
  const report: ReportState | undefined = mode === "ask" ? "ask" : mode === "always" ? "sending" : undefined;
  const failure = report ? reporter?.failureOf(message) : undefined;
  useToasts.setState((s) => ({ toasts: [...s.toasts.slice(-4), { id, level, message, report, failure }] }));
  // A toast offering its report stays until the user answers or closes it.
  if (report !== "ask") dismissLater(id, level === "error" ? 8000 : 4000);
  if (report === "sending") void send(id);
}

export function dismissToast(id: number) {
  useToasts.setState((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
}

/** The user agreed to send this toast's report; `always` also stops asking. */
export async function sendToastReport(id: number, always = false) {
  const t = useToasts.getState().toasts.find((x) => x.id === id);
  if (!t || !reporter || (t.report !== "ask" && t.report !== "failed")) return;
  if (always) {
    try {
      await reporter.alwaysSend();
    } catch {
      // The setting stays as it was; this report still goes.
    }
  }
  await send(id);
  // A failed one stays, to try again.
  if (useToasts.getState().toasts.find((x) => x.id === id)?.report === "done") dismissLater(id, 4000);
}

async function send(id: number) {
  const t = useToasts.getState().toasts.find((x) => x.id === id);
  if (!t || !reporter) return;
  patch(id, { report: "sending", reportNote: undefined });
  try {
    const note = await reporter.send(t.message, t.failure);
    patch(id, { report: "done", reportNote: note });
  } catch (error) {
    patch(id, { report: "failed", reportNote: error instanceof Error ? error.message : String(error) });
  }
}

function patch(id: number, change: Partial<Toast>) {
  useToasts.setState((s) => ({ toasts: s.toasts.map((t) => (t.id === id ? { ...t, ...change } : t)) }));
}

function dismissLater(id: number, ms: number) {
  setTimeout(() => dismissToast(id), ms);
}
