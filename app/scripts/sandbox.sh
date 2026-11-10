#!/usr/bin/env bash
# Starts a throwaway chauffeurd with three local repos (alpha and beta by default, gamma) for UI
# development, in the style of scripts/e2e-cli.sh. Prints the environment to use:
#
#   eval "$(app/scripts/sandbox.sh)"   # starts the daemon, exports CHAUFFEUR_HOME
#   (cd app && bun run dev)            # the UI at http://localhost:1420 talks to it
#   app/scripts/sandbox.sh stop        # stops the daemon and deletes the sandbox
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/target/debug"

if [ "${1:-}" = "stop" ]; then
  : "${CHAUFFEUR_SANDBOX:?run eval \"\$(app/scripts/sandbox.sh)\" first}"
  CHAUFFEUR_HOME="$CHAUFFEUR_SANDBOX/home" "$BIN/chauffeur" daemon stop >/dev/null 2>&1 || true
  rm -rf "$CHAUFFEUR_SANDBOX"
  echo "unset CHAUFFEUR_HOME CHAUFFEUR_WORKSPACES_DIR CHAUFFEUR_REPOS_DIR CHAUFFEUR_SANDBOX"
  exit 0
fi

[ -x "$BIN/chauffeurd" ] || (cd "$ROOT" && cargo build -q -p chauffeurd -p chauffeur-cli) >&2
T=$(mktemp -d)
T=$(cd "$T" && pwd -P)
export CHAUFFEUR_HOME="$T/home" CHAUFFEUR_WORKSPACES_DIR="$T/workspaces" CHAUFFEUR_REPOS_DIR="$T/repos"
export GIT_AUTHOR_NAME=sandbox GIT_AUTHOR_EMAIL=sandbox@example.com
export GIT_COMMITTER_NAME=sandbox GIT_COMMITTER_EMAIL=sandbox@example.com

for name in alpha beta gamma; do
  mkdir -p "$T/seed/$name/.chauffeur" && git -C "$T/seed/$name" init -q -b main
  echo "# $name" > "$T/seed/$name/README.md"
  cat > "$T/seed/$name/.chauffeur/settings.toml" <<TOML
[scripts]
setup = "echo installing $name; sleep 1; echo $name ready"
archive = "echo archiving $name"

[scripts.run.server]
command = "echo serving $name on port \$CHAUFFEUR_PORT; while true; do echo \"tick \$(date +%T)\"; sleep 1; done"
default = true

[scripts.run.build]
command = "echo building $name; sleep 1; echo built"
TOML
  git -C "$T/seed/$name" add -A >&2 && git -C "$T/seed/$name" commit -q -m init >&2
  git clone -q --bare "$T/seed/$name" "$T/origin/$name.git" >&2
  git clone -q "$T/origin/$name.git" "$T/clones/$name" >&2
done
mkdir -p "$CHAUFFEUR_HOME/logs"
cat > "$CHAUFFEUR_HOME/settings.toml" <<TOML
github_org = ""
default_repos = ["alpha", "beta"]
branch_prefix = "sandbox"
clone_roots = ["$T/clones"]
port_range_start = 47200
daemon_port = 0
linear_enabled = false

# Offline: no connection to Dialog's office server.
[office]
url = ""

[repos.alpha]
path = "$T/clones/alpha"
[repos.beta]
path = "$T/clones/beta"
[repos.gamma]
path = "$T/clones/gamma"
TOML

CHAUFFEUR_PORT_OVERRIDE=0 nohup "$BIN/chauffeurd" >"$CHAUFFEUR_HOME/logs/chauffeurd.log" 2>&1 </dev/null &
for _ in $(seq 1 100); do
  [ -f "$CHAUFFEUR_HOME/daemon.json" ] && break
  sleep 0.1
done
[ -f "$CHAUFFEUR_HOME/daemon.json" ] || { echo "chauffeurd did not start; see $CHAUFFEUR_HOME/logs/chauffeurd.log" >&2; exit 1; }

echo "export CHAUFFEUR_SANDBOX='$T' CHAUFFEUR_HOME='$CHAUFFEUR_HOME' CHAUFFEUR_WORKSPACES_DIR='$CHAUFFEUR_WORKSPACES_DIR' CHAUFFEUR_REPOS_DIR='$CHAUFFEUR_REPOS_DIR'"
