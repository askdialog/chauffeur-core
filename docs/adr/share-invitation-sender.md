# Invitations come from the operator's sender, of a subdomain verified alone

- **Date:** 2026-11-05
- **Status:** accepted

## Context

Invitations came from the service's address in the owner's name ([share-invitation-emails](share-invitation-emails.md)), and SES verified the Workspace's whole domain: whoever may send through SES in that AWS account could then sign as any address of the Workspace, and pass its DMARC. Justin wanted a fixed sender, of a subdomain of its own.

## Decision

Decided with Justin 2026-11-05:

- An invitation comes from a fixed sender the operator sets: a name (`--mail-from-name`, `SHARE_MAIL_FROM_NAME`, `Chauffeur` by default) and an address (`--mail-from`). The owner is named in the subject and the text, and replies still go to them.
- The sender's address may be of a subdomain of the Workspace's domain, verified alone in SES, so that SES can sign for no other address of the Workspace. `deploy/aws-mail.sh` verifies the sender's domain, refusing one outside the Workspace's.

## Consequences

- With a subdomain, the Workspace's addresses are not verified recipients: the AWS account leaves SES's sandbox (production access, asked once). With the Workspace's domain, the sandbox is still enough.
- The deploy scripts carry the name to the service through the relay VM's metadata; it is plain ASCII there (letters, digits, spaces, dots, apostrophes, hyphens), and the service encodes any other name as MIME requires.
