#!/usr/bin/env bash
# Deploys the share service (docs/SPEC.md, stories 51 and 53) on the relay's VM, beside the relay:
# a second container behind the same Caddy, serving design shares on design.example.com and
# workspace shares on share.example.com (the design host when not given). Cloud Build builds its
# image, Artifact Registry keeps it, and the VM's startup script runs again over IAP SSH to pull
# it: the VM is not restarted, Caddy reloads for a moment. Explicit command; never run from tests.
#
#   SHARE_GOOGLE_DAEMON_CLIENT_ID=… SHARE_GOOGLE_WEB_CLIENT_ID=… \
#     [SHARE_COOKIE_DOMAIN=example.com] [SHARE_REDIRECT_HOST=old.example.org] \
#     bash crates/chauffeur-design-share/deploy/gcp.sh design.example.com [share.example.com]
#
# The hosts may come from SHARE_DESIGN_HOST and SHARE_WORKSPACE_HOST instead of the arguments.
# SHARE_COOKIE_DOMAIN, a parent of both hosts, makes one sign-in serve both. SHARE_REDIRECT_HOST
# is a host the service had before: it only redirects its pages to the new hosts, and still
# serves the API older daemons call there.
#
# Run again to roll out the current sources. The reviewer page's build (app/dist-share, made by
# `bun run build:share` in app/) goes along when it exists; without it the service serves a word.
# The key sessions are signed with lives in Secret Manager (chauffeur-share-session-key), made at
# the first deploy and read by the VM at each start: a deploy signs no one out.
set -euo pipefail
umask 077

project_id=${GCP_PROJECT:?Set GCP_PROJECT to the GCP project of the relay VM}
region=${GCP_REGION:?Set GCP_REGION, the region of the relay VM}
zone=${GCP_ZONE:-$region-a}
vm=chauffeur-relay
name=chauffeur-design-share
host='^[a-z0-9][a-z0-9.-]+\.[a-z]{2,}$'
domain=${1:-${SHARE_DESIGN_HOST:-}}
workspace_domain=${2:-${SHARE_WORKSPACE_HOST:-$domain}}
redirect_domain=${SHARE_REDIRECT_HOST:-}
cookie_domain=${SHARE_COOKIE_DOMAIN:-}
if [[ ! "$domain" =~ $host || ! "$workspace_domain" =~ $host ]]; then
  echo 'Usage: GCP_PROJECT=… GCP_REGION=… [SHARE_DOMAIN=example.com] [SHARE_SITE_URL=https://…] [SHARE_COOKIE_DOMAIN=example.com] [SHARE_REDIRECT_HOST=old.example.org] SHARE_GOOGLE_DAEMON_CLIENT_ID=… SHARE_GOOGLE_WEB_CLIENT_ID=… bash crates/chauffeur-design-share/deploy/gcp.sh design.example.com [share.example.com]' >&2
  exit 1
fi
if [[ -n "$redirect_domain" && ( ! "$redirect_domain" =~ $host || "$redirect_domain" == "$domain" || "$redirect_domain" == "$workspace_domain" ) ]]; then
  echo 'SHARE_REDIRECT_HOST is a former host of the service (old.example.org), other than the two it serves, or empty.' >&2
  exit 1
fi
# A cookie for a domain reaches every host under it: only a parent of the service's two hosts.
if [[ -n "$cookie_domain" ]]; then
  for served in "$domain" "$workspace_domain"; do
    if [[ ! "$cookie_domain" =~ $host || ( "$served" != "$cookie_domain" && "$served" != *".$cookie_domain" ) ]]; then
      echo "SHARE_COOKIE_DOMAIN is a parent domain of $domain and $workspace_domain (example.com), or empty." >&2
      exit 1
    fi
  done
fi
# The service only trusts Google ID tokens issued for these clients: the daemons' desktop client
# (the Office's) and the reviewer page's web client, of one Google Cloud project; any Google
# account signs in with them.
daemon_client=${SHARE_GOOGLE_DAEMON_CLIENT_ID:-}
web_client=${SHARE_GOOGLE_WEB_CLIENT_ID:-}
for client in "$daemon_client" "$web_client"; do
  if [[ ! "$client" =~ ^[0-9]+-[a-z0-9]+\.apps\.googleusercontent\.com$ ]]; then
    echo 'Set SHARE_GOOGLE_DAEMON_CLIENT_ID and SHARE_GOOGLE_WEB_CLIENT_ID to OAuth client IDs (….apps.googleusercontent.com).' >&2
    exit 1
  fi
done
# The members' domain: its owners have no limit on invitations, and the team shares made before
# a share named its team domain open to it. Empty: no members.
member_domain=${SHARE_DOMAIN:-}
if [[ -n "$member_domain" && ! "$member_domain" =~ ^[a-z0-9][a-z0-9.-]+\.[a-z]{2,}$ ]]; then
  echo 'SHARE_DOMAIN is the members'"'"' domain (example.com), or empty.' >&2
  exit 1
fi
# Where "Shared with Chauffeur" links, in e-mails and on the page; empty: no link.
site_url=${SHARE_SITE_URL:-}
if [[ -n "$site_url" && ! "$site_url" =~ ^https://[a-z0-9][a-z0-9.-]+\.[a-z]{2,}(/[A-Za-z0-9._~/-]*)?$ ]]; then
  echo 'SHARE_SITE_URL is an https address (https://example.com), or empty.' >&2
  exit 1
fi
# Invitations go through SES with this role, from this address (deploy/aws-mail.sh); either empty
# sends none. The sender's name, plain ASCII, defaults to Chauffeur.
mail_role=${SHARE_MAIL_ROLE_ARN:-}
mail_from=${SHARE_MAIL_FROM:-}
mail_from_name=${SHARE_MAIL_FROM_NAME:-Chauffeur}
if [[ ! "$mail_from_name" =~ ^[A-Za-z0-9\ .\'-]{1,64}$ ]]; then
  echo "SHARE_MAIL_FROM_NAME may hold letters, digits, spaces, dots, apostrophes and hyphens only." >&2
  exit 1
fi
if [[ -n "$mail_role" && ! "$mail_role" =~ ^arn:aws:iam::[0-9]{12}:role/[A-Za-z0-9+=,.@_-]+$ ]]; then
  echo 'SHARE_MAIL_ROLE_ARN is not an IAM role ARN.' >&2
  exit 1
fi
for program in gcloud git openssl; do
  command -v "$program" >/dev/null || { echo "Missing $program" >&2; exit 1; }
done
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
root=$(cd -- "$script_dir/../../.." && pwd)
if ! gcloud compute instances describe "$vm" --zone="$zone" --project="$project_id" >/dev/null 2>&1; then
  echo "The relay's VM ($vm) is not there: deploy the relay first (relay/deploy/gcp.sh)." >&2
  exit 1
fi
# The key the service signs sessions with, drawn once and kept in Secret Manager beside the
# relay's; only the VM's service account reads it.
session_secret=chauffeur-share-session-key
if ! gcloud secrets describe "$session_secret" --project="$project_id" >/dev/null 2>&1; then
  gcloud services enable secretmanager.googleapis.com --project="$project_id" --quiet
  gcloud secrets create "$session_secret" --replication-policy=user-managed --locations="$region" --project="$project_id"
  openssl rand -hex 32 | gcloud secrets versions add "$session_secret" --data-file=- --project="$project_id"
fi
gcloud secrets add-iam-policy-binding "$session_secret" --member="serviceAccount:chauffeur-relay@$project_id.iam.gserviceaccount.com" \
  --role=roles/secretmanager.secretAccessor --project="$project_id" --quiet >/dev/null
# The relay's artifacts bucket holds Cloud Build's copy of the sources, in Paris as the VM.
bucket=${RELAY_ARTIFACTS_BUCKET:-${project_id}-relay-artifacts}
registry="$region-docker.pkg.dev/$project_id/chauffeur"
artifact_tmp=$(mktemp -d)
trap 'rm -rf "$artifact_tmp"' EXIT

# The sources of the service, the design crate and the protocol it speaks, in a workspace of their own (the
# repository's other members are not uploaded), with the repository's lock file.
src="$artifact_tmp/src"
mkdir -p "$src/crates" "$src/web"
cp "$root/Cargo.lock" "$src/"
# A distribution's workspace patches crates to its own copies (`[patch.crates-io]`), which are not
# uploaded: those sections go, so that it deploys from there as from Core.
sed 's|^members = .*|members = ["crates/chauffeur-design-share", "crates/chauffeur-design", "crates/chauffeur-protocol"]|' "$root/Cargo.toml" \
  | awk '/^\[/ { patch = ($0 ~ /^\[patch\./) } !patch' > "$src/Cargo.toml"
grep -q '^members = \["crates/chauffeur-design-share", "crates/chauffeur-design", "crates/chauffeur-protocol"\]$' "$src/Cargo.toml" || {
  echo "Could not narrow the workspace in $root/Cargo.toml" >&2
  exit 1
}
for crate in chauffeur-design-share chauffeur-design chauffeur-protocol; do
  mkdir -p "$src/crates/$crate"
  cp -R "$root/crates/$crate/Cargo.toml" "$root/crates/$crate/src" "$src/crates/$crate/"
done
cp "$script_dir/Dockerfile" "$src/"
if [[ -f "$root/app/dist-share/index.html" ]]; then
  cp -R "$root/app/dist-share/." "$src/web/"
else
  echo 'No app/dist-share: the service will serve a placeholder page (bun run build:share in app/ to build it).'
  touch "$src/web/.keep"
fi
# Tagged by when and from which commit it was built; the VM pulls it by digest.
image="$registry/design-share:$(date -u +%Y%m%d-%H%M%S)-$(git -C "$root" rev-parse --short=12 HEAD)"
bash "$root/scripts/gcp-registry.sh" "chauffeur-relay@$project_id.iam.gserviceaccount.com"
# Eight cores build it in a few minutes, where the e2-small took over twenty and filled its disk.
gcloud builds submit "$src" --tag="$image" --region="$region" --project="$project_id" \
  --machine-type=e2-highcpu-8 --gcs-source-staging-dir="gs://$bucket/cloud-build" --quiet
# What was built here, whatever the tag points to later.
digest=$(gcloud artifacts docker images describe "$image" --format='value(image_summary.digest)')
[[ "$digest" =~ ^sha256:[a-f0-9]{64}$ ]] || { echo "No digest for $image" >&2; exit 1; }

# The relay VM's startup script reads these, at boot and when run again below.
gcloud compute instances add-metadata "$vm" --zone="$zone" --project="$project_id" \
  --metadata="chauffeur-share-domain=$domain,chauffeur-share-workspace-domain=$workspace_domain,chauffeur-share-redirect-domain=$redirect_domain,chauffeur-share-cookie-domain=$cookie_domain,chauffeur-share-daemon-client-id=$daemon_client,chauffeur-share-web-client-id=$web_client,chauffeur-share-member-domain=$member_domain,chauffeur-share-site-url=$site_url,chauffeur-share-session-secret=$session_secret,chauffeur-share-image=$registry/design-share@$digest,chauffeur-share-mail-role-arn=$mail_role,chauffeur-share-mail-from=$mail_from,chauffeur-share-mail-from-name=$mail_from_name" \
  --metadata-from-file="startup-script=$root/relay/deploy/startup.sh"
gcloud compute ssh "$vm" --zone="$zone" --project="$project_id" --tunnel-through-iap \
  --command='sudo google_metadata_script_runner startup'
address=$(gcloud compute instances describe "$vm" --zone="$zone" --project="$project_id" --format='value(networkInterfaces[0].accessConfigs[0].natIP)')
printf 'DNS: an A record for %s pointing to %s (Caddy then gets its certificate).\n' "$domain" "$address"
link_urls=()
for served in "$workspace_domain" "$redirect_domain"; do
  [[ -n "$served" && "$served" != "$domain" ]] || continue
  printf 'DNS: an A record for %s pointing to %s.\n' "$served" "$address"
  link_urls+=("\"https://$served\"")
done
printf "Once https://%s/health answers ok, set in each Mac's ~/.chauffeur/settings.toml, [design_share]: url = \"https://%s\"" "$domain" "$domain"
if (( ${#link_urls[@]} )); then
  (IFS=,; printf ', link_urls = [%s]' "${link_urls[*]}")
fi
printf '\n'
if [[ "$workspace_domain" != "$domain" ]]; then
  printf 'The web OAuth client needs both https://%s and https://%s as authorized JavaScript origins.\n' "$domain" "$workspace_domain"
fi
