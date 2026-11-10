//! Smoke-check built native engines against the real Core broker, without a user daemon.
use anyhow::{Context, ensure};
use chauffeur_extension_api::{HOST_API_VERSION, Init};
use chauffeur_protocol::extensions::ExtensionId;
use chauffeurd::{
    Daemon,
    extensions::{Registration, Runtime, process::Process, services::Broker},
    paths::Paths,
};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::{any::Any, collections::BTreeMap, path::Path, sync::Arc};

struct MetadataOnly;

impl Runtime for MetadataOnly {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn dispatch<'a>(
        &'a self,
        _: &'a Arc<Daemon>,
        _: chauffeur_protocol::client::ClientCall,
    ) -> BoxFuture<'a, chauffeurd::Result<Value>> {
        Box::pin(async {
            Err(chauffeurd::Error::Unavailable(
                "This fixture routes directly to its native process".into(),
            ))
        })
    }
}

fn registration(id: ExtensionId) -> Registration {
    Registration {
        id,
        name: "Native fixture",
        description: "Native broker smoke test",
        version: "fixture",
        capabilities: &[],
        build: Arc::new(|_, _, _| Arc::new(MetadataOnly)),
    }
}

async fn start(
    daemon: &Arc<Daemon>,
    id: ExtensionId,
    slug: &str,
    executable: &Path,
    version: &str,
) -> anyhow::Result<Arc<Process>> {
    let broker = Broker::open(
        Arc::downgrade(daemon),
        id,
        &daemon.paths.home.join("extensions").join(slug),
    )?;
    let process = Process::start(
        executable,
        Init {
            api: HOST_API_VERSION,
            extension: slug.into(),
            version: version.into(),
        },
        &BTreeMap::new(),
        broker,
    )
    .await?;
    process.ping().await?;
    Ok(process)
}

async fn database(daemon: &Arc<Daemon>, executable: &Path, version: &str) -> anyhow::Result<()> {
    let engine = start(
        daemon,
        ExtensionId::Database,
        "database",
        executable,
        version,
    )
    .await?;
    let empty = engine
        .call("database.list".into(), json!({"workspace": null}))
        .await?;
    ensure!(empty == json!([]), "a fresh Database starts empty");
    let saved = engine
        .call(
            "database.save".into(),
            json!({
                "id": null, "name": "Native fixture", "workspace": null,
                "host": "127.0.0.1", "port": 9, "database": "fixture",
                "username": "fixture", "environment": "local", "tls": "disable",
                "password": "fixture-password", "read_only": true,
            }),
        )
        .await?;
    ensure!(
        saved["has_password"] == true,
        "the Core credential was saved"
    );
    ensure!(
        saved.get("password").is_none(),
        "credentials stay write-only"
    );
    engine.stop();
    engine.drain_callbacks().await;
    let restarted = start(
        daemon,
        ExtensionId::Database,
        "database",
        executable,
        version,
    )
    .await?;
    let connections = restarted
        .call("database.list".into(), json!({"workspace": null}))
        .await?;
    ensure!(
        connections == json!([saved]),
        "connections survive replacement of the native process"
    );
    restarted.stop();
    restarted.drain_callbacks().await;
    Ok(())
}

async fn office(daemon: &Arc<Daemon>, executable: &Path, version: &str) -> anyhow::Result<()> {
    daemon.store.kv_set("office_skin", "fixture-skin")?;
    let engine = start(daemon, ExtensionId::Office, "office", executable, version).await?;
    let initial = engine.call("office.state".into(), json!({})).await?;
    ensure!(
        initial["configured"] == true,
        "Office reads its Core settings"
    );
    ensure!(
        initial["skin"] == "fixture-skin",
        "Office preserves existing preferences"
    );
    let changed = engine
        .call("office.set_media".into(), json!({"enabled": true}))
        .await?;
    ensure!(
        changed["media"] == true,
        "Office updates its preference through Core"
    );
    ensure!(daemon.store.kv_get("office_media")?.as_deref() == Some("on"));
    engine.stop();
    engine.drain_callbacks().await;
    let restarted = start(daemon, ExtensionId::Office, "office", executable, version).await?;
    let state = restarted.call("office.state".into(), json!({})).await?;
    ensure!(
        state["media"] == true && state["skin"] == "fixture-skin",
        "Office preferences survive replacement of the native process"
    );
    restarted.stop();
    restarted.drain_callbacks().await;
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    ensure!(
        args.len() == 3 || args.len() == 4,
        "usage: extension-host <database executable> <office executable> <database version> [office version]"
    );
    let database_path = std::fs::canonicalize(&args[0]).context("Database executable")?;
    let office_path = std::fs::canonicalize(&args[1]).context("Office executable")?;
    let database_version = args[2].to_str().context("Database version must be UTF-8")?;
    let office_version = args
        .get(3)
        .unwrap_or(&args[2])
        .to_str()
        .context("Office version must be UTF-8")?;
    let directory = tempfile::tempdir()?;
    let paths = Paths::new(
        directory.path().join("home"),
        directory.path().join("workspaces"),
        directory.path().join("repos"),
    );
    paths.ensure()?;
    std::fs::write(
        &paths.settings,
        "github_org = \"\"\nclone_roots = []\nlinear_enabled = false\n[office]\nurl = \"ws://127.0.0.1:9/ws\"\ndev_email = \"fixture@example.invalid\"\n",
    )?;
    let daemon = Daemon::open_with_extensions(
        paths,
        vec![
            registration(ExtensionId::Database),
            registration(ExtensionId::Office),
        ],
    )?;
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        database(&daemon, &database_path, database_version).await?;
        office(&daemon, &office_path, office_version).await?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("native engine smoke test timed out")??;
    eprintln!("Database and Office: native broker, heartbeat and restart persistence passed");
    Ok(())
}
