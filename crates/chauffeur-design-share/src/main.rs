//! `chauffeur-design-share`: the share service (docs/SPEC.md, stories 51 and 53).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};
use chauffeur_design_share::mail::Mailer;
use chauffeur_design_share::ses::Ses;
use chauffeur_design_share::{Auth, Config, Service, data_paths, google, store::Store};
use clap::Parser;

#[derive(Parser)]
#[command(
    about = "The share service: a design chat's canvas for named reviewers, a workspace's chats for named guests, live"
)]
struct Args {
    /// Where to listen; TLS is a reverse proxy's in front of it.
    #[arg(long, default_value = "127.0.0.1:8090")]
    listen: SocketAddr,
    /// Where shares, their comments, chats and notes (SQLite) and designs' files live.
    #[arg(
        long,
        env = "SHARE_DATA_DIR",
        default_value = "/var/lib/chauffeur-design-share"
    )]
    data: PathBuf,
    /// The reviewer page's build (`index.html`, `assets/`); a placeholder page without it.
    #[arg(long, env = "SHARE_WEB_DIR")]
    web: Option<PathBuf>,
    /// Where reviewers open design shares, as the links are written (`https://design.example.com`).
    #[arg(long, env = "SHARE_PUBLIC_URL")]
    public_url: String,
    /// Where guests open workspace shares, as the links are written (`https://share.example.com`);
    /// none: the public URL.
    #[arg(long, env = "SHARE_WORKSPACE_PUBLIC_URL")]
    workspace_public_url: Option<String>,
    /// The parent domain of the public URLs' hosts (`example.com`): signing in on one host signs
    /// in on the others. None: each host has its own sessions.
    #[arg(long, env = "SHARE_COOKIE_DOMAIN")]
    cookie_domain: Option<String>,
    /// The members' domain (`example.com`): owners of its addresses have no limit on invitations;
    /// the others send 50 a day and name 20 people a share. Its accounts also get the team
    /// shares made before a share named its team domain. Empty: no members.
    #[arg(long, alias = "domain", env = "SHARE_DOMAIN", default_value = "")]
    member_domain: String,
    /// The site "Shared with Chauffeur" links to, in e-mails and on the page
    /// (`https://chauffeur.example.com`); none leaves the words without a link.
    #[arg(long, env = "SHARE_SITE_URL")]
    site_url: Option<String>,
    /// The key sessions are signed with, 64 hex digits (`openssl rand -hex 32`), kept by the
    /// operator so that a restart signs no one out; without it, each start draws one.
    #[arg(long, env = "SHARE_SESSION_KEY", hide_env_values = true)]
    session_key: Option<String>,
    /// The OAuth client ID the daemons sign in with (the Office's); ID tokens issued for it.
    #[arg(long, env = "SHARE_GOOGLE_DAEMON_CLIENT_ID")]
    google_daemon_client_id: Option<String>,
    /// The web OAuth client ID the reviewer page signs in with; ID tokens issued for it.
    #[arg(long, env = "SHARE_GOOGLE_WEB_CLIENT_ID")]
    google_web_client_id: Option<String>,
    /// The IAM role e-mails are sent through SES with (`deploy/aws-mail.sh`); none sends no
    /// e-mail.
    #[arg(long, env = "SHARE_MAIL_ROLE_ARN")]
    mail_role_arn: Option<String>,
    /// The address e-mails come from, of the domain verified in SES (`chauffeur@example.com`);
    /// none sends no e-mail.
    #[arg(long, env = "SHARE_MAIL_FROM")]
    mail_from: Option<String>,
    /// The name e-mails come from (`Chauffeur by Example`); an invitation names the owner.
    #[arg(long, env = "SHARE_MAIL_FROM_NAME", default_value = "Chauffeur")]
    mail_from_name: String,
    /// Trust `dev:<email>` as a token, without Google: development only, refused off loopback.
    #[arg(long)]
    insecure_dev: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let auth = match (
        &args.google_daemon_client_id,
        &args.google_web_client_id,
        args.insecure_dev,
    ) {
        (_, _, true) if !args.listen.ip().is_loopback() => {
            bail!("--insecure-dev trusts anyone who connects: listen on 127.0.0.1 with it")
        }
        (_, _, true) => Auth::InsecureDev,
        (Some(daemon), Some(web), false) => {
            Auth::Google(google::Verifier::new(vec![daemon.clone(), web.clone()]))
        }
        _ => bail!(
            "Pass --google-daemon-client-id and --google-web-client-id (or SHARE_GOOGLE_DAEMON_CLIENT_ID and SHARE_GOOGLE_WEB_CLIENT_ID), or --insecure-dev on loopback"
        ),
    };
    std::fs::create_dir_all(&args.data)
        .with_context(|| format!("creating {}", args.data.display()))?;
    let (db, files) = data_paths(&args.data);
    let store = Store::open(&db).with_context(|| format!("opening {}", db.display()))?;
    let member_domain = Some(args.member_domain.trim().to_lowercase()).filter(|d| !d.is_empty());
    if let Some(domain) = &member_domain {
        let adopted = store
            .adopt_team_shares(domain)
            .context("giving earlier team shares the members' domain")?;
        if adopted > 0 {
            tracing::info!(
                adopted,
                domain,
                "earlier team shares opened to the members' domain"
            );
        }
    }
    let session_key = match args.session_key.as_deref().map(str::trim) {
        None | Some("") => {
            tracing::warn!("no --session-key: a restart signs every reader out");
            None
        }
        Some(key) => Some(
            hex::decode(key)
                .ok()
                .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                .context("--session-key is 64 hex digits (openssl rand -hex 32)")?,
        ),
    };
    let service = Service::new(
        store,
        files,
        auth,
        Config {
            public_url: args.public_url.trim_end_matches('/').to_string(),
            workspace_url: args
                .workspace_public_url
                .map(|url| url.trim().trim_end_matches('/').to_string())
                .filter(|url| !url.is_empty()),
            cookie_domain: args.cookie_domain.filter(|d| !d.trim().is_empty()),
            member_domain,
            site_url: args
                .site_url
                .map(|url| url.trim().trim_end_matches('/').to_string())
                .filter(|url| !url.is_empty()),
            session_key,
            web_client_id: args.google_web_client_id,
            web_dir: args.web,
            secure_cookies: !args.insecure_dev,
            mailer: match (
                args.mail_role_arn.filter(|arn| !arn.is_empty()),
                args.mail_from.filter(|from| !from.is_empty()),
            ) {
                (Some(arn), Some(from)) => {
                    Mailer::Ses(Arc::new(Ses::new(arn, &args.mail_from_name, &from)))
                }
                _ => Mailer::Off,
            },
        },
    );
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("listening on {}", args.listen))?;
    tracing::info!(address = %listener.local_addr()?, data = %args.data.display(), "design share service listening");
    axum::serve(listener, service.router()).await?;
    Ok(())
}
