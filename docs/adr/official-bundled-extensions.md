# Chauffeur Core with official bundled extensions

- **Date:** 2026-11-03
- **Status:** accepted

## Context

Database and the 3D Office enrich Chauffeur without being required to manage workspaces or run agents. Justin wants a Core and extensions, with an extension marketplace in Chauffeur.

## Decision

Decided with Justin 2026-11-03: Chauffeur provides official extensions delivered by Chauffeur. Database and Office are the first extensions, presented in an integrated marketplace.

The first implementation bundles them with the app and updates them with the same release. The marketplace enables and disables each extension on the local installation. Existing installations keep both enabled until the user changes them. There is no third-party code installation in this implementation.

## Consequences

The daemon owns the catalogue, persistent activation and request admission. Disabled extensions cannot execute their RPC operations; disabling Office stops its presence link and rewards. An extension with an operation in progress refuses activation changes until it finishes or the user cancels it, so disabling Database cannot interrupt a commit. Saved connections and preferences remain available on reactivation.

Core hosts extension views and keeps workspace, chat, terminal, process, identity and secret services. Google sign-in for sharing does not depend on Office. Extension activation changes reach all connected desktop clients, close disabled views and remove their split-pane targets.

The full application ships both modules, and a Core build can omit their implementations and UI. They are not independently distributed or isolated processes. Separate release channels, a third-party SDK and independently installable packages need further decisions.
