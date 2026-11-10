# Features relying on a distribution's hosted services are for its members only

- **Date:** 2026-11-06
- **Status:** accepted

## Context

Chauffeur opens to people outside Dialog: a private beta installs Dialog's build. Some features rely on services Dialog hosts for its own team (the Office and its EOW board, the Slack service, the phone relay); an outside user would meet them broken. Sharing, which anyone can use, needs a way in for people who never open a share dialog, and the team option of a share needs a real domain.

## Decision

Decided with Justin 2026-11-06:

- A new Core setting `[distribution] member_domain`, empty in Core. A **member** is a user signed in to Chauffeur with a Google account of that domain; the daemon decides it at runtime from the signed-in identity's e-mail and tells the clients, live on sign-in and sign-out, with no restart.
- Outside members (signed out, another domain, or no `member_domain` set): the Office is hidden everywhere (sidebar, Settings › Extensions, menus, its shortcut: no trace), and its EOW board with it; Settings › Slack is hidden; Settings › Mobile shows only "Phone access is coming soon." and the menu's Connect Phone… item is gone. Linear, the user's own cloud workers and local models stay. Members see everything as before.
- Sign-in entry points: Settings › Account (signed out: "Sign in with Google" and "Sign in to share designs and workspaces."; signed in: "Signed in as …" and "Sign out"), and a one-time banner at the bottom of the sidebar ("Sign in with Google to share designs and workspaces", "Sign in" / "Not now"; once dismissed or signed in, never again). The share dialogs keep their sign-in button. No visible text names Dialog or a members-only feature.
- A share's team option reads "Anyone at <domain>", the signed-in account's Google Workspace domain (its ID token's `hd`), and shows only when the account has one; otherwise it is not shown at all.
- The About panel no longer links Chauffeur's private repository.

## Consequences

- The daemon keeps the account's team domain at sign-in; for a sign-in made before, it reads it once from a fresh ID token at its start.
- `identity.state` and the `identity_changed` registry event carry `member`, `team_domain` and `can_sign_in`; the app hides what is a member's, never greys it.
- Hiding is the app's: a non-member's daemon still runs an Office enabled in its settings. Dialog's distribution sets `member_domain = "askdialog.com"`.
