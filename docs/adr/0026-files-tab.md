# 0026. Files tab: media, Finder and HTML preview

- **Date:** 2026-10-19 (also 2026-10-23, 2026-10-25)
- **Status:** accepted

## Context

Users want to watch what an agent made (a video, a PDF, an HTML page) and find it on disk without hunting for it.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 43:

> A video or a sound (MP4, MOV, WebM, MP3, M4A, AAC, WAV, Ogg, FLAC) plays in the tab, a PDF shows there in WebKit's PDF viewer, and each has "Open in default app" (`files.open`), so that I watch what an agent made without hunting for it in Finder (asked by Justin 2026-10-19).

From `SPEC.md`, story 43:

> An HTML file opens as source with a Preview button (Source to come back, as a markdown file's): rendered in the tab with its scripts and the files it loads by relative URLs, served on the design screens' origin (`preview_url` of `files.read`) and framed as a screen is, so that I see the page an agent made without opening a browser (asked by Justin 2026-10-23).

From `SPEC.md`, Files:

> `files.reveal` takes any file or folder under the roots (`files.read`'s rules), or the absolute path of one inside any Chauffeur workspace (the folder of the workspaces, symlinks resolved first, so a link to another workspace's notes works; asked by Justin 2026-10-25), and runs `open -R` on it, which selects it in Finder and runs nothing (`xdg-open` on its folder on Linux); a folder is shown by selecting its first visible entry, never by a plain `open`, which would launch a folder that is an app bundle; the mobile bridge does not forward it either.

## Consequences

`files.open` hands only a video, a sound or a PDF to the default app, never a type it would run; HTML previews are served on the design screens' origin; the mobile bridge forwards neither `files.open` nor `files.reveal`.
