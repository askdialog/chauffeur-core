#!/usr/bin/env bash
# Explicit rollout on the existing Paris relay; no provisioning or IAM changes.
set -euo pipefail
set +x
umask 077
project_id=${GCP_PROJECT:?Set GCP_PROJECT}
zone=${GCP_ZONE:?Set GCP_ZONE}
instance=${GCP_INSTANCE:?Set GCP_INSTANCE}
bucket=${RELAY_ARTIFACTS_BUCKET:?Set RELAY_ARTIFACTS_BUCKET}
expected_sha=${EXPECTED_RELAY_SHA256:?Set EXPECTED_RELAY_SHA256 from the reviewed VM metadata}
[[ "$project_id" == dialog-chauffeur && "$zone" == europe-west9-a && "$instance" == chauffeur-relay && "$bucket" == dialog-chauffeur-relay-artifacts-paris ]] || {
  echo 'Refusing a target outside the approved existing Chauffeur deployment.' >&2; exit 1;
}
[[ "$expected_sha" =~ ^[a-f0-9]{64}$ ]] || exit 1
[[ $# == 0 ]] || exit 1
for program in gcloud bun python3 shasum; do command -v "$program" >/dev/null; done
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/chauffeur-voice-rollout.XXXXXX")
rollout_id="voice-$(date -u +%Y%m%dT%H%M%SZ)-${stage##*.}"
[[ "$rollout_id" =~ ^voice-[A-Za-z0-9-]+$ ]] || exit 1
remote_attempted=false
metadata_attempted=false
completed=false

# Snapshot the reviewed sources before changing any deployment state.
cp "$script_dir/startup.sh" "$stage/startup.sh"
bash "$script_dir/package.sh" "$stage/relay.tar.gz"
sha=$(shasum -a 256 "$stage/relay.tar.gz" | cut -d ' ' -f 1)
artifact_uri="gs://$bucket/relay-$sha.tar.gz"
gcloud compute instances describe "$instance" --project="$project_id" --zone="$zone" --format=json > "$stage/instance.json"
python3 - "$stage" "$expected_sha" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
vm = json.loads((root / 'instance.json').read_text())
assert [s['email'] for s in vm['serviceAccounts']] == ['chauffeur-relay@dialog-chauffeur.iam.gserviceaccount.com'], 'Unexpected VM account'
m = {i['key']: i['value'] for i in vm['metadata']['items']}
assert m['chauffeur-relay-sha256'] == sys.argv[2], 'Deployment changed since review'
assert m['chauffeur-relay-artifact'] == 'gs://dialog-chauffeur-relay-artifacts-paris/relay-' + sys.argv[2] + '.tar.gz', 'Unexpected artifact'
assert m['chauffeur-relay-domain'] == '34.163.232.64', 'Unexpected relay address'
assert m['chauffeur-relay-secret'] == 'chauffeur-relay-admin-paris', 'Unexpected operator secret'
assert m.get('chauffeur-voice-secret', '') in ('', 'chauffeur-mistral-api-key'), 'Unexpected voice secret'
assert m.get('startup-script'), 'Missing original startup script'
(root / 'old-startup.sh').write_text(m['startup-script'])
(root / 'old-voice-secret').write_text(m.get('chauffeur-voice-secret', ''))
PY
old_voice=$(cat "$stage/old-voice-secret")

cat > "$stage/remote.sh" <<'REMOTE'
set -euo pipefail
set +x
umask 077
action=$1; project_id=$2; artifact_uri=$3; sha=$4; rollout_id=$5
[[ "$project_id" == dialog-chauffeur && "$sha" =~ ^[a-f0-9]{64}$ && "$rollout_id" =~ ^voice-[A-Za-z0-9-]+$ ]] || exit 1
[[ "$artifact_uri" == "gs://dialog-chauffeur-relay-artifacts-paris/relay-$sha.tar.gz" ]] || exit 1
[[ $EUID == 0 ]] || exit 1
root=/var/lib/chauffeur-relay-rollouts
backup="$root/$rollout_id"
lock=/run/chauffeur-relay-voice-rollout
main=/opt/chauffeur-relay/main.ts
env_file=/etc/chauffeur-relay.env
health() {
  systemctl is-active --quiet chauffeur-relay &&
    curl --fail --silent --show-error --connect-timeout 2 --max-time 3 --retry 10 --retry-connrefused --retry-delay 1 --retry-max-time 25 \
      http://127.0.0.1:8080/health > "$backup/health" && [[ $(cat "$backup/health") == ok ]]
}
rollback() {
  if [[ -e "$lock" ]]; then
    [[ -f "$lock/id" && $(cat "$lock/id") == "$rollout_id" ]] || return 1
  elif [[ ! -f "$backup/prepared" ]]; then
    return 0
  fi
  if [[ -f "$backup/prepared" ]]; then
    install -m 0644 "$backup/main.ts" "$main.voice-restore" || return 1
    mv -f "$main.voice-restore" "$main" || return 1
    install -m 0600 "$backup/relay.env" "$env_file.voice-restore" || return 1
    mv -f "$env_file.voice-restore" "$env_file" || return 1
    systemctl restart chauffeur-relay || return 1
    health || { echo 'Relay rollback health check failed; retain root-only backups for recovery.' >&2; return 1; }
  fi
  rm -rf "$lock"
}
case "$action" in
  rollback) rollback; exit ;;
  finalize)
    [[ -f "$lock/id" && $(cat "$lock/id") == "$rollout_id" ]]
    health
    rm -rf "$lock"
    exit ;;
  apply) ;;
  *) exit 1 ;;
esac
# This lock spans the local metadata update as well as the remote file swap.
mkdir -m 0700 "$lock"
printf '%s\n' "$rollout_id" > "$lock/id"
finish() {
  code=$?
  trap - EXIT
  unset voice_key
  rm -f "$env_file.voice-new" "$main.voice-new"
  if [[ $code != 0 ]]; then rollback || true; fi
  exit "$code"
}
trap finish EXIT
[[ -f "$env_file" && ! -L "$env_file" && $(stat -c '%u:%a' "$env_file") == 0:600 ]]
[[ -f "$main" && ! -L "$main" ]]
systemctl is-active --quiet chauffeur-relay
install -d -m 0700 "$root"
mkdir -m 0700 "$backup"
# Fetch only on the VM, as its existing service account. Never pass the key in argv.
voice_key=$(gcloud secrets versions access latest --secret=chauffeur-mistral-api-key --project="$project_id" 2>/dev/null)
[[ "$voice_key" =~ ^[A-Za-z0-9_-]{16,255}$ ]] || { echo 'Voice secret is missing or invalid.' >&2; exit 1; }
gcloud storage cp "$artifact_uri" "$backup/relay.tar.gz" --quiet >/dev/null
printf '%s  %s\n' "$sha" "$backup/relay.tar.gz" | sha256sum --check --status
# Do not extract archive paths or accept symlinks, extra files, or oversized bundles.
python3 - "$backup" <<'PY'
import pathlib, sys, tarfile
root = pathlib.Path(sys.argv[1])
with tarfile.open(root / 'relay.tar.gz', 'r:gz') as archive:
    members = archive.getmembers()
    assert len(members) == 1 and members[0].name == 'main.ts' and members[0].isfile()
    assert 0 < members[0].size <= 16 * 1024 * 1024
    source = archive.extractfile(members[0])
    assert source is not None
    (root / 'new-main.ts').write_bytes(source.read())
PY
install -m 0644 "$main" "$backup/main.ts"
install -m 0600 "$env_file" "$backup/relay.env"
# Preserve every existing Docker env-file entry; do not execute or source it.
awk '!/^MISTRAL_API_KEY=/' "$env_file" > "$env_file.voice-new"
printf 'MISTRAL_API_KEY=%s\n' "$voice_key" >> "$env_file.voice-new"
unset voice_key
chmod 0600 "$env_file.voice-new"
install -m 0644 "$backup/new-main.ts" "$main.voice-new"
touch "$backup/prepared"
mv -f "$env_file.voice-new" "$env_file"
mv -f "$main.voice-new" "$main"
systemctl restart chauffeur-relay
health
printf 'Relay files healthy; backup: %s\n' "$backup"
REMOTE

remote() {
  # Every interpolated argument is fixed or validated to contain no shell syntax.
  gcloud compute ssh "$instance" --project="$project_id" --zone="$zone" --tunnel-through-iap --quiet \
    --command="sudo bash -s -- $1 $project_id $artifact_uri $sha $rollout_id" < "$stage/remote.sh"
}
restore_metadata() {
  gcloud compute instances add-metadata "$instance" --project="$project_id" --zone="$zone" --quiet \
    --metadata="chauffeur-relay-artifact=gs://$bucket/relay-$expected_sha.tar.gz,chauffeur-relay-sha256=$expected_sha${old_voice:+,chauffeur-voice-secret=$old_voice}" \
    --metadata-from-file="startup-script=$stage/old-startup.sh" >/dev/null || return 1
  if [[ -z "$old_voice" ]]; then
    gcloud compute instances remove-metadata "$instance" --project="$project_id" --zone="$zone" --quiet --keys=chauffeur-voice-secret >/dev/null
  fi
}
finish() {
  code=$?
  trap - EXIT
  if ! $completed; then
    if $metadata_attempted; then restore_metadata || echo 'Metadata rollback failed; recover from the retained snapshot.' >&2; fi
    if $remote_attempted; then remote rollback || echo 'Remote rollback failed; inspect the retained root-only backup.' >&2; fi
    printf 'Rollout did not complete. Local recovery snapshot retained: %s\n' "$stage" >&2
  else
    rm -rf "$stage"
  fi
  exit "$code"
}
trap finish EXIT
# No secret access on this computer and no API enablement, IAM, VM creation, or Caddy changes.
gcloud storage cp "$stage/relay.tar.gz" "$artifact_uri" --project="$project_id" --quiet >/dev/null
remote_attempted=true
remote apply
metadata_attempted=true
gcloud compute instances add-metadata "$instance" --project="$project_id" --zone="$zone" --quiet \
  --metadata="chauffeur-relay-artifact=$artifact_uri,chauffeur-relay-sha256=$sha,chauffeur-voice-secret=chauffeur-mistral-api-key" \
  --metadata-from-file="startup-script=$stage/startup.sh" >/dev/null
remote finalize
completed=true
printf 'Relay voice rollout complete: %s\n' "$sha"
