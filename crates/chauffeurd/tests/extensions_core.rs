//! The Core is built and usable without compiling either bundled implementation.

use chauffeur_protocol::client::{ClientCall, EmptyParams};
use chauffeur_protocol::extensions::{ExtensionId, ExtensionSetParams};
use chauffeurd::{Daemon, Error, paths::Paths, rpc::handle};

#[tokio::test]
async fn a_core_only_build_has_no_extensions_but_keeps_identity_and_workspace_listing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("settings.toml"), "github_org = \"\"\ngithub_sources = false\nclone_roots = []\nlinear_enabled = false\n[office]\nurl = \"\"\ndev_email = \"fixture@example.com\"\n").unwrap();
    let daemon = Daemon::open(Paths::new(
        home,
        dir.path().join("workspaces"),
        dir.path().join("repos"),
    ))
    .unwrap();
    assert_eq!(
        handle(&daemon, ClientCall::ExtensionsList(EmptyParams {}))
            .await
            .unwrap(),
        serde_json::json!([])
    );
    let account = handle(&daemon, ClientCall::IdentitySignIn(EmptyParams {}))
        .await
        .unwrap();
    assert_eq!(account["email"], "fixture@example.com");
    assert!(
        !handle(&daemon, ClientCall::OfficeState(EmptyParams {}))
            .await
            .unwrap()["configured"]
            .as_bool()
            .unwrap()
    );
    assert!(daemon.workspaces.list(false, None).unwrap().is_empty());
    for id in [ExtensionId::Database, ExtensionId::Office] {
        assert!(matches!(
            handle(
                &daemon,
                ClientCall::ExtensionsSet(ExtensionSetParams { id, enabled: true })
            )
            .await,
            Err(Error::Unavailable(_))
        ));
    }
}
