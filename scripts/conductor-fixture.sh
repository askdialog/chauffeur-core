#!/usr/bin/env bash
# A synthetic Conductor setup in <dir>, for the import's end-to-end check: Conductor 0.93's
# database schema (crates/chauffeurd/tests/fixtures/conductor-0.93.sql) with made-up rows, two
# clones whose `origin` is a GitHub URL rewritten to a local bare repo, and Conductor worktrees:
#   lima  alpha on fix-1-pricing: a commit, tracked, staged and untracked changes, `.context/` notes
#   oslo  alpha on feat/linked, with beta's clone as an /add-dir folder
#   old-work (archived) and cloudy (a Conductor Cloud workspace), without folders.
# Alpha has a setup and a run script typed in Conductor's UI and a variable API_TOKEN.
set -euo pipefail
dir=$(cd "$1" && pwd -P)
schema="$(cd "$(dirname "$0")/.." && pwd)/crates/chauffeurd/tests/fixtures/conductor-0.93.sql"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t

for name in alpha beta; do
  mkdir -p "$dir/seed/$name"
  git -C "$dir/seed/$name" init -q -b main
  printf '%s\n' "$name" > "$dir/seed/$name/README.md"
  printf 'keep\n' > "$dir/seed/$name/old.txt"
  git -C "$dir/seed/$name" add -A && git -C "$dir/seed/$name" commit -q -m init
  git clone -q --bare "$dir/seed/$name" "$dir/origin/$name.git"
  git clone -q "$dir/origin/$name.git" "$dir/clones/$name"
  git -C "$dir/clones/$name" remote set-url origin "https://github.com/fixture/$name.git"
  git -C "$dir/clones/$name" config "url.$dir/origin/$name.git.insteadOf" "https://github.com/fixture/$name.git"
  printf '.context/\n' >> "$dir/clones/$name/.git/info/exclude"
done

lima="$dir/workspaces/alpha/lima"
oslo="$dir/workspaces/alpha/oslo"
git -C "$dir/clones/alpha" worktree add -q -b fix-1-pricing "$lima"
printf 'price filter\n' > "$lima/filter.txt"
git -C "$lima" add filter.txt && git -C "$lima" commit -q -m "price filter"
printf 'alpha, edited\n' > "$lima/README.md"
printf 'staged\n' > "$lima/staged.txt" && git -C "$lima" add staged.txt
git -C "$lima" rm -q old.txt
mkdir -p "$lima/notes" "$lima/.context"
printf 'untracked\n' > "$lima/notes/untracked.txt"
printf '# Plan\nShip the price filter.\n' > "$lima/.context/plan.md"
git -C "$dir/clones/alpha" worktree add -q -b feat/linked "$oslo"

sqlite3 "$dir/conductor.db" < "$schema"
sql() { sqlite3 "$dir/conductor.db" "$1"; }
sql "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (152, 'fixture', 1, x'00', 0);"
sql "INSERT INTO repos (id, name, remote_url, root_path, default_branch, setup_script, run_script, run_script_mode)
     VALUES ('r-alpha', 'alpha', 'https://github.com/fixture/alpha.git', '$dir/clones/alpha', 'main',
             'echo conductor-setup > .conductor-setup-ran', 'echo run', 'nonconcurrent'),
            ('r-beta', 'beta', 'git@github.com:fixture/beta.git', '$dir/clones/beta', 'main', NULL, NULL, 'concurrent');"
sql "INSERT INTO env_vars (key, scope, context, value) VALUES ('API_TOKEN', 'repo', 'r-alpha', 's3cret-fixture-value');"
sql "INSERT INTO workspaces (local_id, id, repository_id, directory_name, workspace_name, branch, state, workspace_path, linked_directory_paths, hosting_server_url)
     VALUES ('w-lima', 'w-lima', 'r-alpha', 'lima', 'Pricing filters', 'fix-1-pricing', 'ready', '$lima', NULL, NULL),
            ('w-oslo', 'w-oslo', 'r-alpha', 'oslo', 'Linked work', 'feat/linked', 'ready', '$oslo', '[\"$dir/clones/beta\"]', NULL),
            ('w-old', 'w-old', 'r-alpha', 'quito', 'Old work', 'old-work', 'archived', NULL, NULL, NULL),
            ('w-cloud', 'w-cloud', 'r-alpha', 'rome', 'Cloudy', 'cloudy', 'ready', NULL, NULL, 'https://cloud.example');"
sql "INSERT INTO sessions (id, workspace_id, agent_type, title, model, created_at) VALUES
     ('s-1', 'w-lima', 'claude', 'Price filter', 'opus', '2026-10-02T05:37:21.545Z'),
     ('s-2', 'w-lima', 'codex', 'Review', 'gpt-5', '2026-10-03 09:00:00'),
     ('s-3', 'w-oslo', 'claude', 'Linked', NULL, '2026-10-04T10:00:00.000Z'),
     ('s-4', 'w-old', 'claude', 'Old chat', NULL, '2026-09-01T10:00:00.000Z'),
     ('s-5', 'w-lima', 'claude', 'Empty', NULL, '2026-10-05T10:00:00.000Z');"
sql "INSERT INTO session_messages (id, session_id, role, content, created_at, cancelled_at) VALUES
     ('m-1', 's-1', 'user', 'Add a price filter to the catalogue, codename PAPAYA-42.', '2026-10-02T05:37:21.545Z', NULL),
     ('m-2', 's-1', 'assistant', '{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"rg price\"}}]}}', '2026-10-02T05:38:00.000Z', NULL),
     ('m-3', 's-1', 'assistant', '{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"The filter lives in filter.txt.\"}]}}', '2026-10-02T05:39:00.000Z', NULL),
     ('m-4', 's-1', 'user', 'never sent', '2026-10-02T05:40:00.000Z', '2026-10-02T05:40:01.000Z'),
     ('m-5', 's-2', 'user', 'Review the filter', '2026-10-03 09:00:00', NULL),
     ('m-6', 's-2', 'assistant', '{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Looks good.\"}]}}', '2026-10-03 09:01:00', NULL),
     ('m-7', 's-3', 'user', 'Work across alpha and beta', '2026-10-04T10:00:00.000Z', NULL),
     ('m-8', 's-4', 'user', 'An old question', '2026-09-01T10:00:00.000Z', NULL);"
