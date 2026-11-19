# A repo declares the logins its work needs; Chauffeur checks them and renews one on approval

- **Date:** 2026-11-18
- **Status:** accepted

## Context

An agent's work often needs a login outside its own: an AWS SSO session, an internal CLI's session, a cloud provider's CLI. Each expires on its own schedule and fails its own way: one tool says "token expired", another a raw stack trace, a third nothing until a command breaks halfway. Every repo's instructions then tell the agent something different (run the login yourself, ask the user, give up), and the user learns that a login ran out from the middle of a chat. A routine that runs at night cannot even ask. Asked by Justin 2026-11-18: one uniform way, approved in a click from the Mac or the phone, without lengthening any session and without Chauffeur holding anyone's credentials. Core is used outside Dialog, so nothing of it may assume AWS or any given tool.

## Decision

Decided with Justin 2026-11-18:

- **Declared, never guessed.** A repo declares the sessions its work needs in its committed `.chauffeur/settings.toml`, one `[sessions.<name>]` table each: a `label` the user reads, a `check` command (exit 0: the session holds) and a `login` command that renews it. The user may declare or override one in `~/.chauffeur/settings.toml` (`[sessions.<name>]`, or `[repos.<name>.sessions.<name>]` for one repo), like run scripts. A workspace's sessions are those of its repos. Nothing declared, nothing shown: Chauffeur knows no tool by name.

  ```toml
  [sessions.dialog-staging]
  label = "Dialog CLI (staging)"
  check = "dialog status --env staging"
  login = "dialog login --env staging"
  login_remote = "dialog login --env staging --no-browser"
  ```

- **Chauffeur runs commands, never reads credentials.** The daemon runs `check` and `login` in the workspace's environment, as it runs scripts, and reads only their exit status and, from `check`, an optional last line `expires_at=<RFC 3339>`. It stores no token, reads no credential file, and copies nothing anywhere (the vault removed by [credential-vault-removed](credential-vault-removed.md) stays removed). Session lengths are the tools' own.
- **One door for agents and scripts.** `chauffeur session ensure <name>` runs `check`; on success it exits 0 at once. Otherwise it puts a request to the user in the chat that ran it, "Renew Dialog CLI (staging)?", answered like a permission request from the app, the CLI or the phone ([0003](0003-questions-and-permissions.md); a child's reaches its parent, [child-questions-reach-the-parent](child-questions-reach-the-parent.md)). Approved, the daemon runs `login`; once `check` passes, `ensure` exits 0. Declined, or no answer within 15 minutes, it exits nonzero with the reason, and the agent stops what needed the session rather than working around it. `chauffeur session ls|check <name>|login <name>` let the user see and renew them directly. An agent cannot approve its own request.
- **A login runs where the user approves it.** A session may declare a second login, `login_remote`, for an approval from another device than the Mac (decided with Justin 2026-11-19). Log in from the Mac's app or CLI runs `login`, which may open the Mac's browser and need no code (a browser flow that answers to `127.0.0.1`). Log in from the phone runs `login_remote`, which prints an `https://` URL and, when the page asks for it, a code (a device code, `--no-browser`): the phone shows what it printed and opens the URL in its own browser, so the sign-in happens on the phone while the Mac only has to be awake. A session without `login_remote` is approved from the Mac only; the phone says so. A link `login` prints is opened on the Mac, and what it printed is shown there, a code included. Neither the URL nor the code is ever logged nor put in a push.
- **Shown where it matters.** A workspace's sessions show beside its run scripts: label, holds or not, and when it ends if `check` says so. A failed `check` is "expired", never an error of the chat.
- **Notifications are opt-in.** A session request notifies on the Mac and pushes to the phone only if the user turned that on, per channel, in Settings › Notifications and the phone's push preferences (a new kind, `sessions`, off by default). The push text is generic ("A login needs renewing"), with the workspace's name only under the existing preference; tapping it opens the workspace where the request waits. A notification never approves anything by itself.
- **Routines check first.** A routine may list the sessions it needs (`sessions = ["dialog-staging"]`). Before a run, the daemon runs their `check`; one that fails does not start the run: the routine records why, and a session request waits in its workspace (notified as above). The run starts once the session holds again, as a missed occurrence is caught up ([routines](routines.md)).

## Consequences

- `.chauffeur/settings.toml` gains `[sessions.*]`, which Conductor's format does not know; `~/.chauffeur/settings.toml` gains the user's declarations and overrides. The settings window lists the workspace's sessions but does not edit them yet.
- The daemon owns the check, the request and the login; the protocol gains session methods (`session.list`, `session.check`, `session.ensure`, `session.login`) and a request kind for renewing one, relayed to the phone like `chat.respond`. Agents need to know their chat: the daemon gives them `CHAUFFEUR_CHAT_ID` beside `CHAUFFEUR_WORKSPACE_ID`. `ensure` run from a terminal or a script, with no chat, puts the request on the workspace's first chat waiting for nothing.
- A `check` must test the session itself, not credentials derived from it and cached (the AWS CLI answers `sts get-caller-identity` from a role's cached keys after the SSO session ended). Each repo writes its own; Chauffeur runs `check` at most once a minute per session and workspace, and on demand.
- `login` runs with no terminal: it must not wait for keyboard input. One that does is stopped after 10 minutes, and the request says so.
- The mobile companion gains the request's card (log in, decline, the printed code, the link opened in the phone's browser) and the `sessions` push preference; the relay gains the push kind. The phone may call `session.login` for `login_remote` only.
- Dialog's repos declare their sessions (AWS SSO per profile, the Dialog CLI per environment) in their own `.chauffeur/settings.toml`, and their agent instructions say to run `chauffeur session ensure` before work that needs one. Nothing of this is in Core.
- Not done yet: waking the Mac for a routine, renewing a session before it ends, and sessions a user shares with colleagues.
