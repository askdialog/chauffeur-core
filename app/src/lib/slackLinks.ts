import type { SlackContext } from "@chauffeur/protocol";
import { isTauri } from "./native";
import { errorMessage } from "./rpc";

/** Serializes imports and coalesces the OS's startup/event duplicate without losing later links. */
export function slackLinkReceiver(
  take: () => Promise<string[]>,
  read: (link: string) => Promise<SlackContext>,
  prefill: (prompt: string) => void,
  report: (message: string, error: boolean) => void,
): () => Promise<void> {
  const seen = new Set<string>();
  let pending = Promise.resolve();
  return () => {
    pending = pending.then(async () => {
      for (const link of await take()) {
        if (seen.has(link)) continue;
        seen.add(link);
        if (seen.size > 128) seen.delete(seen.values().next().value!);
        try {
          const context = await read(link);
          prefill(context.prompt);
          if (context.warning) report(context.warning, false);
        } catch (error) {
          seen.delete(link);
          report(errorMessage(error), true);
        } finally {
          // Allow reopening a cancelled form with the same link.
          setTimeout(() => seen.delete(link), 2000);
        }
      }
    }).catch((error: unknown) => report(`Could not open Slack context: ${errorMessage(error)}`, true));
    return pending;
  };
}

/** Registers before draining startup links, so opening the app and opening an idle app behave alike. */
export async function listenForSlackLinks(
  prefill: (prompt: string) => void,
  report: (message: string, error: boolean) => void,
): Promise<void> {
  if (!isTauri) return;
  const { client, openSettings } = await import("@/state/app");
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const drain = slackLinkReceiver(
    () => invoke<string[]>("take_slack_links"),
    async (link) => {
      const status = await client.call("slack.status", {});
      if (!status.identity) {
        openSettings(true, "slack");
        throw new Error("Connecte ton compte Slack, puis rouvre le message depuis Slack.");
      }
      return client.call("slack.context", { link });
    },
    prefill,
    report,
  );
  await listen("slack-open", drain);
  await drain();
  const receive = slackDesktopReceiver(
    (ack) => client.call("slack.receive", { focused: document.hasFocus(), ack }),
    (link) => client.call("slack.context", { link }),
    prefill,
    () => invoke("focus_slack_desktop"),
    report,
  );
  const poll = async () => {
    await receive();
    setTimeout(() => void poll(), 3000);
  };
  void poll();
}

/** How long the imports service may stay unreachable before the user hears of it: a laptop
 * waking from sleep fails a few polls before its network is back. */
export const SLACK_OUTAGE_GRACE = 60_000;

/** Retries lost acknowledgements without appending the same import twice. Reports an outage
 * only once it lasts, and a message that fails to load once per message. */
export function slackDesktopReceiver(
  receive: (ack: string | null) => Promise<{ link: string | null }>,
  read: (link: string) => Promise<SlackContext>,
  prefill: (prompt: string) => void,
  focus: () => Promise<void>,
  report: (message: string, error: boolean) => void,
  now: () => number = Date.now,
): () => Promise<void> {
  let ack: string | null = null;
  let failingSince: number | null = null;
  let outageReported = false;
  let failedLink: string | null = null;
  return async () => {
    let link: string | null;
    try {
      ({ link } = await receive(ack));
    } catch (error) {
      failingSince ??= now();
      if (!outageReported && now() - failingSince >= SLACK_OUTAGE_GRACE) {
        report(`Could not receive Slack imports for a minute: ${errorMessage(error)}`, true);
        outageReported = true;
      }
      return;
    }
    failingSince = null;
    outageReported = false;
    if (!link) return;
    try {
      const token = new URL(link).pathname.slice(1);
      if (token === ack) return;
      const context = await read(link);
      prefill(context.prompt);
      ack = token;
      failedLink = null;
      if (context.warning) report(context.warning, false);
      await focus();
    } catch (error) {
      if (failedLink !== link) report(`Could not open the Slack message: ${errorMessage(error)}`, true);
      failedLink = link;
    }
  };
}
