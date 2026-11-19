# Checks shared by the e2e scripts, sourced by each: `source scripts/e2e-lib.sh` from the repo's
# root, `source "$(dirname "$0")/e2e-lib.sh"` before a script changes folder.

# A check on a command's output (`c ls | has "^name"`): reads all of it first, so that under
# pipefail the command never dies of SIGPIPE when grep stops at its first match.
has() { local out; out=$(cat); grep -q "$@" <<< "$out"; }

# A command that must be refused (`refuses "<part of its message>" c new …`): it fails and says
# that on stderr, so that a usage error or a crash fails the step instead of passing it.
refuses() {
  local want=$1 err
  shift
  if err=$("$@" 2>&1 >/dev/null); then
    echo "accepted, though it must be refused: $*"
    return 1
  fi
  if ! grep -qF -- "$want" <<< "$err"; then
    echo "refused for another reason than \"$want\": $*: $err"
    return 1
  fi
}
