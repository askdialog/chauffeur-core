# Follow a participant on a shared design canvas

- **Date:** 2026-11-23
- **Status:** accepted

## Context

Bastien asked to show a colleague a sequence of design variants during a live review.
An anchored comment already takes readers to a particular detail.

## Decision

Decided with Justin on 2026-11-23. A participant on a shared desktop canvas can
choose **Follow** beside another participant's name. The canvas follows that
participant's view until the reader stops or navigates independently. Opening a
comment gives the reader control. **Resume following** stays available during
reading and rejoins the participant at their current view.

## Consequences

Presence and viewport updates belong to the live design share. Each participant
controls whom they follow, and leaving the design ends that session's presence.
