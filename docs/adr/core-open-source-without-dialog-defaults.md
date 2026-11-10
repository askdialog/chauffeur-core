# Core is published as open source, with no default pointing at Dialog's services

- **Date:** 2026-11-05
- **Status:** accepted; license resolved by [core-mit-license](core-mit-license.md)

## Context

Chauffeur Core was extracted from Dialog's repository ([separate-extension-repositories](separate-extension-repositories.md)) but still pointed at Dialog's hosted services by default: the share service, the office server, the Slack service, the mobile relay, Google sign-in restricted to Dialog's Workspace, Dialog's release repository for updates and workers, and Dialog's Sentry. Justin wants Core published as open source, usable by anyone without reaching Dialog's infrastructure.

## Decision

Decided with Justin 2026-11-05:

- Chauffeur Core is published as open source.
- Core ships no default pointing at Dialog's services. Every hosted service is off until configured: `[design_share] url`, `[office] url`, `[mobile] relay` and `[slack] url` are empty by default, `[office] board_repos` is empty, and Google sign-in needs `[office] google_client_id` and `google_client_secret` (empty by default), with `[office] google_hosted_domain` to restrict it to one Google Workspace (empty: any Google account).
- Release repository and error reports are build-time values: `CHAUFFEUR_RELEASE_REPO` (`owner/repo`; unset, a build checks for no updates and GCP workers need `[workers] bundle`) and `CHAUFFEUR_SENTRY_DSN` (unset, nothing is sent).
- The share service names no domain of its own: `--public-url` and `--domain` (`SHARE_DOMAIN`) are required, and invitation e-mails need `--mail-from` (`SHARE_MAIL_FROM`).
- A distribution supplies its defaults: its daemon passes Core's runtime a TOML layer of default settings, under the user's `settings.toml`, and its build sets the release repository and the Sentry DSN. Dialog's distribution does so for Dialog's services.
- Dialog keeps the app identifier `com.askdialog.chauffeur` and the "By Dialog" credit.
- The public repository starts from a fresh history.
- The license is to be chosen.

## Consequences

- A build from source runs with no network service beyond GitHub, the agents and Linear when enabled: sharing, the Office, the phone, Slack, updates and error reports stay off, and the app says what to configure where it matters.
- `SPEC.md` (Config, Hosted services) describes the settings; the relay, share service and Slack service READMEs describe how to host each one, with placeholders for the operator's project, region and domain.
- Dialog's distribution must ship the TOML layer and the build-time values for its users to keep today's behaviour.
- Documents that only describe Dialog's distribution or infrastructure (releases, CI runners, Terraform) leave Core.
- Still open: the license, and the public repository's creation.
