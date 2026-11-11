# Core releases

Core's public source, binary and tag suffix use one version. A `core-v0.37.0` release is built from sources declaring `0.37.0`; Core artifacts are named `Chauffeur-Core-0.37.0.dmg`, `chauffeur-core-0.37.0-linux-x64.tar.gz`, and `chauffeur-core_0.37.0_ubuntu24.04_amd64.deb`, each with a SHA-256 file.

Run **Build Core packages** to build and smoke-test the public checkout on macOS and Linux. It uploads workflow artifacts and does not publish a release. Coordinated publication is handled by the release pipeline after review. The reusable workflow pins the exact public commit and validates every version field before building. No private extension source or extension signing key is required. Apple credentials are optional for local or standalone builds: without them, macOS packages are signed ad hoc and not notarized.

Core and the distribution with additional components have different update channels. Core reads only `core-v*` releases and downloads only `*-core-*` / `Chauffeur-Core-*` assets. The existing `v*` releases and unqualified filenames are the Dialog distribution, not Core-only builds. Its proprietary components are not covered by Core's MIT license. In particular, the historical v0.36.0 is a distribution release whose underlying Core snapshot declared 0.30.0; that published tag has not been rewritten.

For packaging from source, use the public commit named by a Core release tag. The Ubuntu `.deb` targets Ubuntu 24.04; it is not an Arch Linux package. An AUR source recipe should build the public sources, while a binary recipe must select Core artifacts rather than the distribution downloads. Third-party tools retain their licenses as described in [the notices](../THIRD_PARTY_NOTICES.md).

Core and Dialog currently install the same application/CLI paths. Install one edition at a time. Updating either edition preserves its channel; changing editions is an explicit installation.
