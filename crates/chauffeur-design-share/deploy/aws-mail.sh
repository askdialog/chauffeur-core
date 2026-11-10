#!/usr/bin/env bash
# Sets up what the share service sends its invitations with (docs/SPEC.md, stories 51 and 53):
# the sender's domain in Amazon SES (Easy DKIM, in the region the service sends through,
# src/ses.rs) and an IAM role the relay VM's service account assumes with its Google ID token,
# allowed to send from one address only. The sender's domain may be the Workspace's or a subdomain
# verified alone, so that SES can sign for no other address of the Workspace's. No AWS key is stored anywhere. Explicit command; never
# run from tests. Run again to check it: each step is skipped once done.
#
#   AWS_PROFILE=… AWS_ACCOUNT=123456789012 GCP_PROJECT=… SHARE_DOMAIN=example.com \
#     SHARE_MAIL_FROM=noreply@share.example.com bash crates/chauffeur-design-share/deploy/aws-mail.sh
#
# It prints the three DKIM CNAMEs to add to the domain's DNS: SES sends nothing before they
# answer. In SES's sandbox only verified addresses receive, and invitations go to any address:
# the account needs SES's production access. Then deploy the service with SHARE_MAIL_ROLE_ARN
# (printed at the end), SHARE_MAIL_FROM and SHARE_MAIL_FROM_NAME.
set -euo pipefail

account=${AWS_ACCOUNT:?Set AWS_ACCOUNT to the AWS account id SES sends from}
region=eu-west-1
workspace_domain=${SHARE_DOMAIN:?Set SHARE_DOMAIN to the Google Workspace domain (example.com)}
from=${SHARE_MAIL_FROM:?Set SHARE_MAIL_FROM to the sender address (noreply@share.example.com)}
# The identity SES signs for: the sender's domain, the Workspace's or one of its subdomains.
domain=${from##*@}
if [[ "$domain" != "$workspace_domain" && "$domain" != *".$workspace_domain" ]]; then
  echo "SHARE_MAIL_FROM must be an address of $workspace_domain or of one of its subdomains" >&2
  exit 1
fi
role=chauffeur-design-share-mail
# The service asks Google for an ID token with this audience; the role trusts no other.
audience=chauffeur-design-share
gcp_project=${GCP_PROJECT:?Set GCP_PROJECT to the GCP project of the relay VM}
service_account="chauffeur-relay@$gcp_project.iam.gserviceaccount.com"

for program in aws gcloud; do
  command -v "$program" >/dev/null || { echo "Missing $program" >&2; exit 1; }
done
if [[ "$(aws sts get-caller-identity --query Account --output text)" != "$account" ]]; then
  echo "Sign in to the AWS account $account (AWS_PROFILE) first" >&2
  exit 1
fi
# Google's ID tokens name a service account by its unique id (`sub`), not its e-mail.
subject=$(gcloud iam service-accounts describe "$service_account" --project="$gcp_project" --format='value(uniqueId)')
[[ "$subject" =~ ^[0-9]{10,30}$ ]] || { echo "No unique id for $service_account" >&2; exit 1; }

if ! aws sesv2 get-email-identity --region "$region" --email-identity "$domain" >/dev/null 2>&1; then
  aws sesv2 create-email-identity --region "$region" --email-identity "$domain" \
    --tags Key=service,Value=chauffeur-design-share >/dev/null
fi

trust=$(cat <<JSON
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Principal": {"Federated": "accounts.google.com"},
    "Action": "sts:AssumeRoleWithWebIdentity",
    "Condition": {"StringEquals": {
      "accounts.google.com:sub": "$subject",
      "accounts.google.com:oaud": "$audience"
    }}
  }]
}
JSON
)
if aws iam get-role --role-name "$role" >/dev/null 2>&1; then
  aws iam update-assume-role-policy --role-name "$role" --policy-document "$trust"
else
  aws iam create-role --role-name "$role" --assume-role-policy-document "$trust" \
    --description "The Chauffeur share service (GCP VM chauffeur-relay) sends invitations through SES" \
    --tags Key=service,Value=chauffeur-design-share >/dev/null
fi
aws iam put-role-policy --role-name "$role" --policy-name send-invitations --policy-document "$(cat <<JSON
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Action": "ses:SendEmail",
    "Resource": "arn:aws:ses:$region:$account:identity/$domain",
    "Condition": {"StringEquals": {"ses:FromAddress": "$from"}}
  }]
}
JSON
)"

echo "Role: arn:aws:iam::$account:role/$role (SHARE_MAIL_ROLE_ARN)"
echo "DKIM records for $domain (CNAME, at the DNS host):"
aws sesv2 get-email-identity --region "$region" --email-identity "$domain" \
  --query 'DkimAttributes.Tokens' --output text | tr '\t' '\n' | while read -r token; do
  printf '  %s._domainkey.%s  CNAME  %s.dkim.amazonses.com\n' "$token" "$domain" "$token"
done
echo "Status: $(aws sesv2 get-email-identity --region "$region" --email-identity "$domain" --query 'DkimAttributes.Status' --output text) (SUCCESS once the DNS answers)"
echo "Production access: $(aws sesv2 get-account --region "$region" --query ProductionAccessEnabled --output text) (needed: invitations go to addresses SES has not verified)"
