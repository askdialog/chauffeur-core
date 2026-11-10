import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { formatContext, parseMessage, parseShortcut, type Message, type Shortcut } from "./context";

const TTL = 10 * 60 * 1000;
const MAX_BODY = 256 * 1024;
const MAX_CONTEXT = 256 * 1024;
const headers = {
  "cache-control": "no-store", "referrer-policy": "no-referrer",
  "x-content-type-options": "nosniff",
  "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
};
type Draft = { shortcut: Shortcut; expires: number; thread?: Promise<Context>; desktop?: string; acknowledged?: boolean };
type Context = { prompt: string; warning: string | null };
type Options = {
  signingSecret: string;
  botToken: string;
  teamId: string;
  publicUrl: string;
  fetch?: typeof fetch;
  now?: () => number;
  identity: (request: Request) => { user_id: string; team_id: string } | null;
};

function response(body: string, status = 200) { return new Response(body, { status, headers }); }
function json(body: unknown) { return Response.json(body, { headers }); }

/** Verifies Slack's raw request body before decoding any user-controlled fields. */
export function signed(body: string, timestamp: string | null, signature: string | null, secret: string, now: number): boolean {
  if (!timestamp || !/^\d+$/.test(timestamp) || Math.abs(now / 1000 - Number(timestamp)) > 300
    || !signature || !/^v0=[a-f0-9]{64}$/.test(signature)) return false;
  const expected = `v0=${createHmac("sha256", secret).update(`v0:${timestamp}:${body}`).digest("hex")}`;
  return timingSafeEqual(Buffer.from(signature), Buffer.from(expected));
}

async function boundedBody(request: { body: ReadableStream<Uint8Array> | null }): Promise<string> {
  const reader = request.body?.getReader();
  if (!reader) return "";
  const chunks: Uint8Array[] = [];
  let size = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.length;
    if (size > MAX_BODY) { await reader.cancel(); throw new Error("body too large"); }
    chunks.push(value);
  }
  return Buffer.concat(chunks).toString("utf8");
}

export function createSlackHandler(options: Options) {
  const network = options.fetch ?? fetch;
  const now = options.now ?? Date.now;
  const publicUrl = new URL(options.publicUrl);
  if (publicUrl.protocol !== "https:" || publicUrl.username || publicUrl.password || publicUrl.pathname !== "/" || publicUrl.search || publicUrl.hash)
    throw new Error("SLACK_PUBLIC_URL must be an HTTPS origin.");
  const drafts = new Map<string, Draft>();
  const desktops = new Map<string, { user: string; team: string; seen: number; focused: number }>();
  const requests = new Map<string, number>();
  const prune = () => {
    for (const [id, desktop] of desktops) if (desktop.seen + 15_000 <= now()) desktops.delete(id);
    for (const [token, draft] of drafts) if (draft.expires <= now()) drafts.delete(token);
    for (const [key, expires] of requests) if (expires <= now()) requests.delete(key);
  };
  const cleanup = setInterval(prune, 30_000);
  cleanup.unref();

  async function slack(method: string, body: object, timeout = 8000): Promise<Record<string, unknown>> {
    const result = await network(`https://slack.com/api/${method}`, {
      method: "POST", headers: { authorization: `Bearer ${options.botToken}`, "content-type": "application/json" },
      body: JSON.stringify(body), signal: AbortSignal.timeout(timeout), redirect: "error",
    });
    if (!result.ok) throw new Error("Slack request failed");
    const value: unknown = JSON.parse(await boundedBody(result));
    if (!value || typeof value !== "object" || !("ok" in value) || value.ok !== true) throw new Error("Slack request refused");
    return value;
  }

  async function thread(draft: Draft): Promise<Context> {
    const messages: Message[] = [];
    let cursor = "";
    const cursors = new Set<string>();
    const start = now();
    try {
      do {
        if (now() - start > 20_000 || cursors.size >= 50) throw new Error("Thread too large");
        const page = await slack("conversations.replies", {
          channel: draft.shortcut.channel.id, ts: draft.shortcut.message.thread_ts ?? draft.shortcut.message.ts,
          limit: 100, ...(cursor ? { cursor } : {}),
        });
        if (!Array.isArray(page.messages)) throw new Error("Missing messages");
        for (const item of page.messages) {
          const message = parseMessage(item);
          if (!message) throw new Error("Invalid message");
          messages.push(message);
        }
        if (messages.length > 2000 || Buffer.byteLength(JSON.stringify(messages)) > MAX_CONTEXT) throw new Error("Thread too large");
        const metadata = page.response_metadata;
        cursor = metadata && typeof metadata === "object" && "next_cursor" in metadata && typeof metadata.next_cursor === "string" ? metadata.next_cursor.trim() : "";
        if (cursor && cursors.has(cursor) || page.has_more === true && !cursor) throw new Error("Incomplete thread");
        if (cursor) cursors.add(cursor);
      } while (cursor);
      if (!messages.length) throw new Error("Empty thread");
      const prompt = formatContext(draft.shortcut, messages, true);
      if (Buffer.byteLength(prompt) > MAX_CONTEXT) throw new Error("Thread too large");
      return { prompt, warning: null };
    } catch {
      return {
        prompt: formatContext(draft.shortcut, [draft.shortcut.message], false),
        warning: "Seul le message sélectionné a été importé : le fil complet est inaccessible, trop volumineux ou Slack est indisponible. Consulte le lien Slack pour les réponses.",
      };
    }
  }

  async function handle(request: Request): Promise<Response> {
    prune();
    const url = new URL(request.url);
    if (url.pathname === "/slack/health" && request.method === "GET") return response("ok");
    if (url.pathname === "/slack/desktop" && request.method === "POST") {
      const user = options.identity(request);
      if (!user) return response("Reconnecte ton compte Slack dans Réglages → Slack.", 401);
      const credential = request.headers.get("authorization");
      if (!credential) return response("Unauthorized", 401);
      let body: unknown;
      try { body = JSON.parse(await boundedBody(request)); } catch { return response("Invalid body", 400); }
      if (!body || typeof body !== "object" || !("focused" in body) || typeof body.focused !== "boolean"
        || ("ack" in body && body.ack !== null && (typeof body.ack !== "string" || !/^[\w-]{43}$/.test(body.ack)))) return response("Invalid body", 400);
      // A session identifies one installation; no credential is retained in memory.
      const id = createHash("sha256").update(credential).digest("base64url");
      const previous = desktops.get(id);
      if (!previous && desktops.size >= 1024) return response("Try again later", 503);
      desktops.set(id, { user: user.user_id, team: user.team_id, seen: now(), focused: body.focused ? now() : previous?.focused ?? 0 });
      if ("ack" in body && typeof body.ack === "string") {
        const draft = drafts.get(body.ack);
        if (draft?.desktop === id) draft.acknowledged = true;
      }
      const entry = [...drafts].find(([, draft]) => draft.desktop === id && !draft.acknowledged
        && draft.shortcut.user.id === user.user_id && draft.shortcut.team.id === user.team_id);
      if (!entry) return json({ link: null });
      const [token, draft] = entry;
      const scope = draft.shortcut.message.thread_ts || (draft.shortcut.message.reply_count ?? 0) > 0 ? "thread" : "message";
      return json({ link: `chauffeur://slack/${token}?scope=${scope}` });
    }
    if (url.pathname === "/slack/actions" && request.method === "POST") {
      let body: string;
      try { body = await boundedBody(request); } catch { return response("Payload too large", 413); }
      const signature = request.headers.get("x-slack-signature");
      if (!signed(body, request.headers.get("x-slack-request-timestamp"), signature, options.signingSecret, now())) return response("Unauthorized", 401);
      let value: unknown;
      try { value = JSON.parse(new URLSearchParams(body).get("payload") ?? ""); } catch { return response("Invalid payload", 400); }
      // Link buttons also generate an interaction; the browser handles their destination.
      if (value && typeof value === "object" && "type" in value && value.type === "block_actions") return response("");
      const shortcut = parseShortcut(value);
      if (!shortcut || shortcut.team.id !== options.teamId) return response("Invalid shortcut", 403);
      if (requests.has(signature!)) return response("");
      if (drafts.size >= 128) return response("Try again in a few minutes", 503);
      const token = randomBytes(32).toString("base64url");
      const draft: Draft = { shortcut, expires: now() + TTL };
      drafts.set(token, draft);
      requests.set(signature!, now() + TTL);
      const desktop = [...desktops].filter(([, device]) => device.user === shortcut.user.id && device.team === shortcut.team.id)
        .sort((a, b) => b[1].focused - a[1].focused)[0];
      if (desktop) {
        draft.desktop = desktop[0];
        return response("");
      }
      const button = (scope: string, label: string) => ({
        type: "button", action_id: `chauffeur_${scope}`, text: { type: "plain_text", text: label },
        url: `${publicUrl.origin}/slack/open/${token}?scope=${scope}`,
      });
      const buttons = [button("message", "Ce message")];
      if (shortcut.message.thread_ts || (shortcut.message.reply_count ?? 0) > 0) buttons.push(button("thread", "Le fil complet"));
      try {
        await slack("views.open", {
          trigger_id: shortcut.trigger_id,
          view: {
            type: "modal", title: { type: "plain_text", text: "Ouvrir dans Chauffeur" }, close: { type: "plain_text", text: "Fermer" },
            blocks: [
              { type: "section", text: { type: "plain_text", text: "Aucun Chauffeur connecté n’est joignable. Ouvre Chauffeur et connecte ton compte dans Réglages → Slack, puis relance le raccourci. Tu peux aussi utiliser un lien ci-dessous." } },
              { type: "actions", elements: buttons },
              { type: "context", elements: [{ type: "plain_text", text: "Lien valable 10 minutes. Le fil nécessite l’accès de l’app Chauffeur au channel. Les pièces jointes sont incluses comme liens." }] },
            ],
          },
        }, 2000);
      } catch {
        drafts.delete(token); requests.delete(signature!);
        return response("Could not open Chauffeur; try the shortcut again", 502);
      }
      return response("");
    }
    const match = /^\/slack\/(open|context)\/([A-Za-z0-9_-]{43})$/.exec(url.pathname);
    if (!match || request.method !== "GET") return response("Not found", 404);
    const token = match[2]!;
    const scope = url.searchParams.get("scope");
    if (url.searchParams.size !== 1 || (scope !== "message" && scope !== "thread")) return response("Invalid scope", 400);
    const draft = drafts.get(token);
    if (!draft) return response("Ce lien a expiré. Relance « Ouvrir dans Chauffeur » depuis le message Slack.", 410);
    if (match[1] === "context") {
      const user = options.identity(request);
      if (!user) return response("Connecte ton compte Slack dans Réglages → Slack.", 401);
      if (user.user_id !== draft.shortcut.user.id || user.team_id !== draft.shortcut.team.id) return response("Utilise le même compte Slack que pour le raccourci.", 403);
      if (scope === "message") return json({ prompt: formatContext(draft.shortcut, [draft.shortcut.message], false), warning: null });
      draft.thread ??= thread(draft);
      return json(await draft.thread);
    }
    const link = `chauffeur://slack/${token}?scope=${scope}`;
    return new Response(`<!doctype html><html lang="fr"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Ouvrir dans Chauffeur</title><style>body{font:17px system-ui;max-width:36rem;margin:15vh auto;padding:24px;background:#141414;color:#eee}a{display:inline-block;padding:14px 20px;background:#eee;color:#141414;border-radius:8px;text-decoration:none}p{line-height:1.6;color:#bbb}</style><h1>Continuer dans Chauffeur</h1><p>Ouvre le formulaire de nouvelle session, avec le ${scope === "thread" ? "fil Slack" : "message Slack"} prérempli. Tu pourras le modifier avant de créer la session.</p><a href="${link}">Ouvrir Chauffeur</a><p>Chauffeur doit être installé sur cet ordinateur et à jour. Si ton navigateur demande d’ouvrir l’application, accepte pour continuer.</p></html>`, { headers: { ...headers, "content-type": "text/html; charset=utf-8" } });
  }
  return { handle, close: () => { clearInterval(cleanup); drafts.clear(); requests.clear(); desktops.clear(); } };
}
