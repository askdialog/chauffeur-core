# chauffeur-design-share

The share service. A design share (docs/SPEC.md, story 51) is the hosted, live copy of a design chat's canvas at `<public-url>/s/<id>`, opened to people named by e-mail, whatever their address, and, when its owner's Google account is a Google Workspace account, to that account's domain (its team domain). See docs/adr/share-with-any-email.md.

- The owner's daemon creates a share, uploads the design's files and keeps one WebSocket per shared chat for the comments and the reloads. Any Google account can own a share.
- A reader signs in on the page with Google (any Google account whose verified e-mail is invited, or one of the share's team domain) or, without a Google account of their address, with a sign-in link they ask for on the page: e-mailed to an invited address only, valid 15 minutes, used once. Either way they get a session cookie of 30 days, read the files and the comments, and keep one WebSocket for what changes; their comments go back to the daemon's `comments.json`.
- Sandboxed screens load their relative scripts, styles, images and fonts through opaque read-only URLs. Each URL belongs to one share and reviewer, expires after 15 minutes without renewal by the signed-in page, and checks current access on every request. The page renews it every five minutes and on return from the background; the cookie stays `HttpOnly; Secure; SameSite=Lax`, and screens keep their opaque origin.
- Shares, reviewers and comments live in SQLite; files on disk; both under the `--data` folder (`/var/lib/chauffeur-design-share` in the GCP deploy).
- With `--cookie-domain` (a parent of its hosts), a session made on a host under it is the whole domain's: one sign-in serves the design and workspace hosts. On any other host the session stays the host's own (docs/adr/share-hosts-per-kind.md).

Comments mention people (docs/adr/mentions-in-share-comments.md, `src/mentions.rs`):

- Every reader of a design share sees who it is open to: `people` (the owner, then the reviewers) in the snapshot, the share's status and `People` on the page's socket whenever the reviewers change or someone's name becomes known. A name is the one a Google sign-in gave (kept in the `people` table), else the e-mail's first word; a sign-in link never overwrites one.
- A comment's `mentions` (from a page's `Add` or `Edit`, or the daemon's `Comments` whole) keep only the owner, the reviewers and addresses of the share's team domain, never the comment's author, each once, 20 at most, named as the service knows them. When an older daemon sends a comment without the field and its text has not changed, the mentions the service had stay.
- The `mentions` table follows the comments: one row per (comment, person), unread until its person opens the thread (`Seen { thread }` from the page, or `Seen { id }` from their daemon); a mention edited out, its comment removed, its person taken off the share or the share stopped, and it goes. The snapshot lists the reader's unread ones (`unread_mentions`, comment ids).
- A signed-in daemon keeps one WebSocket for its user's mentions, `/v1/mentions/daemon` (`Hello` with its Google ID token in the share protocol's version, then `Seen { id }`); the service sends `Mentions` whole (every unread one, then the latest read, 50 at most, newest first; only shares still open to the user; each with the page's link to its thread, `…/s/<id>?thread=<thread>`) at once and after each change of theirs.

It also hosts workspace shares (story 53) at `<workspace-public-url>/w/<id>` (`<public-url>` when not set): a workspace's chats, live, and the markdown of its notes folder, for guests who are viewers or may suggest messages to its agents.

- The owner's daemon creates a share (`POST /v1/workspace-shares`, guests with their role) and keeps one WebSocket per shared workspace (`/v1/workspace-shares/{id}/daemon`): the chats' list, each chat's stored events (whole, then as they come; text being written only passes through), the notes, and the suggestions' fates go up; guests' suggestions and who is looking come down.
- A guest's page reads `/v1/workspace-shares/{id}/snapshot` and a chat's events (`…/chats/{chat}/events`), and keeps one WebSocket (`…/guest`) for what changes; a guest who may suggest sends a suggestion over it, which the owner sends to the agent, edits or dismisses from the app.
- A guest who may suggest comments on a passage of an agent's message, or replies to a comment, over the same socket; the owner does from their daemon (`POST /v1/workspace-shares/{id}/comments`, `DELETE …/comments/{comment}`). The comments go whole to every page and to the daemon. Their mentions work as a design's (above), in the `workspace_mentions` table: among the owner and the guests, viewers too; the link is `…/w/<id>?chat=<chat>&comment=<thread>`. A daemon older than replies (protocol revision 2) gets the threads' first comments only.
- The chats' events and the notes are kept in SQLite, in clear, like a design's files; stopping the share removes them at once.

People a share names get an e-mail with its link (`src/mail.rs`), at its creation and when an update adds them, sent through Amazon SES from `--mail-from-name <--mail-from>` (`SHARE_MAIL_FROM_NAME`, `Chauffeur` by default, and `SHARE_MAIL_FROM`), naming the owner, replies going to the owner (`src/ses.rs`); never to the owner nor to the whole team, never with a sign-in link in it. Every e-mail ends with "Not expecting this? Stop these emails", a link that blocks every later e-mail to the address, and "Shared with Chauffeur", linking to `--site-url` (`SHARE_SITE_URL`) when set. An owner outside the members' domain (`--member-domain`, `SHARE_DOMAIN`) names at most 20 people a share and sends at most 50 invitations a day; an address gets at most 5 sign-in links an hour. What a limit or a blocked address holds back is not sent, the share is saved, and the owner's daemon hears why (`notice`). A failed e-mail is logged and never fails the share. Without `--mail-from`, or without `--mail-role-arn` (`SHARE_MAIL_ROLE_ARN`), nothing is sent, as in development.

## Running it

```
chauffeur-design-share --listen 127.0.0.1:8090 --data /var/lib/chauffeur-design-share \
  --google-daemon-client-id <the Office's desktop client> --google-web-client-id <the page's web client> \
  --web app/dist-share --public-url https://<design-host> [--workspace-public-url https://<share-host>] \
  [--cookie-domain <parent-of-both-hosts>] [--member-domain <your-domain>] \
  [--site-url https://<your-site>] [--session-key <64 hex digits>] \
  [--mail-from chauffeur@<your-domain> --mail-role-arn <role-arn>]
```

`--public-url` (the address design links use, and the links that stop e-mails) is required; `--workspace-public-url` (`SHARE_WORKSPACE_PUBLIC_URL`) is the address of workspace links, the public URL by default; `--cookie-domain` (`SHARE_COOKIE_DOMAIN`) scopes sessions to a parent domain of both. Nothing in the service names a default domain. `--member-domain` (`SHARE_DOMAIN`, formerly `--domain`, which still works) names the members: owners without the limits on invitations; at start, the team shares made before a share named its team domain get it. `--session-key` (`SHARE_SESSION_KEY`, `openssl rand -hex 32`) signs sessions and the links that stop e-mails; without it each start draws one and signs every reader out.

In development, `--insecure-dev` on loopback trusts `dev:<email>` as a token (the daemon's bearer, the page's `POST /v1/session`), with no Google: the account is of the Google Workspace of its e-mail's domain, but at `gmail.com`, a personal account. The page then shows an e-mail box in place of Google's button.

The reviewer page is the app's canvas in a build of its own: `bun run build:share` in `app/` writes `app/dist-share` (`index.html`, `assets/`), which `--web` serves; `bun run dev:share` serves it from Vite on port 1421, proxying `/v1` to a service on `127.0.0.1:8090` (`SHARE_SERVICE` for another address).

## Deploying it

```
GCP_PROJECT=<project> GCP_REGION=<region> [SHARE_DOMAIN=<members' domain>] [SHARE_SITE_URL=https://<your-site>] [SHARE_COOKIE_DOMAIN=<parent-of-both-hosts>] [SHARE_REDIRECT_HOST=<former-host>] SHARE_GOOGLE_DAEMON_CLIENT_ID=… SHARE_GOOGLE_WEB_CLIENT_ID=… bash crates/chauffeur-design-share/deploy/gcp.sh <design-host> [<share-host>]
```

Design shares are served on `<design-host>`, workspace shares on `<share-host>` (`<design-host>` when not given), both by the same container; `SHARE_DESIGN_HOST` and `SHARE_WORKSPACE_HOST` may name them instead of the arguments. `SHARE_COOKIE_DOMAIN`, a parent of both hosts, makes one sign-in serve both. `SHARE_REDIRECT_HOST` is a host the service had before: Caddy redirects its pages (308, path and query kept: `/w/…` to `<share-host>`, the rest to `<design-host>`) and still proxies `/v1/…` and `/slack/…` there, for older daemons and the Slack app.

The script gathers this crate, `chauffeur-design` (the canvas read from a design folder, shared with the daemon), `chauffeur-protocol` and the page's build (`app/dist-share`, when built), has Cloud Build build the image in the project's region (a few minutes on eight cores) into its Artifact Registry (`<region>-docker.pkg.dev/<project>/<repository>`), names it by digest in the relay VM's `chauffeur-share-*` metadata and runs the VM's startup script again over IAP SSH: `relay/deploy/startup.sh` pulls the image and runs the container beside the relay, Caddy serving each host with a Let's Encrypt certificate of its own. The VM is not restarted; Caddy restarts for a moment, and the Macs' bridges reconnect on their own. The first deploy draws the sessions' key into Secret Manager (`chauffeur-share-session-key`, readable by the VM's service account only), which the startup script hands the service at each start: a deploy signs no one out. Deploying needs IAP SSH to the VM (OS Login) and Cloud Build in the project. The script targets one GCP project and region (`<project>`, `<region>`): set them to yours before running it.

Invitations need, once, the SES identity and the IAM role the VM's service account assumes with its Google ID token (no AWS key is stored), then the three DKIM CNAMEs it prints in your domain's DNS:

```
AWS_PROFILE=<your-aws-profile> AWS_ACCOUNT=<account-id> GCP_PROJECT=<project> \
  SHARE_DOMAIN=<your-domain> SHARE_MAIL_FROM=noreply@<your-domain or a subdomain of it> \
  bash crates/chauffeur-design-share/deploy/aws-mail.sh
```

The script verifies the sender's domain in SES and creates a role that may send from `SHARE_MAIL_FROM` only, then prints the role's ARN. A subdomain verified alone keeps SES from signing for the rest of your domain. Invitations go to any address, which SES's sandbox refuses: the AWS account needs SES's production access.

`gcp.sh` (with `GCP_PROJECT`, `GCP_REGION`, the two OAuth client ids, optionally `SHARE_DOMAIN` and `SHARE_SITE_URL` and, for e-mails, `SHARE_MAIL_ROLE_ARN`, `SHARE_MAIL_FROM` and `SHARE_MAIL_FROM_NAME`) passes them to the service through the relay VM's metadata; without the role or the sender it sends nothing.

Before the first deploy: an A record for each host on the VM's address, and a web OAuth client in the Google Cloud project of the daemon's client (type Web application, authorized JavaScript origins `https://<design-host>` and `https://<share-host>`, no redirect URI: the page uses Google Identity Services). Each Mac's daemon calls `[design_share] url = "https://<design-host>"`, and takes the links of the other hosts with `link_urls = ["https://<share-host>", "https://<former-host>"]`. The daemon's tokens are the Office's desktop client's. For accounts outside your Google Workspace to sign in, the project's OAuth consent screen is External and published.

### Native reviewer session handoff

The phone renders the existing hosted reviewer page inside its WebView. Google and
email authentication run in the system browser; no Google authentication is embedded.
A paired Mac is not involved. The authorization code is bound to one share, but
its exchange creates the existing **30-day identity session cookie**, not a
share-scoped session. That identity can access other shares permitted by their
current ACL, and configured cookie-domain rules still apply. This narrow code
exchange is not an OAuth server.

1. Native generates a random PKCE verifier (43–128 RFC 7636 unreserved characters),
   its S256 base64url challenge without padding, and a separate random state
   (32–128 unreserved characters), kept in memory. It opens the approved service
   origin at `/s/{id}?mobile-auth=1&challenge=...&state=...&screen=...&thread=...`.
2. After existing Google/email cookie authentication, the page makes
   `POST /v1/shares/{id}/mobile-authorize` with JSON
   `{challenge,state,screen?,thread?}` and `X-Chauffeur-Mobile: 1`.
   The browser's `Origin` must exactly match the configured `public_url` origin;
   CORS is not enabled. A cookie session and current share ACL are required.
3. The response is `{callback:"chauffeur://review/auth?code=...&state=..."}`.
   This fixed callback contains a random code, never a session bearer or cookie.
   Native validates its pending state before using its verifier. This custom-scheme
   callback is fixed but **not a verified app-claimed association**: a different
   installed app may register the scheme. PKCE prevents an interceptor from redeeming
   this app's code without its verifier; it does not prove that an authorization
   request was created by Chauffeur. The authenticated browser must require explicit
   consent naming the account and design before authorizing a native handoff.
4. Native loads the approved same-share document with `mobile=1&native=1` and
   injects the proof once in its main frame. That trusted document uses same-origin
   `fetch` to POST an `application/x-www-form-urlencoded` body `{code,verifier,state}`
   to `/v1/shares/{id}/mobile-session`, with `X-Chauffeur-Mobile: 1`; the browser
   supplies its `Origin`. A source POST cannot do this on Android because WebView
   ignores custom headers for POST. No native messaging bridge is installed.
   A successful exchange sets the existing HttpOnly/SameSite cookie and returns
   HTTP 303 to the stored relative `/s/{id}?mobile=1&native=1&screen=...&thread=...` target.
   Fetch follows that redirect; the document verifies the final origin, share,
   native flags and exact screen/thread before navigating there.
   No caller-supplied redirect URL is accepted. Missing screen/thread targets stay
   intact so the page can explain that they are missing rather than silently jump.

Codes expire after 60 seconds, are single use, are bound to share, identity, PKCE
challenge, state and original target, and are lost on service restart. Invalid
verifier/state/share attempts do not consume them. A successful proof consumes the
code before rechecking ACL/stopped status, so revoked access never creates a cookie.
The in-memory outstanding-code map is bounded; a multi-instance deployment needs
session affinity for this short exchange or a shared atomic expiring code store.

`POST /v1/shares/{id}/sign-in-link` additionally accepts optional `screen`, `thread`
and `mobile_auth: {challenge,state,screen?,thread?}`. Email links preserve these
validated query parameters before the existing one-time `#sign-in` token. Top-level
targets, if included alongside `mobile_auth`, must match its targets. This extension
does not change the protocol crate or permit a redirect to another origin.

Tests cover real local HTTP cookie handoff, exact target preservation, fixed callback,
CSRF/missing or foreign Origin, absent browser session, unauthorized/revoked/stopped
shares, wrong verifier/state/share, single use, expiration and email continuation.
Production deployment, system-browser callback behavior and native iPhone/Android
WebView POST/cookie/deep-link behavior remain deployment/device validation steps;
local tests do not establish that Universal/App Links are deployed or verified.

The native review route is singular in Expo's root stack and retains one WebView
across sign-in callbacks, error overlays and retries. Main-frame navigation within
that instance preserves its sessionStorage drafts; no private review draft is copied
to native disk or localStorage. Process termination still loses this session-only
state. Physical validation must confirm actual route/cookie/storage behavior.
