# ADRs are named by their slug, without a number

- **Date:** 2026-10-30
- **Status:** accepted

## Context

Each ADR took "the next number" when its pull request was written. On 2026-10-30 twelve ADRs (0040 to 0051) landed on `main` in one day. Pull requests opened in parallel picked the same number and conflicted on the index row in `README.md`: #371 was renumbered twice, #372 and #377 once, and #381 and #404 conflicted too. Each time, an agent or Justin had to rebase and renumber by hand. The daily review of 2026-10-30 raised it.

## Decision

A new ADR has no number: it is `docs/adr/<slug>.md`, and the SPEC and the other docs cite it by that path. No number is given later, neither at merge nor at release. ADRs 0001 to 0053 keep their numbers and their names. The index in `README.md` is generated from the files by `.github/scripts/adr_index.py`, sorted by date, and CI fails when it is stale or when a new ADR has a number. Decided with Justin 2026-10-30.

## Consequences

- Two pull requests can only conflict on an ADR if they pick the same slug, which git reports as an add/add conflict on that one file.
- Nothing pushes to `main` or renames a file after a merge, so a link to an ADR never changes.
- The index is no longer edited by hand: whoever adds or changes an ADR runs the script and commits its output.
- The index lists decisions by date (the first date of each ADR's **Date** line), no longer by number, and an ADR's title comes from its `# ` heading.
