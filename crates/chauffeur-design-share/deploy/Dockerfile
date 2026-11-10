# The design share service's image, built by Cloud Build from the folder deploy/gcp.sh gathers: a
# workspace of chauffeur-design-share, chauffeur-design and chauffeur-protocol only, the repository's
# Cargo.lock, and the reviewer page's build (web/) when there is one.
FROM rust:1.99-slim-trixie AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p chauffeur-design-share

FROM debian:trixie-slim
# Google's token-signing keys are fetched over TLS, checked against the system's roots.
RUN apt-get update -qq \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/chauffeur-design-share /usr/local/bin/chauffeur-design-share
COPY web /srv/chauffeur-design-share/web
RUN chmod -R a+rX /srv/chauffeur-design-share
USER 65532:65532
# Loopback only: Caddy terminates TLS in front of it. The Google client IDs come from the env;
# the data folder is a bind mount of the VM's disk.
ENTRYPOINT ["chauffeur-design-share", "--listen", "127.0.0.1:8090", "--data", "/var/lib/chauffeur-design-share", "--web", "/srv/chauffeur-design-share/web"]
