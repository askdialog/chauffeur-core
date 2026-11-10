# Shares open to any e-mail address, with Google or a sign-in link

- **Date:** 2026-11-06
- **Status:** accepted

## Context

Shares were for one Google Workspace: the service let in only accounts of its domain (`--domain`, `hd` checked), wrote only to its addresses, and "the team" was that Workspace ([0029](0029-design-comments-and-shares.md), [0038](0038-workspace-shares.md), [share-invitation-emails](share-invitation-emails.md)). For the private beta, people outside Dialog install Chauffeur: they must share a design or a workspace with any e-mail address, and whoever receives it must open it, with or without a Google account.

## Decision

Decided with Justin 2026-11-06:

- **Invitees.** Any e-mail address can be an invitee, on design shares and workspace shares. Invitations are e-mailed to every invitee, whatever the domain (not the owner, only newly added ones, as before).
- **Signing in.** An invitee proves who they are by either:
  - Google sign-in: any Google account; the account's verified e-mail must equal an invitee address (case-insensitive). No `hd` requirement anymore.
  - A sign-in link: asked for on the share's page ("Email me a sign-in link"), sent by SES to the address, valid 15 minutes, single use, sent only if the address is an invitee of that share; the page answers the same message either way ("If this address was invited, a link is on its way"). Never a sign-in link in the invitation e-mail itself.
- **Owners.** Any Google account can create design and workspace shares. The app's Google sign-in no longer restricts to a domain (Dialog's `google_hosted_domain` becomes empty).
- **Team domain.** A share can be opened to a team domain only when the owner's Google account is a Google Workspace account (its ID token has `hd`); the team domain is that `hd`, stored on the share when it is opened. A viewer enters through the team only if their own Google ID token carries the same `hd`. Sign-in-link sessions have no `hd`: named invitees only. Existing team shares get `askdialog.com`.
- **Label in the app.** "Anyone at <domain>" (the real domain), shown only when the signed-in account has an `hd`; otherwise not shown at all.
- **Sessions.** 30 days, signed with a stable key kept in GCP Secret Manager and read at start: deploys no longer sign everyone out.
- **Anti-abuse.** Owners who are not members: at most 50 invitation e-mails a day per owner, at most 20 invitees per share. Sign-in links: at most 5 an hour per address. Every e-mail has "Not expecting this? Stop these emails", which blocks further e-mails (invitations and links) to that address. Members (an owner whose e-mail is of the service's member domain) are not rate limited. Over a limit, the share is still saved, the e-mail is not sent, and the owner gets a clear error.
- **Invitation e-mail.** Subject "Alice shared “Pricing filters” with you"; the owner's name and the title, an "Open" button, the line "Sign in with the Google account of this address, or get a sign-in link by email", the footer "Not expecting this? Stop these emails" and "Shared with Chauffeur" linking to the site. The sender stays the operator's (`Chauffeur by Dialog <noreply@chauffeur.askdialog.com>` for Dialog), Reply-To the owner.
- **Share page, signed out**, in order: the logo and "Alice shared a design with you" (or a workspace), a primary "Continue with Google", a secondary "Email me a sign-in link" (an e-mail field, then "Check your inbox"), the footer "Shared with Chauffeur". Signed in without access: "You're signed in as bob@x.com, which doesn't have access." and "Use another account". No "request access".
- No enterprise SSO (Okta, SAML, OIDC).

This supersedes the domain-only parts of [0029](0029-design-comments-and-shares.md) and [0038](0038-workspace-shares.md) (reviewers and guests signed in with the Workspace's Google SSO; the team as every member of the Workspace) and the recipients of [share-invitation-emails](share-invitation-emails.md) (only addresses of the service's domain).

## Consequences

- The service's `--domain` (`SHARE_DOMAIN`) becomes its member domain, `--member-domain` (`--domain` still accepted): optional, it no longer gates who signs in; it lifts the limits for its owners, and at start it is the team domain given to the team shares made before (`askdialog.com` in Dialog's deploy, so existing team shares keep working). A service without one limits every owner and leaves earlier team shares closed. Core still names no domain of its own ([core-open-source-without-dialog-defaults](core-open-source-without-dialog-defaults.md)): the site link is `--site-url`, empty by default.
- The daemon's API does not change shape: the daemon still sends `team: bool`, and the service derives the team domain from the owner's ID token, refusing `team` with a sentence when it has no `hd`. Every refusal's sentence now reaches the owner as the request's error, and a share saved with invitations held back answers with a `notice` that the daemon returns as the request's error once the share is saved and published.
- A sign-in link is the share's page with `#sign-in=<token>`: a fragment, so no server log and no mail scanner's fetch sees or spends it; the service keeps the token's hash only. "Stop these emails" opens a page that asks before blocking, signed with the sessions' key so that no one blocks an address they did not receive at.
- Invitations go to any address: the SES account must leave the sandbox (production access). The Google OAuth consent screen of the project must be External and published for accounts outside its Workspace to sign in.
- `deploy/gcp.sh` draws the sessions' key once into Secret Manager (`chauffeur-share-session-key`), readable by the relay VM's service account only; `relay/deploy/startup.sh` hands it to the service at each start.
- Left for later: requesting access from the page, enterprise SSO, an owner's view of who stopped their e-mails.
