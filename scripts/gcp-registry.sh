#!/usr/bin/env bash
# The Artifact Registry repository the deploy scripts of the share service and the office server
# push their images to (docs/adr/share-image-built-off-the-vm.md): made if missing, keeping the
# five newest images of each service, readable by the service account of the VM that pulls them.
# Explicit deployment step, run by those scripts; never run from tests.
#
#   scripts/gcp-registry.sh <reader service account e-mail>
set -euo pipefail

project_id=${GCP_PROJECT:?Set GCP_PROJECT to the GCP project of the registry}
region=${GCP_REGION:?Set GCP_REGION, the registry location}
reader=${1:?the service account that pulls the images}
if ! gcloud artifacts repositories describe chauffeur --location="$region" --project="$project_id" >/dev/null 2>&1; then
  gcloud artifacts repositories create chauffeur --repository-format=docker --location="$region" \
    --description='Chauffeur images the VMs pull' --project="$project_id"
fi
# A rollback goes back to one of the last five; older images are deleted (a policy runs about daily).
policy=$(mktemp)
trap 'rm -f "$policy"' EXIT
cat > "$policy" <<'POLICY'
[
  {"name": "keep-newest-5", "action": {"type": "Keep"}, "mostRecentVersions": {"keepCount": 5}},
  {"name": "delete-older", "action": {"type": "Delete"}, "condition": {"tagState": "any"}}
]
POLICY
gcloud artifacts repositories set-cleanup-policies chauffeur --location="$region" --project="$project_id" \
  --policy="$policy" --no-dry-run --quiet >/dev/null
gcloud artifacts repositories add-iam-policy-binding chauffeur --location="$region" --project="$project_id" \
  --member="serviceAccount:$reader" --role=roles/artifactregistry.reader --quiet >/dev/null
