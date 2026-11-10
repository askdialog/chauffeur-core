# GCP workers

A worker is a Compute Engine VM the Mac's daemon creates with the user's own `gcloud`, installs
Chauffeur on and reaches through the worker daemon's socket forwarded over SSH. Once ready it is
a transfer machine, named like the ones declared in Settings → Transfer
([driving a transfer](cloud-workspace-transfer.md)). Decided with Justin on 2026-10-26.

## What it creates, and what it never touches

```toml
[workers]
project = "your-cloud-project" # required; no project is configured by default
zone = "europe-west9-a"
bundle = ""                    # a Linux bundle to install instead of the release's
backup_bucket = ""             # the backups' bucket; by default <project>-chauffeur-backups
```

- One VM per worker, `chauffeur-worker-<name>`, labelled `chauffeur-worker=<name>`: Ubuntu 24.04,
  a 50 GB balanced boot disk, Shielded VM, `e2-standard-2`, `-4` (the default) or `-8`, in the
  project and zone of `[workers]`, always passed explicitly (the user's default `gcloud` project
  is not used).
- The VM runs as a service account of its own, `cw-backup-<name>`, with the storage scope only,
  to write its workspaces' backups ([backups](cloud-workspace-backups.md); decided with Justin
  2026-10-27). Its prefix `<name>-<8 random hex>`, recorded on the worker, is a managed folder of
  the backup bucket whose own policy gives it `roles/storage.objectUser`: it creates, reads, lists
  and deletes there only, and can neither read nor delete another worker's backups, nor those of
  an earlier worker of the same name. A condition on object names, the first design, cannot work:
  `gcloud storage cp` lists the bucket before writing, and a listing is checked against the
  bucket, never an object's name (found by the first real trial, 2026-10-27). No narrower predefined role creates, reads, lists and deletes (`objectCreator` cannot
  read or delete, `objectViewer` cannot write, together they cannot delete), and a custom role
  would be one more change to the project. Chauffeur creates the bucket if it is missing (Paris,
  Standard, uniform access, public access prevented, soft delete 7 days, labelled
  `chauffeur-backups=true`; one another worker's creation made meanwhile is used), and refuses
  one of another project, outside Paris or without uniform access. A bucket it made whose soft
  delete was turned off gets its 7 days back; another one without soft delete is used as it is,
  said in the worker's log. A later attempt keeps the bucket the first one was given access to,
  whatever `[workers]` says since. The worker's record names the account once Chauffeur made it:
  only that one is reused or deleted, and an account of that id it did not make for this worker
  refuses the creation. Deleting the worker deletes that account and counts it gone only when
  IAM says NOT_FOUND: a refusal "or it may not exist" leaves the worker failed, to delete again.
  The binding is retried for two minutes while IAM does not know the new account yet (an error
  naming the account as missing, or a 412); any other refusal stops at once. Its metadata blocks the project's SSH keys,
  so `gcloud compute ssh` puts the user's key on the instance, not in the project. No firewall
  rule, other IAM binding or static address is created; GCP gives the VM an ephemeral external
  IPv4 address (in the estimate), and SSH goes through the network's existing rule, as `gcloud
  compute ssh` does by hand.
- Deleting reads the instance's labels first and refuses one without `chauffeur-worker=<name>`:
  the project's other VMs (a relay's, an office server's) can never be deleted by it.
- The app and the CLI ask before creating each worker, showing its cost; past two workers, they
  also say so and the daemon needs that confirmation (`beyond_limit`), which is not a monetary cap ([cloud workspaces](cloud-workspaces.md)).

## Cost

Before anything is created, the app's dialog and `chauffeur worker create` show the estimated
hourly cost: the VM, its disk and its address, from Paris on-demand prices kept in the daemon
with their date (`PRICES_AS_OF`); network traffic and the agents' model use are not included. The
Cloud Billing API is not read; the estimate does not require enabling it in your project. A worker
costs while it runs, with or without workspaces; stopped, only its disk costs (about $5.80 a month).

| Type | vCPU | Memory | Estimate |
|---|---|---|---|
| e2-standard-2 | 2 | 8 GB | $0.09/h |
| e2-standard-4 | 4 | 16 GB | $0.17/h |
| e2-standard-8 | 8 | 32 GB | $0.32/h |

## Creating one

`worker.create {name, machine_type?, beyond_limit}` refuses at once a missing cloud project, a bad name, an unknown type, a
zone outside `europe-west9` (Paris, whose prices the estimate uses), a name a declared machine has, a third worker without confirmation, and a `gcloud` missing or
logged out. Then, in the background, each step logged on the worker:

1. The Linux bundle: `[workers] bundle`, else `chauffeur-<version>-linux-x64.tar.gz` of this
   daemon's own version, downloaded from the GitHub release with the user's `gh` and checked
   against the `.sha256` beside it, kept under `~/.chauffeur/workers/` for the next worker. A
   version without it fails here, before any VM exists.
2. The backup bucket if missing, the worker's service account, its managed folder and the
   folder's binding
   (retried while IAM does not know the new account yet).
3. The VM (`gcloud compute instances create`), unless it exists with the label already. One made
   before workers had an account has no place to back up to: a transfer to it says to delete it
   and create it again.
4. SSH, retried for up to three minutes while the VM boots.
5. The bundle copied (`gcloud compute scp`), then `worker_install.sh` run over SSH: Chauffeur in
   `~/.chauffeur/bin`, settings with Linear and the Office off, the `worker` mark (below), Claude
   Code (its own installer) and Codex, Docker with compose (Ubuntu's `docker.io` and
   `docker-compose-v2`, the user in the `docker` group, for the workspace's declared services),
   lingering on, the user's systemd manager started again so that its services have that group,
   and `chauffeur daemon install`, so the daemon is a systemd user service that outlives the SSH
   session and a reboot. Every `chauffeurd` of the user is stopped before, the service's or one a
   CLI started outside it over SSH, which the manager's restart does not reach: an old version
   left running kept the lock, and the new service restarted in a loop behind it (2026-11-02).
   The installation then waits for the service's own daemon (its `MainPID` in `daemon.json`),
   not just for a socket. `chauffeur daemon stop`, which `install` runs, sends SIGTERM to a
   daemon of another version that refuses the CLI's hello, rather than say none runs. Codex comes from its release package (`codex-package-x86_64-unknown-
   linux-musl`, in `~/.local/lib/codex`) at the version the sidecar speaks
   (`CODEX_APP_SERVER_VERSION`, a test keeps them equal): the bare binary lacks
   `codex-code-mode-host`, through which the current models run every command, and the `bwrap`
   that sandboxes them. An AppArmor profile, `chauffeur-codex-bwrap`, lets that `bwrap` create
   its user namespace, which Ubuntu 24.04 allows only to the programs a profile names. The first
   real trial (2026-10-29) found a Codex chat there answering but unable to run a command
   ("execution host is missing").
6. Its SSH entry read ([its entry](#opening-it-in-an-editor)), then the forward: `ssh -N -L
   ~/.chauffeur/workers/<name>.sock:<its socket>` with that entry's address, user, key and
   host-key options, ready once the worker's daemon answers through it.

GitHub CLI (`gh`) is installed with the worker's Git and curl dependencies, and the AWS CLI
beside them, including when preparation is retried on an existing worker. Their sign-ins come
from the Mac on each send ([GitHub and AWS access](#github-and-aws-access)). Transfer readiness
distinguishes a missing `gh` from one that is not signed in.

mise comes from its signed apt repository, with uv for every repo and mise's shims first on the
login shell's `PATH`, which the daemon reads when it starts: see [The toolchains a repo
declares](#the-toolchains-a-repo-declares).

A failure leaves the worker failed, with its error and whatever exists (the VM may cost): create
it again, which goes on from where the VM is, or delete it. A daemon stopped mid-way leaves it
failed the same way at its next start.

The bundle is built by `scripts/package-linux.sh` (Rust cross-compiled with cargo-zigbuild for
glibc 2.35, the sidecar with `bun --target bun-linux-x64` embedding Claude Code for Linux) and
published by a distribution's release beside the disk image, in the repository the build names
(`CHAUFFEUR_RELEASE_REPO`). For a build without one, or not released, point `[workers] bundle`
at the tarball it writes.

## The worker's keys

A headless VM has no Secret Service, where a Linux daemon otherwise keeps its protected keys (the
endpoint root a transfer identifies it by, the archive keys) without any plaintext fallback. The
installation writes `~/.chauffeur/worker`: a daemon whose state folder holds it keeps its secrets
in `~/.chauffeur/secrets/`, owner-only files, as a throwaway daemon does (decided with Justin
2026-10-26: the VM is a dedicated, single-user, trusted endpoint; sealing them with its vTPM is
later work). The first real trial found it: without the mark, the worker refused its first
transfer with "Vault root unavailable or changed".

## Keeping it reachable

The daemon keeps each ready worker's forward running: restarted when it ends or when the worker's
daemon stops answering through it three times, with a pause growing to 30 seconds, and again at
the daemon's next start. Its `ssh` runs in a process group of its own, stopped with the daemon
and when the worker is deleted. `connected` says the worker's daemon answers through it now.

The forward runs `ssh` itself with the worker's entry (`BatchMode`, so it never asks anything),
not `gcloud compute ssh`: gcloud's session expires (Google asks to sign in again now and then),
and a forward through it would then fail every 30 seconds while the VM answers SSH. gcloud runs
it only for a worker with no entry yet. A run that never reached the worker's daemon reads its
entry again, at most every five minutes (the address may have changed), and leaves why it
failed: ssh's own words, or that gcloud is signed out. The worker's `error` says it while it is
ready but not connected, in Settings → Transfers and `chauffeur worker list`. A transfer waiting
for the worker fails at once on a reason waiting will not fix (gcloud signed out, the key refused,
the host key changed), seen after the wait began, and otherwise after 15 minutes with the last
reason. gcloud's expired session reads, in every gcloud error, as what to do: run `gcloud auth
login`.

A workspace moved to a worker is piloted from the Mac like one moved to a declared machine
([piloting a moved workspace](workspace-relay.md)): `relay::machine_of` finds the worker's
forwarded socket (`workers::machine_socket`), over which the relay sends its calls, follows its
events and tunnels its ports.

While a workspace runs on a worker, the worker backs it up, encrypted, every five minutes to the
bucket of `[workers] backup_bucket` ([backups](cloud-workspace-backups.md)).

## Signing its agents in

The worker's agents use that machine's own logins, one per machine (decided with Justin
2026-10-29): never a copy of the Mac's Keychain or `~/.codex/auth.json` (a refreshed token would
log the other machine out), never an API key or Vertex. They are signed in from the Mac, never
from a terminal there: the login runs on the worker's CLI, and the user only approves it in the
Mac's browser and app.

- **The flows.** The worker's own login (`agent.login` with `headless`, through its forwarded
  socket) is one whose page never comes back to the worker's localhost, which the Mac's browser
  cannot reach. Claude Code's manual page (`claude_authenticate`'s `manualUrl`, redirecting to
  `platform.claude.com/oauth/code/callback`) shows a code, which the user pastes back in the app
  or the CLI and the worker's CLI takes (`claude_oauth_callback`). Codex's device code
  (`account/login/start` `chatgptDeviceCode`, as `codex login --device-auth`) gives a page and a
  code the user types there, and the app-server polls for the approval.
- **What passes through the Mac.** The page and, for Claude Code, its single-use authorization
  code: only the worker's CLI can exchange it, holding the flow's PKCE secret. The credentials are
  written by the worker's CLI where it keeps them; the Mac never sees, logs or stores them.
- **The calls.** `worker.login` starts it and returns the page (`url`, the `user_code` to type,
  `asks_code` when a code comes back); `worker.login_code` passes the code on;
  `worker.login_cancel` ends it; `worker.usage` reads whether each agent is signed in there. The
  worker's daemon reports the end with a `login_ended` event, which the Mac's daemon publishes
  with the worker's name (given up after 11 minutes; the worker's sidecar gives up at 10). A new
  login of the same agent replaces the one under way, which is not reported.
- **Where.** Settings → Transfers lists each ready worker's agents, with Sign in for those signed
  out; a transfer refused because no agent is signed in there leads to it and offers the same
  ([checking the destination](cloud-workspace-transfer.md#checking-the-destination-first)): one
  agent signed in, Claude Code or Codex, is enough (decided with Justin 2026-10-30). `chauffeur worker
  login <name> [--agent claude|codex]` prints the page (opened in the browser from a terminal),
  reads Claude Code's code on its standard input and waits; without `--agent`, each agent signed
  out there.

The worker keeps its login while it exists; an on-demand worker stopped rather than deleted
after its last return keeps it too, so signing in is needed for its first transfer and when a
login expired.

## GitHub and AWS access

Unlike its agents, a worker gets this Mac's GitHub and AWS SSO access, copied again on each send
(decided with Justin 2026-10-31, [ADR](adr/worker-github-and-aws-access.md)), so that the user signs
in to neither there. Before the destination's check, the Mac's daemon reads the token of each host
its `gh` is signed in to (`gh auth status --json hosts`, then `gh auth token --hostname`), the AWS
CLI's `~/.aws/config` and its SSO token cache `~/.aws/sso/cache/*.json`, and hands them to the
worker's daemon through its forwarded socket (`worker.credentials_install`, a peer method). The
worker's daemon:

- signs its `gh` in with each token (`gh auth login --hostname <host> --git-protocol https
  --with-token`, the token on standard input), then `gh auth setup-git`;
- has its `git` fetch and push the host's SSH URLs over HTTPS, which `gh` signs:
  `url.https://<host>/.insteadOf` set to `git@<host>:` and `ssh://git@<host>/` in its global
  configuration. A worktree's `origin` keeps the Mac's URL, often SSH, and the worker has no SSH
  key; the Mac's own configuration is never touched;
- writes the AWS files under its `~/.aws`, owner-only (`0600`, folders `0700`), each over its
  previous copy of the same name; SSO cache files of its own are left.

`~/.aws/credentials` (static keys) is not copied. The tokens are never logged, nor put in a
result, an event or the transfer's log. A part that fails is a warning in the transfer's log, and
the transfer goes on. A Mac whose `gh` is signed out, or without AWS configuration, gives nothing,
and the worker's check still says when its `gh` is signed out. Machines declared by hand and the
way back get nothing. The copies stay on the VM while it exists; an SSO session expires there as
on the Mac, and the next send copies the current one.

## The toolchains a repo declares

A worker installs, in each repo of a workspace it receives, the toolchain versions that repo
declares, before the repo's setup script runs (decided with Justin 2026-11-02,
[ADR](adr/worker-toolchains.md)): a web repo's `pnpm install` failed there with `pnpm: command
not found`. Nothing is named by Chauffeur itself; what a repo declares is:

- the tools of `.tool-versions` and of mise's configuration (`mise.toml`, `.mise.toml`,
  `mise/config.toml`, `.config/mise.toml`), which mise reads only once trusted;
- node from `.nvmrc` or `.node-version`, python from `.python-version`, when those did not name
  them (mise's idiomatic version files, turned on for node and python by `worker_install.sh`);
- uv for a `uv.lock`, installed for every repo by `worker_install.sh`;
- the pnpm or yarn `packageManager` of `package.json`, through corepack.

When the Mac's daemon sends a workspace to a GCP worker it created, it activates it there with
its setup scripts held (`toolchains` in `cloud.transfer.destination_activate`), then, once the
transfer's slot is free, asks `cloud.transfer.destination_setup`: in each worktree that declares
something, the worker's daemon runs `mise trust .` when it has a mise configuration, `mise
install`, and for a `packageManager` `corepack enable`, `corepack install` and `mise reshim`,
answering yes to their questions (`MISE_YES`, `COREPACK_ENABLE_DOWNLOAD_PROMPT=0`), with the login
shell's environment; then it starts the setup scripts. The transfer's log says `installing node
24, pnpm 10.24.0 for web`; a step that fails, or runs past fifteen minutes, adds `could
not install what <repo> declares (<its last line>); its setup runs without it`, and the setup
script runs all the same. The Mac, a machine declared by hand and the way back install nothing:
their setup scripts start at activation, as before. The versions stay installed on the worker's
disk, which a stopped worker keeps, so the next send of a repo with the same versions only checks
them.

## Claude Code configuration

A worker also gets this Mac's Claude Code configuration on each send (decided with Justin
2026-11-02, [ADR](adr/worker-claude-code-configuration.md)), so that its agents work there as
here. Both ends resolve it as their agents do: `CLAUDE_CONFIG_DIR` when set, else `~/.claude`, and
the state file `.claude.json` inside that folder when it is set, else in the home folder. Copied,
when present: `CLAUDE.md`, `settings.json`, `skills/`, `agents/`, `commands/`, `hooks/`,
`plugins/installed_plugins.json`, `plugins/known_marketplaces.json`, `plugins/cache/`,
`plugins/marketplaces/`, and the `mcpServers` of the state file. Never the login, the transcripts,
the history, sessions, caches or backups.

Before the destination's check, the Mac's daemon packs them (tar, zstd: times and owners left
out, so that an unchanged configuration gives the same digest) and asks the worker which digest
it installed (`worker.claude_config_status`). The same one is not sent again. Otherwise the
archive goes in chunks of 8 MiB (`worker.claude_config_write`), then the worker's daemon checks
its digest and puts it in place (`worker.claude_config_install`):

- it unpacks it beside the configuration folder, refusing an absolute path, a `..`, an entry
  under a link, a link leaving its folder or an item outside the list (the Mac leaves out the
  links leaving their folder, such as a plugin's absolute one);
- it makes the Mac's configuration folder and home the worker's in `settings.json`, the two
  plugin lists and the MCP servers, as whole paths only; plugin and skill contents stay as they
  are;
- it replaces each item: a folder whole (a skill deleted on the Mac goes there too), a file by a
  rename; `settings.json` `0600`, the folder `0700`;
- it adds the Mac's MCP servers to its state file, replacing those of the same name and keeping
  its other keys (its own login) and servers, written `0600`. A server on the Mac's own address
  (`localhost`, `127.0.0.1`, `::1`) or whose command it does not have (an absolute path that does
  not exist, a name not on its login PATH) is left out, named in the transfer's log. Headers
  written in a server's configuration travel with it; a server signed in by OAuth needs its
  sign-in on the worker, whose tokens are not copied.

A failure is a warning in the transfer's log, and the transfer goes on. The daemon's log says
when it gave the configuration and when the worker had it already, never its contents.

## Opening it in an editor

A workspace moved to a worker opens there in this machine's Cursor or VS Code, with their
Remote-SSH (decided with Justin 2026-10-27).

**Its entry.** Once a worker is installed, before its forward starts, the daemon asks `gcloud compute ssh <vm> --dry-run` how it
reaches it and writes that as a `Host chauffeur-<6 hex>-<name>` entry (the hex from the daemon's
state folder, so two daemons' workers of one name never shadow each other) in `ssh_config` under
its state folder (`~/.chauffeur/ssh_config`), a file only Chauffeur writes: address, user, key,
`HostKeyAlias` and Google's known-hosts file, with `StrictHostKeyChecking yes` (gcloud asks not to
check before the key is known; its installation over gcloud's ssh has recorded it by then). Any other flag or option of
gcloud's command (a port, IAP's `ProxyCommand`) is refused rather than written half-understood.
The entries are read again at each start (an address may change), and a worker's goes with it.
When it cannot be read (gcloud logged out), the worker keeps the error and opening it says to run
`gcloud auth login`, then restart Chauffeur.

**The `Include`.** Once for the daemon, at its first ready worker (or at the first start after
an update, for workers created before), it adds `Include "<that file>"` at the top of
`~/.ssh/config`, creating the file if needed, and says so in the workers' log; nothing else in
`~/.ssh` is written, and a removed `Include` is not added back. `~/.ssh/config` is replaced
atomically (a unique file beside it, synced, then renamed), only if it is still the file read,
checked just before the rename (same device, inode, modification time, size and content; else read
again), with its permissions; another program can still save it in the microseconds between that
check and the rename. A symbolic link, a file its owner cannot write or one of another user is
left alone, and so is one that cannot be replaced (an `.ssh` folder not writable, a file locked
with `chflags uchg`): the line to add by hand is named instead. A state folder whose path a
quote, a backslash, a control or glob character would break is refused before anything is
written. A daemon whose state is not `~/.chauffeur` (a throwaway one) never touches `~/.ssh`
unless `CHAUFFEUR_SSH_DIR` names an ssh folder (the e2e scripts do); it names the line to add.

**Opening.** `WorkerInfo.ssh_host` names the entry only while a line of the ssh config includes
it; until then `WorkerInfo.ssh_issue` says why, with what to do (the `Include` line to add by
hand…). `workspace.open` on a workspace moved to a worker runs the editor's own command line (inside
its bundle on macOS, on `PATH` on Linux) with `--remote ssh-remote+<host> <its folder there>`, or
a repo's worktree below it. It refuses, saying what to do: a worker without its entry or its
`Include`, a design chat, a worktree outside the workspace's folder, an editor not installed, any
other app, a workspace whose folder there is not known. The app shows the Open button for it, with
Cursor and VS Code only; while it cannot open, the button carries a warning sign and shows that
reason, and the app reads the workers again each time its window comes back (a line added by hand
shows without a restart). A workspace moved to a machine declared by hand has no entry: no Open
button, and `workspace.open` refuses it.

**Not yet.** Whether the ssh config includes the entries is read simply: an `Include` indented
under a `Host` or `Match` block counts as present though it applies to those hosts only, and one
through a glob or a relative path is not recognised. The alias's hex comes from the state folder's
path as written (a trailing slash or a link to it gives another alias). The ssh folder is found
from `HOME` only, where the daemon's paths fall back on the user's passwd entry.

## Recovering a failed worker's workspace

[Manual backup recovery](cloud-workspace-backups.md#recovering-on-the-mac) is the exception to
normal deletion's refusal while workspaces depend on a worker: after confirmation naming all of
them, its VM is deleted with every disk kept, its boot disk checked unattached, and the selected
workspace restored here from its authenticated backup. The worker remains failed with its kept
disk named and billed; its account and backups stay for manual inspection and later cleanup.

## Deleting one

`worker.delete {name}` refuses while a workspace of this daemon runs on the worker (bring it back
first) or a transfer to or from it runs. It stops the forward, checks the label and deletes the VM
with its disks, removes its folder's binding, deletes its service account and then its folder
when empty (its backups, if any, keep it until they are deleted), then
forgets the worker and removes its SSH entry. The bucket, and the backups in it, stay. On-demand workers are stopped after their last confirmed
return, as described below, and deleted only this way.

## In the app and the CLI

Settings → Transfers lists the workers under the declared machines: type, estimated cost, state
(creating, installing, ready or reconnecting, failed with its error, deleting), the log under
Details, Retry for a failed one and Delete with a confirmation. **Create a worker…** asks a name
and a type, shows the estimate and the limit, and creates on confirmation. The transfer dialog
offers ready workers after the declared machines, with their type and cost. A ready worker shows
whether each agent is signed in there, with Sign in for one that is not.

```sh
chauffeur worker list
chauffeur worker create <name> [--machine-type e2-standard-4] [--yes] [--no-wait]
chauffeur worker delete <name> [--yes] [--no-wait]
chauffeur worker login <name> [--agent claude|codex]
chauffeur worker ssh <name> [-- <command>]
chauffeur cloud transfer <workspace> --to <name>
```

## Validation

The daemon's tests run a fake `gcloud`: the create arguments (label, project and zone, its own
account with the storage scope only, no project keys), the bucket, account, folder and binding made before
the VM, a bucket outside Paris or an account Chauffeur did not make refused before any VM, an
account recorded only once made and counted deleted only on NOT_FOUND, the bucket of a first
attempt kept by the next, the binding retried only while IAM does not know the account, the
refusals, a failed worker created again on its VM, deletion, an
unlabelled instance never deleted, a daemon stopped mid-creation, the SSH entries and the
`Include` added once (also for a worker from before the update, and not again once removed), the
`Include` never written into a symbolic link, a read-only config or over an edit made meanwhile,
nor left half-written, an unusual state path quoted or refused, a throwaway daemon leaving
`~/.ssh` alone, an entry rewritten when the address changed, and every refusal of opening. `scripts/e2e-cloud-workers.sh`
drives real throwaway daemons through a fake `gcloud` that starts the worker's daemon and
forwards with a symlink: the estimate and the refusal without confirmation, creation, a workspace
transferred to the worker by its name, its files and a port relayed there from the Mac, the
workspace and a repo opened there in a fake Cursor over SSH (Finder refused), and back; a
workspace with a Codex chat refused while the worker's Codex (a fake app-server on the daemons'
`PATH`, through a fake login shell) is signed out, Claude Code's manual login started and
cancelled without a code (nothing reaches Anthropic), Codex signed in with its device code from
the Mac, and the refused transfer resuming by itself; deletion
refused meanwhile, the forward back after a daemon restart, and deletion with its SSH entry. The
source daemon runs with a temporary `HOME` and `CHAUFFEUR_SSH_DIR`, whose `config` gets the
`Include`. Neither reaches GCP; a run on a real project needs the
user's explicit go.

A real trial of backups on a worker (2026-10-27) is in [backups](cloud-workspace-backups.md#validation).

Real trial (with Justin's go, 2026-10-26), throwaway Mac daemons, the real `gcloud`, one
`e2-standard-4` in a test project and a locally built bundle: the worker was
ready in 2 min 44 s (systemd user service active, lingering on, Claude Code and Codex installed);
a workspace with an edit went there by its name and came back, deletion refused meanwhile; then
the VM and its disk were deleted, leaving the project's other VMs as they were. The first
run found the missing Secret Service (above).

Second real trial (with Justin's go, 2026-10-27), same setup with a bundle built from `main`: the
worker was ready in 2 min 40 s. A workspace with a `postgres:16` service declared as data and a
Haiku chat went there in 34 s: the row written on the Mac was in the worker's database, the chat
answered "kiwi" to a message sent from the Mac through the relay, and a web server on the
workspace's port there answered on its port on the Mac. It came back in 28 s with a row written on
the worker, the chat still answering "kiwi" here; then Chauffeur deleted the VM and its disk in
54 s. Docker was not installed by `worker_install.sh` then: it was added by hand, then the user's
systemd manager restarted for the daemon to join the `docker` group, as the installation now does. The run found the anonymous
volume a service without a named one leaves behind ([development services](workspace-development-services.md)).

## On-demand lifecycle

Choose **On-demand Cloud** in the transfer dialog, or use `chauffeur cloud transfer <workspace> --to cloud`.
The estimate shown before sending covers the default VM, disk and address while it runs;
network and model usage are extra. `--yes` accepts that estimate and exceeding the worker limit.
Chauffeur automatically prepares a local controller unless an advanced override names another one.
Agents still need their own login on the worker, signed in from the Mac when a transfer asks for it.

The daemon reuses workers it created automatically, including for concurrent sends. When the last
workspace has returned to this Mac and its remote copy is discarded, the daemon stops the VM
(`gcloud compute instances stop`) with its disk, which keeps Chauffeur, Docker's images and the
agents' logins (decided with Justin 2026-10-29: deleting it asked for a new login at every round
trip). Its forward stops; its SSH entry, service account and backup access stay. Stopped
(`stopped` in `worker list` and Settings → Transfers), only its disk is billed: about $5.80 a month
for 50 GB balanced, the VM and its ephemeral address cost nothing. The stop holds the admission
lock: a send to Cloud meanwhile waits for it, then starts the worker again.

The next send to Cloud picks the stopped worker before creating one: the daemon starts its VM
(`gcloud compute instances start`, its address may change), waits for SSH, copies this daemon's
Linux bundle and runs `worker_install.sh` again, whatever version the disk kept: a transfer needs
both ends on one version, and the installation restarts the user's systemd manager, so the
worker's daemon runs the new binaries. The SSH entry is read again, then the forward started, and
the transfer goes on as usual; the agents are still logged in. A cancelled preparation leaves a
stopped worker stopped.

A ready worker is updated the same way when a transfer finds it on another Chauffeur version than
the Mac's, which happens once the Mac updated while a workspace ran there: a send to it, or a
return or an archive bringing a workspace back from it, copies the Mac's bundle and runs
`worker_install.sh` again before anything else, then waits for its new daemon (the forward
reconnects by itself). The installation restarts the worker's daemon: agents working there, in
any of its workspaces, are interrupted. Nothing is updated outside a transfer (decided with
Justin 2026-11-02, see [the ADR](adr/worker-updated-by-a-transfer.md)).

A failed return, pending discard or interrupted handoff keeps a durable reservation. Cleanup
resumes after a daemon restart: a stop it did not see through is tried again, and a failed stop
leaves the worker failed with its error (it may still cost), to send to Cloud again or delete.
Failed preparation can be retried or cancelled from the transfer; cancellation does not stop
another workspace on the same worker. A worker never installed (its creation failed and was
cancelled) is still deleted. Deleting a stopped worker, VM, disk, account and SSH entry, is the
user's action: **Delete** in Settings → Transfers or `chauffeur worker delete <name>`. Manual
workers are deleted explicitly as before; retained backup-recovery disks are excluded from
automatic cleanup. A workspace left in the cloud keeps its VM running even while idle.

`AUTOMATIC=1 scripts/e2e-cloud-workers.sh` exercises concurrent creation/reuse, first/last return
and the stop, restart, logged-out failure/cancellation leaving the worker stopped, the stopped
worker started and installed again by the next send, and its explicit deletion, using fake GCP
only.

`AUTOMATIC_CONTROLLER=1 AUTOMATIC=1 scripts/e2e-cloud-workers.sh` runs without a controller
setting or a manually started controller. It verifies the managed process, its shutdown/restart
and a round trip against the same durable journal; `RECOVERY=1` covers recovery on that path.
