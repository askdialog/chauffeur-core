#!/usr/bin/env bash
# Removes the Claude Code transcripts of the sessions that ran inside a throwaway folder (an e2e
# run's mktemp folder, verify-in-app's /tmp/<feature>), and only those.
#
#   scripts/forget-transcripts.sh <throwaway folder>
#
# The folder may already be gone: then give its absolute path, as the run printed it.
#
# Never delete under ~/.claude/projects with a pattern by hand: an empty variable in it
# (`rm -rf ~/.claude/projects/*-$T*`) erases the transcripts of every real chat, which can then
# never resume ("No conversation found with session ID"). So this refuses a folder that is not
# strictly inside a temporary folder ($TMPDIR, /tmp), and keeps a transcript folder unless every
# session in it ran in the throwaway folder (its `cwd`), since the encoded names are ambiguous.
set -euo pipefail

usage() { echo "usage: $0 <throwaway folder inside \$TMPDIR or /tmp>" >&2; exit 2; }
[ $# -eq 1 ] && [ -n "$1" ] || usage
if [ -d "$1" ]; then
  dir=$(cd "$1" && pwd -P)
else
  # Already removed: its absolute path then, as the run printed it.
  case "$1" in /*) ;; *) echo "$0: '$1' does not exist: give its absolute path" >&2; exit 2 ;; esac
  case "/$1/" in */../*|*/./*) echo "$0: '$1' has . or .. in it" >&2; exit 2 ;; esac
  dir=${1%/}
fi

inside=
for root in "${TMPDIR:-/tmp}" /tmp /private/tmp; do
  for form in "${root%/}" "$(cd "$root" 2>/dev/null && pwd -P)"; do
    [ -n "$form" ] || continue
    case "$dir" in "$form"/?*) inside=1 ;; esac
  done
done
[ -n "$inside" ] || { echo "$0: $dir is not inside a temporary folder: refusing" >&2; exit 2; }

projects="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects"
[ -d "$projects" ] || exit 0

# Sessions may have been given the folder through a symlink (/tmp → /private/tmp).
forms=("$dir")
case "$dir" in
  /private/*) forms+=("${dir#/private}") ;;
  /var/*|/tmp/*) forms+=("/private$dir") ;;
esac

ran_inside() {
  local cwd form
  for form in "${forms[@]}"; do
    cwd=$1
    [ "$cwd" = "$form" ] && return 0
    case "$cwd" in "$form"/*) return 0 ;; esac
  done
  return 1
}

removed=0
for form in "${forms[@]}"; do
  # Claude Code names a transcript folder after the session's cwd, every other character a dash.
  prefix=$(printf %s "$form" | sed 's/[^A-Za-z0-9]/-/g')
  for folder in "$projects/$prefix" "$projects/$prefix"-*; do
    [ -d "$folder" ] || continue
    # grep finds nothing in a file of metadata only: not a failure of the whole run.
    cwds=$( { find "$folder" -name '*.jsonl' -type f -exec grep -h -o -m1 '"cwd":"[^"]*"' {} + 2>/dev/null || true; } \
      | sed 's/^"cwd":"//; s/"$//' | sort -u)
    if [ -z "$cwds" ]; then
      echo "kept $folder: no session says where it ran" >&2
      continue
    fi
    ours=1
    while IFS= read -r cwd; do ran_inside "$cwd" || { ours=; break; }; done <<< "$cwds"
    if [ -z "$ours" ]; then
      echo "kept $folder: a session in it ran outside $dir" >&2
      continue
    fi
    rm -rf "$folder"
    removed=$((removed + 1))
  done
done
echo "removed $removed transcript folder(s) of sessions in $dir"
