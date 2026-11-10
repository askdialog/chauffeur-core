#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077

metadata() {
  curl --fail --silent --show-error --retry 5 -H 'Metadata-Flavor: Google' \
    "http://metadata.google.internal/computeMetadata/v1/$1"
}
project_id=$(metadata project/project-id)
relay_domain=$(metadata instance/attributes/chauffeur-relay-domain)
secret_id=$(metadata instance/attributes/chauffeur-relay-secret)
[[ "$secret_id" == chauffeur-relay-admin-paris ]] || exit 1
voice_secret=$(metadata instance/attributes/chauffeur-voice-secret 2>/dev/null || true)
[[ -z "$voice_secret" || "$voice_secret" == chauffeur-mistral-api-key ]] || exit 1
artifact_uri=$(metadata instance/attributes/chauffeur-relay-artifact)
artifact_sha=$(metadata instance/attributes/chauffeur-relay-sha256)
# The share service (crates/chauffeur-design-share/deploy/gcp.sh sets these) runs beside the
# relay, on its own hosts, when its metadata is there.
share_image=$(metadata instance/attributes/chauffeur-share-image 2>/dev/null || true)
if [[ -n "$share_image" ]]; then
  # Design shares' host, workspace shares' (the design host when unset), a former host that only
  # redirects to them, and the parent domain one sign-in serves.
  share_domain=$(metadata instance/attributes/chauffeur-share-domain)
  share_workspace_domain=$(metadata instance/attributes/chauffeur-share-workspace-domain 2>/dev/null || true)
  share_workspace_domain=${share_workspace_domain:-$share_domain}
  share_redirect_domain=$(metadata instance/attributes/chauffeur-share-redirect-domain 2>/dev/null || true)
  share_cookie_domain=$(metadata instance/attributes/chauffeur-share-cookie-domain 2>/dev/null || true)
  share_daemon_client=$(metadata instance/attributes/chauffeur-share-daemon-client-id)
  share_web_client=$(metadata instance/attributes/chauffeur-share-web-client-id)
  # Built by Cloud Build into this project's registry, pulled by digest.
  [[ "$share_image" =~ ^([a-z0-9-]+-docker\.pkg\.dev)/$project_id/chauffeur/design-share@sha256:([a-f0-9]{64})$ ]] || exit 1
  share_registry=${BASH_REMATCH[1]}
  share_digest=${BASH_REMATCH[2]}
  # The members' domain (empty: none), the site the e-mails link to, and the e-mails' sender.
  share_member_domain=$(metadata instance/attributes/chauffeur-share-member-domain 2>/dev/null || true)
  [[ -z "$share_member_domain" || "$share_member_domain" =~ ^[a-z0-9][a-z0-9.-]+\.[a-z]{2,}$ ]] || exit 1
  share_site_url=$(metadata instance/attributes/chauffeur-share-site-url 2>/dev/null || true)
  [[ -z "$share_site_url" || "$share_site_url" =~ ^https://[a-z0-9][a-z0-9.-]+\.[a-z]{2,}(/[A-Za-z0-9._~/-]*)?$ ]] || exit 1
  # The sessions' key, from Secret Manager: a restart signs no one out.
  share_session_secret=$(metadata instance/attributes/chauffeur-share-session-secret)
  [[ "$share_session_secret" == chauffeur-share-session-key ]] || exit 1
  share_mail_from=$(metadata instance/attributes/chauffeur-share-mail-from 2>/dev/null || true)
  [[ -z "$share_mail_from" || "$share_mail_from" =~ ^[A-Za-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}$ ]] || exit 1
  share_mail_from_name=$(metadata instance/attributes/chauffeur-share-mail-from-name 2>/dev/null || true)
  share_mail_from_name=${share_mail_from_name:-Chauffeur}
  [[ "$share_mail_from_name" =~ ^[A-Za-z0-9\ .\'-]{1,64}$ ]] || exit 1
  [[ "$share_domain" =~ ^[a-z0-9][a-z0-9.-]+\.[a-z]{2,}$ ]] || exit 1
  [[ "$share_workspace_domain" =~ ^[a-z0-9][a-z0-9.-]+\.[a-z]{2,}$ ]] || exit 1
  [[ -z "$share_redirect_domain" || "$share_redirect_domain" =~ ^[a-z0-9][a-z0-9.-]+\.[a-z]{2,}$ ]] || exit 1
  [[ "$share_redirect_domain" != "$share_domain" && "$share_redirect_domain" != "$share_workspace_domain" ]] || exit 1
  [[ -z "$share_cookie_domain" || "$share_cookie_domain" =~ ^[a-z0-9][a-z0-9.-]+\.[a-z]{2,}$ ]] || exit 1
  [[ "$share_daemon_client" =~ ^[0-9]+-[a-z0-9]+\.apps\.googleusercontent\.com$ ]] || exit 1
  [[ "$share_web_client" =~ ^[0-9]+-[a-z0-9]+\.apps\.googleusercontent\.com$ ]] || exit 1
  # The IAM role invitations are sent with (deploy/aws-mail.sh); none sends no invitation.
  share_mail_role=$(metadata instance/attributes/chauffeur-share-mail-role-arn 2>/dev/null || true)
  [[ -z "$share_mail_role" || "$share_mail_role" =~ ^arn:aws:iam::[0-9]{12}:role/[A-Za-z0-9+=,.@_-]+$ ]] || exit 1
fi
ip_mode=false
if [[ "$relay_domain" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  ip_mode=true
elif [[ ! "$relay_domain" =~ ^[a-z0-9][a-z0-9.-]+\.[a-z]{2,}$ ]]; then
  exit 1
fi
[[ "$artifact_sha" =~ ^[a-f0-9]{64}$ ]] || exit 1

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends docker.io caddy ca-certificates
if ! command -v gcloud >/dev/null; then
  # Official Compute Engine Debian images enable the Google package repository.
  apt-get install -y --no-install-recommends google-cloud-cli
fi
systemctl enable --now docker
# Bounded persistent journals remain on this VM's Paris disk, without a cloud
# logging agent or HTTP access logs containing websocket credentials.
install -d -m 0755 /etc/systemd/journald.conf.d
cat > /etc/systemd/journald.conf.d/chauffeur-relay.conf <<'JOURNAL'
[Journal]
Storage=persistent
SystemMaxUse=100M
MaxRetentionSec=7day
JOURNAL
systemctl restart systemd-journald
install -d -m 0755 /opt/chauffeur-relay
artifact_tmp=$(mktemp -d)
relay_env=""
trap 'rm -rf "$artifact_tmp"; [[ -z "$relay_env" ]] || rm -f "$relay_env"' EXIT
gcloud storage cp "$artifact_uri" "$artifact_tmp/relay.tar.gz" --quiet
printf '%s  %s\n' "$artifact_sha" "$artifact_tmp/relay.tar.gz" | sha256sum --check --status
# The archive is built from two fixed source paths by gcp.sh, never user data.
tar -xzf "$artifact_tmp/relay.tar.gz" -C /opt/chauffeur-relay
chmod -R a+rX /opt/chauffeur-relay

relay_token=$(gcloud secrets versions access latest --secret="$secret_id" --project="$project_id" | tr -d '\r\n')
[[ "$relay_token" =~ ^[A-Za-z0-9_-]{43,128}$ ]] || exit 1
relay_env=$(mktemp /etc/chauffeur-relay.env.XXXXXX)
printf 'RELAY_ADMIN_TOKEN=%s\nPORT=8080\nRELAY_HOST=127.0.0.1\nFCM_PROJECT=%s\n' "$relay_token" "$project_id" > "$relay_env"
unset relay_token
if [[ -n "$voice_secret" ]]; then
  voice_key=$(gcloud secrets versions access latest --secret="$voice_secret" --project="$project_id")
  [[ "$voice_key" =~ ^[A-Za-z0-9_-]{16,255}$ ]] || exit 1
  printf 'MISTRAL_API_KEY=%s\n' "$voice_key" >> "$relay_env"
  unset voice_key
fi
# APNS is provisioned separately. Preserve its fixed env-file fields without sourcing shell.
if [[ -e /etc/chauffeur-relay.env ]]; then
  [[ -f /etc/chauffeur-relay.env && ! -L /etc/chauffeur-relay.env && $(stat -c '%u:%a' /etc/chauffeur-relay.env) == 0:600 ]] || exit 1
  while IFS='=' read -r name value || [[ -n "$name" ]]; do
    case "$name" in
      APNS_TEAM_ID|APNS_KEY_ID) [[ "$value" =~ ^[A-Z0-9]{10}$ ]] || exit 1 ;;
      APNS_TOPIC) [[ "$value" =~ ^[A-Za-z0-9][A-Za-z0-9.-]{0,254}$ ]] || exit 1 ;;
      APNS_KEY_FILE) [[ "$value" =~ ^/[A-Za-z0-9_./-]+$ && ${#value} -le 1024 ]] || exit 1 ;;
      APNS_ENVIRONMENT) [[ "$value" == production || "$value" == sandbox ]] || exit 1 ;;
      *) continue ;;
    esac
    printf '%s=%s\n' "$name" "$value" >> "$relay_env"
  done < /etc/chauffeur-relay.env
fi
chmod 0600 "$relay_env"
mv -f "$relay_env" /etc/chauffeur-relay.env
relay_env=""

docker pull oven/bun:1.4.3
# Existing units can carry APNS key-file mounts; retain those operator provisions.
if [[ ! -e /etc/systemd/system/chauffeur-relay.service ]]; then
cat > /etc/systemd/system/chauffeur-relay.service <<'SERVICE'
[Unit]
Description=Chauffeur opaque E2E relay
After=docker.service network-online.target
Requires=docker.service
Wants=network-online.target

[Service]
ExecStartPre=-/usr/bin/docker rm -f chauffeur-relay
ExecStart=/usr/bin/docker run --rm --name chauffeur-relay --log-driver none --network host --read-only --cap-drop ALL --security-opt no-new-privileges --user 65532:65532 --pids-limit 128 --memory 256m --cpus 0.5 --env-file /etc/chauffeur-relay.env --mount type=bind,source=/opt/chauffeur-relay,target=/app,readonly --workdir /app oven/bun:1.4.3 bun main.ts
ExecStop=/usr/bin/docker stop chauffeur-relay
Restart=always
RestartSec=5
TimeoutStopSec=20

[Install]
WantedBy=multi-user.target
SERVICE
fi

if $ip_mode; then
  # Serve ACME HTTP-01 without stopping the relay during renewals. Public IP
  # certificates require Certbot >=5.4 and Let's Encrypt's shortlived profile.
  apt-get install -y --no-install-recommends python3-venv
  python3 -m venv /opt/certbot
  /opt/certbot/bin/pip install --disable-pip-version-check certbot==5.4.0
  install -d -m 0755 /var/www/acme
  cat > /etc/caddy/Caddyfile <<CADDY
{
    default_sni $relay_domain
}
:80 {
    handle /.well-known/acme-challenge/* {
        root * /var/www/acme
        file_server
    }
    handle {
        respond 404
    }
}
CADDY
  systemctl restart caddy
  /opt/certbot/bin/certbot certonly --non-interactive --agree-tos --register-unsafely-without-email \
    --preferred-profile shortlived --webroot --webroot-path /var/www/acme \
    --cert-name chauffeur-relay --ip-address "$relay_domain"
  install -d -m 0750 -o root -g caddy /etc/caddy/certs
  cat > /usr/local/sbin/chauffeur-relay-certificate <<'HOOK'
#!/bin/sh
set -eu
install -m 0640 -o root -g caddy /etc/letsencrypt/live/chauffeur-relay/fullchain.pem /etc/caddy/certs/fullchain.pem
install -m 0640 -o root -g caddy /etc/letsencrypt/live/chauffeur-relay/privkey.pem /etc/caddy/certs/privkey.pem
/usr/bin/systemctl reload caddy
HOOK
  chmod 0750 /usr/local/sbin/chauffeur-relay-certificate
  /usr/local/sbin/chauffeur-relay-certificate
  cat >> /etc/caddy/Caddyfile <<CADDY
https://$relay_domain {
    tls /etc/caddy/certs/fullchain.pem /etc/caddy/certs/privkey.pem
    reverse_proxy 127.0.0.1:8080
}
CADDY
  cat > /etc/systemd/system/chauffeur-relay-certificate.service <<'SERVICE'
[Unit]
Description=Renew Chauffeur relay IP certificate
After=network-online.target caddy.service
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/opt/certbot/bin/certbot renew --quiet --deploy-hook /usr/local/sbin/chauffeur-relay-certificate
SERVICE
  cat > /etc/systemd/system/chauffeur-relay-certificate.timer <<'TIMER'
[Unit]
Description=Check Chauffeur relay certificate every hour

[Timer]
OnCalendar=hourly
RandomizedDelaySec=600
Persistent=true

[Install]
WantedBy=timers.target
TIMER
  systemctl daemon-reload
  systemctl enable --now chauffeur-relay-certificate.timer
else
  cat > /etc/caddy/Caddyfile <<CADDY
$relay_domain {
    reverse_proxy 127.0.0.1:8080
}
CADDY
fi
if [[ -n "$share_image" ]]; then
  # Its own names beside the relay's address: Caddy gets and renews their certificates itself.
  # They answer 502 until the service runs.
  share_sites=$share_domain
  [[ "$share_workspace_domain" == "$share_domain" ]] || share_sites="$share_domain, $share_workspace_domain"
  cat >> /etc/caddy/Caddyfile <<CADDY
$share_sites {
    handle /slack/* {
        reverse_proxy 127.0.0.1:8091
    }
    handle {
        reverse_proxy 127.0.0.1:8090
    }
}
CADDY
  if [[ -n "$share_redirect_domain" ]]; then
    # The former host sends people to the new ones, path and query kept; the API and Slack's
    # callbacks stay served there, for older daemons and the Slack app's configured URLs.
    cat >> /etc/caddy/Caddyfile <<CADDY
$share_redirect_domain {
    handle /slack/* {
        reverse_proxy 127.0.0.1:8091
    }
    handle /v1/* {
        reverse_proxy 127.0.0.1:8090
    }
    handle /health {
        reverse_proxy 127.0.0.1:8090
    }
    handle /w/* {
        redir https://$share_workspace_domain{uri} 308
    }
    handle {
        redir https://$share_domain{uri} 308
    }
}
CADDY
  fi
fi
# Caddy serves the relay again now: the phones' bridges need not wait for the service's image.
chmod 0644 /etc/caddy/Caddyfile
/usr/bin/caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
systemctl enable caddy
systemctl restart caddy
if [[ -n "$share_image" ]]; then
  share_local="chauffeur-design-share:$share_digest"
  if ! docker image inspect "$share_local" >/dev/null 2>&1; then
    # The VM's service account reads the registry; gcloud gives docker its token.
    gcloud auth configure-docker "$share_registry" --quiet
    docker pull "$share_image"
    docker tag "$share_image" "$share_local"
  fi
  # Older images are of no use once this one runs.
  docker image ls --format '{{.Repository}}:{{.Tag}}' chauffeur-design-share | grep -vx "$share_local" | xargs -r docker image rm || true
  docker image prune --force >/dev/null || true
  # The shares' files and database stay on this VM's disk, owned by the container's user.
  install -d -m 0750 -o 65532 -g 65532 /var/lib/chauffeur-design-share
  share_session_key=$(gcloud secrets versions access latest --secret="$share_session_secret" --project="$project_id" | tr -d '\r\n')
  [[ "$share_session_key" =~ ^[a-f0-9]{64}$ ]] || exit 1
  printf 'SHARE_GOOGLE_DAEMON_CLIENT_ID=%s\nSHARE_GOOGLE_WEB_CLIENT_ID=%s\nSHARE_PUBLIC_URL=https://%s\nSHARE_WORKSPACE_PUBLIC_URL=https://%s\nSHARE_COOKIE_DOMAIN=%s\nSHARE_DOMAIN=%s\nSHARE_SITE_URL=%s\nSHARE_SESSION_KEY=%s\nSHARE_MAIL_ROLE_ARN=%s\nSHARE_MAIL_FROM=%s\nSHARE_MAIL_FROM_NAME=%s\nRUST_LOG=info\n' \
    "$share_daemon_client" "$share_web_client" "$share_domain" "$share_workspace_domain" "$share_cookie_domain" "$share_member_domain" "$share_site_url" "$share_session_key" "$share_mail_role" "$share_mail_from" "$share_mail_from_name" > /etc/chauffeur-design-share.env
  chmod 0600 /etc/chauffeur-design-share.env
  cat > /etc/systemd/system/chauffeur-design-share.service <<SERVICE
[Unit]
Description=Chauffeur design share service (a design chat's canvas for named reviewers)
After=docker.service network-online.target
Requires=docker.service
Wants=network-online.target

[Service]
ExecStartPre=-/usr/bin/docker rm -f chauffeur-design-share
ExecStart=/usr/bin/docker run --rm --name chauffeur-design-share --log-driver none --network host --read-only --cap-drop ALL --security-opt no-new-privileges --pids-limit 128 --memory 384m --cpus 0.5 --env-file /etc/chauffeur-design-share.env --mount type=bind,source=/var/lib/chauffeur-design-share,target=/var/lib/chauffeur-design-share $share_local
ExecStop=/usr/bin/docker stop chauffeur-design-share
Restart=always
RestartSec=5
TimeoutStopSec=20

[Install]
WantedBy=multi-user.target
SERVICE
fi
systemctl daemon-reload
systemctl enable --now chauffeur-relay
if [[ -n "$share_image" ]]; then
  systemctl enable chauffeur-design-share
  systemctl restart chauffeur-design-share
fi
systemctl reload caddy
curl --fail --silent --show-error --retry 10 --retry-connrefused --retry-delay 1 \
  http://127.0.0.1:8080/health
if [[ -n "$share_image" ]]; then
  curl --fail --silent --show-error --retry 10 --retry-connrefused --retry-delay 1 \
    http://127.0.0.1:8090/health
fi
echo 'Chauffeur relay startup complete.'
