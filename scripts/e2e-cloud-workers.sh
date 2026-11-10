#!/usr/bin/env bash
# End-to-end check of a GCP worker without GCP: a source daemon creates a worker through a fake
# `gcloud` that keeps instances as files, starts a throwaway "worker" daemon when asked to install
# Chauffeur, and a fake `ssh` that forwards its socket with a symlink. A workspace then goes to the worker by its
# name, opens there in a fake Cursor over SSH, is backed up there to a fake bucket, and comes
# back; the worker is deleted only once nothing runs there. Nothing leaves the temporary folder:
# the source daemon's HOME is a temporary one, and its ssh folder is named explicitly (a daemon
# whose state is not ~/.chauffeur never touches ~/.ssh otherwise); no real gcloud, gh, editor or
# GCP resource is used.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q -p chauffeurd -p chauffeur-cli
(cd sidecar && bun run build >/dev/null)
BIN="$PWD/target/debug"
# Short names: a Unix socket path must fit in about 100 bytes on macOS.
T=$(mktemp -d /tmp/cw.XXXXXX)
T=$(cd "$T" && pwd -P)
unset CHAUFFEUR_WORKSPACE_ID
export CHAUFFEUR_PORT_OVERRIDE=0 CHAUFFEURD="$BIN/chauffeurd" CLAUDE_CONFIG_DIR="$T/claude"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
# The daemon's children get the login shell's environment, not this one: the fake has its paths
# written in, and only the daemon itself reads CHAUFFEUR_GCLOUD and CHAUFFEUR_SSH.
export CHAUFFEUR_GCLOUD="$T/fake/gcloud"
# The forward of a worker with an SSH entry runs ssh itself, not gcloud: a fake one too.
export CHAUFFEUR_SSH="$T/fake/ssh"
# Editors are looked for here only: a fake Cursor, never the user's.
export CHAUFFEUR_APP_DIRS="$T/apps"
# The ssh folder whose config gets the workers' Include: a temporary one.
export CHAUFFEUR_SSH_DIR="$T/user/.ssh"
# This run's forwards are `sleep`s named after its folder, told apart from any other run's.
FORWARD="$T/forward"
# The daemons' login shell, which their agents' environment comes from: this one's, with a fake
# `codex` first on PATH (never the user's, nor their login). The worker daemon inherits it too.
export SHELL="$T/fake/shell"

on() {
  local name=$1
  shift
  # The source writes the worker's SSH entry in ~/.ssh: a temporary HOME, never the user's.
  HOME="$T/user" CHAUFFEUR_HOME="$T/$name/home" CHAUFFEUR_WORKSPACES_DIR="$T/$name/workspaces" \
    CHAUFFEUR_REPOS_DIR="$T/$name/repos" "$BIN/chauffeur" "$@"
}
# The worker daemon's home is where the fake installation says: `$T/wrk/.chauffeur`.
on_worker() {
  CHAUFFEUR_HOME="$T/wrk/.chauffeur" CHAUFFEUR_WORKSPACES_DIR="$T/wrk/workspaces" \
    CHAUFFEUR_REPOS_DIR="$T/wrk/repos" "$BIN/chauffeur" "$@"
}
cleanup() {
  for name in src ctl; do on "$name" daemon stop >/dev/null 2>&1 || true; done
  on_worker daemon stop >/dev/null 2>&1 || true
  pkill -f "$FORWARD" 2>/dev/null || true
  chmod u+w "$T/src/workspaces" 2>/dev/null || true
  [ -n "${KEEP:-}" ] && { echo "kept: $T"; return; }
  rm -rf "$T"
}
trap cleanup EXIT
# `! command` never stops a script under `set -e`.
refute() { if "$@"; then echo "unexpected: $*" >&2; exit 1; fi; }

mkdir -p "$T/fake/instances" "$T/fake/bin" "$T/wrk/.chauffeur" "$T/wrk/home" "$T/user"
printf '#!/bin/bash\nPATH="%s/fake/bin:$PATH" exec /bin/bash -c "${@: -1}"\n' "$T" > "$T/fake/shell"
chmod +x "$T/fake/shell"
# A Codex app-server signed out until its device code login is approved on OpenAI's page (the
# file `codex-approve`); it then keeps the login (`codex-signed-in`) and says so, as Codex does.
cat > "$T/fake/bin/codex" <<'CODEX'
#!/usr/bin/env python3
import json, os, sys, threading, time
F = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if "app-server" not in sys.argv:
    print("codex-cli 0.0.0-e2e")
    sys.exit(0)
lock = threading.Lock()
def send(message):
    with lock:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()
def approve(login):
    while not os.path.exists(F + "/codex-approve"):
        time.sleep(0.1)
    time.sleep(0.3)
    open(F + "/codex-signed-in", "w").close()
    send({"method": "account/login/completed", "params": {"loginId": login, "success": True, "error": None}})
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method, params = request["method"], request.get("params") or {}
    signed_in = os.path.exists(F + "/codex-signed-in")
    if method == "initialize":
        result = {"userAgent": "codex-e2e"}
    elif method == "account/read":
        account = {"type": "chatgpt", "email": "e2e@example.com", "planType": "plus"} if signed_in else None
        result = {"account": account, "requiresOpenaiAuth": True}
    elif method == "account/rateLimits/read":
        result = {"rateLimits": {"limitId": "codex", "primary": None, "secondary": None, "planType": "plus"}, "rateLimitsByLimitId": None, "rateLimitResetCredits": None}
    elif method == "account/login/start" and params.get("type") == "chatgptDeviceCode":
        result = {"type": "chatgptDeviceCode", "loginId": "device-e2e", "verificationUrl": "https://auth.openai.com/codex/device", "userCode": "E2E-CODE"}
        threading.Thread(target=approve, args=("device-e2e",), daemon=True).start()
    elif method == "account/login/cancel":
        result = {"status": "canceled"}
    elif method == "model/list":
        result = {"data": [], "nextCursor": None}
    else:
        send({"id": request["id"], "error": {"code": -32601, "message": method + " is not faked"}})
        continue
    send({"id": request["id"], "result": result})
CODEX
chmod +x "$T/fake/bin/codex"
# A gh signed in when its HOME holds `gh-token`, never the user's: the source's HOME is signed in,
# the worker's gets the token through Chauffeur, as `gh auth login --with-token` would keep it.
cat > "$T/fake/bin/gh" <<'GH'
#!/bin/bash
case "$*" in
  "auth status --active --json hosts")
    if [ -f "$HOME/gh-token" ]; then echo '{"hosts":{"github.com":[{"state":"success","active":true}]}}'; else echo '{"hosts":{}}'; fi ;;
  "auth status") [ -f "$HOME/gh-token" ] ;;
  "auth token --hostname github.com") cat "$HOME/gh-token" ;;
  "auth login --hostname github.com --git-protocol https --with-token") cat > "$HOME/gh-token" ;;
  "auth setup-git --hostname github.com") touch "$HOME/gh-setup-git" ;;
  *) exit 1 ;;
esac
GH
chmod +x "$T/fake/bin/gh"
echo "gho_e2e" > "$T/user/gh-token"
# This Mac's AWS SSO sign-in, which each send to a worker copies there.
mkdir -p "$T/user/.aws/sso/cache"
printf '[profile e2e]\nsso_session = e2e\n' > "$T/user/.aws/config"
echo '{"accessToken":"aws-e2e"}' > "$T/user/.aws/sso/cache/e2e.json"
echo '[default]' > "$T/user/.aws/credentials"
# This Mac's Claude Code configuration (its CLAUDE_CONFIG_DIR is $T/claude), which each send to a
# worker copies there once per change; never its login nor its transcripts.
mkdir -p "$T/claude/skills/review" "$T/claude/plugins/cache/p" "$T/claude/projects/x"
echo "# e2e rules" > "$T/claude/CLAUDE.md"
echo "review skill" > "$T/claude/skills/review/SKILL.md"
printf '{"env":{"E2E_TOOL":"%s/tool"}}\n' "$T/user" > "$T/claude/settings.json"
printf '{"plugins":{"p":[{"installPath":"%s/plugins/cache/p"}]}}\n' "$T/claude" > "$T/claude/plugins/installed_plugins.json"
echo '{}' > "$T/claude/plugins/cache/p/plugin.json"
echo "a transcript" > "$T/claude/projects/x/chat.jsonl"
echo "secret" > "$T/claude/.credentials.json"
printf '{"oauthAccount":{"email":"mac"},"mcpServers":{"remote":{"type":"http","url":"https://mcp.example.com/mcp"},"local":{"type":"http","url":"http://127.0.0.1:8787/mcp"},"shell":{"command":"sh"}}}\n' > "$T/claude/.claude.json"
# A fake Cursor: its command line, inside its bundle on macOS and on PATH on Linux, notes how it
# was asked to open.
for cli in "$T/apps/Cursor.app/Contents/Resources/app/bin/cursor" "$T/apps/cursor"; do
  mkdir -p "$(dirname "$cli")"
  printf '#!/bin/bash\necho "$*" >> "%s/opened"\n' "$T" > "$cli"
  chmod +x "$cli"
done
echo "not a real bundle" > "$T/fake/bundle.tar.gz"
cat > "$T/fake/gcloud" <<FAKE
#!/bin/bash
F="$T/fake"
echo "\$*" >> "\$F/calls"
last="\${@: -1}"
case "\$1 \$2 \$3" in
  "auth list "*) [ -e "\$F/logged-out" ] || echo e2e@example.com ;;
  "compute instances describe")
    [ -f "\$F/instances/\$4" ] && cat "\$F/instances/\$4" && exit 0
    echo "ERROR: (gcloud.compute.instances.describe) The resource '\$4' was not found" >&2
    exit 1 ;;
  "compute instances create")
    label=\$(echo "\$*" | sed -n 's/.*--labels chauffeur-worker=\([^ ]*\).*/\1/p')
    echo "{\"id\":\"42\",\"labels\":{\"chauffeur-worker\":\"\$label\"},\"disks\":[{\"boot\":true,\"source\":\"https://www.googleapis.com/compute/v1/projects/e2e-project/zones/europe-west9-c/disks/boot\"}]}" > "\$F/instances/\$4" ;;
  "compute instances delete") rm "\$F/instances/\$4" ;;
  # Stopped, its disk kept: the worker's daemon here goes on, as a disk would keep it.
  "compute instances stop") sed -i.bak 's/^{/{"status":"TERMINATED",/' "\$F/instances/\$4" ;;
  "compute instances start") sed -i.bak 's/"status":"TERMINATED",//' "\$F/instances/\$4" ;;
  "compute disks describe")
    n=0
    [ -f "\$F/disk-check-count" ] && n=\$(cat "\$F/disk-check-count")
    n=\$((n + 1)); echo "\$n" > "\$F/disk-check-count"
    if [ -f "\$F/fail-disk-check-at" ] && [ "\$n" = "\$(cat "\$F/fail-disk-check-at")" ]; then
      echo "ERROR: PERMISSION_DENIED reading the kept disk" >&2; exit 1
    fi
    users='[]'
    [ -e "\$F/disk-attached" ] && users='["other-instance"]'
    echo "{\"selfLink\":\"https://www.googleapis.com/compute/v1/projects/e2e-project/zones/europe-west9-c/disks/boot\",\"users\":\$users}" ;;
  "storage buckets describe")
    [ -f "\$F/buckets/\${4#gs://}" ] && cat "\$F/buckets/\${4#gs://}" && exit 0
    echo "ERROR: (gcloud.storage.buckets.describe) \$4 not found: 404." >&2
    exit 1 ;;
  "storage buckets create")
    mkdir -p "\$F/buckets"
    echo '{"location":"EUROPE-WEST9","iamConfiguration":{"uniformBucketLevelAccess":{"enabled":true}},"projectNumber":"123"}' > "\$F/buckets/\${4#gs://}" ;;
  "storage buckets update"|"storage managed-folders add-iam-policy-binding"|"storage managed-folders remove-iam-policy-binding") ;;
  "storage managed-folders create") mkdir -p "\$F/folders" && touch "\$F/folders/\$(echo "\$4" | tr / _)" ;;
  "storage managed-folders delete") rm "\$F/folders/\$(echo "\$4" | tr / _)" ;;
  "projects describe "*) echo 123 ;;
  "iam service-accounts describe")
    [ -f "\$F/accounts/\$4" ] && cat "\$F/accounts/\$4" && exit 0
    echo "ERROR: (gcloud.iam.service-accounts.describe) NOT_FOUND: Unknown service account" >&2
    exit 1 ;;
  "iam service-accounts create")
    mkdir -p "\$F/accounts"
    echo '{"displayName":"Chauffeur worker"}' > "\$F/accounts/\$4@e2e-project.iam.gserviceaccount.com" ;;
  "iam service-accounts delete") rm "\$F/accounts/\$4" ;;
  # Objects are files under \$F/bucket; nothing answers while \$F/storage-down exists.
  "storage ls "*)
    python3 -c 'import glob,sys; print("\\n".join("gs://"+p[len(sys.argv[1])+1:] for p in glob.glob(sys.argv[1]+"/"+sys.argv[2][5:])))' "\$F/bucket" "\$3" ;;
  "storage cp --quiet")
    case "\$4" in gs://*) source="\${4#gs://}"; cp "\$F/bucket/\${source%%#*}" "\$5"; exit ;; esac
    [ -e "\$F/storage-down" ] && { echo "ERROR: (gcloud.storage.cp) storage is down" >&2; exit 1; }
    mkdir -p "\$(dirname "\$F/bucket/\${5#gs://}")" && cp "\$4" "\$F/bucket/\${5#gs://}" ;;
  "storage objects describe")
    [ -f "\$F/bucket/\${4#gs://}" ] || exit 1
    echo "{\"generation\":\"42\",\"size\": \"\$(wc -c < "\$F/bucket/\${4#gs://}" | tr -d ' ')\"}" ;;
  "storage rm --quiet") rm "\$F/bucket/\${4#gs://}" ;;
  "compute scp "*) cp "\$3" "\$F/received.tar.gz" ;;
  "compute ssh "*)
    case "\$*" in
      *"--command true"*) ;;
      *"--dry-run"*) echo "/usr/bin/ssh -t -i $T/user/.ssh/google_compute_engine -o HostKeyAlias=compute.42 -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes e2e@127.0.0.1" ;;
      *"--command bash -s"*)
        cat > "\$F/install.sh"
        printf 'github_org = ""\ngithub_sources = false\nport_range_start = 47900\nlinear_enabled = false\n[office]\nurl = ""\n' > "$T/wrk/.chauffeur/settings.toml"
        HOME="$T/wrk/home" CHAUFFEUR_HOME="$T/wrk/.chauffeur" CHAUFFEUR_WORKSPACES_DIR="$T/wrk/workspaces" \\
          CHAUFFEUR_REPOS_DIR="$T/wrk/repos" CHAUFFEUR_PORT_OVERRIDE=0 CHAUFFEURD="$BIN/chauffeurd" \\
          CHAUFFEUR_GCLOUD="$T/fake/gcloud" \\
          CLAUDE_CONFIG_DIR="$T/wrk/claude" "$BIN/chauffeur" daemon start >/dev/null
        echo "home=$T/wrk" ;;
      *" -N "*)
        ln -sf "\${last#*:}" "\${last%%:*}"
        exec -a "$FORWARD" sleep 999999 ;;
      *) exit 1 ;;
    esac ;;
  *) exit 1 ;;
esac
FAKE
chmod +x "$T/fake/gcloud"
cat > "$T/fake/ssh" <<FAKE
#!/bin/bash
echo "\$*" >> "$T/fake/ssh-calls"
while [ \$# -gt 0 ]; do
  [ "\$1" = -L ] && binding="\$2"
  shift
done
[ -n "\$binding" ] || exit 1
ln -sf "\${binding#*:}" "\${binding%%:*}"
exec -a "$FORWARD" sleep 999999
FAKE
chmod +x "$T/fake/ssh"
# mise and corepack, which the worker runs in each repo before its setup script: they note what
# they were asked, where, in the order the setup script notes itself; mise fails while
# $T/fake/mise-fail exists.
cat > "$T/fake/bin/mise" <<MISE
#!/bin/bash
[ -e "$T/fake/mise-fail" ] && { echo "mise ERROR could not download node 24" >&2; exit 1; }
echo "mise \$* \$PWD" >> "$T/fake/order.log"
MISE
cat > "$T/fake/bin/corepack" <<COREPACK
#!/bin/bash
echo "corepack \$* \$PWD" >> "$T/fake/order.log"
COREPACK
chmod +x "$T/fake/bin/mise" "$T/fake/bin/corepack"

mkdir -p "$T/seed/alpha" && git -C "$T/seed/alpha" init -q -b main
echo alpha > "$T/seed/alpha/README.md"
# What alpha declares, and a setup script that notes where it ran.
echo v24 > "$T/seed/alpha/.nvmrc"
echo '{"name": "alpha", "packageManager": "pnpm@10.24.0+sha512.e2e"}' > "$T/seed/alpha/package.json"
mkdir -p "$T/seed/alpha/.chauffeur"
printf '[scripts]\nsetup = "echo setup $PWD >> %s/fake/order.log"\n' "$T" > "$T/seed/alpha/.chauffeur/settings.toml"
git -C "$T/seed/alpha" add -A && git -C "$T/seed/alpha" commit -q -m init
git clone -q --bare "$T/seed/alpha" "$T/origin/alpha.git"
git clone -q "$T/origin/alpha.git" "$T/clones/alpha"
port=47600
for name in src ctl; do
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
done
if [ -z "${AUTOMATIC_CONTROLLER:-}" ]; then
  printf '\n[transfer]\ncontroller = "%s/ctl/home/chauffeurd.sock"\n' "$T" >> "$T/src/home/settings.toml"
fi
cat >> "$T/src/home/settings.toml" <<TOML
[workers]
project = "e2e-project"
zone = "europe-west9-c"
bundle = "$T/fake/bundle.tar.gz"
backup_bucket = "e2e-backups"
TOML
on src daemon start >/dev/null
if [ -z "${AUTOMATIC_CONTROLLER:-}" ]; then on ctl daemon start >/dev/null; fi

echo "== list. what a worker costs before any exists"
on src worker list | tee "$T/list.out"
grep -q "^No worker.$" "$T/list.out"
grep -q "e2-standard-4 \$0.17/h" "$T/list.out"

echo "== create. nothing is created without an answer"
if on src worker create w1 </dev/null 2>"$T/refused"; then echo "a worker was created without confirmation"; exit 1; fi
grep -q "about \$0.17 an hour" "$T/refused"
grep -q "Pass --yes" "$T/refused"
refute grep -qs "instances create" "$T/fake/calls"

echo "== create. the VM, labelled, in the configured project; Chauffeur installed; the socket forwarded"
on src worker create w1 --yes 2>&1 | tee "$T/create.out"
grep -q "^w1 is ready" "$T/create.out"
# Its SSH entry for the editors, in Chauffeur's own file, included once from ~/.ssh/config.
grep -qF "Added \`Include \"$T/src/home/ssh_config\"\` at the top of $T/user/.ssh/config" "$T/create.out"
test "$(cat "$T/user/.ssh/config")" = "Include \"$T/src/home/ssh_config\""
# Its alias is this daemon's own: `chauffeur-<6 hex of its state folder>-w1`.
HOST=$(on src --json worker list | python3 -c 'import sys,json; print(json.load(sys.stdin)["workers"][0]["ssh_host"])')
[[ "$HOST" =~ ^chauffeur-[0-9a-f]{6}-w1$ ]] || { echo "unexpected ssh host $HOST"; exit 1; }
grep -q "^Host $HOST$" "$T/src/home/ssh_config"
grep -q "^  StrictHostKeyChecking yes$" "$T/src/home/ssh_config"
grep -q "^  HostName 127.0.0.1$" "$T/src/home/ssh_config"
grep -q "^  IdentityFile $T/user/.ssh/google_compute_engine$" "$T/src/home/ssh_config"
# Its forward runs ssh with that entry, never gcloud: an expired gcloud session cannot cut it.
grep -q -- "-o IdentityFile=$T/user/.ssh/google_compute_engine .*-o BatchMode=yes .*-N .*-L .* e2e@127.0.0.1$" "$T/fake/ssh-calls"
refute grep -q "compute ssh .* -N " "$T/fake/calls"
grep -q "compute instances create chauffeur-worker-w1 .*--labels chauffeur-worker=w1 .*--service-account cw-backup-w1@e2e-project.iam.gserviceaccount.com --scopes https://www.googleapis.com/auth/devstorage.read_write .*--project e2e-project --zone europe-west9-c --quiet" "$T/fake/calls"
# Before the VM: the backup bucket in Paris keeping deleted objects a week, the worker's own
# account, and its access to its own managed folder only.
grep -q "storage buckets create gs://e2e-backups --project e2e-project --location europe-west9 .*--soft-delete-duration 7d" "$T/fake/calls"
grep -q "storage buckets update gs://e2e-backups --update-labels chauffeur-backups=true" "$T/fake/calls"
grep -q "iam service-accounts create cw-backup-w1 --project e2e-project" "$T/fake/calls"
grep -q 'storage managed-folders create gs://e2e-backups/w1-[0-9a-f]\{8\}/$' "$T/fake/calls"
grep -q 'storage managed-folders add-iam-policy-binding gs://e2e-backups/w1-[0-9a-f]\{8\}/ --member serviceAccount:cw-backup-w1@e2e-project.iam.gserviceaccount.com --role roles/storage.objectUser$' "$T/fake/calls"
cmp -s "$T/fake/received.tar.gz" "$T/fake/bundle.tar.gz"
grep -q "chauffeur daemon install" "$T/fake/install.sh"
on src --json worker list | python3 -c '
import sys, json
w = json.load(sys.stdin)["workers"][0]
assert (w["name"], w["state"], w["connected"]) == ("w1", "ready", True), w'

echo "== login. a worker with no agent signed in takes nothing; one agent signed in from here, the transfer resumes"
on src new --name talky >/dev/null
on src chat talky --agent codex >/dev/null
if on src cloud transfer talky --to w1 2>"$T/refused"; then echo "a workspace moved to a worker with no agent signed in"; exit 1; fi
grep -q "w1 cannot take talky yet; nothing was stopped" "$T/refused"
grep -q "no agent is signed in on w1: sign Claude Code or Codex in from Settings → Transfers" "$T/refused"
refute grep -q "codex login" "$T/refused"
on src --json cloud transfer talky --status | python3 -c '
import sys, json
t = json.load(sys.stdin)["transfer"]
assert t["phase"] == "failed" and t["needs_login"] == {"worker": "w1", "agents": ["claude", "codex"]}, t
# Before its check, the worker got the Claude Code configuration, but an MCP server on this
# machine'"'"'s own address.
assert any("MCP server local was not copied to" in line for line in t["log"]), t["log"]'
# Claude Code: its manual page, whose code comes back here; given none, the login is cancelled
# there, and no code ever reaches Anthropic.
if on src worker login w1 --agent claude </dev/null >"$T/claude-login.out" 2>&1; then echo "a login without its code completed"; exit 1; fi
grep -q "^Sign Claude Code in on w1: open$" "$T/claude-login.out"
grep -q "redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback" "$T/claude-login.out"
grep -q "No code given: the login of Claude Code on w1 is cancelled" "$T/claude-login.out"
# Codex: its device code, typed on OpenAI's page (approved here by the fake). The worker's mise
# fails meanwhile: the resumed transfer says so, and alpha's setup still runs.
touch "$T/fake/mise-fail"
: > "$T/fake/order.log"
touch "$T/fake/codex-approve"
on src worker login w1 --agent codex </dev/null | tee "$T/codex-login.out"
grep -q "^Sign Codex in on w1: open$" "$T/codex-login.out"
grep -q "^https://auth.openai.com/codex/device$" "$T/codex-login.out"
grep -q "^and enter the code E2E-CODE$" "$T/codex-login.out"
grep -q "^Codex is signed in on w1.$" "$T/codex-login.out"
# The refused transfer resumes by itself: one agent is enough.
for _ in $(seq 1 120); do
  on src --json show talky | python3 -c 'import sys,json; sys.exit(0 if json.load(sys.stdin).get("moved_to")=="w1" else 1)' && break
  sleep 0.25
done
on src --json show talky | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w.get("moved_to")=="w1", w'
on_worker --json show talky | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="ready", w'
on src --json cloud transfer talky --status | python3 -c '
import sys, json
log = json.load(sys.stdin)["transfer"]["log"]
assert "installing node 24, pnpm 10.24.0 for alpha" in log, log
assert any(l.startswith("  could not install what alpha declares (mise ERROR could not download node 24)") for l in log), log
assert "  setup alpha succeeded" in log, log'
grep -q "^setup .*/wrk/workspaces/talky/alpha$" "$T/fake/order.log"
rm "$T/fake/mise-fail"
on src cloud transfer talky --back >/dev/null
on src --json show talky | python3 -c 'import sys,json; w=json.load(sys.stdin); assert not w.get("moved_to"), w'

echo "== transfer. a workspace goes to the worker by its name"
on src new --name moving >/dev/null
SOURCE_PATH=$(on src --json show moving | python3 -c 'import sys,json; print(json.load(sys.stdin)["path"])')
echo "edited before the move" > "$SOURCE_PATH/alpha/README.md"
: > "$T/fake/order.log"
on src cloud transfer moving --to w1 2>&1 | tee "$T/transfer.out"
grep -q "backing it up every five minutes to gs://e2e-backups/w1-[0-9a-f]\{8\}/" "$T/transfer.out"
# Codex alone is signed in there: Claude Code's chats wait for theirs, without stopping the move.
grep -q "Claude Code is not signed in on w1: its chats wait there for a sign-in" "$T/transfer.out"
on src --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w.get("moved_to")=="w1", w'
on_worker --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="ready", w'
test "$(cat "$T/wrk/workspaces/moving/alpha/README.md")" = "edited before the move"

echo "== toolchains. the worker installs what alpha declares in alpha, before its setup script"
grep -q "installing node 24, pnpm 10.24.0 for alpha" "$T/transfer.out"
sed "s#  *[^ ]*/wrk/workspaces/moving/alpha\$# alpha#" "$T/fake/order.log" > "$T/order"
printf 'mise install alpha\ncorepack enable alpha\ncorepack install alpha\nmise reshim alpha\nsetup alpha\n' | diff - "$T/order"

echo "== access. the worker got this Mac's GitHub and AWS SSO sign-in, owner-only, not its static keys"
test "$(cat "$T/wrk/home/gh-token")" = "gho_e2e"
test -f "$T/wrk/home/gh-setup-git"
refute grep -q "gh is not logged in" "$T/transfer.out"
refute grep -q "gho_e2e\|aws-e2e" "$T/transfer.out" "$T/src/home/logs/chauffeurd.log" "$T/wrk/.chauffeur/logs/chauffeurd.log"
test "$(git config --file "$T/wrk/home/.gitconfig" --get-all url.https://github.com/.insteadOf | tr '\n' ' ')" = "git@github.com: ssh://git@github.com/ "
cmp -s "$T/user/.aws/config" "$T/wrk/home/.aws/config"
cmp -s "$T/user/.aws/sso/cache/e2e.json" "$T/wrk/home/.aws/sso/cache/e2e.json"
test ! -e "$T/wrk/home/.aws/credentials"
python3 - "$T/wrk/home/.aws" <<'MODES'
import os, stat, sys
aws = sys.argv[1]
for path, want in [(aws, 0o700), (aws + "/sso/cache", 0o700), (aws + "/config", 0o600), (aws + "/sso/cache/e2e.json", 0o600)]:
    got = stat.S_IMODE(os.stat(path).st_mode)
    assert got == want, (path, oct(got))
MODES

echo "== claude. the worker got this Mac's Claude Code configuration, its paths made the worker's, once"
cmp -s "$T/claude/CLAUDE.md" "$T/wrk/claude/CLAUDE.md"
cmp -s "$T/claude/skills/review/SKILL.md" "$T/wrk/claude/skills/review/SKILL.md"
test -f "$T/wrk/claude/plugins/cache/p/plugin.json"
grep -q "\"$T/wrk/claude/plugins/cache/p\"" "$T/wrk/claude/plugins/installed_plugins.json"
grep -q "\"$T/wrk/home/tool\"" "$T/wrk/claude/settings.json"
test ! -e "$T/wrk/claude/.credentials.json"
test ! -e "$T/wrk/claude/projects/x"
python3 - "$T/wrk/claude" <<'CLAUDE'
import json, os, stat, sys
claude = sys.argv[1]
for path, want in [(claude, 0o700), (claude + "/settings.json", 0o600), (claude + "/.claude.json", 0o600)]:
    got = stat.S_IMODE(os.stat(path).st_mode)
    assert got == want, (path, oct(got))
state = json.load(open(claude + "/.claude.json"))
assert "oauthAccount" not in state, state
assert sorted(state["mcpServers"]) == ["remote", "shell"], state
CLAUDE
# Sent with talky's first attempt; unchanged since, not sent again with the next sends.
test "$(grep -c "gave a worker the Claude Code configuration" "$T/src/home/logs/chauffeurd.log")" = 1
grep -q "a worker has this Claude Code configuration already" "$T/src/home/logs/chauffeurd.log"

if [ -n "${AUTOMATIC_CONTROLLER:-}" ]; then
  echo "== controller. no controller setting or separate service was needed"
  test ! -e "$T/ctl/home/chauffeurd.sock"
  test -S "$T/src/home/transfer-controller/chauffeurd.sock"
  CONTROLLER_PID=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$T/src/home/transfer-controller/daemon.json")
  python3 - "$T" <<'CHECK_CONTROLLER'
import json,sys
root=sys.argv[1]
s=json.load(open(root+"/src/home/daemon.json")); c=json.load(open(root+"/src/home/transfer-controller/daemon.json"))
assert s["pid"] != c["pid"] and s["version"] == c["version"], (s,c)
CHECK_CONTROLLER
else
  test ! -e "$T/src/home/transfer-controller"
fi

echo "== relay. the moved workspace's calls and ports reach the worker through its forwarded socket"
echo "written there" > "$T/wrk/workspaces/moving/alpha/there.md"
on src --json files moving | grep -q "there.md"
rm "$T/wrk/workspaces/moving/alpha/there.md"
port_base() { "$@" --json show moving | python3 -c 'import sys,json; print(json.load(sys.stdin)["port_base"])'; }
SRC_BASE=$(port_base on src)
WRK_BASE=$(port_base on_worker)
mkdir -p "$T/web" && echo "served on the worker" > "$T/web/index.txt"
python3 -m http.server "$WRK_BASE" --bind 127.0.0.1 --directory "$T/web" >/dev/null 2>&1 &
WEB=$!
for _ in $(seq 1 60); do curl -sf "http://127.0.0.1:$SRC_BASE/index.txt" >/dev/null && break; sleep 0.25; done
test "$(curl -sf "http://127.0.0.1:$SRC_BASE/index.txt")" = "served on the worker"
kill "$WEB"; wait "$WEB" 2>/dev/null || true

echo "== open. the moved workspace opens on the worker in Cursor over SSH; other apps are refused"
opened() {
  for _ in $(seq 1 40); do [ -s "$T/opened" ] && break; sleep 0.1; done
  tail -1 "$T/opened"
}
on src open moving --app cursor >/dev/null
test "$(opened)" = "--remote ssh-remote+$HOST $T/wrk/workspaces/moving"
: > "$T/opened"
on src open moving --repo alpha >/dev/null
test "$(opened)" = "--remote ssh-remote+$HOST $T/wrk/workspaces/moving/alpha"
if on src open moving --app finder 2>"$T/refused"; then echo "Finder opened a workspace on a worker"; exit 1; fi
grep -q "Only Cursor and VS Code open moving on w1" "$T/refused"
# The user takes the Include out: the worker says why it no longer opens, with the line to add back,
# and opening is refused; once the line is back, it opens again.
issue() { on src --json worker list | python3 -c 'import sys,json; print(json.load(sys.stdin)["workers"][0]["ssh_issue"])'; }
test "$(issue)" = "None"
cp "$T/user/.ssh/config" "$T/ssh-config.keep"
printf 'Host mine\n' > "$T/user/.ssh/config"
issue | grep -qF "Include \"$T/src/home/ssh_config\""
if on src open moving --app cursor 2>"$T/refused"; then echo "a worker opened without its Include"; exit 1; fi
grep -qF "Include \"$T/src/home/ssh_config\"" "$T/refused"
cp "$T/ssh-config.keep" "$T/user/.ssh/config"
test "$(issue)" = "None"

echo "== backups. the worker backs the moved workspace up, encrypted; the Mac shows where they stand"
WORKSPACE_ID=$(on src --json show moving | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
count() { find "$BACKUPS" -name '*.bin' 2>/dev/null | wc -l | tr -d ' '; }
on src cloud backup moving | tee "$T/backup.out"
# Under the worker's own prefix, which its creation drew.
BACKUPS=$(echo "$T"/fake/bucket/e2e-backups/w1-*)/$WORKSPACE_ID
grep -q "current" "$T/backup.out"
test "$(count)" = 1
refute grep -rqs "edited before the move" "$BACKUPS"
# Nothing changed: nothing is uploaded.
on src cloud backup moving >/dev/null
test "$(count)" = 1
for n in 1 2 3; do
  echo "change $n" > "$T/wrk/workspaces/moving/alpha/README.md"
  on src cloud backup moving >/dev/null
done
test "$(count)" = 3
health() { on src --json show moving | python3 -c 'import sys,json; print(json.load(sys.stdin)["backup"]["health"])'; }
for _ in $(seq 1 50); do [ "$(health)" = current ] && break; sleep 0.2; done
test "$(health)" = current
echo "change 4" > "$T/wrk/workspaces/moving/alpha/README.md"
touch "$T/fake/storage-down"
on src cloud backup moving | tee "$T/failed.out"
grep -q "the last attempt failed: .*storage is down" "$T/failed.out"
test "$(count)" = 3
for _ in $(seq 1 50); do [ "$(health)" = failed ] && break; sleep 0.2; done
test "$(health)" = failed
rm "$T/fake/storage-down"
on src cloud backup moving >/dev/null
test "$(count)" = 3
on src cloud backup moving --status | grep -q "3 kept"

echo "== delete. refused while a workspace runs there"
if on src worker delete w1 --yes 2>"$T/refused"; then echo "a worker running a workspace was deleted"; exit 1; fi
grep -q "moving runs on w1: bring it back before deleting w1" "$T/refused"
test -f "$T/fake/instances/chauffeur-worker-w1"

echo "== restart. the forward comes back with the daemon"
on src daemon stop >/dev/null
refute pgrep -f "$FORWARD"
if [ -n "${AUTOMATIC_CONTROLLER:-}" ]; then refute kill -0 "$CONTROLLER_PID" 2>/dev/null; fi
on src daemon start >/dev/null
if [ -n "${AUTOMATIC_CONTROLLER:-}" ]; then
  for _ in $(seq 1 60); do [ -S "$T/src/home/transfer-controller/chauffeurd.sock" ] && break; sleep 0.2; done
  test -S "$T/src/home/transfer-controller/chauffeurd.sock"
  echo "== controller crash. its journal survives and Chauffeur restarts it automatically"
  CONTROLLER_PID=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$T/src/home/transfer-controller/daemon.json")
  kill -KILL "$CONTROLLER_PID"
  for _ in $(seq 1 80); do
    NEXT_PID=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$T/src/home/transfer-controller/daemon.json")
    if [ "$NEXT_PID" != "$CONTROLLER_PID" ] && kill -0 "$NEXT_PID" 2>/dev/null; then break; fi
    sleep 0.2
  done
  test "$NEXT_PID" != "$CONTROLLER_PID"
  kill -0 "$NEXT_PID"
fi
for _ in $(seq 1 50); do
  on src --json worker list | python3 -c 'import sys,json; sys.exit(0 if json.load(sys.stdin)["workers"][0]["connected"] else 1)' && break
  sleep 0.2
done
on src --json worker list | python3 -c 'import sys,json; assert json.load(sys.stdin)["workers"][0]["connected"]'

if [ -n "${RECOVERY:-}" ]; then
  if [ -n "${RECOVERY_UI:-}" ]; then
    echo "ui-ready: $T"
    while [ ! -f "$T/ui-continue" ]; do sleep 0.25; done
  fi
  echo "== recovery. a stored backup is authenticated before the VM changes; a disk still attached blocks activation"
  BACKUP=$(find "$BACKUPS" -name '*.bin' | sort | tail -1)
  BACKUP_ID=$(basename "$BACKUP" .bin)
  cp "$BACKUP" "$T/backup.keep"
  echo "newer than the backup" > "$T/wrk/workspaces/moving/alpha/README.md"
  on_worker daemon stop >/dev/null
  echo corrupted > "$BACKUP"
  if on src cloud recover moving --backup "$BACKUP_ID" --yes 2>"$T/refused"; then echo "a corrupted backup recovered"; exit 1; fi
  test -f "$T/fake/instances/chauffeur-worker-w1"
  cp "$T/backup.keep" "$BACKUP"
  touch "$T/fake/disk-attached"
  if on src cloud recover moving --backup "$BACKUP_ID" --yes 2>"$T/refused"; then echo "an attached disk let agents resume"; exit 1; fi
  grep -q "attached" "$T/refused" || { cat "$T/refused"; exit 1; }
  test ! -f "$T/fake/instances/chauffeur-worker-w1"
  on src --json show moving | python3 -c 'import sys,json; assert json.load(sys.stdin)["moved_to"] == "w1"'
  on src daemon stop >/dev/null
  on src daemon start >/dev/null
  rm "$T/fake/disk-attached"
  # The next retry restores the files, then the API refuses the final pre-commit disk check.
  echo 3 > "$T/fake/fail-disk-check-at"
  if on src cloud recover moving --backup "$BACKUP_ID" --yes 2>"$T/refused"; then echo "an unproved final fence activated"; exit 1; fi
  grep -q "PERMISSION_DENIED" "$T/refused" || { cat "$T/refused"; exit 1; }
  on src --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["moved_to"] == "w1", w'
  on src daemon stop >/dev/null
  on src daemon start >/dev/null
  rm "$T/fake/fail-disk-check-at"
  on src cloud recover moving --backup "$BACKUP_ID" --yes | tee "$T/recovered.out"
  grep -q "Recovered in" "$T/recovered.out"
  grep -q "Kept disk" "$T/recovered.out"
  test "$(cat "$SOURCE_PATH/alpha/README.md")" = "change 4"
  test "$(cat "$T/wrk/workspaces/moving/alpha/README.md")" = "newer than the backup"
  on src --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert not w.get("moved_to") and not w.get("backup"), w'
  grep -q "instances delete chauffeur-worker-w1 --keep-disks=all" "$T/fake/calls"
  refute grep -q -- "--delete-disks" "$T/fake/calls"
  if on src worker create w1 --yes 2>"$T/refused"; then echo "a retained worker was recreated"; exit 1; fi
  grep -q "retained for backup recovery" "$T/refused"
  echo "e2e-cloud-workers recovery: ok"
  exit 0
fi

echo "== cancelled return. backups resume on the worker, and a later return still replaces the Mac's copy"
chmod 555 "$T/src/workspaces"
if on src cloud transfer moving --back 2>"$T/return-refused"; then
  echo "a return replaced a copy in a read-only parent folder"; exit 1
fi
on src --json cloud transfer moving --status | python3 -c 'import sys,json; t=json.load(sys.stdin); t=t.get("transfer",t); assert t["phase"]=="failed" and "restoring it on the destination" in t["log"], t'
on src cloud transfer moving --back --cancel >/dev/null
chmod 755 "$T/src/workspaces"
on src --json cloud backup moving | python3 -c 'import sys,json; b=json.load(sys.stdin); assert b["health"]=="current" and b["backups"], b'

echo "== back. the workspace comes back from the worker"
on src cloud transfer moving --back >/dev/null
for _ in $(seq 1 60); do curl -s -o /dev/null "http://127.0.0.1:$SRC_BASE/" || break; sleep 0.25; done
refute curl -s -o /dev/null "http://127.0.0.1:$SRC_BASE/"
on src --json show moving | python3 -c 'import sys,json; w=json.load(sys.stdin); assert not w.get("moved_to") and not w.get("backup"), w'

echo "== delete. the VM and its forward go"
on src worker delete w1 --yes 2>&1 | tee "$T/delete.out"
grep -q "w1 is deleted" "$T/delete.out"
test ! -e "$T/fake/instances/chauffeur-worker-w1"
test ! -e "$T/fake/accounts/cw-backup-w1@e2e-project.iam.gserviceaccount.com"
grep -q "storage managed-folders remove-iam-policy-binding gs://e2e-backups/w1-[0-9a-f]\{8\}/ --member serviceAccount:cw-backup-w1@e2e-project.iam.gserviceaccount.com" "$T/fake/calls"
test "$(find "$T/fake/bucket/e2e-backups" -name '*.bin' | wc -l | tr -d ' ')" = 3
grep -q "compute instances delete chauffeur-worker-w1 --delete-disks=all --project e2e-project --zone europe-west9-c --quiet" "$T/fake/calls"
for _ in $(seq 1 20); do pgrep -f "$FORWARD" >/dev/null || break; sleep 0.1; done
refute pgrep -f "$FORWARD"
test ! -e "$T/src/home/workers/w1.sock"
on src worker list | grep -q "^No worker.$"
# Its entry goes with it; the Include stays, the only line Chauffeur wrote in ~/.ssh/config.
refute grep -q "$HOST" "$T/src/home/ssh_config"
test "$(cat "$T/user/.ssh/config")" = "Include \"$T/src/home/ssh_config\""
if [ -n "${AUTOMATIC:-}" ]; then
  echo "== automatic. concurrent sends create one worker, reused until both workspaces return, then stopped"
  on src new --name auto-one >/dev/null
  on src new --name auto-two >/dev/null
  on src cloud transfer auto-one --to cloud --yes >"$T/auto-one.out" 2>&1 &
  FIRST=$!
  on src cloud transfer auto-two --to cloud --yes >"$T/auto-two.out" 2>&1 &
  SECOND=$!
  wait "$FIRST" || { cat "$T/auto-one.out"; exit 1; }
  wait "$SECOND" || { cat "$T/auto-two.out"; exit 1; }
  AUTO=$(on src --json show auto-one | python3 -c 'import sys,json; print(json.load(sys.stdin)["moved_to"])')
  on src --json show auto-two | python3 -c 'import sys,json; assert json.load(sys.stdin)["moved_to"] == sys.argv[1]' "$AUTO"
  test "$(grep -c '^compute instances create chauffeur-worker-cloud-' "$T/fake/calls")" = 1
  on src cloud transfer auto-one --back >/dev/null
  test -e "$T/fake/instances/chauffeur-worker-$AUTO"
  # The remaining reservation survives a source daemon restart.
  on src daemon stop >/dev/null
  on src daemon start >/dev/null
  test -e "$T/fake/instances/chauffeur-worker-$AUTO"
  on src cloud transfer auto-two --back >/dev/null
  stopped() {
    for _ in $(seq 1 80); do
      on src --json worker list | python3 -c 'import sys,json; sys.exit(json.load(sys.stdin)["workers"][0]["state"] != "stopped")' && return
      sleep 0.25
    done
    on src worker list; exit 1
  }
  stopped
  # Its disk, account and backups stay: only the VM stops.
  grep -q '"status":"TERMINATED"' "$T/fake/instances/chauffeur-worker-$AUTO"
  test -e "$T/fake/accounts/cw-backup-$AUTO@e2e-project.iam.gserviceaccount.com"
  grep -q "compute instances stop chauffeur-worker-$AUTO --project e2e-project --zone europe-west9-c --quiet" "$T/fake/calls"
  refute grep -q "compute instances delete chauffeur-worker-$AUTO" "$T/fake/calls"

  echo "== automatic auth. logged out preparation leaves the workspace local and the worker stopped"
  on src new --name auto-auth >/dev/null
  touch "$T/fake/logged-out"
  if on src cloud transfer auto-auth --to cloud --yes >"$T/auth.out" 2>&1; then echo "logged out creation succeeded"; exit 1; fi
  grep -q 'gcloud is not logged in' "$T/auth.out"
  on src --json show auto-auth | python3 -c 'import sys,json; w=json.load(sys.stdin); assert w["state"]=="ready" and not w.get("moved_to"), w'
  on src cloud transfer auto-auth --to cloud --cancel >/dev/null
  stopped
  rm "$T/fake/logged-out"
  echo "== automatic restart. the next send starts the stopped worker again, Chauffeur installed again"
  on src cloud transfer auto-auth --to cloud --yes >/dev/null
  on src --json show auto-auth | python3 -c 'import sys,json; assert json.load(sys.stdin)["moved_to"] == sys.argv[1]' "$AUTO"
  test "$(grep -c '^compute instances create chauffeur-worker-cloud-' "$T/fake/calls")" = 1
  grep -q "compute instances start chauffeur-worker-$AUTO" "$T/fake/calls"
  awk -v start="compute instances start chauffeur-worker-$AUTO" 'index($0, start) == 1 { s = 1 } s && /^compute scp / { f = 1 } END { exit !f }' "$T/fake/calls"
  refute grep -q '"status":"TERMINATED"' "$T/fake/instances/chauffeur-worker-$AUTO"
  on src cloud transfer auto-auth --back >/dev/null
  stopped
  echo "== automatic delete. a stopped worker is deleted on demand, its disk and account with it"
  on src worker delete "$AUTO" --yes >/dev/null
  on src worker list | grep -q '^No worker.$'
  test ! -e "$T/fake/instances/chauffeur-worker-$AUTO"
  test ! -e "$T/fake/accounts/cw-backup-$AUTO@e2e-project.iam.gserviceaccount.com"
  refute grep -q "$AUTO" "$T/src/home/ssh_config"
fi
echo "e2e-cloud-workers: ok"
