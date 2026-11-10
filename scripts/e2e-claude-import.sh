#!/usr/bin/env bash
# Import a real public Claude Design without a signed-in browser or an agent session.
set -euo pipefail
cd "$(dirname "$0")/.."
ARTIFACT_URL=${1:?usage: scripts/e2e-claude-import.sh https://claude.ai/artifact/...}
cargo build -q -p chauffeurd -p chauffeur-cli
BIN="$PWD/target/debug"
TASK_TMP=$(mktemp -d)
TASK_TMP=$(cd "$TASK_TMP" && pwd -P)
for variable in $(env | sed -n 's/^\(CHAUFFEUR_[A-Z0-9_]*\)=.*/\1/p'); do unset "$variable"; done
export CHAUFFEUR_HOME="$TASK_TMP/home" CHAUFFEUR_WORKSPACES_DIR="$TASK_TMP/workspaces" CHAUFFEUR_REPOS_DIR="$TASK_TMP/repos"
export CHAUFFEUR_PORT_OVERRIDE=0 CHAUFFEURD="$BIN/chauffeurd"
cleanup() {
  "$BIN/chauffeur" daemon stop >/dev/null 2>&1 || true
  scripts/forget-transcripts.sh "$TASK_TMP"
  rm -rf "$TASK_TMP"
}
trap cleanup EXIT
mkdir -p "$CHAUFFEUR_HOME"
cat > "$CHAUFFEUR_HOME/settings.toml" <<'TOML'
github_sources = false
default_repos = []
linear_enabled = false
daily_review = { enabled = false }
[office]
url = ""
[design_share]
url = ""
TOML
"$BIN/chauffeur" daemon start
PARAMS=$(python3 -c 'import json,sys;print(json.dumps({"url":sys.argv[1]}))' "$ARTIFACT_URL")
echo "== preview. read a public Claude canvas without an account"
bun scripts/app-call.ts design.claude_preview "$PARAMS" > "$TASK_TMP/preview.json"
python3 -c 'import json,sys;p=json.load(open(sys.argv[1]));assert p["pages"] and p["screens"] > 0,p;print(p)' "$TASK_TMP/preview.json"
PARAMS=$(python3 -c 'import json,sys;print(json.dumps({"name":"claude-import","no_repos":True,"chat_kind":"design","explore":True,"design":{"kind":"claude","url":sys.argv[1]}}))' "$ARTIFACT_URL")
echo "== create. copy the design into local editable files"
bun scripts/app-call.ts workspace.create "$PARAMS" > "$TASK_TMP/created.json"
CHAT=$(python3 -c 'import json,sys;w=json.load(open(sys.argv[1]));assert w["workspace"]["state"]=="ready" and w["workspace"]["worktrees"]==[],w;assert w["chat"]["kind"]=="design",w;print(w["chat"]["id"])' "$TASK_TMP/created.json")
"$BIN/chauffeur" --json chats claude-import > "$TASK_TMP/chats.json"
python3 -c 'import json,sys;chats=json.load(open(sys.argv[1]))["chats"];assert len(chats)==1 and chats[0]["status"]=="idle" and not chats[0].get("agent_session_id"),chats' "$TASK_TMP/chats.json"
PARAMS=$(python3 -c 'import json,sys;print(json.dumps({"chat":sys.argv[1]}))' "$CHAT")
bun scripts/app-call.ts design.canvas "$PARAMS" > "$TASK_TMP/canvas.json"
python3 - "$TASK_TMP" <<'PY'
import json,pathlib,sys
root=pathlib.Path(sys.argv[1]);preview=json.loads((root/'preview.json').read_text());canvas=json.loads((root/'canvas.json').read_text())
assert canvas.get('error') is None,canvas
assert [p['name'] for p in canvas['pages']]==preview['pages'],canvas
screens=[s for p in canvas['pages'] for sec in p['sections'] for s in sec['screens']]
assert len(screens)==preview['screens'] and not canvas['unsorted'],canvas
for screen in screens:
    assert screen['url'] and (pathlib.Path(canvas['dir'])/screen['file']).is_file(),screen
    assert screen['width']>0 and screen['height']>0,screen
print(f"Imported {preview['title']}: {len(canvas['pages'])} pages, {len(screens)} editable screens")
PY
"$BIN/chauffeur" archive claude-import --force
echo "E2E Claude import OK"
