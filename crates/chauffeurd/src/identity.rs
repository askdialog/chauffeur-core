//! The user's Chauffeur identity, shared by Core features and optional extensions, and whether
//! it makes them a member of the distribution (`[distribution] member_domain`).
use std::sync::Arc;

use crate::google::REFRESH_TOKEN;
use crate::{Daemon, Error, Result, google, secrets::Secrets};
use chauffeur_protocol::events::RegistryEvent;
use chauffeur_protocol::identity::IdentityState;

// Preserve the existing key and Keychain service so upgrading keeps the user's sign-in.
pub const EMAIL_KEY: &str = "office_email";
/// The account's Google Workspace domain; `""` for a personal account. Missing for a sign-in made
/// before it was kept, until `learn_team_domain` reads it. Not `office_`-prefixed: extensions
/// cannot write it.
const TEAM_DOMAIN_KEY: &str = "identity_team_domain";

pub fn state(daemon: &Daemon) -> Result<IdentityState> {
    let email = daemon.store.kv_get(EMAIL_KEY)?;
    let settings = daemon.settings.get();
    let team_domain = match email {
        Some(_) => daemon
            .store
            .kv_get(TEAM_DOMAIN_KEY)?
            .filter(|domain| !domain.is_empty()),
        None => None,
    };
    Ok(IdentityState {
        member: email
            .as_deref()
            .is_some_and(|email| settings.distribution.is_member(email)),
        team_domain,
        email,
        can_sign_in: !settings.office.dev_email.trim().is_empty()
            || google::Client::from_settings(&settings.office).is_some(),
    })
}

/// Tells the clients and the extensions that the identity changed, and follows the new user's
/// mentions.
pub fn changed(daemon: &Daemon) {
    daemon.extensions.identity_changed();
    match state(daemon) {
        Ok(identity) => {
            daemon
                .events
                .registry(RegistryEvent::IdentityChanged { identity });
        }
        Err(error) => tracing::warn!(%error, "could not tell the clients of the new identity"),
    }
    daemon.mentions.restart();
}

/// Reads the team domain of a sign-in made before it was kept, from a fresh ID token, once.
pub fn learn_team_domain(daemon: Arc<Daemon>) {
    let known = daemon
        .store
        .kv_get(TEAM_DOMAIN_KEY)
        .ok()
        .flatten()
        .is_some();
    let signed_in = daemon.store.kv_get(EMAIL_KEY).ok().flatten().is_some();
    if known || !signed_in || !daemon.settings.get().office.dev_email.trim().is_empty() {
        return;
    }
    tokio::spawn(async move {
        let settings = daemon.settings.get().office.clone();
        let Some(client) = google::Client::from_settings(&settings) else {
            return;
        };
        let Some(refresh) = Secrets::for_paths(&daemon.paths).read(&REFRESH_TOKEN) else {
            return;
        };
        let token = match client.id_token(&refresh).await {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!(%error, "could not read the signed-in account's team domain");
                return;
            }
        };
        let domain = google::team_domain_of(&token).unwrap_or_default();
        // Signed out meanwhile: nothing to keep.
        if daemon.store.kv_get(EMAIL_KEY).ok().flatten().is_none() {
            return;
        }
        if let Err(error) = daemon.store.kv_set(TEAM_DOMAIN_KEY, &domain) {
            tracing::warn!(%error, "could not keep the signed-in account's team domain");
            return;
        }
        changed(&daemon);
    });
}

pub fn email(daemon: &Daemon) -> Option<String> {
    let dev = daemon.settings.get().office.dev_email.trim().to_lowercase();
    if dev.is_empty() {
        daemon.store.kv_get(EMAIL_KEY).ok().flatten()
    } else {
        Some(dev)
    }
}

pub async fn sign_in(daemon: &Daemon) -> Result<IdentityState> {
    // OAuth settings retain their old TOML location for compatibility.
    let settings = daemon.settings.get().office.clone();
    let session = if !settings.dev_email.trim().is_empty() {
        google::Session {
            email: settings.dev_email.trim().to_lowercase(),
            team_domain: None,
            refresh_token: None,
        }
    } else {
        let client = google::Client::from_settings(&settings).ok_or_else(|| Error::Conflict(
            "Set office.google_client_id and office.google_client_secret in settings.toml to sign in to Chauffeur".into()
        ))?;
        client.sign_in(|url| open_browser(&url)).await?
    };
    if let Some(token) = &session.refresh_token {
        Secrets::for_paths(&daemon.paths).store(&REFRESH_TOKEN, token)?;
    }
    daemon.store.kv_set(
        TEAM_DOMAIN_KEY,
        session.team_domain.as_deref().unwrap_or(""),
    )?;
    daemon.store.kv_set(EMAIL_KEY, &session.email)?;
    changed(daemon);
    state(daemon)
}

pub fn sign_out(daemon: &Daemon) -> Result<IdentityState> {
    Secrets::for_paths(&daemon.paths).delete(&REFRESH_TOKEN)?;
    daemon.store.kv_delete(EMAIL_KEY)?;
    daemon.store.kv_delete(TEAM_DOMAIN_KEY)?;
    changed(daemon);
    state(daemon)
}

pub fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let program = "/usr/bin/open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    let mut command = std::process::Command::new(program);
    // Started by systemd at login, the daemon may lack the display: xdg-open would then look for a
    // text browser instead of the user's.
    #[cfg(not(target_os = "macos"))]
    command.envs(crate::open::desktop_session(&std::env::vars().collect()));
    command.arg(url).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use chauffeur_protocol::events::Event;

    fn daemon(settings: &str) -> (tempfile::TempDir, Arc<Daemon>) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(
            dir.path().join("home"),
            dir.path().join("workspaces"),
            dir.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            format!("github_org = \"\"\nlinear_enabled = false\n{settings}"),
        )
        .unwrap();
        let daemon = Daemon::open(paths).unwrap();
        (dir, daemon)
    }

    #[tokio::test]
    async fn a_member_is_signed_in_with_the_member_domain_and_clients_are_told_of_each_change() {
        let (_dir, daemon) = daemon(
            "[distribution]\nmember_domain = \"acme.com\"\n[office]\ndev_email = \"alice@acme.com\"\n",
        );
        let signed_out = state(&daemon).unwrap();
        assert_eq!(
            (signed_out.email, signed_out.member, signed_out.can_sign_in),
            (None, false, true)
        );

        let mut events = daemon.events.subscribe();
        let signed_in = sign_in(&daemon).await.unwrap();
        assert_eq!(signed_in.email.as_deref(), Some("alice@acme.com"));
        assert!(signed_in.member);
        let told = events.recv().await.unwrap();
        assert!(matches!(
            &told.event,
            Event::Registry { event: RegistryEvent::IdentityChanged { identity } } if identity.member
        ));

        sign_out(&daemon).unwrap();
        let told = events.recv().await.unwrap();
        assert!(matches!(
            &told.event,
            Event::Registry { event: RegistryEvent::IdentityChanged { identity } }
                if identity.email.is_none() && !identity.member
        ));
    }

    #[tokio::test]
    async fn another_domain_is_no_member_and_a_personal_account_has_no_team_domain() {
        let (_dir, daemon) = daemon(
            "[distribution]\nmember_domain = \"acme.com\"\n[office]\ndev_email = \"bob@gmail.com\"\n",
        );
        let identity = sign_in(&daemon).await.unwrap();
        assert_eq!((identity.member, identity.team_domain), (false, None));

        daemon.store.kv_set(TEAM_DOMAIN_KEY, "globex.com").unwrap();
        assert_eq!(
            state(&daemon).unwrap().team_domain.as_deref(),
            Some("globex.com")
        );
        sign_out(&daemon).unwrap();
        assert_eq!(daemon.store.kv_get(TEAM_DOMAIN_KEY).unwrap(), None);
    }

    #[test]
    fn without_a_member_domain_nobody_is_a_member_and_without_a_client_nobody_can_sign_in() {
        let (_dir, daemon) = daemon("");
        daemon.store.kv_set(EMAIL_KEY, "alice@acme.com").unwrap();
        let identity = state(&daemon).unwrap();
        assert!(!identity.member && !identity.can_sign_in);
    }
}
