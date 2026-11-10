#!/usr/bin/env bash
# Explicit opt-in on the existing VM only. Does not deploy or restart services.
set -euo pipefail
set +x
umask 077
project_id=${GCP_PROJECT:?Set GCP_PROJECT to the existing Chauffeur project}
region=${GCP_REGION:?Set GCP_REGION to the existing Chauffeur region}
zone=${GCP_ZONE:-$region-a}
instance=${GCP_INSTANCE:-chauffeur-relay}
secret_id=chauffeur-mistral-api-key
mode=${1:-}
if [[ $# != 1 || ( "$mode" != --stdin && "$mode" != --existing-secret ) ]]; then
  echo 'Usage: configure-voice.sh (--stdin | --existing-secret)' >&2
  exit 1
fi
command -v gcloud >/dev/null
# Refuse arbitrary principals: only this project's existing Chauffeur VM account.
service_account=$(gcloud compute instances describe "$instance" --zone="$zone" --project="$project_id" --format='value(serviceAccounts[0].email)')
[[ "$service_account" == "chauffeur-relay@$project_id.iam.gserviceaccount.com" ]] || { echo 'Unexpected Chauffeur service account.' >&2; exit 1; }
if [[ "$mode" == --stdin ]]; then
  # Read the value without echo, argv, temporary files, or gcloud output containing it.
  if [[ -t 0 ]]; then
    IFS= read -r -s -p 'Dedicated Mistral API key: ' voice_key
    printf '\n' >&2
  else
    voice_key=$(cat)
  fi
  [[ "$voice_key" =~ ^[A-Za-z0-9_-]{16,255}$ ]] || { echo 'Invalid Mistral API key format.' >&2; exit 1; }
  if ! gcloud secrets describe "$secret_id" --project="$project_id" >/dev/null 2>&1; then
    gcloud secrets create "$secret_id" --replication-policy=user-managed --locations="$region" --project="$project_id" --quiet >/dev/null
  fi
  printf '%s' "$voice_key" | gcloud secrets versions add "$secret_id" --data-file=- --project="$project_id" --quiet >/dev/null
  unset voice_key
else
  gcloud secrets versions describe latest --secret="$secret_id" --project="$project_id" --format='value(state)' | {
    read -r state
    [[ "$state" == ENABLED ]]
  }
fi
# Secret-level access only: never grant a project-wide secretAccessor role.
gcloud secrets add-iam-policy-binding "$secret_id" --member="serviceAccount:$service_account" --role=roles/secretmanager.secretAccessor --project="$project_id" --quiet >/dev/null
gcloud compute instances add-metadata "$instance" --zone="$zone" --project="$project_id" --metadata="chauffeur-voice-secret=$secret_id" --quiet >/dev/null
echo 'Voice secret configured. Deploy the reviewed relay artifact and startup script, then restart the relay through the reviewed rollout.'
