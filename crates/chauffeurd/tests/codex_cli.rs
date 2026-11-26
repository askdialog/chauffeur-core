//! Updating the Codex CLI from Chauffeur (`codex.status`, `codex.update`), against a fake `codex`
//! whose `update` changes the version it reports, and a fake npm registry.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chauffeur_protocol::client::{ClientCall, CodexCliStatus};
use chauffeurd::{Daemon, paths::Paths, rpc};
use serde_json::{Value, json};

const FAKE_CODEX: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "codex-cli $(cat "$FAKE_CODEX_DIR/version")" ;;
  update)
    [ -f "$FAKE_CODEX_DIR/slow" ] && sleep 1
    if [ -f "$FAKE_CODEX_DIR/fail" ]; then
      echo "Updating Codex via \`npm install -g @openai/codex\`..."
      echo "npm ERR! code EACCES" >&2
      exit 1
    fi
    [ "$CODEX_NON_INTERACTIVE" = 1 ] || exit 3
    echo "0.172.0" > "$FAKE_CODEX_DIR/version" ;;
  *) exit 2 ;;
esac
"#;

struct Fixture {
    _root: tempfile::TempDir,
    fake: PathBuf,
    daemon: Arc<Daemon>,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let bin = root.path().join("bin");
        let fake = root.path().join("fake");
        for dir in [&home, &bin, &fake] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(home.join("settings.toml"), "linear_enabled = false\n").unwrap();
        let codex = bin.join("codex");
        std::fs::write(&codex, FAKE_CODEX).unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(fake.join("version"), "0.167.0\n").unwrap();
        let daemon = Daemon::open(Paths::new(
            home,
            root.path().join("workspaces"),
            root.path().join("repos"),
        ))
        .unwrap();
        daemon.shell_env.set_vars(BTreeMap::from([
            ("PATH".into(), format!("{}:/usr/bin:/bin", bin.display())),
            ("FAKE_CODEX_DIR".into(), fake.display().to_string()),
        ]));
        daemon.codex_cli.set_latest_url(&npm_registry().await);
        Self {
            _root: root,
            fake,
            daemon,
        }
    }

    async fn call(&self, method: &str) -> chauffeurd::Result<CodexCliStatus> {
        let call: ClientCall =
            serde_json::from_value(json!({ "method": method, "params": {} })).unwrap();
        let result = rpc::handle(&self.daemon, call).await?;
        Ok(serde_json::from_value(result).unwrap())
    }

    /// The status once the update under way ended.
    async fn after_update(&self) -> CodexCliStatus {
        for _ in 0..100 {
            let status = self.call("codex.status").await.unwrap();
            if !status.updating {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the update never ended");
    }
}

/// Answers npm's `@openai/codex/latest` with 0.172.0.
async fn npm_registry() -> String {
    async fn latest() -> axum::Json<Value> {
        axum::Json(json!({ "name": "@openai/codex", "version": "0.172.0" }))
    }
    let app = axum::Router::new().route("/@openai/codex/latest", axum::routing::get(latest));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}/@openai/codex/latest")
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_is_updated_from_chauffeur_to_the_latest_version() {
    let f = Fixture::new().await;
    let status = f.call("codex.status").await.unwrap();
    assert!(
        status
            .path
            .as_deref()
            .is_some_and(|path| path.ends_with("/bin/codex"))
    );
    assert_eq!(status.version.as_deref(), Some("0.167.0"));
    assert_eq!(status.latest.as_deref(), Some("0.172.0"));
    assert!(!status.updating);

    std::fs::write(f.fake.join("slow"), "").unwrap();
    let started = f.call("codex.update").await.unwrap();
    assert!(started.updating);
    // One update at a time.
    let again = f.call("codex.update").await;
    assert!(
        matches!(again, Err(chauffeurd::Error::Conflict(_))),
        "{again:?}"
    );

    let done = f.after_update().await;
    assert_eq!(done.version.as_deref(), Some("0.172.0"));
    assert_eq!(done.error, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_update_says_why_with_the_end_of_its_output() {
    let f = Fixture::new().await;
    std::fs::write(f.fake.join("fail"), "").unwrap();
    f.call("codex.update").await.unwrap();
    let failed = f.after_update().await;
    assert_eq!(failed.version.as_deref(), Some("0.167.0"));
    let error = failed.error.expect("the failure is kept");
    assert!(error.contains("npm ERR! code EACCES"), "{error}");

    // Tried again once fixed: the failure goes.
    std::fs::remove_file(f.fake.join("fail")).unwrap();
    f.call("codex.update").await.unwrap();
    let done = f.after_update().await;
    assert_eq!(done.version.as_deref(), Some("0.172.0"));
    assert_eq!(done.error, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_codex_there_is_nothing_to_update() {
    let f = Fixture::new().await;
    f.daemon
        .shell_env
        .set_vars(BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]));
    let status = f.call("codex.status").await.unwrap();
    assert_eq!(status.path, None);
    assert_eq!(status.version, None);
    let refused = f.call("codex.update").await;
    assert!(
        matches!(refused, Err(chauffeurd::Error::BadRequest(_))),
        "{refused:?}"
    );
}
