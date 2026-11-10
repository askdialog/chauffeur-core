# 0052. Prefill a new local workspace from a Slack message

- **Date:** 2026-10-29
- **Status:** superseded by [slack-direct-desktop](slack-direct-desktop.md)

## Context

A user asked to create a Chauffeur session from a Slack message. Justin clarified
that this should start from any channel's message menu, using the team's existing
Slack app, and open their local Chauffeur.

## Decision

Decided with Justin 2026-10-29: a message's "Open in Chauffeur" shortcut
preopens the existing new-workspace form, equivalent to ⌘N with the selected
message or Slack thread copied into its first message. The user can edit the
context and choose repos/model before submitting. Neither the workspace nor
an agent turn starts automatically. The user does not go to the bot's chat.

## Consequences

The desktop handles a `chauffeur://slack/…` link and brings its main window to
the front. If a form is already open, its fields and text are preserved and the
context is appended. A short-lived service bridges Slack's message shortcut to
the local form; it never calls the daemon to create workspaces or send messages.
The source permalink accompanies the text. Importing a full thread requires
Slack API access to its conversation; failure imports only the selected message
with an explicit warning. Attachments stay links to Slack.

The existing Slack app needs an interactivity endpoint and `commands` scope;
thread imports also need history scopes. The service holds bounded context in
memory for ten minutes, behind unguessable bearer links offered in a private
modal. It is separate from the opaque mobile relay. Deployment and Slack app
configuration are required in addition to installing the new desktop build;
see [the integration guide](../../slack/README.md).

## Individual Slack sign-in (2026-10-30)

Justin clarified that in-app configuration means each colleague signs in with
Slack from Settings → Slack. The browser grants `openid profile` separately
from the existing bot installation; no colleague enters bot or signing secrets.
The server validates Slack's OIDC signature, issuer, audience, nonce and the team's
Slack workspace. State and PKCE bind the browser exchange; a separate desktop proof
binds polling to the initiating daemon. The local session credential stays in
the system secret store, never in frontend state or agent transcripts.

Sessions last 30 days, are hashed in a private persistent server database, and
are revoked on sign-out. Pending logins expire after ten minutes. The selected
message can only be fetched by the Slack user who invoked its shortcut. A user
who is not signed in is sent to Settings → Slack and reopens the message after
connecting. This SSO identifies the user; thread access still uses the installed
bot and its existing channel permissions. Client secret and callback URL are
one-time server/app setup by an administrator, not per-colleague configuration.
