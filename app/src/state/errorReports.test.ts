import { afterEach, describe, expect, test } from "bun:test";
import type { DaemonClient } from "@/lib/rpc";
import { RpcCallError } from "@/lib/rpc";
import { installErrorReports, rememberFailure, setErrorReports } from "./errorReports";
import { dismissToast, sendToastReport, setReporter, toast, useToasts } from "./toasts";

function failed(message: string, failure?: string) {
  return new RpcCallError("sections.move", { code: 500, message, failure });
}

/** A client whose calls are recorded, `errors.report` answering `sent`. */
function fakeClient() {
  const calls: { method: string; params: unknown }[] = [];
  const client = {
    call: async (method: string, params: unknown) => {
      calls.push({ method, params });
      if (method === "settings.set") return { error_reports: "always", error_reports_available: true };
      return "sent";
    },
  } as unknown as DaemonClient;
  return { client, calls };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

afterEach(() => {
  for (const t of useToasts.getState().toasts) dismissToast(t.id);
  setReporter(null);
});

describe("error toasts", () => {
  test("ask: the toast offers its report, sent with its failure once the user agrees", async () => {
    const { client, calls } = fakeClient();
    installErrorReports(client);
    setErrorReports({ error_reports: "ask", error_reports_available: true });
    rememberFailure(failed("section busy", "f-9"));
    toast("Could not move the section: section busy", "error");
    const [t] = useToasts.getState().toasts;
    expect(t.report).toBe("ask");
    expect(calls).toEqual([]);
    await sendToastReport(t.id);
    expect(calls[0].method).toBe("errors.report");
    expect(calls[0].params).toMatchObject({ message: "Could not move the section: section busy", failure: "f-9" });
    expect(useToasts.getState().toasts[0]).toMatchObject({ report: "done", reportNote: "Report sent" });
  });

  test("Always send turns the setting to always, then sends", async () => {
    const { client, calls } = fakeClient();
    installErrorReports(client);
    setErrorReports({ error_reports: "ask", error_reports_available: true });
    toast("Could not pin it", "error");
    await sendToastReport(useToasts.getState().toasts[0].id, true);
    expect(calls.map((c) => c.method)).toEqual(["settings.set", "errors.report"]);
    expect(calls[0].params).toEqual({ error_reports: "always" });
    // From now on, without asking.
    toast("Could not unpin it", "error");
    await settle();
    expect(useToasts.getState().toasts.at(-1)).toMatchObject({ report: "done" });
    expect(calls.filter((c) => c.method === "errors.report")).toHaveLength(2);
  });

  test("never, a build without Sentry, or an info toast report nothing", async () => {
    const { client, calls } = fakeClient();
    installErrorReports(client);
    setErrorReports({ error_reports: "never", error_reports_available: true });
    toast("Could not pin it", "error");
    setErrorReports({ error_reports: "always", error_reports_available: false });
    toast("Could not pin it", "error");
    setErrorReports({ error_reports: "always", error_reports_available: true });
    toast("Copied", "info");
    await settle();
    expect(useToasts.getState().toasts.every((t) => t.report === undefined)).toBe(true);
    expect(calls).toEqual([]);
  });

  test("a failed report says why and can be tried again", async () => {
    let fail = true;
    setReporter({
      mode: () => "ask",
      failureOf: () => undefined,
      send: async () => {
        if (fail) throw new Error("Sentry refused the report");
        return "Report sent";
      },
      alwaysSend: async () => {},
    });
    toast("Could not pin it", "error");
    const id = useToasts.getState().toasts[0].id;
    await sendToastReport(id);
    expect(useToasts.getState().toasts[0]).toMatchObject({ report: "failed", reportNote: "Sentry refused the report" });
    fail = false;
    await sendToastReport(id);
    expect(useToasts.getState().toasts[0]).toMatchObject({ report: "done", reportNote: "Report sent" });
  });
});
