import { createHash, randomBytes } from "node:crypto";
import { Database } from "bun:sqlite";
import * as oidc from "openid-client";

const TTL = 10 * 60_000;
const SESSION_TTL = 30 * 24 * 60 * 60_000;
const token = () => randomBytes(32).toString("base64url");
const hash = (value: string) => createHash("sha256").update(value).digest("base64url");
const valid = (value: unknown): value is string => typeof value === "string" && /^[\w-]{43}$/.test(value);
const headers = { "cache-control": "no-store", "referrer-policy": "no-referrer", "content-security-policy": "default-src 'none'; frame-ancestors 'none'; base-uri 'none'", "x-content-type-options": "nosniff" };
export type Identity = { user_id: string; team_id: string; name: string };
type Flow = { challenge: string; nonce: string; verifier: string; expires: number; exchanging?: boolean; identity?: Identity; failed?: boolean; session?: string };
type Options = { clientId: string; clientSecret: string; teamId: string; publicUrl: string; database: string; now?: () => number; fetch?: oidc.CustomFetch };

/** Slack credentials stay server-side; the desktop redeems a flow with its own proof. */
export function createSlackAuth(options: Options) {
  const now = options.now ?? Date.now;
  const callback = `${new URL(options.publicUrl).origin}/slack/auth/callback`;
  const config = new oidc.Configuration({
    issuer: "https://slack.com", authorization_endpoint: "https://slack.com/openid/connect/authorize",
    token_endpoint: "https://slack.com/api/openid.connect.token", jwks_uri: "https://slack.com/openid/connect/keys",
    id_token_signing_alg_values_supported: ["RS256"],
  }, options.clientId, options.clientSecret, oidc.ClientSecretPost(options.clientSecret));
  config.timeout = 15;
  if (options.fetch) config[oidc.customFetch] = options.fetch;
  oidc.enableNonRepudiationChecks(config);
  const db = new Database(options.database, { create: true });
  db.exec("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS sessions (hash TEXT PRIMARY KEY, user_id TEXT NOT NULL, team_id TEXT NOT NULL, name TEXT NOT NULL, expires INTEGER NOT NULL)");
  const flows = new Map<string, Flow>();
  function prune() {
    for (const [id, flow] of flows) if (flow.expires <= now()) flows.delete(id);
    db.query("DELETE FROM sessions WHERE expires <= ?").run(now());
  }
  const timer = setInterval(prune, 30_000); timer.unref();
  const json = (value: unknown, status = 200) => Response.json(value, { status, headers });
  const error = (status: number) => json({ error: "Connexion Slack impossible ; relance Se connecter avec Slack dans Chauffeur." }, status);
  const page = (ok: boolean) => new Response(ok
    ? "<!doctype html><html lang=fr><meta charset=utf-8><title>Slack connecté</title><h1>Slack est connecté</h1><p>Retourne dans Chauffeur. Tu peux fermer cet onglet.</p></html>"
    : "<!doctype html><html lang=fr><meta charset=utf-8><title>Connexion Slack interrompue</title><h1>Connexion Slack interrompue</h1><p>Relance Se connecter avec Slack dans Chauffeur et utilise ton compte du workspace Slack de ton équipe.</p></html>",
  { status: ok ? 200 : 400, headers: { ...headers, "content-type": "text/html; charset=utf-8" } });
  function identity(request: Request): Identity | null {
    const credential = request.headers.get("authorization")?.replace(/^Bearer /, "") ?? "";
    if (!valid(credential)) return null;
    return db.query<Identity, [string, number]>("SELECT user_id, team_id, name FROM sessions WHERE hash = ? AND expires > ?").get(hash(credential), now());
  }
  async function handle(request: Request): Promise<Response> {
    prune();
    const url = new URL(request.url);
    if (url.pathname === "/slack/auth/callback" && ["GET", "POST"].includes(request.method)) {
      const params = request.method === "POST" ? new URLSearchParams(await request.clone().text()) : url.searchParams;
      const state = params.get("state") ?? "";
      const flow = flows.get(state);
      if (!flow || flow.exchanging || flow.identity || flow.failed) return page(false);
      flow.exchanging = true;
      try {
        // Reconstruct the public URL: reverse proxies forward a loopback Host.
        const incoming = request.method === "POST"
          ? new Request(callback, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: params })
          : new URL(`${callback}?${params}`);
        const result = await oidc.authorizationCodeGrant(config, incoming, { expectedState: state, expectedNonce: flow.nonce, pkceCodeVerifier: flow.verifier });
        const claims = result.claims();
        const user = claims?.["https://slack.com/user_id"];
        if (claims?.["https://slack.com/team_id"] !== options.teamId || typeof user !== "string" || claims.sub !== user || !/^[UW][A-Z0-9]+$/.test(user)) throw new Error("Invalid workspace identity");
        if (flows.get(state) !== flow || flow.expires <= now()) return page(false);
        flow.identity = { user_id: user, team_id: options.teamId, name: typeof claims.name === "string" ? claims.name.slice(0, 200) : user };
        return page(true);
      } catch { flow.failed = true; return page(false); }
    }
    if (url.pathname === "/slack/auth/me" && request.method === "GET") {
      const user = identity(request); return user ? json(user) : error(401);
    }
    if (url.pathname === "/slack/auth/logout" && request.method === "POST") {
      const credential = request.headers.get("authorization")?.replace(/^Bearer /, "") ?? "";
      if (valid(credential)) db.query("DELETE FROM sessions WHERE hash = ?").run(hash(credential));
      return json({});
    }
    if (request.method !== "POST") return error(404);
    let body: Record<string, unknown>;
    try { body = await request.json(); if (!body || typeof body !== "object") return error(400); } catch { return error(400); }
    if (url.pathname === "/slack/auth/start") {
      if (!valid(body.challenge)) return error(400);
      if (flows.size >= 128) return error(429);
      const id = token();
      const flow: Flow = { challenge: body.challenge, nonce: token(), verifier: oidc.randomPKCECodeVerifier(), expires: now() + TTL };
      flows.set(id, flow);
      const authorization = oidc.buildAuthorizationUrl(config, {
        redirect_uri: callback, scope: "openid profile", response_type: "code", response_mode: "form_post",
        team: options.teamId, state: id, nonce: flow.nonce,
        code_challenge: await oidc.calculatePKCECodeChallenge(flow.verifier), code_challenge_method: "S256",
      });
      return json({ id, url: authorization.href });
    }
    const id = typeof body.id === "string" ? body.id : "";
    const flow = flows.get(id);
    if (!flow || !valid(body.verifier) || hash(body.verifier) !== flow.challenge) return error(410);
    if (url.pathname === "/slack/auth/cancel") {
      if (flow.session) db.query("DELETE FROM sessions WHERE hash = ?").run(hash(flow.session));
      flows.delete(id); return json({});
    }
    if (url.pathname !== "/slack/auth/complete") return error(404);
    if (flow.failed) return error(403);
    if (!flow.identity) return json({ pending: true });
    if (!flow.session) {
      flow.session = token();
      db.query("INSERT INTO sessions VALUES (?, ?, ?, ?, ?)").run(hash(flow.session), flow.identity.user_id, flow.identity.team_id, flow.identity.name, now() + SESSION_TTL);
    }
    // Retain the result until flow expiry: a dropped response must not lose the sign-in.
    return json({ pending: false, credential: flow.session, identity: flow.identity });
  }
  return { handle, identity, close() { clearInterval(timer); flows.clear(); db.close(); } };
}
