#!/usr/bin/env bash
# End-to-end check of a whole workspace transfer between three throwaway daemons on this machine:
# a source, a destination and a controller keeping the journal, driven by `chauffeur cloud
# transfer` over their private sockets, one workspace then a batch of two whose second fails. No
# agent runs; nothing leaves the temporary folder.
# REAL=1 adds a real Claude Haiku chat (this machine's login, a few cents): it remembers a word on
# the source and must recall it on the destination. Its transcripts are forgotten afterwards.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q -p chauffeurd -p chauffeur-cli
(cd sidecar && bun run build >/dev/null)
ROOT=$PWD BIN="$PWD/target/debug"
# Short names: a Unix socket path must fit in about 100 bytes on macOS.
T=$(mktemp -d /tmp/ct.XXXXXX)
T=$(cd "$T" && pwd -P)
# Run from a Chauffeur workspace, the CLI would refuse to start these throwaway daemons.
unset CHAUFFEUR_WORKSPACE_ID
export CHAUFFEUR_PORT_OVERRIDE=0 CHAUFFEURD="$BIN/chauffeurd"
# A destination restores conversations into its agents' homes: keep them in the temporary folder,
# unless a real chat needs this machine's login (its transcripts are forgotten below).
# (Codex has no such variable: the daemon drops every CODEX_ one, and no Codex chat runs here.)
[ -n "${REAL:-}" ] || export CLAUDE_CONFIG_DIR="$T/claude"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
# Editors are looked for in an empty folder: nothing here opens the user's.
export CHAUFFEUR_APP_DIRS="$T/no-apps"

# `on <daemon> <command…>` runs the CLI against one daemon's home.
on() {
  local name=$1
  shift
  # The source's workspaces folder is named through a symlink, as a user's can be.
  local workspaces="$T/$name/workspaces"
  [ "$name" = src ] && workspaces="$T/via-src/workspaces"
  CHAUFFEUR_HOME="$T/$name/home" CHAUFFEUR_WORKSPACES_DIR="$workspaces" \
    CHAUFFEUR_REPOS_DIR="$T/$name/repos" "$BIN/chauffeur" "$@"
}
# Stops a command `on … &` started: the shell running the function, and the CLI under it.
# Waits until `attach` says it follows (its subscription is open), from its standard error.
following() { for _ in $(seq 1 60); do grep -q "^following" "$1" 2>/dev/null && return; sleep 0.25; done; cat "$1"; exit 1; }
stop_bg() { pkill -P "$1" 2>/dev/null || true; kill "$1" 2>/dev/null || true; wait "$1" 2>/dev/null || true; }
cleanup() {
  # The web servers a ports step started, and a preview held from the phone, should a step fail first.
  for pid in "${WEB:-}" "${DEV:-}" "${HELD:-}"; do [ -z "$pid" ] || kill "$pid" 2>/dev/null || true; done
  for name in src dst ctl; do on "$name" daemon stop >/dev/null 2>&1 || true; done
  # KEEP=1 leaves the folder for a look; then `scripts/forget-transcripts.sh <folder> && rm -rf <folder>`.
  [ -n "${KEEP:-}" ] && { echo "kept: $T"; return; }
  # Claude Code writes a conversation's title shortly after its turn: let it land first.
  [ -z "${REAL:-}" ] || { sleep 10; "$ROOT/scripts/forget-transcripts.sh" "$T" >/dev/null || true; }
  # A folder a step locked, should it fail before unlocking it.
  chmod -R u+rwx "$T" 2>/dev/null || true
  rm -rf "$T"
}
trap cleanup EXIT

ln -s "$T/src" "$T/via-src"
mkdir -p "$T/seed/alpha" && git -C "$T/seed/alpha" init -q -b main
echo alpha > "$T/seed/alpha/README.md"
git -C "$T/seed/alpha" add -A && git -C "$T/seed/alpha" commit -q -m init
git clone -q --bare "$T/seed/alpha" "$T/origin/alpha.git"
git clone -q "$T/origin/alpha.git" "$T/clones/alpha"
port=47200
for name in src dst ctl; do
  mkdir -p "$T/$name/home"
  cat > "$T/$name/home/settings.toml" <<TOML
github_org = ""
github_sources = false
default_repos = ["alpha"]
branch_prefix = "e2e"
clone_roots = ["$T/clones"]
port_range_start = $port
linear_enabled = false
TOML
  port=$((port + 100))
  # The source knows the destination as a machine of Settings → Transfer, and the controller.
  if [ "$name" = src ]; then
    cat >> "$T/src/home/settings.toml" <<TOML
[transfer]
controller = "$T/ctl/home/chauffeurd.sock"
[[transfer.machines]]
name = "vm"
socket = "$T/dst/home/chauffeurd.sock"
TOML
  fi
  # The source reminds of a workspace left idle on another machine after 4 s, not an hour.
  if [ "$name" = src ]; then
    CHAUFFEUR_IDLE_REMINDER_SECS=4 on "$name" daemon start >/dev/null
  else
    on "$name" daemon start >/dev/null
  fi
done

echo "== source. a workspace"
if [ -n "${REAL:-}" ]; then
  on src new --name moving --model haiku --prompt "Remember the word kiwi. Reply with the single word ok." >/dev/null
  on src --json wait moving --timeout 180 >/dev/null
else
  on src new --name moving >/dev/null
fi
echo "== check. a destination that cannot take the workspace refuses before anything stops"
# The source's daemon drives each transfer; the CLI follows it.
transfer() {
  on src cloud transfer "$1" --to vm
}
mkdir -p "$T/dst/workspaces/moving"
if transfer moving 2>"$T/refused"; then echo "the destination took a workspace whose folder it has"; exit 1; fi
grep -q "cannot take moving yet; nothing was stopped" "$T/refused"
grep -q "dst/workspaces/moving already exists" "$T/refused"
rmdir "$T/dst/workspaces/moving"
# Still the source's: a worktree can be added and removed, which a fenced workspace refuses.
on src worktree add --workspace moving --repo alpha --name probe >/dev/null
on src worktree remove probe --workspace moving >/dev/null
if [ -z "${REAL:-}" ]; then
  # The destination's Claude has no login here: a workspace with a Claude chat cannot go.
  on src new --name talky >/dev/null
  on src chat talky >/dev/null
  if transfer talky 2>"$T/refused"; then echo "a chat moved to a destination logged out of its agent"; exit 1; fi
  grep -q "Claude Code is not logged in on .*: run \`claude login\` there, then retry" "$T/refused"
fi

SOURCE_PATH=$(on src --json show moving | python3 -c 'import sys,json; print(json.load(sys.stdin)["path"])')
echo "== check. a link in no repo refuses the transfer before anything stops"
ln -s /etc/hosts "$SOURCE_PATH/stray-link"
if transfer moving 2>"$T/refused"; then echo "a workspace with a link in no repo moved"; exit 1; fi
grep -q "stray-link in the workspace folder is a link" "$T/refused" || { cat "$T/refused"; exit 1; }
grep -q "nothing was stopped" "$T/refused"
# Refused before the controller prepared it: cancelling it only ends it here.
on src cloud transfer moving --to vm --cancel | grep -q "^Cancelled; moving runs on the source again.$"
on src worktree add --workspace moving --repo alpha --name probe >/dev/null
on src worktree remove probe --workspace moving >/dev/null
rm "$SOURCE_PATH/stray-link"

echo "== check. an unsupported repo entry refuses before the source is fenced"
mv "$SOURCE_PATH/alpha/README.md" "$T/readme.saved"
mkfifo "$SOURCE_PATH/alpha/README.md"
if transfer moving 2>"$T/refused"; then echo "a workspace with a FIFO moved"; exit 1; fi
grep -q "README.md is not a regular file" "$T/refused"
grep -q "nothing was stopped" "$T/refused"
on src worktree add --workspace moving --repo alpha --name probe >/dev/null
on src worktree remove probe --workspace moving >/dev/null
rm "$SOURCE_PATH/alpha/README.md"
mv "$T/readme.saved" "$SOURCE_PATH/alpha/README.md"

echo "== source. an edit, a plan and an origin carrying a token (unreachable: set last)"
# The daemon reports a restored folder resolved.
SOURCE_REAL=$(cd "$SOURCE_PATH" && pwd -P)
mkdir -p "$SOURCE_PATH/alpha/.agents/skills" "$SOURCE_PATH/alpha/.claude/skills/review"
echo 'review skill' > "$SOURCE_PATH/alpha/.claude/skills/review/SKILL.md"
ln -s ../../.claude/skills/review "$SOURCE_PATH/alpha/.agents/skills/review"
ln -s missing "$SOURCE_PATH/alpha/dangling"
git -C "$SOURCE_PATH/alpha" add .agents .claude
echo "edited before the transfer" > "$SOURCE_PATH/alpha/README.md"
mkdir -p "$SOURCE_PATH/.chauffeur/plans" && echo "carry this plan" > "$SOURCE_PATH/.chauffeur/plans/next.md"
# A tool's output where the agents start, in no repo (Playwright MCP's snapshots): it travels too.
mkdir -p "$SOURCE_PATH/.playwright-mcp" && echo "- snapshot" > "$SOURCE_PATH/.playwright-mcp/page.yml"
git -C "$SOURCE_PATH/alpha" config remote.origin.url "https://x-access-token:secret@github.com/example/alpha.git"

echo "== interrupted. a transfer failing once the source stopped leaves it stopped; cancelled, it runs there again"
# The destination cannot create the workspace's folder: its restoration fails, after the seal.
chmod 555 "$T/dst/workspaces"
if transfer moving 2>"$T/failed"; then echo "a destination that cannot write took the workspace"; exit 1; fi
if on src worktree add --workspace moving --repo alpha --name probe 2>"$T/fenced"; then
  echo "a source stopped for a transfer ran a command"; exit 1
fi
grep -q "fenced for handoff" "$T/fenced"
on src cloud transfer moving --to vm --cancel | grep -q "^Cancelled; moving runs on the source again.$"
on src worktree add --workspace moving --repo alpha --name probe >/dev/null
on src worktree remove probe --workspace moving >/dev/null

echo "== resumed. failing again, the transfer goes on from where it stopped once the destination can write"
if transfer moving 2>"$T/failed"; then echo "a destination that cannot write took the workspace"; exit 1; fi
chmod 755 "$T/dst/workspaces"
# Its progress goes to standard error.
transfer moving 2>&1 | tee "$T/transfer.out"
grep -q "going on with the transfer left at" "$T/transfer.out"
grep -q "^Moved to $T/dst/workspaces/moving; the source keeps it read-only" "$T/transfer.out"
test "$(readlink "$T/dst/workspaces/moving/alpha/.agents/skills/review")" = ../../.claude/skills/review
test "$(readlink "$T/dst/workspaces/moving/alpha/dangling")" = missing
on src --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w.get("moved_to")=="vm", w'

echo "== destination. the workspace runs there, with its files and a credential-free origin"
MOVED="$T/dst/workspaces/moving"
on dst --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="ready", w'
test "$(cat "$MOVED/alpha/README.md")" = "edited before the transfer"
test "$(cat "$MOVED/.chauffeur/plans/next.md")" = "carry this plan"
test "$(cat "$MOVED/.playwright-mcp/page.yml")" = "- snapshot"
test "$(git -C "$MOVED/alpha" remote get-url origin)" = "https://github.com/example/alpha.git"
test "$(git -C "$MOVED/alpha" status --porcelain)" = "$(git -C "$SOURCE_PATH/alpha" status --porcelain)"
if [ -n "${REAL:-}" ]; then
  echo "== destination. the carried conversation resumes with its memory, asked from the source"
  # The source's daemon relays it to the machine the workspace runs on, its wait and its events.
  on src --json attach moving > "$T/relayed.jsonl" 2>"$T/relayed.err" &
  FOLLOW=$!
  following "$T/relayed.err"
  on src send moving "Which word did I ask you to remember? Reply with that word only." >/dev/null
  on src --json wait moving --timeout 180 | python3 -c 'import sys,json; t=json.load(sys.stdin)["last_text"]; print(t); assert "kiwi" in (t or "").lower()'
  sleep 1
  stop_bg "$FOLLOW"
  grep -qi kiwi "$T/relayed.jsonl"
  grep -q '"turn_ended"' "$T/relayed.jsonl"
fi
# Admitted there: it can be renamed (no chat: the source has no Claude login to take it back).
on dst rename moving Worked elsewhere >/dev/null

echo "== source. the workspace stays, read-only and marked moved"
test "$(cat "$SOURCE_PATH/alpha/README.md")" = "edited before the transfer"
# A machine declared by hand has no SSH entry: no editor of this machine opens it there.
if on src open moving 2>"$T/refused"; then echo "a workspace moved to a declared machine opened"; exit 1; fi
grep -q "moving runs on vm, a machine declared by hand: only a GCP worker's workspaces open in this machine's editors" "$T/refused" || { cat "$T/refused"; exit 1; }

echo "== relay. from the source, the moved workspace's chats are the destination's"
ids() { python3 -c 'import sys,json; print(sorted(c["id"] for c in json.load(sys.stdin)["chats"]))'; }
test "$(on src --json chats moving --all | ids)" = "$(on dst --json chats moving --all | ids)"
if [ -n "${REAL:-}" ]; then
  # A chat this machine has no row of: created, renamed and deleted there through this daemon.
  # (Without a Claude login here, a chat left there would refuse the way back.)
  NEW=$(on src --json chat moving --model haiku | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
  on dst --json chats moving | grep -q "$NEW"
  on src chat rename "$NEW" relayed >/dev/null
  on dst --json chats moving | python3 -c 'import sys,json; assert any(c["id"]=="'"$NEW"'" and c["title"]=="relayed" for c in json.load(sys.stdin)["chats"])'
  on src chat delete "$NEW" --yes >/dev/null
  if on dst --json chats moving --all | grep -q "$NEW"; then echo "the relayed delete left the chat"; exit 1; fi
fi

echo "== relay. the moved workspace's events and unread mark are the destination's"
# Its history, as the destination numbered it (a worktree added and removed there writes some).
on dst worktree add --workspace moving --repo alpha --name probe >/dev/null
on dst worktree remove --workspace moving probe >/dev/null
history() {
  on "$1" --json attach moving --history > "$T/history-$1.jsonl" 2>"$T/history-$1.err" &
  local pid=$!
  following "$T/history-$1.err"
  sleep 1
  stop_bg "$pid"
  python3 -c 'import sys,json; print(sorted(e["seq"] for e in map(json.loads, open(sys.argv[1])) if e.get("seq")))' "$T/history-$1.jsonl"
}
HISTORY=$(history dst)
test "$HISTORY" != "[]"
test "$(history src)" = "$HISTORY"
# Marked from here, as the app does: the mark is the destination's, and shows here.
unread_on() { on "$1" --json show moving | python3 -c 'import sys,json; print(json.load(sys.stdin)["unread"])'; }
until_unread() {
  for _ in $(seq 1 40); do [ "$(unread_on src)" = "$1" ] && return; sleep 0.5; done
  echo "the source still shows unread=$(unread_on src), not $1"; exit 1
}
CHAUFFEUR_HOME="$T/src/home" bun "$ROOT/scripts/app-call.ts" workspace.seen '{"workspace": "moving"}' >/dev/null
test "$(unread_on dst)" = False
until_unread False
CHAUFFEUR_HOME="$T/src/home" bun "$ROOT/scripts/app-call.ts" workspace.mark_unread '{"workspace": "moving"}' >/dev/null
test "$(unread_on dst)" = True
until_unread True
# The destination stops: a call says so, a subscription says so and goes on once it is back.
on src attach moving > "$T/follow.out" 2>&1 &
FOLLOW=$!
following "$T/follow.out"
on dst daemon stop >/dev/null
if on src chats moving 2>"$T/down"; then echo "a call reached a stopped destination"; exit 1; fi
grep -q "vm does not answer" "$T/down"
# Followed while it does not answer, as the app opening the workspace: said at once.
on src attach moving > "$T/late.out" 2>&1 &
LATE=$!
following "$T/late.out"
for _ in $(seq 1 20); do grep -q "vm does not answer" "$T/late.out" && break; sleep 0.25; done
stop_bg "$LATE"
grep -q "vm does not answer" "$T/late.out" || { cat "$T/late.out"; exit 1; }
for _ in $(seq 1 20); do grep -q "vm does not answer" "$T/follow.out" && break; sleep 0.5; done
grep -q "vm does not answer" "$T/follow.out"
on dst daemon start >/dev/null
for _ in $(seq 1 80); do grep -q "vm answers again" "$T/follow.out" && break; sleep 0.5; done
stop_bg "$FOLLOW"
grep -q "vm answers again" "$T/follow.out" || { cat "$T/follow.out"; exit 1; }
test "$(grep -c "does not answer" "$T/follow.out")" = 1
on src --json chats moving >/dev/null

echo "== relay. its worktrees, files, diff, terminals, images and models are the destination's"
on src worktree add --workspace moving --repo alpha --name probe >/dev/null
on dst --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert any("probe" in t["path"] and not t["removed"] for t in w["worktrees"]), w["worktrees"]'
on src worktree remove --workspace moving probe >/dev/null
on dst --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert not any("probe" in t["path"] and not t["removed"] for t in w["worktrees"]), w["worktrees"]'
echo "written there" > "$MOVED/alpha/there.md"
on src --json files moving | grep -q "there.md"
on src --json diff moving --stat | grep -q "README.md"
# A terminal opened from here runs there, its input too.
app() { CHAUFFEUR_HOME="$T/src/home" bun "$ROOT/scripts/app-call.ts" "$@"; }
TERM_ID=$(app terminal.open '{"workspace": "moving", "cols": 80, "rows": 24}' | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
app terminal.input "{\"terminal\": \"$TERM_ID\", \"data\": \"echo from-the-source > typed-here.txt\\n\"}" >/dev/null
for _ in $(seq 1 40); do [ -n "$(find "$MOVED" -name typed-here.txt 2>/dev/null)" ] && break; sleep 0.25; done
grep -q from-the-source "$(find "$MOVED" -name typed-here.txt | head -1)"
app terminal.close "{\"terminal\": \"$TERM_ID\"}" >/dev/null
# Opening one of its files with an app of this machine is refused: its copy here is not the one that runs.
if app files.open '{"workspace": "moving", "path": "alpha/README.md"}' 2>"$T/opened"; then echo "a moved file opened here"; exit 1; fi
grep -q "its files are there" "$T/opened"
# An HTML page written there previews here: its rendering, and what it loads, read there.
printf '<link rel="stylesheet" href="page.css"><p>rendered there</p>\n' > "$MOVED/alpha/page.html"
echo "p { color: teal }" > "$MOVED/alpha/page.css"
PREVIEW=$(app files.read '{"workspace": "moving", "path": "alpha/page.html"}' | python3 -c 'import sys,json; print(json.load(sys.stdin)["preview_url"] or "")')
case "$PREVIEW" in http://127.0.0.1:*) ;; *) echo "no preview here: $PREVIEW"; exit 1 ;; esac
curl -sf "$PREVIEW" | grep -q "rendered there"
curl -sf "${PREVIEW%page.html}page.css" | grep -q teal
rm "$MOVED/alpha/page.html" "$MOVED/alpha/page.css"
# An image an answer shows, read there through this daemon's own HTTP route.
python3 -c 'import base64,sys; open(sys.argv[1],"wb").write(base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="))' "$MOVED/alpha/pixel.png"
SRC_PORT=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["port"])' "$T/src/home/daemon.json")
SRC_TOKEN=$(cat "$T/src/home/token")
curl -sf -o "$T/pixel.png" -w '%{content_type}' "http://127.0.0.1:$SRC_PORT/images/moving?path=alpha/pixel.png&token=$SRC_TOKEN" | grep -q image/png
cmp -s "$T/pixel.png" "$MOVED/alpha/pixel.png"
rm "$MOVED/alpha/pixel.png" "$MOVED/alpha/there.md" "$(find "$MOVED" -name typed-here.txt | head -1)"
on src --json models --workspace moving | python3 -c 'import sys,json; assert "agents" in json.load(sys.stdin)'
# A run script started from here runs there, and stops from here by its id alone.
mkdir -p "$MOVED/alpha/.chauffeur" && printf '[scripts]\nrun = "sleep 300"\n' > "$MOVED/alpha/.chauffeur/settings.toml"
on src run moving alpha >/dev/null
PROC=$(on dst --json ps moving | python3 -c 'import sys,json; print([p["id"] for p in json.load(sys.stdin)["processes"] if p["running"]][0])')
on src stop "$PROC" >/dev/null
for _ in $(seq 1 40); do
  on dst --json ps moving | python3 -c 'import sys,json; sys.exit(any(p["running"] for p in json.load(sys.stdin)["processes"]))' && break
  sleep 0.25
done
on dst --json ps moving | python3 -c 'import sys,json; assert not any(p["running"] for p in json.load(sys.stdin)["processes"])'
rm -r "$MOVED/alpha/.chauffeur"

echo "== relay. its ports here reach its dev servers there"
port_base() { on "$1" --json show moving | python3 -c 'import sys,json; print(json.load(sys.stdin)["port_base"])'; }
SRC_BASE=$(port_base src)
DST_BASE=$(port_base dst)
# Its ports there are the destination's own, and its notes for agents say so.
test "$DST_BASE" != 0
grep -q "$DST_BASE" "$MOVED/CLAUDE.md"
mkdir -p "$T/web" && echo "served there" > "$T/web/index.txt"
python3 -m http.server "$DST_BASE" --bind 127.0.0.1 --directory "$T/web" >/dev/null 2>&1 &
WEB=$!
# The source opens its listeners within a few seconds of the move.
for _ in $(seq 1 60); do curl -sf "http://127.0.0.1:$SRC_BASE/index.txt" >/dev/null && break; sleep 0.25; done
test "$(curl -sf "http://127.0.0.1:$SRC_BASE/index.txt")" = "served there"
# A client that sends its request then shuts its writes still reads the answer.
python3 - "$SRC_BASE" <<'PY'
import socket, sys
s = socket.create_connection(("127.0.0.1", int(sys.argv[1])))
s.sendall(b"GET /index.txt HTTP/1.0\r\n\r\n")
s.shutdown(socket.SHUT_WR)
answer = b""
while chunk := s.recv(65536):
    answer += chunk
assert b"served there" in answer, answer
PY
kill "$WEB"; wait "$WEB" 2>/dev/null || true
# Nothing listens there any more: refused here as well.
if curl -sf "http://127.0.0.1:$SRC_BASE/index.txt" >/dev/null; then echo "a closed port answered"; exit 1; fi

echo "== reminder. left idle there, the moved workspace is reminded of; a dev server there does not count"
idle_since() { on src --json show moving | python3 -c 'import sys,json; print(json.load(sys.stdin).get("idle_since") or "")'; }
until_idle() {
  for _ in $(seq 1 60); do [ -n "$(idle_since)" ] && return; sleep 0.25; done
  echo "the source never showed the moved workspace idle"; exit 1
}
reminders() { grep -c "moved workspaces idle: reminder sent" "$T/src/home/logs/chauffeurd.log" || true; }
mkdir -p "$MOVED/alpha/.chauffeur" && printf '[scripts]\nrun = "sleep 300"\n' > "$MOVED/alpha/.chauffeur/settings.toml"
SENT=$(reminders)
on src run moving alpha >/dev/null
# Started from here, the script is the user acting: not idle until the 4 s pass again.
test -z "$(idle_since)"
until_idle
test "$(reminders)" -gt "$SENT"
# Reads a client makes on its own leave it idle; asked to be reminded later, it no longer shows
# idle, and does again once that time passed.
on src --json chats moving >/dev/null
test -n "$(idle_since)"
app workspace.remind_later '{"workspaces": ["moving"]}' >/dev/null
test -z "$(idle_since)"
until_idle
# Acting on it from here (a terminal opened) clears it.
TERM_ID=$(app terminal.open '{"workspace": "moving", "cols": 80, "rows": 24}' | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
test -z "$(idle_since)"
app terminal.close "{\"terminal\": \"$TERM_ID\"}" >/dev/null
# Ignoring the reminder stopped nothing there: its script still runs.
on dst --json ps moving | python3 -c 'import sys,json; assert any(p["running"] for p in json.load(sys.stdin)["processes"])'
PROC=$(on dst --json ps moving | python3 -c 'import sys,json; print([p["id"] for p in json.load(sys.stdin)["processes"] if p["running"]][0])')
on src stop "$PROC" >/dev/null
rm -r "$MOVED/alpha/.chauffeur"
# Only a workspace that runs elsewhere has a reminder to put off.
on src new --name local >/dev/null
if app workspace.remind_later '{"workspaces": ["local"]}' 2>"$T/snooze"; then echo "a local workspace was snoozed"; exit 1; fi
grep -q "runs on this machine" "$T/snooze"
on src archive local >/dev/null

echo "== phone. a declared preview of the moved workspace opens on the phone through the Mac only"
bun "$ROOT/scripts/e2e-phone-preview.ts" serve "$DST_BASE" >/dev/null 2>&1 &
DEV=$!
for _ in $(seq 1 40); do curl -s -o /dev/null "http://127.0.0.1:$DST_BASE/" && break; sleep 0.25; done
# Detected there from here, the port that answers HTTP; declared, by its place in the block.
on src --json services moving detect | python3 -c 'import sys,json; p=json.load(sys.stdin)["previews"]; assert [(x["port"],x["listening"],x["declared"]) for x in p]==[('"$DST_BASE"',True,False)], p'
on src services moving save --preview "web=$DST_BASE" | grep -q "^preview web  port $DST_BASE$"
# Listed here on this machine's port that reaches it, with what its port does there.
app previews.list '{"workspace": "moving"}' | python3 -c 'import sys,json; p=json.load(sys.stdin); assert p["machine"]=="vm" and [(x["name"],x["port"],x["state"]) for x in p["previews"]]==[("web",'"$SRC_BASE"',"ready")], p'
# Through the phone's loopback, the mobile host and the Mac: the login's redirects and session
# cookie, the live-update WebSocket; a port not declared is refused.
SRC_SOCKET="$T/src/home/chauffeurd.sock"
bun "$ROOT/scripts/e2e-phone-preview.ts" check "$SRC_SOCKET" moving
# A connection held open from the phone, which the return below must cut.
bun "$ROOT/scripts/e2e-phone-preview.ts" hold "$SRC_SOCKET" moving >"$T/held" 2>&1 &
HELD=$!
for _ in $(seq 1 40); do grep -q "^open" "$T/held" && break; sleep 0.25; done
grep -q "^open" "$T/held" || { cat "$T/held"; exit 1; }

echo "== again. a second transfer of the moved workspace is refused before anything stops"
# Named with --from, the source is reached without this CLI's own daemon, which never starts.
if CHAUFFEUR_HOME="$T/none" "$BIN/chauffeur" cloud transfer moving --from "$T/src/home/chauffeurd.sock" \
  --to "$T/dst/home/chauffeurd.sock" --controller "$T/ctl/home/chauffeurd.sock" \
  2>"$T/again"; then echo "the moved source was transferred twice"; exit 1; fi
grep -q "already has workspace moving" "$T/again"
test ! -e "$T/none"

echo "== back. a file written in the retired copy since it moved refuses the return before anything stops"
echo "edited on the destination" > "$MOVED/alpha/README.md"
echo "written on the source after the move" > "$SOURCE_PATH/alpha/local.md"
# Brought back from here, as the app does: this daemon drives it, from the machine it went to.
if on src cloud transfer moving --back 2>"$T/edited"; then
  echo "the return overwrote an edited copy"; exit 1
fi
grep -q "alpha/local.md.*--keep-local-copy" "$T/edited"
# Told apart without reading its text: the app offers to bring it back keeping that copy.
on src --json cloud transfer moving --status | python3 -c 'import sys,json; t=json.load(sys.stdin); t=t.get("transfer", t); assert t["phase"]=="failed" and t.get("local_copy_changed") is True, t'
on dst --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="ready", w'

echo "== back. asked to keep it, the workspace comes back and the edited copy stays beside it"
on src cloud transfer moving --back --keep-local-copy | tee "$T/back.out"
grep -q "^Back in $SOURCE_REAL." "$T/back.out"
grep -q "kept at $SOURCE_REAL-local-copy" "$T/back.out"
test "$(cat "$SOURCE_PATH-local-copy/alpha/local.md")" = "written on the source after the move"
test "$(cat "$SOURCE_PATH-local-copy/alpha/README.md")" = "edited before the transfer"
test ! -e "$SOURCE_PATH/alpha/local.md"
test "$(cat "$SOURCE_PATH/alpha/README.md")" = "edited on the destination"
test "$(cat "$SOURCE_PATH/.chauffeur/plans/next.md")" = "carry this plan"
test "$(cat "$SOURCE_PATH/.playwright-mcp/page.yml")" = "- snapshot"
on src --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["title"]=="Worked elsewhere" and w["state"]=="ready", w'
test -z "$(find "$T/src/workspaces" -mindepth 1 -maxdepth 1 -name '.moving.*')"
on src chat moving >/dev/null

echo "== back. its ports here are no longer relayed"
for _ in $(seq 1 60); do curl -s -o /dev/null "http://127.0.0.1:$SRC_BASE/" || break; sleep 0.25; done
if curl -s -o /dev/null "http://127.0.0.1:$SRC_BASE/"; then echo "a returned workspace's port is still relayed"; exit 1; fi

echo "== back. its preview is closed: the connection the phone held is cut, a new one refused"
for _ in $(seq 1 60); do grep -q "^closed" "$T/held" && break; sleep 0.25; done
grep -q "^closed" "$T/held" || { echo "the phone's preview outlived the return"; cat "$T/held"; exit 1; }
wait "$HELD" 2>/dev/null || true
# Its declaration came back with it; here nothing listens once the port relay, which listened
# on this machine's port while it was away, has closed (within 5 s of the return).
returned_preview() { app previews.list '{"workspace": "moving"}' | python3 -c 'import sys,json; p=json.load(sys.stdin); assert p["machine"] is None and [(x["name"],x["state"]) for x in p["previews"]]==[("web","stopped")], p' 2>"$T/preview-state"; }
for _ in $(seq 1 60); do returned_preview && break; sleep 0.25; done
returned_preview || { echo "the returned workspace's preview still answers here"; cat "$T/preview-state"; exit 1; }
if bun "$ROOT/scripts/e2e-phone-preview.ts" check "$SRC_SOCKET" moving 2>"$T/refused-preview"; then echo "a returned workspace's preview opened"; exit 1; fi
kill "$DEV"; wait "$DEV" 2>/dev/null || true

echo "== cleanup. the machine it left deleted its copy; no transfer's package or archive is left"
if on dst show moving >/dev/null 2>&1; then echo "the copy it left is still there"; exit 1; fi
test ! -e "$MOVED"
test -z "$(ls -A "$T/dst/home/restorations")"
for name in src dst; do
  test -z "$(find "$T/$name/home/transfer-packages" "$T/$name/home/workspace-archives" -mindepth 1 -maxdepth 1 2>/dev/null)"
  test -z "$(ls -d "$T/$name/home"/workspace-preparation-* 2>/dev/null || true)"
done
if [ -n "${REAL:-}" ]; then
  # The conversation the destination's restoration wrote into its Claude home went with the copy.
  session=$(on src --json chats moving | python3 -c 'import sys,json; print(json.load(sys.stdin)["chats"][0]["agent_session_id"])')
  test -z "$(ls -d "$HOME"/.claude/projects/*dst-workspaces-moving/"$session".jsonl 2>/dev/null || true)"
  echo "== back. the conversation carried twice still remembers"
  on src send moving "Which word did I ask you to remember? Reply with that word only." >/dev/null
  on src --json wait moving --timeout 180 | python3 -c 'import sys,json; t=json.load(sys.stdin)["last_text"]; print(t); assert "kiwi" in (t or "").lower()'
fi

echo "== round trip. a workspace goes and comes back unedited here, its copy there pending deletion"
on src new --name trip >/dev/null
TRIP=$(on src --json show trip | python3 -c 'import sys,json; print(json.load(sys.stdin)["path"])')
TRIP_REAL=$(cd "$TRIP" && pwd -P)
transfer trip >/dev/null 2>"$T/trip.out" || { cat "$T/trip.out"; exit 1; }
AWAY_PATH="$T/dst/workspaces/trip"
echo "done away" > "$AWAY_PATH/alpha/away.md"
git -C "$AWAY_PATH/alpha" add away.md
git -C "$AWAY_PATH/alpha" commit -q -m "worked on the destination"
AWAY=$(git -C "$AWAY_PATH/alpha" rev-parse HEAD)
BRANCH=$(git -C "$AWAY_PATH/alpha" branch --show-current)
echo "staged away" > "$AWAY_PATH/alpha/staged.md" && git -C "$AWAY_PATH/alpha" add staged.md
# The copy it leaves there cannot be deleted at once: an ignored folder the daemon cannot empty.
EXCLUDE="$(git -C "$AWAY_PATH/alpha" rev-parse --path-format=absolute --git-common-dir)/info"
mkdir -p "$EXCLUDE" && echo "locked/" >> "$EXCLUDE/exclude"
LOCKED="$AWAY_PATH/alpha/locked"
mkdir -p "$LOCKED" && touch "$LOCKED/file" && chmod 500 "$LOCKED"
echo "== round trip. a return cancelled once the source stopped leaves the copy here replaceable"
# This machine cannot set its retired copy aside: the return fails after the seal, then is cancelled.
chmod 555 "$T/src/workspaces"
if on src cloud transfer trip --back 2>"$T/trip-failed"; then echo "a return replaced a copy it could not move"; exit 1; fi
chmod 755 "$T/src/workspaces"
on src cloud transfer trip --back --cancel | grep -q "^Cancelled; trip runs on the source again.$"
on dst --json show trip | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="ready", w'
on src --json cloud transfer trip --back 2>"$T/trip.err" >"$T/trip.json" || { cat "$T/trip.err"; exit 1; }
python3 -c 'import sys,json; t=json.load(open(sys.argv[1])); assert t["returned"] and "could not remove" in t.get("discard_pending",""), t' "$T/trip.json"

echo "== round trip. its worktree is the user's clone's again, with the commit and the change staged away"
test "$(git -C "$TRIP/alpha" rev-parse --path-format=absolute --git-common-dir)" = "$T/clones/alpha/.git"
test "$(git -C "$T/clones/alpha" rev-parse "$BRANCH")" = "$AWAY"
test "$(git -C "$TRIP/alpha" status --porcelain --untracked-files=no)" = "A  staged.md"
test "$(git -C "$T/clones/alpha" worktree list --porcelain | grep -c "^worktree $TRIP_REAL/alpha$")" = 1
# Its restoration, the latest, keeps no private clone (nor anything but its folder for exports).
test -z "$(ls -A "$(ls -td "$T"/src/home/restorations/*/ | head -1)/repos")"

echo "== round trip. the copy left there is said pending, then cleared once that machine deleted it"
on src cloud transfer trip --status | grep -q "^Its copy on vm is not deleted yet (.*); it is retried there.$"
on dst --json show trip >/dev/null
chmod 700 "$LOCKED"
# Each read asks the source again, at most every ten seconds.
for _ in $(seq 30); do
  on src --json cloud transfer trip --status | python3 -c 'import sys,json; sys.exit("discard_pending" in json.load(sys.stdin)["transfer"])' && break
  sleep 1
done
on src cloud transfer trip --status | grep -qx "Back from vm."
if on dst show trip >/dev/null 2>&1; then echo "the copy it left is still there"; exit 1; fi
test ! -e "$AWAY_PATH"

echo "== round trip. it archives as a local workspace: snapshots in the user's clone, worktree gone"
on src archive trip >/dev/null
test -z "$(git -C "$T/clones/alpha" worktree list --porcelain | grep "^worktree $TRIP_REAL/alpha$" || true)"
test -n "$(git -C "$T/clones/alpha" for-each-ref refs/chauffeur)"

echo "== batch. two workspaces go together; one the destination cannot take fails alone"
on src new --name batch-ok >/dev/null
on src new --name batch-blocked >/dev/null
mkdir -p "$T/dst/workspaces/batch-blocked"
if on src cloud transfer batch-ok batch-blocked --to vm >"$T/batch.out" 2>"$T/batch.err"; then
  echo "a batch with a workspace the destination refused went through"; exit 1
fi
cat "$T/batch.out"
grep -q "^batch-ok: Moved to $T/dst/workspaces/batch-ok;" "$T/batch.out"
grep -q "^batch-blocked: failed: .*cannot take batch-blocked yet; nothing was stopped" "$T/batch.out"
grep -q "1 of 2 transfers did not go through; the others did" "$T/batch.err"
# Each one's steps, said with its name.
grep -q "^batch-ok: · restoring it on the destination" "$T/batch.err"
on src --json show batch-ok | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w.get("moved_to")=="vm", w'
on dst --json show batch-ok | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="ready", w'
# The one that failed still runs here, untouched: nothing of it stopped.
on src --json show batch-blocked | python3 -c 'import sys,json; w=json.load(sys.stdin); assert not w.get("moved_to") and w["state"]=="ready", w'
on src worktree add --workspace batch-blocked --repo alpha --name probe >/dev/null
on src worktree remove probe --workspace batch-blocked >/dev/null
on src cloud transfer batch-ok batch-blocked --status > "$T/batch-status"
grep -q "^batch-ok: Moved to vm.$" "$T/batch-status"
grep -q "^batch-blocked: The transfer to vm failed: .*cannot take batch-blocked yet" "$T/batch-status"
# Once the destination can take it, it goes alone.
rmdir "$T/dst/workspaces/batch-blocked"
on src cloud transfer batch-blocked --to vm 2>/dev/null | grep -q "^Moved to $T/dst/workspaces/batch-blocked;"

echo "== relay. a chat made there, which this machine has no row of, is waited for by its id"
# `chauffeur wait|status <chat>` reaches the machine the workspace runs on. It stays there: its
# deletion would open a new chat, which this machine, logged out of Claude, could not take back.
THERE=$(on dst --json chat batch-ok | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
on src --json status "$THERE" | python3 -c 'import sys,json; assert any(c["id"]==sys.argv[1] for c in json.load(sys.stdin)["chats"])' "$THERE"

echo "== archive. a moved workspace archived from here comes back first, then is archived here"
on src archive batch-blocked >"$T/archive.out"
grep -q "coming back from vm first" "$T/archive.out" || { cat "$T/archive.out"; exit 1; }
for _ in $(seq 120); do
  on src --json show batch-blocked | python3 -c 'import sys,json; sys.exit(json.load(sys.stdin)["state"]!="archived")' && break
  sleep 1
done
on src --json show batch-blocked | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="archived" and not w.get("moved_to"), w'
on src cloud transfer batch-blocked --status | grep -qx "Back from vm; archived here."
# Back, its copy there is deleted; archived, its snapshot is in the user's clone here.
if on dst show batch-blocked >/dev/null 2>&1; then echo "the archived workspace's copy is still on vm"; exit 1; fi
BLOCKED=$(on src --json show batch-blocked | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
test -n "$(git -C "$T/clones/alpha" for-each-ref "refs/chauffeur/archive/$BLOCKED")"

echo "== archive. archiving a parent brings its moved child back first, then archives both here"
on src new --name elder >/dev/null
on src new --name kid --parent elder >/dev/null
transfer kid >/dev/null 2>"$T/kid.out" || { cat "$T/kid.out"; exit 1; }
on src archive elder >/dev/null
for _ in $(seq 120); do
  on src --json show kid | python3 -c 'import sys,json; sys.exit(json.load(sys.stdin)["state"]!="archived")' && break
  sleep 1
done
on src --json show elder | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="archived", w'
on src --json show kid | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="archived" and not w.get("moved_to"), w'
if on dst show kid >/dev/null 2>&1; then echo "the archived child's copy is still on vm"; exit 1; fi

echo "E2E CLOUD TRANSFER OK"
