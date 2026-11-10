//! The macOS app menu, whose items show and catch the keys the UI acts on: the user's bindings
//! (`[keyboard.bindings]`, sent by the UI) over the defaults, and none while Settings › Keyboard
//! records keys. A macOS menu takes its keys before the web view, so keys it kept would still act
//! after a rebinding, and would be taken from the recorder. Linux has no menu: the UI's own
//! shortcuts cover it.
//!
//! The menu is built again when its keys change: muda cannot take an item's accelerator away
//! (`set_accelerator(None)` leaves the old one on macOS). Its predefined items (Quit, Hide,
//! Minimize, the Edit menu, Full Screen) keep their keys, which no binding may take.
//!
//! While the Terminal view is the one in use, the File menu's tab items are the terminals' (New
//! Terminal, Close Terminal, on the keys of `terminal.new` and `terminal.close`), and Reopen Closed
//! Chat, which still reopens a chat, gives up its keys: the menu says what its keys do.
//!
//! The Office and Connect Phone… items are a member's only (docs/adr/members-only-features.md):
//! outside members the menu has no trace of them.

#[cfg(any(target_os = "macos", test))]
use std::collections::BTreeMap;

#[cfg(any(target_os = "macos", test))]
use chauffeur_protocol::client::ShortcutId;

/// The menu's items that carry keys: their id, the shortcut they are (none: keys that cannot
/// change) and their default keys, written the macOS way as in `app/src/lib/shortcuts.ts`.
#[cfg(any(target_os = "macos", test))]
const KEYED_ITEMS: &[(&str, Option<ShortcutId>, &str)] = &[
    ("new-workspace", Some(ShortcutId::WorkspaceNew), "⌘N"),
    ("new-exploration", Some(ShortcutId::ExplorationNew), "⇧⌘N"),
    ("switch-workspace", Some(ShortcutId::SwitcherOpen), "⌘K"),
    ("new-chat", Some(ShortcutId::ChatNew), "⌘T"),
    // ⌘W closes the chat shown, as in a browser's tabs; the window closes with ⇧⌘W.
    ("close-chat", Some(ShortcutId::ChatClose), "⌘W"),
    ("reopen-chat", Some(ShortcutId::ChatReopen), "⇧⌘T"),
    ("new-terminal", Some(ShortcutId::TerminalNew), "⌘T"),
    ("close-terminal", Some(ShortcutId::TerminalClose), "⌘W"),
    ("close-window", None, "⇧⌘W"),
    ("settings", Some(ShortcutId::SettingsOpen), "⌘,"),
    ("toggle-sidebar", Some(ShortcutId::SidebarToggle), "⌘B"),
    ("zoom-reset", Some(ShortcutId::ZoomReset), "⌘0"),
    ("zoom-in", Some(ShortcutId::ZoomIn), "⌘="),
    ("zoom-out", Some(ShortcutId::ZoomOut), "⌘-"),
    ("office", Some(ShortcutId::OfficeShow), "⇧⌘O"),
];

/// The accelerator Tauri reads for a binding (`⇧⌘T` → `Shift+Cmd+T`); None for a key a menu
/// accelerator cannot name, a character typed with Shift such as `+`: the menu item then shows no
/// keys and the web view gets them.
#[cfg(any(target_os = "macos", test))]
fn accelerator(keys: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut rest = keys;
    while let Some(symbol) = rest.chars().next() {
        parts.push(match symbol {
            '⌃' => "Ctrl",
            '⌥' => "Alt",
            '⇧' => "Shift",
            '⌘' => "Cmd",
            _ => break,
        });
        rest = &rest[symbol.len_utf8()..];
    }
    let key = match rest {
        "Tab" | "Home" | "End" | "Space" => rest.to_string(),
        "PgUp" => "PageUp".into(),
        "PgDn" => "PageDown".into(),
        "Del" => "Delete".into(),
        "↑" => "Up".into(),
        "↓" => "Down".into(),
        "←" => "Left".into(),
        "→" => "Right".into(),
        "↵" => "Enter".into(),
        _ if (1..=12).any(|n| rest == format!("F{n}")) => rest.to_string(),
        _ => {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase().to_string(),
                (Some(c), None) if "`\\[],=-.';/".contains(c) => c.to_string(),
                _ => return None,
            }
        }
    };
    (!parts.is_empty()).then(|| format!("{}+{key}", parts.join("+")))
}

/// The File menu's tab items: the chats' (New Chat, Close Chat), or the terminals' while the
/// Terminal view is the one in use.
#[cfg(any(target_os = "macos", test))]
fn tab_items(terminal: bool) -> [(&'static str, &'static str); 2] {
    if terminal {
        [
            ("new-terminal", "New Terminal"),
            ("close-terminal", "Close Terminal"),
        ]
    } else {
        [("new-chat", "New Chat"), ("close-chat", "Close Chat")]
    }
}

/// The accelerator of each keyed item: the user's keys over its default; none while recording. In
/// the Terminal view (`terminal`) Reopen Closed Chat has none: its keys would reopen a chat there.
#[cfg(any(target_os = "macos", test))]
fn item_keys(
    bindings: &BTreeMap<ShortcutId, String>,
    recording: bool,
    terminal: bool,
) -> Vec<(&'static str, Option<String>)> {
    KEYED_ITEMS
        .iter()
        .map(|(item, id, default)| {
            let keys = id
                .and_then(|id| bindings.get(&id))
                .map_or(*default, String::as_str);
            let shown = !recording && !(terminal && *item == "reopen-chat");
            (*item, accelerator(keys).filter(|_| shown))
        })
        .collect()
}

/// What the menu is built with: the user's `bindings` over the defaults, none while Settings ›
/// Keyboard is `recording` keys, the terminals' tab items while the Terminal view is the one in use
/// (`terminal`), the Office and Connect Phone… items for a `member`.
#[cfg(target_os = "macos")]
#[derive(Default, PartialEq)]
pub struct MenuState {
    pub bindings: BTreeMap<ShortcutId, String>,
    pub recording: bool,
    pub terminal: bool,
    pub member: bool,
}

/// What the menu was last built with.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub struct MenuKeys(std::sync::Mutex<MenuState>);

/// Builds the menu again when what it is built with changed (see `MenuState`).
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn set_menu_keys(
    app: tauri::AppHandle,
    shown: tauri::State<'_, MenuKeys>,
    bindings: BTreeMap<ShortcutId, String>,
    recording: bool,
    terminal: bool,
    member: bool,
) -> Result<(), String> {
    let mut shown = shown.0.lock().unwrap();
    let wanted = MenuState {
        bindings,
        recording,
        terminal,
        member,
    };
    if *shown == wanted {
        return Ok(());
    }
    let menu = build(&app, &wanted).map_err(|error| error.to_string())?;
    app.set_menu(menu).map_err(|error| error.to_string())?;
    *shown = wanted;
    Ok(())
}

/// The app menu, its items on the keys `item_keys` gives them, its tab items `tab_items`'.
#[cfg(target_os = "macos")]
pub fn build(
    app: &tauri::AppHandle,
    state: &MenuState,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{AboutMetadata, Menu, MenuItemBuilder, SubmenuBuilder};

    let MenuState {
        bindings,
        recording,
        terminal,
        member,
    } = state;
    let (recording, terminal, member) = (*recording, *terminal, *member);
    let accelerators = item_keys(bindings, recording, terminal);
    let item = |id: &'static str, label: &str| {
        let builder = MenuItemBuilder::with_id(id, label);
        let keys = accelerators
            .iter()
            .find(|(item, _)| *item == id)
            .and_then(|(_, keys)| keys.as_deref());
        match keys {
            Some(keys) => builder.accelerator(keys),
            None => builder,
        }
        .build(app)
    };
    let new_workspace = item("new-workspace", "New Workspace")?;
    let new_exploration = item("new-exploration", "New Exploration")?;
    let switch_workspace = item("switch-workspace", "Switch Workspace…")?;
    let [(new_id, new_label), (close_id, close_label)] = tab_items(terminal);
    let new_tab = item(new_id, new_label)?;
    let close_tab = item(close_id, close_label)?;
    let reopen_chat = item("reopen-chat", "Reopen Closed Chat")?;
    let close_window = item("close-window", "Close Window")?;
    let settings = item("settings", "Settings…")?;
    let mobile = MenuItemBuilder::with_id("mobile-connection", "Connect Phone…").build(app)?;
    let mut app_menu = SubmenuBuilder::new(app, "Chauffeur")
        .about(Some(AboutMetadata {
            name: Some("Chauffeur".into()),
            version: Some(app.package_info().version.to_string()),
            copyright: Some("By Dialog".into()),
            credits: Some(
                "A workspace for your coding agents.\n\n\
                 Run Claude Code and Codex side by side,\n\
                 across repos, in isolated workspaces.\n\n\
                 Designed and built at Dialog."
                    .into(),
            ),
            ..Default::default()
        }))
        .separator()
        .item(&settings);
    if member {
        app_menu = app_menu.item(&mobile);
    }
    let app_menu = app_menu
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&new_workspace)
        .item(&new_exploration)
        .item(&switch_workspace)
        .separator()
        .item(&new_tab)
        .item(&close_tab)
        .item(&reopen_chat)
        .separator()
        .item(&close_window)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let toggle_sidebar = item("toggle-sidebar", "Toggle Sidebar")?;
    let zoom_reset = item("zoom-reset", "Actual Size")?;
    let zoom_in = item("zoom-in", "Zoom In")?;
    let zoom_out = item("zoom-out", "Zoom Out")?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&toggle_sidebar)
        .separator()
        .item(&zoom_reset)
        .item(&zoom_in)
        .item(&zoom_out)
        .separator()
        .fullscreen()
        .build()?;
    let office = item("office", "Office")?;
    let mut window = SubmenuBuilder::new(app, "Window").minimize().maximize();
    if member {
        window = window.separator().item(&office);
    }
    let window = window.build()?;
    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_key_is_an_accelerator_muda_reads() {
        for (item, keys) in item_keys(&BTreeMap::new(), false, false) {
            let keys = keys.unwrap_or_else(|| panic!("{item} has no default accelerator"));
            assert!(
                keys.parse::<muda::accelerator::Accelerator>().is_ok(),
                "{item}: {keys}"
            );
        }
        for keys in [
            "⌘T", "⇧⌘t", "⌃⌘1", "⌘\\", "⇧⌘/", "⌘↑", "⌃Tab", "⌘PgDn", "⌘F5", "⌘,", "⌘↵", "⌘Del",
            "⌘Space", "⌘⇧[", "⌥⌘1", "⌃⌥T",
        ] {
            let accelerator = accelerator(keys).unwrap_or_else(|| panic!("{keys}"));
            assert!(
                accelerator
                    .parse::<muda::accelerator::Accelerator>()
                    .is_ok(),
                "{keys}: {accelerator}"
            );
        }
        assert_eq!(accelerator("⇧⌘J").as_deref(), Some("Shift+Cmd+J"));
        assert_eq!(accelerator("⌃⌘K").as_deref(), Some("Ctrl+Cmd+K"));
        assert_eq!(accelerator("⌥⌘1").as_deref(), Some("Alt+Cmd+1"));
    }

    #[test]
    fn the_menu_shows_the_users_keys_and_none_while_recording() {
        let bindings = BTreeMap::from([
            (ShortcutId::SidebarToggle, "⌘J".to_string()),
            (ShortcutId::ChatNew, "⌘+".to_string()),
        ]);
        let keys = item_keys(&bindings, false, false);
        let of = |id: &str| keys.iter().find(|(item, _)| *item == id).unwrap().1.clone();
        assert_eq!(of("toggle-sidebar").as_deref(), Some("Cmd+J"));
        // `+` has no accelerator: the item shows no keys, and the web view gets ⌘+.
        assert_eq!(of("new-chat"), None);
        assert_eq!(of("switch-workspace").as_deref(), Some("Cmd+K"));
        assert_eq!(of("close-window").as_deref(), Some("Shift+Cmd+W"));
        assert!(
            item_keys(&bindings, true, false)
                .iter()
                .all(|(_, keys)| keys.is_none())
        );
    }

    #[test]
    fn in_the_terminal_view_the_menu_opens_and_closes_terminals_and_leaves_reopen_without_keys() {
        assert_eq!(
            tab_items(true),
            [
                ("new-terminal", "New Terminal"),
                ("close-terminal", "Close Terminal")
            ]
        );
        assert_eq!(
            tab_items(false),
            [("new-chat", "New Chat"), ("close-chat", "Close Chat")]
        );
        let bindings = BTreeMap::from([(ShortcutId::TerminalClose, "⇧⌘K".to_string())]);
        let keys = item_keys(&bindings, false, true);
        let of = |id: &str| keys.iter().find(|(item, _)| *item == id).unwrap().1.clone();
        assert_eq!(of("new-terminal").as_deref(), Some("Cmd+T"));
        assert_eq!(of("close-terminal").as_deref(), Some("Shift+Cmd+K"));
        assert_eq!(of("reopen-chat"), None);
        let chat_view = item_keys(&bindings, false, false);
        assert!(chat_view.contains(&("reopen-chat", Some("Shift+Cmd+T".to_string()))));
    }
}
