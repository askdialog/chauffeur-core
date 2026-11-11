#!/usr/bin/env bash
# Runs a build with a Developer ID and App Store Connect API key, from the release
# workflow's secrets (docs/releases.md): Core's scripts/sign-macos.sh then signs and
# notarizes the disk image. The certificate goes into a temporary keychain and the API key into a
# private temporary file, both deleted when the command ends. Without APPLE_DEVELOPER_ID_P12, the
# command runs as is and the image is signed ad hoc.
#
# A developer whose login keychain holds the identity needs none of this: they set
# CHAUFFEUR_CODESIGN_IDENTITY and the CHAUFFEUR_NOTARY_* values themselves.
set -euo pipefail
fail() { echo "with-apple-signing.sh: $1" >&2; exit 1; }

if [[ -z ${APPLE_DEVELOPER_ID_P12:-} ]]; then
  echo "::warning::No APPLE_DEVELOPER_ID_P12 secret: this disk image is signed ad hoc and not notarized, so macOS warns before opening it."
  exec "$@"
fi
for name in APPLE_DEVELOPER_ID_P12_PASSWORD APPLE_TEAM_ID APPLE_NOTARY_KEY_P8 APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER_ID; do
  [[ -n ${!name:-} ]] || fail "the secret $name is missing: set every Apple secret, or none to sign ad hoc."
done
# Checked before the build, not after it at notarization: a pasted id often carries a stray space.
trim() { local value=$1; value=${value#"${value%%[![:space:]]*}"}; printf '%s' "${value%"${value##*[![:space:]]}"}"; }
APPLE_TEAM_ID=$(trim "$APPLE_TEAM_ID")
APPLE_NOTARY_KEY_ID=$(trim "$APPLE_NOTARY_KEY_ID")
APPLE_NOTARY_ISSUER_ID=$(trim "$APPLE_NOTARY_ISSUER_ID")
[[ $APPLE_TEAM_ID =~ ^[A-Z0-9]{10}$ ]] \
  || fail "APPLE_TEAM_ID is not a team id (ten capitals and digits, in parentheses in the certificate's name)."
[[ $APPLE_NOTARY_KEY_ID =~ ^[A-Z0-9]{10}$ ]] \
  || fail "APPLE_NOTARY_KEY_ID is not an App Store Connect key id (ten capitals and digits, as in AuthKey_<key id>.p8)."
[[ $APPLE_NOTARY_ISSUER_ID =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$ ]] \
  || fail "APPLE_NOTARY_ISSUER_ID is not an issuer id: copy the UUID shown above the keys list in App Store Connect → Users and Access → Integrations."

umask 077
work=$(mktemp -d)
keychain=$work/signing.keychain-db
original=()
while IFS= read -r line; do
  line=${line#"${line%%[![:space:]]*}"}
  original+=("${line//\"/}")
done < <(security list-keychains -d user)
cleanup() {
  security list-keychains -d user -s "${original[@]}" || true
  security delete-keychain "$keychain" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

password=$(uuidgen)
security create-keychain -p "$password" "$keychain"
# No automatic lock (a new keychain locks after 5 idle minutes): signing comes an hour into the
# build, and the keychain is deleted when it ends.
security set-keychain-settings "$keychain"
security unlock-keychain -p "$password" "$keychain"
printf '%s' "$APPLE_DEVELOPER_ID_P12" | base64 --decode > "$work/developer-id.p12"
security import "$work/developer-id.p12" -k "$keychain" -f pkcs12 -P "$APPLE_DEVELOPER_ID_P12_PASSWORD" -T /usr/bin/codesign >/dev/null
rm -f "$work/developer-id.p12"
# Lets codesign use the private key without the dialog a keychain shows otherwise.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" >/dev/null
security list-keychains -d user -s "$keychain" "${original[@]}"

# By its SHA-1, so that two certificates of the same name cannot make the choice ambiguous.
identity=$(security find-identity -v -p codesigning "$keychain" |
  awk -v team="($APPLE_TEAM_ID)\"" '/"Developer ID Application: / && index($0, team) { print $2; exit }')
[[ -n $identity ]] || fail "APPLE_DEVELOPER_ID_P12 holds no valid \"Developer ID Application\" identity of team $APPLE_TEAM_ID: export the certificate with its private key from Keychain Access."
# shellcheck disable=SC2153 # a secret from the environment, like the ids trimmed above
printf '%s\n' "$APPLE_NOTARY_KEY_P8" > "$work/notary.p8"

export CHAUFFEUR_CODESIGN_IDENTITY=$identity
export CHAUFFEUR_NOTARY_KEY=$work/notary.p8
export CHAUFFEUR_NOTARY_KEY_ID=$APPLE_NOTARY_KEY_ID
export CHAUFFEUR_NOTARY_ISSUER=$APPLE_NOTARY_ISSUER_ID
# Tauri's bundler reads some APPLE_* names itself: the build sees none of them.
unset APPLE_DEVELOPER_ID_P12 APPLE_DEVELOPER_ID_P12_PASSWORD APPLE_TEAM_ID APPLE_NOTARY_KEY_P8 APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER_ID
"$@"
