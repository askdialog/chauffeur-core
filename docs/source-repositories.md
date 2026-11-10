# Source repositories and distribution

This repository, Chauffeur Core, is the source of the daemon, CLI, sidecar, app UI, wire protocol
and collaboration services. It builds and runs on its own, with an empty extension registry and
no hosted service configured (see [SPEC.md](SPEC.md), "Hosted services").

Core's own code and documentation are [MIT-licensed](../LICENSE), with existing third-party
licenses preserved ([notices](../THIRD_PARTY_NOTICES.md)). Only Core is being opened: Dialog's
distribution, Database and Office implementations, and mobile app remain private. Their own code
is not covered by Core's license. See [the license decision](adr/core-mit-license.md).

Official extensions (Database, Office) and the iOS and Android companion live in their own
repositories. A distribution, such as Dialog's, packages a pinned Core with official extensions,
supplies its own default settings and build-time values (release repository, Sentry DSN), and
publishes the releases. See [extensions.md](extensions.md) and
[ADR core-open-source-without-dialog-defaults](adr/core-open-source-without-dialog-defaults.md).
