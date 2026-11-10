# Open private Canvas screens on the phone

- **Date:** 2026-11-08
- **Status:** accepted

Settled with Justin on 2026-11-08, following Louis's feedback: the mobile companion
must open Canvas links from design conversations without publishing private designs.

A design conversation offers Open Canvas and a list of its screens. Local Markdown
links resolve against that design's enumerated screens; public HTTPS links keep
opening in the system browser. Stale local links fall back to the screen list.

The approved phone reads `design.canvas` and `design.resource` through its existing
authenticated encrypted session. The host replaces Mac preview capability URLs with
availability markers. The phone never gets the daemon's general filesystem-serving
capability. Resource reads remain inside that local design folder, use the desktop's
path/type rules and descriptor-based no-follow opening, and refuse nonregular files.
Moved designs are refused until their execution host can serve them safely.

Resources stream in 256 KiB chunks with a stable opened-file metadata version and a
16 MiB per-file limit. Two resource reads may run at once; ordinary browser loads
wait in a bounded queue. The phone serves them on loopback with a fresh per-view
capability and strict Host/Origin checks. Opaque sandbox origins may load these
resources with narrowly scoped CORS. HTML receives a restrictive sandbox/CSP, no
native messaging bridge or pairing credentials, and no persistent resource cache.
Leaving the view, backgrounding, changing the peer or losing authorization closes
its listener and sockets and discards late responses. No sharing/publication call
is involved. Native iPhone/Android validation remains required.
