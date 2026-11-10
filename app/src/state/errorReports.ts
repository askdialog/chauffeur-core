// Reports of the app's error toasts to Chauffeur's Sentry, through the daemon, as the user chose
// (`error_reports`; docs/adr/error-reports-to-sentry.md).

import type { AppSettings, ErrorReports } from "@chauffeur/protocol";
import { type Failure, failureOf, keepFailure, reportNote } from "@/lib/errorReports";
import type { DaemonClient, RpcCallError } from "@/lib/rpc";
import { setReporter } from "./toasts";
import { version as APP_VERSION } from "../../package.json";

const failures: Failure[] = [];
let mode: ErrorReports | null = null;

/** Keeps a failed call's failure id, for the toast that may show it. */
export function rememberFailure(error: RpcCallError) {
  if (error.failure) keepFailure(failures, { failure: error.failure, message: error.message, at: Date.now() });
}

/** The setting in force, from `settings.get` / `settings.set`. */
export function setErrorReports(settings: Pick<AppSettings, "error_reports" | "error_reports_available">) {
  mode = settings.error_reports_available ? settings.error_reports : null;
}

export function installErrorReports(client: DaemonClient) {
  setReporter({
    mode: () => (mode === "never" ? null : mode),
    failureOf: (message) => failureOf(failures, message, Date.now()),
    send: async (message, failure) => {
      const outcome = await client.call("errors.report", { message, failure, app_version: APP_VERSION });
      return reportNote(outcome);
    },
    alwaysSend: async () => {
      setErrorReports(await client.call("settings.set", { error_reports: "always" }));
    },
  });
}
