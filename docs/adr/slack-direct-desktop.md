# Open Slack context directly in the desktop

- **Date:** 2026-11-02
- **Status:** accepted

## Context

The Slack message shortcut opened a modal, followed by a browser handoff. Justin asked to skip the modal and open Chauffeur directly.

## Decision

Decided with Justin 2026-11-02: the message shortcut sends context to a connected Chauffeur desktop and brings its new-workspace form to the front without a Slack modal. Import the full thread by default, falling back to the selected message with a warning when Slack cannot read the thread. Nothing starts until the user submits the form.

## Consequences

The local app polls through its daemon while open. Authenticated, short-lived desktop presence chooses the last focused installation of the same Slack account among connected desktops; a pending import stays on that installation until acknowledged or expired. Retries do not append the same context twice. Without a reachable desktop, the existing private modal offers manual links and explains how to reconnect. The Slack service and desktop must both be updated; this uses the Slack service's existing authenticated HTTPS origin, separate from the opaque mobile relay.
