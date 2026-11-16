# Make the interface foundations and recovery paths dependable

- **Date:** 2026-11-16
- **Status:** accepted

## Context

The UX/UI audit found modal focus escaping into the background, unnamed form controls,
low-contrast secondary text, a compressed settings search and unclear recovery states.
Justin asked to fix the audit's ten findings in one pull request, explicitly calling out
the settings search's padding and size.

## Decision

As requested by Justin on 2026-11-16:

- Dialogs contain focus, make their background inert, and restore the invoking control.
  Portalled pickers and nested dialogs remain usable. Form labels name their inputs.
- Secondary reading text meets 4.5:1 contrast on neutral surfaces, including custom dark themes.
- Settings search keeps its size while the sections scroll and searches options, with results
  opening and highlighting the relevant setting rather than leaving unrelated content visible.
- Optional GitHub setup does not displace the first prompt when the chosen local repo suffices.
- Running tasks fold by default when a chat pane is short or input is pending; users can still
  expand them. Pending answers and recent context retain room.
- The no-script state offers configuration guidance and a route to the relevant repo or agent
  draft, without starting an agent or process automatically.
- A disconnected app explains recovery and exposes technical details separately; retry preserves
  drafts and queued requests.
- Archived workspaces are named as archives, with a route back from the archive notification and
  retention guidance. Archiving remains immediate and restoration remains explicit.

## Consequences

These are improvements to existing desktop flows, not a mobile or cloud redesign. The audit's
unverified native mobile, cloud and rich Canvas/document scenarios remain separate work.
The implementation is verified with isolated data, component tests and browser checks.
