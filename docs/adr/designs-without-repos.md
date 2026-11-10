# Create a design without repos

- **Date:** 2026-11-06
- **Status:** accepted

## Context

Presentations and standalone designs do not need a source repository. Requiring one
blocks that work and makes users pick an unrelated repo.

## Decision

Decided with Justin 2026-11-06: allow only Design creation without a repo, with a warning
that the agent cannot read a design system from code without a repo. Workspaces
and explorations still require at least one repo.

## Consequences

The Design form offers an explicit choice to clear its repo selection and shows
the warning whenever none are selected. The daemon honors that choice without
adding default or ticket repos, and refuses it for other creation kinds. Existing
archive and restore rules apply. A repo may still be added later.
