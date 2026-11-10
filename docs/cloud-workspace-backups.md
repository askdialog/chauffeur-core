# Backups of a workspace running on a worker

While a workspace runs on a [GCP worker](cloud-workers.md), the worker backs it up, encrypted,
to a GCS bucket in Paris, about every five minutes, keeping the three latest successful backups
(approved scope: [cloud workspaces](cloud-workspaces.md#recovery-notifications-and-cleanup);
decisions with Justin 2026-10-27 in `SPEC.md`). The Mac shows where they stand. Manual recovery on the Mac is available below; deleting a workspace's backups once it is back remains later work.

```toml
[workers]
backup_bucket = ""   # by default <project>-chauffeur-backups, made with the first worker
```

## What a backup holds

A backup is the [transfer package](cloud-workspace-package.md) of the running workspace
(`cloud_workspace_package::capture_running`): a Git snapshot per worktree (its commit, staged
changes, tracked and untracked files), its notes folder, each chat's row, queue and history, and
each chat's native Claude or Codex conversation. Its PostgreSQL exports are not carried yet.

Nothing stops for it. Without stopping the agents no capture is atomic, so each part is coherent
on its own and the whole is as after a power cut:

- a backup is due five minutes after the last attempt; from four minutes on it is taken as soon
  as no agent turn runs, else it is taken live at five and marked so (`live`);
- each chat's history is taken up to the event it had when the capture began, and each native
  transcript up to its last complete line (a subagent's with none yet carries nothing). A
  conversation that still cannot be carried (its main transcript has no complete record yet) is
  named in the status (`incomplete`); the next change brings a new backup that tries again, and
  with nothing else changed it is tried again each hour (each try replaces a backup, which
  storage keeps seven days after);
- a worktree is read twice (`cloud_workspace_snapshot`) with Git's optional locks off, so as not
  to take the index lock from an agent; one that changed in between is read again, up to three
  times, then the backup fails and the next one tries again;
- files of the workspace folder in no repo and outside `.chauffeur/` are left out and named
  (`left_out`), where a transfer refuses them.

Nothing is uploaded when nothing changed since the last backup (the worktrees' files, notes and
chats read the same): once storage confirms it still holds that backup (`gcloud storage objects
describe`), it is still current, and `checked_at` says since when. One gone from storage is taken
again. Old backups a deletion missed go at that check too.

## Encryption and storage

The key is the Mac's: one per workspace (`chauffeur-backup-v1-<workspace>` in its Keychain), made
when the workspace first leaves for a worker. Once the workspace is active there, the Mac's daemon
hands it to the worker with the bucket and prefix the worker was created with
(`cloud.backup.configure`, private socket only; a worker made before backups has none, and the
transfer's log says to make it again); the worker keeps it in its owner-only secrets.
Storage sees ciphertext, opaque ids, sizes and times only.

The package is sealed by the sidecar (`package.seal`, AES-256-GCM, the transfer archives' format)
under a backup's own authenticated data: `chauffeur-workspace-backup-v2`, the workspace, its
owner and controller, the backup's id, the worker and the epoch of the transfer it runs under
(`backup_context`). All of it is what the Mac keeps of that transfer (its retired copy's fence)
and the object's name, so any backup in storage opens with the key (`open_backup`), even one taken
while the Mac slept and never reported to it; the package's manifest checks the rest. A backup
cannot open as a transfer's archive, nor the reverse. The plaintext package is removed before
anything is sent.

The worker writes `gs://<bucket>/<worker>/<workspace>/<backup>.bin` with `gcloud storage cp`,
which checks the upload's hash, then reads its size back (`gcloud storage objects describe`). Only
then does the backup count, and only then is the oldest beyond three deleted (`gcloud storage
rm`); a deletion that fails is tried again after the next success. An upload cut short leaves no
object, so an incomplete backup never replaces a good one, and a failed or slow backup never costs
the last good one. The object's name is recorded before it is written: one that was written but
did not count (its size could not be read back, or the daemon stopped meanwhile) is deleted with
the old ones. A daemon that starts again removes what a backup it stopped left in
`~/.chauffeur/backups/` (the plaintext package, the ciphertext). A storage call that hangs is
killed: an upload after 30 minutes, any other call after 2, and the backup fails with why.

Each stay of the workspace on a worker is its own: when it leaves the worker (`source_retire`),
its backups stop there and the worker deletes its key; when it comes back to the same worker
(`cloud.backup.configure` under another transfer's epoch), its record starts afresh, and what an
earlier stay kept goes once the new stay has a backup of its own, since the Mac no longer opens
backups sealed under an earlier transfer.

The worker writes with a service account of its own, limited to its prefix of the bucket, a
managed folder whose policy names that account (decided with Justin 2026-10-27), which the Mac
makes when it creates the worker, with the bucket if missing ([GCP workers](cloud-workers.md#what-it-creates-and-what-it-never-touches)).
A worker created before has neither bucket nor prefix: the transfer's log says to delete it and
create it again, and its backups show failed with that reason.

Any process of the worker, an agent included, can reach that account through the VM's metadata
server and delete or overwrite its backups. So the bucket keeps deleted and overwritten objects 7
days (soft delete, decided with Justin 2026-10-27): such a backup stays recoverable for a week.
It costs the storage of every backup replaced during those 7 days (one every five minutes while
the workspace changes, each billed a week after it is deleted), and deleting a worker does not
shorten them. Retention is unchanged: the worker keeps three and deletes the older ones.

## Where they stand

`WorkspaceInfo.backup` (`BackupStatus`): the health (`pending` until a first backup, `current`,
`stale` fifteen minutes after the last success, or after the setup while none came, `failed` with
the last attempt's error), the setup, last success and attempt times, the backups kept (each with what opening it needs besides the key) and what was
left out. The worker computes it for a workspace running there; the Mac keeps what the worker last
said (`relay_events` mirrors it on each registry update) and computes the health from its own
clock, so a worker that no longer answers turns stale there. A transfer to a worker logs where the
backups go, or why the worker could not be set up, which then shows as failed. Each transfer
starts that record afresh: to a worker, with nothing yet; back here or to a machine that is not
backed up, it is forgotten.

The moved workspace's banner in the app, and its card on the phone, add one line under "runs on
<machine>": "Backed up, encrypted, at 10:42" (with "taken during a turn" when it was, and
"unchanged at 11:02" when it was found unchanged since), a warning naming the conversations it
could not carry, the first
one on its way, "none succeeded for 15 minutes" (stale), or "the last backup failed; the one
before, at 10:37, is kept" with the error. Clients turn a current status stale on their own
clock fifteen minutes after its last check (`backupView`): the Mac publishes it only when it
changes, and a silent worker sends nothing. The app's tooltip also names the files left out.

`cloud.backup.run {workspace}` takes one now, on the machine the workspace runs on (relayed from
the Mac), and returns the status:

```sh
chauffeur cloud backup <workspace>            # take one now
chauffeur cloud backup <workspace> --status
```

## Recovering on the Mac

The moved workspace's banner offers **Recover a backup…**, or:

```sh
chauffeur cloud recover <workspace> [--backup <id>] [--keep-local-copy] [--yes]
```

The daemon lists the backup objects still in the worker's own storage prefix, including backups
made while the Mac slept. The newest is selected by default. The confirmation shows its time,
warns that newer work may be lost, and names **every workspace on that worker**: deleting its VM
stops them all. The disks remain billed until removed manually. Nothing runs on GCP during the
automated tests.

Before changing the VM, recovery checks the archive size and free disk space, pins its GCS
generation against replacement during download, then authenticates and verifies the selected backup with
the Mac's key and its current committed transfer. It checks the local retired copy for edits
(`--keep-local-copy` preserves them), local readiness and the controller's current journal.
Backups omit development data, so a package declaring services is refused before deleting the VM;
recover those services from the retained disk manually until backup data export is supported.

The journal then reserves the return to this Mac. The daemon records the old VM's instance id
and boot disk before deleting that labelled VM with **all disks kept**. It verifies through GCP
that the VM is absent and its boot disk is unattached: permission denied, a replacement VM with
the same name or a disk attached elsewhere all refuse activation. The package is bound to the
new journal operation, restored into the existing transfer quarantine and activated only after
the controller commits it. Files and conversations use the existing return mechanism; setup
scripts run only after activation.

A durable recovery record keeps its original transfer, backup, local-copy choice and operation.
After an interruption, retry the same recovery; its journal state determines the next step.
Before the journal starts, another backup may be chosen if one failed verification. After it
starts, the backup cannot change. Nothing automatically cancels a recovery or reactivates its
old source. The worker stays failed and cannot be recreated or deleted through Chauffeur while
its recovery marker and kept disk need manual inspection. Its other workspaces can be recovered
from their own backups in turn. Completed recovery staging is removed; its backups, service
account and disks are retained for the later cleanup step.

## Validation

`RECOVERY=1 scripts/e2e-cloud-workers.sh` also stops the fake worker, refuses a corrupted backup
before deleting anything, and refuses activation while the kept disk is attached. After restarting
the Mac daemon, the retry restores files, then a final GCP check is made to fail before commit;
a second restart and retry completes the same journal operation. The backup's edit is restored,
newer work stays on the retained worker disk, and the retained worker cannot be recreated.
Unit tests refuse an unlabelled or replacement VM, permission-denied absence and an attached
disk, and keep the recovery action available while an interrupted restoration is quarantined.


Unit tests cover when a backup is due, the health, accepted bucket names and prefixes, the
sidecar's backup authentication and live transcripts, and the line clients show (`transfer.test.ts`). The lifecycle test backs up a running
workspace with a chat mid-turn. `scripts/e2e-cloud-workers.sh` sets a bucket, transfers a
workspace to the fake worker and checks that a backup lands in the fake `gcloud storage` as
ciphertext, that nothing is uploaded when nothing changed, that three are kept after four
changes, that a failing storage leaves them and shows failed with its error, and that the Mac
shows each state.

Real trial (with Justin's go, 2026-10-27), throwaway Mac and controller daemons, the real `gcloud`,
a test project, one `e2-standard-2` and a bundle built from the branch. Four
runs stopped early, each cleaned up, fixed with a test and run again: `gcloud storage buckets
describe` names the bucket's project only with `--raw`; a condition on object names cannot allow
the listing `gcloud storage cp` makes (hence the managed folder); IAM answers "denied (or it may
not exist)" for an account deleted lately; Compute Engine does not know an account made seconds
earlier. The fifth went through: the worker was ready in 3 min 40 s with its bucket (Paris, soft
delete 7 days, labelled), account and folder binding, and no binding on the bucket; the workspace
moved in 2 s; the first scheduled backup landed 18 s later and the second, after an edit on the
worker, 4 minutes after it, both under the worker's prefix, the only objects in the bucket.
Downloaded, they held nothing readable (only the `CHWSPK01` header in clear, 7.9 bits of entropy
per byte), and each opened with the Mac's key, its committed transfer and the object's name alone,
the first with the Mac's edit and the second with the worker's. From the worker, its account could
read its own backups but not list the bucket, nor read, write or delete under another prefix, nor
list its own folder (which it does not need). The Mac showed the backups current, then nothing once
the workspace was back (2 s). Deleting the worker (56 s) removed the VM and its disk, the folder's
binding, the account and the empty folder; the backups, which no return deletes yet, and the
bucket were removed by hand with soft delete cleared first, so nothing was left billed.
