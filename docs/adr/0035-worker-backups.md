# 0035. Backups of a workspace running on a GCP worker

- **Date:** 2026-10-27
- **Status:** accepted

## Context

A workspace on a worker must survive the worker's loss even while the Mac sleeps.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Cloud workspace follow-up:

> Backups of a workspace running on a GCP worker (decided with Justin 2026-10-27,
> [backups](../cloud-workspace-backups.md)): the worker writes them itself, so they go on while the Mac
> sleeps, with a service account of its own limited to its prefix of the backup bucket, a managed
> folder whose IAM policy names it (an exception to the workers' "no identity, no IAM"; a condition
> on object names, decided first, cannot allow the listing `gcloud storage cp` makes, found by the
> first real trial and changed with Justin the same day). Without stopping the
> agents there is no atomic capture: a backup waits, within its five minutes, for a moment when no
> agent turn runs, else it is taken live (each part coherent, as after a power cut) and marked so.
> The key is the Mac's, one per workspace, given to the worker when the workspace arrives; storage
> sees ciphertext only. Before a backup is restored elsewhere, the old worker's VM is deleted with
> its boot disk kept (no instance can run it again; the disk keeps work newer than the backup for a
> manual recovery), checked through the API. Machines declared by hand are neither backed up nor
> restored for now: nothing proves they stopped. Chauffeur creates the bucket when it creates the
> first worker and finds none (`<project>-chauffeur-backups` unless `[workers] backup_bucket` names
> another; Paris, Standard, uniform access, public access prevented). Its soft delete keeps a
> deleted or overwritten backup 7 days (decided 2026-10-27, after a review found that any process
> of the worker can delete its backups): retention stays three backups, deleted by the worker.

## Consequences

Storage sees ciphertext only; the worker's service account is limited to its own managed folder of the bucket. Details: [`docs/cloud-workspace-backups.md`](../cloud-workspace-backups.md).
