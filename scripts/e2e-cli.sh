#!/usr/bin/env bash
# End-to-end check of the CLI against a real daemon, in a throwaway CHAUFFEUR_HOME.
set -euo pipefail
cd "$(dirname "$0")/.."
DAEMON_PACKAGE=chauffeurd
DAEMON_BINARY=chauffeurd
if [[ -f distribution/build-env.sh ]]; then
  # shellcheck source=/dev/null
  source distribution/build-env.sh
fi
# shellcheck source=release-channel.sh
source scripts/release-channel.sh
UPDATE_DMG="$CHAUFFEUR_DMG_PREFIX-99.0.0.dmg"
cargo build -q -p "$DAEMON_PACKAGE" -p chauffeur-cli
BIN="$PWD/target/debug"
T=$(mktemp -d)
T=$(cd "$T" && pwd -P)
export CHAUFFEUR_HOME="$T/home" CHAUFFEUR_WORKSPACES_DIR="$T/workspaces" CHAUFFEUR_REPOS_DIR="$T/repos"
# Run from a Chauffeur workspace, the CLI would refuse to start this throwaway daemon.
unset CHAUFFEUR_WORKSPACE_ID
export CHAUFFEUR_PORT_OVERRIDE=0 CHAUFFEURD="$BIN/$DAEMON_BINARY"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
cleanup() { "$BIN/chauffeur" daemon stop >/dev/null 2>&1 || true; rm -rf "$T"; }
trap cleanup EXIT

for name in alpha beta; do
  mkdir -p "$T/seed/$name" && git -C "$T/seed/$name" init -q -b main
  echo "$name" > "$T/seed/$name/README.md"
  git -C "$T/seed/$name" add -A && git -C "$T/seed/$name" commit -q -m init
  git clone -q --bare "$T/seed/$name" "$T/origin/$name.git"
  git clone -q "$T/origin/$name.git" "$T/clones/$name"
done
mkdir -p "$CHAUFFEUR_HOME"
cat > "$CHAUFFEUR_HOME/settings.toml" <<TOML
github_org = ""
github_sources = false
default_repos = ["alpha", "beta"]
repo_order = ["beta", "alpha"]
branch_prefix = "e2e"
clone_roots = ["$T/clones"]
port_range_start = 47100
linear_enabled = false
daily_review = { enabled = false }
# The developer's own Ollama is never started for search.
search = { semantic = false }
[office]
url = ""
# Explicit, so a distribution's own share service is never reached.
[design_share]
url = "" # e2e share url
TOML

c() { "$BIN/chauffeur" "$@"; }
c daemon start
if [[ -f distribution/e2e-extensions.sh ]]; then
  # shellcheck source=/dev/null
  source distribution/e2e-extensions.sh
fi
echo "== slack. a fresh daemon has no connected account and cannot import before sign-in"
bun scripts/app-call.ts slack.receive '{"focused":true,"ack":null}' | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r == {"link":None}, r'
bun scripts/app-call.ts slack.status '{}' | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r == {"identity":None,"pending":False}, r'
if bun scripts/app-call.ts slack.context '{"link":"chauffeur://slack/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?scope=message"}' >"$T/slack-error" 2>&1; then
  echo "a signed-out daemon imported Slack context"; exit 1
fi
grep -q 'Connecte ton compte' "$T/slack-error"
echo "== errors. --json prints the daemon's error on stdout and exits by its kind"
set +e; missing=$(c --json show no-such-workspace 2>/dev/null); status=$?; set -e
[ "$status" = 3 ] || { echo "a missing workspace exited with $status"; exit 1; }
echo "$missing" | python3 -c 'import sys,json; e=json.load(sys.stdin)["error"]; assert e["code"] == 404 and "no-such-workspace" in e["message"], e'
echo "== no repos. only designs skip defaults and accept a repo later"
if bun scripts/app-call.ts workspace.create '{"name":"refused","no_repos":true}' >"$T/no-repos-error" 2>&1; then
  echo "a regular workspace was created without repos"; exit 1
fi
grep -q 'only a design' "$T/no-repos-error"
bun scripts/app-call.ts workspace.create '{"name":"standalone","no_repos":true,"chat_kind":"design","explore":true}' >/dev/null
c --json show standalone | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["worktrees"] == [] and w["state"] == "ready", w'
c add-repo standalone alpha
c --json show standalone | python3 -c 'import sys,json; w=json.load(sys.stdin); assert len(w["worktrees"]) == 1 and w["worktrees"][0]["repo"] == "alpha", w'
c archive standalone --force
echo "== repos. the saved display order is used by the CLI"
c --json repos | python3 -c 'import sys,json; r=json.load(sys.stdin); assert [repo["name"] for repo in r["repos"]] == ["beta", "alpha"], r'
c new --ticket E2E-1 --name e2e-one
c ls
# Sections: one created, a workspace filed in it and listed under its header; deleted, the
# workspace goes back to no section.
c section new Back end | grep -q "^Back end "
c move e2e-one "back END" | grep -q "in section Back end$"
c ls | grep -q "^── Back end (1)$"
c --json ls | python3 -c '
import sys, json
r = json.load(sys.stdin)
w = [w for w in r["workspaces"] if w["slug"] == "e2e-one"][0]
assert w["section"] == "Back end" and w["section_id"] == r["sections"][0]["id"], r
assert [s["name"] for s in r["sections"]] == ["Back end"], r'
if c section new back end 2>/dev/null; then echo "a second section took the same name"; exit 1; fi
c section rm "Back end" | grep -q "^no sections"
c --json show e2e-one | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["section"] is None and w["section_id"] is None, w'
test -d "$CHAUFFEUR_WORKSPACES_DIR/e2e-one/alpha" && test -d "$CHAUFFEUR_WORKSPACES_DIR/e2e-one/beta"
PATH_A=$(c --json worktree add --workspace e2e-one --repo alpha --name probe | python3 -c 'import sys,json; print(json.load(sys.stdin)["path"])')
test -d "$PATH_A/alpha"
c worktree remove probe --workspace e2e-one
test ! -d "$PATH_A/alpha"
c --json show e2e-one | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="ready", w'
c status e2e-one

echo "== routines. made from a chat in a workspace of its own, edited as versions, run by hand, activated"
c new --name e2e-routine-src --prompt "Triage the new tickets" >/dev/null
c --json routine new --from e2e-routine-src --name Triage | python3 -c '
import sys, json
r = json.load(sys.stdin)
assert r["state"] == "draft" and r["version"] == 1, r
assert r["spec"]["prompt"].strip() == "Triage the new tickets" and sorted(r["spec"]["repos"]) == ["alpha", "beta"], r'
ROUTINE_WS=$(c --json routine show Triage | python3 -c 'import sys,json; print(json.load(sys.stdin)["workspace_id"])')
c --json show "$ROUTINE_WS" | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["section"] == "Routines" and w["slug"] == "routine-triage", w'
ROUTINE_FILE="$CHAUFFEUR_WORKSPACES_DIR/routine-triage/.chauffeur/routine.toml"
sed 's/"08:00"/"07:30"/' "$ROUTINE_FILE" > "$T/routine.toml"
c routine edit Triage --file "$T/routine.toml" | grep -q "v2"
grep -q '"07:30"' "$ROUTINE_FILE"
if printf 'name = "Triage"\n' | c routine edit Triage --file - 2>/dev/null; then echo "an invalid routine file was saved"; exit 1; fi
# Inside the routine's workspace, the routine is that workspace's.
CHAUFFEUR_WORKSPACE_ID="$ROUTINE_WS" c --json routine show | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r["version"] == 2, r'
c --json routine run Triage | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r["trigger"] == "manual" and r["version"] == 2, r'
c routine activate Triage | grep -q "(active, next run "
c routine history Triage | grep -q "^run .* by hand, v2"
c routine pause Triage | grep -q "(paused)"
c routine rm Triage | grep -q "its workspace stays"
c routine ls | grep -q "^no routines"
c archive routine-triage --force >/dev/null
c archive e2e-routine-src --force >/dev/null
# Find routines: nothing waits before an analysis, and one with no day to read is refused.
c routine suggestions | grep -q "^no suggestion waiting"
if c routine find --days 0 2>/dev/null; then echo "an analysis of 0 days started"; exit 1; fi

if [[ -f distribution/e2e-database.sh ]]; then
  # shellcheck source=/dev/null
  source distribution/e2e-database.sh
fi
# Chats: a second one renamed, closed (listed with --all only), reopened, then deleted for good.
chat_ids() { c --json chats e2e-one "$@" | python3 -c 'import sys,json; print(" ".join(c["id"] for c in json.load(sys.stdin)["chats"]))'; }
FIRST=$(c --json chat e2e-one | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
SECOND=$(c --json chat e2e-one | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
c chat rename "$SECOND" Second look | grep -q "Second look$"
c chat close "$SECOND" | grep -q "\[closed, last active "
test "$(chat_ids)" = "$FIRST"
c chats e2e-one --all | grep -q "^$SECOND .*Second look  \[closed"
c chat reopen "$SECOND" >/dev/null
test "$(chat_ids)" = "$FIRST $SECOND"
if c chat delete "$SECOND" </dev/null 2>/dev/null; then echo "a chat was deleted without asking"; exit 1; fi
c chat delete "$SECOND" --yes >/dev/null
test "$(chat_ids --all)" = "$FIRST"
if c chat rename "$SECOND" again 2>/dev/null; then echo "a deleted chat was renamed"; exit 1; fi
# A shell command (`!`), the app's way (the Unix socket the CLI uses refuses it): run in the
# workspace folder, no agent started.
shell_params() { python3 -c 'import sys,json; print(json.dumps({"chat": sys.argv[1], "command": sys.argv[2]}))' "$@"; }
# The call answers once the command started; its `shell_ended` says how it ended.
SHELL_ID=$(bun scripts/app-call.ts chat.shell "$(shell_params "$FIRST" 'pwd; echo "$CHAUFFEUR_WORKSPACE_NAME" >&2; exit 4')" | python3 -c 'import sys,json; print(json.load(sys.stdin)["shell_id"])')
shell_ended() { sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "select payload from events where payload like '%\"shell_ended\"%' and payload like '%$1%'"; }
for _ in $(seq 1 100); do [ -n "$(shell_ended "$SHELL_ID")" ] && break; sleep 0.1; done
shell_ended "$SHELL_ID" | python3 -c '
import sys, json
r = json.loads(sys.stdin.read())["event"]["event"]
assert r["exit_code"] == 4 and not r["timed_out"], r
assert r["output"].split("\n")[:2] == [sys.argv[1], "e2e-one"], r' "$(cd "$CHAUFFEUR_WORKSPACES_DIR/e2e-one" && pwd -P)"
c --json chats e2e-one | python3 -c 'import sys,json; c=json.load(sys.stdin)["chats"][0]; assert c["status"] == "idle" and not c.get("agent_session_id"), c'
# A design chat: on Claude Code unless Codex is asked for, its permission mode fixed; deleted like any chat.
DESIGN=$(c --json chat e2e-one --design | python3 -c '
import sys,json; c=json.load(sys.stdin)
assert c["kind"] == "design" and c["agent"] == "claude" and c["permission_mode"] == "auto" and "brief" not in c, c
print(c["id"])')
CODEX_DESIGN=$(c --json chat e2e-one --design --agent codex | python3 -c '
import sys,json; c=json.load(sys.stdin)
assert c["kind"] == "design" and c["agent"] == "codex" and c["permission_mode"] == "auto", c
print(c["id"])')
c chat delete "$CODEX_DESIGN" --yes >/dev/null
if c chat e2e-one --from "$FIRST" 2>/dev/null; then echo "a chat that is not a design chat started from another"; exit 1; fi
# Its canvas: canvas.json read from its folder, the screens served on their own origin only.
c canvas "$DESIGN" | grep -q "no screens yet"
DDIR="$CHAUFFEUR_WORKSPACES_DIR/e2e-one/.chauffeur/designs/$DESIGN"
mkdir -p "$DDIR" && echo '<p>signed out</p>' > "$DDIR/signed-out.html" && echo 'x' > "$DDIR/notes.txt"
echo '{"pages": [{"name": "Login", "sections": [{"title": "Sign in", "screens": [{"file": "signed-out.html", "label": "Signed out"}, {"file": "gone.html"}]}]}]}' > "$DDIR/canvas.json"
c canvas "$DESIGN" | grep -q "gone.html  390×844  \[missing\]"
URL=$(c --json canvas "$DESIGN" | python3 -c '
import sys,json; c=json.load(sys.stdin); s=c["pages"][0]["sections"][0]["screens"]
assert s[0]["label"] == "Signed out" and "url" not in s[1], c
print(s[0]["url"])')
curl -sfD "$T/screen-headers" "$URL" | grep -q "signed out"
grep -qi "^content-security-policy: .*frame-ancestors" "$T/screen-headers"
test "$(curl -s -o /dev/null -w '%{http_code}' "${URL%signed-out.html}notes.txt")" = 404
# Export PNG: only screens of the canvas that are in the folder (the rendering itself needs a
# Chrome and the clipboard: covered by the daemon's tests with a fake, never run here).
if EXPORT_ERROR=$(c export "$DESIGN" gone.html 2>&1); then
  echo "an export of a missing screen was accepted"; exit 1
fi
printf '%s\n' "$EXPORT_ERROR" | grep -q "gone.html is missing from the design folder"
if EXPORT_ERROR=$(c export "$DESIGN" other.html 2>&1); then
  echo "an export of an unlisted screen was accepted"; exit 1
fi
printf '%s\n' "$EXPORT_ERROR" | grep -q "other.html is not a screen of this design"
if c export "$DESIGN" 2>/dev/null; then echo "an export without screens was accepted"; exit 1; fi
# Comments (story 51): on a screen of the canvas, in comments.json, listed by screen; a reply follows its thread.
c comments "$DESIGN" | grep -q "no comments yet"
# Sharing (story 51): off without a share service (Core names none); with one, the state of an
# unshared design, and without the Office's sign-in a share is refused.
c share "$DESIGN" | grep -q "sharing is off"
sed -i.bak 's|^url = "" # e2e share url$|url = "https://share.example.com"|' "$CHAUFFEUR_HOME/settings.toml"
sleep 1.5
c share "$DESIGN" | grep -q "not shared"
if c share "$DESIGN" --with bob@example.com 2>/dev/null; then echo "a design was shared without a sign-in"; exit 1; fi
if c share "$DESIGN" --team 2>/dev/null; then echo "a design was opened to the team without a sign-in"; exit 1; fi
if c share "$FIRST" 2>/dev/null; then echo "a plain chat had a share state"; exit 1; fi
# Workspace sharing (story 53): unshared at first; refused without the sign-in; no suggestion to dismiss.
c share-workspace e2e-one | grep -q "not shared"
if c share-workspace e2e-one --suggest bob@example.com 2>/dev/null; then echo "a workspace was shared without a sign-in"; exit 1; fi
if c share-workspace e2e-one --stop 2>/dev/null; then echo "an unshared workspace was stopped"; exit 1; fi
if c import-design e2e-one "https://share.example.com/s/0123456789abcdef" 2>/dev/null; then echo "a design was imported without a sign-in"; exit 1; fi
if c comment "$DESIGN" other.html "nope" 2>/dev/null; then echo "a comment on an unknown screen was accepted"; exit 1; fi
CID=$(c --json comment "$DESIGN" signed-out.html "Too low" --at 48,83 | python3 -c 'import sys,json; c=json.load(sys.stdin); assert c["pin"]["x"] == 48 and "sent" not in c, c; print(c["id"])')
c comment "$DESIGN" signed-out.html "Seen" --reply-to "$CID" >/dev/null
python3 -c 'import json,sys; f=json.load(open(sys.argv[1])); assert [c.get("reply_to") for c in f["comments"]] == [None, sys.argv[2]], f' "$DDIR/comments.json" "$CID"
c comments "$DESIGN" | grep -q "signed-out.html"
c comments "$DESIGN" | grep -q "\[new\]: Too low"
c comments "$DESIGN" | grep -q "      .*: Seen"
# A markdown file's comments (story 57): on a passage of a plan, kept in the notes' comments.json;
# a reply follows its thread; any other file is refused; sending needs a chat that wrote the file.
mkdir -p "$CHAUFFEUR_WORKSPACES_DIR/e2e-one/.chauffeur/plans" && printf '# Plan\n\nShip it on Friday.\n' > "$CHAUFFEUR_WORKSPACES_DIR/e2e-one/.chauffeur/plans/plan.md"
c file-comments e2e-one .chauffeur/plans/plan.md | grep -q "no comments yet"
if c file-comment e2e-one .chauffeur/designs/"$DESIGN"/canvas.json "nope" 2>/dev/null; then echo "a comment on a file that is not markdown was accepted"; exit 1; fi
FCID=$(c --json file-comment e2e-one .chauffeur/plans/plan.md "Too early" --quote "on Friday" | python3 -c 'import sys,json; c=json.load(sys.stdin); assert c["quote"] == "on Friday" and c["screen"] == ".chauffeur/plans/plan.md", c; print(c["id"])')
c file-comment e2e-one .chauffeur/plans/plan.md "Monday then" --reply-to "$FCID" >/dev/null
python3 -c 'import json,sys; f=json.load(open(sys.argv[1])); assert [c.get("reply_to") for c in f["comments"]] == [None, sys.argv[2]], f' "$CHAUFFEUR_WORKSPACES_DIR/e2e-one/.chauffeur/comments.json" "$FCID"
c file-comments e2e-one .chauffeur/plans/plan.md | grep -q 'on "on Friday"  \[new\]: Too early'
if out=$(c file-comments e2e-one .chauffeur/plans/plan.md --send 2>&1); then echo "comments were sent with no chat to send them to"; exit 1; fi
grep -q "say which chat with --to" <<<"$out"
# A markdown file shared alone (story 57): unshared at first; refused without the sign-in, and for
# a file that is not markdown.
c share-file e2e-one .chauffeur/plans/plan.md | grep -q "not shared"
if c share-file e2e-one .chauffeur/plans/plan.md --with bob@example.com 2>/dev/null; then echo "a file was shared without a sign-in"; exit 1; fi
if c share-file e2e-one .chauffeur/comments.json 2>/dev/null; then echo "a file that is not markdown had a share state"; exit 1; fi
if c share-file e2e-one .chauffeur/plans/plan.md --stop 2>/dev/null; then echo "an unshared file was stopped"; exit 1; fi
# The capture script only on a screen framed by the app (capture=1), never on the file nor on a plain fetch.
curl -s "$URL" | grep -q "chauffeur:element" && { echo "the capture script was served without capture=1"; exit 1; }
curl -s "$URL?capture=1" | grep -q "chauffeur:element"
grep -q "chauffeur:element" "$DDIR/signed-out.html" && { echo "the capture script was written into the file"; exit 1; }
c chat delete "$DESIGN" --yes >/dev/null
test "$(chat_ids --all)" = "$FIRST"
# Tickets attached and detached after creation: with Linear off, by identifier only, as at creation.
c ticket add e2e-2 E2E-3 --workspace e2e-one | grep -q "E2E-2 .*(not read from Linear)"
c --json show e2e-one | python3 -c '
import sys, json
tickets = json.load(sys.stdin)["tickets"]
assert [t["identifier"] for t in tickets] == ["E2E-1", "E2E-2", "E2E-3"], tickets
assert all(t["title"] is None for t in tickets), tickets'
grep -q "Linear tickets: E2E-1, E2E-2, E2E-3" "$CHAUFFEUR_WORKSPACES_DIR/e2e-one/CLAUDE.md"
c ticket remove E2E-1 e2e-3 --workspace e2e-one >/dev/null
c ticket refresh --workspace e2e-one >/dev/null
test "$(c --json show e2e-one | python3 -c 'import sys,json; print(" ".join(t["identifier"] for t in json.load(sys.stdin)["tickets"]))')" = "E2E-2"
if c ticket add --workspace e2e-one -- '--upload-pack=touch /tmp/pwned' 2>/dev/null; then echo "a bad ticket was attached"; exit 1; fi
test ! -e "$CHAUFFEUR_WORKSPACES_DIR/e2e-one/.chauffeur/tickets.md"
c archive e2e-one
test ! -d "$CHAUFFEUR_WORKSPACES_DIR/e2e-one"
c ls --all
c restore e2e-one
test -f "$CHAUFFEUR_WORKSPACES_DIR/e2e-one/alpha/README.md"
c archive e2e-one --force

# Jev: off until a key is saved and it is turned on; the key stays in this daemon's secrets.
c jev | grep -q "off, from 60% of the context, no TypeSafe key"
if c jev on 2>/dev/null; then echo "Jev turned on without a key"; exit 1; fi
echo "ts_e2e-key" | c jev key >/dev/null
test "$(cat "$CHAUFFEUR_HOME/secrets/jev-api-key")" = "ts_e2e-key"
c jev on >/dev/null
c jev threshold 40 | grep -q "on, from 40% of the context, TypeSafe key saved"
if c jev threshold 0 2>/dev/null; then echo "a 0% threshold was taken"; exit 1; fi
c jev key --remove | grep -q "off, from 40% of the context, no TypeSafe key"
test ! -e "$CHAUFFEUR_HOME/secrets/jev-api-key"

# The Linear key goes through the daemon too, into this daemon's own secrets.
echo "lin_api_e2e" | c linear login >/dev/null
test "$(cat "$CHAUFFEUR_HOME/secrets/linear-api-key")" = "lin_api_e2e"

# The PRs an agent opened from its own worktrees (one per sub-ticket, say) are the workspace's too:
# listed, then merged, the second stacked on the first and so pointed at main before its merge.
# gh is a stand-in knowing those two PRs; the daemon finds it through a login "shell" that puts it
# first on PATH.
c daemon stop >/dev/null
mkdir -p "$T/bin"
cat > "$T/bin/gh" <<GH
#!/bin/bash
pr() { echo "[{\"number\":\$1,\"baseRefName\":\"\$2\",\"url\":\"https://example.com/alpha/pull/\$1\",\"state\":\"OPEN\",\"title\":\"Sub-ticket\",\"isDraft\":false,\"statusCheckRollup\":[],\"reviewDecision\":\"\",\"mergeable\":\"MERGEABLE\"}]"; }
case "\$1 \$2" in
  "api --hostname")
    case "\$4" in
      user) echo '{"login":"octocat"}' ;;
      user/orgs) echo acme ;;
      *) exit 1 ;;
    esac ;;
  "pr list")
    for arg in "\$@"; do [ "\$prev" = --head ] && head=\$arg; prev=\$arg; done
    case "\$head" in
      *--sub-1) pr 7 main ;;
      *--sub-2) pr 8 e2e/e2e-prs--sub-1 ;;
      e2e/e2e-prs) if [ "\$(basename "\$PWD")" = alpha ]; then pr 5 main; else echo '[]'; fi ;;
      *) echo '[]' ;;
    esac ;;
  "repo view") echo '{"squashMergeAllowed":true,"mergeCommitAllowed":true,"rebaseMergeAllowed":false,"viewerPermission":"WRITE"}' ;;
  # Every repo the account reaches: one of its own, one of an organization named like a clone.
  "api --paginate")
    if [ "\$3" = --slurp ]; then
      [ -d "$T/release" ] || exit 1
      echo '[[{"tag_name":"core-v99.0.0","draft":false,"prerelease":false,"assets":[{"name":"$UPDATE_DMG"},{"name":"$UPDATE_DMG.sha256"}]}]]'
      exit 0
    fi
    echo '{"name":"blog","full_name":"octocat/blog","private":true,"owner_type":"User","default_branch":"main","description":"Mine","pushed_at":"2026-11-01T00:00:00Z"}'
    echo '{"name":"alpha","full_name":"someorg/alpha","private":false,"owner_type":"Organization","default_branch":"main","description":null,"pushed_at":null}' ;;
  "repo clone") git clone -q "$T/origin/alpha.git" "\$4" ;;
  "pr edit") echo "edit \$3 \$4 \$5" >> "$T/merges.log" ;;
  "pr merge") echo "\$(basename "\$PWD") \$3 \$4" >> "$T/merges.log" ;;
  # A newer release, once the update step below published one in $T/release.
  "release view") [ -d "$T/release" ] || exit 1
    echo '{"tagName":"v99.0.0","assets":[{"name":"$UPDATE_DMG"},{"name":"$UPDATE_DMG.sha256"}]}' ;;
  "release download") for arg in "\$@"; do [ "\$prev" = --dir ] && dir=\$arg; prev=\$arg; done
    cp "$T/release/"* "\$dir/" ;;
  *) exit 1 ;;
esac
GH
printf '#!/bin/bash\nexport PATH="%s/bin:$PATH"\n[ -f "%s/profile" ] && . "%s/profile"\nexec /bin/bash -c "$4"\n' "$T" "$T" "$T" > "$T/login-shell"
chmod +x "$T/bin/gh" "$T/login-shell"
SHELL="$T/login-shell" c daemon start >/dev/null
echo "== github. an existing login is detected with a gh lacking auth status JSON flags"
c --json github status | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r["account"] == "octocat" and r["organizations"] == ["acme"], r'
echo "== repos. every GitHub owner's repos, owner/name outside the main organization, cloned per owner"
sed -i.bak 's/^github_sources = false$/github_sources = true/' "$CHAUFFEUR_HOME/settings.toml"
sleep 1.5
c --json repos --refresh | python3 -c '
import sys, json
repos = {r["key"]: r for r in json.load(sys.stdin)["repos"]}
assert set(repos) == {"octocat/blog", "someorg/alpha", "alpha", "beta"}, repos
blog = repos["octocat/blog"]
assert (blog["name"], blog["owner"], blog["owner_is_org"], blog["private"], blog["local"], blog["clone_path"]) == ("blog", "octocat", False, True, False, None), blog
# The clone named alpha is not someorg/alpha'"'"'s: it stays, on this machine only.
assert repos["alpha"]["local"] and not repos["someorg/alpha"]["clone_path"], repos
'
if c new --name e2e-twice --repo alpha --repo someorg/alpha > "$T/twice.out" 2>&1; then exit 1; fi
grep -q "would share the folder alpha" "$T/twice.out"
c new --name e2e-owner --repo octocat/blog >/dev/null
test -d "$CHAUFFEUR_REPOS_DIR/octocat/blog/.git"
OWNER_WS=$(c --json show e2e-owner | python3 -c 'import sys,json; w=json.load(sys.stdin); assert [t["repo"] for t in w["worktrees"]] == ["blog"], w; print(w["path"])')
test -f "$OWNER_WS/blog/README.md"
c --json repos | python3 -c 'import sys,json; r={r["key"]: r for r in json.load(sys.stdin)["repos"]}["octocat/blog"]; assert r["clone_path"].endswith("/repos/octocat/blog") and r["last_used"], r'
c archive e2e-owner --force >/dev/null
echo "== repos. a short name GitHub alone knows resolves while nothing is cached (an upgrade empties it)"
sqlite3 "$CHAUFFEUR_HOME/chauffeur.db" "delete from github_repos"
c new --name e2e-upgrade --repo blog >/dev/null
c --json show e2e-upgrade | python3 -c 'import sys,json; w=json.load(sys.stdin); assert [t["repo"] for t in w["worktrees"]] == ["blog"], w'
c archive e2e-upgrade --force >/dev/null
mv "$CHAUFFEUR_HOME/settings.toml.bak" "$CHAUFFEUR_HOME/settings.toml"
sleep 1.5
c new --name e2e-prs >/dev/null
for sub in sub-1 sub-2; do
  SUB=$(c --json worktree add --workspace e2e-prs --repo alpha --name $sub | python3 -c 'import sys,json; print(json.load(sys.stdin)["path"])')
  echo $sub > "$SUB/alpha/$sub.txt" && git -C "$SUB/alpha" add -A && git -C "$SUB/alpha" commit -q -m $sub
done
c --json prs e2e-prs | python3 -c '
import sys, json
prs = json.load(sys.stdin)["prs"]
subs = [(p["group"], p["number"], p["ahead"]) for p in prs if p["group"]]
assert subs == [("sub-1", 7, 1), ("sub-2", 8, 1)], prs
assert [p["repo"] for p in prs if p["group"] is None] == ["alpha", "beta"], prs
'
c prs e2e-prs | grep -q "alpha (sub-2)"
# Both repos are on the workspace's branch, only alpha has a PR on it: merging beta's merges nothing.
if c merge e2e-prs --pr beta:e2e/e2e-prs > "$T/merge-beta.out"; then exit 1; fi
grep -q "beta has no open pull request on e2e/e2e-prs" "$T/merge-beta.out"
test ! -e "$T/merges.log"
c merge e2e-prs | grep -q "alpha .*--sub-2 merged"
test "$(cat "$T/merges.log")" = "$(printf 'alpha 5 --squash\nalpha 7 --squash\nedit 8 --base main\nalpha 8 --squash')"
# An image an agent's answer names is served from the workspace, with the token; no other file is.
WS=$(c --json show e2e-prs | python3 -c 'import sys,json; w=json.load(sys.stdin); print(w["id"], w["path"])')
ID=${WS%% *}; WS_PATH=${WS#* }
python3 -c 'import zlib, struct, sys
def chunk(kind, data): return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
rows = b"".join(b"\x00" + b"\xff\x00\x00" * 8 for _ in range(8))
sys.stdout.buffer.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 8, 8, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))' > "$WS_PATH/alpha/pixel.png"
URL="http://127.0.0.1:$(python3 -c "import json; print(json.load(open('$CHAUFFEUR_HOME/daemon.json'))['port'])")/images/$ID"
TOKEN=$(cat "$CHAUFFEUR_HOME/token")
test "$(curl -s -o /dev/null -w '%{http_code} %{content_type}' "$URL?token=$TOKEN&path=alpha/pixel.png")" = "200 image/png"
test "$(curl -s -o /dev/null -w '%{http_code}' "$URL?token=$TOKEN&path=$WS_PATH/alpha/pixel.png")" = "200"
test "$(curl -s -o /dev/null -w '%{http_code}' "$URL?token=$TOKEN&path=alpha/README.md")" = "404"
test "$(curl -s -o /dev/null -w '%{http_code}' "$URL?path=alpha/pixel.png")" = "401"
# The notification sound: a file of the user's is served only while it is set, and only as audio.
SOUND_URL="${URL%/images/*}/sound"
test "$(curl -s -o /dev/null -w '%{http_code}' "$SOUND_URL?token=$TOKEN")" = "404"
printf 'RIFF\0\0\0\0WAVEfmt ' > "$T/ding.wav"
printf '\n[notifications]\nsound = "%s"\n' "$T/ding.wav" >> "$CHAUFFEUR_HOME/settings.toml"
sleep 1.5   # the daemon reads settings.toml again within a second
test "$(curl -s -o /dev/null -w '%{http_code} %{content_type}' "$SOUND_URL?token=$TOKEN")" = "200 audio/wav"
test "$(curl -s -o /dev/null -w '%{http_code}' "$SOUND_URL")" = "401"
# A variable exported in the login shell's profile since the daemon started: reloading the
# environment names it, never its value.
c env reload | grep -q "Nothing changed"
echo 'export E2E_MCP_TOKEN=s3cr3t' > "$T/profile"
c env reload > "$T/reload.out"
grep -q "Added: E2E_MCP_TOKEN" "$T/reload.out"
if grep -q s3cr3t "$T/reload.out"; then exit 1; fi
c --json env reload | python3 -c '
import sys, json
r = json.load(sys.stdin)
assert r["added"] == [] and r["profile"] == "~/.profile" and r["shell"].endswith("/login-shell"), r'
c archive e2e-prs --force >/dev/null
# An exploration continues as a workspace only once what its commands left in its checkouts is
# discarded, on the user's say-so: refused with the files named, then --discard deletes them.
c new --explore --name e2e-look >/dev/null
LOOK="$CHAUFFEUR_WORKSPACES_DIR/e2e-look"
echo scribble >> "$LOOK/alpha/README.md"
mkdir -p "$LOOK/beta/.cache" && touch "$LOOK/beta/.cache/out"
if c promote e2e-look 2> "$T/promote.err"; then echo "promoted with leftovers"; exit 1; fi
# Creation timestamps and saved repo order can change the display order of leftovers.
python3 - "$T/promote.err" <<'PY_CHECK'
import pathlib, sys
message = pathlib.Path(sys.argv[1]).read_text()
print(message.strip())
assert "alpha: README.md" in message and "beta: .cache/" in message, message
PY_CHECK
grep -q -- "--discard" "$T/promote.err"
grep -q scribble "$LOOK/alpha/README.md" && test -e "$LOOK/beta/.cache/out"
c --json show e2e-look | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["explore"], w'
c promote e2e-look --discard | grep -q e2e-look
test "$(cat "$LOOK/alpha/README.md")" = alpha && test ! -e "$LOOK/beta/.cache"
test "$(git -C "$LOOK/alpha" branch --show-current)" = e2e/e2e-look
c archive e2e-look --force >/dev/null

echo "== update. a newer release replaces the app the daemon runs from"
# A daemon built from source never offers one.
c --json update | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r["available"] is None and r["phase"] == "idle", r'
if [ "$(uname)" = Darwin ]; then
  mkdir -p "$T/image/Chauffeur.app/Contents/MacOS" "$T/staged" "$T/Applications/Chauffeur.app/Contents/MacOS"
  echo new > "$T/image/Chauffeur.app/Contents/MacOS/chauffeurd"
  echo old > "$T/Applications/Chauffeur.app/Contents/MacOS/chauffeurd"
  hdiutil create -quiet -format UDZO -srcfolder "$T/image" "$T/staged/$UPDATE_DMG"
  (cd "$T/staged" && shasum -a 256 "$UPDATE_DMG" > "$UPDATE_DMG.sha256")
  c daemon stop >/dev/null
  # The daemon passes for one run from $T/Applications/Chauffeur.app. Nothing is published yet.
  CHAUFFEUR_RELEASE_REPO=acme/chauffeur CHAUFFEUR_APP_BUNDLE="$T/Applications/Chauffeur.app" SHELL="$T/login-shell" c daemon start >/dev/null
  c --json update | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r["available"] is None, r'
  # Published after the daemon's own look: `chauffeur update` asks GitHub again and sees it at once.
  mv "$T/staged" "$T/release"
  c update | grep -q "Chauffeur 99.0.0 is available"
  c --json update --install | python3 -c 'import sys,json; r=json.load(sys.stdin); assert r["phase"] == "installed" and r["available"] == "99.0.0", r'
  test "$(cat "$T/Applications/Chauffeur.app/Contents/MacOS/chauffeurd")" = new
  test "$(ls -A "$T/Applications")" = Chauffeur.app
  test ! -e "$CHAUFFEUR_HOME/updates"
  c daemon stop >/dev/null
  SHELL="$T/login-shell" c daemon start >/dev/null
fi

echo "== ollama. local models are read from Ollama's folder, without starting it, and join Codex's catalog"
mkdir -p "$T/ollama-models/manifests/registry.ollama.ai/library/tiny"
echo '{"layers":[{"size":40},{"size":2}]}' > "$T/ollama-models/manifests/registry.ollama.ai/library/tiny/latest"
echo "export OLLAMA_MODELS=$T/ollama-models" >> "$T/profile"
c env reload >/dev/null
c --json ollama | python3 -c '
import sys,json; s=json.load(sys.stdin); print(s)
assert s["models"] == [{"name": "tiny", "size": 42}] and s["install"] and s["pulls"] == [], s'
c --json models | python3 -c '
import sys,json; agents={a["agent"]: a for a in json.load(sys.stdin)["agents"]}
local=[m for m in agents["codex"]["models"] if m["id"] == "ollama/tiny"]
assert local and local[0]["name"] == "tiny (local)", agents["codex"]'
c ollama | grep -q "tiny"
if c ollama pull "bad name" 2>/dev/null; then echo "a model name with a space was pulled"; exit 1; fi

# From inside a workspace or an agent's session, the daemon commands refuse to stop the default
# daemon (~/.chauffeur): the one the agent's own chats run on. This throwaway daemon passes for it
# under a HOME of its own; an explicit CHAUFFEUR_HOME of another folder is still stopped.
mkdir -p "$T/user" && ln -s "$T/home" "$T/user/.chauffeur"
echo "== conductor. the import plans without changing anything, copies workspaces, and runs once"
mkdir -p "$T/conductor"
scripts/conductor-fixture.sh "$T/conductor"
CDB="$T/conductor/conductor.db"
LIMA="$T/conductor/workspaces/alpha/lima"
OSLO="$T/conductor/workspaces/alpha/oslo"
conductor_files() { (cd "$T/conductor" && find . -type f -print0 | sort -z | xargs -0 shasum | shasum); }
before=$(conductor_files)
c import conductor --dry-run --db "$CDB" > "$T/conductor-plan"
grep -q "variables, to your settings: API_TOKEN" "$T/conductor-plan"
grep -q "scripts typed in Conductor, to your settings: setup, run, run_mode" "$T/conductor-plan"
grep -q "^  Pricing filters   fix-1-pricing, fixture/alpha, 4 uncommitted file(s), 2 chat(s), branch released in Conductor$" "$T/conductor-plan"
grep -q "^  Linked work   feat/linked, fixture/alpha + fixture/beta" "$T/conductor-plan"
grep -q "^  (1 archived in Conductor, left out: --include-archived brings their chat history)$" "$T/conductor-plan"
if grep -q "Old work" "$T/conductor-plan"; then echo "an archived workspace was planned without --include-archived"; exit 1; fi
grep -q "^  Cloudy   skipped: a Conductor Cloud workspace: transfer it to local in Conductor" "$T/conductor-plan"
grep -q "^These Conductor workspaces will have their branch released:$" "$T/conductor-plan"
grep -q "^  Linked work       feat/linked$" "$T/conductor-plan"
grep -q "^Dry run: nothing changed.$" "$T/conductor-plan"
c --json import conductor --dry-run --include-archived --db "$CDB" > "$T/conductor-plan.json"
python3 -c '
import sys, json
r = json.load(open(sys.argv[1]))
w = {w["name"]: w for w in r["workspaces"]}
assert r["dry_run"] and w["Old work"]["outcome"] == "import" and w["Old work"]["archived"], r
assert w["Pricing filters"]["changes"] == 4 and w["Pricing filters"]["release"], w
assert r["repos"][0]["env"] == ["API_TOKEN"] and r["repos"][0]["clone"] == "conductor", r' "$T/conductor-plan.json"
if grep -rq "s3cret-fixture-value" "$T/conductor-plan" "$T/conductor-plan.json" "$CHAUFFEUR_HOME/settings.toml"; then
  echo "a variable's value was shown, or written before the import"; exit 1
fi
[ "$(conductor_files)" = "$before" ] || { echo "the dry run changed Conductor's files"; exit 1; }
# Without --yes and nobody to answer, nothing is imported.
if c import conductor --db "$CDB" </dev/null 2>"$T/conductor-refused"; then echo "the import ran without asking"; exit 1; fi
grep -q "Pass --yes" "$T/conductor-refused"
[ "$(conductor_files)" = "$before" ] || { echo "a refused import changed Conductor's files"; exit 1; }
lima_files() { (cd "$LIMA" && find . -type f -print0 | sort -z | xargs -0 shasum | shasum); }
lima_before=$(lima_files)
LIMA_HEAD=$(git -C "$LIMA" rev-parse HEAD)
c --json import conductor --yes --db "$CDB" > "$T/conductor-done.json"
python3 -c '
import sys, json
r = json.load(open(sys.argv[1]))
w = {w["name"]: w for w in r["workspaces"]}
assert w["Pricing filters"]["outcome"] == "imported" and w["Pricing filters"]["workspace"] == "pricing-filters", w
assert w["Linked work"]["outcome"] == "imported" and w["Linked work"]["workspace"] == "linked-work", w
assert w["Cloudy"]["outcome"] == "skipped", w' "$T/conductor-done.json"
if grep -q "s3cret-fixture-value" "$T/conductor-done.json" "$CHAUFFEUR_HOME/logs/chauffeurd.log"; then echo "a variable's value was shown or logged"; exit 1; fi
# Copied on the same branch with the uncommitted and untracked changes; Conductor's copy detached
# on the same commit, its files untouched.
PF="$CHAUFFEUR_WORKSPACES_DIR/pricing-filters"
test "$(git -C "$PF/alpha" branch --show-current)" = fix-1-pricing
test "$(cat "$PF/alpha/README.md")" = "alpha, edited"
test "$(cat "$PF/alpha/staged.txt")" = staged
test "$(cat "$PF/alpha/notes/untracked.txt")" = untracked
test ! -e "$PF/alpha/old.txt"
if git -C "$LIMA" symbolic-ref -q HEAD; then echo "Conductor's lima still holds its branch"; exit 1; fi
test "$(git -C "$LIMA" rev-parse HEAD)" = "$LIMA_HEAD"
[ "$(lima_files)" = "$lima_before" ] || { echo "the import changed Conductor's worktree files"; exit 1; }
if git -C "$OSLO" symbolic-ref -q HEAD; then echo "Conductor's oslo still holds its branch"; exit 1; fi
# The /add-dir folder is a second repo, on the same branch.
c --json show linked-work | python3 -c 'import sys,json; w=json.load(sys.stdin); assert sorted(t["repo"] for t in w["worktrees"]) == ["alpha", "beta"] and w["branch"] == "feat/linked", w'
test "$(git -C "$CHAUFFEUR_WORKSPACES_DIR/linked-work/beta" branch --show-current)" = feat/linked
# The chats as Markdown (a cancelled message left out, an empty chat not written), the notes, and
# the agents told where they are.
grep -q "codename PAPAYA-42" "$PF/.chauffeur/conductor/Price filter.md"
grep -q "^- \*Bash\*: \`rg price\`$" "$PF/.chauffeur/conductor/Price filter.md"
if grep -q "never sent" "$PF/.chauffeur/conductor/Price filter.md"; then echo "a cancelled message was imported"; exit 1; fi
grep -q "^## Codex · 2026-10-03 09:01 UTC$" "$PF/.chauffeur/conductor/Review.md"
test ! -e "$PF/.chauffeur/conductor/Empty.md"
grep -q "Ship the price filter" "$PF/.chauffeur/conductor/context/plan.md"
grep -q "Its earlier chats are in \`.chauffeur/conductor/\`" "$PF/CLAUDE.md"
# The typed scripts and the variable in the user's settings, given to the repo's scripts.
S="$CHAUFFEUR_HOME/settings.toml"
grep -q "^\[repos.alpha.scripts\]$" "$S" && grep -q '^setup = "echo conductor-setup > .conductor-setup-ran"$' "$S"
grep -q '^run = "echo run"$' "$S" && grep -q '^run_mode = "nonconcurrent"$' "$S"
grep -q "^\[repos.alpha.env\]$" "$S" && grep -q '^API_TOKEN = "s3cret-fixture-value"$' "$S"
grep -q "^clone_roots = .*\"$T/conductor/clones\"" "$S"
for _ in $(seq 1 100); do [ -e "$PF/alpha/.conductor-setup-ran" ] && break; sleep 0.1; done
test "$(cat "$PF/alpha/.conductor-setup-ran")" = conductor-setup
# Again, archived ones included: only the archived one is new, as an archived workspace with its chat.
c --json import conductor --yes --include-archived --db "$CDB" | python3 -c '
import sys, json
r = json.load(sys.stdin)
w = {w["name"]: w["outcome"] for w in r["workspaces"]}
assert w == {"Pricing filters": "already_imported", "Linked work": "already_imported", "Old work": "imported", "Cloudy": "skipped"}, w'
c --json show old-work | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"] == "archived", w'
grep -q "An old question" "$CHAUFFEUR_HOME/archive/old-work/conductor/Old chat.md"
c import conductor --yes --include-archived --db "$CDB" | grep -q "^Old work: already imported, as old-work$"
# A Conductor whose schema this Chauffeur does not know: stopped before any change.
cp "$CDB" "$T/conductor-other.db"
sqlite3 "$T/conductor-other.db" "ALTER TABLE workspaces DROP COLUMN linked_directory_paths"
settings_before=$(shasum < "$CHAUFFEUR_HOME/settings.toml")
if c import conductor --yes --db "$T/conductor-other.db" 2>"$T/conductor-schema"; then echo "an unknown schema was imported"; exit 1; fi
grep -q "is not one this Chauffeur knows (no column workspaces.linked_directory_paths)" "$T/conductor-schema"
test "$(shasum < "$CHAUFFEUR_HOME/settings.toml")" = "$settings_before"
c archive pricing-filters --force >/dev/null
c archive linked-work --force >/dev/null
outside() { env -u CHAUFFEUR_HOME -u CHAUFFEUR_WORKSPACE_ID -u CHAUFFEUR_WORKSPACE_PATH -u CLAUDECODE -u CODEX_THREAD_ID -u AI_AGENT HOME="$T/user" "$@"; }
if outside CHAUFFEUR_WORKSPACE_ID=01e2e "$BIN/chauffeur" daemon stop 2> "$T/refused.err"; then
  echo "daemon stop from a workspace stopped the default daemon"; exit 1
fi
grep -q "refusing to stop the Chauffeur daemon of $T/user/.chauffeur" "$T/refused.err"
grep -q "your own chats run on" "$T/refused.err"
grep -q "CHAUFFEUR_HOME=<temp dir> chauffeur daemon stop" "$T/refused.err"
if grep -q -- "--" "$T/refused.err"; then echo "the refusal names a flag: $(cat "$T/refused.err")"; exit 1; fi
# An agent's session that cleared the workspace's variables, and the default folder named outright.
if outside CLAUDECODE=1 CHAUFFEUR_HOME="$T/user/.chauffeur" "$BIN/chauffeur" daemon stop 2> "$T/refused.err"; then
  echo "daemon stop from an agent's session stopped the default daemon"; exit 1
fi
grep -q "(CLAUDECODE is set)" "$T/refused.err"
# (install and uninstall are left to the unit tests: should they go ahead, launchctl would act on
# the user's real LaunchAgent, whatever HOME says.)
c daemon status >/dev/null
# A throwaway daemon is stopped from a workspace, named by its CHAUFFEUR_HOME.
CHAUFFEUR_WORKSPACE_ID=01e2e CLAUDECODE=1 c daemon stop | grep -q "chauffeurd stopped"
# Start, refused in a workspace, says so rather than that the daemon is not running.
if CHAUFFEUR_WORKSPACE_ID=01e2e c daemon start 2> "$T/refused.err"; then exit 1; fi
grep -q "refusing to start a daemon from inside a Chauffeur workspace" "$T/refused.err"
if grep -q "is not running" "$T/refused.err"; then cat "$T/refused.err"; exit 1; fi
# The user's override, which the refusal never names; and outside, the default daemon stops as before.
c daemon start >/dev/null
outside CHAUFFEUR_WORKSPACE_ID=01e2e "$BIN/chauffeur" daemon stop --real-daemon | grep -q "chauffeurd stopped"
if c daemon status >/dev/null; then echo "the override did not stop the daemon"; exit 1; fi
c daemon start >/dev/null
outside "$BIN/chauffeur" daemon stop | grep -q "chauffeurd stopped"
if c daemon status >/dev/null; then echo "daemon stop outside a workspace did not stop it"; exit 1; fi
echo "E2E CLI OK"
