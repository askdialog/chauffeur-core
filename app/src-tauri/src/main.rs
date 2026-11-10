//! The Chauffeur desktop app: a Tauri shell around the React UI.
//!
//! The UI is a client of `chauffeurd` over WebSocket, like the CLI. Tauri only covers native
//! concerns: finding or starting the daemon, the app menu, notifications, window state and, on
//! Linux, the tray icon and files dropped from the desktop.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bundle;
mod daemon;
mod drops;
mod menu;
mod screen_snapshot;
mod slack;
#[cfg(target_os = "linux")]
mod tray;
#[cfg(target_os = "linux")]
mod webkit_media;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{Emitter, Manager, State};

/// Serializes daemon starts: reconnect attempts must not spawn several daemons.
#[derive(Default, Clone)]
struct DaemonLock(Arc<Mutex<()>>);

/// `{url, token}` of the running daemon, which is started if needed.
#[tauri::command]
async fn daemon_connection(lock: State<'_, DaemonLock>) -> Result<daemon::Connection, String> {
    let lock = lock.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock.0.lock().unwrap_or_else(|e| e.into_inner());
        daemon::ensure_daemon(&daemon::chauffeur_home(), Duration::from_secs(20))
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("{error:#}"))
}

/// Quits and reopens a packaged app, to run the version an update put in its place.
#[tauri::command]
fn relaunch_app(app: tauri::AppHandle) -> Result<(), String> {
    let bundle = bundle::bundled_bin_dir()
        .and_then(|bin| Some(bin.parent()?.parent()?.to_path_buf()))
        .ok_or("only a packaged Chauffeur.app reopens itself; quit and start it again")?;
    bundle::reopen_after_exit(&bundle)
        .map_err(|error| format!("could not reopen {}: {error}", bundle.display()))?;
    app.exit(0);
    Ok(())
}

/// Opens an http(s) URL (a ticket, a PR) in the default browser.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("refusing to open {url}"));
    }
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    std::process::Command::new(opener)
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The phone host stays on Mac loopback. Its window has no Tauri capabilities,
/// and cannot navigate to a remote site or use the main window's native APIs.
fn show_mobile_connection(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("mobile-connection") {
        window.show().map_err(|e| e.to_string())?;
        return window.set_focus().map_err(|e| e.to_string());
    }
    let port = std::env::var("CHAUFFEUR_MOBILE_HOST_PORT")
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|_| "Invalid mobile host port".to_string())
        })
        .unwrap_or(Ok(41241))?;
    if port == 0 {
        return Err("Invalid mobile host port".into());
    }
    let url = format!("http://127.0.0.1:{port}/")
        .parse()
        .map_err(|e| format!("{e}"))?;
    tauri::WebviewWindowBuilder::new(app, "mobile-connection", tauri::WebviewUrl::External(url))
        .title("Chauffeur · Mobile")
        .inner_size(840.0, 900.0)
        // The same lower limit as the Settings dialog (720 x 600).
        .min_inner_size(720.0, 600.0)
        .on_navigation(move |url| mobile_navigation_allowed(url, port))
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn mobile_navigation_allowed(url: &tauri::Url, port: u16) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(port)
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

#[cfg(test)]
mod mobile_window_tests {
    use super::mobile_navigation_allowed;

    #[test]
    fn pairing_window_stays_on_its_exact_loopback_page() {
        assert!(mobile_navigation_allowed(
            &"http://127.0.0.1:41241/".parse().unwrap(),
            41241
        ));
        for address in [
            "https://example.com/",
            "http://127.0.0.1:41242/",
            "http://127.0.0.1:41241/approve",
            "http://127.0.0.1:41241/?next=evil",
            "http://user@127.0.0.1:41241/",
            "http://127.0.0.1:41241/#elsewhere",
        ] {
            assert!(
                !mobile_navigation_allowed(&address.parse().unwrap(), 41241),
                "{address}"
            );
        }
    }
}

#[tauri::command]
fn open_mobile_connection(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Only the main Chauffeur window can open mobile settings".into());
    }
    show_mobile_connection(window.app_handle())
}

/// WebKitGTK's DMA-BUF renderer shows a blank window with NVIDIA's proprietary driver, notably
/// under Wayland; fall back to its older renderer there unless the user chose.
#[cfg(target_os = "linux")]
fn work_around_webkitgtk() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
        && std::path::Path::new("/proc/driver/nvidia/version").exists()
    {
        // SAFETY: called first thing in main, before any other thread exists.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
}

fn workspace_share_link(url: &url::Url) -> Option<String> {
    if url.scheme() != "chauffeur"
        || url.host_str() != Some("import-workspace")
        || !matches!(url.path(), "" | "/")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.query_pairs()
        .find(|(key, _)| key == "url")
        .map(|(_, link)| link.into_owned())
}

#[cfg(test)]
mod workspace_link_tests {
    use super::workspace_share_link;

    #[test]
    fn a_workspace_link_opens_the_import_form_with_its_decoded_share_url() {
        let url = url::Url::parse("chauffeur://import-workspace?url=https%3A%2F%2Fshare.example.com%2Fw%2F0123456789abcdef").unwrap();
        assert_eq!(
            workspace_share_link(&url).as_deref(),
            Some("https://share.example.com/w/0123456789abcdef")
        );
        for link in [
            "https://import-workspace?url=x",
            "chauffeur://other?url=x",
            "chauffeur://import-workspace",
            "chauffeur://import-workspace/other?url=x",
        ] {
            assert!(workspace_share_link(&url::Url::parse(link).unwrap()).is_none());
        }
    }
}

#[derive(Default)]
struct SharedWorkspaceLink(std::sync::Mutex<Option<String>>);

#[tauri::command]
fn take_workspace_share_link(state: tauri::State<'_, SharedWorkspaceLink>) -> Option<String> {
    state.0.lock().unwrap().take()
}

fn main() {
    #[cfg(target_os = "linux")]
    work_around_webkitgtk();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            #[cfg(target_os = "linux")]
            for argument in args.iter().skip(1) {
                if let Ok(url) = url::Url::parse(argument)
                    && let Some(link) = workspace_share_link(&url)
                {
                    *app.state::<SharedWorkspaceLink>().0.lock().unwrap() = Some(link);
                    let _ = app.emit("workspace-share-link", ());
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = args;
            slack::focus(app)
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // An old small pairing window must not override its readable default size.
                .with_denylist(&["mobile-connection"])
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .manage(DaemonLock::default())
        .manage(SharedWorkspaceLink::default())
        .manage(drops::LastDrop::default())
        .manage(slack::PendingLinks::default())
        .on_window_event(drops::on_window_event)
        .invoke_handler(tauri::generate_handler![
            daemon_connection,
            take_workspace_share_link,
            relaunch_app,
            open_url,
            open_mobile_connection,
            slack::take_slack_links,
            slack::focus_slack_desktop,
            drops::read_dropped_file,
            screen_snapshot::screen_snapshot,
            screen_snapshot::screen_snapshots_release,
            #[cfg(target_os = "linux")]
            tray::set_attention,
            #[cfg(target_os = "macos")]
            menu::set_menu_keys
        ])
        .setup(|app| {
            #[cfg(target_os = "linux")]
            for argument in std::env::args().skip(1) {
                if let Ok(url) = url::Url::parse(&argument)
                    && let Some(link) = workspace_share_link(&url)
                {
                    *app.state::<SharedWorkspaceLink>().0.lock().unwrap() = Some(link);
                }
            }
            slack::setup(app.handle())?;
            // On Linux the menu would be a bar inside the window; the UI's shortcuts cover it.
            // The default keys, and no member's items, until the UI sends the user's (`set_menu_keys`).
            #[cfg(target_os = "macos")]
            {
                app.manage(menu::MenuKeys::default());
                app.set_menu(menu::build(app.handle(), &menu::MenuState::default())?)?;
            }
            #[cfg(target_os = "linux")]
            webkit_media::enable(app.handle());
            // macOS says what needs the user with the Dock's badge, not a menu-bar icon.
            #[cfg(target_os = "linux")]
            if let Err(error) = tray::create(app.handle()) {
                eprintln!("chauffeur: could not add the tray icon: {error}");
            }
            app.on_menu_event(|app, event| {
                let action = event.id().as_ref();
                if matches!(action, "close-chat" | "close-terminal" | "close-window")
                    && let Some(window) = app
                        .get_webview_window("mobile-connection")
                        .filter(|window| window.is_focused().unwrap_or(false))
                {
                    let _ = window.close();
                    return;
                }
                if action == "close-window" {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.close();
                    }
                    return;
                }
                if matches!(
                    action,
                    "mobile-connection"
                        | "new-workspace"
                        | "new-exploration"
                        | "switch-workspace"
                        | "settings"
                        | "toggle-sidebar"
                        | "zoom-in"
                        | "zoom-out"
                        | "zoom-reset"
                        | "new-chat"
                        | "close-chat"
                        | "reopen-chat"
                        | "new-terminal"
                        | "close-terminal"
                        | "office"
                ) {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.set_focus();
                    }
                    let _ = app.emit("menu", action);
                }
            });
            // Start the daemon early, while the UI loads.
            let lock = app.state::<DaemonLock>().inner().clone();
            std::thread::spawn(move || {
                let _guard = lock.0.lock().unwrap_or_else(|e| e.into_inner());
                // A packaged app: make its CLI, daemon and sidecar runnable and reachable.
                if let Some(bin_dir) = bundle::bundled_bin_dir() {
                    bundle::lift_quarantine(&bin_dir);
                    let home =
                        std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                    if let Err(error) = bundle::link_cli(&bin_dir, &home) {
                        eprintln!("chauffeur: could not link the CLI into ~/.local/bin: {error}");
                    }
                }
                if let Err(error) =
                    daemon::ensure_daemon(&daemon::chauffeur_home(), Duration::from_secs(20))
                {
                    eprintln!("chauffeur: {error:#}");
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Chauffeur app")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = event {
                for url in urls {
                    if let Some(link) = workspace_share_link(&url) {
                        *app.state::<SharedWorkspaceLink>().0.lock().unwrap() = Some(link);
                        let _ = app.emit("workspace-share-link", ());
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
