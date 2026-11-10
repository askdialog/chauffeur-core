# Import public Claude Design canvases

- **Date:** 2026-11-06
- **Status:** accepted

## Context

Designs and presentations started in Claude should remain editable in Chauffeur.
A public Claude Design canvas exposes an index of pages and HTML artboards, which
use a different manifest from Chauffeur's canvas.

## Decision

Decided with Justin 2026-11-06: the Design creation form accepts a public Claude
artifact link, previews its pages and screens, and copies the supported design
into an independent editable Chauffeur canvas. Verify the complete import with
the supplied artifact, including its rendering in the app.

## Consequences

Only Claude Design canvases with a pages/boards manifest are supported initially.
Page names, screen labels, dimensions, ordering and linked local assets are copied;
Claude comments and canvas controls are not. Importing without a prompt leaves
the design chat idle; a prompt or images starts its agent after the copy is ready. Missing required assets refuse the
import. An unavailable legacy support script is removed, with a warning that some
interactions may be lost. Other formats and private links explain how to export
HTML instead.

An installed Chrome or Chromium opens a temporary window to resolve the public
artifact's pinned asset URL in an isolated profile. This lets the normal browser
perform a site's verification; Chauffeur never solves a CAPTCHA automatically. No existing browser profile or login is used.
Downloaded files stay on the validated Claude artifact origin; imports bound
individual file size, total size, screen count and file count, reject unsafe paths
and do not follow HTTP redirects. Imported pages use the existing sandboxed
screens listener. The original Claude design is never changed or synchronized.
