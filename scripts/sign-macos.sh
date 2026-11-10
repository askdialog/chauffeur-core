#!/usr/bin/env bash
# Signs what scripts/package.sh builds for macOS and, when configured, has Apple notarize it.
#
#   scripts/sign-macos.sh check                 refuses an incomplete configuration, before a build
#   scripts/sign-macos.sh app <Chauffeur.app>   the nested programs, then the app around them
#   scripts/sign-macos.sh dmg <disk image>      the disk image built from the signed app
#
# Nothing set, the app is signed ad hoc and the disk image not at all, as before: macOS asks once
# before opening a downloaded copy. Every value comes from the environment; Core names no identity.
#
# - CHAUFFEUR_CODESIGN_IDENTITY: a "Developer ID Application" identity of a keychain in the search
#   list, by name or SHA-1. Each program is signed under the hardened runtime, with its
#   entitlements (scripts/entitlements/) and a secure timestamp, and so is the disk image.
# - CHAUFFEUR_NOTARY_KEY (path of an App Store Connect API key's .p8 file), CHAUFFEUR_NOTARY_KEY_ID
#   and, for a Team key, CHAUFFEUR_NOTARY_ISSUER: Apple notarizes the app and then the disk image,
#   and each gets its ticket stapled, so that Gatekeeper opens them without a warning, offline too.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
identity=${CHAUFFEUR_CODESIGN_IDENTITY:-}

fail() { echo "sign-macos.sh: $1" >&2; exit 1; }

check() {
  if [[ -n $identity ]] && ! security find-identity -v -p codesigning | grep -F -- "$identity" >/dev/null; then
    fail "no valid signing identity matches CHAUFFEUR_CODESIGN_IDENTITY ($identity): import the Developer ID certificate and its key into a keychain, or unset it to sign ad hoc."
  fi
  [[ -z ${CHAUFFEUR_NOTARY_KEY:-}${CHAUFFEUR_NOTARY_KEY_ID:-}${CHAUFFEUR_NOTARY_ISSUER:-} ]] && return
  [[ -n ${CHAUFFEUR_NOTARY_KEY:-} && -n ${CHAUFFEUR_NOTARY_KEY_ID:-} ]] ||
    fail "notarization needs both CHAUFFEUR_NOTARY_KEY and CHAUFFEUR_NOTARY_KEY_ID: set both, or no CHAUFFEUR_NOTARY_* variable."
  [[ -f $CHAUFFEUR_NOTARY_KEY ]] ||
    fail "CHAUFFEUR_NOTARY_KEY is not a file: give the path of the App Store Connect API key (.p8)."
  [[ -n $identity ]] ||
    fail "Apple notarizes only code signed with a Developer ID: set CHAUFFEUR_CODESIGN_IDENTITY too."
}

notarizing() { [[ -n ${CHAUFFEUR_NOTARY_KEY:-} ]]; }

# Right after Apple accepts a submission, its ticket may not be served yet: stapler then fails
# (exit 68, record not found). It is asked again a few times, half a minute apart.
staple() {
  local attempt
  for attempt in 1 2 3 4 5; do
    xcrun stapler staple -q "$1" && return
    ((attempt < 5)) || break
    echo "could not staple $(basename "$1") yet, Apple may not serve its ticket so soon: trying again in 30 s" >&2
    sleep 30
  done
  fail "could not staple Apple's ticket to $(basename "$1") after 5 attempts: check Apple's system status, then build again."
}

# <file> [entitlements]
sign() {
  if [[ -z $identity ]]; then
    codesign --force --sign - "$1"
  else
    codesign --force --sign "$identity" --options runtime --timestamp ${2:+--entitlements "$2"} "$1"
  fi
}

# Submits a zip or a disk image and waits for Apple's verdict; a refusal prints Apple's log, which
# names each file it refused and why.
notarize() {
  local credentials=(--key "$CHAUFFEUR_NOTARY_KEY" --key-id "$CHAUFFEUR_NOTARY_KEY_ID")
  [[ -n ${CHAUFFEUR_NOTARY_ISSUER:-} ]] && credentials+=(--issuer "$CHAUFFEUR_NOTARY_ISSUER")
  local result status id
  echo "notarizing $(basename "$1"), which takes a few minutes" >&2
  result=$(xcrun notarytool submit "$1" "${credentials[@]}" --wait --output-format json) || true
  status=$(plutil -extract status raw -o - - <<<"$result" 2>/dev/null) || status=
  id=$(plutil -extract id raw -o - - <<<"$result" 2>/dev/null) || id=
  [[ -n $id ]] || fail "could not submit $(basename "$1") to Apple's notary service: check the API key, its id and issuer, and the network. $result"
  if [[ $status != Accepted ]]; then
    xcrun notarytool log "$id" "${credentials[@]}" >&2 || true
    fail "Apple did not notarize $(basename "$1") (submission $id: ${status:-no status}); its log above says why."
  fi
}

app() {
  local app=$1 macos=$1/Contents/MacOS
  # Inside out: the nested programs first, then the app, whose signature seals them and signs its
  # main executable, chauffeur-app. Entitlements (plain plists: codesign refuses XML comments):
  # - daemon.plist: Apple events. Started at login, the daemon is the process macOS holds
  #   responsible for its agents and terminals, which may script other apps.
  # - bun.plist: what `bun build --compile` programs need, JavaScriptCore's JIT and the native
  #   modules Bun extracts; the set Claude Code's own Bun binary is signed with.
  # - app.plist: the camera and microphone of the Office's calls, and Apple events for the agents
  #   of a daemon the app started.
  sign "$macos/chauffeur"
  sign "$macos/chauffeurd" "$here/entitlements/daemon.plist"
  sign "$macos/chauffeur-sidecar" "$here/entitlements/bun.plist"
  sign "$macos/chauffeur-mobile-host" "$here/entitlements/bun.plist"
  sign "$app" "$here/entitlements/app.plist"
  codesign --verify --deep --strict "$app"
  notarizing || return 0
  local zip
  zip=$(mktemp -d)/Chauffeur.zip
  ditto -c -k --keepParent "$app" "$zip"
  notarize "$zip"
  rm -rf "$(dirname "$zip")"
  staple "$app"
  spctl --assess --type execute "$app" || fail "Gatekeeper refuses $app, notarized as it is."
}

dmg() {
  [[ -n $identity ]] || return 0
  codesign --force --sign "$identity" --timestamp "$1"
  notarizing || return 0
  notarize "$1"
  staple "$1"
  spctl --assess --type open --context context:primary-signature "$1" ||
    fail "Gatekeeper refuses $1, notarized as it is."
}

check
case ${1:-} in
  check) ;;
  app | dmg) [[ -e ${2:-} ]] || fail "$1 needs the path of what to sign."; "$1" "$2" ;;
  *) fail "usage: scripts/sign-macos.sh check | app <Chauffeur.app> | dmg <disk image>" ;;
esac
