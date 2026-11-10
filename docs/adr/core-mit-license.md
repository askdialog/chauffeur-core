# Only Chauffeur Core is released under MIT

- **Date:** 2026-11-09
- **Status:** accepted

## Context

Core was prepared for open source with independent defaults and a separate source repository,
but its license was still undecided. Justin chose MIT and wants only Core opened for now.

## Decision

Decided with Justin 2026-11-09: Chauffeur Core's own source and documentation use the MIT license.
Third-party code retains its existing licenses and notices. Contributions to Core use MIT too.
The Rust and JavaScript package metadata declare MIT; package registry publication remains off.

The private Dialog distribution, Database and Office extensions, and mobile application are
outside this decision. Importing Core into a distribution must retain Core's license without
declaring the distribution's own code MIT. The number and arrangement of the private repositories
remain unchanged pending a separate decision.

## Consequences

Core carries a root LICENSE and third-party notices. Its README distinguishes Core from the
packaged Dialog distribution. This resolves the license left open by
[core-open-source-without-dialog-defaults](core-open-source-without-dialog-defaults.md).
Making the repository public is a separate publication step after reviewing the files and history
that GitHub would expose; adding the license does not change repository visibility.
