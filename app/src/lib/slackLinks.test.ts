import { expect, test } from "bun:test";
import { slackLinkReceiver } from "./slackLinks";

test("startup and live links import once, in order, without submitting any workspace", async () => {
  const batches = [["first"], ["first", "second"]];
  const prompts: string[] = [];
  const reports: string[] = [];
  const drain = slackLinkReceiver(
    async () => batches.shift() ?? [],
    async (link) => ({ prompt: link, warning: link === "second" ? "Message only" : null }),
    (prompt) => prompts.push(prompt),
    (message) => reports.push(message),
  );
  await Promise.all([drain(), drain()]);
  expect(prompts).toEqual(["first", "second"]);
  expect(reports).toEqual(["Message only"]);
});

test("a failed import reports its error and can be retried with the same link", async () => {
  let fail = true;
  const prompts: string[] = [];
  const reports: boolean[] = [];
  const drain = slackLinkReceiver(async () => ["link"], async () => {
    if (fail) throw new Error("Network unavailable");
    return { prompt: "context", warning: null };
  }, (prompt) => prompts.push(prompt), (_, error) => reports.push(error));
  await drain();
  expect(prompts).toEqual([]);
  fail = false;
  await drain();
  expect(prompts).toEqual(["context"]);
  expect(reports).toEqual([true]);
});

test("the user can reopen a cancelled import using the same unexpired link", async () => {
  const prompts: string[] = [];
  const drain = slackLinkReceiver(async () => ["link"], async () => ({ prompt: "context", warning: null }),
    (prompt) => prompts.push(prompt), () => {});
  await drain();
  await Bun.sleep(2100);
  await drain();
  expect(prompts).toEqual(["context", "context"]);
});

test("direct delivery retries lost acknowledgements without duplicating the draft", async () => {
  const { slackDesktopReceiver } = await import("./slackLinks");
  const token = "a".repeat(43);
  const prompts: string[] = [];
  const acknowledgements: (string | null)[] = [];
  let attempts = 0;
  let focuses = 0;
  const poll = slackDesktopReceiver(async (ack) => {
    acknowledgements.push(ack);
    if (++attempts === 2) throw new Error("Lost response");
    return { link: `chauffeur://slack/${token}?scope=thread` };
  }, async () => ({ prompt: "The thread", warning: null }), text => prompts.push(text),
  async () => { focuses++; }, () => {});
  await poll(); await poll(); await poll();
  expect(prompts).toEqual(["The thread"]);
  expect(focuses).toBe(1);
  expect(acknowledgements).toEqual([null, token, token]);
});

test("failed direct imports stay unacknowledged until their context has loaded", async () => {
  const { slackDesktopReceiver } = await import("./slackLinks");
  const acknowledgements: (string | null)[] = [];
  const token = "b".repeat(43);
  let reads = 0;
  const prompts: string[] = [];
  const reports: string[] = [];
  const poll = slackDesktopReceiver(async ack => {
    acknowledgements.push(ack);
    return { link: ack ? null : `chauffeur://slack/${token}?scope=thread` };
  }, async () => {
    if (++reads === 1) throw new Error("Offline");
    return { prompt: "Message only", warning: "Thread unavailable" };
  }, text => prompts.push(text), async () => {}, message => reports.push(message));
  await poll(); await poll(); await poll();
  expect(acknowledgements).toEqual([null, null, token]);
  expect(prompts).toEqual(["Message only"]);
  expect(reports).toHaveLength(2);
});

test("an outage of the Slack imports service is reported only once it lasts a minute", async () => {
  const { slackDesktopReceiver, SLACK_OUTAGE_GRACE } = await import("./slackLinks");
  let clock = 0;
  let down = true;
  const reports: string[] = [];
  const poll = slackDesktopReceiver(async () => {
    if (down) throw new Error("Offline");
    return { link: null };
  }, async () => ({ prompt: "", warning: null }), () => {}, async () => {}, message => reports.push(message), () => clock);
  // Waking from sleep: a few failed polls, then the network is back.
  await poll(); clock = SLACK_OUTAGE_GRACE - 1; await poll();
  down = false; await poll();
  down = true; clock += 1; await poll();
  clock += SLACK_OUTAGE_GRACE - 1; await poll();
  expect(reports).toEqual([]);
  clock += 1; await poll(); clock += SLACK_OUTAGE_GRACE; await poll();
  expect(reports).toEqual(["Could not receive Slack imports for a minute: Offline"]);
});

test("background imports stay silent offline and restart the outage timer on reconnect", async () => {
  const { slackDesktopReceiver, SLACK_OUTAGE_GRACE } = await import("./slackLinks");
  let clock = 0;
  let online = true;
  let attempts = 0;
  const reports: string[] = [];
  const prompts: string[] = [];
  const token = "d".repeat(43);
  const poll = slackDesktopReceiver(async () => {
    attempts++;
    if (attempts === 2) online = false;
    if (attempts <= 4) throw new Error("Service unreachable");
    return { link: `chauffeur://slack/${token}?scope=message` };
  }, async () => ({ prompt: "Slack message", warning: null }), prompt => prompts.push(prompt),
  async () => {}, message => reports.push(message), () => clock, () => online);

  await poll();
  clock += SLACK_OUTAGE_GRACE;
  await poll(); // The network goes offline during the request.
  clock += 2 * SLACK_OUTAGE_GRACE;
  await poll();
  expect(attempts).toBe(2);
  expect(reports).toEqual([]);

  online = true;
  await poll();
  expect(attempts).toBe(3);
  expect(reports).toEqual([]);
  clock += SLACK_OUTAGE_GRACE;
  await poll();
  expect(reports).toEqual(["Could not receive Slack imports for a minute: Service unreachable"]);
  await poll();
  expect(prompts).toEqual(["Slack message"]);
});

test("a Slack message that keeps failing to load is reported once", async () => {
  const { slackDesktopReceiver } = await import("./slackLinks");
  const reports: string[] = [];
  const poll = slackDesktopReceiver(async () => ({ link: `chauffeur://slack/${"c".repeat(43)}?scope=message` }),
    async () => { throw new Error("Expired"); }, () => {}, async () => {}, message => reports.push(message));
  await poll(); await poll(); await poll();
  expect(reports).toEqual(["Could not open the Slack message: Expired"]);
});
