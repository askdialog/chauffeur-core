import { createSlackAuth } from "./auth";
import { createSlackHandler } from "./server";

function required(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Set ${name} before starting Slack imports.`);
  return value;
}
const publicUrl = required("SLACK_PUBLIC_URL");
const auth = process.env.SLACK_CLIENT_SECRET ? createSlackAuth({
  clientId: required("SLACK_CLIENT_ID"), clientSecret: required("SLACK_CLIENT_SECRET"),
  teamId: required("SLACK_TEAM_ID"), publicUrl, database: process.env.SLACK_DATABASE ?? "/data/slack.sqlite",
}) : null;
const handler = createSlackHandler({
  signingSecret: required("SLACK_SIGNING_SECRET"), botToken: required("SLACK_BOT_TOKEN"),
  teamId: required("SLACK_TEAM_ID"), publicUrl, identity: request => auth?.identity(request) ?? null,
});
const server = Bun.serve({
  hostname: "127.0.0.1", port: Number(process.env.PORT ?? "8091"),
  maxRequestBodySize: 256 * 1024, fetch: request => new URL(request.url).pathname.startsWith("/slack/auth/")
    ? auth?.handle(request) ?? new Response("La connexion Slack doit être activée par un administrateur.", { status: 503 })
    : handler.handle(request),
  error: () => new Response("Slack imports unavailable; try again.", { status: 500 }),
});
console.info(`Slack imports listening on 127.0.0.1:${server.port}`);
function stop() { handler.close(); auth?.close(); void server.stop(true); }
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
