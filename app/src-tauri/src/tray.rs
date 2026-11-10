//! The tray icon on Linux: a StatusNotifierItem (waybar's tray, KDE, GNOME with the AppIndicator
//! extension). It says whether workspaces need the user and shows or hides the main window. It
//! does not change what closing the window does, and its Quit quits the app only: the daemon keeps
//! running, as when the window closes. Clicking the icon opens its menu: all AppIndicator offers.
//!
//! Linux only: on macOS the Dock's badge says what needs the user, and a menu-bar icon would need
//! a monochrome template image.

use std::sync::Mutex;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, WindowEvent, Wry};

/// The sidebar's attention colour (`--color-attention`).
const ATTENTION: [u8; 4] = [0xf0, 0xb5, 0x45, 0xff];

/// What the attention count updates.
struct Tray {
    icon: TrayIcon,
    menu: Menu<Wry>,
    toggle: MenuItem<Wry>,
    attention: MenuItem<Wry>,
    attention_separator: PredefinedMenuItem<Wry>,
    plain: Image<'static>,
    badged: Image<'static>,
    count: Mutex<u32>,
}

/// Puts the icon in the tray. Without AppIndicator there is no tray: the app runs without one
/// rather than failing.
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    if !appindicator_installed() {
        eprintln!(
            "chauffeur: no tray icon: install libayatana-appindicator (or libappindicator-gtk3)"
        );
        return Ok(());
    }
    let Some(plain) = app.default_window_icon().cloned().map(Image::to_owned) else {
        return Ok(());
    };
    let badged = Image::new_owned(
        badge(plain.rgba(), plain.width(), plain.height()),
        plain.width(),
        plain.height(),
    );
    let toggle = MenuItem::with_id(app, "tray-toggle", "Hide Chauffeur", true, None::<&str>)?;
    let attention = MenuItem::with_id(app, "tray-attention", "", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "tray-quit", "Quit Chauffeur", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &PredefinedMenuItem::separator(app)?, &quit])?;
    let icon = TrayIconBuilder::with_id("chauffeur")
        .icon(plain.clone())
        .tooltip("Chauffeur")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray-toggle" => toggle_window(app),
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    if let Some(window) = app.get_webview_window("main") {
        let app = app.clone();
        // Hidden or shown otherwise (the window manager), the item still says what it will do.
        window.on_window_event(move |event| {
            if matches!(event, WindowEvent::Focused(_)) {
                refresh_toggle(&app);
            }
        });
    }
    app.manage(Tray {
        icon,
        menu,
        toggle,
        attention,
        attention_separator: PredefinedMenuItem::separator(app)?,
        plain,
        badged,
        count: Mutex::new(0),
    });
    Ok(())
}

/// The UI's count of workspaces that need the user: the icon gets a dot, the menu a line, the
/// tooltip the count.
#[tauri::command]
pub fn set_attention(app: AppHandle, count: u32) -> Result<(), String> {
    let Some(tray) = app.try_state::<Tray>() else {
        return Ok(());
    };
    let mut shown = tray.count.lock().unwrap();
    if *shown == count {
        return Ok(());
    }
    let result = (|| -> tauri::Result<()> {
        match attention_text(count) {
            Some(text) => {
                tray.attention.set_text(&text)?;
                if *shown == 0 {
                    tray.menu
                        .insert_items(&[&tray.attention, &tray.attention_separator], 0)?;
                }
                tray.icon.set_icon(Some(tray.badged.clone()))?;
                tray.icon.set_tooltip(Some(format!("Chauffeur: {text}")))
            }
            None => {
                tray.menu.remove(&tray.attention)?;
                tray.menu.remove(&tray.attention_separator)?;
                tray.icon.set_icon(Some(tray.plain.clone()))?;
                tray.icon.set_tooltip(Some("Chauffeur"))
            }
        }
    })();
    *shown = count;
    result.map_err(|error| format!("could not update the tray icon: {error}"))
}

/// Shows and focuses the main window.
fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    refresh_toggle(app);
}

fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        refresh_toggle(app);
    } else {
        show_window(app);
    }
}

fn refresh_toggle(app: &AppHandle) {
    let (Some(tray), Some(window)) = (app.try_state::<Tray>(), app.get_webview_window("main"))
    else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    let _ = tray.toggle.set_text(if visible {
        "Hide Chauffeur"
    } else {
        "Show Chauffeur"
    });
}

/// The menu's line on what needs the user; none when nothing does.
fn attention_text(count: u32) -> Option<String> {
    match count {
        0 => None,
        1 => Some("1 workspace needs you".into()),
        n => Some(format!("{n} workspaces need you")),
    }
}

/// The icon with a dot in its top-right corner, a third of its width across.
fn badge(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut pixels = rgba.to_vec();
    let radius = width as f32 / 6.0;
    let (cx, cy) = (width as f32 - radius, radius);
    for y in 0..height {
        for x in 0..width {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= radius * radius {
                let i = ((y * width + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&ATTENTION);
            }
        }
    }
    pixels
}

/// Whether AppIndicator can be loaded: the tray's library panics when it cannot.
fn appindicator_installed() -> bool {
    [c"libayatana-appindicator3.so.1", c"libappindicator3.so.1"]
        .iter()
        // SAFETY: a library name without NUL; the handle stays open for the tray's library.
        .any(|name| !unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY) }.is_null())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_attention_line_counts_workspaces_and_is_gone_at_zero() {
        assert_eq!(attention_text(0), None);
        assert_eq!(attention_text(1).as_deref(), Some("1 workspace needs you"));
        assert_eq!(attention_text(3).as_deref(), Some("3 workspaces need you"));
    }

    #[test]
    fn the_badge_paints_the_top_right_corner_only() {
        let plain = vec![0u8; 32 * 32 * 4];
        let badged = badge(&plain, 32, 32);
        let pixel = |x: usize, y: usize| &badged[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(pixel(26, 5), ATTENTION);
        assert_eq!(pixel(5, 5), [0, 0, 0, 0]);
        assert_eq!(pixel(26, 26), [0, 0, 0, 0]);
    }
}
