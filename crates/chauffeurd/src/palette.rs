//! A terminal colour palette read from a file the user's own tools write (`terminal_theme` in
//! `[appearance]`): pywal JSON, kitty, Ghostty, foot or Alacritty themes, as Dracula and most theme
//! collections ship them, and as wallust's templates write them. The app colours its terminal with it, and its code from its 16
//! ANSI colours.

use std::path::Path;

use chauffeur_protocol::client::Palette;

/// Larger than any theme: a file this big is not one.
const MAX_BYTES: u64 = 256 * 1024;

/// The palette in `path` (`~/` allowed), or why it cannot be read.
pub fn read(path: &str) -> Result<Palette, String> {
    let path = expand(path);
    let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(format!("{} is not a colour theme file", path.display()));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&path, &text)
}

fn expand(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|home| Path::new(&home).join(rest))
            .unwrap_or_else(|| path.into()),
        None => path.into(),
    }
}

/// By the file's extension, else by its lines: `key = value` is Ghostty or foot, `key value` kitty.
pub fn parse(path: &Path, text: &str) -> Result<Palette, String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let palette = match extension.as_str() {
        "json" => json(text)?,
        "toml" => alacritty(text)?,
        _ if text.lines().any(|l| l.trim_start().starts_with("palette")) => lines(text, true),
        _ => {
            let ghostty = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .any(|l| l.contains('='));
            lines(text, ghostty)
        }
    };
    palette.complete()
}

/// What a theme gave, before checking it is whole.
#[derive(Default)]
struct Partial {
    background: Option<String>,
    foreground: Option<String>,
    cursor: Option<String>,
    selection: Option<String>,
    ansi: [Option<String>; 16],
}

impl Partial {
    fn complete(self) -> Result<Palette, String> {
        let missing: Vec<String> = self
            .ansi
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_none())
            .map(|(i, _)| format!("color{i}"))
            .chain(self.background.is_none().then(|| "background".into()))
            .chain(self.foreground.is_none().then(|| "foreground".into()))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "not a whole palette: {} missing (a background, a foreground and 16 colours are needed)",
                missing.join(", ")
            ));
        }
        Ok(Palette {
            background: self.background.unwrap(),
            foreground: self.foreground.unwrap(),
            cursor: self.cursor,
            selection: self.selection,
            ansi: self.ansi.into_iter().map(Option::unwrap).collect(),
        })
    }

    fn set(&mut self, key: &str, value: &str) {
        let Some(colour) = colour(value) else {
            return;
        };
        match key {
            "background" => self.background = Some(colour),
            "foreground" => self.foreground = Some(colour),
            "cursor" | "cursor-color" | "cursor_color" => self.cursor = Some(colour),
            "selection_background" | "selection-background" => self.selection = Some(colour),
            _ => {
                // `color0`…`color15`, or foot's `regular0`…`regular7` and `bright0`…`bright7`.
                let index = |prefix: &str, base: usize, count: usize| {
                    key.strip_prefix(prefix)
                        .and_then(|n| n.parse::<usize>().ok())
                        .filter(|i| *i < count)
                        .map(|i| base + i)
                };
                if let Some(i) = index("color", 0, 16)
                    .or_else(|| index("regular", 0, 8))
                    .or_else(|| index("bright", 8, 8))
                {
                    self.ansi[i] = Some(colour);
                }
            }
        }
    }
}

/// `#rrggbb` from `#rrggbb`, `rrggbb`, `#rgb` or `0xrrggbb`, lower case; None for anything else.
fn colour(value: &str) -> Option<String> {
    let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
    let hex = value
        .strip_prefix('#')
        .or_else(|| value.strip_prefix("0x"))
        .unwrap_or(value);
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        6 => Some(format!("#{}", hex.to_ascii_lowercase())),
        3 => Some(format!(
            "#{}",
            hex.chars()
                .flat_map(|c| [c, c])
                .collect::<String>()
                .to_ascii_lowercase()
        )),
        _ => None,
    }
}

/// kitty (`color4 #6272a4`) or Ghostty (`palette = 4=#6272a4`, `background = 282a36`).
fn lines(text: &str, ghostty: bool) -> Partial {
    let mut partial = Partial::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = if ghostty {
            match line.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => continue,
            }
        } else {
            match line.split_once(char::is_whitespace) {
                Some((k, v)) => (k.trim(), v.trim()),
                None => continue,
            }
        };
        if key == "palette" {
            if let Some((index, value)) = value.split_once('=') {
                partial.set(&format!("color{}", index.trim()), value);
            }
        } else {
            partial.set(key, value);
        }
    }
    partial
}

/// pywal and wallust: `{"special": {"background", "foreground", "cursor"}, "colors": {"color0"…}}`.
fn json(text: &str) -> Result<Partial, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let mut partial = Partial::default();
    for table in ["special", "colors"] {
        if let Some(entries) = value.get(table).and_then(|t| t.as_object()) {
            for (key, value) in entries {
                if let Some(value) = value.as_str() {
                    partial.set(key, value);
                }
            }
        }
    }
    Ok(partial)
}

/// Alacritty: `[colors.primary]`, `[colors.normal]` and `[colors.bright]` by colour name.
fn alacritty(text: &str) -> Result<Partial, String> {
    let value: toml::Value = toml::from_str(text).map_err(|e| format!("not valid TOML: {e}"))?;
    let colors = value
        .get("colors")
        .ok_or("no [colors] table: not an Alacritty theme")?;
    let get = |table: &str, key: &str| {
        colors
            .get(table)
            .and_then(|t| t.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let mut partial = Partial::default();
    let names = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    for (i, name) in names.iter().enumerate() {
        if let Some(c) = get("normal", name) {
            partial.set(&format!("color{i}"), &c);
        }
        if let Some(c) = get("bright", name) {
            partial.set(&format!("color{}", i + 8), &c);
        }
    }
    for (table, key, target) in [
        ("primary", "background", "background"),
        ("primary", "foreground", "foreground"),
        ("cursor", "cursor", "cursor"),
        ("selection", "background", "selection_background"),
    ] {
        if let Some(c) = get(table, key) {
            partial.set(target, &c);
        }
    }
    Ok(partial)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Dracula, as https://draculatheme.com ships it for each terminal.
    const ANSI: [&str; 16] = [
        "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
        "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
    ];

    fn check(palette: Palette) {
        assert_eq!(palette.background, "#282a36");
        assert_eq!(palette.foreground, "#f8f8f2");
        assert_eq!(palette.ansi, ANSI);
    }

    #[test]
    fn kitty() {
        let mut text = String::from(
            "# Dracula\nforeground #f8f8f2\nbackground #282a36\ncursor #f8f8f2\nselection_background #44475a\n",
        );
        for (i, c) in ANSI.iter().enumerate() {
            text.push_str(&format!("color{i}   {c}\n"));
        }
        let palette = parse(Path::new("dracula.conf"), &text).unwrap();
        assert_eq!(palette.selection.as_deref(), Some("#44475a"));
        check(palette);
    }

    #[test]
    fn ghostty() {
        let mut text =
            String::from("background = 282a36\nforeground = f8f8f2\ncursor-color = f8f8f2\n");
        for (i, c) in ANSI.iter().enumerate() {
            text.push_str(&format!("palette = {i}={c}\n"));
        }
        let palette = parse(Path::new("Dracula"), &text).unwrap();
        assert_eq!(palette.cursor.as_deref(), Some("#f8f8f2"));
        check(palette);
    }

    #[test]
    fn foot() {
        // foot.ini's [colors] section, as Dracula ships it and wallust's foot template writes it.
        let mut text = String::from(
            "[main]\nfont=JetBrains Mono:size=11\n\n[colors]\nalpha=1.0\nbackground=282a36\nforeground=f8f8f2\nselection-background=44475a\ncursor=282a36 f8f8f2\n",
        );
        for (i, c) in ANSI.iter().enumerate() {
            let key = if i < 8 {
                format!("regular{i}")
            } else {
                format!("bright{}", i - 8)
            };
            text.push_str(&format!("{key}={}\n", &c[1..]));
        }
        let palette = parse(Path::new("dracula.ini"), &text).unwrap();
        assert_eq!(palette.selection.as_deref(), Some("#44475a"));
        assert_eq!(
            palette.cursor, None,
            "foot's cursor is two colours: left to the app"
        );
        check(palette);
    }

    #[test]
    fn pywal_and_wallust() {
        let colors: serde_json::Map<String, serde_json::Value> = ANSI
            .iter()
            .enumerate()
            .map(|(i, c)| (format!("color{i}"), (*c).into()))
            .collect();
        let text = serde_json::json!({
            "wallpaper": "/home/b/wall.jpg",
            "special": {"background": "#282a36", "foreground": "#f8f8f2", "cursor": "#f8f8f2"},
            "colors": colors,
        })
        .to_string();
        check(parse(Path::new("colors.json"), &text).unwrap());
    }

    #[test]
    fn alacritty() {
        let names = [
            "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
        ];
        let table = |offset: usize| {
            names
                .iter()
                .enumerate()
                .map(|(i, n)| format!("{n} = \"{}\"\n", ANSI[i + offset]))
                .collect::<String>()
        };
        let text = format!(
            "[colors.primary]\nbackground = \"#282a36\"\nforeground = \"#f8f8f2\"\n[colors.normal]\n{}[colors.bright]\n{}",
            table(0),
            table(8)
        );
        check(parse(Path::new("dracula.toml"), &text).unwrap());
    }

    #[test]
    fn a_partial_or_foreign_file_says_what_is_missing() {
        let error = parse(
            Path::new("x.conf"),
            "background #000\nforeground #fff\ncolor0 #000\n",
        )
        .unwrap_err();
        assert!(
            error.contains("color1") && error.contains("16 colours"),
            "{error}"
        );
        assert!(
            parse(Path::new("x.json"), "{not json")
                .unwrap_err()
                .contains("JSON")
        );
        assert!(
            parse(Path::new("x.toml"), "a = 1")
                .unwrap_err()
                .contains("Alacritty")
        );
    }

    #[test]
    fn colours_are_normalised_and_anything_else_ignored() {
        assert_eq!(colour("#ABCDEF").as_deref(), Some("#abcdef"));
        assert_eq!(colour("0x282a36").as_deref(), Some("#282a36"));
        assert_eq!(colour("\"#fff\"").as_deref(), Some("#ffffff"));
        assert_eq!(colour("red"), None);
        assert_eq!(colour("#12345"), None);
        assert_eq!(colour("url(x)"), None);
    }
}
