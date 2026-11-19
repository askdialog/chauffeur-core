# Questions on shared designs

- **Date:** 2026-11-19
- **Status:** accepted

## Context

Design reviewers had to use Slack to vote on variants they could already inspect and comment on in Chauffeur.

## Decision

The author can publish a single-choice question from the canvas by attaching screens as options. Each reviewer can locate those screens in the shared canvas, choose one option and submit a vote with an optional justification. After voting they see the totals and who picked each option. The author can see the results as votes arrive.

## Consequences

The share service stores published questions and votes under the active design link. The form can be filled before sharing, and publishing requires a live share. Published wording, choices and submitted votes are fixed. Variants are linked by screen path and keep their published labels if a screen is renamed or removed. Stopping the share ends the questions and removes their votes.
