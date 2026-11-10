# Slack → local new workspace

In any message's **⋯** menu, under Slack's connected apps, the app's message shortcut (callback ID `chauffeur_open`, named in `manifest.json`), the full thread opens directly in the connected desktop's existing ⌘N form, without a Slack modal or browser click. Keep Chauffeur open and signed in under **Settings → Slack**.

The integration is off until configured: `[slack] url` in `settings.toml` (empty by default) names the Slack service's HTTPS origin, which an operator hosts as below. Among connected installations of your Slack account, the last focused desktop receives the import. The app brings its main window to the front and you choose repos/model and submit. Nothing creates a workspace or runs an agent before submission. A draft already open keeps its fields and appends the imported text.

The desktop polls authenticated `/slack/desktop` through its local daemon every three seconds. Presence expires after 15 seconds without a poll. Each import stays on its chosen installation and is retried until the app acknowledges prefill, or the ten-minute import lifetime expires. Other accounts cannot receive or acknowledge it. No session credential is exposed to the app. A service restart clears presence and pending imports; the desktop registers again on its next poll.

If no desktop is reachable, a private Slack modal explains how to reconnect and offers the existing message/thread HTTPS links, whose pages open `chauffeur://slack/<token>?scope=message|thread`. Older desktops use this fallback. Both the service image and desktop build must be updated for direct delivery.

The selected message is supplied by Slack itself: the bot need not be a member
of that channel. Threads use `conversations.replies` with pagination and require
the bot's history scopes and access to the conversation. If Slack refuses a
thread, rate limits it, returns incomplete pagination or it exceeds the import
limit, the selected message is imported with a visible warning. Attachments are
links to Slack, not downloaded files. Authors retain their Slack user IDs when
Slack does not supply a display name.

## Configure your Slack app

Create a Slack app, or open an existing one at https://api.slack.com/apps. If it
already has settings, do not replace its entire manifest: preserve its incoming
webhooks and other settings. `manifest.json` describes the additions:

1. **Interactivity & Shortcuts**: enable interactivity with Request URL
   `https://<service-host>/slack/actions`. Add an **On messages** shortcut
   (its name as in `manifest.json`), callback ID `chauffeur_open`.
2. **OAuth & Permissions**: add bot scope `commands`. For threads, add
   `channels:history`, `groups:history` (the approved channel scopes),
   then reinstall the app into the same workspace. Invite the bot to channels
   whose threads should be imported. A message alone requires none of the
   history scopes. Slack workspace policy may restrict shortcuts in some
   conversations; this does not bypass it.
3. Provision the service with that app's **Signing Secret**, **Bot User OAuth
   Token**, and workspace ID. Keep these in the host's secret store/root-only
   environment file, never in git, chat, URLs or logs. `SLACK_TEAM_ID` must name
   the same workspace as the bot token. An existing interactivity handler must
   route `chauffeur_open` here rather than be overwritten.

4. **OAuth & Permissions → Redirect URLs**: add exactly
   `https://<service-host>/slack/auth/callback` and save. This is the SSO
   callback, separate from the interactivity URL. Sign-in requests only
   `openid profile`; do not mix these with bot scopes in one OAuth grant.
5. Add `SLACK_CLIENT_ID` and `SLACK_CLIENT_SECRET` from Basic Information to the
   server's root-only environment file. This one-time administrator step is not
   shown to colleagues. Never paste the client secret into a conversation.

## Host the service

This is a separate process from the opaque mobile relay: Slack context is not
end-to-end encrypted. It can share the share service's HTTPS origin, as below.
The desktop only reads from the origin `[slack] url` names, refuses redirects and arbitrary
URLs, and limits response size. Only an opaque Chauffeur session reaches the daemon, which keeps it in its secret store. Slack tokens stay on the server.

Build on the host with `docker build -t chauffeur-slack slack/`. Create
`/etc/chauffeur-slack.env` (mode 0600) containing:

```dotenv
SLACK_SIGNING_SECRET=...
SLACK_BOT_TOKEN=...
SLACK_TEAM_ID=T...
SLACK_CLIENT_ID=...
SLACK_CLIENT_SECRET=...
SLACK_DATABASE=/data/slack.sqlite
SLACK_PUBLIC_URL=https://<service-host>
PORT=8091
```

Run the service with the supplied `deploy/chauffeur-slack.service`, then add
this route **before** the existing design share proxy in the domain's Caddy
block (do not replace the rest of that configuration):

```caddy
handle /slack/* {
    reverse_proxy 127.0.0.1:8091
}
handle {
    reverse_proxy 127.0.0.1:8090
}
```

Validate Caddy before reloading it. `GET /slack/health` must return `ok`.
The relay VM's startup script writes its Caddyfile on reboot: its share hosts'
block, and its former share host's, also include this optional route, so the
service answers on each of them with the same sessions (`SLACK_PUBLIC_URL`
names the one Slack's callbacks use); but the Slack service/image and secret
file must be installed separately; the existing share/relay deploy does not
provision Slack secrets. No production deployment is performed by tests.

After initial deployment, each colleague opens **Chauffeur → Settings → Slack**,
signs in with Slack, authorizes their Slack identity in the browser, and
returns to Chauffeur. The section displays their name and a sign-out button.
No bot token, signing secret or server access is needed on a colleague's Mac.

The service unit creates a private `/var/lib/chauffeur-slack` directory and mounts
it at `/data` in the otherwise read-only container. Sessions survive restarts;
only their hashes, Slack IDs, display names and expiry are persisted. Sign-out
revokes the session before the daemon deletes its local credential. Sign-in
flows and Slack context remain ephemeral. Back up the session database only if
preserving sessions is needed; losing it merely requires signing in again.

Release/install the desktop build too: macOS registers the scheme when its app
bundle is installed; Linux registers it on startup. Previous desktop versions
cannot receive these links. The browser may ask permission to open Chauffeur.

## Retention and tests

The service checks Slack HMAC signatures and a five-minute timestamp window,
pins the Slack workspace, deduplicates retries, and keeps at most 128 imports
in memory for ten minutes. Links use 256-bit random capabilities and require the authenticated Slack
account that invoked the shortcut. A link alone cannot read context. They are offered only in the
requesting user's modal. They can be retried until expiry; opening a landing
page never consumes an import. Restarting the service expires every link.
Context is not written to disk and request URLs/bodies must not be access-logged
by the reverse proxy. Responses are no-store/no-referrer and expose no CORS.

```sh
cd slack
bun install --frozen-lockfile
bun run typecheck
bun test
```

Tests use synthetic Slack payloads and a stub API; no messages are sent to Slack.
For acceptance, use the shortcut on an ordinary message, a parent and a reply,
an inaccessible thread, and an expired link. Check an app cold start and an
already-open create form. Merely opening the form must leave workspace count
unchanged; Escape cancels it.

SSO uses `openid-client` with ID-token signature verification, issuer/audience/
nonce validation, OAuth state and PKCE. The daemon proves ownership of its flow
with an independent random verifier. Browser URLs never contain the desktop
session credential. Flows expire after ten minutes; sessions after 30 days.
Tests use local keys and a fake Slack exchange, and cover wrong workspace,
wrong nonce, wrong desktop proof, cancellation, replay, expiry, persistence,
revocation, and imports belonging to another account. Live Slack authorization
still needs the administrator callback/client-secret setup above.
