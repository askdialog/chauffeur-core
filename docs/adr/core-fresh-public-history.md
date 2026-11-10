# Core starts with a new GitHub repository and a single initial commit

- **Date:** 2026-11-10
- **Status:** accepted

## Context

The first Core extraction still included the mobile application and private infrastructure
in older commits. Removing those files from the current tree did not remove them from GitHub
branches or pull-request references. Justin asked to discard that history before opening Core.

## Decision

Recreate `askdialog/chauffeur-core` from the reviewed current source with one root commit,
including the MIT license and revised CI. Do not import old branches, tags, pull requests or
other historical references. Do not keep a separate GitHub history repository. The replacement
stays private until publication; losing the old history is intentional (Justin, 2026-11-10).

## Consequences

Existing clones and workspaces must start their next work from the new main. Old branches must
not be pushed into the replacement repository; carry any unfinished changes across as patches.
Source consumers must update their Core pins when they next import the source. Their existing
vendored copies still build independently. The distribution and mobile source stay private.

A local recovery bundle is a temporary migration precaution, not part of the published source.
The initial commit has no parent; neither it nor the new repository includes the old Git history.
