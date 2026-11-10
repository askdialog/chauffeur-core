# Piloting a moved workspace

A workspace that moved to a machine of Settings → Transfer runs there; this machine keeps a
read-only copy (`moved_to`). It is piloted from here all the same (decided with Justin
2026-10-26, `SPEC.md`): this daemon relays the calls about it to the daemon of the machine it
runs on, over the socket declared for that machine, and returns that daemon's answers. A
[GCP worker](cloud-workers.md) is such a machine too, reached on its forwarded socket
(`workers::machine_socket`, which `relay::machine_of` asks).

## What is relayed

`relay::Relay` sits in front of the daemon's own call handling (`server.rs`): a call is sent to
the other machine when

- its method is a chat's: `chat.create`, `chat.list`, `chat.send`, `chat.interrupt`,
  `chat.respond` (permissions, questions, MCP confirmations), `chat.stop_task`,
  `chat.context_usage`, `chat.subagent_messages`, `chat.queue_set`, `chat.queue_send`,
  `chat.configure`, `chat.rename`, `chat.move`, `chat.close`, `chat.reopen`, `chat.delete`,
  `chat.fork`, `chat.regenerate_brief`, `commands.list`, `events.history`, and the
  workspace's `workspace.seen`, `workspace.mark_unread` and `workspace.wait` (its chats run
  there, so does what they settle and leave unread);
- or what runs there and what its folders hold: `process.list|run|stop`,
  `terminal.open|list|input|resize|close|snapshot|move|rename|cwd`, `diff.get`,
  `files.list|read|write|search`, `pr.status|create|merge`, `worktree.add|remove`,
  `services.get|detect|save`, `previews.list` (its declared previews' states there, each
  given this machine's port that reaches it, with the machine's name), and `models.list` naming
  the workspace (the catalog of that machine's agents and logins, which the app's model picker
  shows inside it);
- and the workspace it names, its chat's workspace or its terminal's, moved to a machine of `[transfer]`
  (`moved_to` names it). A workspace moved by another daemon, or to a socket only the CLI
  named, is not relayed.

The other daemon knows the workspace and its chats by the same ids. A chat this machine has no
row of (created there) is found through the chats the other machine lists, kept once learnt; a
chat named by its workspace (`chauffeur send <workspace>`) is that workspace's; a terminal is
found the same way through the terminals it lists. Its answer, and its error with its code, are
returned as it gave them. A `files.read` or `files.write` answer's `preview_url` (an HTML
page's rendering, served on that machine's loopback, which this one cannot reach) is replaced by
this daemon's own preview URL: its preview listener (`canvas::file_handler`) reads the page, and
the files it loads by relative URLs, from that machine's daemon (`GET
/workspace-file/{workspace}?path=`, on its Unix socket only), and serves them under its own
policy, as a local page's.

Nothing local is bypassed: any other call about a moved workspace (add a repo, promote, rename
it) reaches this daemon's own code, which refuses it as before ("moved to another machine").
Archiving it brings it back first, its agents interrupted, then archives it here
([archive-a-moved-workspace](adr/archive-a-moved-workspace.md)). Opening one of its files with an app of this machine (`files.open`,
`files.reveal`) is refused: its files are there. A design chat of a moved workspace is refused:
it works against this machine's files.

The app's own HTTP routes for a workspace's files (`/attachments`, `/images`, `/media`: a
message's images, an image an answer shows, a video or PDF of the Files tab) read a moved
workspace's from that machine's daemon, over its socket, with the byte range asked.

A file a local client names by its path (the CLI's `--image`) is read here and sent as its
bytes, within the same 20 MB a message's file may have; the app already sends bytes. A call
that would not fit in one message of the connection (64 MiB, files encoded included) is
refused before it leaves, rather than closing the connection.

## Connection

One connection per machine socket, opened at the first call and kept: a WebSocket over the
Unix socket, as the transfer driver opens, with the 64 MiB message limit of the daemon's own,
several calls at a time. It says Hello with this daemon's protocol version. When it closes, the
calls waiting fail with "<machine> does not answer (…); its chats are back when it does", and
the next call opens it again. A call is never retried: a message sent twice would be sent twice.

## Events

A subscription (`events.subscribe`) naming moved workspaces or their chats is opened on the
machine they run on with the same filter (`relay_events::spawn_subscription`). Its events come
back as that daemon numbered them: `seq` and `after_seq` are that machine's, which a client
following a workspace keeps from the move on. To a subscription with `ephemeral`, this daemon
adds its own transfer progress of the workspace (`WorkspaceEvent::Transfer`, ephemeral), which
it drives. Such a subscription takes workspaces of one machine only: one mixing them with
others, or with `registry` or `office`, is refused. An `all` subscription naming nothing follows
this daemon's own events only: a moved workspace's are followed by naming it. It stops once the
workspace no longer runs there (brought back, checked every 5 s).

When the connection drops, an ephemeral `WorkspaceEvent::Relay { connected: false, error }`
says "vm does not answer (…)" once; the subscription opens again (1 s, doubling to 30 s), goes
on after the last event it passed, and says `connected: true`. One opened while the machine
does not answer is accepted all the same: it says so at once, then follows it once it is back
(a refusal from that machine, such as an unknown workspace, is returned as it is). Ephemeral events of the outage
(text deltas) are lost; stored ones are replayed. Brought back here meanwhile, it stops.

The sidebar: `relay_events::spawn_watch` follows the registry of each machine a workspace moved
to (checked every 5 s). At each connection, the chats each such workspace has there are
published here (those deleted there are removed here); then its chats' upserts and removals are
published as they come, and the workspace
shows the activity ("needs you"), unread mark and running tasks it has there instead of the
read-only copy's. Back here, its own show again.

## In the app

A moved workspace is used as a local one: its chats (send, interrupt, answer its questions and
permissions, new chat, rename, close, delete, fork: `tabActions`), its Terminal, Processes,
Diff and Files tabs, its pull requests ("Create PRs", merge). The model picker inside it shows
that machine's catalog (`state/catalog.tsx`). Stay off: design chats (they work against this
machine's files), opening it in an editor, "Show in Finder" and "Open in default app" on its
files, adding a repo. Archive brings it back, then archives it. While the machine does not answer (the last `relay` event), the
banner says "vm does not answer: the workspace is back once it does" and the chats are
read-only; the other tabs say the machine's error.

A workspace's session (`state/session.ts`) reads its history again from the start whenever it
moves or comes back (`moved_to` changes): the other machine numbers its events its own way.

## Ports

While a workspace runs on a machine of `[transfer]` (decided with Justin 2026-10-26),
`relay_ports` listens on 127.0.0.1 on the workspace's port block here and passes each connection
to the same port of its block there (`port_base + offset`; the two blocks may start, and be
sized, differently: an offset only one of them has is not reached), over that
machine's socket: `GET /tunnel/{workspace}/{offset}`, a WebSocket of the connection's bytes,
served on the Unix socket only, for ports of the workspace's block only. Its dev servers open in
this machine's browser at the ports the header shows, without declaring anything; nothing
listens beyond loopback. The listeners open within 5 s of the move and close within 5 s of its
return; a port something else holds here is skipped (logged). A connection to a port nothing
listens on there is closed at once, as a local one would be refused.

## Idle reminders

A moved workspace keeps its machine running, and paid for (decided with Justin 2026-10-26,
`SPEC.md`). `idle_reminders` keeps, in memory, when each moved workspace was last active:

- the user acting on it from here: any relayed call, from the app, the CLI or the phone, but
  those that only read it (`relay::READS`): `chat.list`, `chat.context_usage`,
  `chat.subagent_messages`, `commands.list`, `events.history`, `workspace.seen`,
  `workspace.wait` (a `chauffeur wait` included), `process.list`, `terminal.list`,
  `terminal.snapshot`, `terminal.resize`, `terminal.cwd`, `diff.get`, `files.list`, `files.read`,
  `files.search`, `pr.status`, `services.get`, `services.detect` and `models.list`;
- its agents there: a chat that works (its `activity`), or whose status or last stored event
  changes (a turn starting or ending, a message, a one-off command), as the registry of its
  machine publishes them.

A script or dev server running there is not activity. After an hour (`CHAUFFEUR_IDLE_REMINDER_SECS`
in tests), checked each minute, the workspaces that reached it together get one
`RegistryEvent::IdleReminder` and show `WorkspaceInfo.idle_since` until something happens there
or `workspace.remind_later` puts it off an hour. An ignored reminder is not repeated. The app
says it in a system notification (a toast while in front), a mark on the sidebar row, and a line
of the workspace's banner with "Remind me in an hour" beside "Bring back here". The phone's host
pushes it as `idle` (its own setting, on for a phone that opts in from then on, off for a
grant saved before; the workspaces' names when they are shown); the relay must be deployed with that category before it delivers one. Nothing is ever
stopped, deleted or brought back for it. A daemon that starts again counts the hour from its
start.

## Previews on the phone

Decided with Justin 2026-10-27 (`SPEC.md`). A moved workspace's declared web previews
([declared](workspace-development-services.md#web-previews) by their place in its port block)
open on the paired phone, privately:

- `previews.list {workspace}` is answered by the machine it runs on: each declared preview with
  its state there (ready: something accepts on its port; starting: nothing yet, while a run script
  of the workspace runs; error: nothing, and the last run script failed; stopped). This daemon
  puts its own port of the same place in its block, drops one its block lacks, and names the
  machine (`machine`, none for a workspace that runs here). The phone shows the list only for a
  workspace that runs on another machine.
- `GET /preview/{workspace}/{offset}`, on the Unix socket only: one connection to a declared
  preview, a WebSocket of its bytes (an empty message ends one side's writes) passed to the
  machine's `/tunnel/{workspace}/{offset}`. Refused for a workspace that runs here, an offset not
  declared there (asked there at each connection), or a port nothing listens on. Its connections
  end once the workspace no longer runs there (checked every 5 s), and with the daemon.
- The mobile host (`relay/src/bridge.ts`) admits `previews.list` with the workspace only, and
  tunnels (`tunnel_open|data|ack|end|close` messages, `packages/remote/src/tunnel.ts`) beside the
  RPCs of the encrypted session: each opens that route once the phone is approved and its device
  checked, at most 12 at a time, bytes base64-encoded, at most 64 KiB of one tunnel unacknowledged
  so that a slow side never fills the session's queue.
- The phone (`app/preview/[workspace].tsx` in the mobile repository), while a preview's screen is open, listens on
  127.0.0.1 on the Mac's port of every declared preview of the workspace (a page on one may call
  another), another free port when the phone uses that one, and opens
  `http://localhost:<port>/` in its web view: a dev server's redirects, cookies and login
  callbacks naming localhost and its port work as on the Mac. Leaving the screen closes the
  listeners and their connections. It lists the previews again every 3 s and closes, saying so,
  once the workspace no longer runs on that machine or the preview is no longer declared.

Android allows cleartext HTTP to localhost only (`mobile/plugins/localhost-cleartext.js`; a
development build keeps Metro's), iOS local networking (`NSAllowsLocalNetworking`). Another app
of the phone could reach those loopback ports while the screen is open, as a process of the Mac
can reach the relayed ports; nothing else can.

## In an editor of this machine

A workspace moved to a [GCP worker](cloud-workers.md#opening-it-in-an-editor) opens there from
this machine: `workspace.open` runs Cursor's or VS Code's own command line with `--remote
ssh-remote+<the worker's ssh host> <its folder there>` (a repo's worktree below it), through the
worker's entry in Chauffeur's ssh config, and the app shows the Open button for it with those two
editors only. Its folder there is the one the transfer that moved it reported. A workspace moved to
a machine declared by hand has no such entry: `workspace.open` refuses it, and the app shows no
Open button (decided with Justin 2026-10-27).

## Validation

`scripts/e2e-cloud-transfer.sh`: after the move, the source lists the same chats as the
destination; with `REAL=1`, the source sends the carried Haiku conversation its question and the
destination's answer recalls the word, and a chat created, renamed and deleted from the source
is created, renamed and deleted on the destination. Then, from the source: `attach` follows the conversation's reply and its end
(`REAL=1`), `wait` waits for it there; `attach --history` replays the destination's events with
the destination's numbers; `workspace.seen` and `workspace.mark_unread` change the destination's
mark, which the source shows; the destination stopped, a call says "vm does not answer" and a
subscription says it once, then "vm answers again" once it is back. Its worktrees, files, diff,
terminals and models are the destination's: a worktree added and removed from the source is
added and removed there, a terminal opened and typed into writes in its folder, an image written
there is served by the source's `/images`, and `files.open` is refused; an HTML page written
there previews from the source's preview URL, with the stylesheet it loads. A web server on the
destination's first port answers on the source's first port, and no longer once it stops nor
once the workspace is back; opening it here is refused (`vm` is declared by hand, with no SSH
entry; `scripts/e2e-cloud-workers.sh` opens one moved to a worker in a fake Cursor). Left idle 4 s (`CHAUFFEUR_IDLE_REMINDER_SECS`) with a script
running there, the source shows it idle; a read does not change that, "remind me later" hides it
until the time passed again, a terminal opened from here clears it, and the script still runs;
a local workspace has no reminder to put off. Through the phone's side (its loopback proxy and tunnels, the real
mobile host bridge on the source's socket; only the relay and Noise left out), a dev server on the
destination declared from the source as a preview (detected there, then `save --preview`) is
listed with the source's port and "ready", answers its login's redirect and session cookie and its
live-update WebSocket, and a port not declared is refused; a WebSocket held open through the
phone is cut once the workspace is back, and no preview opens any more. Unit tests cover the
idle reminders' timing, grouping and snooze, and which
calls are relayed (by workspace, chat, terminal or process) and a file sent by path.
