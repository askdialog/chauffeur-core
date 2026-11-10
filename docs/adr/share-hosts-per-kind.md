# Design and workspace shares on hosts of their own, one sign-in for both

- **Date:** 2026-11-06
- **Status:** accepted

## Context

The share service answered on one public URL (`--public-url`) for both kinds of share: Dialog's design and workspace links lived on `design.askdialog.ai`, a host outside Chauffeur's own domain. Justin wanted the links on Chauffeur's domains, a host per kind, without a second service or VM, and without breaking the links already sent.

## Decision

Decided with Justin 2026-11-06:

- Design shares live at `https://design.chauffeur.askdialog.com/s/<id>` and workspace shares at `https://share.chauffeur.askdialog.com/w/<id>` in Dialog's deploy. One service on one VM answers on both hosts; it writes design links (the API's answers, invitations, sign-in links) with the design host and workspace links (the app, the daemon, invitations, sign-in links) with the workspace host.
- The hosts are configuration; Core names no domain of its own. The service takes `--public-url` (design shares, and the links that stop e-mails) and an optional `--workspace-public-url` (`SHARE_WORKSPACE_PUBLIC_URL`), defaulting to the public URL.
- One sign-in serves both hosts: with `--cookie-domain` (`SHARE_COOKIE_DOMAIN`, `chauffeur.askdialog.com` for Dialog), a session made on a host of that domain is scoped to the domain. It stays `HttpOnly`, `Secure`, `SameSite=Lax`, 30 days. Google sign-in and sign-in links work on both hosts.
- Old links keep working. The former host (`design.askdialog.ai`) only redirects, permanently (308) and keeping the path and query: `/s/…` to the design host, `/w/…` to the workspace host, anything else to the design host. It still serves the API (`/v1/…`, WebSockets included) and Slack's `/slack/…` as before, since older daemons and the Slack app call them there and a redirect would break them.
- The daemon and the app accept share links on the old and new hosts: design links, workspace links (`chauffeur new --workspace-share`, "Import a shared workspace…", the page's "Import into Chauffeur") and imports.
- Caddy on the relay VM gets the three hosts' certificates once their DNS points to it; `deploy/gcp.sh` takes the hosts as arguments and environment and keeps the former host redirect-only.

## Consequences

- `deploy/gcp.sh <design-host> [<workspace-host>]` (or `SHARE_DESIGN_HOST`, `SHARE_WORKSPACE_HOST`), with `SHARE_COOKIE_DOMAIN` (a parent of both hosts, checked) and `SHARE_REDIRECT_HOST` (the former host), writes them to the VM's metadata; `relay/deploy/startup.sh` hands them to the service and writes the Caddy blocks.
- The service scopes the cookie to the domain only when the request's `Host` is under it; on any other host (the former one, loopback) the session stays the host's own. Signing out clears both forms. Every host under the cookie domain receives the session cookie: the operator keeps that domain to hosts it runs.
- The daemon still calls only `[design_share] url`. A new `[design_share] link_urls` lists the service's other addresses whose links it takes as its own (its workspace host, its former host); a workspace link elsewhere is still refused. Design links were never checked against a host.
- A distribution ships both: Dialog's defaults set `url` to the design host and `link_urls` to the workspace host and `design.askdialog.ai`, and its `[slack] url` moves to the design host (one Slack service, reached on every host).
- Shares made before keep the link stored with them, on the former host, which redirects.
- By hand, once: DNS records for the two new hosts, and the web OAuth client's authorized JavaScript origins for both.
