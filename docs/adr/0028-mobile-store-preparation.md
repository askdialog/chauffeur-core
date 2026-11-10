# 0028. Mobile store preparation

- **Date:** 2026-10-20
- **Status:** accepted

## Context

The publisher's developer accounts were being verified; iOS notifications, privacy and support information and review access had to be ready for the stores.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Mobile store preparation (approved 2026-10-20):

> Prepare iOS notifications, privacy/support information and review access while
> the publisher's developer accounts are being verified. iOS uses the same explicit opt-in,
> categories and optional workspace-name disclosure as Android, with native APNs
> delivery directly from the relay. Apple provider credentials stay only on the
> relay. Missing provider on existing grants means FCM. Both clients keep the
> workspace-list destination on notification taps; no action executes from a push.
> APNs environment is relay deployment configuration, production for store builds.
>
> An About screen is accessible before and after pairing, with version, data-flow
> explanations and public HTTPS privacy/support links. Production builds require
> these links. Legal details and operational retention must be finalized by the publisher
> before those pages are published. An explicitly labeled public demo uses only
> synthetic data and simulated actions, with no daemon RPCs or real agent execution.
> Live review access and physical-device release testing remain required work;
> this demo does not imply store approval or verified live push delivery.

## Consequences

Legal details and retention must be finalized by the publisher before the privacy and support pages are published; live review access and physical-device testing remain work to do.
