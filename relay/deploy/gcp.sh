#!/usr/bin/env bash
# Explicit deployment command. Creates billable resources; never run from tests.
set -euo pipefail
umask 077

# The GCP project and region to deploy to, and the names of its artifacts bucket and admin secret
# (defaults derived from the project).
project_id=${GCP_PROJECT:?Set GCP_PROJECT to the GCP project to deploy to}
region=${GCP_REGION:?Set GCP_REGION, e.g. europe-west1}
zone=${GCP_ZONE:-$region-a}
relay_domain=${1:-}
if [[ "$relay_domain" != --ip && ! "$relay_domain" =~ ^[a-z0-9][a-z0-9.-]+\.[a-z]{2,}$ ]]; then
  echo 'Usage: bash deploy/gcp.sh (--ip | relay.your-domain.example)' >&2
  exit 1
fi
for program in gcloud openssl tar shasum bun; do
  command -v "$program" >/dev/null || { echo "Missing $program" >&2; exit 1; }
done
if [[ $(gcloud billing projects describe "$project_id" --format='value(billingEnabled)') != True ]]; then
  echo "Enable billing on $project_id before deploying." >&2
  exit 1
fi
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
bucket=${RELAY_ARTIFACTS_BUCKET:-${project_id}-relay-artifacts}
secret_id=${RELAY_ADMIN_SECRET:-chauffeur-relay-admin}
service_account="chauffeur-relay@${project_id}.iam.gserviceaccount.com"
artifact_tmp=$(mktemp -d)
trap 'rm -rf "$artifact_tmp"' EXIT

# No node_modules, repository code, keys, config or attachments in this archive.
bash "$script_dir/package.sh" "$artifact_tmp/relay.tar.gz"
artifact_sha=$(shasum -a 256 "$artifact_tmp/relay.tar.gz" | cut -d ' ' -f 1)
artifact_uri="gs://$bucket/relay-$artifact_sha.tar.gz"

gcloud services enable compute.googleapis.com secretmanager.googleapis.com storage.googleapis.com iap.googleapis.com fcm.googleapis.com --project="$project_id" --quiet
if ! gcloud iam service-accounts describe "$service_account" --project="$project_id" >/dev/null 2>&1; then
  gcloud iam service-accounts create chauffeur-relay --display-name='Chauffeur relay' --project="$project_id"
fi
if ! gcloud secrets describe "$secret_id" --project="$project_id" >/dev/null 2>&1; then
  gcloud secrets create "$secret_id" --replication-policy=user-managed --locations="$region" --project="$project_id"
  openssl rand -hex 32 | gcloud secrets versions add "$secret_id" --data-file=- --project="$project_id"
fi
if ! gcloud iam roles describe chauffeurPushSender --project="$project_id" >/dev/null 2>&1; then
  gcloud iam roles create chauffeurPushSender --project="$project_id" --title='Chauffeur push sender' --permissions=cloudmessaging.messages.create --stage=GA --quiet
fi
gcloud projects add-iam-policy-binding "$project_id" --member="serviceAccount:$service_account" --role="projects/$project_id/roles/chauffeurPushSender" --condition=None --quiet >/dev/null
gcloud secrets add-iam-policy-binding "$secret_id" --member="serviceAccount:$service_account" --role=roles/secretmanager.secretAccessor --project="$project_id" --quiet
if ! gcloud storage buckets describe "gs://$bucket" --project="$project_id" >/dev/null 2>&1; then
  gcloud storage buckets create "gs://$bucket" --location="$region" --uniform-bucket-level-access --public-access-prevention --project="$project_id"
fi
gcloud storage buckets add-iam-policy-binding "gs://$bucket" --member="serviceAccount:$service_account" --role=roles/storage.objectViewer --project="$project_id" --quiet
gcloud storage cp "$artifact_tmp/relay.tar.gz" "$artifact_uri" --project="$project_id" --quiet

if ! gcloud compute networks describe chauffeur-relay --project="$project_id" >/dev/null 2>&1; then
  gcloud compute networks create chauffeur-relay --subnet-mode=custom --project="$project_id"
fi
if ! gcloud compute networks subnets describe chauffeur-relay --region="$region" --project="$project_id" >/dev/null 2>&1; then
  gcloud compute networks subnets create chauffeur-relay --network=chauffeur-relay --range=10.80.0.0/24 --region="$region" --project="$project_id"
fi
if ! gcloud compute firewall-rules describe chauffeur-relay-web --project="$project_id" >/dev/null 2>&1; then
  gcloud compute firewall-rules create chauffeur-relay-web --network=chauffeur-relay --allow=tcp:80,tcp:443 --source-ranges=0.0.0.0/0 --target-tags=chauffeur-relay --project="$project_id"
fi
if ! gcloud compute firewall-rules describe chauffeur-relay-iap --project="$project_id" >/dev/null 2>&1; then
  gcloud compute firewall-rules create chauffeur-relay-iap --network=chauffeur-relay --allow=tcp:22 --source-ranges=35.235.240.0/20 --target-tags=chauffeur-relay --project="$project_id"
fi
if ! gcloud compute addresses describe chauffeur-relay --region="$region" --project="$project_id" >/dev/null 2>&1; then
  gcloud compute addresses create chauffeur-relay --region="$region" --project="$project_id"
fi
address=$(gcloud compute addresses describe chauffeur-relay --region="$region" --project="$project_id" --format='value(address)')
if [[ "$relay_domain" == --ip ]]; then
  relay_domain=$address
fi
if gcloud compute instances describe chauffeur-relay --zone="$zone" --project="$project_id" >/dev/null 2>&1; then
  echo 'Instance already exists. No running instance was changed.' >&2
  echo 'Use a reviewed rollout for an upgrade; the new archive is uploaded but not applied.' >&2
else
  gcloud compute instances create chauffeur-relay --zone="$zone" --project="$project_id" \
    --machine-type=e2-small --network=chauffeur-relay --subnet=chauffeur-relay \
    --address="$address" --tags=chauffeur-relay --service-account="$service_account" --scopes=cloud-platform \
    --image-family=debian-12 --image-project=debian-cloud --boot-disk-size=20GB --boot-disk-type=pd-balanced \
    --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring \
    --metadata="enable-oslogin=TRUE,block-project-ssh-keys=TRUE,chauffeur-relay-domain=$relay_domain,chauffeur-relay-secret=$secret_id,chauffeur-relay-artifact=$artifact_uri,chauffeur-relay-sha256=$artifact_sha" \
    --metadata-from-file="startup-script=$script_dir/startup.sh"
fi
if [[ "$relay_domain" != "$address" ]]; then
  printf 'DNS: create an A record for %s pointing to %s\n' "$relay_domain" "$address"
fi
printf 'Then verify https://%s/health before using wss://%s\n' "$relay_domain" "$relay_domain"
