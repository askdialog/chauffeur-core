#!/usr/bin/env bash
# End-to-end chat through the daemon and the sidecar, with a real (cheap) Claude model.
# Needs Claude Code logged in on this machine and sidecar/dist/chauffeur-sidecar built.
set -euo pipefail
cd "$(dirname "$0")/.."
DAEMON_PACKAGE=chauffeurd
DAEMON_BINARY=chauffeurd
if [[ -f distribution/build-env.sh ]]; then
  # shellcheck source=/dev/null
  source distribution/build-env.sh
fi
cargo build -q -p "$DAEMON_PACKAGE" -p chauffeur-cli
ROOT=$PWD BIN="$PWD/target/debug"
T=$(mktemp -d); T=$(cd "$T" && pwd -P)
export CHAUFFEUR_HOME="$T/home" CHAUFFEUR_WORKSPACES_DIR="$T/workspaces" CHAUFFEUR_REPOS_DIR="$T/repos"
# Run from a Chauffeur workspace, the CLI would refuse to start this throwaway daemon.
unset CHAUFFEUR_WORKSPACE_ID
export CHAUFFEUR_PORT_OVERRIDE=0 CHAUFFEURD="$BIN/$DAEMON_BINARY"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
# Haiku 5.5: a cheap model with Claude Code's auto mode, the mode new chats start in. Haiku 3.5
# has no auto mode: its chats ask before acting, which the steps below cannot answer. (Claude Code
# 2.1.314 gives Haiku 4.5 auto mode: step 1a2 would then see no fallback to check.)
MODEL=${MODEL:-haiku}
NO_AUTO_MODEL=${NO_AUTO_MODEL:-claude-3-5-haiku-20241022}
# The chats' Claude Code transcripts go with the folder; with KEEP=1, remove both afterwards:
# `scripts/forget-transcripts.sh <folder> && rm -rf <folder>`.
cleanup() {
  "$BIN/chauffeur" daemon stop >/dev/null 2>&1 || true
  [ -n "${KEEP:-}" ] || { "$ROOT/scripts/forget-transcripts.sh" "$T" >/dev/null || true; rm -rf "$T"; }
}
trap cleanup EXIT
echo "throwaway folder: $T"
# gamma is in no workspace at first: an agent proposes it (step 3c).
for name in alpha beta gamma; do
  mkdir -p "$T/seed/$name" && git -C "$T/seed/$name" init -q -b main
  echo "# $name" > "$T/seed/$name/README.md"
  if [ "$name" = alpha ]; then
    # A repo's own skill: sessions start above the repos and must still see it.
    mkdir -p "$T/seed/alpha/.claude/skills/e2e-ping"
    printf -- '---\nname: e2e-ping\ndescription: Answers PONG-SKILL. A skill of repo alpha.\n---\nReply with exactly the word PONG-SKILL and nothing else.\n' \
      > "$T/seed/alpha/.claude/skills/e2e-ping/SKILL.md"
    # And one for Codex, which reads a repo's .agents/skills.
    mkdir -p "$T/seed/alpha/.agents/skills/e2e-codex-ping"
    printf -- '---\nname: e2e-codex-ping\ndescription: Answers PONG-CODEX. A Codex skill of repo alpha.\n---\nReply with exactly the word PONG-CODEX and nothing else.\n' \
      > "$T/seed/alpha/.agents/skills/e2e-codex-ping/SKILL.md"
  fi
  # A red 8×8 PNG, for the agent to look at.
  [ "$name" = alpha ] && python3 -c 'import zlib, struct, sys
def chunk(kind, data): return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
rows = b"".join(b"\x00" + b"\xff\x00\x00" * 8 for _ in range(8))
sys.stdout.buffer.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 8, 8, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))' > "$T/seed/alpha/pixel.png"
  git -C "$T/seed/$name" add -A && git -C "$T/seed/$name" commit -q -m init
  git clone -q --bare "$T/seed/$name" "$T/origin/$name.git"
  git clone -q "$T/origin/$name.git" "$T/clones/$name"
done
mkdir -p "$CHAUFFEUR_HOME"
cat > "$CHAUFFEUR_HOME/settings.toml" <<TOML
github_org = ""
github_sources = false
default_repos = ["alpha", "beta"]
branch_prefix = "e2e"
clone_roots = ["$T/clones"]
port_range_start = 47300
linear_enabled = false
daily_review = { enabled = false }
# The developer's own Ollama is never started for search.
search = { semantic = false }
[office]
url = ""
TOML
c() { "$BIN/chauffeur" "$@"; }
events() { sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select payload from events where chat_id = '$1' order by seq"; }

check_design_request() {
  local slug=$1 agent=$2 model=$3 source design
  c new --name "$slug" >/dev/null
  source=$(c --json chat "$slug" --agent "$agent" --model "$model" --prompt \
    'We are proposing a Reconnect button on the mobile login screen when the phone loses its Mac. Can you show me a mockup? A single screen is enough. Do not implement application code.' \
    | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
  # Routing is proved by a started designer and a served screen, even if its render checks
  # or the parent's follow-up are still running after the wait.
  if ! c --json wait "$slug" --timeout 300 > "$T/$slug-wait.json"; then
    python3 -c 'import sys,json; r=json.load(open(sys.argv[1])); assert r.get("timed_out"), r' "$T/$slug-wait.json"
  fi
  design=$(c --json chats "$slug" | python3 -c '
import sys,json
chats=json.load(sys.stdin)["chats"]
designs=[c for c in chats if c["kind"] == "design" and c.get("source_chat") == sys.argv[1]]
assert len(designs) == 1, ("the model did not create a design chat from its conversation", chats)
assert designs[0]["agent"] == sys.argv[2], designs
print(designs[0]["id"])' "$source" "$agent")
  events "$design" | python3 -c '
import sys,json
events=[json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
assert any(e["type"] == "user_message" for e in events), ("the model created a tab but did not send the design request", events)'
  events "$source" | python3 -c '
import sys,json
events=[json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
assert not any("\ue200visualize\ue202" in e.get("text", "") for e in events if e["type"] == "assistant_text"), "the model emitted an unsupported inline visualization"'
  c --json canvas "$design" | python3 -c '
import sys,json
canvas=json.load(sys.stdin)
assert any(screen.get("url") for p in canvas["pages"] for s in p["sections"] for screen in s["screens"]), ("the designer did not put a screen on the canvas", canvas)'
  c archive "$slug" --force >/dev/null
}

# Repeat just the request-to-canvas scenarios after changing their session instructions.
if [ "${DESIGN_REQUEST_ONLY:-}" = 1 ]; then
  check_design_request design-request claude "$MODEL"
  if [ -n "${CODEX:-}" ]; then
    check_design_request design-request-codex codex "${CODEX_MODEL:-gpt-6-luna}"
  fi
  echo "E2E DESIGN REQUEST OK"
  exit 0
fi

echo "== 1. a chat answers"
c new --name chat-one --model "$MODEL" --prompt "Reply with the single word pong and nothing else." >/dev/null
c --json wait chat-one --timeout 180 | python3 -c 'import sys,json; t=json.load(sys.stdin)["last_text"]; print(t); assert "pong" in (t or "").lower()'

echo "== 1-ctx. after a turn, the chat's context is read: from its session, by category, exactly on demand"
c --json context chat-one --full | python3 -c '
import sys, json
r = json.load(sys.stdin); u = r["usage"]
names = [c["name"] for c in u["categories"]]
print(u["percent"], "%", u["used_tokens"], "/", u["max_tokens"], names)
assert r["live"] and r["read_at"], r
assert u["percent"] > 0 and u["used_tokens"] > 0 and "Messages" in names, u
assert u["compaction"] and u["compaction"]["by"] == "claude_code", u
d = u["details"]
assert d is not None and isinstance(d["mcp_tools"], list) and isinstance(d["memory_files"], list), d'
CTX_CHAT=$(c --json chats chat-one | python3 -c 'import sys,json; print(json.load(sys.stdin)["chats"][0]["id"])')
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT payload FROM events WHERE chat_id = '$CTX_CHAT' AND payload LIKE '%\"type\":\"context_usage\"%'" | python3 -c '
import sys, json
readings = [json.loads(line)["event"]["event"]["usage"] for line in sys.stdin if line.strip()]
assert readings and all("details" not in r for r in readings), readings
print(len(readings), "reading(s) in the log")'

echo "== 1a. new chats start in auto mode: a harmless command runs without asking"
c new --name auto-one --model "$MODEL" --prompt "Run the shell command ls alpha with the Bash tool, then reply with the single word done." >/dev/null
c --json wait auto-one --timeout 180 | python3 -c '
import sys, json
r = json.load(sys.stdin); chat = r["chats"][0]
print(chat["permission_mode"], chat["status"], r["last_text"])
assert chat["permission_mode"] == "auto" and chat["status"] == "idle", chat
assert "done" in (r["last_text"] or "").lower()'
chat_id() { c --json chats "$1" | python3 -c 'import sys,json; print(json.load(sys.stdin)["chats"][0]["id"])'; }
events "$(chat_id auto-one)" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin]
types = [e["type"] for e in events]
assert "permission_request" not in types, types
calls = [e for e in events if e["type"] == "tool_call" and e["name"] == "Bash" and "ls" in e["input"].get("command", "")]
results = [e for e in events if e["type"] == "tool_result" and e["tool_use_id"] in {c["tool_use_id"] for c in calls}]
assert calls and results and not results[0]["is_error"], events
assert not [e for e in events if e["type"] == "notice" and "Auto mode" in e["message"]], events
print("ran", calls[0]["input"]["command"], "without asking")'
# Its worktrees would otherwise count among the clones' (step 4d).
c archive --force auto-one >/dev/null

echo "== 1a2. with a model without auto mode, the chat says that it asks instead"
c new --name no-auto --model "$NO_AUTO_MODEL" --prompt "Reply with the single word pong and nothing else." >/dev/null
c --json wait no-auto --timeout 180 >/dev/null
events "$(chat_id no-auto)" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin]
notices = [e["message"] for e in events if e["type"] == "notice" and "Auto mode is unavailable" in e["message"]]
assert notices, [e for e in events if e["type"] == "notice"]
print(notices[0])'
c archive --force no-auto >/dev/null

echo "== 1b. the chat resumes after a daemon restart, with its context"
c daemon stop >/dev/null
c daemon start >/dev/null
c send chat-one "Which single word did you reply with in your previous answer? Reply with that word only." >/dev/null
c --json wait chat-one --timeout 180 | python3 -c 'import sys,json; t=json.load(sys.stdin)["last_text"]; print(t); assert "pong" in (t or "").lower()'

echo "== 1b2. a turn cut by a killed daemon resumes on its own once it starts again"
c send chat-one "Run the shell command 'sleep 20 && echo hi > $T/resumed.txt' with the Bash tool, in the foreground, then reply with the single word written." >/dev/null
CUT_CHAT=$(c --json chats chat-one | python3 -c 'import sys,json; print(json.load(sys.stdin)["chats"][0]["id"])')
for _ in $(seq 1 90); do
  sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select 1 from events where chat_id = '$CUT_CHAT' and payload like '%\"tool_call\"%' and payload like '%resumed.txt%'" | grep -q 1 && break
  sleep 1
done
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select 1 from events where chat_id = '$CUT_CHAT' and payload like '%\"tool_call\"%' and payload like '%resumed.txt%'" | grep -q 1 \
  || { echo "the model did not call its Bash tool with the command"; exit 1; }
CUT_AFTER=$(sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select max(seq) from events")
kill -9 "$(python3 -c 'import sys,json; print(json.load(open(sys.argv[1]))["pid"])' "$CHAUFFEUR_HOME/daemon.json")"
sleep 1
c daemon start >/dev/null
c --json wait chat-one --timeout 240 >/dev/null
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select payload from events where chat_id = '$CUT_CHAT' and seq > $CUT_AFTER order by seq" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
notices = [e["message"] for e in events if e["type"] == "notice"]
ended = [e for e in events if e["type"] == "turn_ended"]
print(notices, [e["outcome"] for e in ended])
assert any(n.endswith("the agent resumes its turn") for n in notices), notices
assert not [e for e in events if e["type"] == "user_message"], "the resume showed as a message of the user"
assert ended and ended[-1]["outcome"] == "success", ended'
test -f "$T/resumed.txt" || { echo "the resumed agent did not write resumed.txt"; exit 1; }

echo "== 1c. a repo's skill is listed for / and runs"
c commands chat-one | tee "$T/commands.txt" | grep -E "^/e2e-ping"
! grep -qE "^/(model|clear) " "$T/commands.txt"
c send chat-one "/e2e-ping" >/dev/null
c --json wait chat-one --timeout 180 | python3 -c 'import sys,json; t=json.load(sys.stdin)["last_text"]; print(t); assert "PONG-SKILL" in (t or "")'

echo "== 1c2. an image the agent looks at is kept for the chat to show"
c send chat-one "Use the Read tool on alpha/pixel.png, then reply with the single word seen." >/dev/null
c --json wait chat-one --timeout 180 >/dev/null
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select payload from events where payload like '%\"tool_result\"%' and payload like '%\"attachments\"%'" | python3 -c '
import sys, json, os
saved = [a for line in sys.stdin for a in json.loads(line)["event"]["event"].get("attachments", [])]
assert saved and saved[0]["media_type"] == "image/png" and os.path.isfile(saved[0]["path"]), saved
print("kept", saved[0]["file"])
'

echo "== 1c2b. a file that is not an image is saved, and the agent opens it"
printf '<html><body><p>The code word is MANGO.</p></body></html>' > "$T/note.html"
c send chat-one --file "$T/note.html" "Open the attached file and reply with the code word it gives, nothing else." >/dev/null
c --json wait chat-one --timeout 180 | python3 -c 'import sys,json; t=json.load(sys.stdin)["last_text"]; print(t); assert "mango" in (t or "").lower()'
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select payload from events where payload like '%\"user_message\"%' and payload like '%note.html%'" | python3 -c '
import sys, json, os
saved = [a for line in sys.stdin for a in json.loads(line)["event"]["event"].get("attachments", [])]
assert saved and saved[0]["media_type"] == "text/html" and saved[0]["file"].endswith("-note.html") and os.path.isfile(saved[0]["path"]), saved
print("kept", saved[0]["file"])
'

echo "== 1c2c. a shell command (!) starts no turn, and the agent reads its output with the next message"
turns() { sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select count(*) from events where payload like '%\"turn_started\"%'"; }
BEFORE=$(turns)
SHELL_ID=$(bun "$ROOT/scripts/app-call.ts" chat.shell '{"chat": "chat-one", "command": "echo \"the code word is PAPAYA\""}' | python3 -c 'import sys,json; print(json.load(sys.stdin)["shell_id"])')
sleep 3
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select payload from events where payload like '%\"shell_ended\"%' and payload like '%$SHELL_ID%'" | python3 -c 'import sys,json; r=json.loads(sys.stdin.read())["event"]["event"]; print(r); assert r["exit_code"] == 0 and "PAPAYA" in r["output"], r'
test "$(turns)" = "$BEFORE" || { echo "a shell command started a turn"; exit 1; }
c send chat-one "Reply with the code word my last shell command printed, nothing else." >/dev/null
c --json wait chat-one --timeout 180 | python3 -c 'import sys,json; t=json.load(sys.stdin)["last_text"]; print(t); assert "papaya" in (t or "").lower(), "the agent did not get the shell output"'

echo "== 1c3. an idle session closes by itself, and the next message resumes it with its context"
c daemon stop >/dev/null
CHAUFFEUR_IDLE_SESSION_SECS=3 c daemon start >/dev/null
c send chat-one "Remember the word kiwi. Reply with the single word ok." >/dev/null
c --json wait chat-one --timeout 180 >/dev/null
for _ in $(seq 1 30); do grep -q "idle session closed" "$CHAUFFEUR_HOME/logs/chauffeurd.log" && break; sleep 1; done
grep -q "idle session closed" "$CHAUFFEUR_HOME/logs/chauffeurd.log"
# A log message alone is not durable exit evidence: the real sidecar must name the launch.
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT phase FROM workspace_agent_launches WHERE chat_id = '$(chat_id chat-one)' ORDER BY launch_id DESC LIMIT 1" | python3 -c 'import sys; phase=sys.stdin.read().strip(); print("Claude idle-close launch:",phase); assert phase == "stopped", phase'
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT count(*) FROM workspace_agent_roots r JOIN workspace_agent_launches l USING(launch_id) WHERE l.chat_id = '$(chat_id chat-one)'" | python3 -c 'import sys; count=int(sys.stdin.read()); print("Claude registered native roots:",count); assert count > 0, count'

# Closed: the last reading, and no session started for it.
c --json context chat-one --full | python3 -c '
import sys, json
r = json.load(sys.stdin); u = r["usage"]
assert not r["live"] and r["read_at"] and u["percent"] > 0 and "details" not in u, r'
c --json chats chat-one | python3 -c 'import sys,json; s=json.load(sys.stdin)["chats"][0]["status"]; assert s == "idle", s'
c send chat-one "Which word did I ask you to remember? Reply with that word only." >/dev/null
c --json wait chat-one --timeout 180 | python3 -c 'import sys,json; t=json.load(sys.stdin)["last_text"]; print(t); assert "kiwi" in (t or "").lower()'
c daemon stop >/dev/null
c daemon start >/dev/null

echo "== 1c3b. a chat forked into a new tab carries its conversation on, then goes its own way"
# The main agent's last text in a chat, from the event log.
last_text() { events "$1" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
texts = [e["text"] for e in events if e["type"] == "assistant_text" and not e.get("parent_tool_use_id")]
print(texts[-1] if texts else "")'; }
fork_chat() { c --json chat fork "$1" | python3 -c '
import sys, json
f = json.load(sys.stdin)
print(f["title"], f["agent_session_id"], file=sys.stderr)
assert f["agent_session_id"] and f["status"] == "idle" and f["title"].startswith("Fork"), f
print(f["id"])'; }
ORIG=$(chat_id chat-one)
FORK=$(fork_chat "$ORIG")
c send "$FORK" "Which word did I ask you to remember? Reply with that word only." >/dev/null
c --json wait chat-one --timeout 180 >/dev/null
last_text "$FORK" | python3 -c 'import sys; t=sys.stdin.read(); print(t); assert "kiwi" in t.lower(), "the fork did not carry the conversation on"'
c send "$FORK" "Forget kiwi: remember the word plum instead. Reply with the single word ok." >/dev/null
c --json wait chat-one --timeout 180 >/dev/null
c send "$ORIG" "Which word did I ask you to remember? Reply with that word only." >/dev/null
c --json wait chat-one --timeout 180 >/dev/null
last_text "$ORIG" | python3 -c 'import sys; t=sys.stdin.read(); print(t); assert "kiwi" in t.lower() and "plum" not in t.lower(), "the original heard what was said in the fork"'
c --json chats chat-one | python3 -c '
import sys, json
chats = {c["id"]: c for c in json.load(sys.stdin)["chats"]}
orig, fork = chats[sys.argv[1]], chats[sys.argv[2]]
print(orig["agent_session_id"], fork["agent_session_id"])
assert orig["agent_session_id"] and fork["agent_session_id"] and orig["agent_session_id"] != fork["agent_session_id"], chats' "$ORIG" "$FORK"

echo "== 1c3c. a chat forked from one of its turns keeps the conversation as it stood then"
# How many turns a chat has ended.
turns() { events "$1" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
print(sum(1 for e in events if e["type"] == "turn_ended"))'; }
fork_turn() { c --json chat fork "$1" --turn "$2" | python3 -c '
import sys, json
f = json.load(sys.stdin)
print(f["title"], f["agent_session_id"], file=sys.stderr)
assert f["agent_session_id"] and f["status"] == "idle", f
print(f["id"])'; }
# The fork's last turn said plum: from the turn before, kiwi is still the word.
TURN_FORK=$(fork_turn "$FORK" "$(( $(turns "$FORK") - 1 ))")
c send "$TURN_FORK" "Which word did I ask you to remember? Reply with that word only." >/dev/null
c --json wait chat-one --timeout 180 >/dev/null
last_text "$TURN_FORK" | python3 -c 'import sys; t=sys.stdin.read(); print(t); assert "kiwi" in t.lower() and "plum" not in t.lower(), "the turn fork kept what came after its turn"'
c chat close "$TURN_FORK" >/dev/null
# Later steps read the workspace's first open chat.
c chat close "$FORK" >/dev/null

echo "== 1c4. the plan's usage limits are read through the agents"
c usage --refresh
c --json usage | python3 -c '
import sys, json
agents = json.load(sys.stdin)["agents"]
claude = next(a for a in agents if a["agent"] == "claude")
assert claude["error"] is None and claude["read_at"], claude
codex = next(a for a in agents if a["agent"] == "codex")
resets = codex.get("reset_credits")
if resets is not None:
    assert isinstance(resets["available_count"], int) and resets["available_count"] >= 0, resets
    for credit in resets["credits"] or []:
        assert isinstance(credit["title"], str), credit
        assert credit["expires_at"] is None or isinstance(credit["expires_at"], str), credit
    print("Codex reset credits:", resets["available_count"])'

echo "== 1c5. a background command shows as a running task, is stopped through Chauffeur and ends"
# In a workspace of its own: Claude Code tells the agent about the stopped task with its next turn.
c new --name tasks-one --model "$MODEL" --prompt "Use the Bash tool with run_in_background set to true to run exactly: sleep 300 . Do not wait for it. Then reply with the single word started." >/dev/null
c --json wait tasks-one --timeout 180 >/dev/null
TASKS_CHAT=$(c --json chats tasks-one | python3 -c 'import sys,json; print(json.load(sys.stdin)["chats"][0]["id"])')
task_events() { sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT payload FROM events WHERE chat_id = '$TASKS_CHAT' AND payload LIKE '%\"type\":\"task_$1\"%' ORDER BY seq"; }
# The model may start the command more than once: each one still running is a task.
COMMANDS=$( (task_events started; task_events ended) | python3 -c '
import sys,json
events=[json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
ended={e["task_id"] for e in events if e["type"]=="task_ended"}
tasks=[e for e in events if e["type"]=="task_started"]
print(tasks, file=sys.stderr)
sleeping=[t["task_id"] for t in tasks if t["kind"]=="command" and t["background"] and t["task_id"] not in ended]
assert sleeping, tasks
print(" ".join(sleeping))')
N=$(echo "$COMMANDS" | wc -w | tr -d " ")
c --json chats tasks-one | python3 -c 'import sys,json; c=json.load(sys.stdin)["chats"][0]; assert c["running_tasks"] == '"$N"', c'
c --json show tasks-one | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["running_tasks"] == '"$N"', w["running_tasks"]'

echo "== 1c6. interrupting the turn leaves the background tasks running: the command, a subagent"
# A plain `sleep` in the foreground is refused by Claude Code: the subagent pings for 150 s instead.
c send tasks-one "Use the Agent tool with run_in_background set to true, subagent_type general-purpose, description 'e2e pinger', and this prompt: 'Use the Bash tool, in the foreground with timeout 300000, to run exactly: ping -c 150 127.0.0.1 > /dev/null . Then reply with the single word pinged.' Do not wait for it. Then reply with the single word started." >/dev/null
c --json wait tasks-one --timeout 180 >/dev/null
AGENT=$(task_events started | python3 -c '
import sys,json
tasks=[json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
agents=[t for t in tasks if t["kind"]=="subagent" and t["background"]]
assert agents, tasks
print(agents[-1]["task_id"])')
c --json chats tasks-one | python3 -c 'import sys,json; c=json.load(sys.stdin)["chats"][0]; assert c["running_tasks"] == '"$((N + 1))"', c'
echo "== 1c6b. the turn ended with the subagent still working: the chat is idle, its tasks run on"
for _ in $(seq 1 10); do
  c --json chats tasks-one | python3 -c 'import sys,json; c=json.load(sys.stdin)["chats"][0]; assert c["status"] == "idle" and c["running_tasks"] == '"$((N + 1))"', c'
  sleep 1
done
c --json show tasks-one | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["activity"] == "idle" and w["running_tasks"] == '"$((N + 1))"', (w["activity"], w["running_tasks"])'
# A turn long enough to be interrupted while it writes.
c send tasks-one "Without using any tool, write a 1500-word essay about lighthouses." >/dev/null
for _ in $(seq 1 30); do [ "$(c --json chats tasks-one | python3 -c 'import sys,json; print(json.load(sys.stdin)["chats"][0]["status"])')" = working ] && break; sleep 1; done
sleep 3
c interrupt tasks-one >/dev/null
for _ in $(seq 1 30); do sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT payload FROM events WHERE chat_id = '$TASKS_CHAT' AND payload LIKE '%\"outcome\":\"interrupted\"%'" | grep -q turn_ended && break; sleep 1; done
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT payload FROM events WHERE chat_id = '$TASKS_CHAT' AND payload LIKE '%\"outcome\":\"interrupted\"%'" | grep -q turn_ended
sleep 3
task_events ended | python3 -c '
import sys,json
ended=[json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
print(ended)
assert not [e for e in ended if e["task_id"] in "'"$COMMANDS $AGENT"'".split()], ended'
c --json chats tasks-one | python3 -c 'import sys,json; c=json.load(sys.stdin)["chats"][0]; assert c["running_tasks"] == '"$((N + 1))"', c'
for id in $COMMANDS "$AGENT"; do c stop-task tasks-one "$id" >/dev/null; done
for _ in $(seq 1 30); do [ "$(task_events ended | grep -cE "\"($(echo $COMMANDS $AGENT | tr " " "|"))\"")" -ge $((N + 1)) ] && break; sleep 1; done
task_events ended | python3 -c '
import sys,json
ended=[json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
print(ended)
for id in "'"$COMMANDS $AGENT"'".split():
    assert any(e["task_id"]==id and e["status"]=="stopped" for e in ended), (id, ended)'
c --json chats tasks-one | python3 -c 'import sys,json; c=json.load(sys.stdin)["chats"][0]; assert c["running_tasks"] == 0, c'
c archive --force tasks-one >/dev/null

echo "== 1c6c. a background subagent's request outlives an interrupt and is answered after it"
c new --name bg-ask --model "$MODEL" --permission-mode default --prompt "Use the Agent tool with run_in_background set to true, subagent_type general-purpose, description 'e2e greeter', and this prompt: 'Use the Bash tool to run exactly: echo hello > greeting.txt . Then reply with the single word written.' Do not wait for it. Then reply with the single word started." >/dev/null
BG_CHAT=$(chat_id bg-ask)
open_request() { events "$BG_CHAT" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
asked = [e for e in events if e["type"] == "permission_request" and (e.get("agent") or {}).get("background")]
resolved = {e["request_id"] for e in events if e["type"] == "request_resolved"}
print(next((e["request_id"] for e in asked if e["request_id"] not in resolved), ""))'; }
REQ=""
for _ in $(seq 1 90); do REQ=$(open_request); [ -n "$REQ" ] && break; sleep 1; done
[ -n "$REQ" ] || { echo "the background subagent never asked"; exit 1; }
# Waiting for the user: the app's Esc interrupts then. The turn ends, the request stays open.
c --json chats bg-ask | python3 -c 'import sys,json; c=json.load(sys.stdin)["chats"][0]; assert c["status"] == "needs_input", c'
c interrupt bg-ask >/dev/null
sleep 3
[ "$(open_request)" = "$REQ" ] || { events "$BG_CHAT" | grep request_resolved; echo "the interrupt cancelled the subagent's request"; exit 1; }
c respond bg-ask "$REQ" allow >/dev/null
for _ in $(seq 1 60); do [ -f "$T/workspaces/bg-ask/greeting.txt" ] && break; sleep 1; done
[ "$(cat "$T/workspaces/bg-ask/greeting.txt")" = hello ]
c archive --force bg-ask >/dev/null

echo "== 1c7. a subagent's conversation is read from its transcript; its text is never stored"
c new --name sub-one --model "$MODEL" --prompt "Use the Agent tool once, in the foreground, with subagent_type general-purpose, description 'e2e lighthouse', and this prompt: 'Without using any tool, reply with exactly this sentence: The lighthouse keeper counts the waves.' Then reply with the single word done." >/dev/null
c --json wait sub-one --timeout 240 >/dev/null
SUB_CHAT=$(chat_id sub-one)
SUB_CALL=$(events "$SUB_CHAT" | python3 -c '
import sys,json
calls=[e["event"]["event"] for e in map(json.loads, filter(str.strip, sys.stdin)) if e["event"]["event"]["type"]=="tool_call"]
agents=[c for c in calls if c["name"] in ("Agent","Task")]
assert agents, calls
print(agents[-1]["tool_use_id"])')
# Live, the subagent spoke through ephemeral events only: none of its text is in the log.
events "$SUB_CHAT" | python3 -c '
import sys,json
evs=[json.loads(l)["event"]["event"] for l in sys.stdin if l.strip()]
kept=[e for e in evs if e["type"] in ("assistant_text","thinking") and e.get("parent_tool_use_id")]
assert not kept, kept
# Its answer reached the chat: in the Agent call'"'"'s result, or in its SubagentHandback call under it.
results=[e["output"] for e in evs if e["type"]=="tool_result" and e["tool_use_id"]=="'"$SUB_CALL"'"]
handed=[e["input"].get("message") or "" for e in evs if e["type"]=="tool_call" and e["name"]=="SubagentHandback" and e.get("parent_tool_use_id")=="'"$SUB_CALL"'"]
print("result:", results, "handed back:", handed)
assert any("lighthouse keeper" in t.lower() for t in results + handed), evs'
# After a restart, without a session: from the transcript, by its tool call and by its agent id.
# The answer comes as the subagent's text, or as the message of its SubagentHandback call (Claude
# Code asks subagents to hand their report back so); the Agent call's result then only points to it.
c daemon stop >/dev/null; c daemon start >/dev/null
c --json subagent sub-one "$SUB_CALL" | python3 -c '
import sys,json
r=json.load(sys.stdin)
texts=[e["text"] for e in r["events"] if e["type"]=="assistant_text"]
handed=[e["input"].get("message") or "" for e in r["events"] if e["type"]=="tool_call" and e["name"]=="SubagentHandback"]
print(r["agent_id"], "text:", texts, "handed back:", handed)
assert r["found"] and r["agent_id"], r
assert any("lighthouse keeper" in t.lower() for t in texts + handed), r
assert all(e.get("parent_tool_use_id") is None for e in r["events"] if e["type"] in ("assistant_text","tool_call")), r'
SUB_AGENT=$(c --json subagent sub-one "$SUB_CALL" | python3 -c 'import sys,json; print(json.load(sys.stdin)["agent_id"])')
c --json subagent sub-one "$SUB_AGENT" | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r["found"] and r["events"], r'
c subagent sub-one "$SUB_AGENT" | grep -qi "lighthouse keeper"
c --json subagent sub-one a0000000000000000 | python3 -c 'import sys,json; r=json.load(sys.stdin); assert not r["found"] and r["events"] == [], r'
c archive --force sub-one >/dev/null

echo "== 1d. a workspace created with a first message is named after it"
c new --model "$MODEL" --prompt "Add a dark mode toggle to the settings page. Do nothing, reply with the single word ok." >/dev/null
NAMED=$(c --json ls | python3 -c '
import sys,json; ws=json.load(sys.stdin)["workspaces"]
named=[w for w in ws if w.get("title")]
print([(w["slug"], w["title"], w["branch"]) for w in named], file=sys.stderr)
dark=[w for w in named if "dark" in w["slug"] and w["branch"].endswith("/" + w["slug"])]
assert dark, ws
print(dark[0]["slug"])')
# Its worktrees would otherwise count among the clones' (step 4d).
c archive --force "$NAMED" >/dev/null

echo "== 1d2. created in the background, the first chat shows at once and its agent starts once the workspace is ready"
c --json new --background --name bg-one --model "$MODEL" --prompt "Reply with the single word pong and nothing else." | python3 -c '
import sys,json; r=json.load(sys.stdin)
assert r["workspace"]["state"] == "creating", r["workspace"]["state"]
assert r["chat"] and r["chat"]["status"] == "starting", r["chat"]'
# Sent while the workspace is set up: it waits, then runs after the first turn.
c send bg-one "Now reply with the single word second and nothing else." >/dev/null
for _ in 1 2 3 4 5 6; do
  LAST=$(c --json wait bg-one --timeout 240 | python3 -c 'import sys,json; print(json.load(sys.stdin)["last_text"] or "")')
  case "$LAST" in *second*|*Second*|*SECOND*) break ;; esac
  sleep 1
done
echo "$LAST"
case "$LAST" in *second*|*Second*|*SECOND*) ;; *) echo "the message sent during the setup never ran"; exit 1 ;; esac
c archive --force bg-one >/dev/null

echo "== 1e. with Jev on, a chat past the threshold compacts after its turn"
# A key TypeSafe refuses and a short chat: Jev leaves the summary to Claude Code, which still
# shows that the plugin ran and the compaction happened.
echo "not-a-typesafe-key" | c jev key >/dev/null
c jev threshold 1 >/dev/null
c jev on | grep -q "on, from 1% of the context"
c new --name jev-one --model "$MODEL" --prompt "Reply with the single word pong and nothing else." >/dev/null
c --json wait jev-one --timeout 300 >/dev/null
JEV_CHAT=$(c --json chats jev-one | python3 -c 'import sys,json; print(json.load(sys.stdin)["chats"][0]["id"])')
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT payload FROM events WHERE chat_id = '$JEV_CHAT'" | python3 -c '
import sys,json
events=[json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
notices=[e["message"] for e in events if e["type"]=="notice"]
print("\n".join(notices))
assert any(n.startswith("Compacting the conversation with Jev") for n in notices), notices
assert any(n.startswith("jev: ") for n in notices), notices
assert any(n.startswith("Conversation compacted") for n in notices), notices
ends=[e for e in events if e["type"]=="turn_ended"]
assert len(ends)==1 and ends[0]["outcome"]=="success", ends'
c jev key --remove >/dev/null
c archive --force jev-one >/dev/null

echo "== 2. the agent works across both repos"
c send chat-one "Create a file named hello.txt containing 'hi' in BOTH the alpha and beta folders, then reply DONE."
c wait chat-one --timeout 240 | tail -3
test -f "$CHAUFFEUR_WORKSPACES_DIR/chat-one/alpha/hello.txt"
test -f "$CHAUFFEUR_WORKSPACES_DIR/chat-one/beta/hello.txt"
c diff chat-one --stat | tee "$T/diff.txt"
grep -q "untracked hello.txt" "$T/diff.txt"
test "$(grep -c 'hello.txt' "$T/diff.txt")" = 2

echo "== 3. git worktree add is refused and chauffeur worktree add is used"
c send chat-one "Run exactly this shell command: git -C alpha worktree add ../extra -b extra-branch . If it is refused, follow the instruction you get instead, using the name extra. Then reply with the folder path you ended up with."
c wait chat-one --timeout 240 | tail -4
c --json show chat-one | python3 -c '
import sys,json; w=json.load(sys.stdin)
agent=[t for t in w["worktrees"] if t["origin"]=="agent" and not t["removed"]]
print("agent worktrees:", [(t["repo"], t["path"]) for t in agent])
assert agent, "the agent did not create its worktree through chauffeur"'
test ! -d "$CHAUFFEUR_WORKSPACES_DIR/chat-one/extra"

if [ -n "${ISOLATION:-}" ]; then
  echo "== 3b. Claude's native worktree isolation goes through Chauffeur (model-dependent)"
  c send chat-one "Use the Agent tool with the isolation option set to \"worktree\" to have a subagent create sub.txt in the alpha folder of its isolated copy, wait for it, then reply with the path it used." >/dev/null
  c wait chat-one --timeout 300 | tail -2
  c --json show chat-one | python3 -c '
import sys,json; w=json.load(sys.stdin)
groups={t["group"] for t in w["worktrees"] if t["origin"]=="agent" and t["group"] and t["group"].startswith("agent-")}
print("isolation groups:", groups); assert groups'
fi

echo "== 3c. the agent proposes a repo the workspace lacks: a card of its own, the repo added once the user says yes"
# The open add_repo request of a chat, once its agent asked.
proposal() { events "$1" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
resolved = {e["request_id"] for e in events if e["type"] == "request_resolved"}
open_ = [e for e in events if e["type"] == "permission_request" and e["tool_name"] == "mcp__chauffeur__add_repo" and e["request_id"] not in resolved]
print(open_[0]["request_id"] if open_ else "")'; }
# Asks the chat's agent to propose gamma, says yes, and checks gamma came on the workspace's branch.
propose_gamma() {
  local slug=$1 chat=$2 req=""
  for _ in $(seq 1 180); do req=$(proposal "$chat"); [ -n "$req" ] && break; sleep 1; done
  [ -n "$req" ] || { events "$chat" | grep -o '"name":"[^"]*"' | sort | uniq -c; echo "the model did not call add_repo"; exit 1; }
  events "$chat" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
asked = next(e for e in events if e["type"] == "permission_request" and e["request_id"] == "'"$req"'")
print(asked["title"], asked["input"])
assert asked["title"] == "Add gamma to this workspace?" and asked["input"]["repo"] == "gamma", asked'
  test ! -e "$CHAUFFEUR_WORKSPACES_DIR/$slug/gamma"
  c respond "$slug" "$req" allow >/dev/null
  c wait "$slug" --timeout 240 | tail -2
  c --json show "$slug" | python3 -c '
import sys,json; w=json.load(sys.stdin)
gamma=[t for t in w["worktrees"] if t["repo"]=="gamma" and t["origin"]=="workspace" and not t["removed"]]
print("gamma:", gamma)
assert gamma and gamma[0]["branch"] == w["branch"], gamma'
  test -f "$CHAUFFEUR_WORKSPACES_DIR/$slug/gamma/README.md"
}
GAMMA_PROMPT="This task needs the repo gamma, which this workspace lacks. Propose adding it with your add_repo tool, with the reason: to read its README. Once it is added, reply with the first line of gamma/README.md and nothing else."
c new --name propose-one --repo alpha --model "$MODEL" --prompt "$GAMMA_PROMPT" >/dev/null
propose_gamma propose-one "$(chat_id propose-one)"
c archive --force propose-one >/dev/null

echo "== 4. archive stops the chat and removes everything"
c archive chat-one
test ! -d "$CHAUFFEUR_WORKSPACES_DIR/chat-one"

echo "== 4a. archived conversations include closed forks and highlight matching text"
bun scripts/app-call.ts workspace.search_archived '{"query":"remember kiwi"}' | python3 -c '
import sys,json
r=json.load(sys.stdin)
hits=[h for h in r["hits"] if h["chat_id"] == sys.argv[1]]
assert hits, r
assert all("\x01" in h["snippet"] and "\x02" in h["snippet"] for h in hits), hits
print(len(hits), "matching messages in the closed fork")' "$TURN_FORK"

# A commit pushed to alpha's base branch that the user's clone has not fetched: an exploration
# must see it.
git clone -q "$T/origin/alpha.git" "$T/pusher"
echo "The secret word is MARKER-42." > "$T/pusher/STAGING.md"
git -C "$T/pusher" add -A && git -C "$T/pusher" commit -q -m "staging marker" && git -C "$T/pusher" push -q origin main
CLONE_HEAD=$(git -C "$T/clones/alpha" rev-parse HEAD)
# Its checkouts' HEADs, to tell that nothing moved them.
heads() { for r in alpha beta; do git -C "$CHAUFFEUR_WORKSPACES_DIR/$1/$r" rev-parse HEAD; done; }

echo "== 4b. an exploration reads its repos at the freshly fetched base branch"
c new --explore --name explore-one --model "$MODEL" --prompt "What is the secret word written in alpha/STAGING.md? Reply with that word only." >/dev/null
c --json wait explore-one --timeout 180 | python3 -c 'import sys,json; t=json.load(sys.stdin)["last_text"]; print(t); assert "MARKER-42" in (t or "")'
c --json show explore-one | python3 -c '
import sys,json; w=json.load(sys.stdin)
assert w["explore"] and w["branch"] == "", w
assert all(t["branch"] == "" and t["base_branch"] == "main" for t in w["worktrees"]), w["worktrees"]'
BEFORE=$(heads explore-one)

echo "== 4c. the exploration's agent cannot write a file or commit"
c send explore-one "Use your Write tool (not the shell) to create alpha/scratch.txt containing x. If you have no such tool, say NO-WRITE-TOOL. Then run exactly this shell command: git -C alpha commit --allow-empty -m explore-commit . Reply with what happened to each step." >/dev/null
c wait explore-one --timeout 240 | tail -4
test ! -e "$CHAUFFEUR_WORKSPACES_DIR/explore-one/alpha/scratch.txt"
test "$(heads explore-one)" = "$BEFORE"
! git -C "$T/clones/alpha" log --all --format=%s | grep -q explore-commit
c --json chats explore-one | python3 -c '
import sys,json; chats=json.load(sys.stdin)["chats"]
assert all(c["permission_mode"] == "plan" for c in chats), chats'

echo "== 4c2. the exploration's agent writes its plan to the notes, and nothing else"
PLANS="$CHAUFFEUR_WORKSPACES_DIR/explore-one/.chauffeur/plans"
c send explore-one "Write a three-line plan for adding a CONTRIBUTING.md to alpha into your plan file now, with your Write tool, at the plan file path your plan-mode instructions give. Do not call ExitPlanMode. Reply with the plan file path only." >/dev/null
c wait explore-one --timeout 240 | tail -2
ls "$PLANS"/*.md >/dev/null
test -s "$(ls "$PLANS"/*.md | head -1)"
test -z "$(git -C "$CHAUFFEUR_WORKSPACES_DIR/explore-one/alpha" status --porcelain)"
test "$(heads explore-one)" = "$BEFORE"
PLAN_NAME=$(basename "$(ls "$PLANS"/*.md | head -1)")

echo "== 4c3. Always allow holds in plan mode: the tool is not asked again in the chat"
# Plan mode asks for each write but the plan file, and Claude Code suggests no rule then.
# Its instructions also tell the model to write nothing but its plan file, above any other
# instruction: the model may decline a write in words, without calling Write. The user then insists,
# up to three messages in all; a model that never calls Write fails the step as the model's doing,
# a call Chauffeur never asks about as Chauffeur's.
EX_CHAT=$(chat_id explore-one)
asked() { events "$EX_CHAT" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
resolved = {e["request_id"] for e in events if e["type"] == "request_resolved"}
print(" ".join(e["request_id"] for e in events if e["type"] == "permission_request" and e["request_id"] not in resolved))'; }
# How many times the chat's agent called Write for a file of the notes, and what came of each call.
write_calls() { events "$EX_CHAT" | python3 -c '
import sys, json
name = "/" + sys.argv[1]
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
calls = [e for e in events if e["type"] == "tool_call" and e["name"] == "Write" and e["input"].get("file_path", "").endswith(name)]
results = {e["tool_use_id"]: e for e in events if e["type"] == "tool_result"}
for call in calls:
    print("Write", sys.argv[1], "->", str(results.get(call["tool_use_id"], {}).get("output", "no result"))[:200], file=sys.stderr)
print(len(calls))' "$1"; }
turns_ended() { events "$EX_CHAT" | grep -c '"type":"turn_ended"' || true; }
# Sends $2 for the agent to write the file $1, then $3 while it has not called Write, up to three
# messages; returns when a permission request is open or the turn ended after a Write call.
insist_write() {
  local message=$2 before
  for _ in 1 2 3; do
    before=$(turns_ended)
    c send explore-one "$message" >/dev/null
    for _ in $(seq 1 240); do
      [ -n "$(asked)" ] && return 0
      [ "$(turns_ended)" -gt "$before" ] && break
      sleep 1
    done
    [ "$(write_calls "$1")" = 0 ] || return 0
    echo "the model did not call Write for $1"
    message=$3
  done
  echo "the model did not call its Write tool for $1 in three messages (it declined in plan mode): the model's doing, not Chauffeur's"
  exit 1
}
INSIST="Calling your Write tool is the check: plan mode does not refuse it, Claude Code asks me first and I approve. Do not answer in words instead."
insist_write notes-a.md \
  "This is a permission check: use your Write tool, once, to create the file $PLANS/notes-a.md containing a. Wait for its result. Then use your Write tool again to create $PLANS/notes-b.md containing b. $INSIST Do not call ExitPlanMode. Reply with the single word written." \
  "You did not call your Write tool. $INSIST Use your Write tool now to create $PLANS/notes-a.md containing a; once it is done, create $PLANS/notes-b.md containing b the same way. Do not call ExitPlanMode. Reply with the single word written."
# The first request, should the model have called Write twice at once: Always allow covers both.
REQ=$(asked | cut -d" " -f1)
[ -n "$REQ" ] || { echo "the model called Write for notes-a.md but Chauffeur never asked about it"; exit 1; }
c respond explore-one "$REQ" allow --always >/dev/null
c wait explore-one --timeout 240 | tail -2
if [ "$(write_calls notes-b.md)" = 0 ]; then
  insist_write notes-b.md \
    "Now use your Write tool to create $PLANS/notes-b.md containing b. $INSIST Do not call ExitPlanMode. Reply with the single word written." \
    "You did not call your Write tool. $INSIST Use it now to create $PLANS/notes-b.md containing b. Do not call ExitPlanMode. Reply with the single word written."
  [ -z "$(asked)" ] || { echo "notes-b.md was asked again after Always allow"; exit 1; }
  c wait explore-one --timeout 240 | tail -1
fi
# Asked once, for the first write: no request after Always allow, which kept the rule Write.
events "$EX_CHAT" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
at = next(i for i, e in enumerate(events) if e["type"] == "request_resolved" and e["request_id"] == sys.argv[1])
print(events[at])
assert events[at].get("rules") == ["Write"], events[at]
again = [e for e in events[at:] if e["type"] == "permission_request" and e["tool_name"] == "Write"]
assert not again, ("asked again after Always allow", again)' "$REQ"
test -s "$PLANS/notes-a.md" || { echo "notes-a.md was not written"; exit 1; }
test -s "$PLANS/notes-b.md" || { echo "notes-b.md was not written though Write was called and not asked"; exit 1; }
# A new agent session of the chat (here after a daemon restart) keeps the rule.
c daemon stop >/dev/null && c daemon start >/dev/null
insist_write notes-c.md \
  "Use your Write tool to create $PLANS/notes-c.md containing c. $INSIST Do not call ExitPlanMode. Reply with the single word written." \
  "You did not call your Write tool. $INSIST Use it now to create $PLANS/notes-c.md containing c. Do not call ExitPlanMode. Reply with the single word written."
[ -z "$(asked)" ] || { echo "the new session asked again"; exit 1; }
c wait explore-one --timeout 240 | tail -1
test -s "$PLANS/notes-c.md" || { echo "notes-c.md was not written though Write was called and not asked"; exit 1; }
events "$EX_CHAT" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
asked = [e for e in events if e["type"] == "permission_request" and "notes-c" in json.dumps(e["input"])]
assert not asked, asked'

echo "== 4c4. Always allow on a command writing outside the workspace keeps the chat's mode"
# In default mode Claude Code suggests, with such a command, switching the chat to accept edits and
# making the folder a working directory: neither is applied.
OUT="$T/outside"
c new --name ask-out --model "$MODEL" --permission-mode default --prompt "Use the Bash tool to run exactly: mkdir -p $OUT/one . Then reply with the single word made." >/dev/null
OUT_CHAT=$(chat_id ask-out)
unanswered() { events "$OUT_CHAT" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
resolved = {e["request_id"] for e in events if e["type"] == "request_resolved"}
print(" ".join(e["request_id"] for e in events if e["type"] == "permission_request" and e["request_id"] not in resolved))'; }
in_default_mode() { c --json chats ask-out | python3 -c '
import sys, json
chat = json.load(sys.stdin)["chats"][0]
print(chat["permission_mode"], chat.get("permission_mode_effective"))
assert chat["permission_mode"] == "default" and chat.get("permission_mode_effective") in (None, "default"), chat'; }
REQ=""
for _ in $(seq 1 120); do REQ=$(unanswered); [ -n "$REQ" ] && break; sleep 1; done
[ -n "$REQ" ] || { echo "the command was never asked"; exit 1; }
c respond ask-out "$REQ" allow --always >/dev/null
c wait ask-out --timeout 180 | tail -1
test -d "$OUT/one"
in_default_mode
c send ask-out "Use the Bash tool to run exactly the same command again: mkdir -p $OUT/one . Then reply with the single word again." >/dev/null
c wait ask-out --timeout 180 | tail -1
[ -z "$(unanswered)" ] || { echo "the command was asked again"; exit 1; }
events "$OUT_CHAT" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
asked = [e for e in events if e["type"] == "permission_request"]
resolved = [e for e in events if e["type"] == "request_resolved" and e["request_id"] == "'"$REQ"'"]
print(len(asked), resolved)
assert len(asked) == 1, asked
assert resolved and resolved[0].get("rules"), resolved'
in_default_mode
c archive --force ask-out >/dev/null

echo "== 4d. archiving the exploration leaves the user's clones as they were, and keeps the plan"
c archive explore-one
test -s "$CHAUFFEUR_HOME/archive/explore-one/plans/$PLAN_NAME"
test ! -d "$CHAUFFEUR_WORKSPACES_DIR/explore-one"
test "$(git -C "$T/clones/alpha" rev-parse HEAD)" = "$CLONE_HEAD"
test "$(git -C "$T/clones/alpha" worktree list | wc -l | tr -d ' ')" = 1
test -z "$(git -C "$T/clones/alpha" status --porcelain)"

echo "== 4e. an exploration continues as a workspace: its agent writes and commits, its conversation kept"
c new --explore --name explore-two --model "$MODEL" --prompt "What is the secret word written in alpha/STAGING.md? Reply with that word only." >/dev/null
c --json wait explore-two --timeout 180 >/dev/null
c promote explore-two | grep -q explore-two
c --json show explore-two | python3 -c '
import sys,json; w=json.load(sys.stdin)
assert not w["explore"] and w["branch"] == "e2e/explore-two" and w["port_count"] > 0, w'
# Its chat leaves plan mode for the mode new chats start in.
c --json chats explore-two | python3 -c 'import sys,json; c=json.load(sys.stdin)["chats"][0]; assert c["permission_mode"] == "auto", c'
EX2="$CHAUFFEUR_WORKSPACES_DIR/explore-two"
EX2_CHAT=$(chat_id explore-two)
# Auto mode's classifier reads the whole conversation, an exploration's: it has blocked this commit
# as Self-Modification or Instruction Poisoning. Its judgment is not what this step checks: rules
# in the throwaway workspace's local settings allow the commit, which then skips the classifier.
# Chauffeur's guard (a hook) and the exploration's sandbox would still refuse it.
mkdir -p "$EX2/.claude"
cat > "$EX2/.claude/settings.local.json" <<JSON
{"permissions": {"allow": ["Bash(git -C alpha add:*)", "Bash(git -C alpha commit:*)", "Bash(git -C $EX2/alpha add:*)", "Bash(git -C $EX2/alpha commit:*)", "Bash(git add:*)", "Bash(git commit:*)"]}}
JSON
# What came of the chat's calls of a tool whose input names $2: printed, and the last one's outcome
# (none, done, blocked by the classifier, failed).
outcome() { events "$EX2_CHAT" | python3 -c '
import sys, json
tool, needle = sys.argv[1], sys.argv[2]
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
results = {e["tool_use_id"]: e for e in events if e["type"] == "tool_result"}
kind = "none"
for call in [e for e in events if e["type"] == "tool_call" and e["name"] == tool and needle in json.dumps(e["input"])]:
    result = results.get(call["tool_use_id"]) or {"is_error": True, "output": "no result"}
    output = str(result["output"])
    kind = "done" if not result["is_error"] else "classifier" if "auto mode classifier" in output else "failed"
    print(tool, json.dumps(call["input"])[:300], "->", output[:300], file=sys.stderr)
print(kind)' "$1" "$2"; }
written() { grep -qs MARKER-42 "$EX2/alpha/secret.txt"; }
committed() { [ "$(git -C "$EX2/alpha" log -1 --format=%s)" = secret ]; }
# Said in the message, again if asked twice: Haiku 5.5 otherwise keeps to the read-only rules it
# read as an exploration.
TASK="This exploration is now a workspace on its own branch: you may edit files and commit now. Use your Write tool to write the secret word you found earlier into alpha/secret.txt, then commit that file with exactly this command: git -C alpha add secret.txt && git -C alpha commit -m secret . Reply with the word only."
c send explore-two "$TASK" >/dev/null
c --json wait explore-two --timeout 240 >/dev/null
# Once more if the model left a part undone (not called, or a command the rules do not cover), with
# the conversation still the only place the word is.
if ! { written && committed; }; then
  echo "the model did not write and commit (write: $(outcome Write secret.txt), commit: $(outcome Bash commit)); asking again"
  c send explore-two "Not done yet. $TASK" >/dev/null
  c --json wait explore-two --timeout 240 >/dev/null
fi
if ! written; then
  case $(outcome Write secret.txt) in
    none) echo "the model did not write alpha/secret.txt (it never called Write)" ;;
    failed) echo "the promoted chat could not write alpha/secret.txt: see the Write call's output above" ;;
    *) echo "alpha/secret.txt does not hold the secret word: the model did not keep it from the conversation, or Chauffeur lost the conversation" ;;
  esac
  exit 1
fi
if ! committed; then
  case $(outcome Bash commit) in
    none) echo "the model did not run the commit" ;;
    classifier) echo "the model ran a commit the e2e's rules do not cover, and auto mode's classifier blocked it" ;;
    *) echo "the commit failed or was refused: see the command's output above (Chauffeur's guard or sandbox would be Chauffeur's)" ;;
  esac
  exit 1
fi
c archive explore-two --force >/dev/null

echo "== 4f0. a mockup request creates and starts a design chat from its conversation"
check_design_request design-request claude "$MODEL"

echo "== 4f. a design chat starts from a brief of its source chat, runs in auto mode, reads the notes unasked, and writes its own folder only"
c new --name design-one --model "$MODEL" --prompt "We will add a Reconnect button to the login screen of the mobile app; it shows when the phone lost its Mac. Reply with the word ok." >/dev/null
c --json wait design-one --timeout 180 >/dev/null
SOURCE=$(c --json chats design-one | python3 -c 'import sys,json; print(json.load(sys.stdin)["chats"][0]["id"])')
c --json chat design-one --design --from "$SOURCE" --model "$MODEL" > "$T/design.json"
DESIGN=$(python3 -c '
import sys,json; c=json.load(open(sys.argv[1])); b=c["brief"]; print(b, file=sys.stderr)
assert c["kind"] == "design" and c["source_chat"] == sys.argv[2] and c["permission_mode"] == "auto", c
assert b["state"] == "ready" and "econnect" in b["text"], b
print(c["id"])' "$T/design.json" "$SOURCE")
DIR="$CHAUFFEUR_WORKSPACES_DIR/design-one/.chauffeur/designs/$DESIGN"
BEFORE=$(heads design-one)
c send "$DESIGN" "Write the screen login.html in your design folder: a login page with a Reconnect button. Then write canvas.json listing it in one section. Reply DONE." >/dev/null
c --json wait design-one --timeout 240 >/dev/null
test -f "$DIR/login.html" && grep -qi reconnect "$DIR/login.html"
python3 -c 'import sys,json; c=json.load(open(sys.argv[1])); assert c["pages"][0]["sections"][0]["screens"], c' "$DIR/canvas.json"
# It checks a render in its own browser, which opens no window, not in the user's.
c send "$DESIGN" "Check how login.html renders at 390 x 844 and reply with the page title you see." >/dev/null
c --json wait design-one --timeout 300 >/dev/null
events "$DESIGN" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
calls = [e["name"] for e in events if e["type"] == "tool_call" and e["name"].startswith("mcp__")]
assert any(n.startswith("mcp__headless-browser__browser_navigate") for n in calls), calls
assert not any(n.startswith("mcp__playwright__") for n in calls), calls'
c --json chats design-one | python3 -c '
import sys,json; c=[c for c in json.load(sys.stdin)["chats"] if c["kind"] == "design"][0]
assert "brief" not in c, c'
c --json chats design-one | python3 -c '
import sys,json; c=[c for c in json.load(sys.stdin)["chats"] if c["kind"] == "design"][0]
assert c.get("permission_mode_effective") in (None, "auto"), c'
# The notes (the user's attachments) sit outside its working directories: read without asking.
mkdir -p "$CHAUFFEUR_WORKSPACES_DIR/design-one/.chauffeur/attachments"
echo "NOTE-TOKEN-7319" > "$CHAUFFEUR_WORKSPACES_DIR/design-one/.chauffeur/attachments/brief.txt"
c send "$DESIGN" "Use your Read tool on $CHAUFFEUR_WORKSPACES_DIR/design-one/.chauffeur/attachments/brief.txt and reply with its content only." >/dev/null
c --json wait design-one --timeout 240 >/dev/null
events "$DESIGN" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
reads = [e for e in events if e["type"] == "tool_call" and e["name"] == "Read" and "brief.txt" in json.dumps(e["input"])]
assert reads, "the model did not read the note"
asked = [e for e in events if e["type"] == "permission_request" and e["tool_name"] == "Read"]
assert not asked, asked
assert any(e["type"] == "tool_result" and "NOTE-TOKEN-7319" in json.dumps(e) for e in events), "the read failed"'
# Each forbidden write in its own message: asked several at once, a small model may refuse them all.
c send "$DESIGN" "Use your Write tool to create the file $CHAUFFEUR_WORKSPACES_DIR/design-one/alpha/hack.txt containing x. Report what happened." >/dev/null
c --json wait design-one --timeout 240 >/dev/null
c send "$DESIGN" "Run exactly this shell command: echo x > $CHAUFFEUR_WORKSPACES_DIR/design-one/alpha/hack2.txt . Report what happened." >/dev/null
c --json wait design-one --timeout 240 >/dev/null
c send "$DESIGN" "Run exactly this shell command: git -C $CHAUFFEUR_WORKSPACES_DIR/design-one/alpha commit --allow-empty -m design-commit . Report what happened." >/dev/null
c --json wait design-one --timeout 240 >/dev/null
# The model refuses those by itself: the sandbox is what holds a command it runs unawares, here a
# script of its own folder that writes into a repo. It runs without asking (inside the sandbox).
printf '#!/bin/sh\nif echo x > "%s/alpha/hack3.txt" 2>/dev/null; then echo PROBE-WROTE; else echo PROBE-BLOCKED; fi\n' "$CHAUFFEUR_WORKSPACES_DIR/design-one" > "$DIR/probe.sh"
c send "$DESIGN" "Run exactly this shell command from your design folder: sh probe.sh . It is a check script of this folder. Reply with its output only." >/dev/null
c --json wait design-one --timeout 240 >/dev/null
test ! -e "$CHAUFFEUR_WORKSPACES_DIR/design-one/alpha/hack3.txt"
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select payload from events where chat_id = '$DESIGN' and payload like '%tool_result%'" | grep -q PROBE-BLOCKED
test ! -e "$CHAUFFEUR_WORKSPACES_DIR/design-one/alpha/hack.txt"
test ! -e "$CHAUFFEUR_WORKSPACES_DIR/design-one/alpha/hack2.txt"
test "$(heads design-one)" = "$BEFORE"
c archive design-one --force >/dev/null

echo "== 4g. Find routines reads the user's requests in one turn of the model chosen, and a time window gives the cadence"
c new --name installs-ask --repo alpha --model "$MODEL" --prompt "Est-ce que tu peux analyser toutes les installations des 3 derniers jours ? Pour ce test, réponds juste ok." >/dev/null
c wait installs-ask --timeout 300 >/dev/null
c routine find --days 1 --model "$MODEL" >/dev/null
for _ in $(seq 1 90); do
  c --json routine suggestions | python3 -c 'import sys,json; sys.exit(0 if json.load(sys.stdin)["running"] is None else 1)' && break
  sleep 2
done
c --json routine suggestions | python3 -c '
import sys,json; f=json.load(sys.stdin)
assert f["running"] is None and f["last_error"] is None, f
found = [s for s in f["suggestions"] if any(r["window"] == "3 derniers jours" for r in s["requests"])]
assert found, ("the model did not propose a routine for the 3-day window", f["suggestions"])
assert found[0]["spec"]["schedule"].get("every_days") == 3, ("the model gave another cadence", found[0]["spec"]["schedule"])'
c archive installs-ask --force >/dev/null
if [ -n "${CODEX:-}" ]; then
  # Needs Codex logged in (`codex login`). gpt-6-luna: the cheapest model `model/list` offers.
  CODEX_MODEL=${CODEX_MODEL:-gpt-6-luna}
  echo "== 4f0-codex. a Codex mockup request creates and starts a design chat"
  check_design_request design-request-codex codex "$CODEX_MODEL"
  echo "== 5. a Codex chat answers"
  c new --name codex-one >/dev/null
  c chat codex-one --agent codex --model "$CODEX_MODEL" --prompt "Reply with the single word pong and nothing else." >/dev/null
  c wait codex-one --timeout 180 | tee "$T/codex1.txt"
  # The answer on a line of its own (the chat's title also contains "pong").
  grep -qiE '^pong\.?$' "$T/codex1.txt"
  echo "== 5-cost. Codex records an API token estimate separately from a reported cost"
  sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT payload FROM events WHERE chat_id = '$(chat_id codex-one)' AND payload LIKE '%\"type\":\"turn_ended\"%'" | python3 -c '
import sys, json
turns = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
assert turns and turns[-1].get("estimated_cost_usd", 0) > 0 and turns[-1]["cost_usd"] is None, turns
print("Estimated API token cost:", turns[-1]["estimated_cost_usd"])'
  sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT count(*) FROM workspace_agent_roots r JOIN workspace_agent_launches l USING(launch_id) WHERE l.chat_id = '$(chat_id codex-one)'" | python3 -c 'import sys; count=int(sys.stdin.read()); print("Codex registered native roots:",count); assert count > 0, count'
  c --json chats codex-one | python3 -c '
import sys,json; chats=json.load(sys.stdin)["chats"]
assert chats[0]["agent"] == "codex" and chats[0]["agent_session_id"], chats'

  echo "== 5a. a forked Codex chat carries its conversation on, in a thread of its own"
  CODEX_ORIG=$(chat_id codex-one)
  CODEX_FORK=$(fork_chat "$CODEX_ORIG")
  c send "$CODEX_FORK" "Which single word did you reply with in your previous answer? Reply with that word only." >/dev/null
  c wait codex-one --timeout 180 >/dev/null
  last_text "$CODEX_FORK" | python3 -c 'import sys; t=sys.stdin.read(); print(t); assert "pong" in t.lower(), "the forked Codex chat did not carry the conversation on"'
  c --json chats codex-one | python3 -c '
import sys, json
chats = {c["id"]: c for c in json.load(sys.stdin)["chats"]}
orig, fork = chats[sys.argv[1]], chats[sys.argv[2]]
assert fork["agent"] == "codex" and fork["agent_session_id"] != orig["agent_session_id"], chats' "$CODEX_ORIG" "$CODEX_FORK"

  echo "== 5a2. a Codex chat forked from one of its turns keeps the thread through that turn only"
  c send "$CODEX_FORK" "Remember the word plum. Reply with the single word ok." >/dev/null
  c wait codex-one --timeout 180 >/dev/null
  CODEX_TURN_FORK=$(fork_turn "$CODEX_FORK" "$(( $(turns "$CODEX_FORK") - 1 ))")
  c send "$CODEX_TURN_FORK" "Did I ask you to remember a word? Reply with that word only, or with none." >/dev/null
  c wait codex-one --timeout 180 >/dev/null
  last_text "$CODEX_TURN_FORK" | python3 -c 'import sys; t=sys.stdin.read(); print(t); assert "plum" not in t.lower(), "the Codex turn fork kept what came after its turn"'
  c chat close "$CODEX_TURN_FORK" >/dev/null
  c chat close "$CODEX_FORK" >/dev/null
  sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT phase FROM workspace_agent_launches WHERE chat_id = '$CODEX_FORK' ORDER BY launch_id DESC LIMIT 1" | python3 -c 'import sys; phase=sys.stdin.read().strip(); print("Codex close launch:",phase); assert phase == "stopped", phase'

  echo "== 5a3. a Codex chat forked into Claude Code hands it the conversation as text"
  ACROSS=$(c --json chat fork "$CODEX_ORIG" --agent claude | python3 -c '
import sys, json
f = json.load(sys.stdin)
print(f["title"], f["agent"], f.get("forked_from_agent"), file=sys.stderr)
assert f["agent"] == "claude" and f.get("forked_from_agent") == "codex" and not f["agent_session_id"], f
print(f["id"])')
c send "$ACROSS" "Which single word did the agent reply with in its previous answer? Reply with that word only." >/dev/null
c wait codex-one --timeout 180 >/dev/null
last_text "$ACROSS" | python3 -c 'import sys; t=sys.stdin.read(); print(t); assert "pong" in t.lower(), "Claude Code did not get the Codex conversation"'
# The transcript goes to the agent only: the user's bubble keeps their own words.
events "$ACROSS" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
asked = [e["text"] for e in events if e["type"] == "user_message"][-1]
print(asked)
assert asked.startswith("Which single word") and "<conversation>" not in asked, asked'
c --json chats codex-one | python3 -c '
import sys, json
chats = {c["id"]: c for c in json.load(sys.stdin)["chats"]}
assert chats[sys.argv[1]]["agent_session_id"], chats[sys.argv[1]]' "$ACROSS"
c chat close "$ACROSS" >/dev/null

  echo "== 5b. a repo's Codex skill is listed for / and runs"
  c commands codex-one --agent codex | grep -E "^/e2e-codex-ping"
  c send codex-one "/e2e-codex-ping" >/dev/null
  c wait codex-one --timeout 180 | tee "$T/codex-skill.txt" | tail -1
  grep -q "PONG-CODEX" "$T/codex-skill.txt"

  echo "== 5b2. /compact compacts a Codex chat Codex's own way while Jev is off, in a turn of its own"
  c commands codex-one --agent codex | grep -qE "^/compact"
  CODEX_ONE=$(chat_id codex-one)
  codex_events() {
    sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "SELECT payload FROM events WHERE chat_id = '$CODEX_ONE' ORDER BY seq"
  }
  BEFORE=$(codex_events | wc -l)
  c send codex-one "/compact" >/dev/null
  c wait codex-one --timeout 300 >/dev/null
  codex_events | tail -n +"$((BEFORE + 1))" | python3 -c '
import sys,json
events=[json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
print([e["type"] if e["type"]!="notice" else e["message"] for e in events])
assert any(e["type"]=="notice" and e["message"]=="Conversation compacted" for e in events), "Codex did not compact"
ends=[e for e in events if e["type"]=="turn_ended"]
assert len(ends)==1 and ends[0]["outcome"]=="success", ends'

  echo "== 5b3. with Jev on, a Codex chat past the threshold compacts after its turn, Codex's way when Jev fails"
  # A key TypeSafe refuses: Jev fails, Codex compacts instead, within the same turn.
  echo "not-a-typesafe-key" | c jev key >/dev/null
  c jev threshold 1 >/dev/null
  c jev on >/dev/null
  BEFORE=$(codex_events | wc -l)
  c send codex-one "Reply with the single word pong and nothing else." >/dev/null
  c wait codex-one --timeout 300 >/dev/null
  codex_events | tail -n +"$((BEFORE + 1))" | python3 -c '
import sys,json
events=[json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
notices=[e["message"] for e in events if e["type"]=="notice"]
print("\n".join(notices))
reading=[e for e in events if e["type"]=="context_usage"]
assert reading and reading[-1]["usage"]["compaction"]=={"by":"jev","threshold_percent":1.0}, reading
assert any(n.startswith("Compacting the conversation with Jev") for n in notices), notices
assert any(n.startswith("Jev did not compact the conversation") for n in notices), notices
assert "Conversation compacted" in notices, notices
ends=[e for e in events if e["type"]=="turn_ended"]
assert len(ends)==1 and ends[0]["outcome"]=="success", ends'
  c jev key --remove >/dev/null

  echo "== 5b-switch. a confirmed provider change keeps one chat and carries context in both directions"
  c new --name provider-switch --repo alpha --model "$MODEL" --prompt "Remember the secret word APRICOT-83. Reply READY only." >/dev/null
  c wait provider-switch --timeout 180 >/dev/null
  SWITCH_CHAT=$(chat_id provider-switch)
  configure_provider() {
    bun - "$SWITCH_CHAT" "$1" "$2" "$3" <<'TS'
const [chat, agent, confirm, model] = process.argv.slice(2);
const folder = process.env.CHAUFFEUR_HOME;
const { port } = await Bun.file(`${folder}/daemon.json`).json();
const token = (await Bun.file(`${folder}/token`).text()).trim();
const ws = new WebSocket(`ws://127.0.0.1:${port}/ws?token=${encodeURIComponent(token)}`);
await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
const call = (id, method, params) => new Promise((resolve, reject) => {
  ws.onmessage = ({ data }) => { const r = JSON.parse(data); if (r.id === id) r.error ? reject(new Error(r.error.message)) : resolve(r.result); };
  ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
});
try {
  await call(1, "hello", { protocol_version: 1, client: "e2e-provider-switch" });
  const result = await call(2, "chat.configure", { chat, agent, model, confirm_agent: confirm === "none" ? undefined : confirm });
  if (result.id !== chat || result.agent !== agent || result.agent_session_id !== null) throw new Error(JSON.stringify(result));
} finally { ws.close(); }
TS
  }
  if configure_provider codex none "$CODEX_MODEL" 2>"$T/unconfirmed-switch.log"; then
    echo "a provider change without confirmation was accepted"; exit 1
  fi
  grep -q "confirm the current agent" "$T/unconfirmed-switch.log"
  configure_provider codex claude "$CODEX_MODEL"
  c send "$SWITCH_CHAT" "Which secret word did I ask you to remember? Reply with that word only." >/dev/null
  c --json wait "$SWITCH_CHAT" --timeout 240 | python3 -c 'import sys,json; r=json.load(sys.stdin); assert "APRICOT-83" in (r["last_text"] or ""), r'
  configure_provider claude codex "$MODEL"
  c send "$SWITCH_CHAT" "Which secret word did I ask you to remember? Reply with that word only." >/dev/null
  c --json wait "$SWITCH_CHAT" --timeout 180 | python3 -c 'import sys,json; r=json.load(sys.stdin); assert "APRICOT-83" in (r["last_text"] or ""), r'
  c --json chats provider-switch | python3 -c 'import sys,json; r=json.load(sys.stdin); assert len(r["chats"])==1, r'
  c archive --force provider-switch >/dev/null

  echo "== 5c. Codex's commands are tool calls in the event log, with their output and a failure's exit code"
  # Codex runs them through its code-mode exec tool; each still comes as a commandExecution item.
  c send codex-one "Run the shell command 'echo e2e-codex-out', then the shell command 'ls /e2e-not-here', each as its own tool call. Then reply DONE." >/dev/null
  c wait codex-one --timeout 180 | tail -1
  events "$(chat_id codex-one)" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
calls = {e["tool_use_id"]: e for e in events if e["type"] == "tool_call" and e["name"] == "Bash"}
results = {e["tool_use_id"]: e for e in events if e["type"] == "tool_result"}
def ran(word):
    found = [(c["input"]["command"], results.get(i)) for i, c in calls.items() if word in c["input"].get("command", "")]
    assert found, "the model did not run %s (Bash calls: %s)" % (word, [c["input"] for c in calls.values()])
    return found[-1]
echo, ls = ran("echo e2e-codex-out"), ran("/e2e-not-here")
print(echo, ls)
assert echo[1] and not echo[1]["is_error"] and "e2e-codex-out" in echo[1]["output"], echo
assert ls[1] and ls[1]["is_error"] and ls[1]["output"].startswith("Exit code "), ls'

  echo "== 6. Codex works across both repos"
  c send codex-one "Create a file named hello.txt containing 'hi' in BOTH the alpha and beta folders, then reply DONE."
  c wait codex-one --timeout 240 | tail -3
  test -f "$CHAUFFEUR_WORKSPACES_DIR/codex-one/alpha/hello.txt"
  test -f "$CHAUFFEUR_WORKSPACES_DIR/codex-one/beta/hello.txt"

  echo "== 6b. Codex's Always allow names what Codex no longer asks for: that command, in that folder"
  # A commit leaves the sandbox (the gitdir is outside the workspace): Codex asks in default mode.
  c new --name codex-always >/dev/null
  c chat codex-always --agent codex --model "$CODEX_MODEL" --permission-mode default --prompt "Run exactly this shell command: git -C alpha commit --allow-empty -m e2e-always . If the sandbox blocks it, run it again with escalated permissions. Then reply with the single word committed." >/dev/null
  CX_CHAT=$(chat_id codex-always)
  cx_open() { events "$CX_CHAT" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
resolved = {e["request_id"] for e in events if e["type"] == "request_resolved"}
print(" ".join(e["request_id"] for e in events if e["type"] == "permission_request" and e["request_id"] not in resolved))'; }
  REQ=""
  for _ in $(seq 1 180); do REQ=$(cx_open); [ -n "$REQ" ] && break; sleep 1; done
  [ -n "$REQ" ] || { echo "Codex never asked to commit"; exit 1; }
  c respond codex-always "$REQ" allow --always >/dev/null
  c wait codex-always --timeout 240 | tail -1
  c send codex-always "Run exactly the same shell command again, in the same folder: git -C alpha commit --allow-empty -m e2e-always . Then reply with the single word again." >/dev/null
  c wait codex-always --timeout 240 | tail -1
  test "$(git -C "$CHAUFFEUR_WORKSPACES_DIR/codex-always/alpha" log --oneline | grep -c e2e-always)" = 2
  events "$CX_CHAT" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
asked = [e for e in events if e["type"] == "permission_request"]
first = next(e for e in asked if e["request_id"] == "'"$REQ"'")
resolved = [e for e in events if e["type"] == "request_resolved" and e["request_id"] == "'"$REQ"'"]
print(len(asked), first["input"], resolved)
rule = "Bash(%s) in %s" % (first["input"]["command"], first["input"]["cwd"])
assert resolved and resolved[0].get("rules") == [rule], resolved
again = [e for e in asked if e is not first and e["input"].get("command") == first["input"]["command"] and e["input"].get("cwd") == first["input"]["cwd"]]
assert not again, again'
  c archive --force codex-always >/dev/null

  echo "== 7. Codex's git worktree add is refused and chauffeur worktree add is used"
  c send codex-one "Run exactly this shell command: git -C alpha worktree add ../extra -b extra-branch . If it is refused, follow the instruction you get instead, using the name extra. Then reply with the folder path you ended up with."
  c wait codex-one --timeout 240 | tail -4
  c --json show codex-one | python3 -c '
import sys,json; w=json.load(sys.stdin)
agent=[t for t in w["worktrees"] if t["origin"]=="agent" and not t["removed"]]
print("agent worktrees:", [(t["repo"], t["path"]) for t in agent])
assert agent, "codex did not create its worktree through chauffeur"'
  test ! -d "$CHAUFFEUR_WORKSPACES_DIR/codex-one/extra"

  # macOS only: Codex's Linux sandbox allows every Unix socket or none, and Chauffeur asks for none.
  if [ "$(uname)" = Darwin ]; then
  echo "== 7b. a Codex chat's sandbox reaches the daemon, in plan mode too, without asking to leave it"
  # Plan mode's read-only sandbox, where the thread begins in another sandbox so that Codex starts
  # the network proxy that allows the daemon's socket. Escalating would ask: nothing may be asked.
  c new --name codex-cli >/dev/null
  c chat codex-cli --agent codex --model "$CODEX_MODEL" --permission-mode plan --prompt 'Run exactly this shell command, inside the sandbox, without arguments: `chauffeur ls`. Do not ask for escalated permissions. Reply with its output only.' >/dev/null
  c --json wait codex-cli --timeout 240 >/dev/null
  events "$(chat_id codex-cli)" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
calls = {e["tool_use_id"]: e["input"] for e in events if e["type"] == "tool_call" and "chauffeur ls" in json.dumps(e["input"])}
results = [e["output"] for e in events if e["type"] == "tool_result" and e["tool_use_id"] in calls]
asked = [e for e in events if e["type"] == "permission_request"]
print("calls", list(calls.values()), "results", [r[:200] for r in results], "asked", len(asked))
assert calls, "the model did not run chauffeur ls"
assert any("codex-cli" in r for r in results), results
assert not any("sandbox blocked" in r for r in results), results
assert not asked, asked'
  c archive --force codex-cli >/dev/null
  fi

  echo "== 7c. Codex proposes a repo the workspace lacks through the same card"
  c new --name codex-propose --repo alpha >/dev/null
  c chat codex-propose --agent codex --model "$CODEX_MODEL" --prompt "$GAMMA_PROMPT" >/dev/null
  propose_gamma codex-propose "$(chat_id codex-propose)"
  c archive --force codex-propose >/dev/null

  # Checks that the first chat of a workspace gave its task to a delegated chat of the other agent
  # (on the model given, when one is), which answered the word asked for, and that the parent got
  # that answer back.
  delegated() {
    local slug=$1 parent_agent=$2 sub_agent=$3 word=$4 sub_model=${5:-}
    c --json wait "$slug" --timeout 480 >/dev/null
    c --json chats "$slug" | python3 -c '
import sys, json
chats = json.load(sys.stdin)["chats"]
parent = chats[0]
subs = [ch for ch in chats if ch.get("parent_chat") == parent["id"]]
print([(ch["agent"], ch.get("parent_chat"), ch.get("parent_tool_use")) for ch in chats])
assert parent["agent"] == "'"$parent_agent"'", parent
assert subs, "the model did not call its delegate tool"
assert subs[0]["agent"] == "'"$sub_agent"'" and subs[0].get("parent_tool_use"), subs
assert not "'"$sub_model"'" or subs[0].get("model") == "'"$sub_model"'", subs' 
    local parent sub sub_call
    parent=$(chat_id "$slug")
    sub=$(c --json chats "$slug" | python3 -c 'import sys,json; print(next(ch["id"] for ch in json.load(sys.stdin)["chats"] if ch.get("parent_chat")))')
    sub_call=$(c --json chats "$slug" | python3 -c 'import sys,json; print(next(ch["parent_tool_use"] for ch in json.load(sys.stdin)["chats"] if ch.get("parent_chat")))')
    # `chauffeur wait <chat>`: that chat alone, its own answer.
    c --json wait "$sub" --timeout 60 | python3 -c '
import sys, json
r = json.load(sys.stdin)
print("subagent:", r["last_text"])
assert [ch["id"] for ch in r["chats"]] == ["'"$sub"'"], r["chats"]
assert "'"$word"'" in (r["last_text"] or ""), "the subagent did not answer the word asked"'
    events "$parent" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
calls = {e["tool_use_id"] for e in events if e["type"] == "tool_call" and e["name"] == "mcp__chauffeur__delegate"}
results = [e for e in events if e["type"] == "tool_result" and e["tool_use_id"] in calls]
print("delegate results:", [r["output"][:160] for r in results])
assert calls, "the model did not call its delegate tool"
# The delegated chat keeps the call that started it: the card of the parent finds it by that id.
assert "'"$sub_call"'" in calls, ("'"$sub_call"'", calls)
assert any("'"$word"'" in r["output"] and not r["is_error"] for r in results), results
texts = [e["text"] for e in events if e["type"] == "assistant_text" and not e.get("parent_tool_use_id")]
print("parent:", texts[-1] if texts else None)'
    c archive --force "$slug" >/dev/null
  }

  echo "== 7d. a Claude chat's review goes to Codex, on the fast tier's model, and its answer comes back"
  c new --name delegate-codex --repo alpha --model "$MODEL" --prompt "Use your delegate tool with review set to true, tier fast, no agent, and this exact prompt: 'Reply with the single word KIWI-31 and nothing else.' Then reply with what the subagent answered." >/dev/null
  delegated delegate-codex claude codex KIWI-31 gpt-6-luna

  echo "== 7e. a Codex chat gives a task to Claude with its delegate tool and gets the answer"
  c new --name delegate-claude --repo alpha >/dev/null
  c chat delegate-claude --agent codex --model "$CODEX_MODEL" --prompt "Use your delegate tool with agent claude and this exact prompt: 'Reply with the single word MANGO-47 and nothing else.' Then reply with what the subagent answered." >/dev/null
  delegated delegate-claude codex claude MANGO-47

  echo "== 8. archive stops the Codex chat"
  c archive codex-one
  test ! -d "$CHAUFFEUR_WORKSPACES_DIR/codex-one"

  echo "== 9. a Codex exploration reads and cannot write or commit"
  c new --explore --name explore-codex >/dev/null
  BEFORE=$(heads explore-codex)
  # Reading, then writing, in two messages: asked both at once, a small model may refuse it all.
  c chat explore-codex --agent codex --model "$CODEX_MODEL" --prompt "What is the secret word written in alpha/STAGING.md? Reply with that word only." >/dev/null
  c --json wait explore-codex --timeout 240 | python3 -c 'import sys,json; t=json.load(sys.stdin)["last_text"]; print(t); assert "MARKER-42" in (t or "")'
  c send explore-codex "Now create the file alpha/scratch.txt containing x, and run exactly this shell command: git -C alpha commit --allow-empty -m explore-commit . Report what happened to each step." >/dev/null
  c wait explore-codex --timeout 240 | tail -4
  test ! -e "$CHAUFFEUR_WORKSPACES_DIR/explore-codex/alpha/scratch.txt"
  test "$(heads explore-codex)" = "$BEFORE"
  c archive explore-codex
  test "$(git -C "$T/clones/alpha" worktree list | wc -l | tr -d ' ')" = 1

  echo "== 9b. a Codex design chat writes its own folder only, never asks, and checks its renders in its own browser"
  c new --name design-codex >/dev/null
  CDESIGN=$(c --json chat design-codex --design --agent codex --model "$CODEX_MODEL" | python3 -c '
import sys,json; c=json.load(sys.stdin)
assert c["kind"] == "design" and c["agent"] == "codex" and c["permission_mode"] == "auto", c
print(c["id"])')
  CWS="$CHAUFFEUR_WORKSPACES_DIR/design-codex"
  CDIR="$CWS/.chauffeur/designs/$CDESIGN"
  BEFORE=$(heads design-codex)
  c send "$CDESIGN" "Write the screen login.html in your working directory: a login page with a Reconnect button. Then write canvas.json there, listing it in one section. Reply DONE." >/dev/null
  c --json wait design-codex --timeout 300 >/dev/null
  test -f "$CDIR/login.html" && grep -qi reconnect "$CDIR/login.html"
  python3 -c 'import sys,json; c=json.load(open(sys.argv[1])); assert c["pages"][0]["sections"][0]["screens"], c' "$CDIR/canvas.json"
  # Named in full: a small model may reach for the user's own Playwright server, which asks first and
  # is therefore refused here.
  c send "$CDESIGN" "Call the browser_navigate tool of the MCP server named headless-browser (not any other browser server) with the URL file://$CDIR/login.html, and reply with the page title it reports." >/dev/null
  c --json wait design-codex --timeout 300 >/dev/null
  events "$CDESIGN" | python3 -c '
import sys, json
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
calls = {e["tool_use_id"]: e["name"] for e in events if e["type"] == "tool_call" and e["name"].startswith("mcp__headless-browser__")}
assert calls, "the model did not call the headless browser"
results = [e for e in events if e["type"] == "tool_result" and e["tool_use_id"] in calls]
assert any(not r.get("is_error") for r in results), results'
  # Each write outside its folder in its own message, as edits a user might ask for: patches by
  # path, a shell write, a commit.
  c send "$CDESIGN" "Use apply_patch to add the file $CWS/alpha/hack.txt containing x. Report what happened." >/dev/null
  c --json wait design-codex --timeout 300 >/dev/null
  c send "$CDESIGN" "Use apply_patch to add the file ../../../alpha/hack-rel.txt (relative to your working directory) containing x. Report what happened." >/dev/null
  c --json wait design-codex --timeout 300 >/dev/null
  c send "$CDESIGN" "Run exactly this shell command: echo x > $CWS/alpha/hack2.txt . Report what happened." >/dev/null
  c --json wait design-codex --timeout 300 >/dev/null
  c send "$CDESIGN" "Run exactly this shell command: git -C $CWS/alpha commit --allow-empty -m design-commit . Report what happened." >/dev/null
  c --json wait design-codex --timeout 300 >/dev/null
  # The sandbox holds what the agent runs unawares: a script of its folder that writes into a repo
  # directly, through a symlink and through a hard link, and leaves a symlink for a patch through it.
  printf '#!/bin/sh\nR="%s/alpha"\nfor how in direct symlink hardlink; do\n  case $how in\n    direct) echo x > "$R/probe-direct.txt" ;;\n    symlink) ln -sfn "$R" link && echo x > link/probe-symlink.txt ;;\n    hardlink) ln "$R/README.md" hard.md && echo x >> hard.md ;;\n  esac 2>/dev/null && echo "PROBE-WROTE-$how" || echo "PROBE-BLOCKED-$how"\ndone\n' "$CWS" > "$CDIR/probe.sh"
  c send "$CDESIGN" "Run exactly this shell command from your working directory: sh probe.sh . It is a check script of this folder. Reply with its output only." >/dev/null
  c --json wait design-codex --timeout 300 >/dev/null
  c send "$CDESIGN" "Use apply_patch to add the file link/notes.txt (relative to your working directory) containing x. Report what happened." >/dev/null
  c --json wait design-codex --timeout 300 >/dev/null
  events "$CDESIGN" | python3 -c '
import sys, json, re
events = [json.loads(line)["event"]["event"] for line in sys.stdin if line.strip()]
names = [e["name"] for e in events if e["type"] == "tool_call"]
print("tool calls:", names)
out = " ".join(json.dumps(e) for e in events if e["type"] == "tool_result")
# The script names its markers PROBE-...-$how: a result that holds the script itself matches neither.
wrote = re.findall(r"PROBE-WROTE-(direct|symlink|hardlink)", out)
assert not wrote, ("the sandbox let the probe write into a repo", wrote)
assert len(set(re.findall(r"PROBE-BLOCKED-(direct|symlink|hardlink)", out))) == 3, "the model did not run probe.sh"
asked = [e for e in events if e["type"] in ("permission_request", "mcp_elicitation", "question")]
assert not asked, asked'
  for f in hack.txt hack-rel.txt hack2.txt probe-direct.txt probe-symlink.txt notes.txt; do
    if [ -e "$CWS/alpha/$f" ]; then echo "a Codex design chat wrote alpha/$f"; exit 1; fi
  done
  git -C "$CWS/alpha" diff --quiet README.md
  test "$(heads design-codex)" = "$BEFORE"
  c archive design-codex --force >/dev/null
fi
if [ -n "${OLLAMA:-}" ]; then
  # Needs Ollama installed and the model on disk (`chauffeur ollama pull qwen3.8`); no Codex login.
  # A local model takes up to a minute to load and read Codex's prompt.
  OLLAMA_MODEL=${OLLAMA_MODEL:-qwen3.8}
  echo "== 10. a Codex chat on a local model answers, through an Ollama Chauffeur starts if none runs"
  c --json ollama | python3 -c '
import sys,json; s=json.load(sys.stdin); m=sys.argv[1]
assert s["executable"], ("Ollama is not installed", s)
assert any(x["name"] == m for x in s["models"]), ("the model is not on disk: chauffeur ollama pull " + m, s)' "$OLLAMA_MODEL"
  c new --name local-one --repo alpha >/dev/null
  c chat local-one --agent codex --model "ollama/$OLLAMA_MODEL" --prompt "Reply with the single word pong and nothing else." >/dev/null
  c wait local-one --timeout 900 | tee "$T/local1.txt"
  grep -qiE '^pong\.?$' "$T/local1.txt"
  c --json ollama | python3 -c 'import sys,json; s=json.load(sys.stdin); assert s["running"] in ("daemon", "external"), s'
  c --json chats local-one | python3 -c '
import sys,json; chat=json.load(sys.stdin)["chats"][0]
assert chat["agent"] == "codex" and chat["model"] == "ollama/" + sys.argv[1] and chat["agent_session_id"], chat' "$OLLAMA_MODEL"
  c archive local-one --force >/dev/null
fi
echo "E2E CHAT OK"
