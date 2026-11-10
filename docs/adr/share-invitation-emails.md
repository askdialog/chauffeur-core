# Colleagues named on a share get an e-mail, sent through Amazon SES

- **Date:** 2026-11-04
- **Status:** sender superseded by [share-invitation-sender](share-invitation-sender.md), recipients by [share-with-any-email](share-with-any-email.md)

## Context

A design share (story 51) or a workspace share (story 53) names colleagues by e-mail, but nothing told them: the owner had to send the link by hand. Justin asked that inviting someone send them an e-mail, through Amazon's service. The share service runs on GCP (the relay's VM), the operator's AWS accounts had never used SES, and the sending domain's DMARC is `p=reject` with strict alignment.

## Decision

Decided with Justin 2026-11-04:

- The share service writes to each colleague a share names, at its creation, and on an update to the addresses it adds only; never to the owner, to an address outside the service's domain, or to the whole team. The e-mail comes from the service's sending address in the owner's name, replies go to the owner, and it carries the link and what the colleague may do. Sending never fails a share; a failure is logged.
- Amazon SES in the operator's AWS account, the sending domain verified with Easy DKIM (the CNAMEs added to its DNS). The account stays in the sandbox: every recipient is an address of the verified domain, and 200 e-mails a day is enough.
- No AWS key is stored: the VM's service account trades its Google ID token for a role allowed to send from the sending address only (`AssumeRoleWithWebIdentity`).

## Consequences

- `deploy/aws-mail.sh` creates the identity and the role (run by hand, with the AWS account's credentials); `deploy/gcp.sh` passes the role's ARN to the service, which sends nothing without it or without a sending address (`--mail-from`), as in development and tests.
- The service signs its SES and STS calls itself (Signature Version 4 with `hmac` and `sha2`) rather than pulling the AWS SDK into its image; it is tested against AWS's published vectors.
- In the sandbox, e-mails leave one a second; a share naming 50 colleagues takes under a minute. Leaving the sandbox (a support request to AWS) becomes necessary only to write outside the verified domain, which shares do not allow.
- Not done: a link back from the e-mail to unsubscribe, bounces and complaints handling (a configuration set with SNS), an e-mail when a role changes or a share stops.
