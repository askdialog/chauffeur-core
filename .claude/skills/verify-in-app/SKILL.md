---
name: verify-in-app
description: See a change of Chauffeur working in the app, on a throwaway daemon that never touches the user's ~/.chauffeur — seeded repos, the app's dev server in a headless Chrome of the agent's own, a fake gh for pull requests. Use after changing the app or what it shows, before saying a UI change works.
---

# Verify a change in the app

Tests prove the logic; a UI change is done once it was seen. Never point the dev app or a script at the user's `~/.chauffeur`: it holds their real workspaces and chats, and some actions write (marking a workspace read, renaming, archiving).

## 1. A throwaway daemon

```bash
cargo build -q -p chauffeurd -p chauffeur-cli
T=/tmp/<feature> && rm -rf $T && mkdir -p $T/home
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
for name in shop-web shop-api; do       # one or more repos, each with an "origin"
  mkdir -p $T/seed/$name && git -C $T/seed/$name init -q -b main
  echo "# $name" > $T/seed/$name/README.md
  git -C $T/seed/$name add -A && git -C $T/seed/$name commit -q -m init
  git clone -q --bare $T/seed/$name $T/origin/$name.git
  git clone -q $T/origin/$name.git $T/clones/$name
done
for v in $(env | grep -oE '^CHAUFFEUR_[A-Z0-9_]+'); do unset $v; done   # this workspace's own (see below)
printf 'github_org = ""\ndefault_repos = ["shop-web", "shop-api"]\nbranch_prefix = "jc"\nclone_roots = ["%s/clones"]\nport_range_start = 47950\nlinear_enabled = false\n[office]\nurl = ""\n' $T > $T/home/settings.toml
export CHAUFFEUR_HOME=$T/home CHAUFFEUR_WORKSPACES_DIR=$T/workspaces CHAUFFEUR_REPOS_DIR=$T/repos \
  CHAUFFEUR_PORT_OVERRIDE=0 CHAUFFEURD=$PWD/target/debug/chauffeurd CHAUFFEUR_VITE_PORT=5299
C=target/debug/chauffeur
$C daemon start
$C new --name price-filter                 # workspaces, explorations (--explore), worktrees…
```

- `linear_enabled = false` always: a real Linear call would move real tickets.
- `[office] url = ""`, its default: no office server is reached. To check the Office's network side, run a local `chauffeur-office --insecure-dev` (its README) and point `url` and `dev_email` at it.
- Inside a Chauffeur workspace the shell carries that workspace's variables (`CHAUFFEUR_WORKSPACE_ID`, `CHAUFFEUR_PORT`, `CHAUFFEUR_REPO_PATH_*`, `CHAUFFEUR_HOME` pointing at the real `~/.chauffeur`…). The loop clears them: with `CHAUFFEUR_WORKSPACE_ID` set, the CLI refuses to start a daemon, and the others would leak into the throwaway daemon's agents and terminals.
- Pick a `port_range_start` and a `CHAUFFEUR_VITE_PORT` that nothing else uses. The daemon only accepts the dev server's origin on that port, so set it before `daemon start`.
- Create the state the change needs with the CLI (`new`, `worktree add`, `rename`, `promote`…), not by hand: the daemon owns every write.

## 2. The app in a browser

```bash
cd app && (CHAUFFEUR_HOME=$T/home nohup bunx vite > $T/vite.log 2>&1 &) ; sleep 4
```

The dev server reads `$CHAUFFEUR_HOME/daemon.json` and `token`, and listens on `CHAUFFEUR_VITE_PORT`.

Never use the Playwright MCP server (`browser_navigate`, `browser_click`…): every agent of a session shares its one browser, so another agent can navigate your page away under you. Drive a headless Chrome of your own with `browser.ts`, next to this file: its profile lives in `$T/browser`, it has its own process and DevTools port, and it stays open between commands.

```bash
B=$(git rev-parse --show-toplevel)/.claude/skills/verify-in-app/browser.ts
$B $T/browser open
$B $T/browser go http://localhost:5299
$B $T/browser click '[data-workspace="price-filter"]'   # add `right` for a right-click
$B $T/browser type '[data-testid="composer"]' 'hello'
$B $T/browser press Escape
$B $T/browser wait '[data-testid="merge"]'               # or a number of milliseconds
$B $T/browser text '[data-testid="workspace-title"]'     # the whole page without a selector
$B $T/browser shot $T/after.png                          # then Read the PNG; add a selector for one element
$B $T/browser run 'return await page.locator("[data-testid=promote]").isEnabled()'   # any Playwright code on `page`
```

Selectors are Playwright's (CSS, `text=…`, `role=button[name="Merge"]`). It uses Google Chrome, or Playwright's Chromium if installed; set `CHROME` to another binary. Useful handles:

- a sidebar row: `[data-workspace="<slug>"]` (right-click it for its menu);
- `data-testid`: `model-picker`, `merge`, `confirm-merge`, `promote`, `usage-meter`, `composer`, `archive`, `open-menu`, `workspace-title`, `rename-field`… (`grep -rhoE 'data-testid="[a-z-]+"' app/src`).

Look at every screenshot you take, and test the interaction, not only the first render: click, type, press Escape, reorder, check that what changed persisted (`$C --json show <slug>`).

## 3. Agents cost money

A chat on the throwaway daemon runs a real agent with the user's Claude Code or Codex login.

- Use `--model haiku` (Haiku 5.5) for chats you start on purpose: cheap, and it has auto mode, the mode new chats start in. Haiku 4.5 (`claude-haiku-4-5-20251001`) has none: its chats ask before acting.
- Typing in the app can send a chat message by accident (a field inside a button, a focus that moves). If an agent starts that you did not mean to start, stop it at once: `$C interrupt <slug>`.
- Claude Code keeps a transcript per folder: remove the throwaway ones with `scripts/forget-transcripts.sh $T` (step 5), never with an `rm` of your own under `~/.claude/projects`: a pattern with an empty variable erased the transcripts of every real chat once, and none of them could resume.

## 4. A fake gh, for pull requests

PR status and merging call `gh`. To test them without real PRs, give the daemon a login shell that puts a stand-in first on PATH: the daemon captures that environment when it starts.

```bash
mkdir -p $T/bin && cat > $T/bin/gh <<'EOF'
#!/bin/bash
repo=$(basename "$PWD")
case "$1 $2" in
  "pr list")   # one open PR per branch named *--dec-1
    for a in "$@"; do [ "$p" = --head ] && head=$a; p=$a; done
    case "$head" in
      *--dec-1) echo '[{"number":7,"baseRefName":"main","url":"https://example.com/7","state":"OPEN","title":"Sub-ticket","isDraft":false,"statusCheckRollup":[{"conclusion":"SUCCESS"}],"reviewDecision":"APPROVED","mergeable":"MERGEABLE"}]' ;;
      *) echo '[]' ;;
    esac ;;
  "repo view") echo '{"squashMergeAllowed":true,"mergeCommitAllowed":true,"rebaseMergeAllowed":false,"viewerPermission":"WRITE"}' ;;
  "pr edit"|"pr merge") echo "$repo $*" >> /tmp/<feature>/gh.log ;;   # what Chauffeur asked
  *) exit 1 ;;
esac
EOF
printf '#!/bin/bash\nexport PATH="%s/bin:$PATH"\nexec /bin/bash -c "$4"\n' $T > $T/fakeshell
chmod +x $T/bin/gh $T/fakeshell
SHELL=$T/fakeshell $C daemon start           # before the daemon starts, or restart it
```

`scripts/e2e-cli.sh` uses the same trick (search for `login-shell`).

## 5. Clean up

```bash
$B $T/browser close                              # your Chrome and its profile
lsof -ti tcp:$CHAUFFEUR_VITE_PORT | xargs kill   # the dev server you started, not the workspaces' own
CHAUFFEUR_HOME=$T/home target/debug/chauffeur daemon stop
scripts/forget-transcripts.sh $T                  # the throwaway chats' Claude Code transcripts
rm -rf $T
```

Name the throwaway daemon with its `CHAUFFEUR_HOME` on the stop line, even in the shell of step 1: a new shell carries this workspace's variables again, `CHAUFFEUR_HOME` the real `~/.chauffeur`. From a workspace or an agent's session, the CLI refuses to stop that one (it runs your own chats) and says so; never work around the refusal.
