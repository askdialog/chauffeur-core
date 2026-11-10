import { afterEach, expect, test } from "bun:test";
import { createHash, createHmac, randomBytes } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { exportJWK, generateKeyPair, SignJWT } from "jose";
import { createSlackAuth } from "../src/auth";
import { createSlackHandler } from "../src/server";

const origin = "https://share.example.com";
const keys = await generateKeyPair("RS256");
const wrongKeys = await generateKeyPair("RS256");
const jwk = { ...await exportJWK(keys.publicKey), kid: "test", alg: "RS256", use: "sig" };
const closers: (() => void)[] = [];
afterEach(() => { for (const close of closers.splice(0)) close(); });
async function setup(change: Record<string, unknown> = {}, clock = Date.now) {
  const folder = mkdtempSync(join(tmpdir(), "chauffeur-slack-auth-"));
  let nonce = "";
  let exchanges = 0;
  const options = { clientId: "test-client", clientSecret: "test-secret", teamId: "T1", publicUrl: origin, database: join(folder, "sessions.sqlite"), now: clock,
    fetch: async (url: string) => {
      if (url.endsWith("/keys")) return Response.json({keys:[jwk]});
      exchanges++;
      const id_token = await new SignJWT({nonce, "https://slack.com/team_id":"T1", "https://slack.com/user_id":"U1", name:"Test colleague", ...change})
        .setProtectedHeader({alg:"RS256",kid:"test"}).setIssuer(typeof change.issuer === "string" ? change.issuer : "https://slack.com").setAudience(typeof change.audience === "string" ? change.audience : "test-client").setSubject("U1").setIssuedAt().setExpirationTime(change.expired ? "-1m" : "5m").sign(change.badSignature ? wrongKeys.privateKey : keys.privateKey);
      return Response.json({access_token:"test-access",token_type:"Bearer",id_token});
    },
  };
  let auth = createSlackAuth(options);
  closers.push(() => { auth.close(); rmSync(folder, {recursive:true}); });
  const post = (path: string, body: object) => auth.handle(new Request(`${origin}/slack/auth/${path}`, {method:"POST", headers:{"content-type":"application/json"},body:JSON.stringify(body)}));
  const verifier = randomBytes(32).toString("base64url");
  const response = await post("start", {challenge:createHash("sha256").update(verifier).digest("base64url")});
  const start = await response.json();
  const url = new URL(start.url);
  nonce = url.searchParams.get("nonce")!;
  const callback = (state = start.id) => auth.handle(new Request(`${origin}/slack/auth/callback`, {method:"POST",body:new URLSearchParams({state,code:"test-code"}),headers:{"content-type":"application/x-www-form-urlencoded"}}));
  const complete = (proof = verifier) => post("complete", {id:start.id,verifier:proof});
  const me = (credential: string) => auth.handle(new Request(`${origin}/slack/auth/me`,{headers:{authorization:`Bearer ${credential}`}}));
  return { identity: (request: Request) => auth.identity(request), post, url, start, verifier, callback, complete, me, exchanges: () => exchanges,
    restart: () => { auth.close(); auth = createSlackAuth(options); },
    logout: (credential: string) => auth.handle(new Request(`${origin}/slack/auth/logout`, {method:"POST",headers:{authorization:`Bearer ${credential}`}})),
  };
}

test("SSO verifies a signed identity and only the initiating desktop can redeem it; sessions survive restart and logout revokes them", async () => {
  const app = await setup();
  expect(app.url.searchParams.get("scope")).toBe("openid profile");
  expect(app.url.searchParams.get("code_challenge_method")).toBe("S256");
  expect(await (await app.complete()).json()).toEqual({pending:true});
  expect((await app.callback()).status).toBe(200);
  expect((await app.complete("a".repeat(43))).status).toBe(410);
  const result = await (await app.complete()).json();
  expect(result.identity).toEqual({user_id:"U1",team_id:"T1",name:"Test colleague"});
  expect((await (await app.complete()).json()).credential).toBe(result.credential);
  expect((await app.callback()).status).toBe(400);
  expect(app.exchanges()).toBe(1);
  app.restart();
  expect((await app.me(result.credential)).status).toBe(200);
  await app.logout(result.credential);
  expect((await app.me(result.credential)).status).toBe(401);
});

test("wrong workspace, nonce, issuer, audience, signature or expiry never creates a session", async () => {
  for (const change of [{"https://slack.com/team_id":"TOTHER"}, {nonce:"wrong"}, {issuer:"https://example.com"}, {audience:"another-client"}, {badSignature:true}, {expired:true}]) {
    const app = await setup(change);
    expect((await app.callback()).status).toBe(400);
    expect((await app.complete()).status).toBe(403);
  }
});

test("unknown state, cancellation and expired flows cannot connect a desktop", async () => {
  let now = Date.now();
  const app = await setup({}, () => now);
  expect((await app.callback("a".repeat(43))).status).toBe(400);
  expect(app.exchanges()).toBe(0);
  await app.post("cancel", {id:app.start.id,verifier:app.verifier});
  expect((await app.callback()).status).toBe(400);
  expect((await app.complete()).status).toBe(410);
  const expired = await setup({}, () => now);
  now += 600_001;
  expect((await expired.callback()).status).toBe(400);
  expect((await expired.complete()).status).toBe(410);
});

test("a session expires even without a service restart", async () => {
  let now = Date.now();
  const app = await setup({}, () => now);
  await app.callback();
  const {credential} = await (await app.complete()).json();
  now += 30 * 24 * 60 * 60_000 + 1;
  expect((await app.me(credential)).status).toBe(401);
});


test("an authenticated service delivers to its signed-in desktop and logout revokes pending delivery", async () => {
  const app = await setup();
  await app.callback();
  const { credential } = await (await app.complete()).json();
  const service = createSlackHandler({ signingSecret: "signing", botToken: "bot", teamId: "T1", publicUrl: origin, identity: app.identity });
  closers.push(service.close);
  const poll = () => service.handle(new Request(`${origin}/slack/desktop`, { method: "POST", headers: { authorization: `Bearer ${credential}` }, body: JSON.stringify({ focused: true, ack: null }) }));
  expect(await (await poll()).json()).toEqual({ link: null });
  const body = new URLSearchParams({ payload: JSON.stringify({
    type: "message_action", callback_id: "chauffeur_open", trigger_id: "trigger",
    team: { id: "T1", domain: "test" }, channel: { id: "C1" }, user: { id: "U1" }, message: { text: "An incident", ts: "1800000000.000001" },
  }) }).toString();
  const timestamp = String(Math.floor(Date.now() / 1000));
  const signature = `v0=${createHmac("sha256", "signing").update(`v0:${timestamp}:${body}`).digest("hex")}`;
  expect((await service.handle(new Request(`${origin}/slack/actions`, { method: "POST", body, headers: { "x-slack-signature": signature, "x-slack-request-timestamp": timestamp } }))).status).toBe(200);
  const { link } = await (await poll()).json();
  expect(link).toEndWith("?scope=message");
  const context = link.replace("chauffeur://slack/", `${origin}/slack/context/`);
  expect((await (await service.handle(new Request(context, { headers: { authorization: `Bearer ${credential}` } }))).json()).prompt).toContain("An incident");
  await app.logout(credential);
  expect((await poll()).status).toBe(401);
  expect((await service.handle(new Request(context, { headers: { authorization: `Bearer ${credential}` } }))).status).toBe(401);
});
