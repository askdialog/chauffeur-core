import { afterEach, expect, test } from "bun:test";
import { createHmac } from "node:crypto";
import { createSlackHandler, signed } from "../src/server";
import { formatContext, parseShortcut, type Shortcut } from "../src/context";

const secret = "test-signing-secret";
const now = 1_800_000_000_000;
const shortcut: Shortcut = {
  type: "message_action", callback_id: "chauffeur_open", trigger_id: "trigger",
  team: { id: "T1", domain: "acme-test" }, channel: { id: "C1", name: "ops" }, user: { id: "U1" },
  message: { text: "Un bug &amp; son contexte", ts: "1800000000.000001", user: "U2", reply_count: 2 },
};
const closers: (() => void)[] = [];
afterEach(() => { for (const close of closers.splice(0)) close(); });
function request(value: unknown = shortcut, at = now, secretOverride = secret) {
  const body = new URLSearchParams({ payload: JSON.stringify(value) }).toString();
  const timestamp = String(at / 1000);
  const signature = `v0=${createHmac("sha256", secretOverride).update(`v0:${timestamp}:${body}`).digest("hex")}`;
  return new Request("https://share.example.com/slack/actions", { method: "POST", body, headers: {
    "x-slack-request-timestamp": timestamp, "x-slack-signature": signature,
  } });
}
function setup(pages: unknown[] = [], clock = () => now, identity = (_request: Request): { user_id: string; team_id: string } | null => ({ user_id: "U1", team_id: "T1" })) {
  const calls: { url: string; body: Record<string, unknown> }[] = [];
  // Only the fetch signature is used: the server never calls Bun's fetch.preconnect helper.
  const network = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, body: JSON.parse(String(init?.body)) });
    return Response.json(url.endsWith("views.open") ? { ok: true } : pages.shift() ?? { ok: false, error: "not_in_channel" });
  }) as typeof fetch;
  const handler = createSlackHandler({ signingSecret: secret, botToken: "test-token", teamId: "T1", publicUrl: "https://share.example.com", fetch: network, now: clock, identity });
  closers.push(handler.close);
  const link = (scope = "message") => {
    const payload = JSON.stringify(calls.find((call) => call.url.endsWith("views.open"))?.body);
    const token = /\/slack\/open\/([A-Za-z0-9_-]{43})/.exec(payload)?.[1];
    if (!token) throw new Error("No link offered");
    return `https://share.example.com/slack/context/${token}?scope=${scope}`;
  };
  return { ...handler, calls, link };
}

test("only signed, recent shortcuts from this Slack workspace can create an import", async () => {
  const app = setup();
  expect((await app.handle(request(shortcut, now, "wrong"))).status).toBe(401);
  expect((await app.handle(request(shortcut, now - 301_000))).status).toBe(401);
  expect((await app.handle(request({ ...shortcut, team: { ...shortcut.team, id: "TOTHER" } }))).status).toBe(403);
  expect(app.calls).toHaveLength(0);
});

test("a message shortcut offers local draft links without sending a channel message", async () => {
  const app = setup();
  expect((await app.handle(request())).status).toBe(200);
  expect(app.calls.map((call) => call.url)).toEqual(["https://slack.com/api/views.open"]);
  const context = await (await app.handle(new Request(app.link()))).json();
  expect(context.prompt).toContain("Un bug & son contexte");
  expect(context.prompt).toContain("https://acme-test.slack.com/archives/C1/p1800000000000001");
  expect(context.warning).toBeNull();
  expect(app.calls).toHaveLength(1);
});

test("Slack retries do not open duplicate modals", async () => {
  const app = setup();
  await app.handle(request());
  await app.handle(request());
  expect(app.calls).toHaveLength(1);
});

test("the landing page opens a local draft and never embeds private Slack text", async () => {
  const app = setup();
  await app.handle(request());
  const page = await app.handle(new Request(app.link().replace("/context/", "/open/")));
  expect(page.headers.get("cache-control")).toBe("no-store");
  expect(page.headers.get("referrer-policy")).toBe("no-referrer");
  const html = await page.text();
  expect(html).toContain("chauffeur://slack/");
  expect(html).not.toContain("Un bug");
  expect(html).not.toContain("<script");
});

test("expired and unknown links cannot retrieve context", async () => {
  let time = now;
  const app = setup([], () => time);
  await app.handle(request());
  const link = app.link();
  time += 600_001;
  expect((await app.handle(new Request(link))).status).toBe(410);
  expect((await app.handle(new Request(`https://share.example.com/slack/context/${"a".repeat(43)}?scope=message`))).status).toBe(410);
});

test("importing a reply retrieves all pages from the parent and shares concurrent reads", async () => {
  const reply = { ...shortcut, message: { ...shortcut.message, ts: "1800000001.000001", thread_ts: shortcut.message.ts } };
  const app = setup([
    { ok: true, messages: [shortcut.message], has_more: true, response_metadata: { next_cursor: "next" } },
    { ok: true, messages: [{ text: "La réponse", ts: "1800000001.000001", user: "U3" }], response_metadata: {} },
  ]);
  await app.handle(request(reply));
  const [a, b] = await Promise.all([app.handle(new Request(app.link("thread"))), app.handle(new Request(app.link("thread")))]);
  const context = await a.json();
  expect(await b.json()).toEqual(context);
  expect(context.warning).toBeNull();
  expect(context.prompt).toContain("Fil Slack");
  expect(context.prompt).toContain("La réponse");
  expect(app.calls[1]?.body.ts).toBe(shortcut.message.ts);
  expect(app.calls[2]?.body.cursor).toBe("next");
  expect(app.calls).toHaveLength(3);
});

test("denied thread access preserves the selected message and explicitly warns", async () => {
  const app = setup();
  await app.handle(request());
  const context = await (await app.handle(new Request(app.link("thread")))).json();
  expect(context.prompt).toContain("Message Slack");
  expect(context.prompt).toContain("Un bug");
  expect(context.warning).toContain("Seul le message sélectionné");
});

test("incomplete pagination never masquerades as a complete thread", async () => {
  const app = setup([{ ok: true, messages: [shortcut.message], has_more: true }]);
  await app.handle(request());
  expect((await (await app.handle(new Request(app.link("thread")))).json()).warning).not.toBeNull();
});

test("invalid import modes and oversized payloads are refused", async () => {
  const app = setup();
  await app.handle(request());
  expect((await app.handle(new Request(app.link("run")))).status).toBe(400);
  expect((await app.handle(new Request(`${app.link()}&scope=thread`))).status).toBe(400);
  expect((await app.handle(request({ ...shortcut, message: { ...shortcut.message, text: "x".repeat(300_000) } }))).status).toBe(413);
});

test("attachments stay as links and selected message payloads need no channel membership", () => {
  const withFile = { ...shortcut, message: { ...shortcut.message, files: [{ title: "Capture", permalink: "https://acme-test.slack.com/files/F1" }] } };
  const parsed = parseShortcut(withFile);
  expect(parsed).not.toBeNull();
  expect(formatContext(withFile, [withFile.message], false)).toContain("Capture: https://acme-test.slack.com/files/F1");
  expect(parseShortcut({ ...shortcut, team: { id: "T1", domain: "evil.example/path" } })).toBeNull();
  expect(signed("", "bad", "bad", secret, now)).toBe(false);
});

test("bot alerts retain attachment fields and block-only messages retain their text", () => {
  const parsed = parseShortcut({ ...shortcut, message: { ts: shortcut.message.ts, blocks: [
    { type: "rich_text", elements: [{ type: "rich_text_section", elements: [{ type: "text", text: "Incident " }, { type: "user", user_id: "U1" }] }] },
  ], attachments: [{ title: "Checkout", fields: [{ title: "Status", value: "500" }], text: "Payment failed" }] } });
  expect(parsed).not.toBeNull();
  const context = formatContext(parsed!, [parsed!.message], false);
  expect(context).toContain("Incident <@U1>");
  expect(context).toContain("Status: 500");
  expect(context).toContain("Payment failed");
});


test("only the Slack account invoking the shortcut can fetch its context", async () => {
  const app = setup([], () => now, req => req.headers.get("authorization") === "Bearer owner" ? {user_id:"U1",team_id:"T1"} : req.headers.has("authorization") ? {user_id:"U2",team_id:"T1"} : null);
  await app.handle(request());
  expect((await app.handle(new Request(app.link()))).status).toBe(401);
  expect((await app.handle(new Request(app.link(), {headers:{authorization:"Bearer other"}}))).status).toBe(403);
  expect((await app.handle(new Request(app.link(), {headers:{authorization:"Bearer owner"}}))).status).toBe(200);
});

function desktop(credential: string, focused = false, ack: string | null = null) {
  return new Request("https://share.example.com/slack/desktop", {
    method: "POST", headers: { authorization: `Bearer ${credential}` }, body: JSON.stringify({ focused, ack }),
  });
}

test("a connected desktop receives the full thread without a Slack modal and retries until acknowledged", async () => {
  const app = setup();
  expect(await (await app.handle(desktop("mac", true))).json()).toEqual({ link: null });
  await app.handle(request());
  await app.handle(request());
  expect(app.calls).toHaveLength(0);
  const delivery = await (await app.handle(desktop("mac"))).json();
  expect(delivery.link).toMatch(/^chauffeur:\/\/slack\/[\w-]{43}\?scope=thread$/);
  expect(await (await app.handle(desktop("mac"))).json()).toEqual(delivery);
  const token = new URL(delivery.link).pathname.slice(1);
  expect(await (await app.handle(desktop("mac", false, token))).json()).toEqual({ link: null });
  expect(await (await app.handle(desktop("mac"))).json()).toEqual({ link: null });
});

test("direct imports target only the last focused connected installation of the same Slack account", async () => {
  let time = now;
  const app = setup([], () => time, req => {
    const credential = req.headers.get("authorization");
    return credential ? { user_id: credential === "Bearer other" ? "U2" : "U1", team_id: "T1" } : null;
  });
  await app.handle(desktop("mac", true));
  time += 1000;
  await app.handle(desktop("linux", true));
  time += 1000;
  await app.handle(desktop("other", true));
  await app.handle(desktop("mac"));
  await app.handle(request());
  expect(await (await app.handle(desktop("mac"))).json()).toEqual({ link: null });
  expect(await (await app.handle(desktop("other"))).json()).toEqual({ link: null });
  const delivery = await (await app.handle(desktop("linux"))).json();
  const token = new URL(delivery.link).pathname.slice(1);
  await app.handle(desktop("mac", false, token));
  expect(await (await app.handle(desktop("linux"))).json()).toEqual(delivery);
  expect((await app.handle(new Request("https://share.example.com/slack/desktop", { method: "POST", body: "{}" }))).status).toBe(401);
});

test("stale desktops fall back to the manual links and signed-out sessions cannot receive", async () => {
  let time = now;
  let connected = true;
  const app = setup([], () => time, () => connected ? { user_id: "U1", team_id: "T1" } : null);
  await app.handle(desktop("mac", true));
  time += 15_001;
  await app.handle(request());
  expect(app.calls[0]?.url).toContain("views.open");
  connected = false;
  expect((await app.handle(desktop("mac"))).status).toBe(401);
});
